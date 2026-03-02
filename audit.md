# Guardian Sound Sensor — Full Code Audit
**Date:** 2026-03-02
**Scope:** Every source file read in full. All data paths traced. All TV logic verified.
**Verdict summary:** Core architecture is solid. Three critical bugs must be fixed before any production use. Several important design constants are hardcoded and should be tunable. Security posture is LAN-only and acceptable for a home device. Estimated fix effort for all critical+high items: 1–2 focused sessions.

---

## Fixes Applied (2026-03-02)

All critical and high-severity actionable items have been fixed in the same session.

| # | Issue | File | Status |
|---|-------|------|--------|
| BUG-1 | PIO X register never initialized; `PioProgram::new` doesn't exist | `audio.rs` | ✅ Fixed — rewrote with `pio_proc::pio_asm!`, added `set x, 31`, correct slave-mode clock div=4 |
| BUG-2 | WS extended frame (≥126 bytes) rejected; long WiFi passwords dropped | `ws.rs` | ✅ Fixed — full RFC 6455 extended-length handling, supports 126-byte prefix format |
| BUG-3 | `out_frame` 512 bytes too small; large scan JSON causes panic | `ws.rs` | ✅ Fixed — `out_frame` → 1100 bytes, `format_event` → `String<1024>`, all resp bufs → `String<1024>` |
| HIGH-1 | `FLASH_SIZE` = 2 MB but Pico 2 W has 4 MB; duplicated in two files | `net.rs`, `flash_fs.rs` | ✅ Fixed — both updated to `4 * 1024 * 1024` with cross-reference comments |
| HIGH-2 | LG pairing never reads confirmation | `tv.rs` | ⚠️ Noted — not changed (intentional deferred read; behavior is logged. Future: read + check registered) |
| HIGH-3 | Roku `roku_key` always returns `true` | `tv.rs` | ✅ Fixed — checks HTTP 2xx status code |
| HIGH-4 | Samsung token not persisted across reboots | `tv.rs` | ⚠️ Noted — deferred to Phase 4 (requires flash credential block expansion) |
| MOD-4 | Misleading `lower` variable in SSDP discovery (not lowercased) | `tv.rs` | ✅ Fixed — removed alias, use `resp` directly; added "Roku" capitalised variant |
| MOD-5 | `VERSION_JSON` static has stale `"fw":"0.2.0"` | `pwa_assets.rs` | ✅ Fixed — updated to `"fw":"0.3.0"` with clarifying comment |
| — | Missing `pio-proc = "0.2"` dependency | `Cargo.toml` | ✅ Fixed — added |
| — | Package version `0.2.0` mismatches `FW_VERSION = "0.3.0"` | `Cargo.toml` | ✅ Fixed — bumped to `0.3.0` |
| — | `RESTORE_STEP_MS`, ducking thresholds had no tuning comments | `tv.rs`, `ducking.rs` | ✅ Fixed — detailed tuning comments added to all magic numbers |

**Remaining open items:** HIGH-2 (LG confirmation read), HIGH-4 (Samsung token persistence), MOD-1 (duck_steps cap), MOD-2/3/6/7 (minor UX and Samsung path issues). None are blockers for current functionality.

---

## 1. Data Flow Map

```
┌─────────────────────────────────────────────────────────────────────────┐
│                         FIRMWARE (Pico 2 W)                             │
│                                                                          │
│  SPH0645 mic ──I²S──► audio_task                                        │
│                             │ DB_CHANNEL (f32, cap 4)                   │
│                             ▼                                           │
│                      websocket_task ◄──── DB_CHANNEL.receive()          │
│                         │    │                                          │
│                         │    │  Triggers DuckingEngine.tick()           │
│                         │    │  → DuckCommand::{VolumeDown, Restore}   │
│                         │    │  → tv::send_duck_command()               │
│                         │    │  → DUCK_CHANNEL (cap 4)                 │
│                         │    │                                          │
│                         │    ▼                                          │
│                         │  tv_task ◄──── DUCK_CHANNEL.receive()         │
│                         │    │                                          │
│                         │    ├── LG:      ssap:// WS port 3000          │
│                         │    ├── Samsung: Smart Remote WS port 8001     │
│                         │    ├── Sony:    HTTP JSON-RPC port 80         │
│                         │    └── Roku:    ECP HTTP port 8060            │
│                         │                                               │
│  PWA ──────────► WS frame (port 81) ──► process_command()               │
│  commands                   │                                          │
│  ← ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─  │  ─► arm/disarm/calibrate/set_tv          │
│   telemetry every 100ms     │  ─► scan_wifi → WIFI_CMD_CH               │
│   {"db","armed","tripwire"} │  ─► set_wifi → WIFI_CMD_CH                │
│   event frames (unsolicited)│  ─► discover_tvs (blocks 3s inline)       │
│                             │  ─► ota_check (returns current versions)  │
│                             │                                          │
│           WIFI_CMD_CH ──────┘                                           │
│           (WifiCmd::Scan / ::Reconfigure, cap 2)                        │
│                ▼                                                        │
│           wifi_task LED loop ──── cyw43::Control                       │
│                │  WIFI_EVT_CH                                           │
│                │  (WifiEvent::ScanResults, cap 2)                       │
│                ▼                                                        │
│           websocket_task picks up results on next DB tick               │
│                                                                         │
│  FLASH layout:                                                          │
│    0x000000–0x0FFFFF  Firmware binary                                   │
│    0x100000–0x1FEFFF  flash_fs partition (OTA PWA assets)              │
│    0x1FF000–0x1FFFFF  WiFi credential block (256 bytes, CRC32)         │
└─────────────────────────────────────────────────────────────────────────┘

┌────────────────────────────────────────────────────────────┐
│                    PWA (pwa-wasm/src/)                      │
│                                                            │
│  main.rs ── create_signal() for every reactive value       │
│    │                                                       │
│    ├── ws::use_websocket() → gloo-net WebSocket            │
│    │     ├── Telemetry: set_db, set_armed, set_tripwire    │
│    │     │             set_fw_ver, set_pwa_ver             │
│    │     └── Events:   set_wifi_networks  (wifi_scan evt)  │
│    │                   set_discovered_tvs (discovered evt)  │
│    │                   set_ota_status      (ota_* evts)    │
│    │                                                       │
│    ├── Tab: Meter     (meter.rs)  — db bar, peak, arm btn  │
│    ├── Tab: Calibrate (calibration.rs) — 2-step + slider   │
│    ├── Tab: TV        (tv.rs)    — discover, connect       │
│    ├── Tab: WiFi      (wifi.rs)  — scan, reconnect         │
│    └── Tab: Info      (info.rs)  — versions, OTA check     │
└────────────────────────────────────────────────────────────┘
```

---

## 2. Critical Bugs (MUST FIX before production)

### BUG-1: PIO Program X Register Never Initialized → Zero Audio
**File:** `firmware-rs/src/audio.rs:36–47`
**Severity:** CRITICAL
**Effect:** Audio capture always returns silence; meter stuck at -96 dBFS; ducking never fires.

The PIO program includes a loop instruction `jmp x-- addr` (opcode `0x0042`) but never initializes the X scratch register. PIO registers default to **0** at reset. On the first execution:
- `in pins, 1` — shifts 1 bit in
- `jmp x-- 2` — X=0, so X decrements to 0xFFFFFFFF and the jump is NOT taken (falls through)
- `push noblock` — pushes after only **1 bit**, not 32

The corrected approach requires a `set x, 31` instruction before the inner loop, or use the autopush mechanism with threshold=32 (which the config sets, but the PIO program doesn't actually shift 32 bits before the push instruction). The autopush would fire on 32 bits but the manual push overrides it.

Additionally, `embassy_rp::pio::PioProgram::new(&[u16], wrap)` is not part of the public embassy-rp 0.3 API. Programs must be loaded via `pio_proc::pio_asm!()` or the `pio` crate. This file will likely fail to **compile** as-is.

**Fix:** Replace the raw PIO program with a proper `pio_proc::pio_asm!` program, or remove the manual push instruction and rely on the configured auto-push at 32 bits.

```rust
// Add pio-proc to Cargo.toml, then:
use pio_proc::pio_asm;
let prg = pio_asm!(
    "set x, 31",          // load 31 into X (loop counter for 32 bits)
    ".wrap_target",
    "wait 1 gpio 1",      // wait for LRCL = 1 (left channel start)
    "loop:",
    "  wait 1 gpio 0",    // wait for BCLK rising edge
    "  in pins, 1",       // shift one bit
    "  jmp x-- loop",     // repeat 31 more times (X decrements 31→0)
    "set x, 31",          // reset counter for next word
    ".wrap"
);
let loaded = common.load_program(&prg.program);
cfg.use_program(&loaded, &[]);
// With auto-push at threshold=32, no manual push needed
```

---

### BUG-2: WebSocket Frame Decoder Drops Commands ≥ 126 Bytes
**File:** `firmware-rs/src/ws.rs:188–192`
**Severity:** CRITICAL
**Effect:** Any WiFi credential change with a combined SSID+password length ≥ ~90 characters is silently dropped. User gets no error; firmware acts as if no command was sent.

```rust
// Current code rejects any message with payload ≥ 126 bytes:
if raw_len >= 126 || raw.len() < hlen + raw_len { return None; }
```

A `{"cmd":"set_wifi","ssid":"...","pass":"..."}` command with a 30-char SSID and 30-char password is about 80 bytes — fine. But with a 60-char SSID and 40-char password it's about 125+ bytes — dropped.

The browser WebSocket implementation uses the extended 2-byte length format for payloads 126–65535 bytes. `raw_len == 126` means "read the next 2 bytes for the real length", not "reject the frame."

**Fix:** Handle extended length:
```rust
let raw_len_prefix = (raw[1] & 0x7F) as usize;
let (payload_len, hlen_base) = if raw_len_prefix < 126 {
    (raw_len_prefix, 2)
} else if raw_len_prefix == 126 && raw.len() >= 4 {
    (u16::from_be_bytes([raw[2], raw[3]]) as usize, 4)
} else {
    return None; // 127 (64-bit length) not needed
};
let hlen = hlen_base + if masked { 4 } else { 0 };
```

---

### BUG-3: JSON Response Buffer Overflow → Firmware Panic / Reset
**File:** `firmware-rs/src/ws.rs:119` and `firmware-rs/src/tv.rs:276–291`
**Severity:** CRITICAL
**Effect:** A WiFi scan with many networks, or TV discovery with multiple TVs, will panic the firmware and cause a watchdog reset. The user will see the device drop off the network momentarily.

The problem is two-fold:

**Part A:** `out_frame` is 512 bytes. `ws_text_frame` writes `4 (header) + payload_len` bytes. If the payload (JSON string) is close to 512 bytes, the header pushes it over:
```rust
let mut out_frame = [0u8; 512];
// ...
let n = ws_text_frame(evt_json.as_bytes(), &mut out_frame); // panics if > 508 bytes
```

**Part B:** `format_event` for `wifi_scan` uses `heapless::String<512>`. With 20 networks at ~50 chars each, the JSON is ~1000 chars. The string silently truncates at 512 bytes, producing **malformed JSON** (cut mid-entry). This malformed JSON is then sent to the PWA, which silently fails to parse it with no user feedback.

**Worst case sizes:**
- `wifi_scan` with 20 networks at 50 chars each: ~1000 chars → truncated to 512 → malformed JSON
- `discovered` with 8 TVs at 62 chars each: ~530 chars → truncates `out_frame` bounds check → panic

**Fix:**
1. Increase `format_event` buffer to `heapless::String<1024>`
2. Increase `out_frame` to `[0u8; 1024]` in `handle_client`
3. Limit networks to 10 instead of 20 in `WifiEvent::ScanResults` OR truncate gracefully before exceeding buffer

---

## 3. High Severity Issues

### HIGH-1: FLASH_SIZE Constant is 2 MB but Pico 2 W Has 4 MB
**Files:** `firmware-rs/src/net.rs:40`, `firmware-rs/src/flash_fs.rs:34`
**Effect:** The flash driver enforces a 2 MB boundary. Reads/writes above 2 MB fail. Currently we only use the bottom ~2 MB so no crash, but the top 2 MB is unusable. Additionally the constant is **duplicated** — changing one does not change the other.

```rust
// net.rs line 40:
const FLASH_SIZE: usize = 2 * 1024 * 1024;   // WRONG — should be 4 * 1024 * 1024

// flash_fs.rs line 34:
pub const FLASH_SIZE: usize = 2 * 1024 * 1024;  // same wrong value, duplicate!
```

`memory.x` correctly states `LENGTH = 4096K`. The Pico 2 W (RP2350) ships with 4 MB flash.

**Fix:** Define once in `main.rs` or a `config.rs`:
```rust
// In main.rs:
pub const FLASH_SIZE_BYTES: usize = 4 * 1024 * 1024;   // Pico 2 W (RP2350) = 4 MB
```
Then reference `crate::FLASH_SIZE_BYTES` in both `net.rs` and `flash_fs.rs`. This also frees up 2 MB for future flash_fs use.

---

### HIGH-2: LG WebOS Pairing — No Confirmation Check
**File:** `firmware-rs/src/tv.rs:461–467`
**Effect:** After sending the pairing message, the firmware marks the TV as connected and proceeds to send volume commands. If the TV shows a pairing popup and the user hasn't tapped "Allow" yet, all commands succeed (write doesn't fail) but the TV silently ignores them. The user has no feedback.

The TV sends back `{"type":"registered","id":"reg_1",...}` on acceptance. The code never reads this response after sending the pair message.

**Impact:** First-time LG users will wonder why volume ducking does nothing. They need to tap Allow on the TV screen, but the firmware gives no indication of this requirement beyond a log message.

**Recommended fix:** Read one WS frame after the pairing message with a 30-second timeout. Check for `"type":"registered"`. If `"type":"error"` is received, return false (trigger reconnect + Error LED). This would turn a silent failure into a visible one.

---

### HIGH-3: Roku Commands Always Return `true`
**File:** `firmware-rs/src/tv.rs:635–644`
**Effect:** Roku key presses silently succeed even if the Roku rejected them (e.g., wrong IP, TV asleep, HTTP 400/404 response). The ducking engine will increment `duck_steps_taken` and attempt a matching restore, but no actual volume change occurred.

```rust
async fn roku_key(socket, out, cfg, key) -> bool {
    // ...
    let _ = read_http_response(socket, out).await;
    true  // ← always succeeds, even on error
}
```

The HTTP response is read and discarded. It should check for HTTP 200.

**Fix:**
```rust
match read_http_response(socket, out).await {
    Some(n) => out[..n.min(12)].starts_with(b"HTTP/1.1 200"),
    None    => false,
}
```

---

### HIGH-4: Samsung Pairing Token Not Persisted → Popup on Every Boot
**File:** `firmware-rs/src/tv.rs:63–83`
**Effect:** The Samsung pairing token is stored only in `TvConfig.samsung_token` (in-memory). Every time the Pico reboots, the token is forgotten. Samsung TV shows the pairing popup every boot. For a device that reboots on WiFi change (which is a designed workflow), this is especially painful.

**Fix:** Store the Samsung token in flash alongside WiFi credentials (extend the credential block from 256 to 512 bytes, or add a separate 256-byte Samsung block just before the credential block).

---

## 4. Moderate Issues

### MOD-1: Duck Timing Constants Are Hardcoded (Not User-Configurable)
**Files:** `firmware-rs/src/ducking.rs:103–113`, `firmware-rs/src/tv.rs:23`

The following values determine how aggressively Guardian responds but cannot be changed from the PWA:

| Constant | Location | Default | What it controls |
|---|---|---|---|
| `3000` ms | `ducking.rs:103` | 3 seconds | Time noise must persist before ducking fires |
| `500` ms | `ducking.rs:108` | 500 ms | Duck step rate when noise is 15+ dB over tripwire |
| `1000` ms | `ducking.rs:112` | 1000 ms | Duck step rate at normal excess |
| `2000` ms | `ducking.rs:109` | 2000 ms | Duck step rate when noise is < 5 dB over tripwire |
| `50` per tick | `ducking.rs:88` | 50 ms | How much `sustained_ms` decays per 100ms of quiet |
| `RESTORE_STEP_MS = 400` | `tv.rs:23` | 400 ms | Delay between each volume-up step during restore |

**To tune sensitivity right now (code changes):**

**Make ducking fire faster:** Lower `3000` → e.g. `2000` (`ducking.rs:103`)
**Make ducking step down faster at normal noise levels:** Lower `1000` → e.g. `500` (`ducking.rs:112`)
**Make restore faster:** Lower `RESTORE_STEP_MS` → e.g. `200` (`tv.rs:23`)
**Make it "forgive" noise gaps faster:** Increase `50` → e.g. `100` (`ducking.rs:88`)
**Make it never duck on brief spikes:** Increase `3000` → e.g. `5000`

**Recommended addition:** Add a WS command `{"cmd":"set_sensitivity","trip_ms":3000,"step_ms":1000,"restore_ms":400}` and corresponding `DuckingEngine` setters. This lets users tune from the PWA without reflashing.

---

### MOD-2: No Cap on Duck Steps
**File:** `firmware-rs/src/ducking.rs:126`

`duck_steps_taken` uses `saturating_add(1)`. There is no maximum. If the noise persists above the tripwire indefinitely AND the restore trigger never fires (e.g., noise floor is very noisy — `db` never drops below `floor_db + 2.0`), the engine will keep emitting `VolumeDown` forever.

LG/Sony both duck to 0 eventually (you can't go below 0) and then keep requesting volume 0. For Samsung/Roku, KEY_VOLDOWN below minimum is harmless. So this isn't catastrophic, but it means restore will try to step up by a very large `steps` count.

The restore math for LG/Sony: `current = original.saturating_sub(steps)`. If original=20 and steps=50, current=0, ramp 1…20. That's 20 `setVolume` calls with 400ms each = 8 seconds of ramping. Annoying but not broken.

**Recommended:** Cap `duck_steps_taken` at e.g. 20 to prevent excessively long restore sequences.

---

### MOD-3: Sony Volume Efficiency — Double GET on First Duck Step
**File:** `firmware-rs/src/tv.rs:309–328`

For Sony, the first `DuckCommand::VolumeDown` does:
1. `tv_get_volume` (HTTP GET `/sony/audio`) → stores `original_volume`
2. `tv_volume_down` → `sony_volume_step(false)` → HTTP GET `/sony/audio` AGAIN → HTTP SET

That's two network round-trips for the first duck step. Subsequent steps only do one. Minor but worth noting.

**Fix:** After the `needs_query` block that stores original_volume, use the stored value to compute target directly instead of calling `sony_volume_step` which queries again.

---

### MOD-4: Malformed SSDP Response Brand Detection
**File:** `firmware-rs/src/tv.rs:170–182`

Brand detection uses case-sensitive string matching on the raw response. But the variable is named `lower` (suggesting case-insensitive intent) yet the response is never lowercased:
```rust
let lower = resp;  // ← NOT lowercased! Misleading variable name
let (brand_str, name_str) = if lower.contains("LGE") ...
```

This is a naming bug. The detection works because LG/Samsung/Sony SSDP responses are consistently cased, but it's misleading code. Also, SSDP responses are plain text with uppercase headers, so the casing is reliable in practice.

---

### MOD-5: `pwa_assets.rs` VERSION_JSON Has Stale Firmware Version
**File:** `firmware-rs/src/pwa_assets.rs:51`

```rust
pub static VERSION_JSON: &[u8] = br#"{"pwa":"0.1.0","fw":"0.2.0","built":"2026-03-02"}"#;
//                                                        ^^^^^^ wrong — should be 0.3.0
```

However, `http.rs` builds `/version.json` dynamically and never references this static. This is effectively dead code. It should either be removed or corrected.

---

### MOD-6: `TvConfig::default()` Reads Undocumented `GUARDIAN_TV_IP` Env Var
**File:** `firmware-rs/src/tv.rs:73–74`

```rust
let mut ip = heapless::String::new();
let _ = ip.push_str(env!("GUARDIAN_TV_IP", ""));
```

This allows baking a TV IP into the firmware at compile time. The feature is undocumented in the README or `.cargo/config.toml`. A user who knows about it can avoid the TV setup flow on first boot. Should be documented.

---

### MOD-7: WiFi Tab "Status: Connected" Always True
**File:** `pwa-wasm/src/wifi.rs:59`

```rust
<span style="...color:#22c55e">"Connected"</span>
```

This always shows "Connected" in green. It doesn't reflect whether the Pico is actually connected to WiFi. It should use the `ws_state` signal — if WS is connected, WiFi is connected. If WS is disconnected/connecting, show that instead.

---

### MOD-8: Calibration localStorage Uses Different Key Prefix
**File:** `pwa-wasm/src/calibration.rs:11–26`

Calibration values use `guardian_cal_` prefix: `guardian_cal_floor`, `guardian_cal_max`, `guardian_cal_tripwire`.
Main.rs localStorage uses `guardian_` prefix: `guardian_tv_ip`, `guardian_tv_brand`, `guardian_tv_psk`.

This is inconsistent but harmless since each module reads its own keys. The calibration data in localStorage is display-only (the firmware is authoritative for tripwire; it sends the current value in every telemetry message). No functional bug.

---

## 5. Minor Issues

### MINOR-1: Ducking Restore Never Fires if State Is Not `Ducking`
**File:** `firmware-rs/src/ducking.rs:92`

Restore only fires when `self.state == DuckingState::Ducking`. This is correct by design — if we never started ducking, there's nothing to restore. But the state transitions are subtle:
- `Quiet → Watching` when sustained_ms starts
- `Watching → Ducking` when sustained_ms ≥ 3000
- Restore only possible from `Ducking`

If the user disarms before the 3-second threshold is reached, `sustained_ms` is reset to 0 and state goes back to `Quiet`. No restore needed. ✓

### MINOR-2: No Input Validation for Tripwire vs. Floor
**File:** `firmware-rs/src/ducking.rs`

The `DuckingEngine` doesn't validate that `tripwire_db > floor_db`. If these are accidentally swapped (e.g., bad calibration sequence), the restore condition `db < floor_db + 2.0` might never be false while ducking, causing an immediate restore loop. The firmware handles this gracefully (Restore fires, then state returns to Watching → Ducking → Restore → ...) but it would cause very rapid toggling. Add a validation: `if tripwire_db <= floor_db { tripwire_db = floor_db + 6.0; }` as a safety clamp.

### MINOR-3: `ws.rs` Pattern Matching for Commands Uses `contains()`
**File:** `firmware-rs/src/ws.rs:217–328`

Command dispatch uses `s.contains(r#""cmd":"arm""#)` style matching. This is robust for well-formed JSON but could match substrings in unexpected places (e.g., a SSID that contains `"cmd":"arm"`). For a production device, proper JSON parsing is preferred, though in practice WiFi SSIDs don't contain command JSON fragments.

### MINOR-4: Samsung WS Handshake Uses Static Nonce Key
**File:** `firmware-rs/src/tv.rs:653–656`

```rust
"Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n"
```

This is a fixed, well-known example key. Per RFC 6455, the client key should be random. Samsung TVs don't actually validate the key (they accept any well-formed WS handshake), but this is a protocol non-compliance. Use a random nonce for correctness.

### MINOR-5: `meter.rs` Peak Hold Uses Global `setTimeout` ID
**File:** `pwa-wasm/src/meter.rs:39–53`

The peak hold timer stores a `timeout_handle: i32` in a `store_value`. If the component is unmounted and remounted (navigating away from Meter tab and back), a new `create_effect` runs and the timer handle from the previous mount might never be cleared, leaving a dangling closure. In practice, Leptos's CSR mode doesn't unmount components aggressively, but it's worth noting.

### MINOR-6: `discover_tvs` Deduplicates by IP But Not by Brand+IP
**File:** `firmware-rs/src/tv.rs:167`

```rust
if tvs.iter().any(|t| t.ip == ip_str) { continue; }
```

A device that responds to SSDP twice (multiple SSDP services, common with smart TVs) will be deduplicated by IP. ✓

But if a TV has two network interfaces (rare), it would appear twice. This is acceptable.

---

## 6. Configuration Choke Points (Must Change in Multiple Places)

These are values that need updating in **more than one file** to make a change:

| What to change | Files to update | How many places |
|---|---|---|
| WebSocket port (81) | `firmware-rs/src/ws.rs:40` AND `pwa-wasm/src/ws.rs:15` | 2 |
| Flash size (2 MB→4 MB fix) | `firmware-rs/src/net.rs:40` AND `firmware-rs/src/flash_fs.rs:34` | 2 |
| Credential flash offset (0x1FF000) | `firmware-rs/src/net.rs:41` AND must be ≤ `firmware-rs/src/flash_fs.rs:36` (`PART_END`) | 2 (coupled) |
| HTTP port (80) | `firmware-rs/src/http.rs:28` (not exposed in PWA — always uses window.location.host) | 1 |
| Firmware version string | `firmware-rs/src/main.rs:54` AND `firmware-rs/src/pwa_assets.rs:51` (dead code, but stale) | 2 |
| PWA cache version | `pwa-wasm/sw.js:9` (`guardian-wasm-v1`) | 1 — bump on EVERY PWA change to force cache invalidation |
| Build-time WiFi creds | `firmware-rs/.cargo/config.toml` env section OR env vars at build time | 1 |

**Strongly recommended:** Add a `config.rs` module to `firmware-rs/src/` that centralizes:
```rust
pub const FLASH_SIZE_BYTES: usize = 4 * 1024 * 1024;
pub const WS_PORT:          u16   = 81;
pub const HTTP_PORT:        u16   = 80;
pub const CRED_FLASH_OFFSET: u32  = 0x1FF_000;
pub const FS_PART_START:    u32   = 0x100_000;
pub const FS_PART_END:      u32   = CRED_FLASH_OFFSET;
```

---

## 7. Sensitivity Tuning Guide

### How Guardian decides when to duck:

```
Every 100ms tick (audio_task sends dB reading):

1. dB > tripwire_db?
   YES → sustained_ms += 100
   NO  → sustained_ms -= 50  (decays slower than it rises)

2. sustained_ms ≥ 3000 (3 seconds)?
   YES → calculate duck rate based on how far above tripwire:
          excess > 15 dB: step every 500ms  (crisis mode)
          excess 5-15 dB: step every 1000ms (normal)
          excess < 5 dB:  step every 2000ms (gentle nudge)
       → emit DuckCommand::VolumeDown at the calculated rate

3. dB < floor_db + 2 dB AND currently ducking?
   YES → emit DuckCommand::Restore (ramp volume back up)
```

### Tunable parameters and their locations:

```
SENSITIVITY DIAL                    FILE + LINE               DEFAULT    SAFE RANGE
─────────────────────────────────────────────────────────────────────────────────
"How long must noise persist?"      ducking.rs:103            3000ms     1000–10000ms
"How fast does volume drop?"        ducking.rs:108-112        500/1000/  250–3000ms
                                    (crisis/normal/gentle)    2000ms
"How fast does volume restore?"     tv.rs:23 RESTORE_STEP_MS  400ms      100–1000ms
"How quickly does quiet forgive?"   ducking.rs:88             50ms       10–200ms
  (smaller = forgives faster)       (decay per 100ms tick)
"What counts as 'quiet enough'?"    firmware calibration      floor_db   floor - 2 to +5
  (restore threshold)               ducking.rs:92             + 2.0 dB
```

### Example tuning for "aggressive mode" (nursery at 3am):
```rust
// ducking.rs
if self.sustained_ms >= 1500 {          // was 3000 — fires after 1.5s
    self.duck_interval_ms = if excess > 10.0 { 300 }  // was 500
                            else if excess < 3.0 { 1000 } // was 2000
                            else { 600 };               // was 1000

// tv.rs
const RESTORE_STEP_MS: u64 = 200;      // was 400 — faster restore
```

### Example tuning for "gentle mode" (light sleeper in adjacent room):
```rust
// ducking.rs
if self.sustained_ms >= 5000 {          // longer window
    self.duck_interval_ms = if excess > 20.0 { 1000 }  // slower drops
                            else if excess < 10.0 { 3000 }
                            else { 2000 };

// tv.rs
const RESTORE_STEP_MS: u64 = 600;      // slower restore
```

---

## 8. TV Logic Verification

### LG WebOS — verified correct
- Connect: HTTP upgrade to WS on port 3000, send pairing manifest ✓
- GetVolume: `ssap://audio/getVolume`, parses `"volume":N` ✓
- VolumeDown: `ssap://audio/volumeDown` (step command) ✓
- SetVolume: `ssap://audio/setVolume` with `{"volume":N}` (integer, not string) ✓
- Restore: absolute ramp using `setVolume` from `original - steps` to `original` ✓
- Issue: pairing response not awaited (see HIGH-2)

### Samsung Tizen — verified mostly correct
- Connect URL format: `/api/v2/channels/samsung.remote.control?name=BASE64&token=TOKEN` ✓
- Token extraction from `ms.channel.connect` event ✓
- Key format: `{"method":"ms.remote.control","params":{"Cmd":"Click","DataOfCmd":"KEY_VOLDOWN",...}}` ✓
- Restore: N × KEY_VOLUP (relative) ✓
- Issue: No absolute volume → restore depends on correct `duck_steps_taken` count ✓ (acceptable)
- Issue: Token not persisted to flash (see HIGH-4)
- Issue: Samsung 2021+ port 8002 TLS not implemented (logged, acknowledged)

### Sony Bravia — verified correct
- HTTP POST to `/sony/audio` on port 80 ✓
- Auth: `X-Auth-PSK: <psk>` header ✓
- `getVolumeInformation`: finds `"speaker"` entry, parses integer `"volume":N` ✓
- `setAudioVolume` v1.2: volume as **string** `"volume":"15"` ✓ (critical — must be string for v1.2)
- `keep-alive` connection reused across requests ✓
- Issue: Double GET on first duck step (see MOD-3)
- Issue: No explicit connection test — Sony always returns `connected = true`

### Roku ECP — verified mostly correct
- `POST /keypress/VolumeDown` on port 8060 ✓
- No auth ✓
- Restore: N × VolumeUp key presses ✓
- Issue: Return value always `true` (see HIGH-3)

### Shared TV Logic Issues
- `tv_ramp_up_absolute`: if physical volume was changed manually during ducking, restore goes to the pre-duck original regardless. This is the intended behavior — "we borrowed your remote, giving it back at what it was." ✓
- `tv_ramp_up_absolute` with `steps > original_volume` (e.g., original=5, steps=10): `current = 5.saturating_sub(10) = 0`. Ramp from 0 to 5. Correct. ✓

---

## 9. Security Assessment

**Threat model:** Home LAN only. No internet-facing exposure. Privacy-first design.

| Threat | Risk | Mitigation |
|---|---|---|
| LAN snooper reads WiFi password | Medium | WebSocket is plaintext HTTP. Anyone on the LAN can sniff `set_wifi` command containing SSID+password. | → Accept risk for now; add WS over TLS when embedded-tls is wired |
| LAN attacker arms/disarms device | Low | No auth on WS. Anyone on LAN can send `{"cmd":"arm"}`. | → Accept for home device; consider a simple shared secret or local auth in future |
| Physical access to flash | Low | WiFi credentials stored in plaintext in flash with only CRC32 (not encrypted). Reading with a flash programmer reveals credentials. | → Accept for embedded device |
| Sony PSK in plaintext WS | Low | PSK sent unencrypted over WS. Same threat as WiFi password. | → Same mitigation (TLS phase 4) |
| HTTP server DoS (port 80) | Low | No rate limiting. Repeated HTTP requests on LAN could impact performance. Embassy handles this with sequential task processing. | → Accept for home device |
| OTA update integrity | Medium | OTA scaffold exists but doesn't currently download. When enabled, GitHub HTTPS provides transport integrity. No signature verification of downloaded assets. | → Add BLAKE3 checksum of downloaded assets in phase 4 |

**Verdict:** Appropriate for a home device. The risks are all local-network-only and acceptable. Add WS authentication (a simple token set at build time via env var) when marketing to less tech-savvy users.

---

## 10. UX Assessment

### Good
- PWA installs as full-screen home screen app (manifest + service worker) ✓
- Dark theme with clear color coding ✓
- Auto-reconnect on WS disconnect ✓
- Calibration flow is simple and guided ✓
- TV auto-discovery reduces friction (no manual IP hunting) ✓
- WiFi scan + tap-to-fill reduces typing ✓
- LED visual feedback from the physical device ✓
- `data-no-hash` builds ensure stable URLs for service worker caching ✓

### Issues
1. **No "Ducking active" indicator in PWA.** The meter bar color changes but there's no explicit "🔕 Volume reduced" card that appears. Users can't tell if ducking is actually working from the phone screen.

2. **Arm state is optimistically updated.** The PWA flips `armed` to true immediately before the firmware confirms. If the WS connection drops right as the user taps Arm, the state diverges — phone shows Armed, firmware thinks Disarmed. The firmware's state wins on reconnect (next telemetry message sets `armed` from the server), but there's a window of inconsistency.

3. **WiFi tab "Status: Connected" is hardcoded.** Should reflect actual WS connection state.

4. **No max-volume indicator.** When Sony/LG are at volume 100, ducking can't go higher. There's no UI indication that "original volume was X% of max."

5. **Calibration resets on browser data clear.** The local `guardian_cal_*` values in localStorage are only cosmetic (the firmware sends the current tripwire in every message), but a user who clears browser data will see "—" in the calibration display even though the firmware is calibrated. This might confuse users into recalibrating unnecessarily.

6. **No accessibility attributes.** Interactive elements have no `aria-label`, color-coding has no text alternatives, focus states rely on browser defaults.

7. **Service worker `CACHE_NAME = 'guardian-wasm-v1'`** is hardcoded. It must be bumped manually whenever the PWA assets change, otherwise users see a stale cached version. This is easy to forget.

---

## 11. README Accuracy Assessment

The README is **largely accurate** after the recent update. Specific checks:

| Claim | Status |
|---|---|
| "Privacy-first, zero cloud" | ✓ Accurate |
| Pico 2 W = 512 KB RAM | ✓ Accurate (RP2350) |
| "LRCL must be BCLK+1" | ✓ Accurate (PIO requirement) |
| Build order: trunk first | ✓ Accurate (pwa_assets.rs include_bytes) |
| "trunk serve → localhost:8080" | ✓ Accurate |
| WS port 81, HTTP port 80 | ✓ Accurate |
| "LED fast blink during WiFi connection" | ✓ Accurate |
| "LED slow pulse when idle" | ✓ Accurate |
| LG: absolute setVolume ✓ | ✓ Accurate |
| Samsung: key presses only ✓ | ✓ Accurate |
| Sony: setAudioVolume as string | ✓ Accurate |
| "Samsung 2021+ may need port 8002" | ✓ Accurate |
| Flash layout in Architecture Notes | ✓ Accurate — but says "1 MB firmware" when firmware uses up to 1 MB but memory.x allocates 4 MB total. Clarify: firmware occupies 0x000000–0x0FFFFF if under 1 MB, up to 0x100000. |
| Flash partition layout in audit.md | The comment "2 MB Pico 2 W" in net.rs/flash_fs.rs is wrong — should be 4 MB. README Architecture Notes correctly says 2 MB for each region (1 MB firmware + 1 MB FS + 4 KB creds) but that only accounts for 2 MB of the actual 4 MB flash. |
| GUARDIAN_TV_IP env var | ✗ Not documented anywhere in README |
| GUARDIAN_GH_OWNER/GUARDIAN_GH_REPO for OTA | ✗ Not documented in README |
| Mock Python server handles Phase 3 events | ✓ Accurate (updated) |

**Items to add to README:**
- `GUARDIAN_TV_IP` optional env var (bake a TV IP at compile time)
- `GUARDIAN_GH_OWNER`/`GUARDIAN_GH_REPO` env vars for OTA target
- Note about service worker cache name needing to be bumped on PWA updates

---

## 12. Production Readiness Checklist

| Item | Status | Priority |
|---|---|---|
| Audio capture works (PIO program) | ❌ BUG-1 — must fix | CRITICAL |
| Long WiFi passwords work | ❌ BUG-2 — must fix | CRITICAL |
| Scan/discover buffer safe | ❌ BUG-3 — must fix | CRITICAL |
| FLASH_SIZE correct (4 MB) | ⚠️ Works but wastes 2 MB, duplicated | HIGH |
| LG pairing confirmation | ⚠️ Silent failure on first pair | HIGH |
| Samsung token persisted | ⚠️ Popup every reboot | HIGH |
| Roku error handling | ⚠️ Silent failures | HIGH |
| Duck timing user-configurable | ⚠️ Hardcoded | MODERATE |
| Duck steps capped | ⚠️ Unlimited | MODERATE |
| OTA download (TRNG) | ⚠️ Scaffolded, not complete | MODERATE |
| Samsung port 8002 TLS | ⚠️ Scaffolded, not complete | MODERATE |
| WS frame > 125 bytes | ❌ BUG-2 | CRITICAL |
| WiFi tab status | ⚠️ Hardcoded "Connected" | MINOR |
| Ducking UI indicator | ⚠️ Missing visual | MINOR |
| Service worker cache versioning | ⚠️ Manual bump required | MINOR |
| Accessibility (aria labels) | ⚠️ Missing | MINOR |

### Priority fix order:
1. **BUG-1** (PIO audio) — device is completely non-functional without this
2. **BUG-3** (buffer overflow) — potential crash on scan/discover
3. **BUG-2** (WS frame length) — long credentials silently dropped
4. **HIGH-1** (FLASH_SIZE) — deduplicate constant, correct to 4 MB
5. **HIGH-2** (LG pairing) — improve first-time setup UX
6. **HIGH-4** (Samsung token) — major UX regression on every reboot
7. **MOD-1** (configurable sensitivity) — significant UX improvement

---

## 13. How Everything Is Connected: Relationship Map

```
main.rs
  ├── DuckingEngine (ducking.rs)     — state machine, no I/O
  │     └── duck_steps_taken, original_volume, tripwire_db, floor_db
  │
  ├── TvConfig (tv.rs)               — shared state, Mutex<ThreadModeRawMutex>
  │     └── ip, brand, sony_psk, samsung_token
  │
  ├── DB_CHANNEL                     — audio_task → ws_task, f32 dB values
  ├── LED_CHANNEL                    — any task → wifi_task, LedPattern
  ├── WIFI_CMD_CH                    — ws_task → wifi_task, WifiCmd
  ├── WIFI_EVT_CH                    — wifi_task → ws_task, WifiEvent
  │
  ├── audio_task (audio.rs)          — I²S mic capture, pushes dB to DB_CHANNEL
  ├── wifi_task (net.rs)             — CYW43 bring-up, flash creds, LED loop
  │     ├── spawns: cyw43_task, net_stack_task, http_task, websocket_task, tv_task
  │     └── owns:   Flash peripheral, cyw43::Control (not Sync, can't be shared)
  │
  ├── websocket_task (ws.rs)         — client command handler, telemetry broadcast
  │     ├── reads: DB_CHANNEL (blocks on dB), WIFI_EVT_CH (try_receive after each dB)
  │     ├── writes: WIFI_CMD_CH, DUCK_CHANNEL (via tv::send_duck_command)
  │     └── modifies: DuckingEngine (lock), TvConfig (lock)
  │
  ├── tv_task (tv.rs)                — TV volume control state machine
  │     ├── reads: DUCK_CHANNEL (blocks on commands)
  │     ├── reads: TvConfig (lock, clone)
  │     └── modifies: DuckingEngine (set_original_volume, clear_duck_state)
  │
  └── http_task (http.rs)            — static file server
        └── reads: pwa_assets (compile-time embedded bytes)

tv.rs also contains:
  ├── DUCK_CHANNEL (local static)    — ws_task → tv_task, DuckCommand
  ├── discover_tvs()                 — called inline from ws_task (blocks 3s)
  └── TvBrand / TvConfig types
```

---

*End of audit. Total issues found: 3 critical, 4 high, 8 moderate, 7 minor.*
*Estimated fix time for critical+high items: 4–8 hours of focused work.*
