//! ws_frame.rs — WebSocket frame encoder (extracted from firmware ws.rs and tv.rs)

/// Encode a WebSocket text frame (server→client, no mask). Returns total frame length.
pub fn ws_text_frame(payload: &[u8], out: &mut [u8]) -> usize {
    let len = payload.len();
    let hlen = if len < 126 { 2 } else { 4 };
    if hlen + len > out.len() {
        // Truncate: recalculate header size for the smaller payload
        let max2 = out.len().saturating_sub(2);
        let (trunc_hlen, trunc_len) = if max2 < 126 {
            (2, max2)
        } else {
            (4, out.len().saturating_sub(4))
        };
        out[0] = 0x81;
        if trunc_len < 126 {
            out[1] = trunc_len as u8;
        } else {
            out[1] = 126;
            out[2] = (trunc_len >> 8) as u8;
            out[3] = (trunc_len & 0xFF) as u8;
        }
        out[trunc_hlen..trunc_hlen + trunc_len].copy_from_slice(&payload[..trunc_len]);
        return trunc_hlen + trunc_len;
    }
    out[0] = 0x81; // FIN + text opcode
    if len < 126 {
        out[1] = len as u8;
    } else {
        out[1] = 126;
        out[2] = (len >> 8) as u8;
        out[3] = (len & 0xFF) as u8;
    }
    out[hlen..hlen + len].copy_from_slice(payload);
    hlen + len
}

/// Encode a WebSocket text frame (client→server style, unmasked). Same as ws_text_frame.
pub fn ws_frame_unmasked(payload: &[u8], out: &mut [u8]) -> usize {
    ws_text_frame(payload, out)
}

/// Decode a WebSocket frame header. Returns (payload_offset, payload_len) or None.
pub fn decode_ws_frame(raw: &[u8]) -> Option<(usize, usize)> {
    if raw.len() < 2 {
        return None;
    }
    let masked = (raw[1] & 0x80) != 0;
    let raw_len = (raw[1] & 0x7F) as usize;

    let (payload_len, hdr_extra) = if raw_len == 126 {
        if raw.len() < 4 {
            return None;
        }
        let ext = u16::from_be_bytes([raw[2], raw[3]]) as usize;
        (ext, 2)
    } else if raw_len == 127 {
        return None; // Not supported
    } else {
        (raw_len, 0)
    };

    let mask_offset = 2 + hdr_extra;
    let hlen = mask_offset + if masked { 4 } else { 0 };
    if raw.len() < hlen + payload_len {
        return None;
    }

    Some((hlen, payload_len))
}

/// Unmask a WebSocket payload in-place given the mask key.
pub fn unmask_payload(data: &mut [u8], mask: &[u8; 4]) {
    for (i, b) in data.iter_mut().enumerate() {
        *b ^= mask[i % 4];
    }
}
