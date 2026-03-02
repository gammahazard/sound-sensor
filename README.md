# Guardian — Sound Sensor System

Privacy-first nursery monitor. Listens at a baby's door, automatically ducks
the living room TV volume when sustained loud noise is detected. Runs entirely
on your home LAN — zero cloud, zero subscriptions.

---

## How It Works

```
SPH0645 mic → Pico 2 W → dBFS reading every 100ms
                       → WebSocket (port 81) → Leptos/WASM PWA on your phone
                       → If sustained noise > tripwire for 3s → duck TV volume
                       → TV (LG / Samsung / Sony / Roku) via WiFi API
```

The Pico runs the firmware (audio sampling, ducking logic, TV control).
The PWA is the control panel — served by the Pico itself over HTTP.
Your phone never leaves the LAN.

---

## Hardware

### Development unit (what to order)

| Part | Notes |
|---|---|
| Raspberry Pi Pico 2 W (RP2350) | 512 KB RAM, good for iteration |
| SPH0645LM4H (Adafruit breakout) | 24-bit I²S digital microphone |
| USB-C wall adapter (5V) | Power |

### Production unit (once firmware is proven)

| Part | Notes |
|---|---|
| **Raspberry Pi Pico W (RP2040)** | ~15× cheaper, same CYW43439 WiFi chip |
| SPH0645LM4H (Adafruit breakout) | Same mic, same wiring |

#### Porting Pico 2 W → Pico W (three edits)

`firmware-rs/Cargo.toml` — change `rp2350` → `rp2040`
`firmware-rs/.cargo/config.toml` — change target to `thumbv6m-none-eabi`
`firmware-rs/memory.x` — change RAM length to `264K`

### Wiring

```
SPH0645 Pin  │  Pico Pin  │  Notes
─────────────┼────────────┼──────────────────────────────────────
3V           │  Pin 36    │  3.3V only — NOT 5V
GND          │  Any GND   │
BCLK         │  GP0       │  Bit clock (master out)
LRCL         │  GP1       │  Word select (MUST be BCLK+1)
DOUT         │  GP2       │  Serial data mic → Pico
SEL          │  GND       │  GND = left channel
```

> **Critical**: LRCL must be exactly one GPIO above BCLK (adjacent numbers).

---

## Project Structure

```
sound-sensor/
├── phase0_micropython/
│   └── test_i2s.py         Phase 0: hardware verification (MicroPython)
│
├── firmware-rs/             Rust + Embassy firmware (v0.3.0)
│   ├── Cargo.toml           embassy-rp (RP2350), embassy-net, cyw43
│   ├── memory.x
│   └── src/
│       ├── main.rs          Entry point, channels, LED patterns, enums
│       ├── audio.rs         PIO I²S + RMS → dBFS (pio_proc asm)
│       ├── ducking.rs       Adaptive ducking state machine + validation
│       ├── net.rs           WiFi bring-up, flash creds, LED loop, WiFi commands
│       ├── ws.rs            WebSocket server (port 81), 10 commands, events
│       ├── tv.rs            LG / Samsung / Sony / Roku + SSDP discovery
│       ├── http.rs          HTTP server (port 80), serves PWA + OTA endpoint
│       ├── flash_fs.rs      Append-only flash file store (0x100000–0x1FF000)
│       ├── ota.rs           OTA version check scaffold
│       └── pwa_assets.rs    include_bytes! from pwa-wasm/dist/
│
├── pwa-wasm/                Leptos + WASM PWA (v0.1.0)
│   ├── Cargo.toml           leptos 0.7 csr, gloo-net, futures, web-sys
│   ├── Trunk.toml           builds to dist/ with fixed filenames (data-no-hash)
│   ├── index.html
│   ├── sw.js                Service worker — enables "Add to Home Screen"
│   ├── manifest.json
│   ├── icon-192.png
│   ├── icon-512.png
│   └── src/
│       ├── main.rs          App root, 5 tabs, signals, event log
│       ├── ws.rs            WebSocket client, events, OtaStatus, NetworkInfo
│       ├── meter.rs         Live dB bar, ducking banner, peak hold, arm/disarm
│       ├── calibration.rs   Two-step calibration + placement reminder
│       ├── tv.rs            Brand selection, SSDP discover, IP entry, connect
│       ├── wifi.rs          WiFi scan, network list, credentials form
│       └── info.rs          Versions, OTA check button, full event log
│
└── mock_ws.py               Mock firmware WS server for UI testing
```

---

## Prerequisites

```bash
# Rust toolchain
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Firmware target (Pico 2 W = Cortex-M33)
rustup target add thumbv8m.main-none-eabihf

# Firmware flashing
cargo install probe-rs-tools flip-link

# WASM PWA build tool
cargo install trunk
rustup target add wasm32-unknown-unknown
```

---

## Phase 0 — Hardware Verification

Before doing anything else, verify the mic works with MicroPython.

1. Download MicroPython `.uf2` for Pico 2 W from https://micropython.org/download/RPI_PICO2_W/
2. Hold BOOTSEL, plug in USB → drag `.uf2` to the RPI-RP2 drive
3. Wire SPH0645 per the table above
4. Open `phase0_micropython/test_i2s.py` in Thonny and Run
5. Clap near the mic — REPL should show changing RMS values

**Pass**: non-zero, varying values when noise is made.

---

## Building — Rust + Embassy Firmware + Leptos PWA

### Download CYW43 firmware blobs

The WiFi chip needs firmware blobs that are too large to include in the repo.

```bash
mkdir firmware-rs/cyw43-firmware
# Download from the embassy-rs repo:
# https://github.com/embassy-rs/embassy/tree/main/cyw43-firmware
# Place these two files in firmware-rs/cyw43-firmware/:
#   43439A0.bin
#   43439A0_clm.bin
```

### Build order (important — PWA must be built first)

```bash
# 1. Build the WASM PWA
cd pwa-wasm
trunk build --release
# Output: pwa-wasm/dist/  (guardian_pwa.js, guardian_pwa_bg.wasm, index.html, etc.)

# 2. Build and flash the firmware
cd ../firmware-rs
GUARDIAN_SSID="YourWiFiName" GUARDIAN_PASS="YourPassword" cargo run --release
```

The firmware `include_bytes!`s the files from `pwa-wasm/dist/` at compile time.
If you skip step 1, the firmware build will fail with "no such file" errors.

### Environment variables (all optional)

| Variable | Default | Purpose |
|---|---|---|
| `GUARDIAN_SSID` | `MyHomeNetwork` | WiFi SSID (compile-time fallback) |
| `GUARDIAN_PASS` | `password` | WiFi password (compile-time fallback) |
| `GUARDIAN_TV_IP` | (empty) | Default TV IP |
| `GUARDIAN_GH_OWNER` | `gammahazard` | GitHub owner for OTA checks |
| `GUARDIAN_GH_REPO` | `sound-sensor` | GitHub repo for OTA checks |

WiFi credentials saved in-app (via the WiFi tab) are persisted to flash and take priority over compile-time values.

### First boot

1. Pico connects to your WiFi network (LED blinks fast during connect)
2. Open `http://guardian.local` in Safari (or the device IP if mDNS doesn't work)
3. Tap Share → **Add to Home Screen** → opens full-screen, no browser chrome

---

## The PWA — 5 Tabs

| Tab | What it does |
|---|---|
| **Meter** | Live dBFS bar, tripwire marker, peak hold, ducking banner, arm/disarm, recent events |
| **Calibrate** | Placement reminder, Step 1: record silence, Step 2: record TV max, manual slider |
| **TV** | SSDP auto-discover, brand selection, IP entry, Sony PSK, connect/disconnect |
| **WiFi** | Scan networks, signal strength bars, tap to autofill, change credentials |
| **Info** | Firmware/PWA versions, WebSocket state, OTA check button, full event log |

---

## TV Support

| Brand | How it works | Pairing | Volume Restore |
|---|---|---|---|
| **LG WebOS** | WebSocket SSAP on port 3000 | TV popup on first connect | Absolute (setVolume) |
| **Samsung** | WebSocket Smart Remote on port 8001 | TV popup, token saved to flash | Relative (N × VolumeUp) |
| **Sony Bravia** | HTTP JSON-RPC on port 80 | Set a PIN in TV Settings → IP Control | Absolute (setAudioVolume) |
| **Roku** | ECP HTTP on port 8060 | No pairing needed | Relative (N × VolumeUp) |

**LG and Sony** restore to exact original volume.
**Samsung and Roku** restore by replaying the same number of VolumeUp presses.

Samsung pairing tokens are persisted to flash — the TV popup only appears once.

For Sony, you must first enable IP Control:
> TV Settings → Network → Home Network Setup → IP Control → Simple IP Control: **On** → set a Pre-Shared Key PIN

---

## LED Status (CYW43 GPIO_0 on Pico W/2W)

| Pattern | Meaning |
|---|---|
| Fast blink (200ms) | Connecting to WiFi |
| Slow pulse (100ms on / 2s off) | Idle — connected, not armed |
| Double-flash every 3s | Armed — listening |
| Solid on | Ducking — TV volume reduced |
| 3 rapid blinks | Error (WiFi failed) |

---

## Flash Storage

| Region | Size | Purpose |
|---|---|---|
| 0x000000–0x0FFFFF | 1 MB | Firmware binary |
| 0x100000–0x1FEFFF | ~1 MB | Flash FS (OTA PWA assets) |
| 0x1FF000–0x1FFFFF | 4 KB | Config block (WiFi creds + TV config + CRC32) |
| 0x200000–0x3FFFFF | 2 MB | Reserved |

The config block stores WiFi SSID/password and TV settings (IP, brand, Samsung token, Sony PSK). All fields are CRC32-protected and survive reboots.

---

## Calibration Workflow

1. Open Guardian app → **Calibrate** tab
2. **Step 1** — Make the room completely quiet, tap "Record Quiet Level"
3. **Step 2** — Turn TV to your preferred max volume, tap "Record TV Volume Level"
4. Tripwire is auto-set 3 dB below your max
5. Switch to **Meter** tab → tap **Arm Guardian**

The 3-second rule: ducking only fires if noise stays above the tripwire for 3 sustained seconds. Brief sounds (a door, a cough) are ignored.

---

## Testing the PWA Without Hardware

You can run the UI locally from your laptop before the Pico arrives.

### Step 1 — Run the mock firmware

```bash
pip install websockets
python3 mock_ws.py
# Listens on ws://localhost:81
```

The mock server simulates all firmware features:
- Sends telemetry 10x/sec with a sine-wave dB meter
- Responds to arm/disarm, calibration, threshold commands
- Returns mock WiFi scan results (4 networks)
- Returns mock SSDP TV discovery (3 TVs)
- Returns OTA check status
- Simulates ducking state when armed

### Step 2 — Run the PWA dev server

```bash
cd pwa-wasm
trunk serve
# Opens at http://localhost:8080
```

With both running, the full UI is interactive — you can navigate all tabs,
arm/disarm, calibrate, discover TVs, scan WiFi, and check for updates.

> **TV control from the laptop**: not possible — the TV commands run on the Pico firmware.
> To verify your TV responds to volume commands before the Pico arrives,
> see the curl examples in the Troubleshooting section below.

---

## WebSocket Protocol

### Telemetry (server → client, 10×/sec)

```json
{"db":-32.5,"armed":false,"tripwire":-20.0,"ducking":false,"fw":"0.3.0","pwa":"0.1.0"}
```

### Events (server → client)

```json
{"evt":"wifi_scan","networks":[{"ssid":"Home","rssi":-45},...]}
{"evt":"discovered","tvs":[{"ip":"192.168.1.100","name":"LG TV","brand":"lg"},...]}
{"evt":"ota_status","checking":false,"available":false,"current":"0.1.0","latest":"0.1.0","fw":"0.3.0"}
{"evt":"wifi_reconfiguring","ssid":"NewNetwork"}
```

### Commands (client → server)

| Command | JSON |
|---|---|
| Arm | `{"cmd":"arm"}` |
| Disarm | `{"cmd":"disarm"}` |
| Calibrate silence | `{"cmd":"calibrate_silence","db":-42.0}` |
| Calibrate max | `{"cmd":"calibrate_max","db":-28.5}` |
| Manual threshold | `{"threshold":-18.0}` |
| Set TV | `{"cmd":"set_tv","ip":"192.168.1.5","brand":"lg"}` |
| Set WiFi | `{"cmd":"set_wifi","ssid":"...","pass":"..."}` |
| Scan WiFi | `{"cmd":"scan_wifi"}` |
| Discover TVs | `{"cmd":"discover_tvs"}` |
| Check OTA | `{"cmd":"ota_check"}` |

---

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| I²S buffer all zeros | LRCL not BCLK+1, or 5V on mic | Check wiring: 3.3V only, LRCL=GP1 |
| guardian.local not found | mDNS unreliable on some routers | Use device IP from serial monitor |
| WebSocket won't connect | Phone on different subnet | Ensure phone and Pico are on same WiFi |
| LED blinks 3× then stops | WiFi credentials wrong | Check GUARDIAN_SSID/PASS or re-enter via WiFi tab |
| TV not pairing | IP wrong or TV asleep | Wake TV, verify IP in router DHCP table |
| Sony volume not changing | PSK mismatch | Re-enter PIN from TV Settings → IP Control |
| Samsung popup every reboot | Token not persisting | Ensure flash write succeeds (check RTT logs) |
| Samsung popup doesn't appear | Port 8001 blocked or newer model | Some 2021+ Samsung use port 8002 (TLS — future) |
| trunk build fails | Leptos API mismatch | Run `rustup update` then rebuild |
| cargo build fails with "no such file" | pwa-wasm/dist/ missing | Run `trunk build --release` in pwa-wasm/ first |
| Ducking fires too easily | Tripwire too low | Recalibrate or raise manual threshold |
| Ducking never fires | Tripwire too high or not armed | Check Meter tab → ensure Armed + tripwire is reasonable |

### Manually test TV APIs (before Pico arrives)

**Sony Bravia** — enable IP Control first, then:
```bash
# Get current volume
curl -X POST http://192.168.1.X/sony/audio \
  -H "X-Auth-PSK: 1234" \
  -H "Content-Type: application/json" \
  -d '{"method":"getVolumeInformation","params":[],"id":1,"version":"1.0"}'

# Set volume to 15
curl -X POST http://192.168.1.X/sony/audio \
  -H "X-Auth-PSK: 1234" \
  -H "Content-Type: application/json" \
  -d '{"method":"setAudioVolume","params":[{"target":"speaker","volume":"15"}],"id":2,"version":"1.2"}'
```

**Roku** — no auth needed:
```bash
curl -X POST http://192.168.1.X:8060/keypress/VolumeDown
curl -X POST http://192.168.1.X:8060/keypress/VolumeUp
```

---

## Verification Checklist

- [ ] Phase 0: non-zero I²S values in MicroPython REPL when clapping
- [ ] Meter animates in Safari when tapping near mic
- [ ] App installs full-screen from Home Screen (no browser chrome)
- [ ] WebSocket auto-reconnects after WiFi drop
- [ ] Calibration persists after closing and reopening app
- [ ] 4-second clap → ducking fires; 1-second clap → no ducking
- [ ] TV tab → connect → TV volume drops on sustained noise
- [ ] TV volume restores when room goes quiet
- [ ] WiFi tab → scan shows nearby networks
- [ ] WiFi tab → change network → Pico reboots and reconnects
- [ ] Samsung pairing token persists across reboots
- [ ] LED patterns match the table above
- [ ] Info tab → Check Updates shows "Up to date"

---

## Security

- **Zero cloud** — all communication is local LAN only
- **No telemetry** — the device does not contact any external servers (OTA check is opt-in)
- **No audio recording** — only RMS dB levels leave the microphone; raw audio is never stored or transmitted
- **Flash credentials** — WiFi passwords and TV tokens stored with CRC32 integrity check
- **No open ports** — only port 80 (HTTP) and port 81 (WebSocket) on LAN
- **JSON input sanitization** — firmware uses substring matching, not a JSON parser; malformed input is silently dropped

---

## Roadmap

- **OTA downloads** — full TLS client to download PWA updates from GitHub Releases (requires TRNG wiring)
- **Samsung port 8002** — TLS support for newer 2021+ Samsung TV models
- **Pico W port** — production build for the cheaper RP2040 board
