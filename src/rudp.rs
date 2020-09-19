use std::net::UdpSocket;
use crate::udp_packet_handler::{UdpPacketHandler, ReceivedMessage};
use crate::udp_packet::{UdpPacket, Packet};
use std::net::{SocketAddr, ToSocketAddrs};
use std::io::{Error as IoError, ErrorKind as IoErrorKind, Result as IoResult};
use std::sync::Arc;
use crate::ack::Ack;
use crate::sent_data_tracker::SentDataTracker;
use std::collections::VecDeque;
use crate::ping_handler::*;
use std::time::{Duration, Instant};

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

/// Represents how often the message will get sent without ACK.
///
/// A high priority message will be sent very often until we get a successful ack,
/// while a low priority will often wait for the other party to send an ack to send the appropriate data.
#[derive(Debug, Copy, Clone)]
pub enum MessagePriority {
    Lowest,
    VeryLow,
    Low,
    Normal,
    High,
    VeryHigh,
    Highest,
    Custom { resend_delay: Duration }
}

impl Default for MessagePriority {
    fn default() -> Self {
        MessagePriority::Normal
    }
}

impl MessagePriority {
    pub fn resend_delay(&self) -> Duration {
        match self {
            MessagePriority::Highest => Duration::from_millis(20),
            MessagePriority::VeryHigh => Duration::from_millis(40),
            MessagePriority::High => Duration::from_millis(80),
            MessagePriority::Normal => Duration::from_millis(160),
            MessagePriority::Low => Duration::from_millis(320),
            MessagePriority::VeryLow => Duration::from_millis(640),
            MessagePriority::Lowest => Duration::from_millis(1500),
            MessagePriority::Custom { resend_delay } => *resend_delay,
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
    /// The parameter holds the amount of
    /// time this message expires after. If this parameter is 0,
    /// the behavior is the same as Forgettable.
    ///
    /// As long as this message is still valid, it will try to re-send
    /// messages if Socket suspects it did not get the message in time.
    KeyExpirableMessage(Duration),
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
    SynSent(Instant),
    SynReceived,

    TimeoutError(Instant),

    Connected,

    TerminateSent(Instant),
    TerminateReceived(Instant),
}

impl SocketStatus {
    pub fn is_connected(self) -> bool {
        self == SocketStatus::Connected
    }

    pub (crate) fn event(self) -> Option<SocketEvent> {
        match self {
            SocketStatus::TimeoutError(_) => Some(SocketEvent::Timeout),
            SocketStatus::TerminateSent(_) => Some(SocketEvent::Ended),
            // // this is actually commented to tell you that you should NOT uncomment this,
            // // when we receive a packet, we automatically send the right event (ended or aborted)
            // // so there is no need to have a similar event sent here as well
            // SocketStatus::TerminateReceived => Some(SocketEvent::Ended),
            SocketStatus::TerminateReceived(_) => None,
            SocketStatus::Connected => Some(SocketEvent::Connected),
            _ => None
        }
    }

    pub fn is_finished(self) -> bool {
        use SocketStatus::*;
        match self {
            TimeoutError(_) | TerminateSent(_) | TerminateReceived(_) => true,
            _ => false
        }
    }

    /// Returns true if the connection is finished and old enough to be deleted permanently.
    pub fn is_finished_and_old(self, now: Instant) -> bool {
        use SocketStatus::*;
        match self {
            TimeoutError(t) | TerminateSent(t) | TerminateReceived(t) => (now - t).as_secs() >= 10,
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

    pub (self) cached_now: Instant,
    pub (self) last_received_message: Instant,
    pub (self) last_sent_message: Instant,

    /// required before the socket is set as timeout. Default is 10s
    pub (self) timeout_delay: Duration,

    /// required before we send a sample "heartbeat" message to avoid timeouts.
    pub (self) heartbeat_delay: Duration,
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

const DEFAULT_TIMEOUT_DELAY: Duration = Duration::from_secs(10);
const DEFAULT_HEARTBEAT_DELAY: Duration = Duration::from_secs(1);

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

        let now = Instant::now();
        let mut rudp_socket = RUdpSocket {
            socket: UdpSocketWrapper::new(udp_socket, SocketStatus::SynSent(now), remote_addr),
            local_addr,
            sent_data_tracker: SentDataTracker::new(),
            packet_handler: UdpPacketHandler::new(),
            // last_remote_seq_id: 0,
            events: Default::default(),
            ping_handler: PingHandler::new(),
            next_local_seq_id: 0,
            cached_now: now,
            last_received_message: now,
            last_sent_message: now,
            timeout_delay: DEFAULT_TIMEOUT_DELAY,
            heartbeat_delay: DEFAULT_HEARTBEAT_DELAY,
        };
        log::info!("trying to connect to remote {}...", rudp_socket.remote_addr());
        rudp_socket.send_syn()?;

        Ok(rudp_socket)
    }

    pub (crate) fn new_incoming(udp_socket: Arc<UdpSocket>, incoming_packet: UdpPacket<Box<[u8]>>, incoming_address: SocketAddr) -> Result<RUdpSocket, RUdpCreateError> {
        if let Ok(Packet::Syn) = incoming_packet.compute_packet() {
            let local_addr = udp_socket.local_addr()?;
            let now = Instant::now();
            let mut rudp_socket = RUdpSocket {
                socket: UdpSocketWrapper::new(udp_socket, SocketStatus::SynReceived, incoming_address),
                local_addr,
                packet_handler: UdpPacketHandler::new(),
                sent_data_tracker: SentDataTracker::new(),
                // last_remote_seq_id: 0,
                events: Default::default(),
                next_local_seq_id: 0,
                ping_handler: PingHandler::new(),
                cached_now: now,
                last_received_message: now,
                last_sent_message: now,
                timeout_delay: DEFAULT_TIMEOUT_DELAY,
                heartbeat_delay: DEFAULT_HEARTBEAT_DELAY,
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
    pub fn set_timeout_delay(&mut self, timeout_delay: Duration) {
        self.timeout_delay = timeout_delay;
    }

    /// Set the number of iterations required before we send a "heartbeat" message to the remote,
    /// to make sure they don't consider us as timed out.
    pub fn set_heartbeat_delay(&mut self, heartbeat_delay: Duration) {
        self.heartbeat_delay = heartbeat_delay;
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
    ///
    /// No message priority = Normal priority.
    pub fn send_data(&mut self, data: Arc<[u8]>, message_type: MessageType, message_priority: MessagePriority) {
        if message_type.has_ack() {
            self.ping_handler.ping(self.next_local_seq_id);
        }
        self.sent_data_tracker.send_data(self.next_local_seq_id, data, self.cached_now, message_type, message_priority, &self.socket);
        self.next_local_seq_id += 1;
    }

    fn send_udp_packet<P: AsRef<[u8]>>(&mut self, udp_packet: &UdpPacket<P>) -> std::io::Result<()> {
        self.last_sent_message = self.cached_now;
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

    /// Add a packet to a queue, to be processed later.
    pub (crate) fn add_received_packet(&mut self, udp_packet: UdpPacket<Box<[u8]>>) {
        self.last_received_message = self.cached_now;
        log::trace!("received packet {:?} from remote {}", udp_packet, self.socket.remote_addr);
        self.packet_handler.add_received_packet(udp_packet, self.cached_now);
    }

    /// Process the next paquet received in the queue.
    fn next_packet_event(&mut self) -> Option<SocketEvent> {
        loop {
            let r = self.packet_handler.next_received_message();
            match r {
                None => return None,
                Some(ReceivedMessage::Abort(_id)) => {
                    self.set_status(SocketStatus::TerminateReceived(self.cached_now));
                    return Some(SocketEvent::Aborted)
                },
                Some(ReceivedMessage::Ack(seq_id, data)) => {
                    self.ping_handler.pong(seq_id);
                    self.sent_data_tracker.receive_ack(seq_id, data, self.cached_now);
                },
                Some(ReceivedMessage::Data(_id, data)) => {
                    log::trace!("received data {:?} from remote {}", data, self.socket.remote_addr);
                    return Some(SocketEvent::Data(data))
                },
                Some(ReceivedMessage::End(_id)) => {
                    self.set_status(SocketStatus::TerminateReceived(self.cached_now));
                    return Some(SocketEvent::Ended)
                },
                Some(ReceivedMessage::Heartbeat) => {},
                Some(ReceivedMessage::SynAck) => {
                    if let SocketStatus::SynSent(_) = self.socket.status() {
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

    pub (crate) fn update_cached_now(&mut self) {
        self.cached_now = Instant::now();
    }

    pub (crate) fn inner_tick(&mut self) -> IoResult<()> {
        let acks_to_send = self.packet_handler.tick(self.cached_now);
        while let Some(socket_event) = self.next_packet_event() {
            self.events.push_back(socket_event);
        }
        if self.cached_now >= self.last_received_message + self.timeout_delay && !self.socket.status().is_finished() {
            let ago: Duration = self.cached_now - self.last_received_message;
            log::warn!("socket {} timed out: last_received_message was {}s ago", self.remote_addr(), ago.as_secs_f32());
            self.set_status(SocketStatus::TimeoutError(self.cached_now));
        }
        for (seq_id, ack) in acks_to_send {
            self.send_ack(seq_id, ack)?;
        }
        if self.status().is_connected() {
            if self.cached_now - self.last_sent_message > self.heartbeat_delay {
                self.send_heartbeat()?;
            }
        } else { 
            if let SocketStatus::SynSent(last_sent) = self.status() {
                // we're attempting to connect..
                // but if we haven't received an answer for 3 seconds, the message might have been missed and we'll resend it.
                if self.cached_now > last_sent + Duration::from_secs(3) {
                    // every 3 seconds (we incremented tick once before this call so 0 is out)
                    // resend a "syn" to attempt to connect.
                    self.send_syn()?;
                    self.set_status(SocketStatus::SynSent(self.cached_now))
                }
            }
        }
        self.sent_data_tracker.next_tick(self.cached_now, &self.socket);
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
        self.update_cached_now();
        let mut done = false;

        // receive incoming packets and put them in a queue for processing
        while !done {
            match UdpPacket::<Box<[u8]>>::from_udp_socket(&self.socket.udp_socket) {
                Ok((packet, remote_addr)) => {
                    if remote_addr == self.socket.remote_addr {
                        self.add_received_packet(packet);
                    } else {
                        log::trace!("received unexpected UDP data from someone which was not remote server {}", remote_addr);
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
        // process everything we have received
        self.inner_tick()?;
        Ok(())
    }

    #[inline]
    pub fn status(&self) -> SocketStatus {
        self.socket.status
    }

    /// Returns whether or not you should clear this RUdp client.
    pub fn should_clear(&self) -> bool {
        self.socket.status.is_finished_and_old(self.cached_now)
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
            SocketStatus::Connected | SocketStatus::SynSent(_) | SocketStatus::SynReceived => {
                // TODO: At least log the error
                let _r = self.send_abort();
            },
            _ => {},
        }
    }
}