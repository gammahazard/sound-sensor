# Guardian Sound Sensor — Getting Started

Complete guide from unboxing to a working sound-activated TV volume ducker.

---

## What You Need

**Hardware:**
- Raspberry Pi Pico 2 W
- SPH0645LM4H I2S microphone breakout (Adafruit)
- Micro-USB cable
- 6 jumper wires (female-to-female or as needed for your breakout)

**Software (on your computer):**
- Rust (nightly toolchain)
- `trunk` (WASM build tool)
- `wasm32-unknown-unknown` target
- `elf2uf2-rs` (for drag-and-drop flashing)
- OR `probe-rs-tools` (if using a debug probe)

---

## Part 1: Install the Toolchain

If you don't already have Rust installed:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Then install everything else:

```bash
# Nightly toolchain + embedded target (firmware)
rustup toolchain install nightly
rustup target add thumbv8m.main-none-eabihf --toolchain nightly

# WASM target (PWA)
rustup target add wasm32-unknown-unknown

# Trunk (builds the PWA)
cargo install trunk

# flip-link (stack overflow protection for embedded)
cargo install flip-link

# UF2 flasher (drag-and-drop, no debug probe needed)
cargo install elf2uf2-rs
```

---

## Part 2: Wire the Microphone

You need 6 jumper wires to connect the SPH0645LM4H breakout to the Pico 2 W.

### Pico 2 W Pinout

Hold the Pico with the **USB port pointing up** and the board facing you. Pin 1 is the
top-left pin. Pins go down the left side (1–20) and up the right side (21–40):

```
                 ┌──── USB ────┐
  ←── GP0   1  ─┤              ├─  40  VBUS (5V — DO NOT USE)
  ←── GP1   2  ─┤              ├─  39  VSYS
  ←── GND   3  ─┤              ├─  38  GND
  ←── GP2   4  ─┤              ├─  37  3V3_EN
       GP3  5  ─┤              ├─  36  3V3 (OUT) ←──
       GP4  6  ─┤              ├─  35  ADC_VREF
       GP5  7  ─┤              ├─  34  GP28
       GND  8  ─┤              ├─  33  GND
       GP6  9  ─┤              ├─  32  GP27
       GP7 10  ─┤              ├─  31  GP26
       GP8 11  ─┤              ├─  30  RUN
       GP9 12  ─┤              ├─  29  GP22
       GND 13  ─┤              ├─  28  GND
      GP10 14  ─┤              ├─  27  GP21
      GP11 15  ─┤              ├─  26  GP20
      GP12 16  ─┤              ├─  25  GP19
      GP13 17  ─┤              ├─  24  GP18
       GND 18  ─┤              ├─  23  GND
      GP14 19  ─┤              ├─  22  GP17
      GP15 20  ─┤              ├─  21  GP16
                └──────────────┘
```

The arrows (←──) mark the 5 pins you need. All wiring is on the left side except 3V3 power.

### SPH0645 Breakout Pins

The Adafruit SPH0645 breakout has 6 pins. Check the **labels printed on your board** — they
will say things like `BCLK`, `DOUT`, `LRCLK`, `SEL`, `GND`, `3V`. The order varies by
board revision, so always go by the label, not the position.

### Wire-by-Wire Connections

Connect each labelled pin on the mic breakout to the corresponding Pico pin:

```
Wire   Mic label        Pico pin          What it does
────   ─────────        ────────          ────────────────────────────────
 1     BCLK             GP0  (pin 1)      I2S bit clock
 2     LRCLK            GP1  (pin 2)      I2S word select (left/right)
 3     GND (on mic)     GND  (pin 3)      Ground
 4     DOUT             GP2  (pin 4)      I2S audio data out
 5     SEL              GND  (pin 8)      Channel select → left (tie to ground)
 6     3V               3V3  (pin 36)     Power (right side of Pico, 5th from top)
```

The Pico has 8 GND pins (3, 8, 13, 18, 23, 28, 33, 38) — they're all connected internally,
so use any two for the mic's GND and SEL wires. Pins 3 and 8 are closest to the other wires.

### Important

- The mic is **3.3V only** — never connect it to VBUS (pin 40, 5V) or it will be damaged.
- SEL to GND selects the left audio channel (required by the firmware).
- LRCLK **must** be the GPIO directly after BCLK (GP0 → GP1). The PIO program depends on this.
- Keep wires as short as possible (under 10cm) to reduce I2S noise.
- Double-check each wire against the labels before powering on.

---

## Part 3: Build the Software

Everything is built from the project root directory.

### Step 1: Build the PWA

The PWA must be built first — the firmware embeds these files directly into the binary.

```bash
cd pwa-wasm
trunk build --release
```

This creates `pwa-wasm/dist/` containing the web app files. You should see:
```
dist/
  guardian-pwa.js
  guardian-pwa_bg.wasm
  index.html
  sw.js
  manifest.json
  icon-192.png
  icon-512.png
```

### Step 2: Build the firmware

```bash
cd ../firmware-rs

# Standard build:
cargo build --release

# OR with debug logging (adds a "Dev" tab to the app — recommended for first run):
cargo build --release --features dev-mode
```

The binary is at: `target/thumbv8m.main-none-eabihf/release/guardian-firmware`

---

## Part 4: Flash the Pico

### Option A: UF2 Drag-and-Drop (no debug probe)

1. **Hold the BOOTSEL button** on the Pico 2 W (the small white button).
2. While holding it, **plug the Pico into your computer via USB**.
3. Release BOOTSEL. A drive called **RPI-RP2** appears.
4. Convert and copy:

```bash
cd firmware-rs
elf2uf2-rs target/thumbv8m.main-none-eabihf/release/guardian-firmware
```

5. Drag the generated `.uf2` file onto the RPI-RP2 drive.
6. The Pico reboots automatically and starts running.

### Option B: probe-rs (if you have a debug probe)

```bash
cd firmware-rs
cargo run --release
# or with dev mode:
cargo run --release --features dev-mode
```

This flashes and attaches the defmt log viewer.

---

## Part 5: First-Time Setup (AP Mode)

On first boot with no saved WiFi credentials, the Pico enters **AP mode**.

1. The LED blinks rapidly (200ms on/off) — this means AP mode is active.
2. On your phone, go to **Settings → WiFi**.
3. Connect to the network called **Guardian-Setup**.
4. A setup page should appear automatically (captive portal).
   - If it doesn't, open a browser and go to `http://192.168.4.1`.
5. Enter your **home WiFi name (SSID)** and **password**.
6. Tap **Connect**.
7. The Pico saves the credentials to flash and reboots.
8. It joins your home WiFi — the LED changes from rapid blink to a **slow pulse** (100ms on, 2s off).

Your WiFi credentials are saved permanently. The Pico reconnects automatically on every boot.

---

## Part 6: Open the App

1. Make sure your phone is on the **same WiFi network** as the Pico.
2. Find the Pico's IP address:
   - Check your router's admin page for connected devices, or
   - If you flashed with probe-rs, the IP is printed in the defmt log after `[net] DHCP`.
3. Open a browser on your phone and go to `http://<pico-ip>` (e.g., `http://192.168.1.42`).
4. The Guardian app loads. You can **Add to Home Screen** for an app-like experience.

### First-Time App Setup Wizard

The app detects it's the first time and walks you through setup:

1. **Welcome** — Tap "Get Started".
2. **Calibrate Silence** — Make the room quiet, then tap "Calibrate". This records the ambient noise floor.
3. **Calibrate Max** — Make noise at the level you want to trigger ducking (e.g., a baby crying), then tap "Calibrate". This sets the tripwire threshold.
4. **TV Setup** — Tap "Discover TVs" to find TVs on your network. Select your TV and brand. For Sony, enter the PSK (pre-shared key) from your TV's settings.
5. **Done** — Setup complete. The app is ready to use.

---

## Part 7: Using Guardian

### Meter Tab
- Shows the live sound level (dB) as a bar.
- **Arm** button: starts monitoring. When sound exceeds the tripwire for ~3 seconds, the TV volume ducks automatically.
- **Disarm**: stops monitoring and restores the TV volume if it was ducked.

### Calibration Tab
- Re-calibrate silence and max levels any time.
- Manual slider for fine-tuning the tripwire threshold.

### TV Tab
- Change TV, update IP address, or re-pair.

### WiFi Tab
- Scan for networks and switch WiFi if needed.

### Info Tab
- Shows firmware and PWA versions.
- OTA update check (if available).

### Dev Tab (only with `--features dev-mode`)
- Live firmware logs streamed over WebSocket.
- Category filters (audio, ducking, tv, wifi, etc.).
- Raw WebSocket message inspector.
- Useful for debugging if something isn't working right.

---

## LED Quick Reference

| LED Behavior | Meaning |
|---|---|
| Fast blink (200ms on/off) | Connecting to WiFi or AP mode active |
| Slow pulse (100ms on, 2s off) | Connected to WiFi, idle |
| Double flash every 3s | Armed and monitoring |
| Solid on | Ducking active (TV volume being lowered) |
| 3 rapid blinks then off | Error (WiFi connection failed) |

---

## Troubleshooting

**LED just blinks fast and never changes:**
- The Pico can't connect to WiFi. Check that the SSID and password are correct.
- Try moving the Pico closer to your router for the first connection.
- If credentials are wrong: hold BOOTSEL, re-flash, and re-do AP setup.

**dB meter stuck at -80 or doesn't move:**
- Check the mic wiring — BCLK, LRCL, and DOUT must be on the correct pins.
- Make sure SEL is connected to GND (not floating).
- Verify the mic is powered from 3V3 (not 5V VBUS).

**App doesn't load in browser:**
- Confirm your phone is on the same WiFi network as the Pico.
- Try the IP address directly instead of a hostname.
- Check that the LED shows a slow pulse (WiFi connected) not fast blink (still connecting).

**TV doesn't duck:**
- Make sure the TV is on and reachable (try the TV tab → test).
- For Sony: verify the PSK matches your TV's Network → Home Network → IP Control → Pre-Shared Key setting.
- For LG: a pairing popup should appear on the TV the first time — accept it.
- For Samsung: a pairing popup should appear — accept it. The token is saved automatically.
- Check that the tripwire is calibrated correctly. If it's too high, it never triggers.

**Pico crashes or behaves erratically:**
- Re-flash with `--features dev-mode` and check the Dev tab for error logs.
- If using probe-rs, check the defmt output for panic messages.

---

## Re-flashing

To update the firmware or PWA:

```bash
cd pwa-wasm && trunk build --release
cd ../firmware-rs && cargo build --release
```

Then flash again using the same method from Part 4. WiFi credentials and TV config are stored in a separate flash region and survive re-flashing.

---

## Build Order Summary

Always build in this order:

```
1. pwa-wasm    →  trunk build --release
2. firmware-rs →  cargo build --release [--features dev-mode]
3. Flash       →  elf2uf2-rs or probe-rs
```

The firmware won't compile without the PWA dist files present.
