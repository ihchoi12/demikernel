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
        tcp::{
            socket::SharedTcpSocket,
            segment::TcpHeader,
        },
    }, 
    runtime::{
        fail::Fail,
        memory::DemiBuffer,
        network::{
            types::MacAddress,
        },
    }, 
    QDesc,
    runtime::{
        network::{
            NetworkRuntime,
        },
        SharedDemiRuntime,
    },
};

use crate::capy_log_mig;

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
    runtime: SharedDemiRuntime,

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
        runtime: SharedDemiRuntime,
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
            runtime,
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
        capy_log_mig!("\n\n******* START MIGRATION *******\n[TX] PREPARE_MIG ({}, {})", self.origin, self.client);
        capy_time_log!("SEND_PREPARE_MIG,({})", self.client);
        // self.send(tcpmig_hdr, Buffer::Heap(DataBuffer::empty()));
    }

}

//======================================================================================================================
// Functions
//======================================================================================================================
