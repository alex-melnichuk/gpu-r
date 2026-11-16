//! AMD backend: talks to the amdgpu kernel driver directly through
//! DRM_IOCTL_AMDGPU_INFO on the render node, plus sysfs / hwmon.
//!
//! `cu_bitmap` / `cu_active_number` are derived by the driver from the GC
//! harvest registers (CC_GC_SHADER_ARRAY_CONFIG = fuses, and
//! GC_USER_SHADER_ARRAY_CONFIG = VBIOS/driver programmed). So they show what
//! the silicon really has enabled, not what the marketing name says.

use crate::util::*;
use serde::Serialize;
use std::fs::File;
use std::path::{Path, PathBuf};

const DRM_IOCTL_AMDGPU_INFO: u64 = 0x4020_6445;
const DRM_IOCTL_VERSION: u64 = 0xC040_6400;

const AMDGPU_INFO_VRAM_GTT: u32 = 0x14;
const AMDGPU_INFO_DEV_INFO: u32 = 0x16;
const AMDGPU_INFO_MEMORY: u32 = 0x19;
const AMDGPU_INFO_VBIOS: u32 = 0x1B;
const AMDGPU_INFO_VBIOS_SIZE: u32 = 0x1;
const AMDGPU_INFO_VBIOS_IMAGE: u32 = 0x2;
const AMDGPU_INFO_VBIOS_INFO: u32 = 0x3;

#[repr(C)]
#[derive(Default)]
struct DrmAmdgpuInfo {
    return_pointer: u64,
    return_size: u32,
    query: u32,
    u0: u32,
    u1: u32,
    u2: u32,
    u3: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct DevInfo {
    device_id: u32,
    chip_rev: u32,
    external_rev: u32,
    pci_rev: u32,
    family: u32,
    num_shader_engines: u32,
    num_shader_arrays_per_engine: u32,
    gpu_counter_freq: u32,
    max_engine_clock: u64,
    max_memory_clock: u64,
    cu_active_number: u32,
    cu_ao_mask: u32,
    cu_bitmap: [[u32; 4]; 4],
    enabled_rb_pipes_mask: u32,
    num_rb_pipes: u32,
    num_hw_gfx_contexts: u32,
    pcie_gen: u32,
    ids_flags: u64,
    virtual_address_offset: u64,
    virtual_address_max: u64,
    virtual_address_alignment: u32,
    pte_fragment_size: u32,
    gart_page_size: u32,
    ce_ram_size: u32,
    vram_type: u32,
    vram_bit_width: u32,
    vce_harvest_config: u32,
    gc_double_offchip_lds_buf: u32,
    prim_buf_gpu_addr: u64,
    pos_buf_gpu_addr: u64,
    cntl_sb_buf_gpu_addr: u64,
    param_buf_gpu_addr: u64,
    prim_buf_size: u32,
    pos_buf_size: u32,
    cntl_sb_buf_size: u32,
    param_buf_size: u32,
    wave_front_size: u32,
    num_shader_visible_vgprs: u32,
    num_cu_per_sh: u32,
    num_tcc_blocks: u32,
    gs_vgt_table_depth: u32,
    gs_prim_buffer_depth: u32,
    max_gs_waves_per_vgt: u32,
    pcie_num_lanes: u32,
    cu_ao_bitmap: [[u32; 4]; 4],
    high_va_offset: u64,
    high_va_max: u64,
    pa_sc_tile_steering_override: u32,
    pad: u32,
    tcc_disabled_mask: u64,
    min_engine_clock: u64,
    min_memory_clock: u64,
    tcp_cache_size: u32,
    num_sqc_per_wgp: u32,
    sqc_data_cache_size: u32,
    sqc_inst_cache_size: u32,
    gl1c_cache_size: u32,
    gl2c_cache_size: u32,
    mall_size: u64,
    enabled_rb_pipes_mask_hi: u32,
    shadow_size: u32,
    shadow_alignment: u32,
    csa_size: u32,
    csa_alignment: u32,
    userq_ip_mask: u32,
    _reserve: [u32; 32],
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct HeapInfo {
    total_heap_size: u64,
    usable_heap_size: u64,
    heap_usage: u64,
    max_allocation: u64,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct MemoryInfo {
    vram: HeapInfo,
    cpu_accessible_vram: HeapInfo,
    gtt: HeapInfo,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct VramGtt {
    vram_size: u64,
    vram_cpu_accessible_size: u64,
    gtt_size: u64,
}

#[repr(C)]
struct VbiosInfoRaw {
    name: [u8; 64],
    vbios_pn: [u8; 64],
    version: u32,
    pad: u32,
    vbios_ver_str: [u8; 32],
    date: [u8; 32],
}

#[repr(C)]
struct DrmVersion {
    major: i32,
    minor: i32,
    patch: i32,
    name_len: usize,
    name: *mut u8,
    date_len: usize,
    date: *mut u8,
    desc_len: usize,
    desc: *mut u8,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct AmdInfo {
    pub render_node: String,
    pub ioctl_ok: bool,
    pub drm_version: Option<String>,
    // ioctl DEV_INFO
    pub device_id: u32,
    pub pci_rev: u32,
    pub chip_rev: u32,
    pub external_rev: u32,
    pub family: u32,
    pub num_se: u32,
    pub num_sh_per_se: u32,
    pub num_cu_per_sh: u32,
    pub cu_active_number: u32,
    /// Popcount of cu_bitmap (cross-check of cu_active_number).
    pub cu_bitmap_count: u32,
    pub cu_bitmap: Vec<Vec<u32>>,
    pub num_rb_pipes: u32,
    pub enabled_rb_mask: u64,
    pub vram_type: u32,
    pub vram_bit_width: u32,
    pub max_engine_clock_khz: u64,
    pub max_memory_clock_khz: u64,
    pub min_engine_clock_khz: u64,
    pub min_memory_clock_khz: u64,
    pub wave_size: u32,
    pub gl2_cache: u32,
    pub mall_size: u64,
    pub pcie_gen: u32,
    pub pcie_lanes: u32,
    // memory
    pub ioctl_vram_size: u64,
    pub ioctl_vram_cpu_visible: u64,
    pub ioctl_heap_vram_total: u64,
    pub ioctl_heap_vram_usable: u64,
    pub ioctl_gtt_size: u64,
    pub sysfs_vram_total: Option<u64>,
    pub sysfs_vis_vram_total: Option<u64>,
    pub mem_vendor: Option<String>,
    // vbios through the driver
    pub vbios_name: Option<String>,
    pub vbios_pn: Option<String>,
    pub vbios_ver_str: Option<String>,
    pub vbios_date: Option<String>,
    #[serde(skip)]
    pub vbios_image: Option<Vec<u8>>,
    pub sysfs_vbios_version: Option<String>,
    // IP discovery
    pub gc_ip: Option<(u8, u8, u8)>,
    pub gc_harvest: Option<String>,
    // power play
    #[serde(skip)]
    pub pp_table: Option<Vec<u8>>,
    pub sclk_levels: Vec<u32>,
    pub mclk_levels: Vec<u32>,
    pub fw_versions: Vec<(String, String)>,
    pub module_params: Vec<(String, String)>,
}

impl AmdInfo {
    pub fn vram_type_name(&self) -> &'static str {
        match self.vram_type {
            1 => "GDDR1",
            2 => "DDR2",
            3 => "GDDR3",
            4 => "GDDR4",
            5 => "GDDR5",
            6 => "HBM",
            7 => "DDR3",
            8 => "DDR4",
            9 => "GDDR6",
            10 => "DDR5",
            11 => "LPDDR4",
            12 => "LPDDR5",
            13 => "HBM3E",
            14 => "HBM4",
            _ => "Unknown",
        }
    }

    /// Physical shader array CU capacity (all SE*SH*CU slots).
    pub fn cu_slots(&self) -> u32 {
        self.num_se * self.num_sh_per_se * self.num_cu_per_sh
    }

    /// ROPs = render backends * pixels per RB (RB+ on gfx10.3+ handles 8).
    pub fn rops(&self) -> u32 {
        let per_rb = match self.gc_ip {
            Some((maj, min, _)) if maj > 10 || (maj == 10 && min >= 3) => 8,
            _ => 4,
        };
        self.num_rb_pipes * per_rb
    }

    /// Effective data-rate multiplier relative to the driver-reported MCLK.
    pub fn mem_rate_mult(&self) -> f64 {
        match self.vram_type {
            9 => 16.0, // GDDR6: pp_dpm_mclk reports 1/8 WCK, data rate 16x
            5 => 4.0,
            6 | 13 | 14 => 2.0,
            _ => 2.0,
        }
    }
}

fn info_ioctl(fd: i32, query: u32, extra: [u32; 4], out: &mut [u8]) -> bool {
    let mut req = DrmAmdgpuInfo {
        return_pointer: out.as_mut_ptr() as u64,
        return_size: out.len() as u32,
        query,
        u0: extra[0],
        u1: extra[1],
        u2: extra[2],
        u3: extra[3],
    };
    sys_ioctl(fd, DRM_IOCTL_AMDGPU_INFO, &mut req as *mut _ as *mut u8)
}

fn as_bytes_mut<T>(v: &mut T) -> &mut [u8] {
    unsafe { std::slice::from_raw_parts_mut(v as *mut T as *mut u8, std::mem::size_of::<T>()) }
}

pub fn render_node_for(pci_sysfs: &Path) -> Option<PathBuf> {
    let drm = pci_sysfs.join("drm");
    let rd = std::fs::read_dir(drm).ok()?;
    for e in rd.flatten() {
        let n = e.file_name().to_string_lossy().to_string();
        if n.starts_with("renderD") {
            return Some(PathBuf::from("/dev/dri").join(n));
        }
    }
    None
}

pub fn card_node_for(pci_sysfs: &Path) -> Option<String> {
    let rd = std::fs::read_dir(pci_sysfs.join("drm")).ok()?;
    for e in rd.flatten() {
        let n = e.file_name().to_string_lossy().to_string();
        if n.starts_with("card") {
            return Some(n);
        }
    }
    None
}

pub fn hwmon_dir(pci_sysfs: &Path) -> Option<PathBuf> {
    std::fs::read_dir(pci_sysfs.join("hwmon"))
        .ok()?
        .flatten()
        .next()
        .map(|e| e.path())
}

fn drm_version(fd: i32) -> Option<String> {
    let mut name = [0u8; 64];
    let mut date = [0u8; 64];
    let mut desc = [0u8; 128];
    let mut v = DrmVersion {
        major: 0,
        minor: 0,
        patch: 0,
        name_len: name.len(),
        name: name.as_mut_ptr(),
        date_len: date.len(),
        date: date.as_mut_ptr(),
        desc_len: desc.len(),
        desc: desc.as_mut_ptr(),
    };
    if !sys_ioctl(fd, DRM_IOCTL_VERSION, &mut v as *mut _ as *mut u8) {
        return None;
    }
    Some(format!("{} {}.{}.{}", cstr(&name), v.major, v.minor, v.patch))
}

fn parse_levels(s: Option<String>) -> Vec<u32> {
    s.map(|s| {
        s.lines()
            .filter_map(|l| {
                let v = l.split(':').nth(1)?.trim();
                let num: String = v.chars().take_while(|c| c.is_ascii_digit()).collect();
                num.parse().ok()
            })
            .collect()
    })
    .unwrap_or_default()
}

pub fn collect(pci_sysfs: &Path) -> AmdInfo {
    let mut a = AmdInfo::default();
    let dev = pci_sysfs;

    a.sysfs_vram_total = read_num(dev.join("mem_info_vram_total"));
    a.sysfs_vis_vram_total = read_num(dev.join("mem_info_vis_vram_total"));
    a.mem_vendor = read_str(dev.join("mem_info_vram_vendor"));
    a.sysfs_vbios_version = read_str(dev.join("vbios_version"));
    a.pp_table = read_bytes(dev.join("pp_table")).filter(|b| !b.is_empty());
    a.sclk_levels = parse_levels(read_str(dev.join("pp_dpm_sclk")));
    a.mclk_levels = parse_levels(read_str(dev.join("pp_dpm_mclk")));

    // IP discovery: GC block version + harvest info.
    let gc = dev.join("ip_discovery/die/0/GC/0");
    if let (Some(ma), Some(mi), Some(rv)) = (
        read_num(gc.join("major")),
        read_num(gc.join("minor")),
        read_num(gc.join("revision")),
    ) {
        a.gc_ip = Some((ma as u8, mi as u8, rv as u8));
        a.gc_harvest = read_str(gc.join("harvest"));
    }

    if let Ok(rd) = std::fs::read_dir(dev.join("fw_version")) {
        let mut v: Vec<_> = rd
            .flatten()
            .filter_map(|e| {
                let n = e.file_name().to_string_lossy().replace("_fw_version", "");
                read_str(e.path()).map(|val| (n, val))
            })
            .collect();
        v.sort();
        a.fw_versions = v;
    }
    for p in ["disable_cu", "ppfeaturemask", "vramlimit", "vis_vramlimit", "gttsize", "virtual_display", "dpm", "aspm"] {
        if let Some(v) = read_str(format!("/sys/module/amdgpu/parameters/{p}")) {
            a.module_params.push((p.to_string(), v));
        }
    }

    let Some(node) = render_node_for(dev) else { return a };
    a.render_node = node.display().to_string();
    let Ok(f) = File::open(&node) else { return a };
    let fd = raw_fd(&f);
    a.drm_version = drm_version(fd);

    let mut di: DevInfo = unsafe { std::mem::zeroed() };
    if info_ioctl(fd, AMDGPU_INFO_DEV_INFO, [0; 4], as_bytes_mut(&mut di)) {
        a.ioctl_ok = true;
        a.device_id = di.device_id;
        a.pci_rev = di.pci_rev;
        a.chip_rev = di.chip_rev;
        a.external_rev = di.external_rev;
        a.family = di.family;
        a.num_se = di.num_shader_engines;
        a.num_sh_per_se = di.num_shader_arrays_per_engine;
        a.num_cu_per_sh = di.num_cu_per_sh;
        a.cu_active_number = di.cu_active_number;
        a.cu_bitmap = di.cu_bitmap.iter().map(|r| r.to_vec()).collect();
        let (nse, nsh) = (di.num_shader_engines.min(4) as usize, di.num_shader_arrays_per_engine.min(4) as usize);
        a.cu_bitmap_count = (0..nse)
            .flat_map(|s| (0..nsh).map(move |h| (s, h)))
            .map(|(s, h)| di.cu_bitmap[s][h].count_ones())
            .sum();
        // gfx11+ with >4 SE folds extra SEs into cu_bitmap[i][j+2]; fall back to whole array.
        if a.cu_bitmap_count == 0 || di.num_shader_engines > 4 {
            a.cu_bitmap_count = di.cu_bitmap.iter().flatten().map(|x| x.count_ones()).sum();
        }
        a.num_rb_pipes = di.num_rb_pipes;
        a.enabled_rb_mask = di.enabled_rb_pipes_mask as u64 | ((di.enabled_rb_pipes_mask_hi as u64) << 32);
        a.vram_type = di.vram_type;
        a.vram_bit_width = di.vram_bit_width;
        a.max_engine_clock_khz = di.max_engine_clock;
        a.max_memory_clock_khz = di.max_memory_clock;
        a.min_engine_clock_khz = di.min_engine_clock;
        a.min_memory_clock_khz = di.min_memory_clock;
        a.wave_size = di.wave_front_size;
        a.gl2_cache = di.gl2c_cache_size;
        a.mall_size = di.mall_size;
        a.pcie_gen = di.pcie_gen;
        a.pcie_lanes = di.pcie_num_lanes;
    }

    let mut vg = VramGtt::default();
    if info_ioctl(fd, AMDGPU_INFO_VRAM_GTT, [0; 4], as_bytes_mut(&mut vg)) {
        a.ioctl_vram_size = vg.vram_size;
        a.ioctl_vram_cpu_visible = vg.vram_cpu_accessible_size;
        a.ioctl_gtt_size = vg.gtt_size;
    }
    let mut mi = MemoryInfo::default();
    if info_ioctl(fd, AMDGPU_INFO_MEMORY, [0; 4], as_bytes_mut(&mut mi)) {
        a.ioctl_heap_vram_total = mi.vram.total_heap_size;
        a.ioctl_heap_vram_usable = mi.vram.usable_heap_size;
    }

    let mut vi: VbiosInfoRaw = unsafe { std::mem::zeroed() };
    if info_ioctl(fd, AMDGPU_INFO_VBIOS, [AMDGPU_INFO_VBIOS_INFO, 0, 0, 0], as_bytes_mut(&mut vi)) {
        a.vbios_name = Some(cstr(&vi.name)).filter(|s| !s.is_empty());
        a.vbios_pn = Some(cstr(&vi.vbios_pn)).filter(|s| !s.is_empty());
        a.vbios_ver_str = Some(cstr(&vi.vbios_ver_str)).filter(|s| !s.is_empty());
        a.vbios_date = Some(cstr(&vi.date)).filter(|s| !s.is_empty());
    }
    let mut size: u32 = 0;
    if info_ioctl(fd, AMDGPU_INFO_VBIOS, [AMDGPU_INFO_VBIOS_SIZE, 0, 0, 0], as_bytes_mut(&mut size)) && size > 0 {
        let mut img = vec![0u8; size as usize];
        if info_ioctl(fd, AMDGPU_INFO_VBIOS, [AMDGPU_INFO_VBIOS_IMAGE, 0, 0, 0], &mut img) {
            a.vbios_image = Some(img);
        }
    }
    a
}

/// Live sensor readout through hwmon + sysfs (cheap, called every sample).
pub fn read_sensors(pci_sysfs: &Path, hwmon: Option<&Path>, out: &mut Vec<(String, String, f64)>) {
    let mut push = |n: &str, u: &str, v: Option<f64>| {
        if let Some(v) = v {
            out.push((n.to_string(), u.to_string(), v));
        }
    };
    if let Some(h) = hwmon {
        for i in 1..=4 {
            let label = read_str(h.join(format!("freq{i}_label")));
            if let Some(l) = label {
                let name = match l.as_str() {
                    "sclk" => "GPU Clock",
                    "mclk" => "Memory Clock",
                    _ => continue,
                };
                push(name, "MHz", read_num(h.join(format!("freq{i}_input"))).map(|x| x as f64 / 1e6));
            }
        }
        for i in 1..=4 {
            if let Some(l) = read_str(h.join(format!("temp{i}_label"))) {
                let name = match l.as_str() {
                    "edge" => "GPU Temperature",
                    "junction" => "Hot Spot",
                    "mem" => "Memory Temperature",
                    _ => continue,
                };
                push(name, "°C", read_num(h.join(format!("temp{i}_input"))).map(|x| x as f64 / 1000.0));
            }
        }
        push("Fan Speed", "RPM", read_num(h.join("fan1_input")).map(|x| x as f64));
        if let (Some(p), Some(m)) = (read_num(h.join("pwm1")), read_num(h.join("pwm1_max"))) {
            if m > 0 {
                push("Fan Speed (%)", "%", Some(p as f64 * 100.0 / m as f64));
            }
        }
        let pw = read_num(h.join("power1_average")).or_else(|| read_num(h.join("power1_input")));
        push("Board Power", "W", pw.map(|x| x as f64 / 1e6));
        push("Power Limit", "W", read_num(h.join("power1_cap")).map(|x| x as f64 / 1e6));
        push("GPU Voltage", "V", read_num(h.join("in0_input")).map(|x| x as f64 / 1000.0));
    }
    push("GPU Load", "%", read_num(pci_sysfs.join("gpu_busy_percent")).map(|x| x as f64));
    push("Memory Controller Load", "%", read_num(pci_sysfs.join("mem_busy_percent")).map(|x| x as f64));
    push("Memory Used", "MB", read_num(pci_sysfs.join("mem_info_vram_used")).map(|x| x as f64 / 1048576.0));
    push("GTT Used", "MB", read_num(pci_sysfs.join("mem_info_gtt_used")).map(|x| x as f64 / 1048576.0));
}

pub fn vram_used(pci_sysfs: &Path) -> Option<u64> {
    read_num(pci_sysfs.join("mem_info_vram_used"))
}

#[cfg(unix)]
fn raw_fd(f: &std::fs::File) -> i32 {
    use std::os::fd::AsRawFd;
    f.as_raw_fd()
}
#[cfg(not(unix))]
fn raw_fd(_: &std::fs::File) -> i32 {
    -1
}

#[cfg(unix)]
fn sys_ioctl(fd: i32, req: u64, arg: *mut u8) -> bool {
    unsafe { libc::ioctl(fd, req as _, arg) == 0 }
}
#[cfg(not(unix))]
fn sys_ioctl(_: i32, _: u64, _: *mut u8) -> bool {
    false
}
