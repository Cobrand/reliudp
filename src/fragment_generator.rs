use fragment::{Fragment, FragmentMeta};

pub struct FragmentGenerator<'a, I> where I: Iterator<Item = &'a [u8]> + Clone {
    seq_id: u32,
    frag_total: u8,
    next_frag: u8,
    frag_meta: FragmentMeta,
    iterator: I
}

impl<'a, I> FragmentGenerator<'a, I> where I: Iterator<Item = &'a [u8]> + Clone {
    pub fn new(iterator: I, seq_id: u32, frag_total: u8, frag_meta: FragmentMeta) -> Self {
        FragmentGenerator {
            seq_id,
            frag_total,
            iterator,
            frag_meta,
            next_frag: 0,
        }
    }
}

impl<'a, I: Iterator<Item = &'a [u8]> + Clone> Iterator for FragmentGenerator<'a, I> {
    type Item = Fragment<&'a [u8]>;
    fn next(&mut self) -> Option<Self::Item> {
        let data = self.iterator.next();
        data.map(|data| {
            let current_frag = self.next_frag;
            self.next_frag += 1;
            Fragment {
                seq_id: self.seq_id,
                frag_total: self.frag_total,
                frag_id: current_frag,
                frag_meta: self.frag_meta,
                data,
            }
        })
    }
}

impl<'a, I: Iterator<Item = &'a [u8]> + Clone> Clone for FragmentGenerator<'a, I> {
    fn clone(&self) -> Self {
        FragmentGenerator {
            seq_id: self.seq_id,
            next_frag: self.next_frag,
            frag_total: self.frag_total,
            frag_meta: self.frag_meta,
            iterator: self.iterator.clone(),
        }
    }
}