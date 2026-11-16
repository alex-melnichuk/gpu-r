//! Driver integrity: is the kernel module / user-space driver the one the
//! distribution shipped, or was it patched? Patched drivers can lie about
//! anything (IDs, memory size, unit counts), so every other check is only as
//! trustworthy as this one.

use crate::util::*;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, Default)]
pub struct FileCheck {
    pub what: String,
    pub path: String,
    pub package: Option<String>,
    /// Some(true) = matches package manager checksum, Some(false) = modified,
    /// None = could not verify (not packaged / no package manager).
    pub ok: Option<bool>,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct DriverIntegrity {
    pub module: String,
    pub loaded: bool,
    pub module_taint: Option<String>,
    pub srcversion_loaded: Option<String>,
    pub srcversion_disk: Option<String>,
    pub module_version: Option<String>,
    pub signer: Option<String>,
    pub kernel_taint: u64,
    pub kernel_taint_flags: Vec<String>,
    pub cmdline_params: Vec<String>,
    pub preload: Vec<String>,
    pub files: Vec<FileCheck>,
}

const TAINTS: &[(u32, char, &str)] = &[
    (0, 'P', "proprietary module loaded"),
    (1, 'F', "module force-loaded"),
    (2, 'S', "kernel running on out-of-spec system"),
    (3, 'R', "module force-unloaded"),
    (4, 'M', "machine check exception"),
    (5, 'B', "bad page referenced"),
    (6, 'U', "taint requested by userspace"),
    (7, 'D', "kernel died recently (OOPS/BUG)"),
    (8, 'A', "ACPI table overridden"),
    (9, 'W', "kernel issued warning"),
    (10, 'C', "staging driver loaded"),
    (11, 'I', "firmware bug workaround"),
    (12, 'O', "out-of-tree module loaded"),
    (13, 'E', "unsigned module loaded"),
    (14, 'L', "soft lockup occurred"),
    (15, 'K', "kernel live-patched"),
    (16, 'X', "auxiliary taint"),
    (17, 'T', "struct randomization plugin"),
    (18, 'N', "in-kernel test"),
];

pub fn md5_file(path: &str) -> Option<String> {
    run("md5sum", &[path]).and_then(|s| s.split_whitespace().next().map(str::to_string))
}

/// Verify a file against the package manager database (dpkg or rpm).
pub fn verify_file(what: &str, path: &str) -> FileCheck {
    let mut fc = FileCheck { what: what.into(), path: path.into(), ..Default::default() };
    let real = std::fs::canonicalize(path).map(|p| p.display().to_string()).unwrap_or(path.to_string());
    // dpkg: try the canonical path and the /usr-merged alias
    let mut cands = vec![real.clone(), path.to_string()];
    if let Some(r) = real.strip_prefix("/usr") {
        cands.push(r.to_string());
    } else {
        cands.push(format!("/usr{real}"));
    }
    if std::path::Path::new("/var/lib/dpkg/info").exists() {
        for c in &cands {
            let Some(out) = run("dpkg-query", &["-S", c]) else { continue };
            let Some(pkg) = out.lines().next().and_then(|l| l.split(':').next()).map(|s| s.trim().to_string()) else {
                continue;
            };
            fc.package = Some(pkg.clone());
            let rel = c.trim_start_matches('/');
            let arch = run("dpkg", &["--print-architecture"]).unwrap_or_default();
            let sums = read_str(format!("/var/lib/dpkg/info/{pkg}.md5sums"))
                .or_else(|| read_str(format!("/var/lib/dpkg/info/{pkg}:{arch}.md5sums")));
            let Some(sums) = sums else {
                fc.detail = "package has no md5sums list".into();
                return fc;
            };
            let expected = sums
                .lines()
                .find(|l| l.ends_with(rel) && l.split_whitespace().nth(1) == Some(rel))
                .and_then(|l| l.split_whitespace().next());
            let actual = md5_file(&real);
            match (expected, actual) {
                (Some(e), Some(a)) => {
                    fc.ok = Some(e == a);
                    fc.detail = if e == a {
                        format!("md5 {a} matches package {pkg}")
                    } else {
                        format!("md5 {a} != packaged {e} — FILE MODIFIED")
                    };
                }
                _ => fc.detail = "checksum not listed".into(),
            }
            return fc;
        }
    }
    if run("rpm", &["--version"]).is_some() {
        if let Some(pkg) = run("rpm", &["-qf", &real]) {
            fc.package = Some(pkg);
            let out = std::process::Command::new("rpm").args(["-Vf", &real]).output();
            if let Ok(o) = out {
                let txt = String::from_utf8_lossy(&o.stdout);
                let line = txt.lines().find(|l| l.ends_with(&real));
                fc.ok = Some(line.map_or(true, |l| !l.starts_with("..5") && !l.contains('5')));
                fc.detail = line.unwrap_or("rpm -V clean").to_string();
            }
            return fc;
        }
    }
    fc.detail = "not owned by any package (self-built or third-party)".into();
    fc
}

fn vulkan_icd_libs(vendor: u16) -> Vec<String> {
    let want: &[&str] = match vendor {
        0x1002 => &["radeon_icd"],
        0x10DE => &["nvidia_icd", "nouveau_icd"],
        0x8086 => &["intel_icd"],
        _ => &[],
    };
    let mut libs = vec![];
    for dir in ["/usr/share/vulkan/icd.d", "/etc/vulkan/icd.d"] {
        let Ok(rd) = std::fs::read_dir(dir) else { continue };
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if !want.iter().any(|w| n.starts_with(w)) {
                continue;
            }
            let Some(txt) = read_str(e.path()) else { continue };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) else { continue };
            if let Some(p) = v["ICD"]["library_path"].as_str() {
                let full = if p.starts_with('/') {
                    p.to_string()
                } else {
                    find_lib(p).unwrap_or(p.to_string())
                };
                if !libs.contains(&full) {
                    libs.push(full);
                }
            }
        }
    }
    libs
}

pub fn find_lib(name: &str) -> Option<String> {
    let out = run("ldconfig", &["-p"]).or_else(|| run("/sbin/ldconfig", &["-p"]))?;
    out.lines()
        .find(|l| l.trim_start().starts_with(&format!("{name} ")) && (l.contains("x86-64") || !cfg!(target_arch = "x86_64")))
        .and_then(|l| l.split("=>").nth(1))
        .map(|s| s.trim().to_string())
}

pub fn collect(module: &str, vendor: u16) -> DriverIntegrity {
    let mut d = DriverIntegrity { module: module.into(), ..Default::default() };
    let m = format!("/sys/module/{module}");
    d.loaded = std::path::Path::new(&m).exists();
    d.module_taint = read_str(format!("{m}/taint")).filter(|s| !s.is_empty());
    d.srcversion_loaded = read_str(format!("{m}/srcversion"));
    d.module_version = read_str(format!("{m}/version"));
    d.srcversion_disk = run("modinfo", &["-F", "srcversion", module]).filter(|s| !s.is_empty());
    d.signer = run("modinfo", &["-F", "signer", module]).filter(|s| !s.is_empty());
    d.kernel_taint = read_num("/proc/sys/kernel/tainted").unwrap_or(0);
    for (bit, c, desc) in TAINTS {
        if d.kernel_taint & (1 << bit) != 0 {
            d.kernel_taint_flags.push(format!("{c}: {desc}"));
        }
    }
    if let Some(cl) = read_str("/proc/cmdline") {
        d.cmdline_params = cl
            .split_whitespace()
            .filter(|p| {
                ["amdgpu.", "radeon.", "nvidia.", "nvidia-drm.", "i915.", "xe.", "nouveau."]
                    .iter()
                    .any(|x| p.starts_with(x))
            })
            .map(str::to_string)
            .collect();
    }
    if let Ok(p) = std::env::var("LD_PRELOAD") {
        if !p.is_empty() {
            d.preload.push(format!("LD_PRELOAD={p}"));
        }
    }
    if let Some(p) = read_str("/etc/ld.so.preload") {
        for l in p.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#')) {
            d.preload.push(format!("/etc/ld.so.preload: {l}"));
        }
    }

    if let Some(f) = run("modinfo", &["-F", "filename", module]).filter(|f| f.starts_with('/')) {
        d.files.push(verify_file("kernel module", &f));
    }
    for lib in vulkan_icd_libs(vendor) {
        d.files.push(verify_file("Vulkan driver", &lib));
    }
    let extra: &[&str] = match vendor {
        0x1002 => &["libdrm_amdgpu.so.1"],
        0x10DE => &["libnvidia-ml.so.1", "libcuda.so.1"],
        _ => &[],
    };
    for l in extra {
        if let Some(p) = find_lib(l) {
            d.files.push(verify_file("driver library", &p));
        }
    }
    d
}
