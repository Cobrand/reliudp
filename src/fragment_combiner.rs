use fnv::FnvHashMap as HashMap;
use std::collections::VecDeque;
use itertools::Itertools;
use ack::{Acks, Ack};
use fragment::{Fragment, build_data_from_fragments};
#[cfg(test)]
use fragment::FragmentMeta;

#[derive(Debug)]
pub (crate) enum FragmentSetState<B: AsRef<[u8]> + 'static> {
    Incomplete {
        fragments: HashMap<u8, Fragment<B>>,
    },
    /// (iteration_n of completion, n of fragments)
    Complete(u64, u8)
}

/// Represents fragments for a given seq_id
#[derive(Debug)]
pub (crate) struct FragmentSet<B: AsRef<[u8]> + 'static> {
    pub (crate) seq_id: u32,

    pub (crate) state: FragmentSetState<B>,
    /// Id of the last iteration we sent an ack for this FragmentSet
    pub (crate) last_sent_ack_iteration: Option<u64>,
    /// Acks sent since last update. Resets whenver new fragments are received.
    pub (crate) acks_sent_count: u32,
}

impl<B: AsRef<[u8]> + 'static> FragmentSet<B> {
    /// Panic is the state is ALREADY complete
    pub (crate) fn complete(&mut self, iteration_n: u64) -> HashMap<u8, Fragment<B>> {
        // frag_total is set to 0 at first, but is modified right after. It could e any number for all we care.
        let old_state = ::std::mem::replace(&mut self.state, FragmentSetState::Complete(iteration_n, 0));
        if let FragmentSetState::Incomplete { fragments } = old_state {
            self.reset_ack_sent_count();
            if let FragmentSetState::Complete(_, ref mut frag_total) = &mut self.state {
                *frag_total = fragments.len() as u8
            } else {
                unreachable!()
            };
            fragments
        } else {
            panic!("seq_id {} has already been completed", self.seq_id)
        }
    }
    
    pub (crate) fn with_capacity(seq_id: u32, frag_total: usize) -> FragmentSet<B> {
        FragmentSet {
            seq_id,
            state: FragmentSetState::Incomplete { fragments: HashMap::with_capacity_and_hasher(frag_total, Default::default()) },
            last_sent_ack_iteration: None,
            acks_sent_count: 0,
        }
    }

    pub (crate) fn generate_ack(&self) -> Ack<Box<[u8]>> {
        match &self.state {
            FragmentSetState::Complete(_, frag_total) => {
                Ack::create_complete(*frag_total)
            },
            FragmentSetState::Incomplete { fragments } => {
                let frag_total = fragments.values().next().unwrap().frag_total;
                let frag_ids_iter = fragments.keys().cloned();
                Ack::create_from_frag_ids(frag_ids_iter, frag_total)
            },
        }
    }

    pub (crate) fn send_ack(&mut self, iteration_n: u64) {
        self.last_sent_ack_iteration = Some(iteration_n);
        self.acks_sent_count += 1;
    }

    pub (crate) fn reset_ack_sent_count(&mut self) {
        self.last_sent_ack_iteration = None;
        self.acks_sent_count = 0;
    }

    pub (crate) fn is_incomplete(&self) -> bool {
        if let FragmentSetState::Incomplete { .. } = self.state {
            true
        } else {
            false
        }
    }
}

#[derive(Debug)]
pub struct FragmentCombiner<B: AsRef<[u8]> + 'static> {
    // TODO: Against DOS attacks, we should make this a VecDeque of small size and get rid
    // of the old stuff automatically.
    pub (crate) pending_fragments: HashMap<u32, FragmentSet<B>>,

    // (seq_id, data)
    pub (crate) out_messages: VecDeque<(u32, Box<[u8]>)>,
}

impl<B: AsRef<[u8]> + 'static> FragmentCombiner<B> {
    pub fn new() -> Self {
        FragmentCombiner {
            pending_fragments: HashMap::default(),
            out_messages: VecDeque::new(),
        }
    }

    /// Removes the HashMap for key `seq_id`, an tries to create a message out of that.
    ///
    /// Panics if there is no HashMap at `seq_id`, or if the message is already complete
    ///
    /// Returns an Error if all the fragments do not have the same frag_total,
    /// or if "build_message_from_fragments" encountered an error
    fn transform_message(&mut self, seq_id: u32, iteration_n: u64) -> Result<(), ()> {
        if let Some(fragment_set) = self.pending_fragments.get_mut(&seq_id) {

            let fragments = fragment_set.complete(iteration_n);
            if !fragments.values().map(|f| f.frag_total).all_equal() {
                // some fragments don't have the same frag_total
                // TODO: remove set, because it is corrupted
                return Err(())
            }
            let message = build_data_from_fragments(fragments.into_iter().map(|(_k, v)| v))?;

            // build_data_from_fragments with an IntoIterator with just the values
            self.out_messages.push_back((seq_id, message));
            Ok(())
        } else {
            panic!("seq_id {} does not exist in fragment_combiner.fragments", seq_id);
        }
    }

    pub fn next_out_message(&mut self) -> Option<(u32, Box<[u8]>)> {
        self.out_messages.pop_front()
    }

    /// Push a fragment into the internal queue.
    ///
    /// If the fragment is the last to arrive
    pub fn push(&mut self, fragment: Fragment<B>, iteration_n: u64) {
        let seq_id = fragment.seq_id;
        let frag_total = fragment.frag_total;

        let try_transform = { 
            let entry = self.pending_fragments.entry(seq_id);

            // if the hashmap doesn't exist, create an empty one
            let fragment_set = entry.or_insert_with(|| {
                FragmentSet::with_capacity(seq_id, frag_total as usize)
            });

            if fragment_set.is_incomplete() {
                fragment_set.acks_sent_count = 0;
            }

            // if the seq_id/frag_id combo already existed, override it. It can happen when the sender re-sends a packet we've already received
            // because it didn't receive the ack on time.
            if let FragmentSetState::Incomplete { ref mut fragments } = fragment_set.state {
                fragments.insert(fragment.frag_id, fragment);
                // try to transform fragments into a message, because we have enough of them here
                // if len() > frag_total + 1, that means that there are too many messages!
                // This can only happen when a packet "lied" about its frag_total.
                // If we try to re-build the message here, we will get an error because all of the fragments
                // don't have the same frag_total, but we still return true to "clear" the queue.
                fragments.len() >= frag_total as usize + 1
            } else {
                // We are trying to push a dragment to something that is already complete.
                // So let's do nothing instead.
                false
            }
        };

        if try_transform {
            let _r = self.transform_message(seq_id, iteration_n);
            // failures to transform messages are ignored. Logging may be an option here TODO
        }
    }

    pub (crate) fn tick(&mut self, iteration_n: u64) -> Acks<Box<[u8]>> {
        let mut acks_to_send = Acks::new();
        let mut acks_to_remove: Vec<u32> = Vec::new();
        for (seq_id, mut fragment_set) in &mut self.pending_fragments {
            if fragment_set.acks_sent_count >= 10 {
                acks_to_remove.push(*seq_id);
                continue;
            }
            let should_send: bool = match fragment_set.last_sent_ack_iteration {
                Some(last_iter) => {
                    debug_assert!(iteration_n > last_iter);
                    iteration_n - last_iter >= ::consts::ACK_SEND_INTERVAL
                },
                // if there are no previous recordings of an ack being sent, send it right away
                None => true,
            };
            if should_send {
                acks_to_send.push((*seq_id, fragment_set.generate_ack()));
                fragment_set.send_ack(iteration_n);
            }
        }
        for seq_id in acks_to_remove {
            self.pending_fragments.remove(&seq_id);
        }
        acks_to_send
    }
}

#[test]
fn fragment_combiner_success() {
    let fragments: Vec<Fragment<Box<[u8]>>> = vec![
        Fragment { seq_id: 3, frag_id: 1, frag_total: 2, frag_meta: FragmentMeta::Key, data: Box::new([0, 5]) },
        Fragment { seq_id: 4, frag_id: 1, frag_total: 2, frag_meta: FragmentMeta::Key, data: Box::new([4, 0]) },
        Fragment { seq_id: 7, frag_id: 0, frag_total: 0, frag_meta: FragmentMeta::Key, data: Box::new([64, 64]) },
        Fragment { seq_id: 5, frag_id: 1, frag_total: 2, frag_meta: FragmentMeta::Key, data: Box::new([4, 5]) },
        Fragment { seq_id: 5, frag_id: 0, frag_total: 2, frag_meta: FragmentMeta::Key, data: Box::new([1, 2, 3]) },
        Fragment { seq_id: 5, frag_id: 2, frag_total: 2, frag_meta: FragmentMeta::Key, data: Box::new([6, 7, 8, 9]) },
        Fragment { seq_id: 6, frag_id: 1, frag_total: 2, frag_meta: FragmentMeta::Key, data: Box::new([14, 5]) },
    ];
    let mut fragment_combiner = FragmentCombiner::new();
    for fragment in fragments {
        fragment_combiner.push(fragment, 0);
    }

    let out_message = fragment_combiner.next_out_message().unwrap();
    assert_eq!(out_message.1.as_ref(), &[64, 64]);
    let out_message = fragment_combiner.next_out_message().unwrap();
    assert_eq!(out_message.1.as_ref(), &[1, 2, 3, 4, 5, 6, 7, 8, 9]);
}