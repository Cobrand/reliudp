use fnv::FnvHashMap as HashMap;
use rudp::UdpSocketWrapper;
use fragment::{build_fragments_from_bytes, FragmentMeta};
use udp_packet::UdpPacket;
use ack::Ack;
use rudp::MessageType;
use misc::BoxedSlice;

use hex::encode as hex_encode;

#[derive(Debug, Clone, Copy)]
pub (crate) enum PacketExpiration {
    Key,
    ExpirableKey {
        expiration_iter_n: u64,
    }
}

impl From<Option<PacketExpiration>> for FragmentMeta {
    fn from(packet_expiration: Option<PacketExpiration>) -> Self {
        match packet_expiration {
            None => FragmentMeta::Forgettable,
            Some(PacketExpiration::Key) => FragmentMeta::Key,
            Some(PacketExpiration::ExpirableKey { .. }) => FragmentMeta::KeyExpirable,
        }
    }
}

impl PacketExpiration {
    fn from_message_type(message_type: MessageType, iteration_n: u64) -> Option<PacketExpiration> {
        match message_type {
            MessageType::Forgettable => None,
            MessageType::KeyExpirableMessage(v) => Some(PacketExpiration::ExpirableKey {
                expiration_iter_n: u64::from(v).saturating_add(iteration_n)
            }),
            MessageType::KeyMessage => Some(PacketExpiration::Key),
        }
    }
}

pub (self) struct SentDataSet<D: AsRef<[u8]> + 'static + Clone> {
    pub (self) data: D,
    pub (self) frag_total: u8,
    pub (self) expiration_type: PacketExpiration,
    /// (iteration_n, ack_data)
    pub (self) last_received_ack: Option<(u64, Ack<BoxedSlice<u8>>)>,
    pub (self) last_sent_packet: u64,
}

impl<D: AsRef<[u8]> + 'static + Clone> ::std::fmt::Debug for SentDataSet<D> {
    fn fmt(&self, f: &mut ::std::fmt::Formatter) -> Result<(), ::std::fmt::Error> {
        let data = hex_encode(&self.data);
        write!(f, "SentDataSet {{ frag_total: {}, expiration_type: {:?}, last_received_ack: {:?}, last_sent_packet: {:?}, data: [hex 0x{}] }}",
            self.frag_total,
            self.expiration_type,
            self.last_received_ack,
            self.last_sent_packet,
            data
        )
    }
}

impl<D: AsRef<[u8]> + 'static + Clone> SentDataSet<D> {
    pub fn new(data: D, frag_total: u8, iteration_n: u64, expiration_type: PacketExpiration) -> SentDataSet<D> {
        SentDataSet {
            data,
            frag_total,
            expiration_type,
            last_received_ack: None,
            last_sent_packet: iteration_n,
        }
    }

    /// Returns whether or not all acks have been received by the other party
    pub (self) fn attempt_resend_packets(&mut self, seq_id: u32, iteration_n: u64, socket: &UdpSocketWrapper) -> bool {
        if self.last_sent_packet + ::consts::PACKET_RESEND_INTERVAL >= iteration_n {
            self.resend_packets(seq_id, iteration_n, socket)
        } else {
            false
        }
    }

    #[inline]
    pub fn is_expired(&self, iteration_n: u64) -> bool {
        match self.expiration_type {
            PacketExpiration::ExpirableKey { expiration_iter_n } =>
                iteration_n > expiration_iter_n,
            _ => false,
        }
    }

    /// Returns whether or not all acks have been received by the other party
    pub (self) fn resend_packets(&mut self, seq_id: u32, iteration_n: u64, socket: &UdpSocketWrapper) -> bool {
        let frag_meta = FragmentMeta::from(Some(self.expiration_type));
        let (fragments, frag_total) = build_fragments_from_bytes(self.data.as_ref(), seq_id, frag_meta).expect("Unreachable: message has been sent once but couldn't be resent because too big");
        let complete = match &self.last_received_ack {
            Some((_ack_iteration_n, ack)) => {
                let all_fragments: Vec<_> = fragments.collect();
                debug_assert!(! all_fragments.is_empty());
                debug_assert_eq!((all_fragments.len() - 1) as u8, self.frag_total);
                debug_assert_eq!(frag_total, self.frag_total);
                let ack_missing_frags = ack.missing_iter(frag_total);

                // variable storing whether or not every ack is "ok"
                let mut complete = true;
                for frag_id in ack_missing_frags {
                    complete = false;
                    let fragment = &all_fragments[frag_id as usize];
                    let _r = socket.send_udp_packet(&UdpPacket::from(fragment));
                    // TODO log the error if any
                }
                complete
            },
            None => {
                // no ack has been received, resend everything we have
                for fragment in fragments {
                    let _r = socket.send_udp_packet(&UdpPacket::from(&fragment));
                    // TODO log the error if any
                }

                // obviously no acks have been received, so this set can't be complete
                false
            },
        };
        self.last_sent_packet = iteration_n;
        complete
    } 
}

#[derive(Debug)]
pub (crate) struct SentDataTracker<D: AsRef<[u8]> + 'static + Clone> {
    pub (self) sets: HashMap<u32, SentDataSet<D>>,
}

impl<D: AsRef<[u8]> + 'static + Clone> SentDataTracker<D> {
    pub fn new() -> SentDataTracker<D> {
        SentDataTracker {
            sets: Default::default(),
        }
    }

    pub fn send_data(&mut self, seq_id: u32, data: D, iteration_n: u64, message_type: MessageType, socket: &UdpSocketWrapper) {
        let expiration = PacketExpiration::from_message_type(message_type, iteration_n);
        let (fragments, frag_total) = build_fragments_from_bytes(data.as_ref(), seq_id, FragmentMeta::from(expiration)).expect("Your message is too big to be sent via RUDP.");
        for fragment in fragments {
            let _r = socket.send_udp_packet(&UdpPacket::from(&fragment));
            // TODO log the error if any
        }

        if let Some(packet_expiration) = expiration {
            let sent_data_set = SentDataSet::new(data.clone(), frag_total, iteration_n, packet_expiration);

            if self.sets.insert(seq_id, sent_data_set).is_some() {
                panic!("seq_id {:?} is already registered in sent_data_tracker", seq_id);
            }
        }
    }

    pub fn receive_ack(&mut self, seq_id: u32, data: BoxedSlice<u8>, iteration_n: u64, socket: &UdpSocketWrapper) {
        if let Some(set) = self.sets.get_mut(&seq_id) {
            let ack = Ack::new(data);
            set.last_received_ack = Some((iteration_n, ack));
            set.resend_packets(seq_id, iteration_n, socket);
        } else {
            // couldn't find the matching fragment set... 2 possibilities:
            // * The remote lied, we never had such a seq_id
            // * We dropped the message on our end, so we can't even try to recover it 
            // in either case, the only thing we can do is to drop the ack and give up on life.
        }
    }

    pub fn next_tick(&mut self, iteration_n: u64, socket: &UdpSocketWrapper) {
        let mut entries_to_remove: Vec<_> = vec!();
        for (seq_id, ref mut set) in &mut self.sets {
            if set.is_expired(iteration_n) {
                entries_to_remove.push(*seq_id);
                continue;
            }
            let ack_received = set.attempt_resend_packets(*seq_id, iteration_n, socket);
            if ack_received {
                entries_to_remove.push(*seq_id);
            }
        }
        for seq_id in entries_to_remove {
            if self.sets.remove(&seq_id).is_none() {
                panic!("removing entry seq_id={} that doesn't exist... this is a bug", seq_id);
            }
        }
    }
}