//! Video BIOS (option ROM) parser.
//!
//! Extracts what is needed to detect modified / cross-flashed BIOSes:
//!  * PCI Data Structure ("PCIR") of every image: vendor/device ID baked into
//!    the BIOS. If it differs from the device's real ID the card was
//!    cross-flashed (classic "fake" card trick).
//!  * Legacy image checksum (sum of bytes == 0 mod 256).
//!  * UEFI GOP image presence.
//!  * AMD ATOM: subsystem IDs in the ROM header, part number, PowerPlay table
//!    (compared with the table the driver actually uses -> soft PP tables).
//!  * NVIDIA: version from the BIT 'B' token.

use crate::util::*;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct RomImage {
    pub offset: usize,
    pub length: usize,
    pub vendor: u16,
    pub device: u16,
    pub code_type: u8,
    pub last: bool,
    pub checksum_ok: Option<bool>,
}

impl RomImage {
    pub fn code_type_name(&self) -> &'static str {
        match self.code_type {
            0 => "x86 PC-AT (legacy)",
            1 => "Open Firmware",
            2 => "HP PA-RISC",
            3 => "UEFI",
            0x70 => "NVIDIA private (signed)",
            _ => "other",
        }
    }
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct VbiosInfo {
    pub source: String,
    pub size: usize,
    pub sha256: String,
    /// Hash over the PCI image chain only (ignores padding after the last image).
    pub images_sha256: String,
    pub images: Vec<RomImage>,
    pub uefi: bool,
    pub legacy_checksum_ok: Option<bool>,
    pub version: Option<String>,
    pub part_number: Option<String>,
    pub build_date: Option<String>,
    // AMD ATOM specific
    pub atom: bool,
    pub atom_subsys_vendor: Option<u16>,
    pub atom_subsys_device: Option<u16>,
    #[serde(skip)]
    pub powerplay_table: Option<Vec<u8>>,
}

impl VbiosInfo {
    /// The image chain does not end with a "last image" flag: more images
    /// (usually the UEFI GOP driver) exist but are not part of this copy.
    pub fn truncated(&self) -> bool {
        self.images.last().is_some_and(|i| !i.last)
    }

    pub fn pcir_ids(&self) -> Vec<(u16, u16)> {
        let mut v: Vec<(u16, u16)> = self.images.iter().map(|i| (i.vendor, i.device)).collect();
        v.dedup();
        v
    }
}

pub fn parse(rom: &[u8], source: &str) -> VbiosInfo {
    let mut info = VbiosInfo {
        source: source.to_string(),
        size: rom.len(),
        sha256: sha256_hex(rom),
        ..Default::default()
    };
    info.images = find_images(rom);
    info.uefi = info.images.iter().any(|i| i.code_type == 3);
    if let (Some(f), Some(l)) = (info.images.first(), info.images.last()) {
        let end = (l.offset + l.length).min(rom.len());
        info.images_sha256 = sha256_hex(&rom[f.offset..end]);
    }
    info.legacy_checksum_ok = info
        .images
        .iter()
        .find(|i| i.code_type == 0)
        .and_then(|i| i.checksum_ok);

    let base = info.images.first().map(|i| i.offset).unwrap_or(0);
    let img = &rom[base..];
    parse_atom(img, &mut info);
    if !info.atom {
        parse_nvidia(rom, &mut info);
    }
    info
}

fn find_images(rom: &[u8]) -> Vec<RomImage> {
    let mut out = Vec::new();
    // Some ROM dumps (NVIDIA Turing+) are prefixed by a private header, so scan
    // 512-byte aligned offsets for the first valid image, then follow the chain.
    let mut off = (0..rom.len().saturating_sub(0x1a))
        .step_by(512)
        .find(|&o| valid_image_at(rom, o));
    let mut guard = 0;
    while let Some(o) = off {
        guard += 1;
        if guard > 16 {
            break;
        }
        let Some(pcir) = le16(rom, o + 0x18).map(|x| o + x as usize) else { break };
        if rom.get(pcir..pcir + 4) != Some(b"PCIR") {
            break;
        }
        let vendor = le16(rom, pcir + 4).unwrap_or(0);
        let device = le16(rom, pcir + 6).unwrap_or(0);
        let len = le16(rom, pcir + 0x10).unwrap_or(0) as usize * 512;
        let code_type = rom.get(pcir + 0x14).copied().unwrap_or(0xff);
        let last = rom.get(pcir + 0x15).copied().unwrap_or(0x80) & 0x80 != 0;
        let checksum_ok = if code_type == 0 {
            let sz = rom.get(o + 2).copied().unwrap_or(0) as usize * 512;
            rom.get(o..o + sz)
                .filter(|_| sz > 0)
                .map(|b| b.iter().fold(0u8, |a, &x| a.wrapping_add(x)) == 0)
        } else {
            None
        };
        out.push(RomImage { offset: o, length: len, vendor, device, code_type, last, checksum_ok });
        if last || len == 0 {
            break;
        }
        let next = o + len;
        off = if valid_image_at(rom, next) { Some(next) } else { None };
    }
    out
}

fn valid_image_at(rom: &[u8], o: usize) -> bool {
    if rom.get(o..o + 2) != Some(&[0x55, 0xAA]) {
        return false;
    }
    match le16(rom, o + 0x18) {
        Some(p) => rom.get(o + p as usize..o + p as usize + 4) == Some(b"PCIR"),
        None => false,
    }
}

fn parse_atom(img: &[u8], info: &mut VbiosInfo) {
    let Some(hdr) = le16(img, 0x48).map(|x| x as usize) else { return };
    if img.get(hdr + 4..hdr + 8) != Some(b"ATOM") {
        return;
    }
    info.atom = true;
    info.atom_subsys_vendor = le16(img, hdr + 24);
    info.atom_subsys_device = le16(img, hdr + 26);

    // Part number: AMD part numbers look like "113-XXXXXXXX-XXX".
    if let Some(p) = find_ascii(img, b"113-", 0x2000) {
        let s: String = img[p..]
            .iter()
            .take_while(|c| c.is_ascii_graphic())
            .map(|&c| c as char)
            .collect();
        info.part_number = Some(s);
    }
    // Build date string "YYYY/MM/DD HH:MM" is located near offset 0x50.
    for o in 0x40..0x200usize {
        if let Some(b) = img.get(o..o + 16) {
            if b[4] == b'/' && b[7] == b'/' && b[0..4].iter().all(u8::is_ascii_digit) {
                info.build_date = Some(String::from_utf8_lossy(&b[..16]).trim().to_string());
                break;
            }
        }
    }

    // Master data table -> PowerPlayInfo (index 15 in both atombios and atomfirmware).
    if let Some(mdt) = le16(img, hdr + 32).map(|x| x as usize) {
        if let Some(pp_off) = le16(img, mdt + 4 + 15 * 2).map(|x| x as usize) {
            if pp_off != 0 {
                if let Some(sz) = le16(img, pp_off).map(|x| x as usize) {
                    if let Some(t) = img.get(pp_off..pp_off + sz) {
                        info.powerplay_table = Some(t.to_vec());
                    }
                }
            }
        }
    }
}

fn parse_nvidia(rom: &[u8], info: &mut VbiosInfo) {
    // BIT header: FF B8 'B' 'I' 'T' 00
    let Some(p) = rom.windows(6).position(|w| w == [0xFF, 0xB8, b'B', b'I', b'T', 0]) else {
        return;
    };
    let hdr_size = rom.get(p + 8).copied().unwrap_or(0) as usize;
    let tok_size = rom.get(p + 9).copied().unwrap_or(0) as usize;
    let tok_cnt = rom.get(p + 10).copied().unwrap_or(0) as usize;
    if tok_size < 6 {
        return;
    }
    // Offsets in BIT tokens are relative to the start of the first PCI image.
    let base = info.images.first().map(|i| i.offset).unwrap_or(0);
    for i in 0..tok_cnt {
        let t = p + hdr_size + i * tok_size;
        if rom.get(t) == Some(&b'B') {
            let Some(data) = le16(rom, t + 4).map(|x| base + x as usize) else { return };
            let Some(ver) = le32(rom, data) else { return };
            let oem = rom.get(data + 4).copied().unwrap_or(0);
            info.version = Some(format!(
                "{:02X}.{:02X}.{:02X}.{:02X}.{:02X}",
                ver >> 24,
                (ver >> 16) & 0xff,
                (ver >> 8) & 0xff,
                ver & 0xff,
                oem
            ));
            return;
        }
    }
}

fn find_ascii(b: &[u8], needle: &[u8], limit: usize) -> Option<usize> {
    let end = b.len().min(limit);
    b[..end].windows(needle.len()).position(|w| w == needle)
}
