use std::net::UdpSocket;
use udp_packet_handler::{UdpPacketHandler, ReceivedMessage};
use udp_packet::{UdpPacket, Packet};
use std::net::{SocketAddr, ToSocketAddrs};
use std::io::{Error as IoError, ErrorKind as IoErrorKind, Result as IoResult};
use std::sync::Arc;
use ack::Ack;
use sent_data_tracker::SentDataTracker;
use std::collections::VecDeque;

pub enum SocketEvent {
    Data(Box<[u8]>),
    Connected,
    Aborted,
    Ended,
    Timeout,
}

impl ::std::fmt::Debug for SocketEvent {
    fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        match self {
            SocketEvent::Data(d) => write!(f, "Data({:?} bytes)", d.len()),
            SocketEvent::Connected => write!(f, "Connected"),
            SocketEvent::Aborted => write!(f, "Aborted"),
            SocketEvent::Ended => write!(f, "Ended"),
            SocketEvent::Timeout => write!(f, "Timeout"),
        }
    }
}

#[derive(Debug, Copy, Clone)]
pub enum MessageType {
    /// Forgettable message type.
    ///
    /// If the message did not make
    /// it through the end the first time, abandon this message.
    Forgettable,
    /// A Key but expirable message.
    ///
    /// The parameter holds the number of
    /// milliseconds this message expires after. If this parameter is 0,
    /// the behavior is the same as Forgettable.
    ///
    /// As long as this message is still valid, it will try to re-send
    /// messages if Socket suspects it did not get the message in time.
    KeyExpirableMessage(u32),
    /// A key message that should arrive everytime.
    ///
    /// A long at the socket doesn't receive the correct ack for this message,
    /// this message will be re-sent.
    KeyMessage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketStatus {
    SynSent,
    SynReceived,

    TimeoutError,

    Connected,

    TerminateSent,
    TerminateReceived,
}

impl SocketStatus {
    pub fn is_connected(self) -> bool {
        self == SocketStatus::Connected
    }

    pub fn event(self) -> Option<SocketEvent> {
        match self {
            SocketStatus::TimeoutError => Some(SocketEvent::Timeout),
            SocketStatus::TerminateSent => Some(SocketEvent::Ended),
            SocketStatus::TerminateReceived => Some(SocketEvent::Ended),
            SocketStatus::Connected => Some(SocketEvent::Connected),
            _ => None
        }
    }

    pub fn is_finished(self) -> bool {
        use SocketStatus::*;
        match self {
            TimeoutError | TerminateSent | TerminateReceived => true,
            _ => false
        }
    }
}

#[derive(Debug)]
pub struct RUdpSocket {
    pub (crate) local_addr: SocketAddr,

    pub (crate) socket: UdpSocketWrapper,

    pub (crate) sent_data_tracker: SentDataTracker<Arc<[u8]>>,

    // Packet handler takes care of the combiner. A good guy, really.
    pub (crate) packet_handler: UdpPacketHandler,

    pub (crate) events: VecDeque<SocketEvent>,

    // pub (self) last_remote_seq_id: u32,
    pub (self) next_local_seq_id: u32,

    pub (self) iteration_n: u64,
    pub (self) last_answer: u64,
}

#[derive(Debug)]
pub enum RUdpCreateError {
    IoError(IoError),
    UnexpectedData,
}

impl From<IoError> for RUdpCreateError {
    fn from(io_error: IoError) -> RUdpCreateError {
        RUdpCreateError::IoError(io_error)
    }
}

#[derive(Debug)]
pub (crate) struct UdpSocketWrapper {
    pub (self) udp_socket: Arc<UdpSocket>,
    pub (self) remote_addr: SocketAddr,
    pub (self) status: SocketStatus,
}

impl UdpSocketWrapper {
    pub (self) fn new(udp_socket: Arc<UdpSocket>, status: SocketStatus, remote_addr: SocketAddr) -> Self {
        UdpSocketWrapper {
            udp_socket,
            remote_addr,
            status,
        }
    } 

    /// Send some bytes without splitting in any way
    #[inline]
    pub (self) fn send_raw_bytes(&self, bytes: &[u8]) -> IoResult<()> {
        let sent_size = self.udp_socket.send_to(bytes, self.remote_addr)?;
        debug_assert_eq!(sent_size, bytes.len(), "udp packet did not contain whole packet");
        Ok(())
    }

    #[inline]
    pub (crate) fn send_udp_packet<P: AsRef<[u8]>>(&self, udp_packet: &UdpPacket<P>) -> ::std::io::Result<()> {
        if ! self.status.is_finished() {
            self.send_raw_bytes(udp_packet.as_bytes())
        } else {
            // useless to send more data is the connection is terminated
            Ok(())
        }
    }

    #[inline]
    pub fn status(&self) -> SocketStatus {
        self.status
    }

    #[inline]
    pub fn set_status(&mut self, new_status: SocketStatus) {
        self.status = new_status;
    }
}

impl RUdpSocket {
    /// Creates a Socket and connects to the remote instantly.
    ///
    /// If you want to accept a new connection, use `new_incoming` instead.
    pub fn connect<A: ToSocketAddrs>(remote_addr: A) -> IoResult<RUdpSocket> {
        let remote_addr = remote_addr.to_socket_addrs()?.next().unwrap();

        let udp_socket = Arc::new(UdpSocket::bind("0.0.0.0:0")?);
        udp_socket.set_nonblocking(true)?;
        let local_addr = udp_socket.local_addr()?;

        let rudp_socket = RUdpSocket {
            socket: UdpSocketWrapper::new(udp_socket, SocketStatus::SynSent, remote_addr),
            local_addr,
            sent_data_tracker: SentDataTracker::new(),
            packet_handler: UdpPacketHandler::new(),
            // last_remote_seq_id: 0,
            events: Default::default(),
            next_local_seq_id: 0,
            iteration_n: 0,
            last_answer: 0,
        };
        rudp_socket.send_syn()?;

        Ok(rudp_socket)
    }

    pub fn new_incoming(udp_socket: Arc<UdpSocket>, incoming_packet: UdpPacket<Box<[u8]>>, incoming_address: SocketAddr) -> Result<RUdpSocket, RUdpCreateError> {
        if let Ok(Packet::Syn) = incoming_packet.compute_packet() {
            let local_addr = udp_socket.local_addr()?;
            let mut rudp_socket = RUdpSocket {
                socket: UdpSocketWrapper::new(udp_socket, SocketStatus::SynReceived, incoming_address),
                local_addr,
                packet_handler: UdpPacketHandler::new(),
                sent_data_tracker: SentDataTracker::new(),
                // last_remote_seq_id: 0,
                events: Default::default(),
                next_local_seq_id: 0,
                iteration_n: 0,
                last_answer: 0,
            };
            rudp_socket.send_synack()?;

            Ok(rudp_socket)
        } else {
            // reject everything that is not a Syn packet.
            Err(RUdpCreateError::UnexpectedData)
        }
    }

    #[inline]
    pub fn drain_events<'a>(&'a mut self) -> impl Iterator<Item=SocketEvent> + 'a {
        self.events.drain(..)
    }

    #[inline]
    pub (self) fn set_status(&mut self, status: SocketStatus) {
        self.socket.set_status(status);
        if let Some(event) = status.event() {
            // We should notify this event
            self.events.push_back(event);
        }
    }
    
    #[inline]
    pub fn send_data(&mut self, data: Arc<[u8]>, message_type: MessageType) {
        self.sent_data_tracker.send_data(self.next_local_seq_id, Arc::from(data), self.iteration_n, message_type, &self.socket);
        self.next_local_seq_id += 1;
    }

    /// Should only be used by connect
    fn send_syn(&self) -> ::std::io::Result<()> {
        let p: Packet<Box<[u8]>> = Packet::Syn;
        let udp_packet = UdpPacket::from(&p);
        self.socket.send_udp_packet(&udp_packet)
    }

    /// Should only be used by new_incoming
    pub (self) fn send_synack(&mut self) -> ::std::io::Result<()> {
        let p: Packet<Box<[u8]>> = Packet::SynAck;
        let udp_packet = UdpPacket::from(&p);
        self.set_status(SocketStatus::Connected);
        self.socket.send_udp_packet(&udp_packet)
    }

    pub (self) fn send_ack<D: AsRef<[u8]> + 'static>(&self, seq_id: u32, ack: Ack<D>) -> ::std::io::Result<()> {
        let p: Packet<D> = Packet::Ack(seq_id, ack.into_inner());
        let udp_packet = UdpPacket::from(&p);
        self.socket.send_udp_packet(&udp_packet)
    }

    pub fn send_end(&self) -> ::std::io::Result<()> {
        let p: Packet<Box<[u8]>> = Packet::End(self.next_local_seq_id.saturating_sub(1));
        let udp_packet = UdpPacket::from(&p);
        self.socket.send_udp_packet(&udp_packet)
    }

    pub fn terminate(self) -> IoResult<()> {
        self.send_end()
    }

    pub (self) fn send_abort(&self) -> ::std::io::Result<()> {
        let p: Packet<Box<[u8]>> = Packet::Abort(self.next_local_seq_id.saturating_sub(1));
        let udp_packet = UdpPacket::from(&p);
        self.socket.send_udp_packet(&udp_packet)
    }

    pub (crate) fn add_received_packet(&mut self, udp_packet: UdpPacket<Box<[u8]>>) {
        self.last_answer = self.iteration_n;
        self.packet_handler.add_received_packet(udp_packet, self.iteration_n);
    }

    fn next_packet_event(&mut self) -> Option<SocketEvent> {
        loop {
            let r = self.packet_handler.next_received_message();
            match r {
                None => return None,
                Some(ReceivedMessage::Abort(_id)) => {
                    self.set_status(SocketStatus::TerminateReceived);
                    return Some(SocketEvent::Aborted)
                },
                Some(ReceivedMessage::Ack(seq_id, data)) => {
                    self.sent_data_tracker.receive_ack(seq_id, data, self.iteration_n, &self.socket);
                },
                Some(ReceivedMessage::Data(_id, data)) => {
                    return Some(SocketEvent::Data(data))
                },
                Some(ReceivedMessage::End(_id)) => {
                    self.set_status(SocketStatus::TerminateReceived);
                    return Some(SocketEvent::Ended)
                },
                Some(ReceivedMessage::SynAck) => {
                    if let SocketStatus::SynSent = self.socket.status() {
                        self.set_status(SocketStatus::Connected);
                    } else {
                        /* received synack when the status isn't even SynSent? Mmmh... */
                    }
                },
                Some(ReceivedMessage::Syn) => {
                    /* do nothing for now, but we may want to handle "syn" later to
                    have a 'reconnect' feature or something? */
                }
            };
        };
    }

    pub (crate) fn incr_tick(&mut self) {
        self.iteration_n += 1;
    }

    pub (crate) fn inner_tick(&mut self) -> IoResult<()> {
        let acks_to_send = self.packet_handler.tick(self.iteration_n);
        while let Some(socket_event) = self.next_packet_event() {
            self.events.push_back(socket_event);
        }
        if self.iteration_n >= self.last_answer + 60 * 10 && !self.socket.status().is_finished() {
            self.set_status(SocketStatus::TimeoutError);
        }
        for (seq_id, ack) in acks_to_send {
            self.send_ack(seq_id, ack)?;
        }
        self.sent_data_tracker.next_tick(self.iteration_n, &self.socket);
        Ok(())
    }

    /// Receive packets from this single source
    ///
    /// Do NOT use from the server, all packets not coming from the remote will be discarded!
    pub fn next_tick(&mut self) -> IoResult<()> {
        self.incr_tick();
        let mut done = false;

        while !done {
            match UdpPacket::<Box<[u8]>>::from_udp_socket(&self.socket.udp_socket) {
                Ok((packet, remote_addr)) => {
                    if remote_addr == self.socket.remote_addr {
                        self.add_received_packet(packet);
                    } else {
                        /* received packet from unknown source */
                    }
                },
                Err(err) => {
                    match err.kind() {
                        IoErrorKind::WouldBlock => { done = true },
                        err_kind => {
                            panic!("SingleSocket: Received other unexpected net error {:?}", err_kind)
                        }
                    }
                },
            };
        };
        self.inner_tick()?;
        Ok(())
    }

    pub fn status(&self) -> SocketStatus {
        self.socket.status
    }
    
    #[inline]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
}

impl Drop for RUdpSocket {
    fn drop(&mut self) {
        match self.socket.status() {
            SocketStatus::Connected | SocketStatus::SynSent | SocketStatus::SynReceived => {
                // TODO: At least log the error
                let _r = self.send_abort();
            },
            _ => {},
        }
    }
}