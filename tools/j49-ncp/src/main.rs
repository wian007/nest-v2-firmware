// j49-ncp: talk to Nest's SPI bootloader ("nlbl-em35x") on the EM357 802.15.4 radio.
// READ-ONLY on purpose: version, CRC of a range, read flash. No erase, no write.
//
//   j49-ncp enter                     force the EM357 into its bootloader (CS low during reset)
//   j49-ncp version                   bootloader version string
//   j49-ncp crc  <addr> <len>         CRC the bootloader computes over a flash range
//   j49-ncp read <addr> <len> <file>  dump flash, 4 bytes per request
//   j49-ncp reset                     reset the EM357 into its application
//
// Protocol decoded from stock /nestlabs/bin/zb-loader (see docs/em357-radio.md). Request frame,
// little endian:  FF FE | u16 len (= total - 4) | u16 class | u16 cmd | u32 (unused) | payload |
//                 u32 0xCAFE1A7E | FF
//   version: class 0, cmd 0xFFFF, no payload            answer 33 bytes, string at offset 12
//   erase:   class 2, cmd 2, payload addr,size          (not implemented here)
//   write:   class 2, cmd 3, payload addr,size,data[64] (not implemented here)
//   read:    class 2, cmd 6, payload 0,addr,?           answer 21 bytes, word at offset 12
//   crc:     class 2, cmd 7, payload addr,size          answer 21 bytes, crc at offset 12
// SPI: /dev/spidev1.0, mode 0, 8 bits, 500 kHz. The answer is clocked out with 0xEE fill bytes.
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::time::Duration;

extern "C" {
    fn ioctl(fd: i32, req: u32, ...) -> i32;
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
}

const SPIDEV: &str = "/dev/spidev1.0";
const GPIO: &str = "/sys/class/gpio";
const GPIO_RESET: u32 = 512 + 61; // ZigBee Reset# (active low)
const GPIO_IRQ: u32 = 512 + 182; // nHOST_INT
const GPIO_CS: u32 = 512 + 181; // mcspi2_cs0 pad when muxed as GPIO
const PADCONF_BASE: u64 = 0x4800_2000;
const PAD_MCSPI2_CS0: usize = 0x1dc;
const MAGIC: u32 = 0xCAFE_1A7E;

#[repr(C)]
#[derive(Default)]
struct SpiXfer {
    tx_buf: u64,
    rx_buf: u64,
    len: u32,
    speed_hz: u32,
    delay_usecs: u16,
    bits_per_word: u8,
    cs_change: u8,
    tx_nbits: u8,
    rx_nbits: u8,
    word_delay_usecs: u8,
    pad: u8,
}
const SPI_IOC_MESSAGE_1: u32 = 0x4020_6b00;

fn ms(n: u64) {
    std::thread::sleep(Duration::from_millis(n));
}
fn wr(path: &str, v: &str) -> std::io::Result<()> {
    std::fs::write(path, v)
}
fn export(n: u32) {
    if !std::path::Path::new(&format!("{GPIO}/gpio{n}")).exists() {
        let _ = wr(&format!("{GPIO}/export"), &n.to_string());
    }
}
fn gpio_out(n: u32, v: u8) -> std::io::Result<()> {
    export(n);
    // "high"/"low" sets direction and level in one step (no glitch)
    wr(&format!("{GPIO}/gpio{n}/direction"), if v != 0 { "high" } else { "low" })
}
fn reset_pulse() -> std::io::Result<()> {
    export(GPIO_IRQ);
    let _ = wr(&format!("{GPIO}/gpio{GPIO_IRQ}/edge"), "falling");
    gpio_out(GPIO_RESET, 0)?;
    ms(1000);
    gpio_out(GPIO_RESET, 1)
}

/// Read or write the 16-bit pad configuration register of mcspi2_cs0.
fn pad(new: Option<u16>) -> std::io::Result<u16> {
    let f = OpenOptions::new().read(true).write(true).custom_flags(0o4010000).open("/dev/mem")?;
    let p = unsafe { mmap(std::ptr::null_mut(), 4096, 3, 1, f.as_raw_fd(), PADCONF_BASE as i64) };
    if p as isize == -1 {
        return Err(std::io::Error::last_os_error());
    }
    let reg = unsafe { p.add(PAD_MCSPI2_CS0) as *mut u16 };
    let old = unsafe { std::ptr::read_volatile(reg) };
    if let Some(v) = new {
        unsafe { std::ptr::write_volatile(reg, v) };
    }
    Ok(old)
}

/// Stock's force-bootload: hold the SPI chip select low (as GPIO 181) while the chip is reset.
fn enter() -> std::io::Result<()> {
    let old = pad(None)?;
    eprintln!("mcspi2_cs0 pad was {old:#06x}");
    gpio_out(GPIO_CS, 0)?;
    pad(Some(0x0004))?; // mode 4 = gpio_181, output
    reset_pulse()?;
    ms(1000);
    pad(Some(old))?;
    let _ = wr(&format!("{GPIO}/unexport"), &GPIO_CS.to_string());
    eprintln!("pad restored to {:#06x}", pad(None)?);
    Ok(())
}

fn xfer(spi: &File, tx: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut rx = vec![0xaau8; tx.len()];
    // stock sends an empty transfer with a 20 us delay first, then the data with cs_change = 1
    let pre = SpiXfer { delay_usecs: 20, ..Default::default() };
    unsafe { ioctl(spi.as_raw_fd(), SPI_IOC_MESSAGE_1, &pre as *const SpiXfer) };
    let x = SpiXfer {
        tx_buf: tx.as_ptr() as u64,
        rx_buf: rx.as_mut_ptr() as u64,
        len: tx.len() as u32,
        speed_hz: 500_000,
        bits_per_word: 8,
        cs_change: 1,
        ..Default::default()
    };
    let r = unsafe { ioctl(spi.as_raw_fd(), SPI_IOC_MESSAGE_1, &x as *const SpiXfer) };
    if r < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(rx)
}
fn open_spi() -> std::io::Result<File> {
    let f = OpenOptions::new().read(true).write(true).open(SPIDEV)?;
    let (mode, bits, speed) = (0u8, 8u8, 500_000u32);
    unsafe {
        ioctl(f.as_raw_fd(), 0x4001_6b01, &mode as *const u8); // SPI_IOC_WR_MODE
        ioctl(f.as_raw_fd(), 0x4001_6b03, &bits as *const u8); // SPI_IOC_WR_BITS_PER_WORD
        ioctl(f.as_raw_fd(), 0x4004_6b04, &speed as *const u32); // SPI_IOC_WR_MAX_SPEED_HZ
    }
    Ok(f)
}
fn frame(class: u16, cmd: u16, payload: &[u8]) -> Vec<u8> {
    let mut v = vec![0xff, 0xfe];
    v.extend_from_slice(&((12 + payload.len() + 5 - 4) as u16).to_le_bytes());
    v.extend_from_slice(&class.to_le_bytes());
    v.extend_from_slice(&cmd.to_le_bytes());
    v.extend_from_slice(&[0, 0, 0, 0]);
    v.extend_from_slice(payload);
    v.extend_from_slice(&MAGIC.to_le_bytes());
    v.push(0xff);
    v
}
fn irq_low(timeout_ms: u64) -> bool {
    let p = format!("{GPIO}/gpio{GPIO_IRQ}/value");
    for _ in 0..timeout_ms {
        if std::fs::read_to_string(&p).map(|s| s.starts_with('0')).unwrap_or(false) {
            return true;
        }
        ms(1);
    }
    false
}
fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" ")
}
fn num(s: &str) -> u32 {
    let s = s.trim_start_matches("0x");
    u32::from_str_radix(s, 16).expect("hex number")
}

fn main() -> std::io::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let verbose = a.iter().any(|x| x == "-v");
    let a: Vec<&str> = a.iter().filter(|x| *x != "-v").map(|s| s.as_str()).collect();
    match a.as_slice() {
        ["enter"] => enter(),
        ["reset"] => reset_pulse(),
        ["version"] => {
            let spi = open_spi()?;
            let _ = xfer(&spi, &frame(0, 0xffff, &[]))?;
            ms(2);
            let r = xfer(&spi, &[0xee; 33])?;
            println!("raw: {}", hex(&r));
            let s: String = r[12..].iter().take_while(|c| **c != 0 && **c != 0xff).map(|c| *c as char).collect();
            println!("bootloader version: {s}");
            Ok(())
        }
        ["crc", addr, len] => {
            let spi = open_spi()?;
            let mut p = num(addr).to_le_bytes().to_vec();
            p.extend_from_slice(&num(len).to_le_bytes());
            let _ = xfer(&spi, &frame(2, 7, &p))?;
            if !irq_low(5000) {
                eprintln!("no interrupt from the NCP");
            }
            let r = xfer(&spi, &[0xee; 21])?;
            println!("raw: {}", hex(&r));
            println!("crc: {:#010x}", u32::from_le_bytes([r[12], r[13], r[14], r[15]]));
            Ok(())
        }
        ["read", addr, len, file] => {
            let spi = open_spi()?;
            let (start, len) = (num(addr), num(len));
            let mut out = File::create(file)?;
            let mut off = 0u32;
            while off < len {
                let ad = start + off;
                let mut p = 0u32.to_le_bytes().to_vec();
                p.extend_from_slice(&ad.to_le_bytes());
                p.extend_from_slice(&4u32.to_le_bytes());
                let _ = xfer(&spi, &frame(2, 6, &p))?;
                ms(2);
                let r = xfer(&spi, &[0xee; 21])?;
                if verbose || off < 16 {
                    eprintln!("{ad:#010x}: {}", hex(&r));
                }
                out.write_all(&r[12..16])?;
                off += 4;
                if off % 0x1000 == 0 {
                    eprintln!("{:#010x} ({} %)", start + off, off as u64 * 100 / len as u64);
                }
            }
            Ok(())
        }
        _ => {
            eprintln!("usage: j49-ncp [-v] enter | version | crc <addr> <len> | read <addr> <len> <file> | reset   (hex numbers)");
            let _ = std::io::stdin().read(&mut [0u8; 0]);
            std::process::exit(2);
        }
    }
}
