// j49-dfu [dfu|console|boot] [--no-reboot]
//
// Put a one-shot flag word where U-Boot looks for it and reboot. U-Boot (tools/dfu-install/uboot,
// files/nest_j49.env) reads 0x83e00000 in bootcmd and clears it before acting:
//   0xdf00df00  stay in the DFU window instead of booting boot0  (dfu-util 0451:df10; stage 2/3 install,
//               kernel/rootfs update from the host)
//   0xac0ac0ac  USB CDC-ACM U-Boot console (/dev/ttyACM0, prompt "nest #") instead of booting
//   0           normal boot (clears a stale flag)
// DRAM survives the warm reset, so the flag does too. 0x83e00000 is outside the kernel's RAM
// (mem=62M window top) so /dev/mem read()/write() refuse it on ARM; it has to be mmap'ed.
use std::fs::OpenOptions;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;

const O_SYNC: i32 = 0o4010000; // Linux generic value (ARM uses it)

const FLAG_ADDR: u64 = 0x83e0_0000;

extern "C" {
    // With glibc on 32-bit ARM plain mmap takes a 32-bit offset. Declared with an i64 it read
    // garbage as the offset, so the Yocto build wrote the flag to some other physical page: the
    // board reset at once and U-Boot saw no flag (2026-11-26). glibc: use mmap64. musl: its mmap
    // always takes 64 bits, and it has no mmap64 symbol any more.
    #[cfg_attr(target_env = "gnu", link_name = "mmap64")]
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    fn sync();
    fn reboot(cmd: i32) -> i32;
    fn ioctl(fd: i32, req: u32, ...) -> i32;
    fn sleep(secs: u32) -> u32;
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.iter().find(|a| !a.starts_with("--")).map(|s| s.as_str()).unwrap_or("dfu");
    let no_reboot = args.iter().any(|a| a == "--no-reboot");
    let flag: u32 = match mode {
        "dfu" => 0xdf00_df00,
        "console" => 0xac0a_c0ac,
        "boot" => 0,
        _ => {
            eprintln!("usage: j49-dfu [dfu|console|boot] [--no-reboot]");
            std::process::exit(2);
        }
    };
    // O_SYNC matters: the flag page is valid RAM to the kernel (reserved ramoops region), so a
    // plain /dev/mem mapping is CACHED and the word can still sit in the CPU cache when a hard
    // watchdog reset hits (measured 2026-11-25: two resets, U-Boot never saw the flag). With
    // O_SYNC the kernel maps it write-combined, i.e. uncached.
    let f = OpenOptions::new().read(true).write(true).custom_flags(O_SYNC).open("/dev/mem").unwrap_or_else(|e| {
        eprintln!("j49-dfu: /dev/mem: {e}");
        std::process::exit(1);
    });
    // PROT_READ|PROT_WRITE = 3, MAP_SHARED = 1
    let p = unsafe { mmap(std::ptr::null_mut(), 4096, 3, 1, f.as_raw_fd(), FLAG_ADDR as i64) };
    if p as isize == -1 {
        eprintln!("j49-dfu: mmap {FLAG_ADDR:#x} failed: {}", std::io::Error::last_os_error());
        std::process::exit(1);
    }
    unsafe { std::ptr::write_volatile(p as *mut u32, flag) };
    let back = unsafe { std::ptr::read_volatile(p as *const u32) };
    if back != flag {
        eprintln!("j49-dfu: flag readback {back:#010x} != {flag:#010x}");
        std::process::exit(1);
    }
    eprintln!("j49-dfu: flag {flag:#010x} set at {FLAG_ADDR:#x} ({mode})");
    if no_reboot {
        return;
    }
    unsafe { sync() };
    // A plain reboot(2) sometimes leaves the OMAP3 dead until a hard reset (same finding as the
    // installer's wdtreset). A watchdog expiry is a reliable warm reset and keeps DRAM.
    // WDIOC_SETTIMEOUT = 0xC0045706; the fd stays open and unfed, so the timer runs out.
    if let Ok(w) = OpenOptions::new().write(true).open("/dev/watchdog") {
        let mut secs: i32 = 2;
        unsafe { ioctl(w.as_raw_fd(), 0xC004_5706, &mut secs as *mut i32) };
        eprintln!("j49-dfu: watchdog reset in ~{secs} s");
        unsafe { sleep(30) };
    }
    // fallback: RB_AUTOBOOT (warm reset, DRAM and flag survive)
    unsafe { reboot(0x0123_4567) };
}
