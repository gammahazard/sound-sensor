//! flash_fs.rs — Minimal pure-Rust flash file store (no C dependencies)
//!
//! Layout (partition at 0x100000–0x1FF000, 1 MB - 4 KB):
//!
//!   Sector 0  (4 KB at 0x100000):  Directory
//!     16 × 64-byte entries:
//!       [u32 magic=0xF511F511][u8×48 name][u32 offset][u32 size][u8×4 reserved]
//!
//!   Sectors 1–254 (data):
//!     Files stored sequentially.  This is an append store — no in-place update.
//!     Call `reset_partition()` to start over (erases the full partition).

use defmt::*;
use embassy_rp::flash::{Blocking, Flash};
use embassy_rp::peripherals::FLASH;

// ── Partition layout ──────────────────────────────────────────────────────────

pub const FLASH_SIZE:      usize = 4 * 1024 * 1024;
pub const PART_START:      u32   = 0x100_000;
pub const PART_END:        u32   = 0x1FF_000;
const     SECTOR_SIZE:     u32   = 4096;
const     DIR_OFFSET:      u32   = PART_START;
const     DATA_START:      u32   = PART_START + SECTOR_SIZE;
const     PART_SECTORS:    u32   = (PART_END - PART_START) / SECTOR_SIZE;

// ── Directory ─────────────────────────────────────────────────────────────────

const DIR_MAGIC:    u32   = 0xF511_F511;
const MAX_ENTRIES:  usize = 16;
const ENTRY_SIZE:   usize = 64;

#[derive(Clone, Default)]
struct DirEntry {
    magic:  u32,
    name:   [u8; 48],
    offset: u32,
    size:   u32,
}

impl DirEntry {
    fn is_valid(&self)  -> bool { self.magic == DIR_MAGIC }

    fn filename(&self) -> &str {
        let len = self.name.iter().position(|&b| b == 0).unwrap_or(48);
        core::str::from_utf8(&self.name[..len]).unwrap_or("")
    }
}

fn entry_to_bytes(e: &DirEntry) -> [u8; ENTRY_SIZE] {
    let mut b = [0u8; ENTRY_SIZE];
    b[0..4].copy_from_slice(&e.magic.to_le_bytes());
    b[4..52].copy_from_slice(&e.name);
    b[52..56].copy_from_slice(&e.offset.to_le_bytes());
    b[56..60].copy_from_slice(&e.size.to_le_bytes());
    b
}

fn entry_from_bytes(b: &[u8; ENTRY_SIZE]) -> DirEntry {
    DirEntry {
        magic:  u32::from_le_bytes(b[0..4].try_into().unwrap()),
        name:   b[4..52].try_into().unwrap(),
        offset: u32::from_le_bytes(b[52..56].try_into().unwrap()),
        size:   u32::from_le_bytes(b[56..60].try_into().unwrap()),
    }
}

// ── FlashFs ───────────────────────────────────────────────────────────────────

pub struct FlashFs {
    flash:     Flash<'static, FLASH, Blocking, FLASH_SIZE>,
    dir:       [DirEntry; MAX_ENTRIES],
    n_files:   usize,
    write_ptr: u32,
}

impl FlashFs {
    pub fn new(flash: Flash<'static, FLASH, Blocking, FLASH_SIZE>) -> Self {
        Self {
            flash,
            dir: core::array::from_fn(|_| DirEntry::default()),
            n_files: 0,
            write_ptr: DATA_START,
        }
    }

    fn read_dir_raw(&mut self) -> [DirEntry; MAX_ENTRIES] {
        let mut raw = [0u8; MAX_ENTRIES * ENTRY_SIZE];
        let _ = self.flash.blocking_read(DIR_OFFSET, &mut raw);
        core::array::from_fn(|i| {
            let b: &[u8; ENTRY_SIZE] =
                raw[i * ENTRY_SIZE..(i + 1) * ENTRY_SIZE].try_into().unwrap();
            entry_from_bytes(b)
        })
    }

    pub fn find(&mut self, name: &str) -> Option<(u32, u32)> {
        let dir = self.read_dir_raw();
        for e in &dir {
            if e.is_valid() && e.filename() == name {
                return Some((e.offset, e.size));
            }
        }
        None
    }

    pub fn read_chunk(&mut self, offset: u32, buf: &mut [u8]) -> bool {
        self.flash.blocking_read(offset, buf).is_ok()
    }

    pub fn read_file(&mut self, name: &str, buf: &mut [u8]) -> usize {
        let Some((offset, size)) = self.find(name) else { return 0; };
        let n = (size as usize).min(buf.len());
        if self.flash.blocking_read(offset, &mut buf[..n]).is_err() { return 0; }
        n
    }

    pub fn file_size(&mut self, name: &str) -> Option<u32> {
        self.find(name).map(|(_, s)| s)
    }

    pub fn reset_partition(&mut self) -> bool {
        info!("[fs] Erasing partition ({} sectors)", PART_SECTORS);
        let mut off = PART_START;
        while off < PART_END {
            if self.flash.blocking_erase(off, off + SECTOR_SIZE).is_err() {
                warn!("[fs] Erase failed at {:08x}", off);
                return false;
            }
            off += SECTOR_SIZE;
        }
        self.n_files = 0;
        self.write_ptr = DATA_START;
        for e in &mut self.dir { *e = DirEntry::default(); }
        info!("[fs] Partition erased");
        true
    }

    pub fn alloc_file(&mut self, name: &str, size: u32) -> Option<u32> {
        if self.n_files >= MAX_ENTRIES { return None; }
        let aligned = align4(size as usize) as u32;
        if self.write_ptr + aligned > PART_END { return None; }

        let offset = self.write_ptr;
        self.write_ptr += aligned;

        let mut name_bytes = [0u8; 48];
        let nb = name.as_bytes();
        name_bytes[..nb.len().min(47)].copy_from_slice(&nb[..nb.len().min(47)]);

        self.dir[self.n_files] = DirEntry {
            magic: DIR_MAGIC,
            name:  name_bytes,
            offset,
            size,
        };
        self.n_files += 1;
        info!("[fs] alloc '{}': {} bytes at {:08x}", name, size, offset);
        Some(offset)
    }

    pub fn write_chunk(&mut self, offset: u32, data: &[u8]) -> bool {
        self.flash.blocking_write(offset, data).is_ok()
    }

    pub fn commit_dir(&mut self) -> bool {
        let mut raw = [0u8; MAX_ENTRIES * ENTRY_SIZE];
        for (i, e) in self.dir[..self.n_files].iter().enumerate() {
            let b = entry_to_bytes(e);
            raw[i * ENTRY_SIZE..(i + 1) * ENTRY_SIZE].copy_from_slice(&b);
        }
        if self.flash.blocking_write(DIR_OFFSET, &raw).is_ok() {
            info!("[fs] Directory committed ({} files)", self.n_files);
            true
        } else {
            warn!("[fs] Directory commit failed");
            false
        }
    }
}

fn align4(n: usize) -> usize { (n + 3) & !3 }

// ── OTA file offset table (wifi_task → http_task) ────────────────────────────

use embassy_sync::{
    blocking_mutex::raw::ThreadModeRawMutex,
    mutex::Mutex,
};

pub static OTA_FILE_OFFSETS: Mutex<ThreadModeRawMutex, OtaFileTable> =
    Mutex::new(OtaFileTable::new());

#[derive(Clone, Copy)]
pub struct OtaFileTable {
    pub index_html:    (u32, u32),
    pub guardian_js:   (u32, u32),
    pub guardian_wasm: (u32, u32),
    pub sw_js:         (u32, u32),
    pub manifest_json: (u32, u32),
    pub version_json:  (u32, u32),
}

impl OtaFileTable {
    pub const fn new() -> Self {
        Self {
            index_html:    (0, 0),
            guardian_js:   (0, 0),
            guardian_wasm: (0, 0),
            sw_js:         (0, 0),
            manifest_json: (0, 0),
            version_json:  (0, 0),
        }
    }

    pub fn has_ota_files(&self) -> bool {
        self.guardian_js.1 > 0 || self.guardian_wasm.1 > 0
    }
}
