//! GPU-side tests through Vulkan (wgpu compute):
//!
//! * VRAM test: fills (almost) all free VRAM with address-dependent patterns,
//!   then verifies. Because every chunk gets a unique pattern and the whole
//!   range is written *before* it is verified, a card whose BIOS claims more
//!   memory than is physically soldered (address aliasing) fails, and so do
//!   faulty chips. On amdgpu/NVML the VRAM usage counter is watched to prove
//!   the allocations really live in VRAM and were not silently spilled to
//!   system RAM.
//! * FP32 throughput: FMA-bound kernel; measured FLOPS / (2 × clock) gives the
//!   number of *working* FP32 lanes, independent of what driver/BIOS claim.

use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, Default)]
pub struct MemTestResult {
    pub reported_vram: u64,
    pub free_before: Option<u64>,
    pub target_bytes: u64,
    pub allocated_bytes: u64,
    pub chunk_bytes: u64,
    /// Increase of the driver VRAM-used counter while our buffers were resident.
    pub vram_delta: Option<u64>,
    pub patterns_done: u32,
    pub passes_done: u32,
    pub errors: u64,
    pub error_samples: Vec<(u64, u32)>,
    pub write_gbs: f64,
    pub read_gbs: f64,
    pub seconds: f64,
    pub aborted: Option<String>,
    pub alloc_note: Option<String>,
    /// Stress mode: ran until stopped / error / device loss.
    pub stress: bool,
    pub device_lost: Option<String>,
    pub log_file: Option<String>,
}

impl MemTestResult {
    pub fn in_vram_ratio(&self) -> Option<f64> {
        self.vram_delta.map(|d| d as f64 / self.allocated_bytes.max(1) as f64)
    }

    /// Capacity is considered verified when the test covered nearly all free
    /// memory, everything sat in VRAM and no errors were found.
    pub fn verified_capacity(&self) -> bool {
        let covered = self.free_before.map_or(self.allocated_bytes as f64 >= 0.8 * self.reported_vram as f64, |f| {
            self.allocated_bytes as f64 >= 0.85 * f as f64
        });
        self.errors == 0
            && self.aborted.is_none()
            && self.patterns_done > 0
            && covered
            && self.in_vram_ratio().is_none_or(|r| r > 0.9)
    }

    pub fn summary(&self) -> String {
        let mut s = format!(
            "tested {} of {} reported in {} chunks, {} pattern(s) × {} pass(es), {} error(s)",
            crate::util::fmt_bytes(self.allocated_bytes),
            crate::util::fmt_bytes(self.reported_vram),
            self.allocated_bytes / self.chunk_bytes.max(1),
            self.patterns_done,
            self.passes_done,
            self.errors,
        );
        if self.write_gbs > 0.0 && self.read_gbs > 0.0 {
            s.push_str(&format!(", write {:.0} GB/s, verify {:.0} GB/s", self.write_gbs, self.read_gbs));
        }
        let t = self.seconds as u64;
        s.push_str(&if t >= 60 { format!(", {:02}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60) } else { format!(", {:.1}s", self.seconds) });
        if let Some(r) = self.in_vram_ratio() {
            s.push_str(&format!(", resident in VRAM {:.0}%", r * 100.0));
            if r < 0.9 {
                s.push_str(" — buffers spilled to system RAM: the card has LESS usable VRAM than reported");
            }
        }
        if let Some(a) = &self.alloc_note {
            s.push_str(&format!(" ({a})"));
        }
        if let Some(a) = &self.aborted {
            s.push_str(&format!(" — ABORTED: {a}"));
        }
        if self.errors > 0 {
            s.push_str(" — MEMORY ERRORS / ADDRESS ALIASING: faulty or fake VRAM");
        }
        if let Some(d) = &self.device_lost {
            s.push_str(&format!(" — GPU DEVICE LOST (hang / driver reset): {d}"));
        }
        s
    }
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ComputeResult {
    pub gflops: f64,
    pub clock_mhz: Option<f64>,
    pub effective_lanes: Option<f64>,
    pub claimed_shaders: Option<u32>,
    pub verdict_ok: Option<bool>,
    pub note: String,
}

impl ComputeResult {
    pub fn summary(&self) -> String {
        let mut s = format!("{:.0} GFLOPS FP32", self.gflops);
        if let Some(c) = self.clock_mhz {
            s.push_str(&format!(" @ {c:.0} MHz"));
        }
        if let Some(e) = self.effective_lanes {
            s.push_str(&format!(" → {e:.0} effective FP32 lanes"));
        }
        if let Some(c) = self.claimed_shaders {
            s.push_str(&format!(" / {c} claimed"));
        }
        if !self.note.is_empty() {
            s.push_str(&format!(" — {}", self.note));
        }
        s
    }
}

#[derive(Default)]
pub struct Progress {
    /// Per-chunk state for the VRAM map: 0 = not allocated, 1 = allocated,
    /// 2 = written, 3 = verified OK, 4 = errors.
    pub chunks: Vec<u8>,
    pub chunk_bytes: u64,
    pub phase: String,
    pub frac: f32,
    pub errors: u64,
    pub log: Vec<String>,
    /// Stress mode bookkeeping, read by the UI.
    pub pass: u32,
    pub started: Option<Instant>,
    /// Last time the worker made progress; a stale heartbeat means the GPU stopped responding.
    pub heartbeat: Option<Instant>,
    pub device_lost: Option<String>,
}

pub type Shared = Arc<Mutex<Progress>>;

const LOG_KEEP: usize = 500;

fn log(p: &Shared, msg: impl Into<String>) {
    let m = msg.into();
    let mut g = p.lock().unwrap();
    g.log.push(m);
    if g.log.len() > LOG_KEEP + 100 {
        let n = g.log.len();
        let cut = (n - LOG_KEEP).min(n - 4);
        g.log.drain(4..4 + cut); // keep the header lines
    }
}

fn set(p: &Shared, phase: impl Into<String>, frac: f32) {
    let mut g = p.lock().unwrap();
    g.phase = phase.into();
    g.frac = frac;
    g.heartbeat = Some(Instant::now());
}

pub struct Ctx {
    device: wgpu::Device,
    queue: wgpu::Queue,
    limits: wgpu::Limits,
    pub adapter_name: String,
}

pub fn open(vendor: u16, device: u16) -> Result<Ctx, String> {
    let inst = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: crate::util::BACKENDS,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let mut adapters = pollster::block_on(inst.enumerate_adapters(crate::util::BACKENDS));
    adapters.sort_by_key(|a| a.get_info().backend != wgpu::Backend::Vulkan);
    let adapter = adapters
        .into_iter()
        .find(|a| {
            let i = a.get_info();
            i.vendor == vendor as u32 && i.device == device as u32 && i.device_type != wgpu::DeviceType::Cpu
        })
        .ok_or("no Vulkan/DX12 adapter for this GPU")?;
    let info = adapter.get_info();
    let al = adapter.limits();
    let mut limits = wgpu::Limits::default();
    limits.max_buffer_size = al.max_buffer_size;
    limits.max_storage_buffer_binding_size = al.max_storage_buffer_binding_size;
    limits.max_compute_workgroups_per_dimension = al.max_compute_workgroups_per_dimension;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("gpu-r test"),
        required_limits: limits.clone(),
        memory_hints: wgpu::MemoryHints::Performance,
        ..Default::default()
    }))
    .map_err(|e| e.to_string())?;
    device.on_uncaptured_error(Arc::new(|e: wgpu::Error| {
        eprintln!("wgpu error: {e}");
    }));
    Ok(Ctx { device, queue, limits, adapter_name: format!("{} ({})", info.name, info.driver) })
}

const MEM_WGSL: &str = r#"
struct Params { pattern: u32, seed: u32, chunk: u32, words: u32, mode: u32, p0: u32, p1: u32, p2: u32 }
@group(0) @binding(0) var<storage, read_write> buf: array<u32>;
@group(0) @binding(1) var<uniform> p: Params;
@group(0) @binding(2) var<storage, read_write> res: array<atomic<u32>, 1088>;

fn hash(x: u32) -> u32 {
    let s = x * 747796405u + 2891336453u;
    let w = ((s >> ((s >> 28u) + 4u)) ^ s) * 277803737u;
    return (w >> 22u) ^ w;
}

fn value(i: u32) -> u32 {
    let tag = hash(p.chunk * 7919u + p.seed);
    switch p.pattern {
        case 0u: { return i ^ tag; }                       // address-in-address (unique per chunk)
        case 1u: { return ~(i ^ tag); }                    // inverted address
        case 2u: { return hash(i ^ tag); }                 // pseudo-random
        case 3u: { return 1u << ((i + p.seed) & 31u); }    // walking ones
        case 4u: { return select(0x55555555u, 0xAAAAAAAAu, ((i ^ p.seed) & 1u) == 1u); } // checkerboard
        default: { return select(0u, 0xFFFFFFFFu, (p.seed & 1u) == 1u); } // solid
    }
}

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let stride = nwg.x * 256u;
    var i = gid.x;
    if (p.mode == 0u) {
        loop {
            if (i >= p.words) { break; }
            buf[i] = value(i);
            i += stride;
        }
    } else {
        var errs = 0u;
        loop {
            if (i >= p.words) { break; }
            let got = buf[i];
            let exp = value(i);
            if (got != exp) {
                errs += 1u;
                let slot = atomicAdd(&res[1], 1u);
                if (slot < 20u) {
                    atomicStore(&res[4u + slot * 3u], p.chunk);
                    atomicStore(&res[5u + slot * 3u], i);
                    atomicStore(&res[6u + slot * 3u], got ^ exp);
                }
            }
            i += stride;
        }
        if (errs > 0u) { atomicAdd(&res[0], errs); atomicAdd(&res[64u + min(p.chunk, 1023u)], errs); }
    }
}
"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    pattern: u32,
    seed: u32,
    chunk: u32,
    words: u32,
    mode: u32,
    pad: [u32; 3],
}

const RES_BYTES: u64 = 1088 * 4;

const PATTERN_NAMES: [&str; 6] = ["address", "inverted address", "random", "walking ones", "checkerboard", "solid"];

pub struct MemTestOptions {
    pub reported_vram: u64,
    pub used_vram: Option<u64>,
    /// Fraction of *free* VRAM to test.
    pub fraction: f64,
    pub passes: u32,
    pub patterns: Vec<u32>,
    /// Loop passes until cancelled, an error is found or the device is lost.
    pub stress: bool,
    /// Keep going after errors in stress mode (count them instead of stopping).
    pub continue_on_error: bool,
    /// Interleave an FP32 FMA load so the core heats up while memory is tested.
    pub heat: bool,
    /// Stress mode: append one line per pass here and fsync it, so the result survives a system hang.
    pub log_path: Option<std::path::PathBuf>,
}

/// `vram_used` returns the driver's current VRAM usage counter if known.
pub fn run_memtest(
    ctx: &Ctx,
    opt: &MemTestOptions,
    vram_used: &dyn Fn() -> Option<u64>,
    sensors_line: &dyn Fn() -> String,
    prog: &Shared,
    cancel: &AtomicBool,
) -> MemTestResult {
    let t0 = Instant::now();
    let dev = &ctx.device;
    let mut r = MemTestResult { reported_vram: opt.reported_vram, stress: opt.stress, ..Default::default() };
    {
        let mut g = prog.lock().unwrap();
        g.started = Some(t0);
        g.heartbeat = Some(t0);
    }
    let lost: Arc<Mutex<Option<String>>> = Default::default();
    {
        let (l2, p2) = (lost.clone(), prog.clone());
        dev.set_device_lost_callback(move |reason, msg| {
            let m = format!("{reason:?}: {msg}");
            *l2.lock().unwrap() = Some(m.clone());
            p2.lock().unwrap().device_lost = Some(m);
        });
    }
    let mut logf = opt.log_path.as_ref().and_then(|p| {
        std::fs::OpenOptions::new().create(true).append(true).open(p).ok()
    });
    r.log_file = opt.log_path.as_ref().map(|p| p.display().to_string());
    let mut stress_log = |line: String| {
        if let Some(f) = logf.as_mut() {
            use std::io::Write;
            let _ = writeln!(f, "{line}");
            let _ = f.sync_data();
        }
    };
    if opt.stress {
        stress_log(format!("# gpu-r stress test, adapter {}, started (unix) {}", ctx.adapter_name,
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)));
    }

    let max_bind = ctx.limits.max_storage_buffer_binding_size.min(ctx.limits.max_buffer_size as u64);
    let chunk = (256u64 << 20).min(max_bind) & !0xfffu64;
    r.chunk_bytes = chunk;
    let free = opt.used_vram.map(|u| opt.reported_vram.saturating_sub(u));
    r.free_before = free;
    let base = free.unwrap_or(opt.reported_vram);
    let margin = 128u64 << 20;
    r.target_bytes = ((base.saturating_sub(margin)) as f64 * opt.fraction) as u64;
    let n_chunks = (r.target_bytes / chunk).clamp(1, 1024);
    log(prog, format!("Adapter: {}", ctx.adapter_name));
    log(prog, format!(
        "Target {} in {} chunks of {} MB (free VRAM {})",
        crate::util::fmt_bytes(r.target_bytes),
        n_chunks,
        chunk >> 20,
        free.map(crate::util::fmt_bytes).unwrap_or("unknown".into())
    ));

    let shader = dev.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("memtest"),
        source: wgpu::ShaderSource::Wgsl(MEM_WGSL.into()),
    });
    let pipe = dev.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("memtest"),
        layout: None,
        module: &shader,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let bgl = pipe.get_bind_group_layout(0);
    let res_buf = dev.create_buffer(&wgpu::BufferDescriptor {
        label: Some("results"),
        size: RES_BYTES,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let staging = dev.create_buffer(&wgpu::BufferDescriptor {
        label: Some("staging"),
        size: RES_BYTES,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    // ---------------- optional core load ----------------
    let heat = opt.heat.then(|| {
        let sm = dev.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("heat"), source: wgpu::ShaderSource::Wgsl(FMA_WGSL.into()) });
        let hp = dev.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("heat"), layout: None, module: &sm, entry_point: Some("main"), compilation_options: Default::default(), cache: None,
        });
        let out = dev.create_buffer(&wgpu::BufferDescriptor { label: None, size: 4096 * 256 * 4, usage: wgpu::BufferUsages::STORAGE, mapped_at_creation: false });
        let ub = dev.create_buffer(&wgpu::BufferDescriptor { label: None, size: 16, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let bg = dev.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None, layout: &hp.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: out.as_entire_binding() }, wgpu::BindGroupEntry { binding: 1, resource: ub.as_entire_binding() }],
        });
        // calibrate one dispatch to ~25 ms so the memory work still dominates scheduling
        let mut iters = 256u32;
        for _ in 0..8 {
            ctx.queue.write_buffer(&ub, 0, bytemuck::cast_slice(&[iters, 0u32, 0, 0]));
            let mut enc = dev.create_command_encoder(&Default::default());
            {
                let mut cp = enc.begin_compute_pass(&Default::default());
                cp.set_pipeline(&hp);
                cp.set_bind_group(0, &bg, &[]);
                cp.dispatch_workgroups(4096, 1, 1);
            }
            let t = Instant::now();
            ctx.queue.submit([enc.finish()]);
            let _ = dev.poll(wgpu::PollType::wait_indefinitely());
            let el = t.elapsed().as_secs_f64();
            if el > 0.005 {
                iters = ((iters as f64) * 0.025 / el).clamp(64.0, 1e6) as u32;
                break;
            }
            iters *= 4;
        }
        ctx.queue.write_buffer(&ub, 0, bytemuck::cast_slice(&[iters, 0u32, 0, 0]));
        log(prog, format!("Core load enabled ({iters} FMA iterations per dispatch)"));
        (hp, bg, out, ub)
    });

    // ---------------- allocation ----------------
    let used0 = vram_used();
    let mut chunks: Vec<(wgpu::Buffer, wgpu::Buffer, wgpu::BindGroup)> = vec![];
    set(prog, "Allocating VRAM", 0.0);
    {
        let mut g = prog.lock().unwrap();
        g.chunks = vec![0; n_chunks.min(1024) as usize];
        g.chunk_bytes = chunk;
    }
    for i in 0..n_chunks {
        if cancel.load(Ordering::Relaxed) {
            r.aborted = Some("cancelled".into());
            break;
        }
        let scope = dev.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let b = dev.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: chunk,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        if let Some(e) = pollster::block_on(scope.pop()) {
            r.alloc_note = Some(format!("allocation stopped at chunk {i}: {e}"));
            log(prog, r.alloc_note.clone().unwrap());
            break;
        }
        let pb = dev.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // GPUR_SIMULATE_ALIAS=1: make the last chunk point at chunk 0's memory,
        // emulating a fake card whose upper address range wraps onto lower memory.
        let alias = std::env::var("GPUR_SIMULATE_ALIAS").is_ok() && i + 1 == n_chunks && !chunks.is_empty();
        let target = if alias { &chunks[0].0 } else { &b };
        let bg = dev.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: target.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: pb.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: res_buf.as_entire_binding() },
            ],
        });
        chunks.push((b, pb, bg));
        if let Some(st) = prog.lock().unwrap().chunks.get_mut(i as usize) {
            *st = 1;
        }
        set(prog, "Allocating VRAM", (i + 1) as f32 / n_chunks as f32);
    }
    r.allocated_bytes = chunks.len() as u64 * chunk;
    if chunks.is_empty() {
        r.aborted.get_or_insert("could not allocate any VRAM".into());
        r.seconds = t0.elapsed().as_secs_f64();
        return r;
    }
    log(prog, format!("Allocated {}", crate::util::fmt_bytes(r.allocated_bytes)));

    let words = (chunk / 4) as u32;
    let wg = 8192u32.min(ctx.limits.max_compute_workgroups_per_dimension);
    let steps_per_pass = (opt.patterns.len() as u32 * 2).max(1);
    let total_steps = (opt.passes * steps_per_pass).max(1);
    let mut step = 0u32;
    let (mut wbytes, mut wsecs, mut rbytes, mut rsecs) = (0f64, 0f64, 0f64, 0f64);
    let passes = if opt.stress { u32::MAX } else { opt.passes };

    let mut last_line = Instant::now() - Duration::from_secs(3600);
    'outer: for pass in 0..passes {
        prog.lock().unwrap().pass = pass + 1;
        let errors_before = r.errors;
        for &pat in &opt.patterns {
            if cancel.load(Ordering::Relaxed) {
                r.aborted = Some("cancelled".into());
                break 'outer;
            }
            if let Some(l) = lost.lock().unwrap().clone() {
                r.device_lost = Some(l);
                break 'outer;
            }
            let seed = 0x9E37_79B9u32.wrapping_mul(pass + 1).wrapping_add(pat);
            let pname = PATTERN_NAMES.get(pat as usize).copied().unwrap_or("?");
            for mode in 0..2u32 {
                let frac = if opt.stress { (step % steps_per_pass) as f32 / steps_per_pass as f32 } else { step as f32 / total_steps as f32 };
                set(prog, format!("Pass {} — {} — {}", pass + 1, pname, if mode == 0 { "write" } else { "verify" }), frac);
                for (ci, (_, pb, _)) in chunks.iter().enumerate() {
                    let p = Params { pattern: pat, seed, chunk: ci as u32, words, mode, pad: [0; 3] };
                    ctx.queue.write_buffer(pb, 0, bytemuck::bytes_of(&p));
                }
                if mode == 1 {
                    ctx.queue.write_buffer(&res_buf, 0, &[0u8; RES_BYTES as usize]);
                }
                let t = Instant::now();
                // Submit in groups so the UI keeps updating and no single submission is too long.
                for group in chunks.chunks(8) {
                    let mut enc = dev.create_command_encoder(&Default::default());
                    {
                        let mut cp = enc.begin_compute_pass(&Default::default());
                        cp.set_pipeline(&pipe);
                        for (_, _, bg) in group {
                            cp.set_bind_group(0, bg, &[]);
                            cp.dispatch_workgroups(wg, 1, 1);
                        }
                    }
                    if let Some((hp, hbg, _, _)) = &heat {
                        let mut cp = enc.begin_compute_pass(&Default::default());
                        cp.set_pipeline(hp);
                        cp.set_bind_group(0, hbg, &[]);
                        cp.dispatch_workgroups(4096, 1, 1);
                    }
                    ctx.queue.submit([enc.finish()]);
                }
                if mode == 0 && pass == 0 && step == 0 {
                    let mut g = prog.lock().unwrap();
                    for st in g.chunks.iter_mut().filter(|s| **s == 1) {
                        *st = 2;
                    }
                }
                let _ = dev.poll(wgpu::PollType::wait_indefinitely());
                let secs = t.elapsed().as_secs_f64();
                if mode == 0 && !(pass == 0 && step == 0) && heat.is_none() {
                    wbytes += r.allocated_bytes as f64;
                    wsecs += secs;
                }
                if mode == 0 {
                    if pass == 0 && r.vram_delta.is_none() {
                        if let (Some(a), Some(b)) = (used0, vram_used()) {
                            r.vram_delta = Some(b.saturating_sub(a));
                        }
                    }
                } else {
                    if heat.is_none() {
                        rbytes += r.allocated_bytes as f64;
                        rsecs += secs;
                    }
                    let (errs, samples, per_chunk) = read_results(ctx, &res_buf, &staging);
                    {
                        let mut g = prog.lock().unwrap();
                        for (ci, st) in g.chunks.iter_mut().enumerate() {
                            if *st >= 1 {
                                *st = if per_chunk.get(ci).copied().unwrap_or(0) > 0 || *st == 4 { 4 } else { 3 };
                            }
                        }
                    }
                    if errs > 0 {
                        r.errors += errs as u64;
                        for (c, i, x) in samples {
                            let addr = c as u64 * chunk + i as u64 * 4;
                            if r.error_samples.len() < 32 {
                                r.error_samples.push((addr, x));
                            }
                        }
                        log(prog, format!("  pass {} {pname}: {errs} error(s)", pass + 1));
                    } else if !opt.stress {
                        log(prog, format!("  pass {} {pname}: OK", pass + 1));
                    }
                    prog.lock().unwrap().errors = r.errors;
                    r.patterns_done += 1;
                }
                step += 1;
            }
        }
        r.passes_done = pass + 1;
        if opt.stress {
            let el = t0.elapsed().as_secs();
            let line = format!("pass {:>5}  {:02}:{:02}:{:02}  new errors {:>10}  total errors {:>10}  {}",
                pass + 1, el / 3600, el / 60 % 60, el % 60, r.errors - errors_before, r.errors, sensors_line());
            // one line per 10 s (and every pass that found errors) keeps hour-long runs readable
            if r.errors > errors_before || last_line.elapsed() >= Duration::from_secs(10) || pass == 0 {
                last_line = Instant::now();
                log(prog, line.clone());
                stress_log(line);
            }
            if r.errors > 0 && !opt.continue_on_error {
                r.aborted = Some(format!("stopped at the first error, pass {}", pass + 1));
                break;
            }
        }
    }
    if let Some(l) = lost.lock().unwrap().clone() {
        r.device_lost.get_or_insert(l);
    }
    r.write_gbs = if wsecs > 0.0 { wbytes / wsecs / 1e9 } else { 0.0 };
    r.read_gbs = if rsecs > 0.0 { rbytes / rsecs / 1e9 } else { 0.0 };
    r.seconds = t0.elapsed().as_secs_f64();
    set(prog, "Done", 1.0);
    log(prog, r.summary());
    if opt.stress {
        stress_log(format!("# end: {}", r.summary()));
    }
    drop(chunks);
    r
}

fn read_results(ctx: &Ctx, res: &wgpu::Buffer, staging: &wgpu::Buffer) -> (u32, Vec<(u32, u32, u32)>, Vec<u32>) {
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    enc.copy_buffer_to_buffer(res, 0, staging, 0, RES_BYTES);
    ctx.queue.submit([enc.finish()]);
    let slice = staging.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    let _ = ctx.device.poll(wgpu::PollType::wait_indefinitely());
    let out = {
        let data = slice.get_mapped_range().expect("map");
        let w: &[u32] = bytemuck::cast_slice(&data);
        let errs = w[0];
        let n = w[1].min(20) as usize;
        let samples = (0..n).map(|s| (w[4 + s * 3], w[5 + s * 3], w[6 + s * 3])).collect();
        (errs, samples, w[64..].to_vec())
    };
    staging.unmap();
    out
}

const FMA_WGSL: &str = r#"
@group(0) @binding(0) var<storage, read_write> outv: array<f32>;
@group(0) @binding(1) var<uniform> q: vec4<u32>;

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = f32(gid.x) * 1e-7;
    var a = vec4<f32>(x, x + 1.0, x + 2.0, x + 3.0);
    var b = a + vec4<f32>(0.1);
    var c = a + vec4<f32>(0.2);
    var d = a + vec4<f32>(0.3);
    var e = a + vec4<f32>(0.4);
    var f = a + vec4<f32>(0.5);
    var g = a + vec4<f32>(0.6);
    var h = a + vec4<f32>(0.7);
    let m = vec4<f32>(0.99991);
    let k = vec4<f32>(0.00013);
    for (var i = 0u; i < q.x; i++) {
        a = fma(a, m, k); b = fma(b, m, k); c = fma(c, m, k); d = fma(d, m, k);
        e = fma(e, m, k); f = fma(f, m, k); g = fma(g, m, k); h = fma(h, m, k);
    }
    outv[gid.x] = dot(a + b + c + d + e + f + g + h, vec4<f32>(1.0));
}
"#;

pub fn run_compute(
    ctx: &Ctx,
    claimed_shaders: Option<u32>,
    gpu_clock: &dyn Fn() -> Option<f64>,
    prog: &Shared,
) -> ComputeResult {
    let dev = &ctx.device;
    let shader = dev.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("fma"),
        source: wgpu::ShaderSource::Wgsl(FMA_WGSL.into()),
    });
    let pipe = dev.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("fma"),
        layout: None,
        module: &shader,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let groups: u32 = 8192;
    let threads = groups as u64 * 256;
    let out = dev.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: threads * 4,
        usage: wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    });
    let ub = dev.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 16,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bg = dev.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipe.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: out.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: ub.as_entire_binding() },
        ],
    });
    let run = |iters: u32| -> f64 {
        ctx.queue.write_buffer(&ub, 0, bytemuck::cast_slice(&[iters, 0u32, 0, 0]));
        let mut enc = dev.create_command_encoder(&Default::default());
        {
            let mut cp = enc.begin_compute_pass(&Default::default());
            cp.set_pipeline(&pipe);
            cp.set_bind_group(0, &bg, &[]);
            cp.dispatch_workgroups(groups, 1, 1);
        }
        let t = Instant::now();
        ctx.queue.submit([enc.finish()]);
        let _ = dev.poll(wgpu::PollType::wait_indefinitely());
        t.elapsed().as_secs_f64()
    };
    set(prog, "Compute: warm-up", 0.05);
    run(64);
    // calibrate to ~0.3 s per run
    let mut iters = 256u32;
    loop {
        let t = run(iters);
        if t > 0.15 || iters >= 1 << 20 {
            iters = ((iters as f64) * (0.3 / t.max(1e-4))).clamp(256.0, (1u32 << 22) as f64) as u32;
            break;
        }
        iters *= 4;
    }
    // keep the GPU busy ~2s so clocks settle, sample clock while running
    let mut best = f64::MAX;
    let mut clocks = vec![];
    let start = Instant::now();
    let mut n = 0;
    while start.elapsed() < Duration::from_secs(3) || n < 4 {
        let t = run(iters);
        best = best.min(t);
        if let Some(c) = gpu_clock() {
            clocks.push(c);
        }
        n += 1;
        set(prog, "Compute: FP32 FMA benchmark", 0.1 + 0.9 * (start.elapsed().as_secs_f32() / 3.0).min(1.0));
    }
    let flops = threads as f64 * iters as f64 * 8.0 * 4.0 * 2.0;
    let gflops = flops / best / 1e9;
    clocks.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let clock = clocks.get(clocks.len() / 2).copied().filter(|&c| c > 100.0);
    let mut r = ComputeResult { gflops, clock_mhz: clock, claimed_shaders, ..Default::default() };
    if let Some(c) = clock {
        let lanes = gflops * 1e9 / (2.0 * c * 1e6);
        r.effective_lanes = Some(lanes);
        if let Some(cs) = claimed_shaders {
            let ratio = lanes / cs as f64;
            r.verdict_ok = Some(ratio >= 0.70);
            r.note = if ratio < 0.70 {
                format!("only {:.0}% of the claimed shaders are doing work — fewer real units than reported (fake/relabelled or throttling)", ratio * 100.0)
            } else if ratio > 1.4 {
                format!("{:.0}% of claimed: dual-issue/ dual-FP32 architecture or clock read lower than actual", ratio * 100.0)
            } else {
                format!("{:.0}% of claimed — consistent", ratio * 100.0)
            };
        }
    } else {
        r.note = "GPU clock not readable, cannot derive unit count".into();
    }
    log(prog, r.summary());
    set(prog, "Done", 1.0);
    r
}
