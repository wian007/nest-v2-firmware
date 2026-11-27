// nest-mtdflash -- stream a (decompressed) jffs2 image into a NAND MTD
// character device while publishing exact progress for the installer screen.
//
// Same conventions as tools/kernel-firstlight/ramoops-read.rs: no crates and
// no libc dependency beyond the two raw calls Rust's std cannot make, so it
// cross-builds offline (armv7 static musl). std covers open/write/close;
// only ioctl (MEMERASE64) is declared externally -- as a variadic extern
// declaration, which is what modern rustc requires for runtime symbols.
//
// Usage:  nest-mtdflash <mtddev> <statefile> <total_bytes>              (stdin)
//         nest-mtdflash <mtddev> <statefile> --erase
//         nest-mtdflash <mtddev> <statefile> --from-mem <addr> <total>   (RAM)
//
// --from-mem reads the image straight out of DRAM at <addr> through
// /dev/mem (the stage-2 DFU window puts the raw jffs2 at 0x82000000; it is
// ~22 MB and does not fit inside a kernel initramfs on a 64 MB board). The
// kernel's strict /dev-mem rules are off on this config (CONFIG_STRICT_DEVMEM
// is not set -- ramoops-read relies on it), and RAM read() works on ARM
// where IO-space read() does not.
//
// The kernel's mtdchar write() runs through mtd_write -> nand_write, so ECC
// and bad-block handling stay kernel-side (mainline bch4 -- the same writer
// the kernel reads root0 back with). Progress goes to <statefile> in the
// KEY=VALUE format j49-screen's install screen polls:
//   PHASE=erase|write  PCT=0..100  TARGET=ROOT0  INFO=<bytes>  CODE=
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Seek, Write};
use std::os::unix::io::AsRawFd;

const MEMERASE64: u32 = 0x4010_4D14; // _IOW('M', 20, struct erase_info_user64)

#[repr(C)]
struct EraseInfo64 {
    start: u64,
    length: u64,
}

extern "C" {
    fn ioctl(fd: i32, req: u32, ...) -> i32;
}

fn put_state(path: &str, phase: &str, pct: u32, info: &str) {
    let _ = fs::write(
        path,
        format!("PHASE={phase}\nPCT={pct}\nTARGET=ROOT0\nINFO={info}\nCODE=\n"),
    );
}

fn sysfs(dev: &str, attr: &str) -> u64 {
    let name = dev.rsplit('/').next().unwrap_or(dev);
    fs::read_to_string(format!("/sys/class/mtd/{name}/{attr}"))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: nest-mtdflash <mtddev> <statefile> [--erase|<total_bytes>]");
        std::process::exit(2);
    }
    let dev = &args[1];
    let state = &args[2];

    if args.get(3).map(|s| s.as_str()) == Some("--erase") {
        let erasesize = sysfs(dev, "erasesize");
        let size = sysfs(dev, "size");
        if erasesize == 0 || size == 0 {
            eprintln!("nest-mtdflash: cannot read erasesize/size for {dev}");
            std::process::exit(1);
        }
        let blocks = size / erasesize;
        let f = OpenOptions::new().write(true).open(dev).unwrap_or_else(|e| {
            eprintln!("nest-mtdflash: open {dev}: {e}");
            std::process::exit(1);
        });
        let fd = f.as_raw_fd();
        for b in 0..blocks {
            let mut ei = EraseInfo64 {
                start: b * erasesize,
                length: erasesize,
            };
            let rc = unsafe { ioctl(fd, MEMERASE64, &mut ei as *mut EraseInfo64) };
            if rc < 0 {
                eprintln!(
                    "nest-mtdflash: MEMERASE64 block {b} failed: {} (start {:#x} len {:#x})",
                    io::Error::last_os_error(),
                    ei.start,
                    ei.length
                );
                std::process::exit(1);
            }
            put_state(
                state,
                "erase",
                ((b + 1) * 100 / blocks) as u32,
                &format!("{} / {} blokken", b + 1, blocks),
            );
        }
        return;
    }

    // --verify-mem <addr> <total>: read the MTD device back and compare it byte for
    // byte with the staging image in DRAM (cheap on RAM, unlike mounting a 46 MB jffs2
    // on a 64 MB board that still holds the 22 MB staging window).
    if args.get(3).map(|s| s.as_str()) == Some("--verify-mem") {
        let addr: u64 = args.get(4)
            .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
            .unwrap_or(0);
        let total: u64 = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(0);
        if addr == 0 || total == 0 {
            eprintln!("nest-mtdflash: --verify-mem needs <addr> <total_bytes>");
            std::process::exit(2);
        }
        let mut mem = std::fs::File::open("/dev/mem").unwrap_or_else(|e| {
            eprintln!("nest-mtdflash: /dev/mem: {e}");
            std::process::exit(1);
        });
        mem.seek(std::io::SeekFrom::Start(addr)).unwrap_or_else(|e| {
            eprintln!("nest-mtdflash: seek /dev/mem {addr:#x}: {e}");
            std::process::exit(1);
        });
        let mut nand = std::fs::File::open(dev).unwrap_or_else(|e| {
            eprintln!("nest-mtdflash: open {dev}: {e}");
            std::process::exit(1);
        });
        let mut a = vec![0u8; 128 * 1024];
        let mut b = vec![0u8; 128 * 1024];
        let mut done: u64 = 0;
        while done < total {
            let want = std::cmp::min(a.len() as u64, total - done) as usize;
            if let Err(e) = mem.read_exact(&mut a[..want]) {
                eprintln!("nest-mtdflash: verify: read /dev/mem at {:#x}: {e}", addr + done);
                std::process::exit(1);
            }
            if let Err(e) = nand.read_exact(&mut b[..want]) {
                eprintln!("nest-mtdflash: verify: read {dev} at {done}: {e}");
                std::process::exit(1);
            }
            if a[..want] != b[..want] {
                let off = a[..want].iter().zip(b[..want].iter()).position(|(x, y)| x != y).unwrap_or(0);
                eprintln!("nest-mtdflash: verify MISMATCH at byte {}", done + off as u64);
                std::process::exit(1);
            }
            done += want as u64;
            put_state(
                state,
                "verify",
                92 + ((done * 8) / total).min(8) as u32,
                &format!("{} / {} MiB", done >> 20, total >> 20),
            );
        }
        eprintln!("nest-mtdflash: verify OK ({done} bytes)");
        return;
    }

    if args.get(3).map(|s| s.as_str()) == Some("--from-mem") {
        let addr: u64 = args.get(4)
            .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
            .unwrap_or(0);
        let total: u64 = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(0);
        if addr == 0 || total == 0 {
            eprintln!("nest-mtdflash: --from-mem needs <addr> <total_bytes>");
            std::process::exit(2);
        }
        let mut mem = std::fs::File::open("/dev/mem").unwrap_or_else(|e| {
            eprintln!("nest-mtdflash: /dev/mem: {e}");
            std::process::exit(1);
        });
        if let Err(e) = mem.seek(std::io::SeekFrom::Start(addr)) {
            eprintln!("nest-mtdflash: seek /dev/mem {addr:#x}: {e}");
            std::process::exit(1);
        }
        // staging sanity: a JFFS2 image starts with node magic 0x1985 (bytes 85 19 on little-endian, 19 85 on big-endian).
        // Missing/poisoned staging (rootfs alt not downloaded) must fail
        // clearly, not flash RAM leftovers.
        let mut probe = [0u8; 2];
        if mem.read_exact(&mut probe).is_err() || (probe != [0x85, 0x19] && probe != [0x19, 0x85]) {
            eprintln!("nest-mtdflash: no jffs2 staging at {addr:#x} (magic {:#04x}{:02x})",
                      probe[0], probe[1]);
            std::process::exit(3);
        }
        if let Err(e) = mem.seek(std::io::SeekFrom::Start(addr)) {
            eprintln!("nest-mtdflash: reseek /dev/mem {addr:#x}: {e}");
            std::process::exit(1);
        }
        let mut out = OpenOptions::new().write(true).open(dev).unwrap_or_else(|e| {
            eprintln!("nest-mtdflash: open {dev}: {e}");
            std::process::exit(1);
        });
        let mtd_writesize = sysfs(dev, "writesize");
        let mut buf = vec![0u8; 256 * 1024];
        let mut written: u64 = 0;
        while written < total {
            let want = std::cmp::min(buf.len() as u64, total - written) as usize;
            let n = match mem.read(&mut buf[..want]) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) => {
                    eprintln!("nest-mtdflash: read /dev/mem at {:#x}: {e}", addr + written);
                    std::process::exit(1);
                }
            };
            // mtdchar write() needs whole NAND pages: pad the very last chunk with
            // 0xFF (erased state; JFFS2 ignores trailing 0xFF).
            let mut wlen = n;
            let ws = if mtd_writesize == 0 { 2048 } else { mtd_writesize } as usize;
            if written + n as u64 >= total && wlen % ws != 0 {
                let padded = (wlen + ws - 1) / ws * ws;
                for b in buf[wlen..padded].iter_mut() { *b = 0xFF; }
                wlen = padded;
            }
            if let Err(e) = out.write_all(&buf[..wlen]) {
                eprintln!("nest-mtdflash: write failed at {written}: {e}");
                std::process::exit(1);
            }
            written += n as u64;
            put_state(
                state,
                "write",
                ((written * 100) / total).min(100) as u32,
                &format!("{} / {} MiB", written >> 20, total >> 20),
            );
        }
        if written < total {
            eprintln!("nest-mtdflash: short write ({written} of {total})");
            std::process::exit(1);
        }
        return;
    }

    let total: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
    if total == 0 {
        eprintln!("nest-mtdflash: <total_bytes> missing or zero");
        std::process::exit(2);
    }
    let mut f = OpenOptions::new().write(true).open(dev).unwrap_or_else(|e| {
        eprintln!("nest-mtdflash: open {dev}: {e}");
        std::process::exit(1);
    });

    let mut buf = vec![0u8; 256 * 1024];
    let mut written: u64 = 0;
    let stdin = io::stdin();
    let mut lock = stdin.lock();
    loop {
        let n = match lock.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                eprintln!("nest-mtdflash: stdin: {e}");
                std::process::exit(1);
            }
        };
        if let Err(e) = f.write_all(&buf[..n]) {
            eprintln!("nest-mtdflash: write failed at {written}: {e}");
            std::process::exit(1);
        }
        written += n as u64;
        put_state(
            state,
            "write",
            ((written * 100) / total).min(100) as u32,
            &format!("{} / {} MiB", written >> 20, total >> 20),
        );
    }
    if written < total {
        eprintln!("nest-mtdflash: short write ({written} of {total})");
        std::process::exit(1);
    }
}
