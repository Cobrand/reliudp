

/// (seq_id, Ack)
pub type Acks<D> = Vec<(u32, Ack<D>)>;

#[derive(Debug, Clone)]
pub struct Ack<D: AsRef<[u8]> + 'static>(D);

fn ack_size_from_frag_total(frag_total: u8) -> usize {
    if frag_total % 8 == 0 {
        (frag_total / 8) as usize
    } else {
        (frag_total / 8 + 1) as usize
    }
}

#[cfg(test)]
pub (self) fn frag_ids_received_from_ack<I: Iterator<Item=u8>>(ack_bytes: I, frag_total: u8) -> impl Iterator<Item=u8> {
    ack_bytes.enumerate().flat_map(move |(index, bits): (usize, u8)| {
        (0..8).filter_map(move |bit_index| {
            debug_assert!(index < 32); // 31 * 8 + 7 is max value at most in u8
            let bit = 1 << bit_index;
            if bits & bit > 0 {
                let v: u8 = (index * 8) as u8 + bit_index;
                if v <= frag_total {
                    Some(v)
                } else {
                    None
                }
            } else {
                None
            }
        })
    })
}

pub (self) fn frag_ids_missing_from_ack<'a, I: Iterator<Item=u8> + 'a>(ack_bytes: I, frag_total: u8) -> impl Iterator<Item=u8> + 'a {
    ack_bytes.enumerate().flat_map(move |(index, bits): (usize, u8)| {
        (0..8).filter_map(move |bit_index| {
            debug_assert!(index < 32); // 31 * 8 + 7 is max value at most in u8
            let bit = 1 << bit_index;
            if bits & bit == 0 {
                let v: u8 = (index * 8) as u8 + bit_index;
                if v <= frag_total {
                    Some(v)
                } else {
                    None
                }
            } else {
                None
            }
        })
    })
}

impl Ack<Box<[u8]>> {
    pub (crate) fn create_complete(frag_total: u8) -> Ack<Box<[u8]>> {
        Ack(vec!(0xFFu8; ack_size_from_frag_total(frag_total)).into_boxed_slice())
    }

    pub (crate) fn create_from_frag_ids<I: Iterator<Item=u8>>(iter: I, frag_total: u8) -> Ack<Box<[u8]>> {
        let mut ack = vec!(0x0u8; ack_size_from_frag_total(frag_total));

        // this loop may be totally unoptimized!! If encountering performance issues,
        // please test whether or not this is a good solution!
        for frag_id in iter {
            let byte_index = (frag_id / 8) as usize;
            let bit_index: u8 = frag_id % 8;
            ack[byte_index] |= 1 << bit_index;
        }

        Ack(ack.into_boxed_slice())
    }
}

impl<D: AsRef<[u8]> + 'static> Ack<D> {
    pub (crate) fn new(data: D) -> Ack<D> {
        Ack(data)
    }

    #[cfg(test)]
    pub (crate) fn into_iter(self, frag_total: u8) -> impl Iterator<Item=u8> {
        let v = Vec::from(self.0.as_ref());
        frag_ids_received_from_ack(v.into_iter(), frag_total)
    }

    #[cfg(test)]
    pub (crate) fn into_missing_iter(self, frag_total: u8) -> impl Iterator<Item=u8> {
        let v = Vec::from(self.0.as_ref());
        frag_ids_missing_from_ack(v.into_iter(), frag_total)
    }
    
    pub (crate) fn missing_iter<'a>(&'a self, frag_total: u8) -> impl Iterator<Item=u8> + 'a {
        frag_ids_missing_from_ack(self.0.as_ref().iter().cloned(), frag_total)
    }

    pub fn into_inner(self) -> D {
        self.0
    }
}

#[test]
fn ack_ser() {
    let frag_ids = vec!(1u8, 2u8, 8u8, 9u8);
    let frag_total: u8 = 15;
    let ack = Ack::create_from_frag_ids(frag_ids.iter().cloned(), frag_total);

    assert_eq!(ack.0.as_ref(), &[0b00000110, 0b00000011]);
}

#[test]
fn ack_missing() {
    let frag_ids = vec!(1u8, 2u8, 8u8, 9u8);
    let frag_total: u8 = 15;
    let ack = Ack::create_from_frag_ids(frag_ids.iter().cloned(), frag_total);
    let missing: Vec<u8> = ack.into_missing_iter(frag_total).collect();

    assert_eq!(missing.as_slice(), &[0, 3, 4, 5, 6, 7, 10, 11, 12, 13, 14, 15]);
}

#[test]
fn ack_deser() {
    let ack = Ack(vec!(0b00000110u8, 0b00000011).into_boxed_slice());
    let frag_total: u8 = 15;

    let expected_frag_ids = &[1u8, 2u8, 8u8, 9u8];

    let ack_frag_ids: Vec<_> = ack.into_iter(frag_total).collect();

    assert_eq!(ack_frag_ids, expected_frag_ids);
}

#[test]
fn ack_ser_deser() {
    let vec1: Vec<u8> = (0..255u8).into_iter().collect();
    let frag_total: u8 = 254;
    let vec2: Vec<u8> = (0..255u8).into_iter().step_by(2).collect();
    let vec3: Vec<u8> = (0..255u8).into_iter().step_by(3).collect();
    
    let ack1 = Ack::create_from_frag_ids(vec1.iter().cloned(), frag_total);
    let ack2 = Ack::create_from_frag_ids(vec2.iter().cloned(), frag_total);
    let ack3 = Ack::create_from_frag_ids(vec3.iter().cloned(), frag_total);

    let ack_vec1: Vec<u8> = ack1.into_iter(frag_total).collect();
    let ack_vec2: Vec<u8> = ack2.into_iter(frag_total).collect();
    let ack_vec3: Vec<u8> = ack3.into_iter(frag_total).collect();

    assert_eq!(ack_vec1, vec1);
    assert_eq!(ack_vec2, vec2);
    assert_eq!(ack_vec3, vec3);
}