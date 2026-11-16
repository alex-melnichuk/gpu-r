//! Unified GPU model, field provenance and cross-validation.

use crate::amd::{self, AmdInfo};
use crate::db::{self, DbMatch};
use crate::gputest::{ComputeResult, MemTestResult};
use crate::integrity::{self, DriverIntegrity};
use crate::nvidia::{self, NvInfo};
use crate::pci::{self, PciDevice};
use crate::util::*;
use crate::vbios::{self, VbiosInfo};
use serde::Serialize;

#[derive(Debug, Clone, Serialize, Default)]
pub struct VkInfo {
    pub name: String,
    pub vendor: u32,
    pub device: u32,
    pub driver: String,
    pub driver_info: String,
    pub backend: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Gpu {
    pub pci: PciDevice,
    pub amd: Option<AmdInfo>,
    pub nv: Option<NvInfo>,
    pub db: Option<DbMatch>,
    /// VBIOS image as the driver loaded it.
    pub vbios_driver: Option<VbiosInfo>,
    /// VBIOS read straight from the PCI expansion ROM BAR (root only).
    pub vbios_rom: Option<VbiosInfo>,
    pub vk: Option<VkInfo>,
    pub integrity: Option<DriverIntegrity>,
    pub card: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Status {
    Ok,
    Info,
    Warn,
    Fail,
    Skipped,
}

impl Status {
    pub fn tag(&self) -> &'static str {
        match self {
            Status::Ok => "OK",
            Status::Info => "INFO",
            Status::Warn => "WARN",
            Status::Fail => "FAIL",
            Status::Skipped => "SKIP",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub group: &'static str,
    pub title: String,
    pub status: Status,
    pub detail: String,
}

/// A single displayed value with its provenance.
#[derive(Debug, Clone, Serialize)]
pub struct Field {
    pub value: String,
    pub status: Status,
    pub sources: String,
    /// Independent sources consulted and whether each one agreed.
    pub evidence: Vec<(String, bool)>,
}

impl Field {
    pub fn new(v: impl Into<String>, st: Status, src: impl Into<String>) -> Self {
        let sources: String = src.into();
        let agree = !matches!(st, Status::Fail | Status::Skipped);
        let evidence = sources
            .split([',', '+', ';'])
            .map(|x| x.trim())
            .filter(|x| !x.is_empty())
            .map(|x| (x.to_string(), agree))
            .collect();
        Self { value: v.into(), status: st, sources, evidence }
    }
    fn ev(mut self, e: Vec<(String, bool)>) -> Self {
        if !e.is_empty() {
            self.evidence = e;
        }
        self
    }
    pub fn na() -> Self {
        Self::new("N/A", Status::Skipped, "no source")
    }
}

pub fn vendor_name(v: u16) -> &'static str {
    match v {
        0x1002 => "AMD",
        0x10DE => "NVIDIA",
        0x8086 => "Intel",
        0x1A03 => "ASPEED",
        0x15AD => "VMware",
        0x1AF4 => "Red Hat (virtio)",
        _ => "Unknown",
    }
}

pub fn enumerate_vulkan() -> Vec<VkInfo> {
    let inst = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: crate::util::BACKENDS,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let mut ad = pollster::block_on(inst.enumerate_adapters(crate::util::BACKENDS));
    ad.sort_by_key(|a| a.get_info().backend != wgpu::Backend::Vulkan);
    ad.into_iter()
        .map(|a| {
            let i = a.get_info();
            VkInfo {
                name: i.name,
                vendor: i.vendor,
                device: i.device,
                driver: i.driver,
                driver_info: i.driver_info,
                backend: format!("{:?}", i.backend),
            }
        })
        .collect()
}

pub fn collect_all() -> Vec<Gpu> {
    let vks = enumerate_vulkan();
    pci::enumerate()
        .into_iter()
        .map(|p| collect_one(p, &vks))
        .collect()
}

fn read_rom(p: &PciDevice) -> Option<Vec<u8>> {
    if !is_root() {
        return None;
    }
    let rom = p.sysfs.join("rom");
    std::fs::write(&rom, "1").ok()?;
    let data = std::fs::read(&rom).ok();
    let _ = std::fs::write(&rom, "0");
    data.filter(|d| d.len() > 512 && d[0] == 0x55 && d[1] == 0xAA)
}

pub fn collect_one(pci: PciDevice, vks: &[VkInfo]) -> Gpu {
    let mut g = Gpu { card: amd::card_node_for(&pci.sysfs), ..Default::default() };
    let drv = pci.driver.clone().unwrap_or_default();
    if drv == "amdgpu" {
        let a = amd::collect(&pci.sysfs);
        if let Some(img) = &a.vbios_image {
            g.vbios_driver = Some(vbios::parse(img, "amdgpu driver (AMDGPU_INFO_VBIOS)"));
        }
        g.amd = Some(a);
    }
    if drv == "nvidia" {
        if let Some(n) = nvidia::nvml() {
            g.nv = n.info(&pci.addr);
        }
    }
    if let Some(rom) = read_rom(&pci) {
        g.vbios_rom = Some(vbios::parse(&rom, "PCI expansion ROM (sysfs rom)"));
    }
    let units = g.amd.as_ref().map(|a| a.cu_active_number).filter(|&x| x > 0).or_else(|| {
        // NVIDIA: cores / cores-per-SM is architecture dependent; use cores via DB shaders instead.
        None
    });
    g.db = db::lookup(pci.vendor, pci.device, pci.rev_known.then_some(pci.revision), units);
    if g.db.is_none() || g.nv.as_ref().and_then(|n| n.cores).is_some() {
        // For NVIDIA, refine by core count if possible.
        if let (Some(n), Some(m)) = (g.nv.as_ref().and_then(|n| n.cores), g.db.as_ref()) {
            if m.spec.shaders != n {
                if let Some(s) = db::all().iter().find(|s| s.device == pci.device && s.shaders == n) {
                    g.db = Some(DbMatch { spec: s.clone(), how: "device ID + core count", candidates: m.candidates.clone() });
                }
            }
        }
    }
    g.vk = vks
        .iter()
        .find(|v| v.vendor == pci.vendor as u32 && v.device == pci.device as u32)
        .cloned();
    if !drv.is_empty() && cfg!(unix) {
        g.integrity = Some(integrity::collect(&drv, pci.vendor));
    }
    g.pci = pci;
    g
}

impl Gpu {
    pub fn title(&self) -> String {
        if let Some(n) = self.nv.as_ref().and_then(|n| n.name.clone()) {
            return n;
        }
        if let Some(m) = &self.db {
            let ambiguous = m.how.contains("ambiguous");
            if !(ambiguous && self.pci.device_name.is_some()) {
                return format!("{} {}", vendor_name(self.pci.vendor), m.spec.name);
            }
        }
        if let Some(n) = &self.pci.device_name {
            return n.clone();
        }
        format!("{} GPU {:04X}", vendor_name(self.pci.vendor), self.pci.device)
    }

    pub fn vbios(&self) -> Option<&VbiosInfo> {
        self.vbios_rom.as_ref().or(self.vbios_driver.as_ref())
    }

    /// Best estimate of VRAM size and how many sources agree on it.
    pub fn vram_sources(&self) -> Vec<(&'static str, u64)> {
        let mut v = vec![];
        if let Some(a) = &self.amd {
            if let Some(s) = a.sysfs_vram_total {
                v.push(("sysfs mem_info_vram_total", s));
            }
            if a.ioctl_heap_vram_total > 0 {
                v.push(("ioctl AMDGPU_INFO_MEMORY", a.ioctl_heap_vram_total));
            }
        }
        if let Some(n) = self.nv.as_ref().and_then(|n| n.mem_total) {
            v.push(("NVML memory total", n));
        }
        if let Some(n) = self.pci.os_vram {
            v.push(("Windows driver registry", n));
        }
        v
    }

    pub fn vram_bytes(&self) -> Option<u64> {
        self.vram_sources().first().map(|x| x.1)
    }

    pub fn bus_width(&self) -> Option<u32> {
        self.amd
            .as_ref()
            .map(|a| a.vram_bit_width)
            .filter(|&x| x > 0)
            .or_else(|| self.nv.as_ref().and_then(|n| n.bus_width))
    }

    pub fn shaders(&self) -> Option<u32> {
        if let Some(a) = &self.amd {
            if a.cu_active_number > 0 {
                return Some(a.cu_active_number * 64);
            }
        }
        self.nv.as_ref().and_then(|n| n.cores)
    }

    pub fn rops(&self) -> Option<u32> {
        self.amd.as_ref().map(|a| a.rops()).filter(|&x| x > 0)
    }

    pub fn tmus(&self) -> Option<u32> {
        self.amd.as_ref().map(|a| a.cu_active_number * 4).filter(|&x| x > 0)
    }

    pub fn max_gpu_clock(&self) -> Option<u32> {
        if let Some(a) = &self.amd {
            if let Some(&m) = a.sclk_levels.iter().max() {
                return Some(m);
            }
            if a.max_engine_clock_khz > 0 {
                return Some((a.max_engine_clock_khz / 1000) as u32);
            }
        }
        self.nv.as_ref().and_then(|n| n.max_gpu_clock)
    }

    pub fn max_mem_clock(&self) -> Option<u32> {
        if let Some(a) = &self.amd {
            if let Some(&m) = a.mclk_levels.iter().max() {
                return Some(m);
            }
            if a.max_memory_clock_khz > 0 {
                return Some((a.max_memory_clock_khz / 1000) as u32);
            }
        }
        self.nv.as_ref().and_then(|n| n.max_mem_clock)
    }

    /// Effective memory data rate in Gbps per pin.
    pub fn mem_gbps(&self) -> Option<f64> {
        let mc = self.max_mem_clock()? as f64;
        if let Some(a) = &self.amd {
            return Some(mc * a.mem_rate_mult() / 1000.0);
        }
        if self.nv.is_some() {
            return Some(mc * 2.0 / 1000.0);
        }
        None
    }

    pub fn bandwidth_gbs(&self) -> Option<f64> {
        Some(self.bus_width()? as f64 / 8.0 * self.mem_gbps()?)
    }

    pub fn mem_type(&self) -> Option<String> {
        if let Some(a) = &self.amd {
            if a.vram_type > 0 {
                return Some(a.vram_type_name().to_string());
            }
        }
        self.db.as_ref().map(|m| m.spec.mem.name().to_string())
    }

    pub fn driver_version(&self) -> String {
        if let Some(n) = &self.nv {
            return format!("NVIDIA {}", n.driver_version.clone().unwrap_or_default());
        }
        if let Some(v) = &self.pci.os_driver_version {
            let date = self.pci.os_driver_date.clone().map(|d| format!(" ({d})")).unwrap_or_default();
            return format!("{v}{date} / Windows");
        }
        let k = read_str("/proc/sys/kernel/osrelease").unwrap_or_default();
        let drm = self.amd.as_ref().and_then(|a| a.drm_version.clone()).unwrap_or_else(|| self.pci.driver.clone().unwrap_or_default());
        let mut s = format!("{drm} / Linux {k}");
        if let Some(vk) = &self.vk {
            s.push_str(&format!(" / {} {}", vk.driver, vk.driver_info));
        }
        s
    }

    // ------------------------------------------------------------------
    // Displayed fields (GPU-info tab) with provenance
    // ------------------------------------------------------------------

    pub fn fields(&self, mt: Option<&MemTestResult>, cr: Option<&ComputeResult>) -> Vec<(&'static str, Field)> {
        let mut f = vec![];
        let spec = self.db.as_ref().map(|m| &m.spec);
        let p = &self.pci;

        let ambiguous = self.db.as_ref().is_some_and(|m| m.how.contains("ambiguous"));
        let name = match &self.db {
            // several SKUs share this ID and nothing told them apart: say so instead of guessing
            Some(m) if ambiguous && self.nv.is_none() => format!("{} ({})", m.candidates.join(" / "), m.spec.chip),
            _ => self.title(),
        };
        f.push(("Name", Field::new(name, if self.db.is_some() && !ambiguous { Status::Ok } else { Status::Info },
            self.db.as_ref().map(|m| format!("reference DB by {}", m.how)).unwrap_or("pci.ids / driver".into()))));
        f.push(("GPU", spec.map(|s| Field::new(s.chip, Status::Ok, "reference DB")).unwrap_or(Field::na())));
        f.push(("Revision", if p.rev_known {
            Field::new(format!("{:02X}", p.revision),
                if p.cfg_revision.is_none_or(|r| r == p.revision) { Status::Ok } else { Status::Fail },
                if cfg!(windows) { "PnP instance ID" } else { "PCI config space + sysfs" })
        } else {
            Field::na()
        }));
        f.push(("Technology", spec.map(|s| Field::new(s.process, Status::Info, "reference DB")).unwrap_or(Field::na())));
        f.push(("Die Size", spec.map(|s| Field::new(format!("{} mm²", s.die_mm2), Status::Info, "reference DB")).unwrap_or(Field::na())));
        f.push(("Release Date", spec.map(|s| Field::new(s.release, Status::Info, "reference DB")).unwrap_or(Field::na())));
        f.push(("Transistors", spec.map(|s| Field::new(format!("{}M", s.transistors_m), Status::Info, "reference DB")).unwrap_or(Field::na())));

        // BIOS version
        let bv = self.amd.as_ref().and_then(|a| a.vbios_ver_str.clone().or(a.sysfs_vbios_version.clone()))
            .or_else(|| self.nv.as_ref().and_then(|n| n.vbios.clone()))
            .or_else(|| self.vbios().and_then(|v| v.version.clone()));
        let bios_st = self.vbios().map(|v| match v.legacy_checksum_ok {
            Some(false) => Status::Fail,
            _ => Status::Ok,
        }).unwrap_or(Status::Info);
        f.push(("BIOS Version", Field::new(bv.unwrap_or("N/A".into()), bios_st,
            self.vbios().map(|v| format!("{} sha256 {}…", v.source, &v.sha256[..16])).unwrap_or("driver".into()))));
        f.push(("UEFI", Field::new(
            self.vbios().map(|v| if v.uefi { "Yes" } else if v.truncated() { "Unknown (driver copy has legacy image only; run as root)" } else { "No" }).unwrap_or("?"),
            Status::Info, "VBIOS image PCIR code types")));

        let sv = pci::board_vendor(p.subsys_vendor).map(str::to_string)
            .or(p.subsys_name.clone()).unwrap_or(format!("{:04X}", p.subsys_vendor));
        f.push(("Subvendor", Field::new(sv, Status::Info, "PCI subsystem ID")));
        let id_ok = p.cfg_device.is_none_or(|d| d == p.device)
            && self.amd.as_ref().is_none_or(|a| !a.ioctl_ok || a.device_id == p.device as u32)
            && self.vk.as_ref().is_none_or(|v| v.device == p.device as u32);
        let mut idev = vec![("sysfs".to_string(), true)];
        if let Some(d) = p.cfg_device { idev.push(("PCI config space".into(), d == p.device)); }
        if let Some(a) = self.amd.as_ref().filter(|a| a.ioctl_ok) { idev.push(("amdgpu ioctl".into(), a.device_id == p.device as u32)); }
        if let Some(n) = self.nv.as_ref().and_then(|n| n.pci_device_id) { idev.push(("NVML".into(), (n >> 16) as u16 == p.device)); }
        if let Some(v) = &self.vk { idev.push(("Vulkan".into(), v.device == p.device as u32)); }
        if let Some(v) = self.vbios() { if !v.images.is_empty() { idev.push(("VBIOS PCIR".into(), v.images.iter().all(|i| i.device == p.device))); } }
        f.push(("Device ID", Field::new(p.id_string(), if id_ok { Status::Ok } else { Status::Fail },
            "config space, sysfs, driver ioctl, Vulkan").ev(idev)));

        let rops_tmus = match (self.rops(), self.tmus()) {
            (Some(r), Some(t)) => format!("{r} / {t}"),
            _ => spec.map(|s| format!("{} / {} (ref)", s.rops, s.tmus)).unwrap_or("N/A".into()),
        };
        let rt_st = match (self.rops(), spec) {
            (Some(r), Some(s)) if r == s.rops => Status::Ok,
            (Some(_), Some(_)) => Status::Warn,
            _ => Status::Info,
        };
        f.push(("ROPs/TMUs", Field::new(rops_tmus, rt_st, "driver RB/CU count vs DB")));

        let bus = format!(
            "PCIe x{} {} @ x{} {}",
            p.max_link_width.unwrap_or(0),
            p.max_link_speed.as_deref().and_then(PciDevice::link_gen).map(|g| format!("{g}.0")).unwrap_or("?".into()),
            p.link_width.unwrap_or(0),
            p.link_speed.as_deref().and_then(PciDevice::link_gen).map(|g| format!("{g}.0")).unwrap_or("?".into()),
        );
        f.push(("Bus Interface", Field::new(bus, Status::Info, "PCIe link registers (sysfs)")));

        let sh = self.shaders();
        let sh_st = match (sh, spec, cr) {
            (Some(a), Some(s), _) if a != s.shaders => Status::Warn,
            (Some(_), Some(_), Some(c)) if c.verdict_ok == Some(false) => Status::Warn,
            (Some(_), Some(_), _) => Status::Ok,
            _ => Status::Info,
        };
        f.push(("Shaders", Field::new(
            sh.map(|x| format!("{x} Unified")).or(spec.map(|s| format!("{} (ref)", s.shaders))).unwrap_or("N/A".into()),
            sh_st, if cr.is_some() { "driver CU bitmap + DB + measured FP32" } else { "driver CU bitmap + DB" }).ev({
                let mut e = vec![];
                if let Some(a) = self.amd.as_ref().filter(|a| a.ioctl_ok) {
                    e.push(("driver CU count".to_string(), true));
                    e.push(("CU bitmap (harvest regs)".to_string(), a.cu_bitmap_count == a.cu_active_number));
                }
                if let Some(c) = self.nv.as_ref().and_then(|n| n.cores) { e.push(("NVML cores".into(), true)); let _ = c; }
                if let (Some(a), Some(s)) = (sh, spec) { e.push(("reference DB".into(), a == s.shaders)); }
                if let Some(c) = cr { e.push(("measured FP32".into(), c.verdict_ok != Some(false))); }
                e
            })));
        f.push(("Vulkan", Field::new(
            self.vk.as_ref().map(|v| format!("{} ({})", v.name, v.driver)).unwrap_or("not available".into()),
            Status::Info, "wgpu/Vulkan adapter")));

        if let (Some(r), Some(c)) = (self.rops().or(spec.map(|s| s.rops)), self.max_gpu_clock()) {
            f.push(("Pixel Fillrate", Field::new(format!("{:.1} GPixel/s", r as f64 * c as f64 / 1000.0), Status::Info, "ROPs × max clock")));
        }
        if let (Some(t), Some(c)) = (self.tmus().or(spec.map(|s| s.tmus)), self.max_gpu_clock()) {
            f.push(("Texture Fillrate", Field::new(format!("{:.1} GTexel/s", t as f64 * c as f64 / 1000.0), Status::Info, "TMUs × max clock")));
        }

        let mt_st = match (self.mem_type(), spec) {
            (Some(m), Some(s)) if !m.eq_ignore_ascii_case(s.mem.name()) => Status::Warn,
            (Some(_), Some(_)) => Status::Ok,
            _ => Status::Info,
        };
        let vendor = self.amd.as_ref().and_then(|a| a.mem_vendor.clone()).map(|v| format!(" ({})", capitalize(&v))).unwrap_or_default();
        f.push(("Memory Type", Field::new(format!("{}{}", self.mem_type().unwrap_or("N/A".into()), vendor), mt_st, "driver (VBIOS vram_info) vs DB")));

        let bw_st = match (self.bus_width(), spec) {
            (Some(b), Some(s)) if b != s.bus_width => Status::Warn,
            (Some(_), Some(_)) => Status::Ok,
            _ => Status::Info,
        };
        f.push(("Bus Width", Field::new(self.bus_width().map(|b| format!("{b} bit")).unwrap_or("N/A".into()), bw_st, "driver vs DB")));

        let srcs = self.vram_sources();
        let all_same = srcs.windows(2).all(|w| w[0].1 == w[1].1);
        let vram = self.vram_bytes();
        let mut vst = if !all_same { Status::Fail } else if vram.is_some() { Status::Ok } else { Status::Info };
        if let (Some(v), Some(s)) = (vram, spec) {
            if !s.size_matches(v) {
                vst = Status::Warn;
            }
        }
        let mut vsrc = format!("{} driver source(s) agree={all_same}", srcs.len());
        if let Some(m) = mt {
            if m.verified_capacity() {
                vsrc.push_str(&format!("; memtest verified {}", fmt_bytes(m.allocated_bytes)));
            } else if m.errors > 0 {
                vst = Status::Fail;
                vsrc.push_str("; MEMTEST ERRORS");
            }
        }
        let mut vev: Vec<(String, bool)> = srcs.iter().map(|(n, v)| (n.to_string(), Some(*v) == vram)).collect();
        if let Some(s) = spec {
            vev.push(("reference DB".into(), vram.is_some_and(|v| s.size_matches(v))));
        }
        if let Some(b) = p.vram_bar() { if vram.is_some_and(|v| b.size >= v) { vev.push(("PCI BAR aperture".into(), true)); } }
        if let Some(m) = mt { vev.push(("VRAM test".into(), m.verified_capacity())); }
        f.push(("Memory Size", Field::new(vram.map(fmt_bytes).unwrap_or("N/A".into()), vst, vsrc).ev(vev)));

        let mut bw = Field::new(self.bandwidth_gbs().map(|b| format!("{b:.1} GB/s")).unwrap_or("N/A".into()), Status::Info, "bus width × data rate");
        if let Some(m) = mt {
            if m.read_gbs > 0.0 {
                bw.value.push_str(&format!(" (measured {:.0})", m.read_gbs.max(m.write_gbs)));
            }
        }
        f.push(("Bandwidth", bw));
        f.push(("Driver Version", Field::new(self.driver_version(), Status::Info, "DRM ioctl / NVML / Vulkan")));
        let drv_st = match &self.integrity {
            Some(i) if i.files.iter().any(|f| f.ok == Some(false)) || !i.preload.is_empty() => Status::Fail,
            Some(i) if i.srcversion_disk.is_some() && i.srcversion_disk != i.srcversion_loaded => Status::Warn,
            Some(i) if i.files.iter().any(|f| f.ok == Some(true)) => Status::Ok,
            _ => Status::Info,
        };
        f.push(("Driver Integrity", Field::new(match drv_st {
            Status::Ok => "Distribution package, unmodified",
            Status::Fail => "MODIFIED / injected",
            Status::Warn => "loaded module differs from disk",
            _ => "unverified",
        }, drv_st, "package-manager checksums, srcversion, preload")));

        let clk = self.max_gpu_clock();
        f.push(("Boost Clock", Field::new(clk.map(|c| format!("{c} MHz")).unwrap_or("N/A".into()), Status::Info, "driver DPM table")));
        f.push(("Reference Boost", spec.map(|s| Field::new(format!("{} MHz", s.boost_mhz), Status::Info, "reference DB")).unwrap_or(Field::na())));
        f.push(("Memory Clock", Field::new(
            self.max_mem_clock().map(|c| format!("{c} MHz ({:.1} Gbps eff.)", self.mem_gbps().unwrap_or(0.0))).unwrap_or("N/A".into()),
            Status::Info, "driver DPM table")));

        let rebar = match (p.vram_bar(), vram) {
            (Some(b), Some(v)) if b.size >= v => "Enabled (full VRAM aperture)".to_string(),
            (Some(b), _) if !p.rebar_sizes.is_empty() => format!("Supported, BAR = {}", fmt_bytes(b.size)),
            (Some(b), _) => format!("Disabled, BAR = {}", fmt_bytes(b.size)),
            _ => "Unknown".into(),
        };
        f.push(("Resizable BAR", Field::new(rebar, Status::Info, "PCI BAR sizes + resource0_resize")));
        let mut comp = vec![];
        if self.vk.is_some() { comp.push("Vulkan"); }
        if std::path::Path::new("/dev/kfd").exists() && self.pci.vendor == 0x1002 { comp.push("ROCm/HIP"); }
        if self.nv.is_some() { comp.push("CUDA"); }
        if std::fs::read_dir("/etc/OpenCL/vendors").map(|r| r.count() > 0).unwrap_or(false) { comp.push("OpenCL"); }
        f.push(("Computing", Field::new(comp.join(", "), Status::Info, "runtime presence")));
        f
    }

    // ------------------------------------------------------------------
    // Validation
    // ------------------------------------------------------------------

    pub fn checks(&self, mt: Option<&MemTestResult>, cr: Option<&ComputeResult>) -> Vec<Check> {
        let mut c = vec![];
        let mut add = |group: &'static str, title: &str, status: Status, detail: String| {
            c.push(Check { group, title: title.into(), status, detail })
        };
        let p = &self.pci;
        let spec = self.db.as_ref().map(|m| &m.spec);

        // ---- Identity ----
        let mut ids = vec![format!("sysfs {:04X}:{:04X}", p.vendor, p.device)];
        let mut ok = true;
        if let (Some(v), Some(d)) = (p.cfg_vendor, p.cfg_device) {
            ids.push(format!("config space {v:04X}:{d:04X}"));
            ok &= v == p.vendor && d == p.device;
        }
        if let Some(a) = self.amd.as_ref().filter(|a| a.ioctl_ok) {
            ids.push(format!("amdgpu ioctl {:04X} rev {:02X}", a.device_id, a.pci_rev));
            ok &= a.device_id == p.device as u32;
        }
        if let Some(n) = self.nv.as_ref().and_then(|n| n.pci_device_id) {
            ids.push(format!("NVML {:04X}:{:04X}", n & 0xffff, n >> 16));
            ok &= (n >> 16) as u16 == p.device;
        }
        if let Some(v) = &self.vk {
            ids.push(format!("Vulkan {:04X}:{:04X}", v.vendor, v.device));
            ok &= v.device == p.device as u32;
        }
        add("Identity", "Device ID consistent across sources", if ok { Status::Ok } else { Status::Fail }, ids.join(", "));

        match &self.db {
            Some(m) => add("Identity", "Reference SKU", Status::Ok,
                format!("{} ({}) — selected by {}; SKUs on this ID: {}", m.spec.name, m.spec.chip, m.how, m.candidates.join(", "))),
            None => add("Identity", "Reference SKU", Status::Warn,
                format!("{:04X}:{:04X} rev {:02X} not in reference DB — hardware checks below are driver-only", p.vendor, p.device, p.revision)),
        }

        if let Some(m) = &self.db {
            add("Identity", "Reference data consistency", if m.spec.verified { Status::Ok } else { Status::Info },
                if m.spec.verified {
                    format!("published numbers agree with each other; specs: {}; IDs: {}", m.spec.spec_source, m.spec.id_sources.join(", "))
                } else {
                    format!("published numbers disagree ({}); differences vs reference are less reliable",
                        m.spec.checks.iter().filter(|c| c.starts_with("FAIL")).cloned().collect::<Vec<_>>().join("; "))
                });
        }

        if let (Some(a), Some(s)) = (&self.amd, spec) {
            match (a.gc_ip, s.gc_ip) {
                (Some(h), Some(e)) => add("Identity", "Graphics core IP version (silicon)",
                    if h == e { Status::Ok } else { Status::Fail },
                    format!("IP discovery GC {}.{}.{} / expected {}.{}.{} for {}{}", h.0, h.1, h.2, e.0, e.1, e.2, s.chip,
                        if h != e { " — chip is NOT what the device ID claims (relabelled/fake?)" } else { "" })),
                (None, Some(_)) => add("Identity", "Graphics core IP version (silicon)", Status::Skipped, "IP discovery not exposed".into()),
                _ => {}
            }
        }
        if let (Some(n), Some(s)) = (&self.nv, spec) {
            if let (Some(cc), Some(e)) = (n.cuda_cc, s.cuda_cc) {
                let ok = cc.0 == e.0 as u32 && cc.1 == e.1 as u32;
                add("Identity", "CUDA compute capability (architecture)", if ok { Status::Ok } else { Status::Fail },
                    format!("{}.{} reported / {}.{} expected for {}{}", cc.0, cc.1, e.0, e.1, s.chip,
                        if ok { "" } else { " — different silicon generation: relabelled card" }));
            }
        }

        // ---- VBIOS ----
        let vb = self.vbios();
        match vb {
            None => add("VBIOS", "VBIOS image", Status::Skipped,
                if is_root() { "could not read ROM".into() } else { "run as root to read the ROM directly".into() }),
            Some(v) => {
                let ids = v.pcir_ids();
                let mismatch: Vec<_> = ids.iter().filter(|(ve, d)| *ve != p.vendor || *d != p.device).collect();
                add("VBIOS", "Device ID embedded in VBIOS (PCIR)",
                    if ids.is_empty() { Status::Skipped } else if mismatch.is_empty() { Status::Ok } else { Status::Warn },
                    format!("images: {}{}", v.images.iter().map(|i| format!("{:04X}:{:04X} [{}]", i.vendor, i.device, i.code_type_name())).collect::<Vec<_>>().join(", "),
                        if mismatch.is_empty() { String::new() } else { " — BIOS made for another device: cross-flashed".into() }));
                add("VBIOS", "Legacy image checksum", match v.legacy_checksum_ok {
                    Some(true) => Status::Ok, Some(false) => Status::Fail, None => Status::Skipped },
                    match v.legacy_checksum_ok {
                        Some(true) => "byte sum == 0".into(),
                        Some(false) => "checksum invalid — image was edited without fixing the checksum".into(),
                        None => "no legacy image".into(),
                    });
                if v.atom {
                    if let (Some(sv), Some(sd)) = (v.atom_subsys_vendor, v.atom_subsys_device) {
                        let ok = sv == p.subsys_vendor && sd == p.subsys_device;
                        add("VBIOS", "Board (subsystem) ID in ATOM header", if ok { Status::Ok } else { Status::Warn },
                            format!("BIOS {sv:04X}:{sd:04X} vs board {:04X}:{:04X}{}", p.subsys_vendor, p.subsys_device,
                                if ok { "" } else { " — BIOS from another board vendor/model" }));
                    }
                }
                if let (Some(d), Some(r)) = (&self.vbios_driver, &self.vbios_rom) {
                    let same = d.images_sha256 == r.images_sha256;
                    add("VBIOS", "Driver VBIOS == physical ROM", if same { Status::Ok } else { Status::Info },
                        format!("images sha256: driver {:.16}… / ROM {:.16}…{}", d.images_sha256, r.images_sha256,
                            if same { "" } else { " (differs: driver may use the ACPI VFCT copy or an override)" }));
                }
                if let Some(a) = &self.amd {
                    let pn_rom = v.part_number.clone();
                    let pn_drv = a.vbios_pn.clone().or(a.sysfs_vbios_version.clone());
                    if let (Some(x), Some(y)) = (&pn_rom, &pn_drv) {
                        add("VBIOS", "Part number (image vs driver)", if x == y { Status::Ok } else { Status::Warn },
                            format!("image {x} / driver {y}"));
                    }
                    if let (Some(pp_bios), Some(pp_drv)) = (&v.powerplay_table, &a.pp_table) {
                        let same = pp_bios == pp_drv;
                        add("VBIOS", "PowerPlay table in use == VBIOS table", if same { Status::Ok } else { Status::Warn },
                            if same { format!("{} bytes identical", pp_drv.len()) }
                            else { format!("VBIOS {} bytes vs active {} bytes — soft PowerPlay table (SPPT / MorePowerTool) is applied: clocks/voltages/power limits are user-modified", pp_bios.len(), pp_drv.len()) });
                    }
                }
                if let (Some(n), Some(bv)) = (&self.nv, &v.version) {
                    if let Some(nv) = &n.vbios {
                        add("VBIOS", "VBIOS version (ROM vs NVML)",
                            if nv.eq_ignore_ascii_case(bv) { Status::Ok } else { Status::Warn }, format!("ROM {bv} / NVML {nv}"));
                    }
                }
            }
        }

        // ---- Shader configuration ----
        if let Some(a) = self.amd.as_ref().filter(|a| a.ioctl_ok) {
            add("Configuration", "CU count self-consistency", if a.cu_bitmap_count == a.cu_active_number { Status::Ok } else { Status::Warn },
                format!("cu_active_number {} / CU bitmap popcount {} / physical slots {} ({} SE × {} SH × {} CU)",
                    a.cu_active_number, a.cu_bitmap_count, a.cu_slots(), a.num_se, a.num_sh_per_se, a.num_cu_per_sh));
            if let Some(s) = spec {
                let cu = a.cu_active_number;
                let (st, msg) = if cu == s.units {
                    (Status::Ok, "matches reference".to_string())
                } else if cu > s.units {
                    (Status::Warn, format!("MORE CUs than reference {} — unlocked (BIOS/fuse mod) or different SKU strap", s.units))
                } else {
                    (Status::Warn, format!("FEWER CUs than reference {} — extra harvesting, amdgpu.disable_cu, or fake/relabelled card", s.units))
                };
                add("Configuration", "Active compute units vs reference", st, format!("{cu} active ({} shaders) — {msg}", cu * 64));
                let rops = a.rops();
                add("Configuration", "ROPs vs reference", if rops == s.rops { Status::Ok } else { Status::Warn },
                    format!("{} RB → {} ROPs / reference {}", a.num_rb_pipes, rops, s.rops));
            }
            if let Some(dc) = a.module_params.iter().find(|(k, _)| k == "disable_cu") {
                if !dc.1.is_empty() && dc.1 != "(null)" {
                    add("Configuration", "amdgpu.disable_cu", Status::Warn, format!("CUs disabled by module parameter: {}", dc.1));
                }
            }
        }
        if let (Some(n), Some(s)) = (&self.nv, spec) {
            if let Some(cores) = n.cores {
                add("Configuration", "CUDA cores vs reference", if cores == s.shaders { Status::Ok } else { Status::Warn },
                    format!("NVML {cores} / reference {}", s.shaders));
            }
        }
        if let (Some(b), Some(s)) = (self.bus_width(), spec) {
            add("Configuration", "Memory bus width vs reference", if b == s.bus_width { Status::Ok } else { Status::Warn },
                format!("{b} bit / reference {} bit", s.bus_width));
        }
        if let (Some(m), Some(s)) = (self.mem_type(), spec) {
            add("Configuration", "Memory type vs reference", if m.eq_ignore_ascii_case(s.mem.name()) { Status::Ok } else { Status::Warn },
                format!("{m} / reference {}", s.mem.name()));
        }

        // ---- Memory size ----
        if let Some(a) = &self.amd {
            if let (Some(t), true) = (a.sysfs_vram_total, a.ioctl_vram_size > 0) {
                let reserved = t.saturating_sub(a.ioctl_vram_size);
                let ok = a.ioctl_vram_size <= t && reserved < (512 << 20);
                add("Memory", "Usable VRAM vs total (kernel reservation)", if ok { Status::Ok } else { Status::Warn },
                    format!("usable {} of {} — {} MB reserved by kernel/firmware", fmt_bytes(a.ioctl_vram_size), fmt_bytes(t), reserved >> 20));
            }
        }
        let srcs = self.vram_sources();
        if !srcs.is_empty() {
            let same = srcs.windows(2).all(|w| w[0].1 == w[1].1);
            add("Memory", "VRAM size agreement between driver interfaces", if same { Status::Ok } else { Status::Fail },
                srcs.iter().map(|(n, v)| format!("{n}: {}", fmt_bytes(*v))).collect::<Vec<_>>().join("; "));
        }
        if let Some(v) = self.vram_bytes() {
            let gb = v as f64 / (1u64 << 30) as f64;
            if let Some(s) = spec {
                let ok = s.size_matches(v);
                add("Memory", "VRAM size vs factory configurations", if ok { Status::Ok } else { Status::Warn },
                    format!("{gb:.1} GB / factory {}{}", s.sizes_str(),
                        if ok { "" } else { " — non-factory size: memory mod (chip swap + strap/BIOS) or spoofed size; run the memory test" }));
            }
            if let Some(b) = self.bus_width() {
                // Each 32-bit channel (or 16-bit pair in clamshell) holds 0.5/1/1.5/2/3/4 GB chips.
                let chans = (b / 32).max(1) as f64;
                let per = gb / chans;
                let ok = [0.25, 0.5, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0].iter().any(|d| (per - d).abs() < 0.05);
                add("Memory", "VRAM size plausible for bus width", if ok { Status::Ok } else { Status::Warn },
                    format!("{gb:.1} GB over {} channels = {per:.2} GB/channel", chans as u32));
            }
            if let Some(bar) = p.vram_bar() {
                if bar.size >= v {
                    add("Memory", "VRAM size vs PCI BAR aperture", Status::Ok,
                        format!("BAR{} {} ≥ VRAM (ReBAR exposes the full framebuffer)", bar.index, fmt_bytes(bar.size)));
                } else {
                    add("Memory", "VRAM size vs PCI BAR aperture", Status::Info,
                        format!("BAR{} {} (small aperture, no cross-check possible)", bar.index, fmt_bytes(bar.size)));
                }
            }
        }
        match mt {
            None => add("Memory", "Physical VRAM test", Status::Skipped, "run the Memory Test tab to verify real capacity and integrity".into()),
            Some(m) => {
                let st = if m.errors > 0 { Status::Fail } else if m.verified_capacity() { Status::Ok } else { Status::Warn };
                add("Memory", "Physical VRAM test", st, m.summary());
            }
        }

        // ---- Empirical compute ----
        match cr {
            None => add("Measured", "FP32 throughput vs claimed shaders", Status::Skipped, "run the compute benchmark".into()),
            Some(r) => add("Measured", "FP32 throughput vs claimed shaders",
                match r.verdict_ok { Some(true) => Status::Ok, Some(false) => Status::Warn, None => Status::Info }, r.summary()),
        }

        // ---- Driver ----
        if let Some(i) = &self.integrity {
            for f in &i.files {
                add("Driver", &format!("{} integrity", f.what), match f.ok {
                    Some(true) => Status::Ok, Some(false) => Status::Fail, None => Status::Info }, format!("{} — {}", f.path, f.detail));
            }
            if i.srcversion_disk.is_some() {
                let ok = i.srcversion_disk == i.srcversion_loaded;
                add("Driver", "Loaded module == module on disk", if ok { Status::Ok } else { Status::Warn },
                    format!("loaded {} / disk {}", i.srcversion_loaded.clone().unwrap_or_default(), i.srcversion_disk.clone().unwrap_or_default()));
            }
            add("Driver", "Module taint", if i.module_taint.is_none() { Status::Ok } else { Status::Warn },
                i.module_taint.clone().map(|t| format!("'{t}' (O = out-of-tree, E = unsigned, P = proprietary)")).unwrap_or("clean".into()));
            add("Driver", "Kernel taint", if i.kernel_taint_flags.iter().any(|f| f.starts_with('O') || f.starts_with('E') || f.starts_with('F') || f.starts_with('K')) { Status::Warn } else { Status::Ok },
                if i.kernel_taint_flags.is_empty() { "0 (clean)".into() } else { i.kernel_taint_flags.join("; ") });
            if let Some(s) = &i.signer {
                add("Driver", "Module signature", Status::Info, format!("signed by: {s}"));
            }
            add("Driver", "Library injection (LD_PRELOAD / ld.so.preload)", if i.preload.is_empty() { Status::Ok } else { Status::Fail },
                if i.preload.is_empty() { "none".into() } else { i.preload.join("; ") });
            if !i.cmdline_params.is_empty() {
                add("Driver", "GPU driver kernel parameters", Status::Info, i.cmdline_params.join(" "));
            }
        }
        if let Some(a) = &self.amd {
            if let Some((_, v)) = a.module_params.iter().find(|(k, _)| k == "ppfeaturemask") {
                let def = v == "0xfff7bfff" || v == "0xfff7bffb";
                add("Driver", "amdgpu.ppfeaturemask", if def { Status::Ok } else { Status::Info },
                    format!("{v}{}", if def { " (default)" } else { " — non-default: OverDrive / power features changed" }));
            }
        }

        // ---- Link ----
        if let (Some(w), Some(mw)) = (p.link_width, p.max_link_width) {
            add("Bus", "PCIe link width", if w == mw { Status::Ok } else { Status::Info },
                format!("x{w} of x{mw}{}", if w < mw { " (degraded or slot-limited)" } else { "" }));
        }
        c
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}
