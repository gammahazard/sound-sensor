# Guardian Sound Sensor

Privacy-first nursery sound monitor. Place a microphone at your baby's door — Guardian listens for sustained loud noise and automatically ducks your living room TV volume in seconds. When the noise stops, volume restores smoothly. It also detects baby crying using spectral analysis (Goertzel algorithm + harmonic tracking + temporal pattern recognition) and sends a real-time notification to your phone. Runs entirely on your home LAN with zero cloud, zero subscriptions, and zero audio recording.

---

## Table of Contents

- [How It Works](#how-it-works)
- [The Ducking Algorithm](#the-ducking-algorithm)
- [Baby Cry Detection](#baby-cry-detection)
- [Hardware](#hardware)
- [Project Structure](#project-structure)
- [Firmware Architecture](#firmware-architecture)
- [PWA (Web UI) Architecture](#pwa-web-ui-architecture)
- [TV Protocol Support](#tv-protocol-support)
- [Flash Storage Layout](#flash-storage-layout)
- [WebSocket Protocol](#websocket-protocol)
- [LED Status Patterns](#led-status-patterns)
- [Build Instructions](#build-instructions)
- [Testing](#testing)
- [Testing the PWA Without Hardware](#testing-the-pwa-without-hardware)
- [Calibration Workflow](#calibration-workflow)
- [Troubleshooting](#troubleshooting)
- [Security](#security)
- [Roadmap](#roadmap)

---

## How It Works

```
                           ┌──────────────────────────────────────────────────┐
                           │              Raspberry Pi Pico 2 W               │
SPH0645LM4H ──I²S──►      │                                                  │
   (mic at               │  audio_task ──┬── DB_CHANNEL ──► ducking_task    │
   baby's door)          │   (Goertzel   │                    │    │         │
                           │    + Hanning  └── CRY_CHANNEL ──►│    │         │
                           │    + ZCR)           │             │    ▼         │
                           │                     │         tv_task  LED      │
                           │                     │         (LG/Samsung       │
                           │                     │          Sony/Roku)       │
                           │                     ▼                           │
                           │  websocket_task ◄── TELEMETRY + CRY_EVENT_CH   │
                           │      │  (port 81)                               │
                           │  http_task (port 80) ◄─── PWA assets            │
                           └──────┼──────────────────────────────────────────┘
                                  │
                                  ▼
                           Your phone
                           (Leptos/WASM PWA)
```

The Pico 2 W runs seven concurrent Embassy tasks in normal (station) mode, or nine in AP setup mode:

1. **audio_task** — Captures 24-bit I²S audio from the SPH0645 microphone at 16 kHz using a PIO state machine. Applies a 2nd-order Butterworth high-pass filter (200 Hz cutoff) per-sample. Every 100ms (1,600 samples) computes RMS dBFS with EMA smoothing and pushes to `DB_CHANNEL`. Also runs 10 Goertzel bins (5 F0 + 5 harmonic) with Hanning windowing, zero-crossing rate counting, and energy accumulation for volume-independent baby cry spectral detection (5-check pipeline), pushing results to `CRY_CHANNEL`.

2. **ducking_task** — Receives dB readings from `DB_CHANNEL` and cry detection results from `CRY_CHANNEL`. Ticks the ducking state machine and the `CryTracker` temporal pattern detector. Updates the shared `TELEMETRY` snapshot for the WebSocket task.

3. **websocket_task** — Reads telemetry from the shared snapshot, broadcasts JSON to the connected PWA at 10 Hz, sends one-shot `baby_cry` events via `CRY_EVENT_CH`, and processes incoming commands (arm, disarm, calibrate, set TV, etc.).

4. **tv_task** — Receives duck/restore commands from the ducking engine and executes them against the configured TV using the appropriate protocol (WebSocket for LG/Samsung, HTTP for Sony/Roku).

5. **wifi_task** — Initializes the CYW43439 WiFi chip, joins the network, runs the DHCP stack, handles WiFi scan requests, saves credentials to flash, and drives the onboard LED pattern in a 100ms loop.

6. **http_task** — Serves the PWA (HTML + WASM + JS) on port 80. The entire web UI is compiled into the firmware binary via `include_bytes!`, so there are no external file dependencies at runtime. In AP mode, serves the setup page instead.

In AP setup mode, three additional tasks run:

7. **dhcp_server_task** — Minimal DHCP server assigning `192.168.4.2` to connecting phones with a 5-minute lease.
8. **dns_server_task** — Resolves all DNS A queries to `192.168.4.1`, triggering captive portal detection on phones so the setup page auto-opens.
9. **mdns_responder_task** — Announces `guardian.local` (station) or `guardiansetup.local` (AP) via multicast DNS on port 5353. Sends startup announcements and responds to A/ANY queries.

Your phone connects to the Pico's IP address (or `http://guardian.local` if mDNS is working), loads the WASM PWA, and communicates over a WebSocket on port 81. Everything stays on your local network — the Pico never contacts any external server unless you explicitly press the OTA check button.

---

## The Ducking Algorithm

The ducking engine is the heart of Guardian. It runs every 100ms as part of the WebSocket task and makes volume-control decisions based on a simple state machine.

### States

| State | Meaning |
|---|---|
| **Quiet** | No sustained noise detected. Waiting. |
| **Watching** | Noise above tripwire detected, accumulating sustained_ms (0–2999). |
| **Ducking** | Sustained noise confirmed (3+ seconds). Actively sending VolumeDown commands. |
| **Restoring** | Noise has stopped. Volume being restored to original level. |

### Step 1: Accumulate / Decay (every 100ms tick)

```
if dB > tripwire:
    sustained_ms += 100     (noise building up)
else:
    sustained_ms -= 50      (noise fading — decays at half speed)
```

The asymmetric decay (100ms up / 50ms down) means brief dips below the tripwire (e.g., a pause between cries) don't reset the counter. A baby crying in bursts will still accumulate toward the 3-second threshold.

### Step 2: Ducking Trigger (sustained_ms >= 3000)

Once noise has been above the tripwire for a cumulative 3 seconds, Guardian starts sending VolumeDown commands. The rate depends on how far above the tripwire the noise is:

| Excess (dB above tripwire) | VolumeDown interval | Behavior |
|---|---|---|
| > 15 dB (crisis) | Every 500ms | Baby is really upset — drop volume fast |
| 5–15 dB (standard) | Every 1,000ms | Moderate noise — steady reduction |
| < 5 dB (gentle) | Every 2,000ms | Barely over threshold — nudge slowly |

**Why adaptive rates?** A screaming baby 20 dB above threshold needs the TV volume cut quickly (within 2–5 seconds of detection). But if the noise is just barely over the threshold, aggressive ducking would be jarring and unnecessary.

**Baby wake timing:**
- Detection: 3 seconds (filters brief sounds — doors, coughs, footsteps)
- Ramp-down: 2–10 seconds depending on volume excess
- Total loud exposure before significant volume reduction: 5–13 seconds
- Research suggests babies in light sleep tolerate ~10–20 seconds of moderate noise

### Step 3: Restore (two independent paths)

Once ducking has started, Guardian monitors for the right time to restore volume:

**Path A — Near-silence (immediate restore):**
If the room dB drops below `floor + 2 dB`, volume is restored immediately. This handles:
- TV turned off
- Commercial break (much quieter than show)
- User manually muted the TV
- Baby fell back asleep and room is quiet

**Path B — Hold timer (30-second delayed restore):**
If `sustained_ms` decays to 0 (noise has been below tripwire long enough) AND at least 30 seconds have passed since the last VolumeDown command, volume is restored. This handles:
- The loud scene ended, but the ducked TV is still audible above the floor
- Without the 30-second hold, we'd restore immediately, the TV goes loud again, and we'd re-duck 3 seconds later — oscillation

The 30-second hold prevents the duck/restore/duck oscillation loop that would otherwise occur when the ducked TV volume sits between the floor and tripwire.

### Volume Restoration Speed

For brands with absolute volume control (LG, Sony):
- Guardian captures the original TV volume before the first VolumeDown
- On restore, it ramps from the current (reduced) volume back to the original in steps of 1, with 400ms between each step
- Example: original volume 30, ducked to 22 → restores 22→23→24→...→30 over ~3.2 seconds

For brands with relative-only control (Samsung, Roku):
- Guardian counts how many VolumeDown presses were sent (`duck_steps_taken`)
- On restore, it sends the same number of VolumeUp presses, 400ms apart
- Example: 8 VolumeDown presses → 8 VolumeUp presses over ~3.2 seconds

If a restore fails (TV unreachable, connection dropped), Guardian clears the duck state to prevent stale volume data from accumulating across sessions.

### Validation Rules

- **Floor minimum**: `set_floor` rejects values below -80 dB (likely a dead microphone) and clamps to -60 dB
- **Tripwire minimum gap**: Tripwire must be at least `floor + 6 dB`. If you set a tripwire closer to the floor, it's automatically clamped. This prevents false positives from tiny ambient fluctuations.
- **Calibration gap**: `set_tripwire` enforces the 6 dB minimum gap automatically

---

## Baby Cry Detection

Guardian includes a dedicated baby cry detection system that runs independently of the ducking engine. It identifies crying using spectral analysis and temporal pattern recognition, and sends a real-time notification to the app. This is notification-only — it does not affect TV volume.

### Why Not Just Use the dB Level?

A simple volume threshold would trigger on any loud sound — TV action scenes, music, door slams, coughing. Baby cries have distinctive spectral and temporal characteristics that allow reliable discrimination:

- **Fundamental frequency (F0)**: 350–550 Hz (adult male speech is 85–180 Hz, female 165–255 Hz)
- **Harmonics**: Strong energy at exact multiples (2× F0: 700–1100 Hz)
- **Temporal pattern**: Rhythmic ~1 Hz burst cycle (0.5–1.6s of crying, 0.3–0.5s breath pause, repeating)

### Spectral Detection (per 100ms window)

The audio pipeline runs 10 Goertzel bins inline with the existing sample loop — no FFT needed:

| Bins | Frequencies | Purpose |
|---|---|---|
| 5 F0 bins | 350, 400, 450, 500, 550 Hz | Baby cry fundamental range |
| 5 harmonic bins | 700, 800, 900, 1000, 1100 Hz | 2nd harmonic at 2× each F0 |

A **Hanning window** (via recursive cosine oscillator) reduces spectral leakage from -13 dB to -31 dB sidelobes, preventing nearby frequencies from bleeding into cry bins.

Each 100ms window runs a 5-check pipeline (volume-independent — no tripwire gate):

1. **F0 energy**: strongest cry bin > noise floor (meaningful tonal energy)
2. **Adaptive harmonic**: harmonic at 2× the strongest F0 is ≥ 5% of fundamental (confirms tonal source, not broadband noise)
3. **Zero-crossing rate**: 50–130 crossings per window (consistent with 350–550 Hz; rejects low-frequency rumble and high-frequency hiss)
4. **Tonal energy ratio**: F0 + harmonic energy ≥ 0.5% of total energy (cry concentrates energy; broadband noise spreads it)
5. **Spectral peakedness**: best F0 bin is ≥ 1.8× the average of all F0 bins (one dominant frequency, not flat spectrum)

Cry detection is completely decoupled from the ducking tripwire — it runs on spectral characteristics alone, detecting crying at any volume level above the Goertzel noise floor.

### Temporal Pattern (CryTracker)

A single cry-positive window is not enough — a TV scene with a 450 Hz tone would trigger it. The `CryTracker` requires a sustained rhythmic burst-gap pattern:

- **Cry burst**: 3–16 consecutive cry-positive windows (300ms–1.6s)
- **Breath gap**: 2–5 cry-negative windows (200ms–500ms)
- **Confirmed crying**: 3+ completed burst-gap cycles → notification sent
- **Cooldown**: crying flag stays active for 3 seconds after the pattern stops

This means the fastest possible detection is ~3 seconds (3 short burst-gap cycles). A single 1-second TV cry scene will not trigger.

### Always Active, Volume-Independent

Cry detection runs regardless of whether Guardian is armed or disarmed, and regardless of the current dB level relative to the ducking tripwire. It relies purely on spectral characteristics (Goertzel bin powers, harmonic ratios, ZCR, peakedness) rather than volume thresholds. This means a quiet cry across the house will still be detected if it has the right spectral signature. The ducking system (TV volume control) only works when armed, but cry notification always works — parents always get alerted.

### Resource Cost

- **CPU**: ~3% (10 Goertzel multiply-accumulates + Hanning + ZCR per sample at 16 kHz)
- **RAM**: ~130 bytes (10 bins × 12 bytes + 10 bytes CryTracker state)
- **Flash**: ~600 bytes code

---

## Hardware

### Development Unit

| Part | Notes |
|---|---|
| Raspberry Pi Pico 2 W (RP2350) | 512 KB RAM, WiFi via CYW43439 |
| SPH0645LM4H (Adafruit breakout) | 24-bit I²S digital MEMS microphone, 3.3V only |
| USB-C wall adapter (5V) | Power supply |

### Production Unit

| Part | Notes |
|---|---|
| Raspberry Pi Pico W (RP2040) | ~15x cheaper, same CYW43439 WiFi chip |
| SPH0645LM4H (same) | Same mic, same wiring |

#### Porting Pico 2 W to Pico W (three edits)

1. `firmware-rs/Cargo.toml` — change `rp235xa` to `rp2040`
2. `firmware-rs/.cargo/config.toml` — change target to `thumbv6m-none-eabi`
3. `firmware-rs/memory.x` — change RAM length to `264K`

### Wiring

```
SPH0645 Pin  │  Pico Pin  │  Notes
─────────────┼────────────┼──────────────────────────────────────
3V           │  Pin 36    │  3.3V only — NOT 5V (will damage mic)
GND          │  Any GND   │
BCLK         │  GP0       │  Bit clock
LRCL         │  GP1       │  Word select (MUST be BCLK+1 GPIO)
DOUT         │  GP2       │  Serial data (mic → Pico)
SEL          │  GND       │  GND = left channel
```

**Critical**: LRCL must be the GPIO immediately after BCLK (GP0 → GP1). The PIO state machine uses adjacent pin addressing and will read garbage data if this isn't satisfied.

**LED note**: On the Pico W and Pico 2 W, the onboard LED is connected to the CYW43 WiFi chip's GPIO_0, not to an RP2350 GPIO pin. It's driven via `control.gpio_set(0, true/false)` through the WiFi driver. PIN_25 is the CYW43 SPI clock — do NOT configure it as a GPIO output or WiFi will break.

---

## Project Structure

```
sound-sensor/
├── firmware-rs/              Rust + Embassy firmware (v0.3.0)
│   ├── Cargo.toml            embassy-rp (RP2350), embassy-net, cyw43, heapless
│   ├── memory.x              Flash: 4096K, RAM: 512K
│   ├── build.rs              Linker script setup + gzip compression of PWA assets
│   ├── cyw43-firmware/       WiFi chip firmware blobs (not in repo)
│   │   ├── 43439A0.bin
│   │   └── 43439A0_clm.bin
│   └── src/
│       ├── main.rs           Entry point, AP_MODE flag, task spawning, channels, dev_log! macro
│       ├── audio.rs          PIO I²S capture + HPF + Goertzel cry detection + RMS → dBFS
│       ├── ducking.rs        Ducking state machine + CryTracker temporal pattern detector
│       ├── net.rs            WiFi: AP mode fork or station join, flash creds, LED loop, mDNS
│       ├── ws.rs             WebSocket server (port 81), SHA-1, Base64, frame codec
│       ├── tv.rs             LG/Samsung/Sony/Roku TV control + SSDP discovery
│       ├── http.rs           HTTP server (port 80), AP mode → setup page, else → PWA (gzip)
│       ├── ap_services.rs    DHCP server (port 67) + DNS responder (port 53) for AP mode
│       ├── setup_html.rs     Self-contained HTML WiFi provisioning page
│       ├── dev_log.rs        [dev-mode only] LogLevel/LogCat/LogEntry, DEV_LOG_CH channel
│       ├── flash_fs.rs       Append-only flash file system for OTA assets
│       ├── ota.rs            OTA version comparison + status JSON builder
│       └── pwa_assets.rs     include_bytes! of gzip-compressed PWA from pwa-wasm/dist/
│
├── pwa-wasm/                 Leptos 0.7 + WASM PWA (v0.1.0)
│   ├── Cargo.toml            leptos 0.7 csr, gloo-net, serde, web-sys
│   ├── Trunk.toml            Build config: dist/ output, fixed filenames
│   ├── index.html            HTML shell (dark theme, viewport meta)
│   ├── sw.js                 Service worker for "Add to Home Screen"
│   ├── manifest.json         PWA manifest
│   ├── icon-192.png          App icon (192x192)
│   ├── icon-512.png          App icon (512x512)
│   └── src/
│       ├── main.rs           App root, 6 tabs (Dev tab conditional), setup wizard
│       ├── setup.rs          First-time setup wizard (Welcome → Cal → TV → Done)
│       ├── ws.rs             WebSocket client, auto-reconnect, event dispatch
│       ├── meter.rs          Live dB bar, peak hold, ducking banner, cry alert banner
│       ├── calibration.rs    Two-step calibration + manual threshold slider
│       ├── tv.rs             Brand buttons, SSDP discover list, IP input, connect
│       ├── wifi.rs           WiFi scan, signal bars, network list, credential form
│       ├── info.rs           Versions, OTA check, event log
│       └── dev.rs            [dev-mode] State dashboard, log stream, WS inspector
│
├── guardian-test/             Host-side unit tests for firmware pure logic
│   ├── Cargo.toml            heapless + libm (same deps as firmware)
│   ├── src/
│   │   ├── lib.rs            Module declarations
│   │   ├── ducking.rs        DuckingEngine (Instant replaced with injectable u64)
│   │   ├── parsers.rs        parse_f32_field, parse_str_field, parse_ip, etc.
│   │   ├── crypto.rs         CRC32, SHA-1, Base64, ws_accept_header
│   │   ├── audio.rs          compute_db, GoertzelBin, is_cry_like, CryTracker, ZCR
│   │   ├── ota.rs            is_newer, parse_tag_name, status_json
│   │   ├── flash_layout.rs   Config block serialize/deserialize on [u8; 256]
│   │   ├── ws_frame.rs       WebSocket frame encode/decode
│   │   └── tv_brand.rs       TvBrand enum with parse/to_u8/from_u8
│   └── tests/
│       ├── test_ducking.rs    35 tests — state machine, rates, restore paths
│       ├── test_parsers.rs    44 tests — JSON/string/IP/SSDP parsers
│       ├── test_crypto.rs     14 tests — SHA-1, Base64, CRC32, WS accept
│       ├── test_flash_layout.rs 18 tests — roundtrip, interleave, corruption
│       ├── test_ws_frame.rs   11 tests — encode, decode, roundtrip, unmask
│       ├── test_audio.rs      38 tests — dB, Goertzel, Hanning, ZCR, cry detection, integration
│       ├── test_tv_brand.rs   11 tests — brand parse, port, u8 roundtrip
│       └── test_ota.rs        15 tests — version comparison, tag parse, status JSON
│
├── phase0_micropython/       Hardware verification (MicroPython)
│   └── test_i2s.py
│
├── mock_ws.py                Mock firmware WS server for UI testing
└── tools/                    Build/flash helper scripts
```

---

## Firmware Architecture

The firmware is `#![no_std]` + `#![no_main]`, compiled for `thumbv8m.main-none-eabihf` (Cortex-M33). It uses the [Embassy](https://embassy.dev/) async framework for cooperative multitasking — no RTOS, no heap allocator.

### Task Communication

Tasks communicate through typed, fixed-capacity Embassy channels:

| Channel | Type | Capacity | From → To |
|---|---|---|---|
| `DB_CHANNEL` | `f32` | 4 | audio_task → ducking_task |
| `CRY_CHANNEL` | `bool` | 4 | audio_task → ducking_task |
| `CRY_EVENT_CH` | `()` | 1 | ducking_task → ws_task (one-shot) |
| `LED_CHANNEL` | `LedPattern` | 4 | any task → wifi_task |
| `WIFI_CMD_CH` | `WifiCmd` | 4 | ws_task / tv_task → wifi_task |
| `WIFI_EVT_CH` | `WifiEvent` | 2 | wifi_task → ws_task |
| `DUCK_CHANNEL` | `DuckCommand` | 8 | ducking_task → tv_task |
| `TELEM_SIGNAL` | `()` | 1 | ducking_task → ws_task |

### Audio Pipeline

The audio pipeline uses the RP2350's PIO (Programmable I/O) to implement an I²S master-mode receiver entirely in hardware:

1. PIO program (8 instructions) generates BCLK + WS clocks via side-set and reads DOUT
2. Each 32-bit word is right-shifted 8 bits to extract the signed 24-bit sample
3. A **2nd-order Butterworth high-pass filter** (200 Hz cutoff) removes the MEMS mic DC bias and low-frequency rumble per-sample
4. **Hanning window** (recursive cosine oscillator) applied to the filtered sample for Goertzel input
5. **10 Goertzel bins** fed with windowed samples (5 F0 bins at 350–550 Hz + 5 harmonic bins at 700–1100 Hz)
6. **Zero-crossing rate** counted on the unwindowed filtered signal
7. **Total energy** accumulated (sum of filtered²) for harmonic-to-total ratio
8. Every 1,600 samples (100ms at 16 kHz), RMS is computed:
   ```
   rms = sqrt(sum(filtered²) / count)
   dBFS = 20 * log10(rms / 8388608)    // 2^23 full-scale
   ```
9. An **exponential moving average** (attack=0.3, decay=0.08) smooths the dB output
10. **5-check cry detection pipeline** (volume-independent) evaluates the Goertzel powers, ZCR, and energy
11. dB pushed to `DB_CHANNEL`, cry result pushed to `CRY_CHANNEL`
12. If RMS < 1.0 (effectively silence), returns -96.0 dBFS

### WebSocket Server

The WebSocket implementation is fully custom — no external WS crate. It includes:
- **SHA-1** hash (inline, no_std compatible, RFC 3174 compliant)
- **Base64** encoder
- **RFC 6455 frame codec** with extended length (126 + 2-byte) support
- **Frame unmasking** for client→server messages
- **10 command handlers** dispatched via substring matching on JSON payloads

The server accepts one client at a time on port 81 with a 60-second idle timeout.

### WiFi Credential Fallback + AP Mode Provisioning

On boot, the firmware checks for WiFi credentials in order:

1. **Flash-stored credentials** — if present, tries 5 times with 3-second backoff
2. **Compile-time credentials** — from `GUARDIAN_SSID`/`GUARDIAN_PASS` env vars, 5 attempts
3. **Error retry** — if both fail, Error LED, 10-second wait, retry the cycle

If **neither flash nor compile-time credentials exist** (i.e., fresh device with default env vars), the firmware enters **AP mode**:

- Creates open WiFi network `Guardian-Setup` on channel 6
- Runs DHCP server (assigns 192.168.4.2) and DNS responder (all queries → 192.168.4.1)
- The DNS redirect triggers captive portal detection on phones — the setup page auto-opens
- HTTP server (port 80) serves a self-contained setup page for ALL GET requests
- Setup page connects via WebSocket (port 81) and reuses existing `scan_wifi`/`set_wifi` commands
- User scans networks, selects one, enters password → creds saved to flash → device reboots → joins home WiFi

This means developers who set `GUARDIAN_SSID` at build time bypass AP mode entirely. Only production devices (with default compile-time values) enter AP mode on first boot.

---

## PWA (Web UI) Architecture

The PWA is built with [Leptos 0.7](https://leptos.dev/) (client-side rendering) and compiled to WebAssembly. It runs entirely in the browser — no server-side rendering.

### Build Output

Trunk compiles the PWA to fixed filenames (via `data-no-hash` in Trunk.toml):
- `guardian-pwa.js` — WASM loader/glue
- `guardian-pwa_bg.wasm` — Compiled WASM binary
- `index.html` — App shell
- `sw.js` — Service worker (enables offline/Add to Home Screen)
- `manifest.json`, `icon-192.png`, `icon-512.png` — PWA metadata

The JS and WASM files are **gzip-compressed** at build time (`build.rs` runs `gzip -9` automatically) and served with `Content-Encoding: gzip`. This reduces over-the-wire transfer from ~580 KB to ~210 KB, significantly improving mobile load times. The remaining assets are embedded uncompressed via `include_bytes!` in `pwa_assets.rs`.

### First-Time Setup Wizard

On the first PWA load (when `guardian_setup_done` is not set in localStorage), a full-screen wizard overlay guides the user through setup:

1. **Welcome** — explains what Guardian does, "Get Started" button
2. **Calibrate** — instructions + "Open Calibration" button (switches to Calibrate tab)
3. **TV** — instructions + "Open TV Setup" button (switches to TV tab)
4. **Done** — "Finish Setup" marks setup complete in localStorage

The wizard step is tracked in the parent component, so when the user navigates to a tab (e.g., Calibration) and comes back via the "Continue Setup" banner, the wizard resumes at the next step. After completion, the wizard never shows again.

### Six Tabs

**Meter** — The main screen. Shows a real-time dB level bar (updated 10x/sec) with color coding: green (quiet), yellow (moderate), red (loud). A white vertical marker shows the tripwire position. Peak hold displays the highest dB in the last 2 seconds. When ducking is active, an amber banner reads "Volume Ducked". When baby crying is confirmed, a red pulsing banner reads "Baby Crying Detected" with "Rhythmic cry pattern confirmed." subtitle — it auto-dismisses when crying stops. The Arm/Disarm button controls whether Guardian is actively listening. A recent events list shows the last 5 events (ducking started, volume restored, baby crying detected/stopped, calibration changes, etc.).

**Calibrate** — Two-step calibration wizard. Step 1: make the room quiet, tap "Record Quiet Level" — this sets the noise floor. Step 2: turn your TV to the loudest volume you'd ever use, tap "Record TV Volume" — this sets the tripwire 3 dB below that level. A manual slider allows fine-tuning the tripwire after calibration. Validation ensures the tripwire is at least 6 dB above the floor.

**TV** — TV connection management. Four brand buttons (LG, Samsung, Sony, Roku) with a "Discover TVs on Network" button that sends an SSDP M-SEARCH and displays found TVs. Tap a discovered TV to auto-fill the IP and brand. Manual IP entry field. Sony shows an additional "Pre-Shared Key" field. Pairing instructions displayed per-brand. Connected state shows the TV info with a "Change TV" button that clears the configuration.

**WiFi** — WiFi network management. "Scan Networks" button triggers a WiFi scan on the Pico. Found networks are listed with signal strength bars (using Unicode block characters). Tap a network to auto-fill the SSID. Password field with "Connect" button. Changing WiFi triggers a firmware reboot. Dynamic status shows the current WebSocket connection state.

**Info** — System information. Shows firmware version, PWA version, WebSocket state, and message count. "Check for Updates" button queries GitHub Releases (with a 15-second timeout that auto-resets). Full scrollable event log showing all events with timestamps.

**Dev** *(conditional)* — Only visible when firmware is built with `--features dev-mode`. Shows a live state dashboard, firmware log stream with 8 category filters, raw WebSocket inspector, calibration debug values, and connection stats. Log forwarding can be paused/resumed at runtime via a toggle button.

### Reactive Architecture

The PWA uses Leptos signals (fine-grained reactivity) for all UI state. The WebSocket hook (`use_websocket`) drives all signals from incoming telemetry and event messages. A single `send` callback is distributed via `StoredValue` to all tabs for outgoing commands. JSON strings sent from the PWA escape quotes and backslashes to prevent injection.

---

## TV Protocol Support

### LG WebOS

- **Protocol**: WebSocket (SSAP) on port 3000
- **Handshake**: Client sends a registration message with app manifest. TV shows a pairing popup on first connection. User taps "OK" on the TV.
- **Volume**: Absolute — `getVolume` returns current level, `setVolume` sets exact level
- **Restore**: Exact — captures original volume, restores to that exact number

### Samsung Tizen

- **Protocol**: WebSocket (Smart Remote) on port 8001
- **Handshake**: Client connects to `/api/v2/channels/samsung.remote.control?name=<base64>`. TV shows pairing popup. On approval, TV sends an `ms.channel.connect` event with a token.
- **Token**: Stored in flash. Subsequent connections include the token in the URL to skip the pairing popup. If the token is rejected (expired), Guardian clears it and forces re-pairing.
- **Volume**: Relative only — sends `KEY_VOLDOWN` / `KEY_VOLUP` key presses
- **Restore**: Counted — replays the same number of VolumeUp presses

### Sony Bravia

- **Protocol**: HTTP JSON-RPC on port 80 (plain HTTP, not HTTPS)
- **Auth**: `X-Auth-PSK` header with a user-configured Pre-Shared Key (set in TV Settings → Network → IP Control)
- **Volume**: Absolute — `getVolumeInformation` and `setAudioVolume` (v1.2, volume as **string** not integer)
- **Restore**: Exact — same as LG

### Roku

- **Protocol**: ECP (External Control Protocol) HTTP on port 8060
- **Auth**: None required
- **Volume**: Relative only — `POST /keypress/VolumeDown` and `VolumeUp`
- **Restore**: Counted — same as Samsung
- **Response validation**: Guardian checks for HTTP 2xx status on Roku commands

### SSDP Discovery

Guardian sends an SSDP M-SEARCH multicast (`239.255.255.250:1900`) and listens for 3 seconds. Responses are classified by brand based on keywords in the response body (e.g., "webos" or "LG" → LG, "samsung" or "Samsung" → Samsung, etc.). Results are deduplicated by IP address. The friendly name is extracted from the SERVER header.

---

## Flash Storage Layout

The Pico 2 W has 4 MB of flash. Guardian uses it as follows:

| Address Range | Size | Purpose |
|---|---|---|
| `0x000000`–`0x0FFFFF` | 1 MB | Firmware binary |
| `0x100000`–`0x1FEFFF` | ~1 MB | Flash FS partition (OTA PWA assets, append-only) |
| `0x1FF000`–`0x1FFFFF` | 4 KB | Config block (256 bytes used) |
| `0x200000`–`0x3FFFFF` | 2 MB | Reserved (unused) |

### Config Block Layout (256 bytes at 0x1FF000)

```
Bytes 0–3:     Magic (0xBADC0FFE, little-endian)
Bytes 4–67:    WiFi SSID (null-terminated, max 63 chars)
Bytes 68–131:  WiFi password (null-terminated, max 63 chars)
Byte 132:      TV enabled flag (0=none, 1=configured)
Bytes 133–148: TV IP address (null-terminated, max 15 chars)
Byte 149:      TV brand (0=LG, 1=Samsung, 2=Sony, 3=Roku)
Bytes 150–165: Samsung token (null-terminated, max 15 chars)
Bytes 166–173: Sony PSK (null-terminated, max 8 chars)
Byte 174:      Calibration valid flag (0=no, 1=yes)
Bytes 175–178: Floor dB (f32, little-endian)
Bytes 179–182: Tripwire dB (f32, little-endian)
Bytes 183–188: TV MAC address (6 bytes, for Wake-on-LAN power on)
Bytes 189–251: Reserved (63 bytes, zero-filled)
Bytes 252–255: CRC32 over bytes 0–251 (little-endian)
```

**Read-modify-write**: `save_wifi_creds()` reads the existing block first to preserve TV config fields, and vice versa. If the block has an invalid magic or CRC, it starts fresh.

**Backward compatible**: Blocks written before TV support was added will have `tv_enabled=0`, so `load_tv_config()` returns None.

---

## WebSocket Protocol

### Telemetry (server → client, 10x/sec)

```json
{"db":-32.5,"armed":false,"tripwire":-20.0,"ducking":false,"crying":false,"tv_status":0,"fw":"0.3.0","pwa":"0.1.0"}
```

All 8 fields are always present. `ducking` is true when the state machine is in the Ducking state. `crying` is true when the CryTracker has confirmed a rhythmic cry pattern (3+ burst-gap cycles). `tv_status` is 0=off, 1=connecting, 2=connected, 3=error.

### Events (server → client, on demand)

```json
{"evt":"baby_cry"}
{"evt":"wifi_scan","networks":[{"ssid":"Home","rssi":-45},{"ssid":"Guest","rssi":-72}]}
{"evt":"discovered","tvs":[{"ip":"192.168.1.100","name":"LG WebOS TV","brand":"lg"}]}
{"evt":"ota_status","checking":false,"available":true,"current":"0.1.0","latest":"0.2.0","fw":"0.3.0"}
{"evt":"ota_done","pwa":"0.2.0","fw":"0.3.0"}
{"evt":"wifi_reconfiguring","ssid":"NewNetwork"}
```

The `baby_cry` event is a one-shot rising-edge notification sent when crying is first confirmed. The continuous `crying` state is tracked in telemetry.

### Commands (client → server)

| Command | JSON | Description |
|---|---|---|
| Arm | `{"cmd":"arm"}` | Start listening for noise |
| Disarm | `{"cmd":"disarm"}` | Stop listening, clear ducking state |
| Calibrate silence | `{"cmd":"calibrate_silence","db":-42.0}` | Set noise floor |
| Calibrate max | `{"cmd":"calibrate_max","db":-28.5}` | Set tripwire = db - 3 |
| Manual threshold | `{"threshold":-18.0}` | Override tripwire directly |
| Set TV | `{"cmd":"set_tv","ip":"192.168.1.5","brand":"lg","psk":"1234"}` | Configure TV (psk optional, for Sony) |
| Disconnect TV | `{"cmd":"set_tv","ip":"","brand":"lg"}` | Clear TV configuration |
| Set WiFi | `{"cmd":"set_wifi","ssid":"...","pass":"..."}` | Change WiFi (triggers reboot) |
| Scan WiFi | `{"cmd":"scan_wifi"}` | Trigger WiFi network scan |
| Discover TVs | `{"cmd":"discover_tvs"}` | Trigger SSDP M-SEARCH |
| Check OTA | `{"cmd":"ota_check"}` | Check GitHub for updates |

---

## LED Status Patterns

The onboard LED (CYW43 GPIO_0) is driven by the wifi_task in a 100ms loop:

| Pattern | Meaning |
|---|---|
| Fast blink (200ms on/off) | Connecting to WiFi / AP setup mode |
| Slow pulse (100ms on, 2s off) | Idle — connected, not armed |
| Double-flash every 2.9s | Armed — listening for noise |
| Solid on | Ducking — TV volume actively reduced |
| 3 rapid blinks, then off for 1s | Error — WiFi connection failed |

---

## Build Instructions

### Prerequisites

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

### Download CYW43 Firmware Blobs

```bash
mkdir -p firmware-rs/cyw43-firmware
# Download from: https://github.com/embassy-rs/embassy/tree/main/cyw43-firmware
# Place 43439A0.bin and 43439A0_clm.bin in firmware-rs/cyw43-firmware/
```

### Build Order (PWA must be built first)

```bash
# 1. Build the WASM PWA
cd pwa-wasm
trunk build --release
# Output: pwa-wasm/dist/ (guardian-pwa.js, guardian-pwa_bg.wasm, index.html, etc.)

# 2. Build the firmware (build.rs auto-gzips the JS and WASM)
cd ../firmware-rs
cargo build --release                     # production (no dev logs)
cargo build --release --features dev-mode # development (dev logs + Dev tab in PWA)

# 3. Generate UF2 and flash
./mk-uf2.sh              # production UF2 (no dev logs)
./mk-uf2.sh --dev        # dev UF2 (dev logs + Dev tab in PWA)
# Hold BOOTSEL on Pico, plug in USB, copy guardian.uf2 to the RPI-RP2 drive
```

The firmware uses `include_bytes!` to embed gzip-compressed files from `pwa-wasm/dist/` at compile time. If you skip step 1, the firmware build will fail. The `build.rs` script automatically compresses the JS and WASM files with `gzip -9` during `cargo build`.

### Environment Variables (all optional)

| Variable | Default | Purpose |
|---|---|---|
| `GUARDIAN_SSID` | `MyHomeNetwork` | WiFi SSID (compile-time fallback) |
| `GUARDIAN_PASS` | `password` | WiFi password (compile-time fallback) |
| `GUARDIAN_TV_IP` | (empty) | Default TV IP at boot |
| `GUARDIAN_GH_OWNER` | `gammahazard` | GitHub owner for OTA checks |
| `GUARDIAN_GH_REPO` | `sound-sensor` | GitHub repo for OTA checks |

Credentials saved through the WiFi tab are persisted to flash and take priority over compile-time values on subsequent boots.

### First Boot (Fresh Device — No WiFi Credentials)

1. Plug in Pico → LED blinks fast (AP setup mode)
2. On your phone, connect to the `Guardian-Setup` WiFi network
3. A setup page should auto-appear (captive portal). If not, open any URL in your browser.
4. Tap "Scan for Networks" → select your home WiFi → enter password → tap "Connect"
5. Pico saves credentials, reboots → LED blinks fast (connecting), then slow pulse (connected)
6. Reconnect your phone to your home WiFi
7. Open `http://guardian.local` (or the device IP) → PWA loads with a first-time setup wizard
8. The wizard guides you through calibration and TV setup
9. Tap Share → "Add to Home Screen" for a full-screen app experience

### First Boot (Developer — Compile-Time Credentials)

1. Build with `GUARDIAN_SSID="YourWiFi" GUARDIAN_PASS="YourPassword" cargo run --release`
2. Pico joins WiFi directly (skips AP mode) → LED blinks fast, then slow pulse
3. Open `http://guardian.local` → PWA loads with setup wizard

---

## Testing

Guardian has a comprehensive test suite covering all pure-logic code in both the firmware and the PWA.

### Test Architecture

The firmware is `#![no_std]` + `#![no_main]` targeting `thumbv8m.main-none-eabihf` — you can't run `cargo test` on it directly. The solution is a separate **guardian-test** crate that:

1. Copies all pure-logic functions from the firmware source files
2. Replaces hardware-dependent types (`embassy_time::Instant`) with injectable parameters (`u64` milliseconds)
3. Compiles and runs on the host (x86_64-unknown-linux-gnu)
4. Uses the same dependencies as the firmware (`heapless`, `libm`) to ensure identical behavior

PWA tests are `#[cfg(test)]` inline modules that compile natively (not to WASM).

### Running Tests

```bash
# Firmware logic tests (185 tests, runs on host)
cd guardian-test && cargo test

# PWA logic tests (40 tests, runs on host)
cd pwa-wasm && cargo test --lib --target x86_64-unknown-linux-gnu
```

### Test Coverage: guardian-test (185 tests)

#### Ducking State Machine — 35 tests (`test_ducking.rs`)

| Test | What It Verifies |
|---|---|
| `quiet_when_not_armed` | tick() returns None when not armed, regardless of dB level |
| `sustained_accumulation` | 30 ticks above tripwire → sustained_ms = 3000 |
| `sustained_decay` | Below tripwire → sustained_ms decays at 50ms/tick |
| `duck_trigger_at_3s` | First VolumeDown emitted exactly when sustained_ms reaches 3000 |
| `duck_rate_crisis` | excess > 15 dB → 500ms interval between VolumeDowns |
| `duck_rate_standard` | 5–15 dB excess → 1000ms interval |
| `duck_rate_gentle` | < 5 dB excess → 2000ms interval |
| `restore_path_a_near_silence` | dB < floor + 2 while Ducking → immediate Restore |
| `restore_path_b_hold_timer` | sustained_ms=0 + 30s elapsed → Restore |
| `no_restore_during_hold` | sustained_ms=0 but only 15s elapsed → stays Ducking |
| `disarm_clears_everything` | disarm() zeros all state |
| `set_floor_clamps_dead_mic` | set_floor(-85) → floor = -60 (clamp below -80) |
| `set_floor_bumps_tripwire` | set_floor(-30) with tripwire=-28 → tripwire bumped to -24 |
| `set_tripwire_enforces_gap` | set_tripwire(floor+3) → clamped to floor+6 |
| `intermittent_noise_no_reset` | Brief dip below tripwire doesn't reset sustained_ms to 0 |
| `oscillation_prevention` | After restore, re-duck requires full 3s accumulation again |
| `duck_steps_increment` | Each VolumeDown increments duck_steps_taken |
| `original_volume_captured_once` | set_original_volume only stores the first value |
| `watching_to_quiet_transition` | sustained_ms decays to 0 in Watching → transitions to Quiet |
| `ducking_stays_ducking_during_hold` | sustained_ms=0 in Ducking → stays Ducking (not Quiet) |

#### JSON/String Parsers — 44 tests (`test_parsers.rs`)

Tests all parser functions extracted from the firmware: `parse_f32_field` (positive, negative, missing, no digits, trailing chars, integer), `parse_str_field` (basic, empty, missing quote, embedded quote, second field), `parse_ip` (valid, invalid octet, too few parts, zeros, max, letters), `parse_volume_from_json` (basic, missing, zero, 100), `extract_ssdp_field` (case-insensitive, missing, different fields), `parse_json_str` (basic, missing).

#### Cryptography — 14 tests (`test_crypto.rs`)

| Test | What It Verifies |
|---|---|
| `sha1_empty` | SHA-1("") matches known hash (da39a3ee...) |
| `sha1_abc` | SHA-1("abc") matches RFC 3174 test vector |
| `sha1_long` | SHA-1 of 448-bit message matches RFC 3174 |
| `sha1_incremental` | Multiple update() calls produce same result as single call |
| `base64_empty/one/two/three/six` | Base64 encoding with 0/1/2/3/6 byte inputs (padding) |
| `ws_accept_known_key` | RFC 6455 example key produces known accept value |
| `crc32_empty` | CRC32([]) = 0x00000000 |
| `crc32_known_vector` | CRC32("123456789") = 0xCBF43926 |
| `crc32_single_bit_flip` | Flipping one bit changes CRC |
| `crc32_deterministic` | Same input always produces same CRC |

#### Flash Layout — 18 tests (`test_flash_layout.rs`)

Tests serialize/deserialize of the 256-byte config block: WiFi roundtrip, TV config roundtrip, interleaved saves (both orders), bad magic/CRC rejection, max-length SSID (63 chars), SSID truncation at 64 chars, Sony PSK max 8 chars, PSK truncation at 9 chars, empty IP disables TV, all 4 brands roundtrip, null byte in SSID truncates, empty SSID returns None.

#### WebSocket Framing — 11 tests (`test_ws_frame.rs`)

Tests frame encoding for short payloads (< 126 bytes, 2-byte header), extended payloads (>= 126 bytes, 4-byte header), boundary cases (exactly 125 and 126 bytes), encode→decode roundtrips, too-short frame rejection, payload unmasking, and equivalence of `ws_text_frame` and `ws_frame_masked`.

#### Audio + Cry Detection — 37 tests (`test_audio.rs`)

| Category | Count | What They Cover |
|---|---|---|
| compute_db | 9 | Silence, full-scale, known RMS, negative samples, single sample, empty, very quiet, sub-1 RMS, clamp above 0 |
| Goertzel | 4 | Detects 450 Hz, rejects wrong frequency, detects harmonic, reset clears state |
| Hanning window | 3 | Endpoints near zero, center is 1.0, reduces spectral leakage 10× |
| Multi-bin coverage | 2 | Catches 500 Hz cry (older baby), catches 350 Hz cry (newborn) |
| Zero-crossing rate | 3 | 450 Hz → ~90 crossings, 200 Hz → ~40, silence → 0 |
| is_cry_like v2 | 9 | True at 350/450/500 Hz, false for no harmonic, ZCR too low/high, flat spectrum, silence, low tonal ratio |
| CryTracker | 5 | 3 cycles confirms, 2 not enough, single burst rejected, cooldown clears, brief bursts rejected |
| Integration | 2 | Full pipeline (synthetic 450+900 Hz → 10 Goertzel bins → is_cry_like = true), broadband noise rejected |

#### TV Brand — 11 tests (`test_tv_brand.rs`)

Tests brand parsing (LG aliases, Samsung, Sony aliases, Roku, unknown), u8 roundtrip for all brands, default ports, absolute volume support flags.

#### OTA Version Comparison — 15 tests (`test_ota.rs`)

Tests `is_newer` (basic, with "v" prefix, equal, patch, major, older, both with "v", major-only), `parse_tag_name` (basic, missing, with spaces, empty), and `status_json` (checking state, available update, done event).

### Test Coverage: PWA Inline Tests (40 tests)

| Module | Tests | What They Cover |
|---|---|---|
| `meter.rs` | 10 | `db_to_pct` (min, max, mid, clamp below, clamp above), `bar_color` (green, yellow, red, boundaries) |
| `main.rs` | 8 | `json_escape` (clean string, quotes, backslashes, both, empty, newline, tab/cr, control char) |
| `ws.rs` | 7 | `rssi_bars` (excellent, good, fair, weak, boundary values at -50, -60, -70) |
| `tv.rs` | 15 | `is_valid_ip` (valid, zeros, max, too few/many parts, overflow, letters, empty, negative), `ip_prefix` (valid, empty, hostname, invalid, zeros) |

### Bug Found by Tests

The ducking test suite discovered a state machine bug: when `sustained_ms` drops below 3000 during the Ducking state, the `else if sustained_ms > 0` clause was overwriting the state to Watching, which broke Path B restore. The fix adds a guard: `&& self.state != DuckingState::Ducking`. This bug was fixed in both the test crate and the firmware.

---

## Testing the PWA Without Hardware

You can run the full UI locally before the Pico arrives.

### Step 1 — Run the mock firmware

```bash
pip install websockets
python3 mock_ws.py
# Listens on ws://localhost:81
```

The mock server simulates:
- Telemetry 10x/sec with a sine-wave dB meter
- Arm/disarm, calibration, threshold commands
- Mock WiFi scan results (4 networks)
- Mock SSDP TV discovery (3 TVs)
- OTA check status
- Ducking state when armed

### Step 2 — Run the PWA dev server

```bash
cd pwa-wasm
trunk serve
# Opens at http://localhost:8080
```

---

## Calibration Workflow

1. Place the microphone at your baby's door (ideally hallway-facing, not inside the nursery)
2. Open Guardian → **Calibrate** tab
3. **Step 1**: Make the room completely quiet. Tap "Record Quiet Level". This sets the noise floor.
4. **Step 2**: Turn your TV to the loudest volume you'd ever reasonably use. Tap "Record TV Volume Level". The tripwire is automatically set 3 dB below this level.
5. Go to **Meter** tab → tap **Arm Guardian**

**Why 3 dB below?** The TV at max volume represents the upper bound of normal listening. Setting the tripwire slightly below ensures that only sounds louder than your TV at max (e.g., a crying baby) trigger ducking.

**Manual adjustment**: The slider on the Calibrate tab lets you fine-tune the tripwire. Moving it lower makes Guardian more sensitive (triggers on quieter sounds). Moving it higher makes it less sensitive.

---

## Troubleshooting

| Symptom | Likely Cause | Fix |
|---|---|---|
| I²S buffer all zeros | LRCL not BCLK+1, or 5V on mic | Check wiring: 3.3V only, LRCL=GP1 |
| guardian.local not found | mDNS multicast dropped by WiFi power save | Use device IP directly (check router DHCP client list) |
| WebSocket won't connect | Phone on different subnet | Ensure phone and Pico on same WiFi |
| LED blinks fast indefinitely | AP setup mode (no WiFi creds) | Connect to "Guardian-Setup" WiFi and configure |
| Setup page doesn't auto-open | Captive portal not triggered | Open any URL in browser while on Guardian-Setup WiFi |
| LED blinks 3x then stops | WiFi credentials wrong | Check GUARDIAN_SSID/PASS or re-enter via WiFi tab |
| TV not pairing | IP wrong or TV asleep | Wake TV, verify IP in router DHCP table |
| Sony volume not changing | PSK mismatch | Re-enter PIN from TV Settings → IP Control |
| Samsung popup every reboot | Token not persisting | Check RTT logs for flash write errors |
| Samsung popup doesn't appear | Port 8001 blocked or newer model | 2021+ Samsung may need port 8002 (TLS, future) |
| trunk build fails | Leptos API mismatch | Run `rustup update` then rebuild |
| cargo build fails "no such file" | pwa-wasm/dist/ missing | Run `trunk build --release` first |
| Ducking fires too easily | Tripwire too low | Recalibrate or raise manual threshold |
| Ducking never fires | Tripwire too high or not armed | Check Meter → ensure Armed + reasonable tripwire |
| Volume doesn't restore | TV connection dropped during duck | Guardian clears duck state on failure; it'll reconnect |

### Manually Test TV APIs (before Pico arrives)

**Sony Bravia:**
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

**Roku:**
```bash
curl -X POST http://192.168.1.X:8060/keypress/VolumeDown
curl -X POST http://192.168.1.X:8060/keypress/VolumeUp
```

---

## Security

- **Zero cloud** — all communication is local LAN only
- **No telemetry** — the device does not contact any external servers (OTA check is opt-in and user-initiated)
- **No audio recording** — only RMS dB levels leave the microphone; raw audio is never stored, transmitted, or accessible
- **Flash credentials** — WiFi passwords and TV tokens stored with CRC32 integrity check; not encrypted (physical access to the Pico would expose them)
- **No open ports beyond LAN** — only port 80 (HTTP) and 81 (WebSocket), not exposed to the internet
- **Input sanitization** — firmware uses substring matching, not a full JSON parser; malformed input is silently dropped
- **JSON escaping** — PWA escapes quotes and backslashes in user input before embedding in JSON commands

---

## Current Status

### What Works
- **Onboarding**: AP mode WiFi provisioning (Guardian-Setup hotspot → captive portal → credential entry → reboot)
- **PWA**: Loads and renders correctly on desktop and mobile via direct IP access
- **Sound detection**: I²S mic captures audio with 2nd-order Butterworth HPF for proper dynamic range
- **Baby cry detection**: 10-bin Goertzel + Hanning window + ZCR + temporal pattern recognition, always active, volume-independent (decoupled from ducking tripwire)
- **Calibration**: Two-step calibration (silence + max) persists to flash
- **Dev mode**: `--features dev-mode` enables Dev tab with log stream, state dashboard, WS inspector
- **Gzip compression**: JS + WASM served compressed (~210 KB vs ~580 KB raw)

### Known Issues / In Progress
- **mDNS unreliable**: `guardian.local` resolves initially but stops working after WiFi power save kicks in. CYW43 power save drops incoming multicast. **Workaround**: use the device IP directly (find it from your router's DHCP client list)
- **Mobile load time**: Even with gzip, initial PWA load takes 3-5 seconds due to the Pico's serial HTTP server (one connection at a time). Subsequent loads are faster.
- **TV control untested on hardware**: Sony Bravia connection flow is implemented (SSDP discovery + PSK auth + volume control) but needs real-world testing. LG, Samsung, Roku protocols are implemented but also untested.
- **SSDP discovery**: Multicast group join/leave and CYW43 MAC filter registration added but not yet verified on hardware.

---

## Roadmap

### Phase 5 — WiFi Provisioning + Setup Wizard — Done

AP mode WiFi provisioning for first-time setup (no compile-time credentials needed):
- Fresh device creates `Guardian-Setup` open AP with DHCP + captive-portal DNS
- Self-contained HTML setup page served on port 80
- User scans networks, enters password → saved to flash → device reboots into station mode
- PWA includes a first-time setup wizard guiding calibration and TV connection

### Phase 6 — Dev/Debug Mode — Done

- Cargo feature flag `dev-mode`; `dev_log!` macro; structured WS log forwarding
- PWA Dev tab: state dashboard, log stream with category filters, raw WS inspector

### Next Up

**mDNS reliability** — WiFi power save (`PowerManagementMode::PowerSave`) causes the CYW43 radio to sleep and drop incoming multicast mDNS queries. Fix options: disable power save (more power draw but reliable multicast), or configure CYW43 to wake for DTIM intervals.

**TV control validation** — Test Sony Bravia end-to-end (discover → PSK → connect → duck → restore). Then LG, Samsung, Roku.

**Mobile load performance** — Explore options: service worker caching (requires HTTPS), further asset size reduction, or hosting PWA externally (adds cloud dependency).

### Future

**OTA Downloads (Phase 4B)** — Download PWA updates from GitHub Releases over HTTPS using embedded-tls + RP2350 TRNG. Flash FS and HTTP fallback already scaffolded.

**Samsung Port 8002 TLS** — Samsung 2021+ TVs require WebSocket over TLS on port 8002. Uses the same embedded-tls stack as OTA.

**Pico W Port** — Production build for the cheaper RP2040-based Pico W. Three changes: Cargo.toml feature, target triple, memory.x RAM size. Same CYW43439 WiFi chip and GPIO assignments.
