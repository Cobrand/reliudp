use udp_packet::*;
use fragment_combiner::*;
use misc::BoxedSlice;
use std::collections::VecDeque;
use ack::Acks;

#[derive(Debug)]
pub (crate) enum ReceivedMessage {
    Ack(u32, BoxedSlice<u8>),
    Data(u32, Box<[u8]>),
    Syn,
    SynAck,
    Heartbeat,
    End(u32),
    Abort(u32),
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

    pub (crate) fn add_received_packet(&mut self, udp_packet: UdpPacket<Box<[u8]>>, iteration_n: u64) {
        match udp_packet.compute_packet() {
            Ok(Packet::Fragment(f)) => {
                log::trace!("received fragment {:?} at n={}", f, iteration_n);
                self.fragment_combiner.push(f, iteration_n);
                if let Some((seq_id, data)) = self.fragment_combiner.next_out_message() {
                    self.out_messages.push_back(ReceivedMessage::Data(seq_id, data));
                }
            },
            Ok(Packet::Ack(seq_id, data)) => {
                log::trace!("received ack({}) {:?} at n={}", seq_id, data, iteration_n);
                self.out_messages.push_back(ReceivedMessage::Ack(seq_id, data));
            },
            Ok(Packet::Heartbeat) => {
                log::trace!("received heartbeat at n={}", iteration_n);
                self.out_messages.push_back(ReceivedMessage::Heartbeat);
            },
            Ok(Packet::Syn) => {
                log::trace!("received Syn at n={}", iteration_n);
                self.out_messages.push_back(ReceivedMessage::Syn);
            },
            Ok(Packet::SynAck) => {
                log::trace!("received SynAck at n={}", iteration_n);
                self.out_messages.push_back(ReceivedMessage::SynAck);
            },
            Ok(Packet::End(last_seq_id)) => {
                log::trace!("received End({}) at n={}", last_seq_id, iteration_n);
                self.out_messages.push_back(ReceivedMessage::End(last_seq_id));
            },
            Ok(Packet::Abort(last_seq_id)) => {
                log::trace!("received Abort({}) at n={}", last_seq_id, iteration_n);
                self.out_messages.push_back(ReceivedMessage::Abort(last_seq_id));
            },
            Err(_) => { /* ignore errors */ }
        };
    }

    /// Should be called every "tick", whatever you choose your tick to be.
    #[inline]
    pub (crate) fn tick(&mut self, iteration_n: u64) -> Acks<Box<[u8]>> {
        self.fragment_combiner.tick(iteration_n)
    }
    
    pub (crate) fn next_received_message(&mut self) -> Option<ReceivedMessage> {
        self.out_messages.pop_front()
    }
}