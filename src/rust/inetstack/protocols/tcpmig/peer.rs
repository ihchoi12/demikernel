// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//==============================================================================
// Imports
//==============================================================================

use super::{
    constants::*, 
    segment::{TcpMigSegment, TcpMigHeader},
    active::ActiveMigration,
};
use crate::{
    inetstack::protocols::{
        arp::SharedArpPeer,
        ipv4::Ipv4Header, 
        tcp::{
            socket::SharedTcpSocket,
            peer::state::TcpState,
        },
        tcpmig::segment::MigrationStage,
        ethernet2::{EtherType2, Ethernet2Header},
        ip::IpProtocol,
        // udp::{datagram::UdpDatagram, UdpHeader},
    },
    runtime::{
        fail::Fail,
        memory::DemiBuffer,
        network::{
            types::MacAddress,
            NetworkRuntime,
        },
        SharedDemiRuntime,
    },
    QDesc,
    capy_profile, capy_profile_merge_previous, capy_time_log,
};

use std::cell::RefCell;
use std::collections::hash_map::Entry;
use std::time::Instant;
use ::std::{
    collections::HashMap,
    net::{
        Ipv4Addr,
        SocketAddrV4,
    },
    thread,
    rc::Rc,
    env,
};

#[cfg(feature = "profiler")]
use crate::timer;

use crate::capy_log_mig;

//======================================================================================================================
// Structures
//======================================================================================================================

pub enum TcpmigReceiveStatus {
    Ok,
    SentReject,
    Rejected(SocketAddrV4, SocketAddrV4),
    ReturnedBySwitch(SocketAddrV4, SocketAddrV4),
    PrepareMigrationAcked(SocketAddrV4, SocketAddrV4),
    StateReceived(TcpState),
    // MigrationCompleted,

    // Heartbeat protocol.
    // HeartbeatResponse(usize),
}

/// TCPMig Peer
pub struct TcpMigPeer<N: NetworkRuntime> {
    /// Underlying runtime.
    transport: N,
    
    /// Local link address.
    local_link_addr: MacAddress,
    /// Local IPv4 address.
    local_ipv4_addr: Ipv4Addr,

    /// Connections being actively migrated in/out.
    /// 
    /// key = remote.
    active_migrations: HashMap<SocketAddrV4, ActiveMigration<N>>,

    self_udp_port: u16,

    // heartbeat_message: Box<TcpMigSegment>,
    
    /// for testing
    additional_mig_delay: u32,

    arp: SharedArpPeer<N>,
}


//======================================================================================================================
// Associate Functions
//======================================================================================================================

/// Associate functions for [TcpMigPeer].
impl<N: NetworkRuntime> TcpMigPeer<N> {
    /// Creates a TCPMig peer.
    pub fn new(
        transport: N,
        local_link_addr: MacAddress,
        local_ipv4_addr: Ipv4Addr,
        arp: SharedArpPeer<N>,
    ) -> Self {
        // log_init();

        Self {
            transport: transport,
            local_link_addr,
            local_ipv4_addr,
            active_migrations: HashMap::new(),
            self_udp_port: SELF_UDP_PORT, // TEMP

            // heartbeat_message: Box::new(TcpMigSegment::new(
            //     Ethernet2Header::new(FRONTEND_MAC, local_link_addr, EtherType2::Ipv4),
            //     Ipv4Header::new(local_ipv4_addr, FRONTEND_IP, IpProtocol::UDP),
            //     TcpMigHeader::new(
            //         SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0),
            //         SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0),
            //         4, 
            //         MigrationStage::HeartbeatUpdate,
            //         SELF_UDP_PORT, 
            //         FRONTEND_PORT
            //     ),
            //     DemiBuffer::new(4),
            // )),

            // for testing
            additional_mig_delay: env::var("MIG_DELAY")
            .unwrap_or_else(|_| String::from("0")) // Default value is 0 if MIG_DELAY is not set
            .parse::<u32>()
            .expect("Invalid DELAY value"),

            arp,
        }
    }

    pub fn should_migrate(&self) -> bool {
        if self.additional_mig_delay != 0 {
            return false;
        }
        
        static mut FLAG: i32 = 0;
        
        unsafe {
            // if FLAG == 5 {
            //     FLAG = 0;
            // }
            FLAG += 1;
            // eprintln!("FLAG: {}", FLAG);
            FLAG == 5
        }
    }

    pub fn initiate_migration(&mut self, socket: SharedTcpSocket<N>)
    where
        N: NetworkRuntime, 
    {
        // capy_profile!("additional_delay");
        let (local, remote) = (socket.local().unwrap(), socket.remote().unwrap());
        eprintln!("initiate_migration ({}, {})", local, remote);


        let active = ActiveMigration::new(
            self.transport.clone(),
            self.local_ipv4_addr,
            self.local_link_addr,
            TARGET_IP,
            TARGET_MAC, 
            self.self_udp_port,
            TARGET_PORT, // dest_udp_port is unknown until it receives PREPARE_MIGRATION_ACK, so it's 0 initially.
            local,
            remote,
            Some(socket),
        );

        let active = match self.active_migrations.entry(remote) {
            Entry::Occupied(..) => panic!("duplicate initiate migration"),
            Entry::Vacant(entry) => entry.insert(active),
        };
        active.initiate_migration();
    }


    pub fn receive(&mut self, ipv4_hdr: &Ipv4Header, buf: DemiBuffer) -> Result<TcpmigReceiveStatus, Fail> {
        // Parse header.
        let (hdr, buf) = TcpMigHeader::parse(ipv4_hdr, buf)?;
        capy_log_mig!("\n\n[RX] TCPMig");
        let remote = hdr.client;
        
        // First packet that target receives.
        if hdr.stage == MigrationStage::PrepareMigration {
            // capy_profile!("prepare_ack");

            capy_log_mig!("******* MIGRATION REQUESTED *******");
            capy_log_mig!("PREPARE_MIG {}", remote);
            let target = SocketAddrV4::new(self.local_ipv4_addr, self.self_udp_port);
            capy_log_mig!("I'm target {}", target);

            capy_time_log!("RECV_PREPARE_MIG,({})", remote);

            let active = ActiveMigration::new(
                self.transport.clone(),
                self.local_ipv4_addr,
                self.local_link_addr,
                *hdr.origin.ip(),
                self.arp.query_cache(*hdr.origin.ip())?, // Need to go through the switch 
                self.self_udp_port,
                hdr.origin.port(), 
                hdr.origin,
                hdr.client,
                None,
            );

            if let Some(..) = self.active_migrations.insert(remote, active) {
                // It happens when a backend send PREPARE_MIGRATION to the switch
                // but it receives back the message again (i.e., this is the current minimum workload backend)
                // In this case, remove the active migration.
                capy_log_mig!("It returned back to itself, maybe it's the current-min-workload server");
                self.active_migrations.remove(&remote); 
                return Ok(TcpmigReceiveStatus::ReturnedBySwitch(hdr.origin, hdr.client));
            }
        }
        let mut entry = match self.active_migrations.entry(remote) {
            Entry::Vacant(..) => panic!("no such active migration: {:#?}", hdr),
            Entry::Occupied(entry) => entry,
        };
        let active = entry.get_mut();

        capy_log_mig!("Active migration {:?}", remote);
        let mut status = active.process_packet(ipv4_hdr, hdr, buf)?;

        match status {
            TcpmigReceiveStatus::PrepareMigrationAcked(..) => (),
            TcpmigReceiveStatus::StateReceived(ref mut state) => {

            },
            TcpmigReceiveStatus::Rejected(..) | TcpmigReceiveStatus::SentReject => {
                // Remove active migration.
                entry.remove();
                capy_log_mig!("Removed rejected active migration: {remote}");
            },
            TcpmigReceiveStatus::Ok => (),
            TcpmigReceiveStatus::ReturnedBySwitch(..) => panic!("ReturnedBySwitch returned by active migration"),
        }

        // if hdr.stage == MigrationStage::PrepareMigrationAck {
        //     capy_log_mig!("RECV_PREPARE_ACK");
        // }
        Ok(status)
    }

    pub fn send_tcp_state(&mut self, mut state: TcpState) {
        let remote = state.remote();


        let active = self.active_migrations.get_mut(&remote).unwrap();
        capy_log_mig!("tcpmig::send_tcp_state()");
        active.send_connection_state(state);

    }
}

/*************************************************************/
/* LOGGING QUEUE LENGTH */
/*************************************************************/

// static mut LOG: Option<Vec<usize>> = None;
// const GRANULARITY: i32 = 1; // Logs length after every GRANULARITY packets.

// fn log_init() {
//     unsafe { LOG = Some(Vec::with_capacity(1024*1024)); }
// }

// fn log_len(len: usize) {
//     static mut GRANULARITY_FLAG: i32 = GRANULARITY;

//     unsafe {
//         GRANULARITY_FLAG -= 1;
//         if GRANULARITY_FLAG > 0 {
//             return;
//         }
//         GRANULARITY_FLAG = GRANULARITY;
//     }
    
//     unsafe { LOG.as_mut().unwrap_unchecked() }.push(len);
// }

// pub fn log_print() {
//     unsafe { LOG.as_ref().unwrap_unchecked() }.iter().for_each(|len| println!("{}", len));
// }