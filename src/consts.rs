// CRC32 = u32 = 4bytes
pub (crate) const CRC32_SIZE: usize = 4;

// 4 bytes for the seq_id, 1 for the frag_id, 1 for the frag_total
pub (crate) const COMMON_HEADER_SIZE: usize = 4 + 1 + 1;

// 1 other byte for frag_meta
pub (crate) const FRAG_ADD_HEADER_SIZE: usize = 1;

pub (crate) const PACKET_DATA_START_BYTE: usize = CRC32_SIZE + COMMON_HEADER_SIZE;

pub (crate) const FRAG_DATA_START_BYTE: usize = PACKET_DATA_START_BYTE + FRAG_ADD_HEADER_SIZE;

// 1024 + 128 = 1152 is an arbitrary value below most common MTU values
// since the baseline is around 1400, 1280 for the "inner" message + udp message header of 10 bytes
// We need to take ino account ipv4 headers (max of 60bytes) and udp headers (8 bytes)
// 1152 + 60 + 8 = 1220 is not too bad, because the "common" MTU for ipv6 is 1280.
// Although we arguably could do better. Needs tweaking & testing if changed to a higher value.
pub (crate) const MAX_UDP_MESSAGE_SIZE: usize = 1024 + 128 + FRAG_DATA_START_BYTE;

// Since the max is 255, we can have at most 256 frags in a message.
pub (crate) const MAX_FRAGMENTS_IN_MESSAGE: usize = 256;

/// Number of iterations we must wait to send the next ack since the last one.
pub (crate) const ACK_SEND_INTERVAL: u64 = 5;

/// Number of iterations we must wait to resend a packet since the last one, if we haven't received a ack.
pub (crate) const PACKET_RESEND_INTERVAL: u64 = ACK_SEND_INTERVAL * 2;