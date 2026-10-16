//! Lower-case hex of bytes, for names that are derived from a digest or a random number.

/// `bytes` as lower-case hex.
pub(crate) fn encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(char::from(DIGITS[usize::from(b >> 4)]));
        out.push(char::from(DIGITS[usize::from(b & 0x0f)]));
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn hex_is_lower_case_and_two_digits_a_byte() {
        assert_eq!(super::encode(&[0x00, 0x0f, 0xa0, 0xff]), "000fa0ff");
        assert_eq!(super::encode(&[]), "");
    }
}
