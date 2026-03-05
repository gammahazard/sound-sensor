use guardian_test::ws_frame::*;

#[test]
fn text_frame_short() {
    let payload = b"hello";
    let mut out = [0u8; 128];
    let n = ws_text_frame(payload, &mut out);
    assert_eq!(n, 2 + 5); // 2-byte header + payload
    assert_eq!(out[0], 0x81); // FIN + text
    assert_eq!(out[1], 5); // length
    assert_eq!(&out[2..7], b"hello");
}

#[test]
fn text_frame_extended() {
    // Payload of 200 bytes → needs extended length (126 + 2-byte length)
    let payload = [b'A'; 200];
    let mut out = [0u8; 300];
    let n = ws_text_frame(&payload, &mut out);
    assert_eq!(n, 4 + 200); // 4-byte header + payload
    assert_eq!(out[0], 0x81);
    assert_eq!(out[1], 126); // Extended length marker
    assert_eq!(u16::from_be_bytes([out[2], out[3]]), 200);
    assert_eq!(&out[4..204], &[b'A'; 200]);
}

#[test]
fn text_frame_exactly_125() {
    // 125 bytes should use short format (< 126)
    let payload = [b'X'; 125];
    let mut out = [0u8; 200];
    let n = ws_text_frame(&payload, &mut out);
    assert_eq!(n, 2 + 125);
    assert_eq!(out[1], 125);
}

#[test]
fn text_frame_exactly_126() {
    // 126 bytes needs extended format
    let payload = [b'Y'; 126];
    let mut out = [0u8; 200];
    let n = ws_text_frame(&payload, &mut out);
    assert_eq!(n, 4 + 126);
    assert_eq!(out[1], 126);
    assert_eq!(u16::from_be_bytes([out[2], out[3]]), 126);
}

#[test]
fn frame_roundtrip() {
    let payload = b"test message";
    let mut out = [0u8; 128];
    let n = ws_text_frame(payload, &mut out);

    // Decode the frame
    let (hlen, plen) = decode_ws_frame(&out[..n]).unwrap();
    assert_eq!(plen, payload.len());
    assert_eq!(&out[hlen..hlen + plen], payload);
}

#[test]
fn frame_roundtrip_extended() {
    let payload = [b'Z'; 300];
    let mut out = [0u8; 512];
    let n = ws_text_frame(&payload, &mut out);

    let (hlen, plen) = decode_ws_frame(&out[..n]).unwrap();
    assert_eq!(plen, 300);
    assert_eq!(&out[hlen..hlen + plen], &[b'Z'; 300]);
}

#[test]
fn decode_too_short() {
    let data = [0x81]; // Only 1 byte
    assert!(decode_ws_frame(&data).is_none());
}

#[test]
fn unmask_basic() {
    let mask = [0x37, 0xfa, 0x21, 0x3d];
    let original = b"Hello";
    let mut masked: Vec<u8> = original
        .iter()
        .enumerate()
        .map(|(i, b)| b ^ mask[i % 4])
        .collect();
    unmask_payload(&mut masked, &mask);
    assert_eq!(&masked, original);
}

#[test]
fn ws_text_frame_truncates_on_overflow() {
    // Payload bigger than output buffer should truncate, not panic
    let payload = [b'A'; 200];
    let mut out = [0u8; 50]; // Too small for 200-byte payload
    let n = ws_text_frame(&payload, &mut out);
    // Should fit whatever it can: 50 - 2 (short header) = 48 bytes
    assert!(n <= out.len());
    assert_eq!(out[0], 0x81);
    // Verify it's a valid frame
    let (hlen, plen) = decode_ws_frame(&out[..n]).unwrap();
    assert_eq!(hlen, 2); // Short header since truncated payload < 126
    assert_eq!(plen, 48);
    assert!(out[hlen..hlen + plen].iter().all(|&b| b == b'A'));
}

#[test]
fn decode_masked_frame() {
    // Build a masked frame manually
    let mask = [0x12, 0x34, 0x56, 0x78];
    let payload = b"test";
    let mut frame = vec![0x81u8, 0x80 | 4]; // FIN + text, masked + len=4
    frame.extend_from_slice(&mask);
    for (i, &b) in payload.iter().enumerate() {
        frame.push(b ^ mask[i % 4]);
    }
    let (hlen, plen) = decode_ws_frame(&frame).unwrap();
    assert_eq!(plen, 4);
    assert_eq!(hlen, 6); // 2 header + 4 mask
    // Unmask
    let mut data = frame[hlen..hlen + plen].to_vec();
    unmask_payload(&mut data, &mask);
    assert_eq!(&data, payload);
}

#[test]
fn ws_frame_masked_same_as_text_frame() {
    let payload = b"test";
    let mut out1 = [0u8; 64];
    let mut out2 = [0u8; 64];
    let n1 = ws_text_frame(payload, &mut out1);
    let n2 = ws_frame_masked(payload, &mut out2);
    assert_eq!(n1, n2);
    assert_eq!(&out1[..n1], &out2[..n2]);
}
