//! NVIDIA backend via NVML (libnvidia-ml.so.1), loaded dynamically so the
//! program runs on machines without the NVIDIA driver.

use libloading::{Library, Symbol};
use serde::Serialize;
use std::ffi::{c_char, c_int, c_uint, c_void};
use std::sync::OnceLock;

type Dev = *mut c_void;
type Ret = c_int;

#[repr(C)]
struct PciInfo {
    bus_id_legacy: [c_char; 16],
    domain: c_uint,
    bus: c_uint,
    device: c_uint,
    pci_device_id: c_uint,
    pci_subsystem_id: c_uint,
    bus_id: [c_char; 32],
}

#[repr(C)]
#[derive(Default)]
struct Memory {
    total: u64,
    free: u64,
    used: u64,
}

#[repr(C)]
#[derive(Default)]
struct Utilization {
    gpu: c_uint,
    memory: c_uint,
}

pub struct Nvml {
    lib: Library,
}

unsafe impl Send for Nvml {}
unsafe impl Sync for Nvml {}

static NVML: OnceLock<Option<Nvml>> = OnceLock::new();

pub fn nvml() -> Option<&'static Nvml> {
    NVML.get_or_init(|| unsafe {
        let names: &[&str] = if cfg!(windows) {
            &["nvml.dll", r"C:\Program Files\NVIDIA Corporation\NVSMI\nvml.dll"]
        } else {
            &["libnvidia-ml.so.1", "libnvidia-ml.so"]
        };
        let lib = names.iter().find_map(|n| Library::new(*n).ok())?;
        let init: Symbol<unsafe extern "C" fn() -> Ret> = lib.get(b"nvmlInit_v2\0").ok()?;
        if init() != 0 {
            return None;
        }
        Some(Nvml { lib })
    })
    .as_ref()
}

macro_rules! sym {
    ($s:expr, $name:literal, $t:ty) => {
        unsafe { $s.lib.get::<$t>(concat!($name, "\0").as_bytes()).ok() }
    };
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct NvInfo {
    pub index: u32,
    pub name: Option<String>,
    pub pci_device_id: Option<u32>,
    pub pci_subsystem_id: Option<u32>,
    pub bus_id: Option<String>,
    pub mem_total: Option<u64>,
    pub bar1_total: Option<u64>,
    pub bus_width: Option<u32>,
    pub cores: Option<u32>,
    pub vbios: Option<String>,
    pub driver_version: Option<String>,
    pub nvml_version: Option<String>,
    pub cuda_cc: Option<(u32, u32)>,
    pub max_gpu_clock: Option<u32>,
    pub max_mem_clock: Option<u32>,
    pub default_app_gpu_clock: Option<u32>,
    pub pcie_gen_max: Option<u32>,
    pub pcie_width_max: Option<u32>,
    pub architecture: Option<u32>,
}

impl Nvml {
    fn handle_by_bus(&self, bus_id: &str) -> Option<Dev> {
        let f = sym!(self, "nvmlDeviceGetHandleByPciBusId_v2", unsafe extern "C" fn(*const c_char, *mut Dev) -> Ret)?;
        let c = std::ffi::CString::new(bus_id).ok()?;
        let mut d: Dev = std::ptr::null_mut();
        (unsafe { f(c.as_ptr(), &mut d) } == 0).then_some(d)
    }

    fn get_u32(&self, d: Dev, name: &str) -> Option<u32> {
        let n = format!("{name}\0");
        let f = unsafe { self.lib.get::<unsafe extern "C" fn(Dev, *mut c_uint) -> Ret>(n.as_bytes()).ok()? };
        let mut v: c_uint = 0;
        (unsafe { f(d, &mut v) } == 0).then_some(v)
    }

    fn get_u32_arg(&self, d: Dev, name: &str, arg: c_uint) -> Option<u32> {
        let n = format!("{name}\0");
        let f = unsafe { self.lib.get::<unsafe extern "C" fn(Dev, c_uint, *mut c_uint) -> Ret>(n.as_bytes()).ok()? };
        let mut v: c_uint = 0;
        (unsafe { f(d, arg, &mut v) } == 0).then_some(v)
    }

    fn get_str(&self, d: Dev, name: &str) -> Option<String> {
        let n = format!("{name}\0");
        let f = unsafe { self.lib.get::<unsafe extern "C" fn(Dev, *mut c_char, c_uint) -> Ret>(n.as_bytes()).ok()? };
        let mut buf = [0 as c_char; 128];
        if unsafe { f(d, buf.as_mut_ptr(), buf.len() as c_uint) } != 0 {
            return None;
        }
        Some(unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().to_string())
    }

    fn sys_str(&self, name: &str) -> Option<String> {
        let n = format!("{name}\0");
        let f = unsafe { self.lib.get::<unsafe extern "C" fn(*mut c_char, c_uint) -> Ret>(n.as_bytes()).ok()? };
        let mut buf = [0 as c_char; 128];
        if unsafe { f(buf.as_mut_ptr(), buf.len() as c_uint) } != 0 {
            return None;
        }
        Some(unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().to_string())
    }

    pub fn info(&self, pci_addr: &str) -> Option<NvInfo> {
        let d = self.handle_by_bus(pci_addr)?;
        let mut n = NvInfo {
            name: self.get_str(d, "nvmlDeviceGetName"),
            vbios: self.get_str(d, "nvmlDeviceGetVbiosVersion"),
            driver_version: self.sys_str("nvmlSystemGetDriverVersion"),
            nvml_version: self.sys_str("nvmlSystemGetNVMLVersion"),
            bus_width: self.get_u32(d, "nvmlDeviceGetMemoryBusWidth"),
            cores: self.get_u32(d, "nvmlDeviceGetNumGpuCores"),
            architecture: self.get_u32(d, "nvmlDeviceGetArchitecture"),
            max_gpu_clock: self.get_u32_arg(d, "nvmlDeviceGetMaxClockInfo", 0),
            max_mem_clock: self.get_u32_arg(d, "nvmlDeviceGetMaxClockInfo", 2),
            default_app_gpu_clock: self.get_u32_arg(d, "nvmlDeviceGetDefaultApplicationsClock", 0),
            pcie_gen_max: self.get_u32(d, "nvmlDeviceGetMaxPcieLinkGeneration"),
            pcie_width_max: self.get_u32(d, "nvmlDeviceGetMaxPcieLinkWidth"),
            ..Default::default()
        };
        if let Some(f) = sym!(self, "nvmlDeviceGetPciInfo_v3", unsafe extern "C" fn(Dev, *mut PciInfo) -> Ret) {
            let mut p: PciInfo = unsafe { std::mem::zeroed() };
            if unsafe { f(d, &mut p) } == 0 {
                n.pci_device_id = Some(p.pci_device_id);
                n.pci_subsystem_id = Some(p.pci_subsystem_id);
                n.bus_id = Some(unsafe { std::ffi::CStr::from_ptr(p.bus_id.as_ptr()) }.to_string_lossy().to_string());
            }
        }
        if let Some(f) = sym!(self, "nvmlDeviceGetMemoryInfo", unsafe extern "C" fn(Dev, *mut Memory) -> Ret) {
            let mut m = Memory::default();
            if unsafe { f(d, &mut m) } == 0 {
                n.mem_total = Some(m.total);
            }
        }
        if let Some(f) = sym!(self, "nvmlDeviceGetBAR1MemoryInfo", unsafe extern "C" fn(Dev, *mut Memory) -> Ret) {
            let mut m = Memory::default();
            if unsafe { f(d, &mut m) } == 0 {
                n.bar1_total = Some(m.total);
            }
        }
        if let Some(f) = sym!(self, "nvmlDeviceGetCudaComputeCapability", unsafe extern "C" fn(Dev, *mut c_int, *mut c_int) -> Ret) {
            let (mut a, mut b) = (0, 0);
            if unsafe { f(d, &mut a, &mut b) } == 0 {
                n.cuda_cc = Some((a as u32, b as u32));
            }
        }
        Some(n)
    }

    pub fn read_sensors(&self, pci_addr: &str, out: &mut Vec<(String, String, f64)>) {
        let Some(d) = self.handle_by_bus(pci_addr) else { return };
        let mut push = |n: &str, u: &str, v: Option<f64>| {
            if let Some(v) = v {
                out.push((n.to_string(), u.to_string(), v));
            }
        };
        push("GPU Clock", "MHz", self.get_u32_arg(d, "nvmlDeviceGetClockInfo", 0).map(f64::from));
        push("Memory Clock", "MHz", self.get_u32_arg(d, "nvmlDeviceGetClockInfo", 2).map(f64::from));
        push("GPU Temperature", "°C", self.get_u32_arg(d, "nvmlDeviceGetTemperature", 0).map(f64::from));
        push("Fan Speed (%)", "%", self.get_u32(d, "nvmlDeviceGetFanSpeed").map(f64::from));
        push("Board Power", "W", self.get_u32(d, "nvmlDeviceGetPowerUsage").map(|x| x as f64 / 1000.0));
        push("Power Limit", "W", self.get_u32(d, "nvmlDeviceGetEnforcedPowerLimit").map(|x| x as f64 / 1000.0));
        if let Some(f) = sym!(self, "nvmlDeviceGetUtilizationRates", unsafe extern "C" fn(Dev, *mut Utilization) -> Ret) {
            let mut u = Utilization::default();
            if unsafe { f(d, &mut u) } == 0 {
                push("GPU Load", "%", Some(u.gpu as f64));
                push("Memory Controller Load", "%", Some(u.memory as f64));
            }
        }
        if let Some(f) = sym!(self, "nvmlDeviceGetMemoryInfo", unsafe extern "C" fn(Dev, *mut Memory) -> Ret) {
            let mut m = Memory::default();
            if unsafe { f(d, &mut m) } == 0 {
                push("Memory Used", "MB", Some(m.used as f64 / 1048576.0));
            }
        }
    }

    pub fn mem_used(&self, pci_addr: &str) -> Option<u64> {
        let d = self.handle_by_bus(pci_addr)?;
        let f = sym!(self, "nvmlDeviceGetMemoryInfo", unsafe extern "C" fn(Dev, *mut Memory) -> Ret)?;
        let mut m = Memory::default();
        (unsafe { f(d, &mut m) } == 0).then_some(m.used)
    }
}
