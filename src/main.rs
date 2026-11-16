// GUI app on Windows: no console window; CLI mode re-attaches to the parent console.
#![cfg_attr(windows, allow(dead_code))]
#![cfg_attr(windows, windows_subsystem = "windows")]

mod amd;
mod db;
mod gpu;
mod gputest;
mod gui;
mod i18n;
mod integrity;
mod nvidia;
mod pci;
mod sensors;
mod util;
mod vbios;
#[cfg(windows)]
mod win;

use gpu::{Gpu, Status};
use std::sync::atomic::AtomicBool;

const HELP: &str = "GPU-R — GPU information & verification tool

USAGE:
  gpu-r                 start the GUI
  gpu-r --report        print full report (info + validation) to stdout
  gpu-r --json          print JSON report
  gpu-r --memtest [F]   run VRAM test over fraction F (default 0.9) of free VRAM
  gpu-r --stress [F]    VRAM + core stress test, loops until Ctrl+C, an error or a GPU hang
  gpu-r --compute       run FP32 throughput test (verifies shader count)
  gpu-r --sensors [N]   print N sensor samples (default 5)
  gpu-r --gpu I         select GPU index (default 0)
  gpu-r --dump-rom FILE save VBIOS image (driver copy, or physical ROM as root)

Run as root for physical ROM read and full PCI config space.";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        gui::run();
        return;
    }
    #[cfg(windows)]
    attach_console();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{HELP}");
        return;
    }
    let idx: usize = arg_val(&args, "--gpu").and_then(|v| v.parse().ok()).unwrap_or(0);
    let gpus = gpu::collect_all();
    if gpus.is_empty() {
        eprintln!("No display-class PCI devices found");
        std::process::exit(1);
    }
    let Some(g) = gpus.get(idx) else {
        eprintln!("GPU index {idx} out of range ({} found)", gpus.len());
        std::process::exit(1);
    };

    let mut mt = None;
    let mut cr = None;
    if args.iter().any(|a| a == "--memtest") {
        let frac = arg_val(&args, "--memtest").and_then(|v| v.parse().ok()).unwrap_or(0.9);
        mt = Some(cli_memtest(g, frac, false));
    }
    if args.iter().any(|a| a == "--stress") {
        let frac = arg_val(&args, "--stress").and_then(|v| v.parse().ok()).unwrap_or(0.9);
        mt = Some(cli_memtest(g, frac, true));
    }
    if args.iter().any(|a| a == "--compute") {
        cr = Some(cli_compute(g));
    }
    if let Some(n) = args.iter().position(|a| a == "--sensors") {
        let cnt: usize = args.get(n + 1).and_then(|v| v.parse().ok()).unwrap_or(5);
        let src = source_of(g);
        for i in 0..cnt {
            let s = src.read();
            println!("--- sample {i}");
            for (n, u, v) in s {
                println!("  {n:<26} {v:>10.1} {u}");
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }
    if let Some(f) = arg_val(&args, "--dump-rom") {
        let img = g.amd.as_ref().and_then(|a| a.vbios_image.clone());
        match img {
            Some(i) => {
                std::fs::write(&f, &i).expect("write");
                println!("saved {} bytes (driver copy) to {f}", i.len());
            }
            None => eprintln!("no VBIOS image available (try as root)"),
        }
    }
    if args.iter().any(|a| a == "--json") {
        let v = serde_json::json!({
            "gpus": gpus.iter().map(|g| serde_json::json!({
                "gpu": g,
                "fields": g.fields(mt.as_ref(), cr.as_ref()),
                "checks": g.checks(mt.as_ref(), cr.as_ref()),
            })).collect::<Vec<_>>(),
            "memtest": mt, "compute": cr,
        });
        println!("{}", serde_json::to_string_pretty(&v).unwrap());
        return;
    }
    if args.iter().any(|a| a == "--report" || a == "--memtest" || a == "--stress" || a == "--compute") {
        print_report(g, mt.as_ref(), cr.as_ref());
    }
}

#[cfg(windows)]
fn attach_console() {
    unsafe extern "system" {
        fn AttachConsole(pid: u32) -> i32;
    }
    unsafe {
        AttachConsole(u32::MAX); // ATTACH_PARENT_PROCESS
    }
}

fn arg_val(args: &[String], k: &str) -> Option<String> {
    let i = args.iter().position(|a| a == k)?;
    args.get(i + 1).filter(|v| !v.starts_with("--")).cloned()
}

pub fn source_of(g: &Gpu) -> sensors::Source {
    sensors::Source {
        pci_addr: g.pci.addr.clone(),
        sysfs: g.pci.sysfs.clone(),
        driver: g.pci.driver.clone().unwrap_or_default(),
    }
}

/// Short "temperature / clock / power" line for stress logs.
pub fn sensors_line(src: &sensors::Source) -> String {
    let s = src.read();
    let pick = |n: &str| s.iter().find(|x| x.0 == n).map(|x| x.2);
    let mut v = vec![];
    if let Some(t) = pick("GPU Temperature") { v.push(format!("GPU {t:.0}°C")); }
    if let Some(t) = pick("Hot Spot") { v.push(format!("hotspot {t:.0}°C")); }
    if let Some(t) = pick("Memory Temperature") { v.push(format!("mem {t:.0}°C")); }
    if let Some(c) = pick("GPU Clock") { v.push(format!("{c:.0} MHz")); }
    if let Some(p) = pick("Board Power") { v.push(format!("{p:.0} W")); }
    v.join(", ")
}

fn spawn_printer(p: gputest::Shared) -> std::sync::Arc<AtomicBool> {
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let s2 = stop.clone();
    std::thread::spawn(move || {
        let mut printed = 0;
        let mut last_phase = String::new();
        while !s2.load(std::sync::atomic::Ordering::Relaxed) {
            {
                let g = p.lock().unwrap();
                for l in &g.log[printed..] {
                    eprintln!("{l}");
                }
                printed = g.log.len();
                if g.phase != last_phase {
                    eprintln!("[{:>3.0}%] {}", g.frac * 100.0, g.phase);
                    last_phase = g.phase.clone();
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    });
    stop
}

fn cli_memtest(g: &Gpu, frac: f64, stress: bool) -> gputest::MemTestResult {
    let ctx = gputest::open(g.pci.vendor, g.pci.device).unwrap_or_else(|e| {
        eprintln!("memtest: {e}");
        std::process::exit(2)
    });
    let src = source_of(g);
    let prog = gputest::Shared::default();
    let stop = spawn_printer(prog.clone());
    let opt = gputest::MemTestOptions {
        reported_vram: g.vram_bytes().unwrap_or(0),
        used_vram: src.vram_used(),
        fraction: frac,
        passes: 1,
        patterns: vec![0, 1, 2, 3, 4],
        stress,
        continue_on_error: false,
        heat: stress,
        log_path: stress.then(|| gui::home_dir().join(format!("gpu-r-stress-{}.log",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)))),
    };
    if let Some(p) = &opt.log_path {
        eprintln!("stress test: runs until Ctrl+C, an error or a GPU hang; log: {}", p.display());
    }
    let r = gputest::run_memtest(&ctx, &opt, &|| src.vram_used(), &|| sensors_line(&src), &prog, &AtomicBool::new(false));
    std::thread::sleep(std::time::Duration::from_millis(200));
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    r
}

fn cli_compute(g: &Gpu) -> gputest::ComputeResult {
    let ctx = gputest::open(g.pci.vendor, g.pci.device).unwrap_or_else(|e| {
        eprintln!("compute: {e}");
        std::process::exit(2)
    });
    let src = source_of(g);
    let prog = gputest::Shared::default();
    let r = gputest::run_compute(&ctx, g.shaders().or(g.db.as_ref().map(|d| d.spec.shaders)), &|| src.gpu_clock(), &prog);
    r
}

fn print_report(g: &Gpu, mt: Option<&gputest::MemTestResult>, cr: Option<&gputest::ComputeResult>) {
    println!("=== {} [{}] driver={} ===", g.title(), g.pci.addr, g.pci.driver.clone().unwrap_or("-".into()));
    for (k, f) in g.fields(mt, cr) {
        println!("{:<18} {:<5} {:<55} ({})", k, f.status.tag(), f.value, f.sources);
    }
    println!();
    println!("=== Validation ===");
    let checks = g.checks(mt, cr);
    let mut group = "";
    for c in &checks {
        if c.group != group {
            group = c.group;
            println!("-- {group}");
        }
        println!("  [{:<4}] {:<48} {}", c.status.tag(), c.title, c.detail);
    }
    let fails = checks.iter().filter(|c| c.status == Status::Fail).count();
    let warns = checks.iter().filter(|c| c.status == Status::Warn).count();
    let skipped = checks.iter().filter(|c| c.status == Status::Skipped).count();
    println!();
    println!(
        "Verdict: {}  ({} fail, {} warn, {} skipped)",
        if fails > 0 { "NOT GENUINE / FAULTY" } else if warns > 0 { "MODIFIED or non-reference — review warnings" } else if skipped > 0 { "consistent (some checks skipped)" } else { "GENUINE — all checks passed" },
        fails, warns, skipped
    );
    if let Some(a) = &g.amd {
        println!();
        println!("=== Advanced (amdgpu) ===");
        println!("family {} chip_rev {} ext_rev {} | SE {} SH/SE {} CU/SH {} | RB {} mask {:#x} | wave{} | MALL (Infinity Cache) {} MB",
            a.family, a.chip_rev, a.external_rev, a.num_se, a.num_sh_per_se, a.num_cu_per_sh, a.num_rb_pipes, a.enabled_rb_mask,
            a.wave_size, a.mall_size >> 20);
        for (se, row) in a.cu_bitmap.iter().enumerate().take(a.num_se.min(4) as usize) {
            let s: Vec<String> = row.iter().take(a.num_sh_per_se as usize).map(|m| format!("{:0w$b}", m, w = a.num_cu_per_sh as usize)).collect();
            println!("  SE{se} CU bitmap: {}", s.join(" "));
        }
        println!("  VRAM: usable {} MB, CPU-visible {} MB, GTT {} MB", a.ioctl_vram_size >> 20, a.ioctl_vram_cpu_visible >> 20, a.ioctl_gtt_size >> 20);
        println!("  VBIOS (driver): {:?} pn {:?} ver {:?} date {:?}", a.vbios_name, a.vbios_pn, a.vbios_ver_str, a.vbios_date);
        println!("  firmware: {}", a.fw_versions.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(" "));
    }
    if let Some(v) = g.vbios() {
        println!("  VBIOS image ({}): {} bytes, sha256 {}, part {:?}, date {:?}, ATOM subsys {:04X?}:{:04X?}",
            v.source, v.size, v.sha256, v.part_number, v.build_date, v.atom_subsys_vendor, v.atom_subsys_device);
    }
}
