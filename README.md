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
├── firmware-rs/             Phase 2: Rust + Embassy firmware  ← ACTIVE
│   ├── Cargo.toml           embassy-rp (RP2350), embassy-net, cyw43
│   ├── memory.x
│   └── src/
│       ├── main.rs          Entry point, task spawning, static cells
│       ├── audio.rs         PIO I²S + RMS → dBFS, pushes to DB_CHANNEL
│       ├── ducking.rs       Adaptive ducking state machine
│       ├── net.rs           CYW43 WiFi bring-up, spawns http/ws/tv tasks
│       ├── ws.rs            WebSocket server (port 81), Embassy select loop
│       ├── tv.rs            LG / Samsung / Sony / Roku volume control
│       ├── http.rs          HTTP server (port 80), serves WASM PWA
│       └── pwa_assets.rs    include_bytes! from pwa-wasm/dist/
│
└── pwa-wasm/                Phase 2: Leptos + WASM PWA  ← ACTIVE
    ├── Cargo.toml           leptos 0.7 csr, gloo-net, futures, web-sys
    ├── Trunk.toml           builds to dist/ with fixed filenames (data-no-hash)
    ├── index.html
    ├── sw.js                Service worker — enables "Add to Home Screen"
    ├── manifest.json
    ├── icon-192.png
    ├── icon-512.png
    └── src/
        ├── main.rs          App root, 5 tabs, localStorage, event log
        ├── ws.rs            WebSocket client, drives all reactive signals
        ├── meter.rs         Live dB bar, peak hold, arm/disarm
        ├── calibration.rs   Two-step calibration + manual slider
        ├── tv.rs            Brand selection, IP/PSK entry, connect/disconnect
        ├── wifi.rs          WiFi network switching
        └── info.rs          Versions, WS state, full event log
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

## Phase 2 — Rust + Embassy Firmware + Leptos PWA

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

### First boot

1. Pico connects to your WiFi network
2. Open `http://guardian.local` in Safari (or the device IP if mDNS doesn't work)
3. Tap Share → **Add to Home Screen** → opens full-screen, no browser chrome

---

## The PWA — 5 Tabs

| Tab | What it does |
|---|---|
| **Meter** | Live dBFS bar, tripwire marker, peak hold, arm/disarm button, recent events |
| **Calibrate** | Step 1: record silence, Step 2: record TV max → auto-sets tripwire 3 dB below |
| **TV** | Select brand, enter IP, connect → Pico controls volume via WiFi |
| **WiFi** | Switch the Pico to a different WiFi network |
| **Info** | Firmware/PWA versions, WebSocket state, full event log |

---

## TV Support

| Brand | How it works | Pairing |
|---|---|---|
| **LG WebOS** | WebSocket SSAP on port 3000, absolute `setVolume` | TV shows popup on first connect |
| **Samsung** | WebSocket Smart Remote on port 8001, key presses | TV shows popup, saves token for session |
| **Sony Bravia** | HTTP JSON-RPC on port 80, absolute `setAudioVolume` | Set a PIN in TV Settings → IP Control |
| **Roku** | ECP HTTP on port 8060, key presses | No pairing needed |

**LG and Sony** support absolute volume restore (snaps back to original level).
**Samsung and Roku** restore by replaying N × VolumeUp key presses.

For Sony, you must first enable IP Control:
> TV Settings → Network → Home Network Setup → IP Control → Simple IP Control: **On** → set a Pre-Shared Key PIN

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

### Step 1 — Run the dev server

```bash
cd pwa-wasm
trunk serve
# Opens at http://localhost:8080
# Will show "Connecting to Guardian…" banner (no firmware yet)
```

### Step 2 — Optional: mock firmware with a Python WebSocket server

```bash
pip install websockets
```

```python
# mock_ws.py — paste and run: python3 mock_ws.py
import asyncio, websockets, json, math

async def handler(ws):
    t = 0
    async for _ in ws:  # absorb any commands sent by the PWA
        pass

async def broadcast(ws):
    t = 0
    while True:
        db = -35.0 + 15.0 * math.sin(t * 0.05)   # sine-wave meter for testing
        msg = json.dumps({
            "db":       round(db, 2),
            "armed":    False,
            "tripwire": -20.0,
            "fw":       "0.2.0",
            "pwa":      "0.1.0",
        })
        try:
            await ws.send(msg)
        except Exception:
            break
        await asyncio.sleep(0.1)
        t += 1

async def serve(ws):
    await asyncio.gather(broadcast(ws), handler(ws))

asyncio.run(websockets.serve(serve, "localhost", 81).__aenter__().__await__().__next__())
```

With the mock server running, the meter bar will animate and you can navigate all tabs.

> **TV control from the laptop**: not possible — the TV commands run on the Pico firmware.
> If you want to verify your TV responds to volume commands before the Pico arrives,
> see the TV-specific curl examples in the Troubleshooting section below.

---

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| I²S buffer all zeros | LRCL not BCLK+1, or 5V on mic | Check wiring: 3.3V only, LRCL=GP1 |
| guardian.local not found | mDNS unreliable on some routers | Use device IP from serial monitor |
| WebSocket won't connect | Phone on different subnet | Ensure phone and Pico are on same WiFi |
| TV not pairing | IP wrong or TV asleep | Wake TV, verify IP in router DHCP table |
| Sony volume not changing | PSK mismatch | Re-enter PIN from TV Settings → IP Control |
| Samsung popup doesn't appear | Port 8001 blocked or newer model | Some 2021+ Samsung use port 8002 (TLS — Phase 3) |
| trunk build fails | Leptos API mismatch | Run `rustup update` then rebuild |
| cargo build fails with "no such file" | pwa-wasm/dist/ missing | Run `trunk build --release` in pwa-wasm/ first |

### Manually test TV APIs (before Pico arrives)

**LG WebOS** — find IP in TV Settings → All Settings → Network → Wired/Wireless Connection:
```bash
# LG uses WebSocket on port 3000 — hard to curl, use a WS client
# Or just trust the firmware; LG pairing is reliable
```

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
- [ ] Phase 2: meter animates in Safari when tapping near mic
- [ ] Phase 2: app installs full-screen from Home Screen (no browser chrome)
- [ ] Phase 2: WebSocket auto-reconnects after WiFi drop
- [ ] Phase 2: calibration persists after closing and reopening app
- [ ] Phase 2: 4-second clap → ducking fires; 1-second clap → no ducking
- [ ] Phase 2: TV tab → connect → TV volume drops on sustained noise

---

## Phase 3 Roadmap

- **OTA updates** — download new PWA from GitHub Releases, write to LittleFS, no reflash
- **WiFi captive portal** — in-app WiFi switching (firmware receives `set_wifi` command, reboots into AP mode)
- **Samsung token persistence** — save pairing token to flash so popup only appears once ever
- **Samsung port 8002** — TLS support for newer 2021+ models
- **SSDP/mDNS TV discovery** — auto-detect TVs on the network, no manual IP entry
- **LED status indicator** — blink pattern for armed/ducking state (via CYW43 GPIO_0)
