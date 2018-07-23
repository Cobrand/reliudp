pub (crate) trait ClonableIterator<'a>: Iterator {
    fn clone_box(&self) -> Box<ClonableIterator<'a, Item = Self::Item> + 'a>;
}

pub (crate) type BoxedSlice<T> = OwnedSlice<T, Box<[T]>>;

impl<'a, T: Clone + Iterator + 'a> ClonableIterator<'a> for T {
    fn clone_box(&self) -> Box<ClonableIterator<'a, Item = Self::Item> + 'a> {
        Box::new(self.clone())
    }
}

/// An Owned Slice
pub (crate) struct OwnedSlice<T, D: AsRef<[T]> + 'static> {
    _d: ::std::marker::PhantomData<T>,
    data: D,
    strip_begin: usize,
}

impl<T, D: AsRef<[T]> + 'static> ::std::fmt::Debug for OwnedSlice<T, D> where T: ::std::fmt::Debug {
    fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        // format StrippedBoxedSlice exactly as a slice starting from strip.
        (self.data.as_ref()[self.strip_begin..]).fmt(f)
    }
}

impl<T, D: AsRef<[T]> + 'static> OwnedSlice<T, D> {
    #[inline]
    pub fn new(data: D, strip_begin: usize) -> Self {
        if strip_begin > data.as_ref().len() {
            panic!("OwnedSlice: cannot strip a higher amount than the length of original boxed slice: {} > {}", strip_begin, data.as_ref().len());
        }
        OwnedSlice {
            _d: ::std::marker::PhantomData,
            data,
            strip_begin
        }
    }

    #[inline]
    pub fn with_added_strip(self, added_strip: usize) -> Self {
        let new_strip: usize = self.strip_begin.saturating_add(added_strip);
        OwnedSlice::new(self.data, new_strip)
    }
    
    pub fn as_slice(&self) -> &[T] {
        &self.data.as_ref()[self.strip_begin..]
    }
}

impl<T, D: AsRef<[T]>> AsRef<[T]> for OwnedSlice<T, D> {
    fn as_ref(&self) -> &[T] {
        self.as_slice()
    }
}