//! Small helpers for reading sysfs / procfs and running helper commands.

use std::fs;
use std::path::Path;
use std::process::Command;

pub fn read_str<P: AsRef<Path>>(p: P) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

pub fn read_bytes<P: AsRef<Path>>(p: P) -> Option<Vec<u8>> {
    fs::read(p).ok()
}

/// Parses "0x1002", "1002", "123".
pub fn parse_num(s: &str) -> Option<u64> {
    let s = s.trim();
    if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(h, 16).ok()
    } else {
        s.parse().ok()
    }
}

pub fn read_num<P: AsRef<Path>>(p: P) -> Option<u64> {
    read_str(p).and_then(|s| parse_num(&s))
}

pub fn read_hex<P: AsRef<Path>>(p: P) -> Option<u64> {
    read_str(p).and_then(|s| {
        let s = s.trim_start_matches("0x");
        u64::from_str_radix(s, 16).ok()
    })
}

/// Graphics APIs used for adapter matching and the GPU tests.
pub const BACKENDS: wgpu::Backends = if cfg!(windows) { wgpu::Backends::VULKAN.union(wgpu::Backends::DX12) } else { wgpu::Backends::VULKAN };

pub fn command(cmd: &str) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(cmd);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

pub fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = command(cmd).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(unix)]
pub fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}
#[cfg(not(unix))]
pub fn is_root() -> bool {
    false
}

pub fn le16(b: &[u8], o: usize) -> Option<u16> {
    b.get(o..o + 2).map(|x| u16::from_le_bytes([x[0], x[1]]))
}

pub fn le32(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4).map(|x| u32::from_le_bytes([x[0], x[1], x[2], x[3]]))
}

pub fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).trim().to_string()
}

pub fn fmt_bytes(b: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let mb = b as f64 / MB;
    if mb >= 1024.0 * 10.0 {
        format!("{:.0} MB ({:.1} GB)", mb, mb / 1024.0)
    } else {
        format!("{:.0} MB", mb)
    }
}

pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let h = Sha256::digest(data);
    h.iter().map(|b| format!("{b:02x}")).collect()
}
