// Dump a ramoops/pstore-RAM console zone from /dev/mem on the Nest (stock
// kernel), to recover a modern kernel's boot log after a kexec + watchdog
// warm-reset. ECC is disabled in the DT, so the console zone is plain text in a
// simple persistent_ram ring buffer. No crates: raw libc via extern "C" so it
// cross-builds offline (armv7-unknown-linux-musleabihf, static).
//
// Layout (from omap3-nest-j49.dts): region 0x83f00000 + 0x100000; record-size
// 0x20000 -> 6 oops dump zones (0..0xc0000); console zone 0x40000 @ 0xc0000.
// Each persistent_ram zone: u32 sig, u32 start, u32 size, then data[capacity].
//
// Usage: ramoops-read [base_hex] [region_hex] [console_off_hex] [console_sz_hex]

use std::env;

const PERSISTENT_RAM_SIG: u32 = 0x4347_4244; // "DBGC"
const O_RDONLY: i32 = 0;
const O_SYNC: i32 = 0o10000;
const PROT_READ: i32 = 1;
const MAP_SHARED: i32 = 1;

extern "C" {
    fn open(path: *const u8, flags: i32) -> i32;
    fn close(fd: i32) -> i32;
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
}

fn arg_hex(args: &[String], i: usize, default: u64) -> u64 {
    args.get(i)
        .map(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).unwrap_or(default))
        .unwrap_or(default)
}

fn rd_u32(p: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([p[off], p[off + 1], p[off + 2], p[off + 3]])
}

/// Reconstruct a persistent_ram ring buffer at `off` within the mapped region.
fn dump_zone(map: &[u8], off: usize, zone_sz: usize, label: &str) {
    if off + 12 > map.len() {
        return;
    }
    let sig = rd_u32(map, off);
    if sig != PERSISTENT_RAM_SIG {
        println!("[{label} @ +0x{off:x}] empty (sig=0x{sig:08x})");
        return;
    }
    let cap = zone_sz - 12;
    let start = (rd_u32(map, off + 4) as usize).min(cap);
    let size = (rd_u32(map, off + 8) as usize).min(cap);
    let data = &map[off + 12..off + 12 + cap];
    let text: Vec<u8> = if size < cap {
        data[..size].to_vec()
    } else {
        // wrapped: oldest bytes begin at `start`
        let mut v = data[start..].to_vec();
        v.extend_from_slice(&data[..start]);
        v
    };
    println!("===== {label} @ +0x{off:x}  ({} bytes) =====", text.len());
    print!("{}", String::from_utf8_lossy(&text));
    println!("\n===== end {label} =====");
}

fn main() {
    let args: Vec<String> = env::args().collect();
    // Defaults match omap3-nest-j49.dts: console-only ramoops at 0x83f00000,
    // 256 KB, offset 0 (the confirmed-safe low window).
    let base = arg_hex(&args, 1, 0x83f0_0000);
    let region = arg_hex(&args, 2, 0x0004_0000) as usize;
    let cons_off = arg_hex(&args, 3, 0x0000_0000) as usize;
    let cons_sz = arg_hex(&args, 4, 0x0004_0000) as usize;
    let rec_sz = 0x0002_0000usize;

    let path = b"/dev/mem\0";
    let fd = unsafe { open(path.as_ptr(), O_RDONLY | O_SYNC) };
    if fd < 0 {
        eprintln!("open /dev/mem failed (root? CONFIG_STRICT_DEVMEM off?)");
        std::process::exit(1);
    }
    let p = unsafe { mmap(std::ptr::null_mut(), region, PROT_READ, MAP_SHARED, fd, base as i64) };
    if p as isize == -1 {
        eprintln!("mmap /dev/mem @ 0x{base:x} failed");
        unsafe { close(fd) };
        std::process::exit(1);
    }
    let map = unsafe { std::slice::from_raw_parts(p, region) };

    // console zone = the main boot log
    dump_zone(map, cons_off, cons_sz, "console");
    // oops/dump zones may hold a panic
    let mut off = 0usize;
    let mut n = 0;
    while off + rec_sz <= cons_off {
        if rd_u32(map, off) == PERSISTENT_RAM_SIG {
            dump_zone(map, off, rec_sz, &format!("dump{n}"));
        }
        off += rec_sz;
        n += 1;
    }
    unsafe { close(fd) };
}
