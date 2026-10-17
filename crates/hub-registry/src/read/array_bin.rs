//! One-dimensional `PostgreSQL` arrays in the binary format, read in place (P7, P12).
//!
//! The setting rows of a blob come as six arrays (one row on the wire for a file of 6,000 settings, see [`super::settings`]). The
//! generic decoder turns each element into a value of its own (a `String` for every path, value, type and rule: 24,000 allocations for
//! four blobs of the target scale's 6,000-line file) and the registry then copies most of them again. This reads the elements as slices
//! of the bytes the server sent, so that the registry makes exactly the strings it keeps.
//!
//! The format is the server's `array_send`: an `int4` dimension count (0 for an empty array, 1 here), an `int4` flag for null elements,
//! the element type's OID, then for each dimension its size and lower bound (`int4` each), then the elements, each an `int4` length
//! (-1 for a null) and that many bytes. Everything is big-endian. Input that is not that, in any way (more dimensions, another element
//! type, an element or a size that runs past the end, trailing bytes), is an error: nothing here indexes, slices or allocates by a
//! number it has not checked against the bytes it has.

/// The array is not what it should be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Malformed;

/// The OID of `text`.
pub const TEXT: u32 = 25;
/// The OID of `int4`.
pub const INT4: u32 = 23;

/// A one-dimensional binary array: how many elements, and the bytes of them.
#[derive(Debug, Clone, Copy)]
pub struct Array<'a> {
    len: usize,
    elements: &'a [u8],
}

fn be_i32(bytes: &[u8], at: usize) -> Result<i32, Malformed> {
    let end = at.checked_add(4).ok_or(Malformed)?;
    let four: [u8; 4] = bytes
        .get(at..end)
        .ok_or(Malformed)?
        .try_into()
        .map_err(|_| Malformed)?;
    Ok(i32::from_be_bytes(four))
}

impl<'a> Array<'a> {
    /// An array with no elements (a `NULL` from `array_agg` over no rows).
    pub const fn empty() -> Self {
        Self {
            len: 0,
            elements: &[],
        }
    }

    /// Read the header of `bytes`, an array of elements of type `element_oid`.
    ///
    /// # Errors
    ///
    /// `Malformed` for anything but a zero- or one-dimensional array of that type whose size is possible for the bytes there are.
    pub fn parse(bytes: &'a [u8], element_oid: u32) -> Result<Self, Malformed> {
        let dimensions = be_i32(bytes, 0)?;
        let oid = u32::try_from(be_i32(bytes, 8)?).map_err(|_| Malformed)?;
        if oid != element_oid {
            return Err(Malformed);
        }
        match dimensions {
            0 => {
                // `array_send` writes nothing after the header of an empty array.
                if bytes.len() == 12 {
                    Ok(Self::empty())
                } else {
                    Err(Malformed)
                }
            }
            1 => {
                let len = usize::try_from(be_i32(bytes, 12)?).map_err(|_| Malformed)?;
                let elements = bytes.get(20..).ok_or(Malformed)?;
                // Every element is at least its four-byte length: a size beyond that is a lie about the bytes, and nothing is
                // allocated for it.
                if len > elements.len() / 4 {
                    return Err(Malformed);
                }
                Ok(Self { len, elements })
            }
            _ => Err(Malformed),
        }
    }

    /// How many elements.
    pub const fn len(&self) -> usize {
        self.len
    }

    /// The elements, each `None` for a null. The iterator ends with an error if the bytes end before the elements do or hold more.
    pub const fn iter(&self) -> Elements<'a> {
        Elements {
            rest: self.elements,
            left: self.len,
            failed: false,
        }
    }
}

/// What the iterator found next.
enum Step<'a> {
    End,
    Null,
    Value(&'a [u8]),
}

/// The elements of an [`Array`].
#[derive(Debug, Clone)]
pub struct Elements<'a> {
    rest: &'a [u8],
    left: usize,
    failed: bool,
}

impl<'a> Elements<'a> {
    /// The next element, or the end of the array.
    fn take(&mut self) -> Result<Step<'a>, Malformed> {
        if self.left == 0 {
            // Nothing may follow the last element.
            return if self.rest.is_empty() {
                Ok(Step::End)
            } else {
                Err(Malformed)
            };
        }
        self.left -= 1;
        let length = be_i32(self.rest, 0)?;
        let after = self.rest.get(4..).ok_or(Malformed)?;
        if length == -1 {
            self.rest = after;
            return Ok(Step::Null);
        }
        let length = usize::try_from(length).map_err(|_| Malformed)?;
        let value = after.get(..length).ok_or(Malformed)?;
        self.rest = after.get(length..).ok_or(Malformed)?;
        Ok(Step::Value(value))
    }
}

impl<'a> Iterator for Elements<'a> {
    type Item = Result<Option<&'a [u8]>, Malformed>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        match self.take() {
            Ok(Step::Value(bytes)) => Some(Ok(Some(bytes))),
            Ok(Step::Null) => Some(Ok(None)),
            Ok(Step::End) => None,
            Err(e) => {
                self.failed = true;
                Some(Err(e))
            }
        }
    }
}

/// The element as text; a null or bytes that are not UTF-8 are errors (the columns read this way are `NOT NULL` and the server's
/// encoding is UTF-8, so neither happens, and neither is guessed at).
pub fn text(element: Option<&[u8]>) -> Result<&str, Malformed> {
    std::str::from_utf8(element.ok_or(Malformed)?).map_err(|_| Malformed)
}

/// The element as an `int4`; a null or another length is an error.
pub fn int4(element: Option<&[u8]>) -> Result<i32, Malformed> {
    let four: [u8; 4] = element.ok_or(Malformed)?.try_into().map_err(|_| Malformed)?;
    Ok(i32::from_be_bytes(four))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// What `array_send` writes for a one-dimensional array.
    fn one_dimensional(oid: u32, elements: &[Option<&[u8]>]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&1_i32.to_be_bytes());
        out.extend_from_slice(&i32::from(elements.iter().any(Option::is_none)).to_be_bytes());
        out.extend_from_slice(&oid.to_be_bytes());
        out.extend_from_slice(&i32::try_from(elements.len()).unwrap().to_be_bytes());
        out.extend_from_slice(&1_i32.to_be_bytes());
        for e in elements {
            match e {
                Some(b) => {
                    out.extend_from_slice(&i32::try_from(b.len()).unwrap().to_be_bytes());
                    out.extend_from_slice(b);
                }
                None => out.extend_from_slice(&(-1_i32).to_be_bytes()),
            }
        }
        out
    }

    fn collect(a: &Array<'_>) -> Result<Vec<Option<Vec<u8>>>, Malformed> {
        a.iter().map(|e| e.map(|o| o.map(<[u8]>::to_vec))).collect()
    }

    #[test]
    fn elements_come_back_as_they_were_sent_nulls_included() {
        let bytes = one_dimensional(TEXT, &[Some(b"a.b"), None, Some(b""), Some("é".as_bytes())]);
        let a = Array::parse(&bytes, TEXT).unwrap();
        assert_eq!(a.len(), 4);
        assert_eq!(
            collect(&a).unwrap(),
            vec![
                Some(b"a.b".to_vec()),
                None,
                Some(Vec::new()),
                Some("é".as_bytes().to_vec())
            ]
        );
    }

    #[test]
    fn ints_and_text_are_read_from_their_elements() {
        let bytes = one_dimensional(INT4, &[Some(&7_i32.to_be_bytes()), Some(&(-3_i32).to_be_bytes())]);
        let a = Array::parse(&bytes, INT4).unwrap();
        let got: Vec<i32> = a.iter().map(|e| int4(e.unwrap()).unwrap()).collect();
        assert_eq!(got, [7, -3]);
        assert_eq!(text(Some("ok".as_bytes())), Ok("ok"));
        assert_eq!(text(Some(&[0xff, 0xfe])), Err(Malformed));
        assert_eq!(text(None), Err(Malformed));
        assert_eq!(int4(Some(&[1, 2, 3])), Err(Malformed));
        assert_eq!(int4(None), Err(Malformed));
    }

    #[test]
    fn an_empty_array_and_a_null_are_no_elements() {
        let mut empty = Vec::new();
        empty.extend_from_slice(&0_i32.to_be_bytes());
        empty.extend_from_slice(&0_i32.to_be_bytes());
        empty.extend_from_slice(&TEXT.to_be_bytes());
        let a = Array::parse(&empty, TEXT).unwrap();
        assert_eq!(a.len(), 0);
        assert!(a.iter().next().is_none());
        assert_eq!(Array::empty().iter().count(), 0);
        empty.push(0);
        assert_eq!(Array::parse(&empty, TEXT).unwrap_err(), Malformed);
    }

    #[test]
    fn another_element_type_or_more_dimensions_are_refused() {
        let bytes = one_dimensional(INT4, &[Some(&1_i32.to_be_bytes())]);
        assert_eq!(Array::parse(&bytes, TEXT).unwrap_err(), Malformed);
        let mut two = bytes.clone();
        two[..4].copy_from_slice(&2_i32.to_be_bytes());
        assert_eq!(Array::parse(&two, INT4).unwrap_err(), Malformed);
        let mut negative = bytes;
        negative[12..16].copy_from_slice(&(-1_i32).to_be_bytes());
        assert_eq!(Array::parse(&negative, INT4).unwrap_err(), Malformed);
    }

    #[test]
    fn a_size_beyond_the_bytes_is_refused_before_anything_is_made_for_it() {
        let mut bytes = one_dimensional(TEXT, &[Some(b"x")]);
        bytes[12..16].copy_from_slice(&i32::MAX.to_be_bytes());
        assert_eq!(Array::parse(&bytes, TEXT).unwrap_err(), Malformed);
    }

    #[test]
    fn a_cut_element_and_trailing_bytes_end_the_iterator_with_an_error() {
        let whole = one_dimensional(TEXT, &[Some(b"abcdef"), Some(b"gh")]);
        let cut = &whole[..whole.len() - 1];
        let a = Array::parse(cut, TEXT).unwrap();
        let items: Vec<_> = a.iter().collect();
        assert_eq!(items.len(), 2);
        assert!(items[0].is_ok() && items[1].is_err(), "{items:?}");
        let mut more = whole;
        more.push(9);
        let a = Array::parse(&more, TEXT).unwrap();
        let items: Vec<_> = a.iter().collect();
        assert!(items.last().unwrap().is_err(), "{items:?}");
        // A length that is negative but not -1 is no element.
        let mut bad = one_dimensional(TEXT, &[Some(b"a")]);
        bad[20..24].copy_from_slice(&(-2_i32).to_be_bytes());
        let a = Array::parse(&bad, TEXT).unwrap();
        assert!(a.iter().next().unwrap().is_err());
    }

    proptest! {
        #[test]
        fn no_bytes_make_it_panic_or_allocate_by_a_claimed_size(bytes in prop::collection::vec(any::<u8>(), 0..200)) {
            if let Ok(a) = Array::parse(&bytes, TEXT) {
                prop_assert!(a.len() <= bytes.len() / 4);
                let _ = a.iter().count();
            }
        }

        #[test]
        fn what_is_sent_is_read_back(
            elements in prop::collection::vec(prop::option::of(prop::collection::vec(any::<u8>(), 0..20)), 0..30)
        ) {
            let refs: Vec<Option<&[u8]>> = elements.iter().map(|e| e.as_deref()).collect();
            let bytes = one_dimensional(TEXT, &refs);
            let a = Array::parse(&bytes, TEXT).unwrap();
            prop_assert_eq!(a.len(), elements.len());
            prop_assert_eq!(collect(&a).unwrap(), elements);
        }
    }
}
