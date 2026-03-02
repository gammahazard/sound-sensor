"""
Guardian — Phase 0 Hardware Verification
=========================================
Run this on the Pico 2 W via Thonny or mpremote BEFORE writing any C++ firmware.

Wiring check:
  SPH0645 3V   → Pico Pin 36 (3.3V)
  SPH0645 GND  → Pico Any GND
  SPH0645 BCLK → Pico GP0  (Pin 1)
  SPH0645 LRCL → Pico GP1  (Pin 2)   ← must be BCLK+1
  SPH0645 DOUT → Pico GP2  (Pin 4)
  SPH0645 SEL  → GND (left channel)

Pass criteria: printing non-zero, changing values when you clap/speak near the mic.
"""

import time
from machine import I2S, Pin

# --- Config ---
SCK_PIN  = 0   # BCLK
WS_PIN   = 1   # LRCL  (must be SCK_PIN + 1)
SD_PIN   = 2   # DOUT (data from mic)
SAMPLE_RATE = 16_000
BUF_LEN     = 4096   # bytes  →  1024 samples × 4 bytes each (32-bit words)

def rms(buf):
    """Compute root-mean-square of a bytearray of 32-bit little-endian samples."""
    import struct
    n = len(buf) // 4
    total = 0
    for i in range(n):
        # SPH0645 packs 24-bit data left-aligned inside a 32-bit word
        raw = struct.unpack_from('<i', buf, i * 4)[0]
        sample = raw >> 8   # shift to 24-bit signed
        total += sample * sample
    return (total / n) ** 0.5 if n else 0

def db(rms_val, ref=8_388_608):   # ref = 2^23 (24-bit full-scale)
    import math
    if rms_val < 1:
        return -96.0
    return 20 * math.log10(rms_val / ref)


print("Guardian Phase 0 — I2S mic test")
print(f"  SCK=GP{SCK_PIN}  WS=GP{WS_PIN}  SD=GP{SD_PIN}")
print(f"  Sample rate: {SAMPLE_RATE} Hz,  Buffer: {BUF_LEN} bytes")
print()

audio_in = I2S(
    0,
    sck=Pin(SCK_PIN),
    ws=Pin(WS_PIN),
    sd=Pin(SD_PIN),
    mode=I2S.RX,
    bits=32,            # SPH0645 sends 32-bit words (24 data + 8 padding)
    format=I2S.MONO,
    rate=SAMPLE_RATE,
    ibuf=BUF_LEN,
)

buf = bytearray(BUF_LEN)

print("Reading... clap or talk near the mic. Press Ctrl-C to stop.")
print("-" * 55)
print(f"{'Sample[0:4]':>20}  {'RMS':>10}  {'dBFS':>8}")
print("-" * 55)

try:
    while True:
        num_bytes = audio_in.readinto(buf)
        r = rms(buf[:num_bytes])
        d = db(r)
        # Print first 4 raw bytes + computed RMS + dBFS
        print(f"  {list(buf[:4])}  {r:>10.1f}  {d:>8.2f} dBFS")
        time.sleep_ms(100)
except KeyboardInterrupt:
    pass
finally:
    audio_in.deinit()
    print("\nDone. If RMS was 0 throughout, check wiring (especially BCLK/LRCL pins).")
