use misc::ClonableIterator;
use consts::*;
use fragment_generator::FragmentGenerator;

const MAX_FRAGMENT_MESSAGE_SIZE: usize = MAX_UDP_MESSAGE_SIZE - FRAG_DATA_START_BYTE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FragmentMeta {
    Forgettable = 0,
    KeyExpirable = 1,
    Key = 2,
}

/// A fragment is a destructed UdpPacket that can hold at most
///
#[derive(Debug, PartialEq, Eq)]
pub struct Fragment<T: AsRef<[u8]>> {
    pub seq_id: u32,
    pub frag_id: u8,
    // real frag total is +1, meaning that 0 => 1 and 63 => 64
    // so if frag_id = 0 and frag_total = 0, there is only one message and nothing else
    pub frag_total: u8,
    pub frag_meta: FragmentMeta,
    pub data: T
}

impl<T: AsRef<[u8]>> Fragment<T> {
    pub fn as_borrowed_frag<'a>(&'a self) -> Fragment<&'a T> {
        Fragment {
            seq_id: self.seq_id,
            frag_id: self.frag_id,
            frag_total: self.frag_total,
            frag_meta: self.frag_meta,
            data: &self.data,
        }
    }
}

impl<'a> Clone for Fragment<&'a [u8]> {
    fn clone(&self) -> Self {
        Fragment {
            seq_id: self.seq_id,
            frag_id: self.frag_id,
            frag_total: self.frag_total,
            frag_meta: self.frag_meta,
            data: self.data
        }
    }
}

impl<'a> Fragment<&'a [u8]> {
    #[cfg(test)]
    pub fn into_boxed(self) -> Fragment<Box<[u8]>> {
        Fragment {
            seq_id: self.seq_id,
            frag_id: self.frag_id,
            frag_total: self.frag_total,
            frag_meta: self.frag_meta,
            data: Box::from(self.data)
        }
    }
}

/// Restore the data from multiple fragments
///
/// This method accepts an iterator, but the iterator doesn't have to be sorted,
/// sorting is done by this function itself.
///
/// Panics if the number of fragment is not equal to the length of the given Vec
///
/// returns an error if the message couldn't be restored properly: a frag_id is higher than frag_total,
/// 2 frag_id are the same, ...
pub (crate) fn build_data_from_fragments<I, B>(fragments: I) -> Result<Box<[u8]>, ()> 
where   B: AsRef<[u8]> + 'static,
        I: Iterator<Item = Fragment<B>> + ExactSizeIterator {
    // start with vec!(None; n) and for every fragment, replace None by Some(...)
    // it does not matter if the original slice is out of order, this vec will be in order
    // Note that we can't do `= vec!(None; fragments.len())` because Option<Fragment<_>> is not `Clone`
    let mut fragments_vec: Vec<Option<Fragment<B>>> = (0..fragments.len()).map(|_| None).collect();
    // track the size of all data chunks summed
    let mut total_data_size: usize = 0;
    for fragment in fragments {
        let frag_id = fragment.frag_id as usize;
        if frag_id >= fragments_vec.len() || fragments_vec[frag_id].is_some() {
            return Err(())
        };
        total_data_size += fragment.data.as_ref().len();
        fragments_vec[frag_id] = Some(fragment);
    }
    // security check: no None are left, otherwise that means the message is incomplete
    assert!(fragments_vec.iter().all(Option::is_some));
    assert_eq!(usize::from(fragments_vec[0].as_ref().unwrap().frag_total) + 1, fragments_vec.len());

    let mut reassembled_data: Vec<u8> = Vec::with_capacity(total_data_size);
    for o in fragments_vec.iter() {
        // unwrapping is 0 cost here since we assert-ed earlier that all the elements are "is_some"
        let fragment = o.as_ref().unwrap();
        reassembled_data.extend(fragment.data.as_ref());
    };
    Ok(reassembled_data.into_boxed_slice())
}

#[test]
fn build_data_from_fragments_success() {
    let fragments: Vec<Fragment<Box<[u8]>>> = vec![
        Fragment { seq_id: 5, frag_id: 1, frag_total: 2, frag_meta: FragmentMeta::Key, data: Box::new([4, 5]) },
        Fragment { seq_id: 5, frag_id: 0, frag_total: 2, frag_meta: FragmentMeta::Key, data: Box::new([1, 2, 3]) },
        Fragment { seq_id: 5, frag_id: 2, frag_total: 2, frag_meta: FragmentMeta::Key, data: Box::new([6, 7, 8, 9]) },
    ];

    let message: Box<[u8]> = build_data_from_fragments(fragments.into_iter()).unwrap();
    assert_eq!(message.as_ref(), &[1u8, 2, 3, 4, 5, 6, 7, 8, 9]);
}

#[test]
#[should_panic]
fn build_data_from_fragments_fail_wrong_frag_total() {
    let fragments: Vec<Fragment<Box<[u8]>>> = vec![
        Fragment { seq_id: 5, frag_id: 1, frag_total: 3, frag_meta: FragmentMeta::Key, data: Box::new([4, 5]) },
        Fragment { seq_id: 5, frag_id: 0, frag_total: 3, frag_meta: FragmentMeta::Key, data: Box::new([1, 2, 3]) },
        Fragment { seq_id: 5, frag_id: 2, frag_total: 3, frag_meta: FragmentMeta::Key, data: Box::new([6, 7, 8, 9]) },
    ];

    build_data_from_fragments(fragments.into_iter()).unwrap();
}

#[test]
fn build_data_from_fragments_fail_wrong_frag_id() {
    let fragments: Vec<Fragment<Box<[u8]>>> = vec![
        Fragment { seq_id: 5, frag_id: 0, frag_total: 1, frag_meta: FragmentMeta::Key, data: Box::new([1, 2, 3]) },
        Fragment { seq_id: 5, frag_id: 5, frag_total: 1, frag_meta: FragmentMeta::Key, data: Box::new([6, 7, 8, 9]) },
    ];

    let e = build_data_from_fragments(fragments.into_iter()).unwrap_err();
    assert_eq!(e, ());
}

#[test]
fn build_data_from_fragments_fail_duplicate_frag_id() {
    let fragments: Vec<Fragment<Box<[u8]>>> = vec![
        Fragment { seq_id: 5, frag_id: 0, frag_total: 1, frag_meta: FragmentMeta::Key, data: Box::new([1, 2, 3]) },
        Fragment { seq_id: 5, frag_id: 0, frag_total: 1, frag_meta: FragmentMeta::Key, data: Box::new([6, 7, 8, 9]) },
    ];

    let e = build_data_from_fragments(fragments.into_iter()).unwrap_err();
    assert_eq!(e, ());
}

pub (crate) fn build_fragments_from_bytes<'a>(data: &'a [u8], seq_id: u32, frag_meta: FragmentMeta) -> Result<(Box<'a + ClonableIterator<Item = Fragment<&[u8]>>>, u8), ()> {
    if data.is_empty() {
        panic!("build_fragments_from_data cannot build fragments if the message is empty");
    }

    let mut fragments_count = data.len() / MAX_FRAGMENT_MESSAGE_SIZE;
    if data.len() % MAX_FRAGMENT_MESSAGE_SIZE != 0 {
        // if we can fix message into boxes exactly that's great! otherwise it means that there is a left-over,
        // and we should build the left over accordingly as well.
        fragments_count += 1;
    }
    debug_assert!(fragments_count > 0, "number of fragments to build cannot be 0");
    if fragments_count > MAX_FRAGMENTS_IN_MESSAGE {
        return Err(())
    }
    let frag_total = (fragments_count - 1) as u8;
    let iter = data.chunks(MAX_FRAGMENT_MESSAGE_SIZE);
    Ok((Box::new(FragmentGenerator::new(iter, seq_id, frag_total, frag_meta)), frag_total))
}

#[test]
fn build_rebuild_data() {
    let seq_id: u32 = 1;
    let data = vec!(0; 1024);
    let (frags_iter_boxed, _frag_total) = build_fragments_from_bytes(data.as_ref(), seq_id, FragmentMeta::Key).unwrap();
    let frags: Vec<Fragment<Box<[u8]>>> = frags_iter_boxed.map(|f| f.into_boxed()).collect();
    let new_data = build_data_from_fragments(frags.into_iter()).unwrap();
    assert_eq!(new_data.len(), data.len());
}

#[test]
fn build_one_frag_from_data() {
    let seq_id: u32 = 1;
    let data = vec!(0; 1024);
    let (mut frags_iter, frag_total) = build_fragments_from_bytes(data.as_ref(), seq_id, FragmentMeta::KeyExpirable).unwrap();
    let frag = frags_iter.next().unwrap();
    assert!(frags_iter.next().is_none()); 
    assert_eq!(frag.data.len(), 1024);
    assert_eq!(frag.seq_id, seq_id);
    assert_eq!(frag.frag_id, 0);
    assert_eq!(frag.frag_meta, FragmentMeta::KeyExpirable);
    assert_eq!(frag.frag_total, 0);
    assert_eq!(frag_total, 0);
}

#[test]
fn build_multiple_frags_from_data() {
    let seq_id: u32 = 1;
    let data = vec!(0; 2048);
    let (mut frags_iter, frag_total) = build_fragments_from_bytes(data.as_ref(), seq_id, FragmentMeta::KeyExpirable).unwrap();
    let frag_1 = frags_iter.next().unwrap();
    let frag_2 = frags_iter.next().unwrap();
    assert!(frags_iter.next().is_none()); 
    assert_eq!(frag_1.data.len(), MAX_FRAGMENT_MESSAGE_SIZE);
    assert_eq!(frag_2.data.len(), 2048 - MAX_FRAGMENT_MESSAGE_SIZE);
    assert_eq!(frag_1.seq_id, seq_id);
    assert_eq!(frag_2.seq_id, seq_id);
    assert_eq!(frag_1.frag_id, 0);
    assert_eq!(frag_2.frag_id, 1);
    assert_eq!(frag_1.frag_total, 1);
    assert_eq!(frag_2.frag_total, 1);
    assert_eq!(frag_total, 1);
}

#[test]
fn build_frags_from_data_fail() {
    let seq_id: u32 = 1;
    let data = vec!(0; MAX_FRAGMENTS_IN_MESSAGE * MAX_FRAGMENT_MESSAGE_SIZE + 1);
    assert!(build_fragments_from_bytes(data.as_ref(), seq_id, FragmentMeta::KeyExpirable).is_err());
}