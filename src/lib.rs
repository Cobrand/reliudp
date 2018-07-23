extern crate fnv;

extern crate itertools;

extern crate crc;
extern crate byteorder;

/// TODO: reorganize stuff.
/// Stuff is working, but it's really not well oragnized at all. A refactor will be needed
/// (at least name-wise, but also to define precisely which module has which limits and which role)

mod misc;
mod consts;
mod fragment_combiner;
mod fragment_generator;
mod fragment;
pub mod udp_packet;
mod rudp;
mod udp_packet_handler;
mod rudp_server;
mod ack;
mod sent_data_tracker;

pub use rudp::*;
pub use rudp_server::*;