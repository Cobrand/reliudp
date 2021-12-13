use hashbrown::HashMap;
use crate::rudp::UdpSocketWrapper;
use crate::fragment::{build_fragments_from_bytes, FragmentMeta};
use crate::udp_packet::UdpPacket;
use crate::ack::Ack;
use crate::rudp::{MessageType, MessagePriority};
use crate::misc::BoxedSlice;
use crate::consts::SEQ_DATA_CLEANUP_DELAY;
use std::time::Instant;

#[cfg(feature = "extended_debug")]
use hex::encode as hex_encode;

#[derive(Debug, Clone, Copy)]
pub (crate) enum PacketExpiration {
    Key,
    ExpirableKey {
        expiration: Instant,
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
    fn from_message_type(message_type: MessageType, now: Instant) -> Option<PacketExpiration> {
        match message_type {
            MessageType::Forgettable => None,
            MessageType::KeyExpirableMessage(v) => Some(PacketExpiration::ExpirableKey {
                expiration: now + v,
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
    pub (self) last_received_ack: Option<(Instant, Ack<BoxedSlice<u8>>)>,
    pub (self) last_sent_packet: Instant,

    pub (self) complete_since: Option<Instant>,
    /// (Oldest unanswered ack, Newest unanswered ack)
    pub (self) unanswered_ack: Option<(Instant, Instant)>,
    pub (self) message_priority: MessagePriority,
}

#[cfg(feature = "extended_debug")]
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

#[cfg(not(feature = "extended_debug"))]
impl<D: AsRef<[u8]> + 'static + Clone> ::std::fmt::Debug for SentDataSet<D> {
    fn fmt(&self, f: &mut ::std::fmt::Formatter) -> Result<(), ::std::fmt::Error> {
        let len = self.data.as_ref().len();
        write!(f, "SentDataSet {{ frag_total: {}, expiration_type: {:?}, last_received_ack: {:?}, last_sent_packet: {:?}, data: {} bytes }}",
            self.frag_total,
            self.expiration_type,
            self.last_received_ack,
            self.last_sent_packet,
            len
        )
    }
}

impl<D: AsRef<[u8]> + 'static + Clone> SentDataSet<D> {
    pub fn new(data: D, frag_total: u8, now: Instant, expiration_type: PacketExpiration, message_priority: MessagePriority) -> SentDataSet<D> {
        SentDataSet {
            data,
            frag_total,
            expiration_type,
            last_received_ack: None,
            last_sent_packet: now,
            unanswered_ack: None,
            complete_since: None,
            message_priority,
        }
    }

    /// Returns since when the remote party has received all acks.
    ///
    /// None means the remote has not received the message yet (as of what we know)
    /// Some(instant) is the time when the first complete ack has been received
    pub (self) fn attempt_resend_packets(&mut self, seq_id: u32, now: Instant, socket: &UdpSocketWrapper) -> Option<Instant> {
        let resend_delay = self.message_priority.resend_delay();
        if now >= self.last_sent_packet + resend_delay {
            self.resend_packets(seq_id, now, socket)
        } else {
            if let Some((old, new)) = self.unanswered_ack {
                // if we have received an unanswered ack 80% of resend_delay ago,
                // OR if we have NOT received an ack for 60% of resend_delay, resend the packets
                if now >= old + resend_delay * 4 / 5 || now - new >= resend_delay * 3 / 5 {
                    self.resend_packets(seq_id, now, socket)
                } else {
                    None
                }
            } else {
                None
            }
        }
    }

    #[inline]
    pub fn is_expired(&self, now: Instant) -> bool {
        match self.expiration_type {
            PacketExpiration::ExpirableKey { expiration } =>
                now > expiration,
            _ => false,
        }
    }

    /// Returns whether or not all acks have been received by the other party
    pub (self) fn resend_packets(&mut self, seq_id: u32, now: Instant, socket: &UdpSocketWrapper) -> Option<Instant> {
        let frag_meta = FragmentMeta::from(Some(self.expiration_type));
        let (fragments, frag_total) = build_fragments_from_bytes(self.data.as_ref(), seq_id, frag_meta).expect("Unreachable: message has been sent once but couldn't be resent because too big");
        
        let mut last_complete_ack: Option<Instant> = None;
        match &self.last_received_ack {
            Some((ack_received_instant, ack)) => {
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
                    log::trace!("resending seq_id={} frag_id={} because we received incomplete ack", seq_id, frag_id);
                    let _r = socket.send_udp_packet(&UdpPacket::from(fragment));
                    // TODO log the error if any
                }
                if complete {
                    last_complete_ack = Some(*ack_received_instant);
                }
            },
            None => {
                // no ack has been received, resend everything we have
                for fragment in fragments {
                    log::trace!("resending seq_id={} frag_id={} because we received no ack", seq_id, fragment.frag_id);
                    let _r = socket.send_udp_packet(&UdpPacket::from(&fragment));
                    // TODO log the error if any
                }

                // obviously no acks have been received, so this set can't be complete, so don't set "last_received_ack"
            },
        };
        self.unanswered_ack = None;
        self.last_sent_packet = now;
        last_complete_ack
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

    pub fn send_data(&mut self, seq_id: u32, data: D, now: Instant, message_type: MessageType, message_priority: MessagePriority, socket: &UdpSocketWrapper) {
        let expiration = PacketExpiration::from_message_type(message_type, now);
        let (fragments, frag_total) = build_fragments_from_bytes(data.as_ref(), seq_id, FragmentMeta::from(expiration)).expect("Your message is too big to be sent via RUDP.");
        for fragment in fragments {
            let _r = socket.send_udp_packet(&UdpPacket::from(&fragment));
            // TODO log the error if any
        }

        if let Some(packet_expiration) = expiration {
            let sent_data_set = SentDataSet::new(data.clone(), frag_total, now, packet_expiration, message_priority);

            if self.sets.insert(seq_id, sent_data_set).is_some() {
                panic!("seq_id {:?} is already registered in sent_data_tracker", seq_id);
            }
        }
    }

    fn remove_seq_id(&mut self, seq_id: u32) {
        self.sets.remove(&seq_id);
    }

    pub fn is_seq_id_received(&self, seq_id: u32) -> Result<bool, ()> {
        match self.sets.get(&seq_id) {
            None => Err(()),
            Some(set) => Ok(set.complete_since.is_some())
        }
    }

    pub fn receive_ack(&mut self, seq_id: u32, data: BoxedSlice<u8>, now: Instant) {
        if let Some(set) = self.sets.get_mut(&seq_id) {
            let ack = Ack::new(data);
            set.last_received_ack = Some((now, ack));
            match set.unanswered_ack {
                Some((old, _)) => {
                    set.unanswered_ack = Some((old, now))
                },
                None => {
                    set.unanswered_ack = Some((now, now))
                }
            };
        } else {
            // couldn't find the matching fragment set... 2 possibilities:
            // * The remote lied, we never had such a seq_id
            // * We dropped the message on our end, so we can't even try to recover it 
            // in either case, the only thing we can do is to drop the ack and give up on life.
        };
        // if remove_ack {
        //     self.remove_seq_id(seq_id);
        // }
    }

    /// Clears data that is too old to be stored here (acks missing a part taht are too old, ...)
    pub fn next_tick(&mut self, now: Instant, socket: &UdpSocketWrapper) {
        let mut entries_to_remove: Vec<_> = vec!();
        for (seq_id, ref mut set) in &mut self.sets {
            if set.is_expired(now) {
                entries_to_remove.push(*seq_id);
                continue;
            }
            if let Some(complete_time) = set.complete_since {
                let delta = now - complete_time;
                if delta >= SEQ_DATA_CLEANUP_DELAY {
                    entries_to_remove.push(*seq_id);
                }
            } else {
                let ack_received = set.attempt_resend_packets(*seq_id, now, socket);
                if let Some(ack_received) = ack_received {
                    set.complete_since = Some(ack_received);
                }
            }
        }
        for seq_id in entries_to_remove {
            self.remove_seq_id(seq_id);
        }
    }
}