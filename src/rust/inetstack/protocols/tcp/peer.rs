// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//======================================================================================================================
// Imports
//======================================================================================================================

use crate::{
    demikernel::config::Config,
    inetstack::protocols::{
        arp::SharedArpPeer,
        ipv4::Ipv4Header,
        tcp::{
            isn_generator::IsnGenerator,
            segment::TcpHeader,
            socket::SharedTcpSocket,
            SeqNumber,
        },
    },
    runtime::{
        fail::Fail,
        memory::DemiBuffer,
        network::{
            config::TcpConfig,
            socket::{
                option::{
                    SocketOption,
                    TcpSocketOptions,
                },
                SocketId,
            },
            types::MacAddress,
            NetworkRuntime,
        },
        QDesc,
        SharedDemiRuntime,
        SharedObject,
    },
};
use ::futures::channel::mpsc;
use ::rand::{
    prelude::SmallRng,
    Rng,
    SeedableRng,
};

use ::std::{
    collections::HashMap,
    net::{
        Ipv4Addr,
        SocketAddr,
        SocketAddrV4,
    },
    ops::{
        Deref,
        DerefMut,
    },
};

#[cfg(feature = "tcp-migration")]
use state::TcpState;
#[cfg(feature = "tcp-migration")]
use crate::inetstack::protocols::tcpmig::TcpMigPeer;

use crate::{capy_log, capy_log_mig};


//======================================================================================================================
// Structures
//======================================================================================================================

pub struct TcpPeer<N: NetworkRuntime> {
    runtime: SharedDemiRuntime,
    isn_generator: IsnGenerator,
    transport: N,
    local_link_addr: MacAddress,
    local_ipv4_addr: Ipv4Addr,
    tcp_config: TcpConfig,
    default_socket_options: TcpSocketOptions,
    arp: SharedArpPeer<N>,
    rng: SmallRng,
    dead_socket_tx: mpsc::UnboundedSender<QDesc>,
    addresses: HashMap<SocketId, SharedTcpSocket<N>>,

    #[cfg(feature = "tcp-migration")]
    tcpmig: TcpMigPeer<N>,
}

#[derive(Clone)]
pub struct SharedTcpPeer<N: NetworkRuntime>(SharedObject<TcpPeer<N>>);

//======================================================================================================================
// Associated Functions
//======================================================================================================================

impl<N: NetworkRuntime> SharedTcpPeer<N> {
    pub fn new(
        config: &Config,
        runtime: SharedDemiRuntime,
        transport: N,
        arp: SharedArpPeer<N>,
        rng_seed: [u8; 32],
    ) -> Result<Self, Fail> {
        let mut rng: SmallRng = SmallRng::from_seed(rng_seed);
        let nonce: u32 = rng.gen();
        let (tx, _) = mpsc::unbounded();

        #[cfg(feature = "tcp-migration")]
        let tcpmig = TcpMigPeer::new(transport.clone(), config.local_link_addr()?, config.local_ipv4_addr()?, arp.clone());

        Ok(Self(SharedObject::<TcpPeer<N>>::new(TcpPeer {
            isn_generator: IsnGenerator::new(nonce),
            runtime,
            transport,
            local_link_addr: config.local_link_addr()?,
            local_ipv4_addr: config.local_ipv4_addr()?,
            tcp_config: TcpConfig::new(config)?,
            default_socket_options: TcpSocketOptions::new(config)?,
            arp,
            rng,
            dead_socket_tx: tx,
            addresses: HashMap::<SocketId, SharedTcpSocket<N>>::new(),
            #[cfg(feature = "tcp-migration")]
            tcpmig,
        })))
    }

    /// Creates a TCP socket.
    pub fn socket(&mut self) -> Result<SharedTcpSocket<N>, Fail> {
        Ok(SharedTcpSocket::<N>::new(
            self.runtime.clone(),
            self.transport.clone(),
            self.local_link_addr,
            self.tcp_config.clone(),
            self.default_socket_options.clone(),
            self.arp.clone(),
            self.dead_socket_tx.clone(),
        ))
    }

    /// Sets an option on a TCP socket.
    pub fn set_socket_option(&mut self, socket: &mut SharedTcpSocket<N>, option: SocketOption) -> Result<(), Fail> {
        socket.set_socket_option(option)
    }

    /// Sets an option on a TCP socket.
    pub fn get_socket_option(
        &mut self,
        socket: &mut SharedTcpSocket<N>,
        option: SocketOption,
    ) -> Result<SocketOption, Fail> {
        socket.get_socket_option(option)
    }

    /// Gets a peer address on a TCP socket.
    pub fn getpeername(&mut self, socket: &mut SharedTcpSocket<N>) -> Result<SocketAddrV4, Fail> {
        socket.getpeername()
    }

    /// Binds a socket to a local address supplied by [local].
    pub fn bind(&mut self, socket: &mut SharedTcpSocket<N>, local: SocketAddrV4) -> Result<(), Fail> {
        // All other checks should have been done already.
        debug_assert!(!Ipv4Addr::is_unspecified(local.ip()));
        debug_assert!(local.port() != 0);
        debug_assert!(self.addresses.get(&SocketId::Passive(local)).is_none());

        // Issue operation.
        socket.bind(local)?;
        capy_log!("[BIND]");
        self.addresses.insert(SocketId::Passive(local), socket.clone());
        capy_log!("addresses: {:#?}", self.addresses);
        Ok(())
    }

    // Marks the target socket as passive.
    pub fn listen(&mut self, socket: &mut SharedTcpSocket<N>, backlog: usize) -> Result<(), Fail> {
        // Most checks should have been performed already
        debug_assert!(socket.local().is_some());
        let nonce: u32 = self.rng.gen();
        socket.listen(backlog, nonce)
    }

    /// Runs until a new connection is accepted.
    pub async fn accept(&mut self, socket: &mut SharedTcpSocket<N>) -> Result<SharedTcpSocket<N>, Fail> {
        // Wait for accept to complete.
        match socket.accept().await {
            Ok(socket) => {
                capy_log!("Insert Active socket ({}, {})", socket.local().unwrap(), socket.remote().unwrap());
                self.addresses.insert(
                    SocketId::Active(socket.local().unwrap(), socket.remote().unwrap()),
                    socket.clone(),
                );
                capy_log!("addresses: {:#?}", self.addresses);
                Ok(socket)
            },
            Err(e) => Err(e),
        }
    }

    /// Runs until the connect to remote is made or times out.
    pub async fn connect(&mut self, socket: &mut SharedTcpSocket<N>, remote: SocketAddrV4) -> Result<(), Fail> {
        // Check whether we need to allocate an ephemeral port.
        let local: SocketAddrV4 = match socket.local() {
            Some(addr) => {
                // If socket is already bound to a local address, use it but remove the old binding.
                self.addresses.remove(&SocketId::Passive(addr));
                addr
            },
            None => {
                let local_port: u16 = self.runtime.alloc_ephemeral_port()?;
                SocketAddrV4::new(self.local_ipv4_addr, local_port)
            },
        };
        // Insert the connection to receive incoming packets for this address pair.
        // Should we remove the passive entry for the local address if the socket was previously bound?
        if self
            .addresses
            .insert(SocketId::Active(local, remote.clone()), socket.clone())
            .is_some()
        {
            // We should panic here because the ephemeral port allocator should not allocate the same port more than
            // once.
            unreachable!(
                "There is already a socket listening on this address: {:?} {:?}",
                local, remote
            );
        }
        let local_isn: SeqNumber = self.isn_generator.generate(&local, &remote);
        // Wait for connect to complete.
        if let Err(e) = socket.connect(local, remote, local_isn).await {
            self.addresses.remove(&SocketId::Active(local, remote.clone()));
            Err(e)
        } else {
            Ok(())
        }
    }

    /// Pushes immediately to the socket and returns the result asynchronously.
    pub async fn push(&self, socket: &mut SharedTcpSocket<N>, buf: &mut DemiBuffer) -> Result<(), Fail> {
        // TODO: Remove this copy after merging with the transport trait.
        // Wait for push to complete.
        socket.push(buf.clone()).await?;
        buf.trim(buf.len())
    }

    /// Sets up a coroutine for popping data from the socket.
    pub async fn pop(
        &self,
        socket: &mut SharedTcpSocket<N>,
        size: usize,
    ) -> Result<(Option<SocketAddr>, DemiBuffer), Fail> {
        // Grab the queue, make sure it hasn't been closed in the meantime.
        // This will bump the Rc refcount so the coroutine can have it's own reference to the shared queue data
        // structure and the SharedTcpQueue will not be freed until this coroutine finishes.
        let incoming: DemiBuffer = socket.pop(Some(size)).await?;
        Ok((None, incoming))
    }

    /// Frees an ephemeral port (if any) allocated to a given socket.
    fn free_ephemeral_port(&mut self, socket_id: &SocketId) {
        let local: &SocketAddrV4 = match socket_id {
            SocketId::Active(local, _) => local,
            SocketId::Passive(local) => local,
        };
        // Rollback ephemeral port allocation.
        if SharedDemiRuntime::is_private_ephemeral_port(local.port()) {
            if self.runtime.free_ephemeral_port(local.port()).is_err() {
                // We fail if and only if we attempted to free a port that was not allocated.
                // This is unexpected, but if it happens, issue a warning and keep going,
                // otherwise we would leave the queue in a dangling state.
                warn!("bind(): leaking ephemeral port (port={})", local.port());
            }
        }
    }

    /// Closes a TCP socket.
    pub async fn close(&mut self, socket: &mut SharedTcpSocket<N>) -> Result<(), Fail> {
        // Wait for close to complete.
        // Handle result: If unsuccessful, free the new queue descriptor.
        if let Some(socket_id) = socket.close().await? {
            self.addresses.remove(&socket_id);
            self.free_ephemeral_port(&socket_id);
        }
        Ok(())
    }

    pub fn hard_close(&mut self, socket: &mut SharedTcpSocket<N>) -> Result<(), Fail> {
        if let Some(socket_id) = socket.hard_close()? {
            self.addresses.remove(&socket_id);
            self.free_ephemeral_port(&socket_id);
        }
        Ok(())
    }

    /// Processes an incoming TCP segment.
    pub fn receive(&mut self, ip_hdr: Ipv4Header, buf: DemiBuffer) {
        
        let (tcp_hdr, data): (TcpHeader, DemiBuffer) =
            match TcpHeader::parse(&ip_hdr, buf, self.tcp_config.get_rx_checksum_offload()) {
                Ok(result) => result,
                Err(e) => {
                    let cause: String = format!("invalid tcp header: {:?}", e);
                    error!("receive(): {}", &cause);
                    return;
                },
            };
        debug!("TCP received {:?}", tcp_hdr);
        let local: SocketAddrV4 = SocketAddrV4::new(ip_hdr.get_dest_addr(), tcp_hdr.dst_port);
        let remote: SocketAddrV4 = SocketAddrV4::new(ip_hdr.get_src_addr(), tcp_hdr.src_port);
        
        if remote.ip().is_broadcast() || remote.ip().is_multicast() || remote.ip().is_unspecified() {
            let cause: String = format!("invalid remote address (remote={})", remote.ip());
            error!("receive(): {}", &cause);
            return;
        }
        #[cfg(feature = "tcp-migration")]
        let should_migrate = self.tcpmig.should_migrate();

        // Retrieve the queue descriptor based on the incoming segment.
        let socket: &mut SharedTcpSocket<N> = match self.addresses.get_mut(&SocketId::Active(local, remote)) {
            Some(socket) => socket,
            None => {

                #[cfg(feature = "tcp-migration")]
                // Check if migrating queue exists. If yes, push buffer to queue and return, else continue normally.
                match self.tcpmig.try_buffer_packet(remote, tcp_hdr.clone(), data.clone()) {
                    Ok(()) => return,
                    Err(val) => {},
                };

                match self.addresses.get_mut(&SocketId::Passive(local)) {
                    Some(socket) => socket,
                    None => {
                        let cause: String = format!("no queue descriptor for remote address (remote={})", remote.ip());
                        error!("receive(): {}", &cause);
                        return;
                    },
                }
            },

        };
        capy_log!("TCP msg {} to socket {:#?}", tcp_hdr.seq_num, socket);
        // Dispatch to further processing depending on the socket state.
        socket.receive(ip_hdr, tcp_hdr, data);


        #[cfg(feature = "tcp-migration")]
        if should_migrate {
            // Clone the socket and initiate migration
            let socket_clone = socket.clone();
            self.tcpmig.initiate_migration(socket_clone);

        }
    }
}

//======================================================================================================================
// Trait Implementations
//======================================================================================================================

impl<N: NetworkRuntime> Deref for SharedTcpPeer<N> {
    type Target = TcpPeer<N>;

    fn deref(&self) -> &Self::Target {
        self.0.deref()
    }
}

impl<N: NetworkRuntime> DerefMut for SharedTcpPeer<N> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.deref_mut()
    }
}


//==========================================================================================================================
// TCP Migration
//==========================================================================================================================

//==============================================================================
//  Implementations
//==============================================================================

#[cfg(feature = "tcp-migration")]
impl<N: NetworkRuntime> SharedTcpPeer<N> {
    pub fn receive_tcpmig(&mut self, ip_hdr: &Ipv4Header, buf: DemiBuffer) -> Result<(), Fail> {
        use super::super::tcpmig::TcpmigReceiveStatus;
        match self.tcpmig.receive(ip_hdr, buf)? {
            TcpmigReceiveStatus::Ok | TcpmigReceiveStatus::SentReject => {},

            TcpmigReceiveStatus::Rejected(local, remote) => {
                capy_log_mig!("MIGRATION REJECTED");
            },

            TcpmigReceiveStatus::ReturnedBySwitch(local, remote) => {
                // #[cfg(not(feature = "manual-tcp-migration"))]
                // match self.established.get(&(local, remote)) {
                //     Some(s) => s.cb.enable_stats(&mut self.recv_queue_stats, &mut self.rps_stats),
                //     None => panic!("migration rejected for non-existent connection: {:?}", (local, remote)),
                // }
    
                // // Re-initiate another migration if manual migration returned by switch.
                // #[cfg(feature = "manual-tcp-migration")]
                // self.initiate_migration_by_addr((local, remote));
                panic!("PREPARE_MIG is returned by the switch");
                
            },

            TcpmigReceiveStatus::PrepareMigrationAcked(local, remote) => {
                let mut socket: SharedTcpSocket<N> = match self.addresses.remove(&SocketId::Active(local, remote)) {
                    Some(socket) => socket,
                    None => panic!("PrepareMigrationAcked for non-existing socket: {:?}", (local, remote)),
                };
                let state = socket.get_tcp_state()?;
                // let state = socket.get_tcp_state();
                capy_log_mig!("PrepareMigrationAcked for {:#?}", socket);
                capy_log_mig!("TcpState: {:#?}", state.cb);
                
                self.tcpmig.send_tcp_state(state);
            },
            TcpmigReceiveStatus::StateReceived(state) => {
                self.migrate_in_connection(state);
            },
        }
        Ok(())
    }

    fn migrate_in_connection(&mut self, state: TcpState) -> Result<(), Fail> {
        // Create EstablishedSocket
        // 1. take the passive socket using the local address
        let local = state.cb.local();
        let socket: &mut SharedTcpSocket<N> = match self.addresses.get_mut(&SocketId::Passive(local)) {
            Some(socket) => socket,
            None => panic!("Passive socket binding {:?} doesn't exist.", state.cb.local()),
        };
        socket.migrate_in_connection(state);
        Ok(())
    }
}


//==============================================================================
// TCP State
//==============================================================================
#[cfg(feature = "tcp-migration")]
pub mod state {
    use std::{net::SocketAddrV4};

    use crate::{
        capy_log_mig, capy_profile, 
        inetstack::protocols::{
            tcp::established::ControlBlockState, 
        },
        runtime::memory::DemiBuffer,
    };

    pub trait Serialize {
        /// Serializes into the buffer and returns its unused part.
        fn serialize_into<'buf>(&self, buf: &'buf mut [u8]) -> &'buf mut [u8];
    }

    pub trait Deserialize: Sized {
        /// Deserializes and removes the deserialised part from the buffer.
        fn deserialize_from(buf: &mut DemiBuffer) -> Self;
    }

    pub struct TcpState {
        pub cb: ControlBlockState,
    }
    

    impl TcpState {
        pub fn new(cb: ControlBlockState) -> Self {
            Self { cb }
        }

        pub fn remote(&self) -> SocketAddrV4 {
            self.cb.remote()
        }

        pub fn serialize(&self) -> DemiBuffer {
            // capy_profile!("PROF_SERIALIZE");
            let mut buf = DemiBuffer::new(self.serialized_size() as u16);
            let remaining = self.cb.serialize_into(&mut buf);
            buf
        }

        fn serialized_size(&self) -> usize {
            self.cb.serialized_size()
        }

        pub fn deserialize(mut buf: DemiBuffer) -> Self {
            // capy_profile!("PROF_DESERIALIZE");
            let cb: ControlBlockState = ControlBlockState::deserialize_from(&mut buf);
            Self { cb }
        }

        pub fn set_local(&mut self, local: SocketAddrV4) {
            self.cb.set_local(local)
        }

        pub fn connection(&self) -> (SocketAddrV4, SocketAddrV4) {
            self.cb.endpoints()
        }
    }
}