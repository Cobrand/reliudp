use crate::rudp::*;
use std::net::{SocketAddr, UdpSocket, ToSocketAddrs};
use std::io::{ErrorKind as IoErrorKind, Result as IoResult};
use std::sync::Arc;
use crate::udp_packet::UdpPacket;
use std::time::Duration;

use std::collections::hash_map::Entry;
use fnv::{FnvHashMap as HashMap};
use crate::rudp::MessageType;
use std::ops::{Index, IndexMut};

#[derive(Debug)]
/// A Server that holds multiple remotes
///
/// It handles incoming connections automatically, expired connections (timeouts),
/// and obviously the ability to send/receive data and events to all remotes, either by handpicking
/// or all at the same time.
///
/// The `get_mut` method allows you to get mutably a socket to send a specific remote some data.
/// However, if you choose to not send everyone the same data, you **will** have to
/// keep track of the socket addresses of the remotes in one way or another.
pub struct RUdpServer {
    pub (crate) remotes: HashMap<SocketAddr, RUdpSocket>,
    pub (crate) udp_socket: Arc<UdpSocket>,
    pub (self) timeout_delay: Option<Duration>,
    pub (self) heartbeat_delay: Option<Duration>,
}

impl RUdpServer {
    /// Tries to create a new server with the binding address.
    ///
    /// It's often a good idea to have a value like "0.0.0.0:YOUR_PORT",
    /// to bind your address to the internet.
    pub fn new<A: ToSocketAddrs>(local_addr: A) -> IoResult<RUdpServer> {
        let udp_socket = Arc::new(UdpSocket::bind(local_addr)?);
        udp_socket.set_nonblocking(true)?;
        Ok(RUdpServer {
            remotes: HashMap::default(),
            udp_socket,
            timeout_delay: None,
            heartbeat_delay: None,
        })
    }

    fn update_timeout_delay_for_remotes(&mut self) {
        if let Some(delay) = self.timeout_delay {
            for socket in self.remotes.values_mut() {
                socket.set_timeout_delay(delay);
            }
        }
    }

    fn update_heartbeat_delay_for_remotes(&mut self) {
        if let Some(delay) = self.heartbeat_delay {
            for socket in self.remotes.values_mut() {
                socket.set_heartbeat_delay(delay);
            }
        }
    }

    /// Set the number of iterations required before a remote is set as "dead" for all past and all new remotes.
    /// 
    /// For instance, if your tick is every 50ms, and your timeout_delay is of 24,
    /// then roughly 50*24=1200ms (=1.2s) without a message from the remote will cause a timeout error.
    pub fn set_timeout_delay(&mut self, timeout_delay: Duration) {
        self.timeout_delay = Some(timeout_delay);
        self.update_timeout_delay_for_remotes();
    }

    /// Set the number of iterations required before we send a "heartbeat" message to the clients, so that they avoid seeing us as timeout-ed.
    ///
    /// This delay is applied to all existing and new clients
    pub fn set_heartbeat(&mut self, delay: Duration) {
        self.heartbeat_delay = Some(delay);
        self.update_heartbeat_delay_for_remotes();
    }

    fn process_one_incoming(&mut self, udp_packet: UdpPacket<Box<[u8]>>, remote_addr: SocketAddr) -> IoResult<()> {
        match self.remotes.entry(remote_addr) {
            Entry::Occupied(mut o) => {
                o.get_mut().add_received_packet(udp_packet)
            },
            Entry::Vacant(vacant) => {
                // buffer len is used for debug/log purposes
                match RUdpSocket::new_incoming(self.udp_socket.clone(), udp_packet, remote_addr) {
                    Err(RUdpCreateError::IoError(io_error)) => return Err(io_error),
                    Err(RUdpCreateError::UnexpectedData) => {
                        /* ignore unexpected data */
                        log::trace!("received unexpected UDP data from unknown remote {}", remote_addr);
                    },
                    Ok(mut rudp_socket) => {
                        if let Some(delay) = self.timeout_delay {
                            rudp_socket.set_timeout_delay(delay)
                        }
                        if let Some(heartbeat) = self.heartbeat_delay {
                            rudp_socket.set_heartbeat_delay(heartbeat)
                        }
                        vacant.insert(rudp_socket);
                    },
                };
            }
        };
        Ok(())
    }

    /// Returns a copy of the Arc holding the UdpSocket.
    pub fn udp_socket(&self) -> Arc<UdpSocket> {
        Arc::clone(&self.udp_socket)
    }

    pub (crate) fn process_all_incoming(&mut self) -> IoResult<()> {
        let mut done = false;

        while !done {
            match UdpPacket::<Box<[u8]>>::from_udp_socket(&self.udp_socket) {
                Ok((packet, remote_addr)) => {
                    self.process_one_incoming(packet, remote_addr)?;
                },
                Err(err) => {
                    match err.kind() {
                        IoErrorKind::WouldBlock => { done = true },
                        err_kind => {
                            panic!("received other unexpected net error {:?}", err_kind)
                        }
                    }
                },
            };
        };
        Ok(())
    }

    /// Send some data to ALL remotes
    pub fn send_data(&mut self, data: &Arc<[u8]>, message_type: MessageType) {
        for socket in self.remotes.values_mut() {
            socket.send_data(Arc::clone(data), message_type);
        }
    }

    #[inline]
    pub fn remotes_len(&self) -> usize {
        self.remotes.len()
    }

    /// Does internal processing for all remotes. Must be done before receiving events.
    pub fn next_tick(&mut self) -> IoResult<()> {
        self.remotes.retain(|_, v| {
            ! v.should_clear()
        });
        for socket in self.remotes.values_mut() {
            socket.update_cached_now();
        }
        self.process_all_incoming()?;
        for socket in self.remotes.values_mut() {
            socket.inner_tick()?;
        }
        Ok(())
    }

    pub fn iter(&self) -> impl Iterator<Item=(&SocketAddr, &RUdpSocket)> {
        self.remotes.iter()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item=(&SocketAddr, &mut RUdpSocket)> {
        self.remotes.iter_mut()
    }

    pub fn addresses(&self) -> impl Iterator<Item=&SocketAddr> {
        self.remotes.keys()
    }

    /// Get the socket stored for given the address
    pub fn get(&self, socket_addr: SocketAddr) -> Option<&RUdpSocket> {
        self.remotes.get(&socket_addr)
    }
    
    /// Get the mutable socket stored for given the address
    pub fn get_mut(&mut self, socket_addr: SocketAddr) -> Option<&mut RUdpSocket> {
        self.remotes.get_mut(&socket_addr)
    }

    /// Returns an iterator that drain events for all remotes.
    pub fn drain_events<'a>(&'a mut self) -> impl 'a + Iterator<Item=(SocketAddr, SocketEvent)> {
        self.remotes.iter_mut().flat_map(|(addr, socket)| {
            socket.drain_events().map(move |event| (*addr, event) )
        })
    }
}

impl Index<SocketAddr> for RUdpServer {
    type Output = RUdpSocket;

    fn index<'a>(&'a self, index: SocketAddr) -> &'a RUdpSocket {
        self.get(index).expect("socket_addr {} does not exist for this server instance")
    }
}

impl IndexMut<SocketAddr> for RUdpServer {
    fn index_mut<'a>(&'a mut self, index: SocketAddr) -> &'a mut RUdpSocket {
        self.get_mut(index).expect("socket_addr {} does not exist for this server instance")
    }
}