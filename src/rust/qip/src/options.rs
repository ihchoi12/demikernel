use crate::protocols::arp;
use crate::rand::Seed;
use eui48::MacAddress;
use std::net::Ipv4Addr;

#[derive(Clone)]
pub struct Options {
    pub my_link_addr: MacAddress,
    pub my_ipv4_addr: Ipv4Addr,
    pub rng_seed: Seed,
    pub arp: arp::Options,
}
