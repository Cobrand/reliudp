use crate::udp_packet::*;
use crate::fragment_combiner::*;
use crate::misc::BoxedSlice;
use std::collections::VecDeque;
use crate::ack::Acks;
use std::time::Instant;

#[derive(Debug)]
pub (crate) enum ReceivedMessage {
    Ack(u32, BoxedSlice<u8>),
    Data(u32, Box<[u8]>),
    Syn,
    SynAck,
    Heartbeat,
    End(u32),
    Abort(u32),
    // impossible to decode, so return the raw message
    Raw(Box<[u8]>),
}

#[derive(Debug)]
pub (crate) struct UdpPacketHandler {
    fragment_combiner: FragmentCombiner<BoxedSlice<u8>>,
    
    out_messages: VecDeque<ReceivedMessage>,
}

impl UdpPacketHandler {
    pub fn new() -> Self {
        UdpPacketHandler {
            fragment_combiner: FragmentCombiner::new(),
            out_messages: VecDeque::with_capacity(32),
        }
    }

    pub (crate) fn add_received_packet(&mut self, udp_packet: UdpPacket<Box<[u8]>>, now: Instant) {
        match udp_packet.compute_packet() {
            Ok(Packet::Fragment(f)) => {
                log::trace!("received fragment {:?}", f);
                self.fragment_combiner.push(f, now);
                if let Some((seq_id, data)) = self.fragment_combiner.next_out_message() {
                    self.out_messages.push_back(ReceivedMessage::Data(seq_id, data));
                }
            },
            Ok(Packet::Ack(seq_id, data)) => {
                log::trace!("received ack({}) {:?}", seq_id, data);
                self.out_messages.push_back(ReceivedMessage::Ack(seq_id, data));
            },
            Ok(Packet::Heartbeat) => {
                log::trace!("received heartbeat");
                self.out_messages.push_back(ReceivedMessage::Heartbeat);
            },
            Ok(Packet::Syn) => {
                log::trace!("received Syn");
                self.out_messages.push_back(ReceivedMessage::Syn);
            },
            Ok(Packet::SynAck) => {
                log::trace!("received SynAck");
                self.out_messages.push_back(ReceivedMessage::SynAck);
            },
            Ok(Packet::End(last_seq_id)) => {
                log::trace!("received End({})", last_seq_id);
                self.out_messages.push_back(ReceivedMessage::End(last_seq_id));
            },
            Ok(Packet::Abort(last_seq_id)) => {
                log::trace!("received Abort({})", last_seq_id);
                self.out_messages.push_back(ReceivedMessage::Abort(last_seq_id));
            },
            Err((_e, data)) => {
                // errors are not ignored, but simply transferred as raw packets to the user
                self.out_messages.push_back(ReceivedMessage::Raw(data.buffer));
            }
        };
    }

    /// Should be called every "tick", whatever you choose your tick to be.
    #[inline]
    pub (crate) fn tick(&mut self, now: Instant) -> Acks<Box<[u8]>> {
        self.fragment_combiner.tick(now)
    }
    
    pub (crate) fn next_received_message(&mut self) -> Option<ReceivedMessage> {
        self.out_messages.pop_front()
    }
}