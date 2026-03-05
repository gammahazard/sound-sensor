//! crypto.rs — CRC32, SHA-1, Base64, WS accept header
//! Extracted from firmware net.rs and ws.rs

// ── CRC32 ────────────────────────────────────────────────────────────────────
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

// ── SHA-1 ────────────────────────────────────────────────────────────────────
pub struct Sha1 {
    state: [u32; 5],
    count: u64,
    buf: [u8; 64],
    buf_len: usize,
}

impl Sha1 {
    pub fn new() -> Self {
        Self {
            state: [
                0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0,
            ],
            count: 0,
            buf: [0u8; 64],
            buf_len: 0,
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        for &b in data {
            self.buf[self.buf_len] = b;
            self.buf_len += 1;
            self.count += 8;
            if self.buf_len == 64 {
                self.compress();
                self.buf_len = 0;
            }
        }
    }

    pub fn finalise(mut self) -> [u8; 20] {
        self.buf[self.buf_len] = 0x80;
        self.buf_len += 1;
        if self.buf_len > 56 {
            while self.buf_len < 64 {
                self.buf[self.buf_len] = 0;
                self.buf_len += 1;
            }
            self.compress();
            self.buf_len = 0;
        }
        while self.buf_len < 56 {
            self.buf[self.buf_len] = 0;
            self.buf_len += 1;
        }
        let b = self.count;
        self.buf[56..64].copy_from_slice(&b.to_be_bytes());
        self.compress();
        let mut o = [0u8; 20];
        for (i, &w) in self.state.iter().enumerate() {
            o[i * 4..i * 4 + 4].copy_from_slice(&w.to_be_bytes());
        }
        o
    }

    fn compress(&mut self) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(self.buf[i * 4..i * 4 + 4].try_into().unwrap());
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = self.state;
        for i in 0..80 {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1u32),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDCu32),
                _ => (b ^ c ^ d, 0xCA62C1D6u32),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(w[i]);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
        self.state[4] = self.state[4].wrapping_add(e);
    }
}

// ── Base64 ───────────────────────────────────────────────────────────────────
const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(input: &[u8]) -> heapless::String<32> {
    let mut out: heapless::String<32> = heapless::String::new();
    let mut i = 0;
    while i + 2 < input.len() {
        let [b0, b1, b2] = [input[i] as usize, input[i + 1] as usize, input[i + 2] as usize];
        let _ = out.push(B64[b0 >> 2] as char);
        let _ = out.push(B64[((b0 & 3) << 4) | (b1 >> 4)] as char);
        let _ = out.push(B64[((b1 & 0xF) << 2) | (b2 >> 6)] as char);
        let _ = out.push(B64[b2 & 0x3F] as char);
        i += 3;
    }
    match input.len() - i {
        1 => {
            let b0 = input[i] as usize;
            let _ = out.push(B64[b0 >> 2] as char);
            let _ = out.push(B64[(b0 & 3) << 4] as char);
            let _ = out.push('=');
            let _ = out.push('=');
        }
        2 => {
            let [b0, b1] = [input[i] as usize, input[i + 1] as usize];
            let _ = out.push(B64[b0 >> 2] as char);
            let _ = out.push(B64[((b0 & 3) << 4) | (b1 >> 4)] as char);
            let _ = out.push(B64[(b1 & 0xF) << 2] as char);
            let _ = out.push('=');
        }
        _ => {}
    }
    out
}

// ── WebSocket accept header (RFC 6455) ───────────────────────────────────────
pub fn ws_accept_header(key: &str) -> heapless::String<32> {
    const GUID: &[u8] = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
    let mut sha = Sha1::new();
    sha.update(key.as_bytes());
    sha.update(GUID);
    let hash = sha.finalise();
    base64_encode(&hash)
}
