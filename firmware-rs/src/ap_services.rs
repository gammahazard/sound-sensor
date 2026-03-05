//! ap_services.rs — DHCP server + DNS responder for AP mode
//!
//! Spawned only during AP mode. DHCP assigns 192.168.4.2 to connecting phones.
//! DNS resolves all A queries to 192.168.4.1 for captive portal detection.

use defmt::*;
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::{IpAddress, IpEndpoint, Ipv4Address, Stack};

const SERVER_IP: [u8; 4] = [192, 168, 4, 1];
const CLIENT_IP: [u8; 4] = [192, 168, 4, 2];
const SUBNET_MASK: [u8; 4] = [255, 255, 255, 0];
const LEASE_SECS: u32 = 300;
const MAGIC_COOKIE: [u8; 4] = [99, 130, 83, 99]; // 0x63825363

// ── DHCP Server ─────────────────────────────────────────────────────────────

#[embassy_executor::task]
pub async fn dhcp_server_task(stack: Stack<'static>) {
    let mut rx_meta = [PacketMetadata::EMPTY; 4];
    let mut tx_meta = [PacketMetadata::EMPTY; 4];
    let mut rx_buf = [0u8; 1024];
    let mut tx_buf = [0u8; 1024];

    let mut socket = UdpSocket::new(stack, &mut rx_meta, &mut rx_buf, &mut tx_meta, &mut tx_buf);
    if socket.bind(67).is_err() {
        warn!("[dhcp] Failed to bind port 67");
        return;
    }
    info!("[dhcp] Server listening on port 67");

    let mut pkt = [0u8; 576];
    loop {
        let (n, _) = match socket.recv_from(&mut pkt).await {
            Ok(r) => r,
            Err(_) => continue,
        };

        if n < 240 { continue; }
        if pkt[0] != 1 { continue; } // Not BOOTREQUEST
        if pkt[236..240] != MAGIC_COOKIE { continue; }

        let xid = [pkt[4], pkt[5], pkt[6], pkt[7]];
        let mut chaddr = [0u8; 6];
        chaddr.copy_from_slice(&pkt[28..34]);

        let msg_type = find_dhcp_option(&pkt[240..n], 53);
        let reply_type = match msg_type {
            Some(1) => {
                info!("[dhcp] DISCOVER → OFFER 192.168.4.2");
                2u8 // OFFER
            }
            Some(3) => {
                info!("[dhcp] REQUEST → ACK 192.168.4.2");
                5u8 // ACK
            }
            _ => continue,
        };

        let len = build_dhcp_reply(&mut pkt, &xid, &chaddr, reply_type);
        let broadcast = IpEndpoint::new(
            IpAddress::Ipv4(Ipv4Address::new(255, 255, 255, 255)),
            68,
        );
        let _ = socket.send_to(&pkt[..len], broadcast).await;
    }
}

fn find_dhcp_option(options: &[u8], target: u8) -> Option<u8> {
    let mut i = 0;
    while i < options.len() {
        let opt = options[i];
        if opt == 255 { break; }
        if opt == 0 { i += 1; continue; }
        if i + 1 >= options.len() { break; }
        let len = options[i + 1] as usize;
        if i + 2 + len > options.len() { break; } // bounds check
        if opt == target && len >= 1 {
            return Some(options[i + 2]);
        }
        i += 2 + len;
    }
    None
}

fn build_dhcp_reply(buf: &mut [u8; 576], xid: &[u8; 4], chaddr: &[u8; 6], msg_type: u8) -> usize {
    buf.fill(0);
    buf[0] = 2; // BOOTREPLY
    buf[1] = 1; // Ethernet
    buf[2] = 6; // HW addr len
    buf[4..8].copy_from_slice(xid);
    buf[16..20].copy_from_slice(&CLIENT_IP); // yiaddr (assigned IP)
    buf[20..24].copy_from_slice(&SERVER_IP); // siaddr (server IP)
    buf[28..34].copy_from_slice(chaddr);
    buf[236..240].copy_from_slice(&MAGIC_COOKIE);

    let mut i = 240;
    // Option 53: DHCP Message Type
    buf[i] = 53; buf[i + 1] = 1; buf[i + 2] = msg_type;
    i += 3;
    // Option 54: Server Identifier
    buf[i] = 54; buf[i + 1] = 4;
    buf[i + 2..i + 6].copy_from_slice(&SERVER_IP);
    i += 6;
    // Option 1: Subnet Mask
    buf[i] = 1; buf[i + 1] = 4;
    buf[i + 2..i + 6].copy_from_slice(&SUBNET_MASK);
    i += 6;
    // Option 3: Router
    buf[i] = 3; buf[i + 1] = 4;
    buf[i + 2..i + 6].copy_from_slice(&SERVER_IP);
    i += 6;
    // Option 6: DNS Server (resolves to us for captive portal)
    buf[i] = 6; buf[i + 1] = 4;
    buf[i + 2..i + 6].copy_from_slice(&SERVER_IP);
    i += 6;
    // Option 51: Lease Time
    buf[i] = 51; buf[i + 1] = 4;
    buf[i + 2..i + 6].copy_from_slice(&LEASE_SECS.to_be_bytes());
    i += 6;
    // Option 114: Captive-Portal URI (RFC 8910)
    // Tells iOS 14+/Android 11+ "this is a captive portal, show this URL"
    // Avoids the unreliable probe-based detection dance.
    const PORTAL_URI: &[u8] = b"http://192.168.4.1/";
    buf[i] = 114; buf[i + 1] = PORTAL_URI.len() as u8;
    buf[i + 2..i + 2 + PORTAL_URI.len()].copy_from_slice(PORTAL_URI);
    i += 2 + PORTAL_URI.len();
    // End
    buf[i] = 255;
    i += 1;
    i
}

// ── DNS Responder ───────────────────────────────────────────────────────────
//
// Resolves ALL A queries to 192.168.4.1 so phones trigger captive portal
// detection and auto-show the setup page.

#[embassy_executor::task]
pub async fn dns_server_task(stack: Stack<'static>) {
    let mut rx_meta = [PacketMetadata::EMPTY; 4];
    let mut tx_meta = [PacketMetadata::EMPTY; 4];
    let mut rx_buf = [0u8; 768];
    let mut tx_buf = [0u8; 768];

    let mut socket = UdpSocket::new(stack, &mut rx_meta, &mut rx_buf, &mut tx_meta, &mut tx_buf);
    if socket.bind(53).is_err() {
        warn!("[dns] Failed to bind port 53");
        return;
    }
    info!("[dns] Captive portal DNS on port 53");

    let mut pkt = [0u8; 512];
    let mut resp = [0u8; 512];
    loop {
        let (n, sender) = match socket.recv_from(&mut pkt).await {
            Ok(r) => r,
            Err(_) => continue,
        };

        if let Some(resp_len) = build_dns_response(&pkt[..n], &mut resp) {
            let _ = socket.send_to(&resp[..resp_len], sender).await;
        }
    }
}

/// Build a DNS response that resolves A queries to 192.168.4.1.
/// Non-A queries get an empty answer (ANCOUNT=0).
fn build_dns_response(query: &[u8], resp: &mut [u8; 512]) -> Option<usize> {
    if query.len() < 12 { return None; }
    // Must be a standard query (QR=0, Opcode=0)
    if query[2] & 0x80 != 0 { return None; }
    if query[2] & 0x78 != 0 { return None; }

    let qdcount = u16::from_be_bytes([query[4], query[5]]);
    if qdcount != 1 { return None; }

    // Parse QNAME to find end of question section
    let mut pos = 12;
    while pos < query.len() {
        let label_len = query[pos] as usize;
        if label_len == 0 { pos += 1; break; }
        if pos + 1 + label_len >= query.len() { return None; }
        pos += 1 + label_len;
    }
    // QTYPE + QCLASS
    if pos + 4 > query.len() { return None; }
    let qtype = u16::from_be_bytes([query[pos], query[pos + 1]]);
    pos += 4;

    // Copy header + question section
    let q_end = pos;
    if q_end + 16 > 512 { return None; }
    resp[..q_end].copy_from_slice(&query[..q_end]);

    // Set response flags: QR=1, AA=1, RA=0, RCODE=0
    resp[2] = 0x84;
    resp[3] = 0x00;
    // NSCOUNT = 0, ARCOUNT = 0
    resp[8] = 0; resp[9] = 0;
    resp[10] = 0; resp[11] = 0;

    // Only answer A queries (type 1) with our IP
    if qtype != 1 {
        resp[6] = 0; resp[7] = 0; // ANCOUNT = 0
        return Some(q_end);
    }

    // ANCOUNT = 1
    resp[6] = 0; resp[7] = 1;

    // Answer: A record pointing to 192.168.4.1
    let mut out = q_end;
    // Name pointer to question name (offset 0x0C = 12)
    resp[out] = 0xC0; resp[out + 1] = 0x0C; out += 2;
    // Type A
    resp[out] = 0; resp[out + 1] = 1; out += 2;
    // Class IN
    resp[out] = 0; resp[out + 1] = 1; out += 2;
    // TTL (60 seconds)
    resp[out..out + 4].copy_from_slice(&60u32.to_be_bytes()); out += 4;
    // RDLENGTH (4 bytes)
    resp[out] = 0; resp[out + 1] = 4; out += 2;
    // RDATA (192.168.4.1)
    resp[out..out + 4].copy_from_slice(&SERVER_IP); out += 4;

    Some(out)
}
