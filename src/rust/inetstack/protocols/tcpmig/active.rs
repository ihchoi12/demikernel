// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//==============================================================================
// Imports
//==============================================================================

use super::{
    segment::{
        TcpMigHeader,
        TcpMigSegment,
        TcpMigDefragmenter,
        MigrationStage,
    },
    TcpmigReceiveStatus
};
use crate::{
    capy_profile, capy_profile_merge_previous, capy_time_log, 
    inetstack::protocols::{
        ethernet2::{
            EtherType2,
            Ethernet2Header,
        },
        ip::IpProtocol,
        ipv4::Ipv4Header,
        tcp::{
            socket::SharedTcpSocket,
            segment::TcpHeader,
        },
        tcpmig::segment::MAX_FRAGMENT_SIZE,
    }, 
    runtime::{
        network::{
            NetworkRuntime,
        },
        SharedDemiRuntime,
        memory::DemiBuffer,
        network::{
            types::MacAddress,
        },
    }, 
    QDesc,
};

use crate::{capy_log, capy_log_mig};

use ::std::{
    net::{
        Ipv4Addr,
        SocketAddrV4,
    },
    rc::Rc,
};

//======================================================================================================================
// Structures
//======================================================================================================================

pub struct ActiveMigration<N: NetworkRuntime> {
    transport: N,

    local_ipv4_addr: Ipv4Addr,
    local_link_addr: MacAddress,
    remote_ipv4_addr: Ipv4Addr,
    remote_link_addr: MacAddress,
    self_udp_port: u16,
    dest_udp_port: u16,

    origin: SocketAddrV4,
    client: SocketAddrV4,

    last_sent_stage: MigrationStage,

    /// QDesc representing the connection, only on the origin side.
    socket: SharedTcpSocket<N>,

    recv_queue: Vec<(TcpHeader, DemiBuffer)>,

    defragmenter: TcpMigDefragmenter,
}

//======================================================================================================================
// Associate Functions
//======================================================================================================================

impl<N: NetworkRuntime> ActiveMigration<N> {
    pub fn new(
        transport: N,
        local_ipv4_addr: Ipv4Addr,
        local_link_addr: MacAddress,
        remote_ipv4_addr: Ipv4Addr,
        remote_link_addr: MacAddress,
        self_udp_port: u16,
        dest_udp_port: u16,
        origin: SocketAddrV4,
        client: SocketAddrV4,
        socket: SharedTcpSocket<N>,
    ) -> Self {
        Self {
            transport,
            local_ipv4_addr,
            local_link_addr,
            remote_ipv4_addr,
            remote_link_addr,
            self_udp_port,
            dest_udp_port, 
            origin,
            client,
            last_sent_stage: MigrationStage::None,
            socket,
            recv_queue: Vec::new(),
            defragmenter: TcpMigDefragmenter::new(),
        }
    }

    pub fn initiate_migration(&mut self) {
        assert_eq!(self.last_sent_stage, MigrationStage::None);

        let tcpmig_hdr = TcpMigHeader::new(
            self.origin,
            self.client, 
            0, 
            MigrationStage::PrepareMigration, 
            self.self_udp_port, 
            if self.self_udp_port == 10001 { 10000 } else { 10001 }
        );
        self.last_sent_stage = MigrationStage::PrepareMigration;
        eprintln!("active - initiate_migration");
        capy_log!("\n\n******* START MIGRATION *******\n[TX] PREPARE_MIG ({}, {})", self.origin, self.client);
        capy_time_log!("SEND_PREPARE_MIG,({})", self.client);
        self.send(tcpmig_hdr, DemiBuffer::new(0));
    }

    /// Sends a TCPMig segment from local to remote.
    fn send(
        &mut self,
        tcpmig_hdr: TcpMigHeader,
        buf: DemiBuffer,
    ) {
        debug!("TCPMig send {:?}", tcpmig_hdr);
        // eprintln!("TCPMig sent: {:#?}\nto {:?}:{:?}", tcpmig_hdr, self.remote_link_addr, self.remote_ipv4_addr);
        
        // Layer 4 protocol field marked as UDP because DPDK only supports standard Layer 4 protocols.
        let ip_hdr = Ipv4Header::new(self.local_ipv4_addr, self.remote_ipv4_addr, IpProtocol::UDP);

        if buf.len() / MAX_FRAGMENT_SIZE > u16::MAX as usize {
            panic!("TcpState too large")
        }
        let segment = TcpMigSegment::new(
            Ethernet2Header::new(self.remote_link_addr, self.local_link_addr, EtherType2::Ipv4),
            ip_hdr,
            tcpmig_hdr,
            buf,
        );
        for fragment in segment.fragments() {
            self.transport.transmit(Box::new(fragment));
        }
    }
}

//======================================================================================================================
// Functions
//======================================================================================================================
