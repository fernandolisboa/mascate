//! Base64 with the standard alphabet and padding (RFC 4648, section 4), for
//! a photo sent inside JSON and an HTTP Basic login.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// `bytes` as Base64 text.
pub(crate) fn encoded(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let group = chunk.iter().enumerate().fold(0u32, |group, (at, &byte)| {
            group | u32::from(byte) << (16 - 8 * at)
        });
        for at in 0..4 {
            if at <= chunk.len() {
                let index = (group >> (18 - 6 * at)) & 0x3f;
                text.push(char::from(ALPHABET[index as usize]));
            } else {
                text.push('=');
            }
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::encoded;

    #[test]
    fn the_rfc_examples_encode_as_the_rfc_says() {
        for (plain, coded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(encoded(plain.as_bytes()), coded);
        }
        assert_eq!(encoded(&[0xff, 0xfe, 0xfd]), "//79");
    }

    proptest! {
        #[test]
        fn every_three_bytes_become_four_characters_of_the_alphabet(
            bytes in proptest::collection::vec(any::<u8>(), 0..200)
        ) {
            let text = encoded(&bytes);
            prop_assert_eq!(text.len(), bytes.len().div_ceil(3) * 4);
            let unpadded = text.trim_end_matches('=');
            prop_assert!(text.len() - unpadded.len() <= 2);
            prop_assert!(unpadded.bytes().all(|byte| super::ALPHABET.contains(&byte)));
        }
    }
}
