use guardian_test::crypto::*;

// ── SHA-1 ────────────────────────────────────────────────────────────────────

#[test]
fn sha1_empty() {
    let sha = Sha1::new();
    let hash = sha.finalise();
    // SHA-1("") = da39a3ee5e6b4b0d3255bfef95601890afd80709
    assert_eq!(
        hash,
        [
            0xda, 0x39, 0xa3, 0xee, 0x5e, 0x6b, 0x4b, 0x0d, 0x32, 0x55, 0xbf, 0xef, 0x95, 0x60,
            0x18, 0x90, 0xaf, 0xd8, 0x07, 0x09,
        ]
    );
}

#[test]
fn sha1_abc() {
    // RFC 3174 test vector: SHA-1("abc") = a9993e364706816aba3e25717850c26c9cd0d89d
    let mut sha = Sha1::new();
    sha.update(b"abc");
    let hash = sha.finalise();
    assert_eq!(
        hash,
        [
            0xa9, 0x99, 0x3e, 0x36, 0x47, 0x06, 0x81, 0x6a, 0xba, 0x3e, 0x25, 0x71, 0x78, 0x50,
            0xc2, 0x6c, 0x9c, 0xd0, 0xd8, 0x9d,
        ]
    );
}

#[test]
fn sha1_long() {
    // SHA-1("abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")
    // = 84983e441c3bd26ebaae4aa1f95129e5e54670f1
    let mut sha = Sha1::new();
    sha.update(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq");
    let hash = sha.finalise();
    assert_eq!(
        hash,
        [
            0x84, 0x98, 0x3e, 0x44, 0x1c, 0x3b, 0xd2, 0x6e, 0xba, 0xae, 0x4a, 0xa1, 0xf9, 0x51,
            0x29, 0xe5, 0xe5, 0x46, 0x70, 0xf1,
        ]
    );
}

#[test]
fn sha1_incremental() {
    // Feeding data in multiple update() calls should produce the same result
    let mut sha1 = Sha1::new();
    sha1.update(b"ab");
    sha1.update(b"c");
    let hash1 = sha1.finalise();

    let mut sha2 = Sha1::new();
    sha2.update(b"abc");
    let hash2 = sha2.finalise();

    assert_eq!(hash1, hash2);
}

// ── Base64 ───────────────────────────────────────────────────────────────────

#[test]
fn base64_empty() {
    assert_eq!(base64_encode(&[]).as_str(), "");
}

#[test]
fn base64_one_byte() {
    // 'M' = 0x4D → "TQ=="
    assert_eq!(base64_encode(&[0x4D]).as_str(), "TQ==");
}

#[test]
fn base64_two_bytes() {
    // "Ma" = [0x4D, 0x61] → "TWE="
    assert_eq!(base64_encode(&[0x4D, 0x61]).as_str(), "TWE=");
}

#[test]
fn base64_three_bytes() {
    // "Man" = [0x4D, 0x61, 0x6E] → "TWFu"
    assert_eq!(base64_encode(&[0x4D, 0x61, 0x6E]).as_str(), "TWFu");
}

#[test]
fn base64_six_bytes() {
    // "foobar" → "Zm9vYmFy"
    assert_eq!(base64_encode(b"foobar").as_str(), "Zm9vYmFy");
}

// ── WS accept header ────────────────────────────────────────────────────────

#[test]
fn ws_accept_known_key() {
    // RFC 6455 Section 4.2.2 example:
    // Key: "dGhlIHNhbXBsZSBub25jZQ=="
    // Accept: "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
    let accept = ws_accept_header("dGhlIHNhbXBsZSBub25jZQ==");
    assert_eq!(accept.as_str(), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
}

// ── CRC32 ────────────────────────────────────────────────────────────────────

#[test]
fn crc32_empty() {
    // CRC32 of empty data = 0x00000000
    assert_eq!(crc32(&[]), 0x00000000);
}

#[test]
fn crc32_known_vector() {
    // CRC32("123456789") = 0xCBF43926
    assert_eq!(crc32(b"123456789"), 0xCBF43926);
}

#[test]
fn crc32_single_bit_flip() {
    let data = b"hello world";
    let c1 = crc32(data);

    let mut flipped = *data;
    flipped[0] ^= 0x01; // Flip one bit
    let c2 = crc32(&flipped);

    assert_ne!(c1, c2);
}

#[test]
fn crc32_deterministic() {
    let data = b"test data";
    assert_eq!(crc32(data), crc32(data));
}
