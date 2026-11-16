//! PCI layer: raw configuration space + sysfs attributes.
//!
//! The configuration space header is produced by the device itself (straps /
//! fuses / VBIOS-programmed values), not by the GPU driver, so it is the first
//! independent source we compare everything else against.

use crate::util::*;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Default)]
pub struct Bar {
    pub index: usize,
    pub start: u64,
    pub size: u64,
    pub prefetch: bool,
    pub is64: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct PciDevice {
    pub addr: String,
    #[serde(skip)]
    pub sysfs: PathBuf,
    // from sysfs attribute files (kernel PCI core cache)
    pub vendor: u16,
    pub device: u16,
    pub subsys_vendor: u16,
    pub subsys_device: u16,
    pub revision: u8,
    /// False when the OS gave no revision (Windows adapter fallback).
    pub rev_known: bool,
    pub class: u32,
    // straight from config space bytes (independent read)
    pub cfg_vendor: Option<u16>,
    pub cfg_device: Option<u16>,
    pub cfg_subsys_vendor: Option<u16>,
    pub cfg_subsys_device: Option<u16>,
    pub cfg_revision: Option<u8>,
    pub cfg_len: usize,
    pub driver: Option<String>,
    pub link_speed: Option<String>,
    pub link_width: Option<u32>,
    pub max_link_speed: Option<String>,
    pub max_link_width: Option<u32>,
    pub bars: Vec<Bar>,
    /// Supported BAR0 sizes (bytes) from resource0_resize, if the device supports ReBAR.
    pub rebar_sizes: Vec<u64>,
    /// Extended capability 0x15 found in config space (needs root to read).
    pub rebar_cap: Option<bool>,
    pub boot_vga: bool,
    pub vendor_name: Option<String>,
    pub device_name: Option<String>,
    pub subsys_name: Option<String>,
    /// Windows: driver version / date / VRAM size from the display class registry key.
    pub os_driver_version: Option<String>,
    pub os_driver_date: Option<String>,
    pub os_vram: Option<u64>,
}

impl PciDevice {
    pub fn id_string(&self) -> String {
        if self.subsys_vendor == 0 && self.subsys_device == 0 {
            return format!("{:04X} {:04X}", self.vendor, self.device);
        }
        format!(
            "{:04X} {:04X} - {:04X} {:04X}",
            self.vendor, self.device, self.subsys_vendor, self.subsys_device
        )
    }

    pub fn bar0(&self) -> Option<&Bar> {
        self.bars.iter().find(|b| b.index == 0)
    }

    /// The largest prefetchable BAR (VRAM aperture on most GPUs).
    pub fn vram_bar(&self) -> Option<&Bar> {
        self.bars.iter().filter(|b| b.prefetch).max_by_key(|b| b.size)
    }

    pub fn link_gen(speed: &str) -> Option<u32> {
        let gt: f32 = speed.split_whitespace().next()?.parse().ok()?;
        Some(match gt as u32 {
            0..=2 => 1,
            3..=5 => 2,
            6..=8 => 3,
            9..=16 => 4,
            17..=32 => 5,
            _ => 6,
        })
    }
}

#[cfg(windows)]
pub fn enumerate() -> Vec<PciDevice> {
    crate::win::enumerate()
}

#[cfg(not(windows))]
pub fn enumerate() -> Vec<PciDevice> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir("/sys/bus/pci/devices") else {
        return out;
    };
    let ids = PciIds::load();
    let mut paths: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    paths.sort();
    for p in paths {
        let class = read_hex(p.join("class")).unwrap_or(0) as u32;
        if class >> 16 != 0x03 {
            continue;
        }
        out.push(read_device(&p, &ids));
    }
    out
}

fn read_device(p: &Path, ids: &PciIds) -> PciDevice {
    let mut d = PciDevice {
        addr: p.file_name().unwrap().to_string_lossy().to_string(),
        sysfs: p.to_path_buf(),
        vendor: read_hex(p.join("vendor")).unwrap_or(0) as u16,
        device: read_hex(p.join("device")).unwrap_or(0) as u16,
        subsys_vendor: read_hex(p.join("subsystem_vendor")).unwrap_or(0) as u16,
        subsys_device: read_hex(p.join("subsystem_device")).unwrap_or(0) as u16,
        revision: read_hex(p.join("revision")).unwrap_or(0) as u8,
        rev_known: p.join("revision").exists(),
        class: read_hex(p.join("class")).unwrap_or(0) as u32,
        ..Default::default()
    };
    if let Some(cfg) = read_bytes(p.join("config")) {
        d.cfg_len = cfg.len();
        d.cfg_vendor = le16(&cfg, 0);
        d.cfg_device = le16(&cfg, 2);
        d.cfg_revision = cfg.get(8).copied();
        d.cfg_subsys_vendor = le16(&cfg, 0x2c);
        d.cfg_subsys_device = le16(&cfg, 0x2e);
        if cfg.len() >= 4096 {
            d.rebar_cap = Some(find_ext_cap(&cfg, 0x15).is_some());
        }
    }
    d.driver = std::fs::read_link(p.join("driver"))
        .ok()
        .and_then(|l| l.file_name().map(|s| s.to_string_lossy().to_string()));
    d.link_speed = read_str(p.join("current_link_speed"));
    d.link_width = read_num(p.join("current_link_width")).map(|x| x as u32);
    d.max_link_speed = read_str(p.join("max_link_speed"));
    d.max_link_width = read_num(p.join("max_link_width")).map(|x| x as u32);
    d.boot_vga = read_num(p.join("boot_vga")) == Some(1);
    if let Some(res) = read_str(p.join("resource")) {
        for (i, line) in res.lines().enumerate().take(6) {
            let f: Vec<u64> = line.split_whitespace().filter_map(parse_num).collect();
            if f.len() == 3 && f[1] > f[0] {
                d.bars.push(Bar {
                    index: i,
                    start: f[0],
                    size: f[1] - f[0] + 1,
                    prefetch: f[2] & 0x2000 != 0,
                    is64: f[2] & 0x100000 != 0,
                });
            }
        }
    }
    if let Some(mask) = read_hex(p.join("resource0_resize")) {
        for bit in 0..48u32 {
            if mask & (1 << bit) != 0 {
                d.rebar_sizes.push((1u64 << 20) << bit);
            }
        }
    }
    d.vendor_name = ids.vendor(d.vendor);
    d.device_name = ids.device(d.vendor, d.device);
    d.subsys_name = ids.subsystem(d.vendor, d.device, d.subsys_vendor, d.subsys_device);
    d
}

fn find_ext_cap(cfg: &[u8], id: u16) -> Option<usize> {
    let mut off = 0x100usize;
    for _ in 0..64 {
        let hdr = le32(cfg, off)?;
        if hdr == 0 || hdr == 0xffff_ffff {
            return None;
        }
        if (hdr & 0xffff) as u16 == id {
            return Some(off);
        }
        off = ((hdr >> 20) & 0xffc) as usize;
        if off < 0x100 {
            return None;
        }
    }
    None
}

/// Minimal pci.ids parser (only the parts we need).
pub struct PciIds {
    text: String,
}

impl PciIds {
    pub fn load() -> Self {
        for p in ["/usr/share/misc/pci.ids", "/usr/share/hwdata/pci.ids", "/usr/share/pci.ids"] {
            if let Ok(t) = std::fs::read_to_string(p) {
                return Self { text: t };
            }
        }
        Self { text: String::new() }
    }

    fn vendor_block(&self, v: u16) -> Option<&str> {
        let key = format!("\n{:04x}  ", v);
        let start = self.text.find(&key)? + 1;
        let rest = &self.text[start..];
        let mut end = 0;
        for (i, line) in rest.split_inclusive('\n').enumerate() {
            if i > 0 && line.chars().next().is_some_and(|c| c.is_ascii_hexdigit()) {
                break;
            }
            end += line.len();
        }
        Some(&rest[..end])
    }

    pub fn vendor(&self, v: u16) -> Option<String> {
        let b = self.vendor_block(v)?;
        Some(b.lines().next()?[6..].trim().to_string())
    }

    pub fn device(&self, v: u16, d: u16) -> Option<String> {
        let b = self.vendor_block(v)?;
        let key = format!("\t{:04x}  ", d);
        b.lines()
            .find(|l| l.starts_with(&key))
            .map(|l| l[key.len()..].trim().to_string())
    }

    pub fn subsystem(&self, v: u16, d: u16, sv: u16, sd: u16) -> Option<String> {
        let b = self.vendor_block(v)?;
        let key = format!("\t{:04x}  ", d);
        let mut in_dev = false;
        let skey = format!("\t\t{:04x} {:04x}  ", sv, sd);
        for l in b.lines() {
            if l.starts_with(&key) {
                in_dev = true;
                continue;
            }
            if in_dev {
                if !l.starts_with("\t\t") {
                    break;
                }
                if l.starts_with(&skey) {
                    return Some(l[skey.len()..].trim().to_string());
                }
            }
        }
        None
    }
}

/// Well-known board partners (subsystem vendor IDs).
pub fn board_vendor(id: u16) -> Option<&'static str> {
    Some(match id {
        0x1002 => "AMD",
        0x10DE => "NVIDIA",
        0x8086 => "Intel",
        0x1043 => "ASUS",
        0x1462 => "MSI",
        0x1458 => "Gigabyte",
        0x1849 => "ASRock",
        0x1DA2 => "Sapphire",
        0x174B => "Sapphire / PC Partner",
        0x148C => "PowerColor (TUL)",
        0x1682 => "XFX",
        0x3842 => "EVGA",
        0x19DA => "Zotac",
        0x1569 => "Palit",
        0x10B0 => "Gainward",
        0x196E => "PNY",
        0x1B4C => "KFA2 / Galax",
        0x7377 => "Colorful",
        0x1ACC => "Point of View",
        0x1028 => "Dell",
        0x103C => "HP",
        0x17AA => "Lenovo",
        0x1025 => "Acer",
        0x144D => "Samsung",
        0x1D05 => "Tongfang",
        0x1558 => "Clevo",
        _ => return None,
    })
}
