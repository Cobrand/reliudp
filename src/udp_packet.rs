use byteorder::{BigEndian, ByteOrder};
use consts::*;
use fragment::*;
use misc::*;

use crc::crc32::checksum_ieee as crc32_check;

#[derive(Debug, PartialEq)]
pub (crate) enum Packet<P: AsRef<[u8]>> {
    Fragment(Fragment<P>),
    Ack(u32, P),
    Syn,
    SynAck,
    Heartbeat,
    End(u32),
    Abort(u32)
}

impl<P: AsRef<[u8]>> Packet<P> {
    pub (crate) fn udp_packet_size(&self) -> usize {
        let data_size = match *self {
            Packet::Fragment(Fragment { ref data, .. }) => FRAG_ADD_HEADER_SIZE + data.as_ref().len(),
            Packet::Ack(_, ref data) => data.as_ref().len(),
            _ => 0,
        };
        CRC32_SIZE + COMMON_HEADER_SIZE + data_size
    }

    /// Returns seq_id, frag_id, frag_total
    pub (crate) fn header(&self) -> (u32, u8, u8) {
        match *self {
            Packet::Fragment(Fragment { seq_id, frag_id, frag_total, .. }) => (seq_id, frag_id, frag_total),
            Packet::Ack(seq_id, _) => (seq_id, 255, 0),
            Packet::Syn => (0, 255, 1),
            Packet::SynAck => (0, 255, 2),
            Packet::End(last_seq_id) => (last_seq_id, 255, 3),
            Packet::Abort(last_seq_id) => (last_seq_id, 255, 4),
            Packet::Heartbeat => (0, 255, 5),
        }
    }

    // Should be sent the part of the payload ( &[10..] ) to write to
    #[inline]
    pub (crate) fn write_payload(&self, payload: &mut [u8]) {
        match *self {
            Packet::Fragment(Fragment { ref data, frag_meta, ..}) => {
                payload[0] = frag_meta as u8;
                payload[1..].copy_from_slice(data.as_ref())
            },
            Packet::Ack(_, ref data) => payload.copy_from_slice(data.as_ref()),
            _ => {/* don't write a payload for the other kinds */}
        }
    }

    /// For testing purposes
    #[inline]
    #[cfg(test)]
    pub (crate) fn cmp_with<T2: AsRef<[u8]>>(&self, other: &Packet<T2>) -> bool {
        use self::Packet::*;
        match (self, other) {
            (Fragment(f1), Fragment(f2)) => 
                f1.seq_id == f2.seq_id && f1.frag_id == f2.frag_id && f1.frag_total == f2.frag_total
                && f1.data.as_ref() == f2.data.as_ref(),
            (Ack(s1, ref d1), Ack(s2, ref d2)) => s1 == s2 && d1.as_ref() == d2.as_ref(),
            (Syn, Syn) => true,
            (SynAck, SynAck) => true,
            (End(s1), End(s2)) => s1 == s2,
            (Abort(s1), Abort(s2)) => s1 == s2,
            (Heartbeat, Heartbeat) => true,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
/// Describes the "meta" (6 bytes after CRC32) part of a Packet.
pub enum PacketMeta {
    /// A regular fragment with (seq_id, frag_id, frag_total)
    Fragment(u32, u8, u8, FragmentMeta),
    /// A regular Fragment Ack with seq_id
    Ack(u32),
    Syn,
    SynAck,
    Heartbeat,
    End(u32),
    Abort(u32),
}

impl PacketMeta {
    /// Takes as input the **stripped** data. The 10 first bytes of the packet **must**
    /// have been stripped before hand. This method cannot fail.
    pub (crate) fn build_packet_with<P: 'static + AsRef<[u8]>>(self, data: OwnedSlice<u8, P>) -> Packet<OwnedSlice<u8, P>> {
        match self {
            PacketMeta::Fragment(seq_id, frag_id, frag_total, frag_meta) =>
                Packet::Fragment(Fragment {
                    seq_id, frag_id, frag_total, data: data.with_added_strip(1), frag_meta,
                }),
            PacketMeta::Ack(seq_id) =>
                Packet::Ack(seq_id, data),
            PacketMeta::Syn => Packet::Syn,
            PacketMeta::SynAck => Packet::SynAck,
            PacketMeta::Heartbeat => Packet::Heartbeat,
            PacketMeta::End(last_seq_id) => Packet::End(last_seq_id),
            PacketMeta::Abort(last_seq_id) => Packet::Abort(last_seq_id),
        }
    }
}

/// A UdpPacket must contain a buffer that is AT LEAST
/// 10 bytes long. The structure for the udp message is as follow:
///
/// [0-3]: CRC32 check of [4-] as BigEndian u32
/// [4-7]:
///     * if type == Fragment, the sequence id
///     * if type == Ack, the sequence id of the acknowledged sequence
///     * if type == Syn, type == SynAck, nothing (0s)
///     * if type == End or type == Abort, the last SeqId sent
/// [8]: "Frag Id"
/// [9] "Frag total"
/// [10] "Frag meta": required ONLY if the type of the message is frag.
///
/// For now, there are 6 types of messages: `Fragment`s, `Ack`s,
/// `Syn`, `SynAck`, `End` and `Abort`.
///
/// # Determine the type of the packet:
///
/// Note that FragTotal+1 represents the number of frags there may be, so it IS possible
/// to have 0 as a frag_total (1 fragment) and 255 as well (255 fragments).
/// 
/// However, it does not make sense if frag_id is greater than frag_total, so some of those
/// couples are reserved to determine the type of the received (or sent!) packet:
///
/// * If Frag ID <= Frag Total, type = Fragment.
/// * If Frag ID == 255, Frag Total == 0: type = Ack. Ack packet for a fragment/sequence element.
/// * If Frag ID == 255, Frag Total == 1: type = Syn. This type is sent when trying to initiate
/// a connection with a remote.
/// * If Frag ID == 255, Frag Total == 2: type = SynAck: confirm that a connection has been created.
/// * If Frag ID == 255, Frag Total == 3: type = End. The other end has nothing else to send,
/// and the connection is immediatly closed.
/// * If Frag ID == 255, Frag Total == 4: type = Abort: Other program has been terminated
/// unexpectedly and will not receive nor send packets anymore.
/// * If Frag ID == 255, Frag Total == 5: type = Heartbeat: Message sent every few iterations
/// to make sure the remote does not disconnect unexpectedly.
/// * Other uses for Frag ID == 255 and Frag Total != 255 are reserved for other packets like these.
///
/// # Fragment
///
/// A Fragment is a chunk of a message, represented with the structure above.
///
/// Rather than a length explanation, let's start with a simply message: [1, 2, 3, 4, 5].
///
/// Let's say the the maximum payload size (TOTAL_SIZE - HEADER_SIZE) is 2 bytes. That
/// means we have to split our message in 3 fragments: one containing [1, 2], the other [3, 4]
/// and the last [5]. Let's say this is the second packet we were to send, the seq_id
/// would be INITIAL_SEQUENCE_NUMBER + SEQUENCE_INDEX = 0 + 1 (if the initial sequence number is 0).
///
/// Frag total would be 2, because we have 3 packets total, and frag_total is always frags.len()-1.
///
/// As for the respective frag_id, they are 0 for [1, 2], 1 for [3, 4] and so on.
///
/// Finally, based on that, for every fragment the CRC32 is generated: it is based on the slice that
/// starts at the end of the CRC32 and ends at the end of the UDP Packet. Meaning,
/// for TOTAL_SIZE=12, the CRC32 would be based on frag[4..12] of the packet.
///
/// # Ack
///
/// Payload will contain additional data on top of the header, not defined by the user.
/// This additional data will be at most the size of (Type<FragId>::Max + 1) / 8, meaning
/// (255 + 1) / 8 = 32 bytes.
/// 
/// Hence for a Ack packet, the maximum length will be of 10bytes (header) + 32bytes = 42 bytes.
///
/// Those 32 bytes are filled with binaries (1 or 0), and are used to send which of the frag IDs
/// have been received.
///
/// If the maximum a sequence can be is 64 packets, the first 64 bits
/// (so 1 x u64, or 8 x u8) represent whether or not each corresponding packet has been
/// acknowledged. For instance, if a sender sends a packet with frag total of 3 (so, truly 4 fragments total),
/// and the bits are like so: 0101; then it means the packets 0 and 2 have *not* been received and
/// must be sent again by the client receiving the ACK.
///
/// The receiver will send 1 of these packets per iteration at *most*, unless the packet is totally received (all 1s to send),
/// then the packet is sent once per iteration, for 10 iterations (to make sure the ack goes through).
pub struct UdpPacket<B: AsRef<[u8]>> {
   pub (crate) buffer: B
}

impl<B: AsRef<[u8]>> ::std::fmt::Debug for UdpPacket<B> {
    fn fmt(&self, f: &mut ::std::fmt::Formatter) -> Result<(), ::std::fmt::Error> {
        let data = hex::encode(&self.buffer);
        write!(f, "UdpPacket [hex 0x{}]", data)
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub (crate) enum UdpPacketError {
    /// Received data was not big enough to be a message readable by this crate.
    ///
    /// (It must be at least 10 bytes, 11 bytes for frags)
    NotBigEnough, // (That's what she said)
    /// The Crc inside the message was not valid
    InvalidCrc,
    /// Frag Layout is incorrect (frag_id, frag_total)
    InvalidFragLayout(u8, u8),
    InvalidFragMeta,
}

impl<'a, T: AsRef<[u8]>> From<&'a Fragment<T>> for UdpPacket<Box<[u8]>> {
    fn from(f: &'a Fragment<T>) -> UdpPacket<Box<[u8]>> {
        let p = Packet::Fragment(Fragment::as_borrowed_frag(f));
        Self::from(&p)
    }
}

impl<'a, T: AsRef<[u8]>> From<&'a Packet<T>> for UdpPacket<Box<[u8]>> {
    fn from(p: &'a Packet<T>) -> UdpPacket<Box<[u8]>> {
        let mut bytes_mut = vec!(0; p.udp_packet_size());
        let (seq_id, frag_id, frag_total) = p.header();
        BigEndian::write_u32(&mut bytes_mut[4..8], seq_id);
        // write frag_id and frag_total as u8s
        bytes_mut[8] = frag_id;
        bytes_mut[9] = frag_total;
        p.write_payload(&mut bytes_mut[10..]);
        let generated_crc: u32 = crc32_check(&bytes_mut[4..]);
        BigEndian::write_u32(&mut bytes_mut[0..4], generated_crc);
        UdpPacket {buffer: bytes_mut.into_boxed_slice()}
    }
}

impl<B: AsRef<[u8]>> UdpPacket<B> {
    fn check_header_crc(udp_message: &[u8]) -> Result<(), UdpPacketError> {
        let buffer = udp_message;
        if buffer.len() < 10 {
            return Err(UdpPacketError::NotBigEnough);
        }
        let message_crc32: u32 = BigEndian::read_u32(&buffer[0..4]);
        let computed_crc32 = crc32_check(&buffer[4..]);
        if computed_crc32 != message_crc32 {
            Err(UdpPacketError::InvalidCrc)
        } else {
            Ok(())
        }
    }

    #[cfg(test)]
    pub fn new(b: B) -> UdpPacket<B>{
        UdpPacket {buffer: b}
    }

    /// Reads one message from a udp socket and returns its content as a UdpPacket
    ///
    /// Proper parameters that you see fit must have been set on UdpSocket. For instance,
    /// it may be wise to set this udp socket as non-blocking  if you don't want to block
    /// your thread forever trying to read one message.
    pub fn from_udp_socket(udp_socket: &::std::net::UdpSocket) -> ::std::io::Result<(UdpPacket<Box<[u8]>>, ::std::net::SocketAddr)> {
        let mut buffer = vec!(0; MAX_UDP_MESSAGE_SIZE);
        let (message_size, socket_addr) = udp_socket.recv_from(buffer.as_mut_slice())?;
        buffer.truncate(message_size);
        let udp_message = UdpPacket {buffer: buffer.into_boxed_slice()};
        Ok((udp_message, socket_addr))
    }

    #[inline]
    pub (crate) fn as_bytes(&self) -> &[u8] {
        self.buffer.as_ref()
    }
    
    pub (crate) fn compute_packet_meta(&self) -> Result<PacketMeta, UdpPacketError> {
        Self::check_header_crc(self.buffer.as_ref())?;
        let buffer = self.buffer.as_ref();
        if buffer.len() < 10 {
            return Err(UdpPacketError::NotBigEnough);
        }
        let frag_total: u8 = buffer[9];
        let frag_id: u8 = buffer[8];
        let seq_id: u32 = BigEndian::read_u32(&buffer[4..8]);
        let message_crc32: u32 = BigEndian::read_u32(&buffer[0..4]);
        let computed_crc32 = crc32_check(&buffer[4..]);
        if computed_crc32 != message_crc32 {
            return Err(UdpPacketError::InvalidCrc)
        }
        match (frag_id, frag_total) {
            (255, 0) => Ok(PacketMeta::Ack(seq_id)),
            (255, 1) => Ok(PacketMeta::Syn),
            (255, 2) => Ok(PacketMeta::SynAck),
            (255, 3) => Ok(PacketMeta::End(seq_id)),
            (255, 4) => Ok(PacketMeta::Abort(seq_id)),
            (255, 5) => Ok(PacketMeta::Heartbeat),

            // since frag_total is really +1, if frag_id == frag_total, it's actually the last fragment
            // that we received. if frag_id = frag_total = 0, the first and last fragment of a message was received.
            (frag_id, frag_total) if frag_id <= frag_total => {
                // it's a fragment
                if buffer.len() < 11 {
                    // we need another byte here for the "frag_meta" field.
                    return Err(UdpPacketError::NotBigEnough);
                }
                let frag_meta = buffer[10];
                let frag_meta = match frag_meta {
                    0 => FragmentMeta::Forgettable,
                    1 => FragmentMeta::KeyExpirable,
                    2 => FragmentMeta::Key,
                    _ => return Err(UdpPacketError::InvalidFragMeta),
                };
                Ok(PacketMeta::Fragment(seq_id, frag_id, frag_total, frag_meta))
            },
            (frag_id, frag_total) => Err(UdpPacketError::InvalidFragLayout(frag_id, frag_total)),
        }
    }
}

impl<D: AsRef<[u8]> + 'static> UdpPacket<D> {
    pub (crate) fn compute_packet(self) -> Result<Packet<OwnedSlice<u8, D>>, UdpPacketError> {
        let packet_meta = self.compute_packet_meta()?;
        Ok(packet_meta.build_packet_with(OwnedSlice::new(self.buffer, PACKET_DATA_START_BYTE)))
    }
}

#[test]
fn udp_fail_not_big_enough() {
    let received_message: &'static [u8] = &[0u8, 0u8, 0u8, 0u8, 1u8, 2u8, 5u8];
    let received_fragment = UdpPacket::new(received_message);
    let e = received_fragment.compute_packet().unwrap_err();
    assert_eq!(e, UdpPacketError::NotBigEnough);
}

#[test]
fn udp_fail_invalid_crc() {
    let received_message: &'static [u8] = &[0; 20];
    let received_udp_message = UdpPacket::new(received_message);
    let e = received_udp_message.compute_packet().unwrap_err();
    assert_eq!(e, UdpPacketError::InvalidCrc);
}

#[test]
fn udp_success_fragment_parse() {
    let received_message_bytes: &'static [u8] = &[0x12, 0x25, 0xEF, 0xFF, 0, 0, 0, 0, 0, 0, 0, 1];
    let udp_message = UdpPacket::new(received_message_bytes);
    let packet = udp_message.compute_packet().unwrap();
    if let Packet::Fragment(Fragment { seq_id, frag_id, frag_total, data: b, frag_meta}) = packet {
        assert_eq!(seq_id, 0);
        assert_eq!(frag_id, 0);
        assert_eq!(frag_total, 0);
        assert_eq!(frag_meta, FragmentMeta::Forgettable);
        assert_eq!(b.as_ref().len(), 1);
        assert_eq!(b.as_ref(), &[1]);
    } else {
        panic!("Received packet was not a meta message");
    }
}

#[test]
fn udp_fail_fragment_invalid_layout() {
    let received_message_bytes: &'static [u8] = &[0xF8, 0xF1, 0xE3, 0x31, 0, 0, 0, 0, 254, 253];
    let udp_message = UdpPacket::new(received_message_bytes);
    let err = udp_message.compute_packet().unwrap_err();
    assert_eq!(err, UdpPacketError::InvalidFragLayout(254, 253));
}

#[test]
fn udp_success_ack_parse() {
    let received_message_bytes: &'static [u8] = &[0x05, 0xCD, 0x02, 0xE4, 0, 0, 0, 5, 255, 0, 255, 255, 255, 255, 255, 255, 255, 255];
    let udp_message = UdpPacket::new(received_message_bytes);
    let packet = udp_message.compute_packet().unwrap();
    if let Packet::Ack(seq_id, b) = packet {
        assert_eq!(seq_id, 5);
        assert_eq!(b.as_ref().len(), 8);
    } else {
        panic!("Received packet was not a fragment ACK");
    }
}

#[test]
fn udp_success_syn_parse() {
    let received_message_bytes: &'static [u8] = &[0x55, 0xE1, 0x6C, 0x47, 0, 0, 0, 0, 255, 1];
    let udp_message = UdpPacket::new(received_message_bytes);
    let packet = udp_message.compute_packet().unwrap();
    if let Packet::Syn = packet {
        // Ok
    } else {
        panic!("Received packet was not a fragment SYN");
    }
}

#[test]
fn udp_success_synack_parse() {
    let received_message_bytes: &'static [u8] = &[0xCC, 0xE8, 0x3D, 0xFD, 0, 0, 0, 0, 255, 2];
    let udp_message = UdpPacket::new(received_message_bytes);
    let packet = udp_message.compute_packet().unwrap();
    if let Packet::SynAck = packet {
        // Ok
    } else {
        panic!("Received packet was not a fragment SYNACK");
    }
}

#[test]
fn udp_ser_de_ack() {
    let ack1 = Packet::Ack(5, &[0u8; 8]);
    let udp_packet = UdpPacket::from(&ack1);
    let ack2 = udp_packet.compute_packet().unwrap();
    if !ack1.cmp_with(&ack2) {
        panic!("{:?} != {:?}, ack serialized is different from deserialized", ack1, ack2);
    }
}

#[test]
fn udp_ser_de_syn_synack_others() {
    let syn1: Packet<Box<[u8]>> = Packet::Syn;
    let synack1: Packet<Box<[u8]>> = Packet::SynAck;
    let end1: Packet<Box<[u8]>> = Packet::End(5);
    let abort1: Packet<Box<[u8]>> = Packet::Abort(10);
    let heartbeat1: Packet<Box<[u8]>> = Packet::Heartbeat;
    let syn_packet = UdpPacket::from(&syn1);
    let synack_packet = UdpPacket::from(&synack1);
    let end_packet = UdpPacket::from(&end1);
    let abort_packet = UdpPacket::from(&abort1);
    let heartbeat_packet = UdpPacket::from(&heartbeat1);

    let syn2 = syn_packet.compute_packet().unwrap();
    let synack2 = synack_packet.compute_packet().unwrap();
    let end2 = end_packet.compute_packet().unwrap();
    let abort2 = abort_packet.compute_packet().unwrap();
    let heartbeat2 = heartbeat_packet.compute_packet().unwrap();
    if !syn1.cmp_with(&syn2) {
        panic!("{:?} != {:?}, syn serialized is different from deserialized", syn1, syn2);
    }
    if !synack1.cmp_with(&synack2) {
        panic!("{:?} != {:?}, synack serialized is different from deserialized", synack1, synack2);
    }
    if !end1.cmp_with(&end2) {
        panic!("{:?} != {:?}, end serialized is different from deserialized", end1, end2);
    }
    if !abort1.cmp_with(&abort2) {
        panic!("{:?} != {:?}, abort serialized is different from deserialized", abort1, abort2);
    }
    if !heartbeat1.cmp_with(&heartbeat2) {
        panic!("{:?} != {:?}, heartbeat serialized is different from deserialized", heartbeat1, heartbeat2);
    }
}

#[test]
fn udp_success_frag_conversions() {
    let sent_fragment = Fragment {
        seq_id: 12,
        frag_id: 0,
        frag_total: 0,
        frag_meta: FragmentMeta::Key,
        data: &[1u8, 2, 3, 4]
    };
    let udp_message: UdpPacket<_> = UdpPacket::from(&sent_fragment);

    let received_packet = udp_message.compute_packet().unwrap();

    if let Packet::Fragment(Fragment {seq_id, frag_id, frag_total, data, frag_meta}) = received_packet {
        assert_eq!(seq_id, sent_fragment.seq_id);
        assert_eq!(frag_id, sent_fragment.frag_id);
        assert_eq!(frag_total, sent_fragment.frag_total);
        assert_eq!(frag_meta, FragmentMeta::Key);
        assert_eq!(data.as_ref(), sent_fragment.data);
    } else {
        panic!("Received message is not of fragment type!")
    }
}