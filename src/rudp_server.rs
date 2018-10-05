use rudp::*;
use std::net::{SocketAddr, UdpSocket, ToSocketAddrs};
use std::io::{ErrorKind as IoErrorKind, Result as IoResult};
use std::sync::Arc;
use udp_packet::UdpPacket;

use std::collections::hash_map::Entry;
use fnv::{FnvHashMap as HashMap};
use rudp::MessageType;
use std::ops::{Index, IndexMut};

#[derive(Debug)]
pub struct RUdpServer {
    pub (crate) remotes: HashMap<SocketAddr, RUdpSocket>,
    pub (crate) udp_socket: Arc<UdpSocket>,
}

impl RUdpServer {
    pub fn new<A: ToSocketAddrs>(local_addr: A) -> IoResult<RUdpServer> {
        let udp_socket = Arc::new(UdpSocket::bind(local_addr)?);
        udp_socket.set_nonblocking(true)?;
        Ok(RUdpServer {
            remotes: HashMap::default(),
            udp_socket,
        })
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
                    },
                    Ok(rudp_socket) => {
                        vacant.insert(rudp_socket);
                    },
                };
            }
        };
        Ok(())
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
        for mut socket in self.remotes.values_mut() {
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
            ! v.socket.status().is_finished()
        });
        for mut socket in self.remotes.values_mut() {
            socket.incr_tick();
        }
        self.process_all_incoming()?;
        for mut socket in self.remotes.values_mut() {
            socket.inner_tick()?;
        }
        Ok(())
    }

    pub fn iter<'a>(&'a self) -> impl Iterator<Item=(&'a SocketAddr, &'a RUdpSocket)> {
        self.remotes.iter()
    }

    pub fn iter_mut<'a>(&'a mut self) -> impl Iterator<Item=(&'a SocketAddr, &'a mut RUdpSocket)> {
        self.remotes.iter_mut()
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