/* memory.x — RP2350 (Pico 2 W) memory map
 *
 * RP2350 has:
 *   4 MB external Flash  (XIP at 0x10000000)
 *   512 KB SRAM          (0x20000000)
 *
 * The second_stage bootloader occupies the first 256 bytes of flash.
 */

MEMORY {
    BOOT2  : ORIGIN = 0x10000000, LENGTH = 0x100       /* 256-byte 2nd-stage bootloader */
    FLASH  : ORIGIN = 0x10000100, LENGTH = 4096K - 256 /* application flash            */
    RAM    : ORIGIN = 0x20000000, LENGTH = 512K         /* SRAM (all 512 KB)            */
}

EXTERN(BOOT2_FIRMWARE)
SECTIONS {
    .boot2 ORIGIN(BOOT2) : {
        KEEP(*(.boot2));
    } > BOOT2
}
