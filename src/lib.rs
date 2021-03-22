//! Reliudp: A custom Reliable UDP protocol for Rust
//!
//! # Examples
//! 
//! ## Server
//!
//! ```rust,no_run
//! extern crate reliudp;
//! use std::sync::Arc;
//! 
//! fn generate_really_big_message(i: u8) -> Arc<[u8]> {
//!     let really_big_message: Vec<u8> = (0..2000).map(|_v| i).collect();
//!     let really_big_message: Arc<[u8]> = Arc::from(really_big_message.into_boxed_slice());
//!     really_big_message
//! }
//! 
//! fn main() -> Result<(), Box<::std::error::Error>> {
//!     let mut server = reliudp::RUdpServer::new("0.0.0.0:61244").expect("Failed to create server");
//! 
//!     let mut n = 0;
//!     for i in 0u64.. {
//!         server.next_tick()?;
//!         for server_event in server.drain_events() {
//!             println!("Server: Incoming event {:?}", server_event);
//!         }
//! 
//!         if i % 300 == 0 {
//!             let big_message = generate_really_big_message(n);
//!             println!("Sending (n={:?}) {:?} bytes to all {:?} remotes", n, big_message.as_ref().len(), server.remotes_len());
//!             if n % 2 == 0 {
//!                 server.send_data(&big_message, reliudp::MessageType::KeyMessage, Default::default());
//!             } else {
//!                 server.send_data(&big_message, reliudp::MessageType::Forgettable, Default::default());
//!             }
//!             n += 1;
//!         }
//!         
//!         ::std::thread::sleep(::std::time::Duration::from_millis(5));
//!     }
//!     Ok(())
//! }
//! ```
//!
//! ## Client
//!
//! ```rust,no_run
//! extern crate reliudp;
//! use reliudp::SocketEvent;
//! 
//! fn main() -> Result<(), Box<::std::error::Error>> {
//!     let mut client = reliudp::RUdpSocket::connect("127.0.0.1:61244").expect("Failed to create client");
//!     for i in 0.. {
//!         client.next_tick()?;
//!         for client_event in client.drain_events() {
//!             if let SocketEvent::Data(d) = client_event {
//!                 println!("Client: Incoming {:?} bytes (n={:?}) at frame {:?}", d.len(), d[0], i);
//!             } else {
//!                 println!("Client: Incoming event {:?} at frame {:?}", client_event, i);
//!             }
//!         }
//! 
//!         ::std::thread::sleep(::std::time::Duration::from_millis(5));
//!     }
//!     Ok(())
//! }
//! ```

/// TODO: reorganize stuff.
/// Stuff is working, but it's really not well organized at all. A refactor will be needed
/// (at least name-wise, but also to define precisely which module has which limits and which role)

mod misc;
mod consts;
mod fragment_combiner;
mod fragment_generator;
mod fragment;
mod udp_packet;
mod rudp;
mod udp_packet_handler;
mod rudp_server;
mod ack;
mod sent_data_tracker;
mod ping_handler;

pub use rudp::*;
pub use rudp_server::*;