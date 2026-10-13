//! CRC-32C (Castagnoli), the checksum Google's APIs offer for end-to-end integrity.
//!
//! TLS already protects the bytes on the wire. The checksums protect what TLS cannot: a request that was corrupted in a
//! proxy's memory, or a response that was. Signing a certificate over a damaged digest, or reading a damaged secret, is
//! worse than failing, so the clients send the checksum of what they send and compare the checksum of what they get.

const POLY: u32 = 0x82F6_3B78;

const TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        #[allow(clippy::cast_possible_truncation)]
        let mut crc = i as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 == 1 { (crc >> 1) ^ POLY } else { crc >> 1 };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
};

/// The CRC-32C of `data`.
#[must_use]
pub fn crc32c(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc = TABLE[usize::from((crc ^ u32::from(byte)).to_le_bytes()[0])] ^ (crc >> 8);
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::crc32c;

    #[test]
    fn matches_the_published_check_values() {
        // RFC 3720 appendix B.4 and the standard "123456789" check value.
        assert_eq!(crc32c(b"123456789"), 0xE306_9283);
        assert_eq!(crc32c(&[0u8; 32]), 0x8A91_36AA);
        assert_eq!(crc32c(&[0xffu8; 32]), 0x62A8_AB43);
        assert_eq!(crc32c(b""), 0);
    }

    #[test]
    fn one_flipped_bit_changes_the_checksum() {
        let mut data = *b"digest of the certificate to sign";
        let before = crc32c(&data);
        data[7] ^= 0x10;
        assert_ne!(before, crc32c(&data));
    }
}
