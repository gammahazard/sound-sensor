#!/usr/bin/env bash
# mk-uf2.sh — Build firmware and generate RP2350-compatible UF2
#
# Usage:
#   ./mk-uf2.sh              # release build (no dev logs)
#   ./mk-uf2.sh --dev        # release build with dev-mode feature
set -euo pipefail
cd "$(dirname "$0")"

FEATURES=""
if [[ "${1:-}" == "--dev" ]]; then
    FEATURES="--features dev-mode"
    echo "Building with dev-mode..."
fi

cargo build --release $FEATURES

ELF="target/thumbv8m.main-none-eabihf/release/guardian"
UF2="guardian.uf2"

# Generate UF2 with elf2uf2-rs, then patch family ID for RP2350
elf2uf2-rs "$ELF" "$UF2"

# elf2uf2-rs hardcodes RP2040 family (0xe48bff56).
# Pico 2 W (RP2350) needs 0xe48bff59 — patch every 512-byte block.
python3 -c "
import struct, sys
with open('$UF2', 'r+b') as f:
    while True:
        block = f.read(512)
        if len(block) < 512: break
        f.seek(-512 + 28, 1)
        f.write(struct.pack('<I', 0xe48bff59))
        f.seek(512 - 32, 1)
print('UF2 patched → RP2350 ARM-S')
"

SIZE=$(ls -lh "$UF2" | awk '{print $5}')
echo "Done: $UF2 ($SIZE)"
