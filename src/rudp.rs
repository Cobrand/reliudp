use std::net::UdpSocket;
use udp_packet_handler::{UdpPacketHandler, ReceivedMessage};
use udp_packet::{UdpPacket, Packet};
use std::net::{SocketAddr, ToSocketAddrs};
use std::io::{Error as IoError, ErrorKind as IoErrorKind, Result as IoResult};
use std::sync::Arc;
use ack::Ack;
use sent_data_tracker::SentDataTracker;
use std::collections::VecDeque;
use ping_handler::*;

/// Represents an event of the Socket.
///
/// They fall in mostly 2 categories: meta events, and data events.
pub enum SocketEvent {
    /// Data sent by the remote, re-assembled
    Data(Box<[u8]>),
    /// Represents when the handshake with the other side was done successfully
    Connected,
    /// Connection was aborted unexpectedly by the other end (not the same as Timeout or Ended)
    Aborted,
    /// Connection was ended peacefully by the other end
    Ended,
    /// We haven't got any packet coming from the other for a certain amount of time
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

/// Represents the type of message you are able to send (key, forgettable, ...)
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
    /// frames this message expires after. If this parameter is 0,
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

impl MessageType {
    pub fn has_ack(self) -> bool {
        use MessageType::{KeyExpirableMessage, KeyMessage};
        match self {
            KeyExpirableMessage(_) | KeyMessage => true,
            _ => false
        }
    } 
}


/// Represents the internal connection status of the Socket
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

    pub (crate) fn event(self) -> Option<SocketEvent> {
        match self {
            SocketStatus::TimeoutError => Some(SocketEvent::Timeout),
            SocketStatus::TerminateSent => Some(SocketEvent::Ended),
            // // this is actually commented to tell you that you should NOT uncomment this,
            // // when we receive a packet, we automatically send the right event (ended or aborted)
            // // so there is no need to have a similar event sent here as well
            // SocketStatus::TerminateReceived => Some(SocketEvent::Ended),
            SocketStatus::TerminateReceived => None,
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

/// A RUdp Client Socket
///
/// Represents a connection between you (the host) and the remote. You
/// can send messages, receive messages and poll it for various events.
///
/// Once dropped, the socket will send a "terminate" message to the remote before shutting down.
///
/// A `RUdpServer` holds 0, 1 or more of them.
#[derive(Debug)]
pub struct RUdpSocket {
    pub (crate) local_addr: SocketAddr,

    pub (crate) socket: UdpSocketWrapper,

    pub (crate) sent_data_tracker: SentDataTracker<Arc<[u8]>>,

    // Packet handler takes care of the combiner. A good guy, really.
    pub (crate) packet_handler: UdpPacketHandler,

    pub (crate) events: VecDeque<SocketEvent>,

    pub (crate) ping_handler: PingHandler,

    // pub (self) last_remote_seq_id: u32,
    pub (self) next_local_seq_id: u32,

    pub (self) iteration_n: u64,
    pub (self) last_received_message: u64,
    pub (self) last_sent_message: u64,

    /// number of iterations required before the socket is set as timeout. Default is 600, but it's heavily
    /// recommended to change this value to your needs.
    pub (self) timeout_delay: u64,
}

#[derive(Debug)]
pub (crate) enum RUdpCreateError {
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
    /// This will fail ONLY if there is something wrong with the network,
    /// preventing it to create a UDP Socket. This is NOT blocking,
    /// so any timeout event or things or the like will arrive as `SocketEvent`s.
    ///
    /// The socket will be created with the status SynSent, after which there will be 2 outcomes:
    ///
    /// * The remote answered SynAck, and we set the status as "Connected"
    /// * The remote did not answer, and we will get a timeout
    // If you want to accept a new connection, use `new_incoming` instead.
    pub fn connect<A: ToSocketAddrs>(remote_addr: A) -> IoResult<RUdpSocket> {
        let remote_addr = remote_addr.to_socket_addrs()?.next().unwrap();

        let udp_socket = Arc::new(UdpSocket::bind("0.0.0.0:0")?);
        udp_socket.set_nonblocking(true)?;
        let local_addr = udp_socket.local_addr()?;

        let mut rudp_socket = RUdpSocket {
            socket: UdpSocketWrapper::new(udp_socket, SocketStatus::SynSent, remote_addr),
            local_addr,
            sent_data_tracker: SentDataTracker::new(),
            packet_handler: UdpPacketHandler::new(),
            // last_remote_seq_id: 0,
            events: Default::default(),
            ping_handler: PingHandler::new(),
            next_local_seq_id: 0,
            iteration_n: 0,
            last_received_message: 0,
            last_sent_message: 0,
            timeout_delay: 600,
        };
        log::info!("trying to connect to remote {}...", rudp_socket.remote_addr());
        rudp_socket.send_syn()?;

        Ok(rudp_socket)
    }

    pub (crate) fn new_incoming(udp_socket: Arc<UdpSocket>, incoming_packet: UdpPacket<Box<[u8]>>, incoming_address: SocketAddr) -> Result<RUdpSocket, RUdpCreateError> {
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
                ping_handler: PingHandler::new(),
                iteration_n: 0,
                last_received_message: 0,
                last_sent_message: 0,
                timeout_delay: 600,
            };
            rudp_socket.send_synack()?;
            log::info!("received incoming connection from {}", rudp_socket.remote_addr());

            Ok(rudp_socket)
        } else {
            // reject everything that is not a Syn packet.
            Err(RUdpCreateError::UnexpectedData)
        }
    }

    /// Set the number of iterations required before a remote is set as "dead".
    /// 
    /// For instance, if your tick is every 50ms, and your timeout_delay is of 24,
    /// then roughly 50*24=1200ms (=1.2s) without a message from the remote will cause a timeout error.
    pub fn set_timeout_delay(&mut self, timeout_delay: u64) {
        self.timeout_delay = timeout_delay;
    }

    pub fn set_timeout_delay_with(&mut self, milliseconds: u64, tick_interval_milliseconds: u64) {
        assert!(tick_interval_milliseconds > 0);
        self.timeout_delay = milliseconds / tick_interval_milliseconds;
    }

    #[inline]
    /// Drains socket events for this Socket.
    ///
    /// This is one of the 2 ways to loop over all incoming events. See the examples
    /// for how to use it.
    pub fn drain_events<'a>(&'a mut self) -> impl Iterator<Item=SocketEvent> + 'a {
        self.events.drain(..)
    }

    #[inline]
    /// Gets the next socket event for this socket.
    pub fn next_event(&mut self) -> Option<SocketEvent> {
        self.events.pop_front()
    }

    #[inline]
    pub (self) fn set_status(&mut self, status: SocketStatus) {
        log::debug!("socket {}: new status {:?}", self.remote_addr(), status);
        self.socket.set_status(status);
        if let Some(event) = status.event() {
            // We should notify this event
            self.events.push_back(event);
        }
    }
    
    #[inline]
    /// Send data to the remote.
    pub fn send_data(&mut self, data: Arc<[u8]>, message_type: MessageType) {
        if message_type.has_ack() {
            self.ping_handler.ping(self.next_local_seq_id);
        }
        self.sent_data_tracker.send_data(self.next_local_seq_id, data, self.iteration_n, message_type, &self.socket);
        self.next_local_seq_id += 1;
    }

    fn send_udp_packet<P: AsRef<[u8]>>(&mut self, udp_packet: &UdpPacket<P>) -> std::io::Result<()> {
        self.last_sent_message = self.iteration_n;
        self.socket.send_udp_packet(&udp_packet)
    }

    /// Should only be used by connect
    fn send_syn(&mut self) -> ::std::io::Result<()> {
        let p: Packet<Box<[u8]>> = Packet::Syn;
        let udp_packet = UdpPacket::from(&p);
        self.send_udp_packet(&udp_packet)
    }

    /// Should only be used by new_incoming
    pub (self) fn send_synack(&mut self) -> ::std::io::Result<()> {
        let p: Packet<Box<[u8]>> = Packet::SynAck;
        let udp_packet = UdpPacket::from(&p);
        self.set_status(SocketStatus::Connected);
        self.send_udp_packet(&udp_packet)
    }

    pub (self) fn send_ack<D: AsRef<[u8]> + 'static>(&mut self, seq_id: u32, ack: Ack<D>) -> ::std::io::Result<()> {
        let p: Packet<D> = Packet::Ack(seq_id, ack.into_inner());
        let udp_packet = UdpPacket::from(&p);
        self.send_udp_packet(&udp_packet)
    }

    /// Same as `terminate`, but leave the Socket alive.
    ///
    /// This is mostly useful if you want to still receive the data the other remote is currently
    /// sending at this time. However, note that no acks will be sent, so its usefulness
    /// is still limited.
    pub fn send_end(&mut self) -> ::std::io::Result<()> {
        let p: Packet<Box<[u8]>> = Packet::End(self.next_local_seq_id.saturating_sub(1));
        let udp_packet = UdpPacket::from(&p);
        self.send_udp_packet(&udp_packet)
    }

    /// Terminates the socket, by sending a "Ended" event to the remote.
    pub fn terminate(mut self) -> IoResult<()> {
        self.send_end()
    }

    fn send_heartbeat(&mut self) -> ::std::io::Result<()> {
        let p: Packet<Box<[u8]>> = Packet::Heartbeat;
        let udp_packet = UdpPacket::from(&p);
        self.send_udp_packet(&udp_packet)
    }

    pub (self) fn send_abort(&mut self) -> ::std::io::Result<()> {
        let p: Packet<Box<[u8]>> = Packet::Abort(self.next_local_seq_id.saturating_sub(1));
        let udp_packet = UdpPacket::from(&p);
        self.send_udp_packet(&udp_packet)
    }

    pub (crate) fn add_received_packet(&mut self, udp_packet: UdpPacket<Box<[u8]>>) {
        self.last_received_message = self.iteration_n;
        log::trace!("received packet {:?} from remote {} at n={}", udp_packet, self.socket.remote_addr, self.iteration_n);
        self.packet_handler.add_received_packet(udp_packet, self.iteration_n);
    }

    fn next_packet_event(&mut self) -> Option<SocketEvent> {
        loop {
            let r = self.packet_handler.next_received_message();
            if r.is_some() {
                log::debug!("received message {:?} from remote {} at n={}", r, self.socket.remote_addr, self.iteration_n);
            }
            match r {
                None => return None,
                Some(ReceivedMessage::Abort(_id)) => {
                    self.set_status(SocketStatus::TerminateReceived);
                    return Some(SocketEvent::Aborted)
                },
                Some(ReceivedMessage::Ack(seq_id, data)) => {
                    self.ping_handler.pong(seq_id);
                    self.sent_data_tracker.receive_ack(seq_id, data, self.iteration_n, &self.socket);
                },
                Some(ReceivedMessage::Data(_id, data)) => {
                    return Some(SocketEvent::Data(data))
                },
                Some(ReceivedMessage::End(_id)) => {
                    self.set_status(SocketStatus::TerminateReceived);
                    return Some(SocketEvent::Ended)
                },
                Some(ReceivedMessage::Heartbeat) => {},
                Some(ReceivedMessage::SynAck) => {
                    if let SocketStatus::SynSent = self.socket.status() {
                        log::info!("connected to remote {}", self.remote_addr());
                        self.set_status(SocketStatus::Connected);
                    } else {
                        log::warn!("received synack while the status isn't synsent for {}", self.remote_addr());
                        /* received synack when the status isn't even SynSent? Mmmh... */
                    }
                },
                Some(ReceivedMessage::Syn) => {
                    log::warn!("received a syn message while already connected {}", self.remote_addr());
                    /* do nothing for now, but we may want to handle "syn" later to
                    have a 'reconnect' feature or something? */
                }
            };
        };
    }

    /// Returns the ping to the remote as ms
    ///
    /// Returns None if the ping has not been computed yet
    pub fn ping(&self) -> Option<u32> {
        self.ping_handler.current_ping_ms()
    }

    pub (crate) fn incr_tick(&mut self) {
        self.iteration_n += 1;
    }

    pub (crate) fn inner_tick(&mut self) -> IoResult<()> {
        let acks_to_send = self.packet_handler.tick(self.iteration_n);
        while let Some(socket_event) = self.next_packet_event() {
            self.events.push_back(socket_event);
        }
        if self.iteration_n >= self.last_received_message + self.timeout_delay && !self.socket.status().is_finished() {
            log::warn!("socket {} timed out: last_received_message={}, iteration_n={}", self.remote_addr(), self.last_received_message, self.iteration_n);
            self.set_status(SocketStatus::TimeoutError);
        }
        for (seq_id, ack) in acks_to_send {
            self.send_ack(seq_id, ack)?;
        }
        if self.iteration_n.saturating_sub(self.last_received_message) >= 20 {
            self.send_heartbeat()?;
        }
        self.sent_data_tracker.next_tick(self.iteration_n, &self.socket);
        Ok(())
    }

    /// Internal processing for this single source
    ///
    /// Must be done before draining events. Even if there are no events,
    /// you will want to re-send acks, keep track of sent data, etc. `next_tick` does that for you.
    ///
    /// Do NOT use this method if you have multiple remotes for a single UdpSocket (a single port):
    /// all packets not coming from the right remote (matching IP and port) will be discarded!
    /// This warning applies if this socket has been borrowed from a `RUdpServer` as well,
    /// because all the remotes are sharing the same port.
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
                            log::error!("SingleSocket: Received other unexpected net error {:?}", err_kind)
                        }
                    }
                },
            };
        };
        self.inner_tick()?;
        Ok(())
    }

    #[inline]
    pub fn status(&self) -> SocketStatus {
        self.socket.status
    }
    
    #[inline]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn remote_addr(&self) -> SocketAddr {
        self.socket.remote_addr
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