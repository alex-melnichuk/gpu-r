//! egui front-end: compact GPU-Z style window with dark/light themes.
//!
//! Every value box carries a thin status edge (cross-checked / differs /
//! contradicts) and a tooltip listing the independent sources consulted.

use crate::gpu::{self, Check, Field, Gpu, Status};
use crate::i18n::{self, tr, trf};
use crate::gputest::{self, ComputeResult, MemTestResult};
use crate::sensors::{Sampler, Series};
use eframe::egui::{self, Color32, FontFamily, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use egui_plot::{Legend, Line, Plot, PlotPoints};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------- theme

#[derive(Clone, Copy)]
struct Pal {
    bg: Color32,
    surface: Color32,
    raised: Color32,
    well: Color32,
    chrome: Color32,
    line: Color32,
    text: Color32,
    muted: Color32,
    accent: Color32,
    ok: Color32,
    warn: Color32,
    bad: Color32,
    skip: Color32,
    series: [Color32; 5],
}

const fn hex(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

const DARK: Pal = Pal {
    bg: hex(0x222832),
    surface: hex(0x212833),
    raised: hex(0x2E3746),
    well: hex(0x161A21),
    chrome: hex(0x1A1E25),
    line: hex(0x343E4D),
    text: hex(0xE4E8EE),
    muted: hex(0x8B95A5),
    accent: hex(0x6CB4D9),
    ok: hex(0x47B881),
    warn: hex(0xE0A43A),
    bad: hex(0xE5484D),
    skip: hex(0x5A6475),
    series: [hex(0x6CB4D9), hex(0xE0A43A), hex(0xC08BE8), hex(0x47B881), hex(0xE5484D)],
};

const LIGHT: Pal = Pal {
    bg: hex(0xF2F4F7),
    surface: hex(0xFFFFFF),
    raised: hex(0xE1E6EC),
    well: hex(0xFFFFFF),
    chrome: hex(0xE4E8ED),
    line: hex(0xD3D9E1),
    text: hex(0x1B2230),
    muted: hex(0x5D6878),
    accent: hex(0x2A77A6),
    ok: hex(0x1F8F59),
    warn: hex(0xB07408),
    bad: hex(0xC9323A),
    skip: hex(0xA3ACB9),
    series: [hex(0x2A77A6), hex(0xC27C0E), hex(0x8A4FC4), hex(0x1F8F59), hex(0xC9323A)],
};

fn semi() -> FontFamily {
    FontFamily::Name("semi".into())
}
fn medium() -> FontFamily {
    FontFamily::Name("medium".into())
}

fn install_fonts(ctx: &egui::Context) {
    let mut f = egui::FontDefinitions::default();
    let add = |f: &mut egui::FontDefinitions, k: &str, b: &'static [u8]| {
        f.font_data.insert(k.into(), Arc::new(egui::FontData::from_static(b)));
    };
    add(&mut f, "plex", include_bytes!("../assets/fonts/plexsans-400.ttf"));
    add(&mut f, "plex-med", include_bytes!("../assets/fonts/plexsans-500.ttf"));
    add(&mut f, "plex-semi", include_bytes!("../assets/fonts/plexsans-600.ttf"));
    add(&mut f, "plex-mono", include_bytes!("../assets/fonts/plexmono-400.ttf"));
    // Cyrillic (uk/ru) and Latin Extended (pl/de) subsets, used as per-glyph fallbacks
    add(&mut f, "plex-cyr", include_bytes!("../assets/fonts/plexsans-cyrillic-400.ttf"));
    add(&mut f, "plex-cyr-med", include_bytes!("../assets/fonts/plexsans-cyrillic-500.ttf"));
    add(&mut f, "plex-cyr-semi", include_bytes!("../assets/fonts/plexsans-cyrillic-600.ttf"));
    add(&mut f, "plex-ext", include_bytes!("../assets/fonts/plexsans-latin-ext-400.ttf"));
    add(&mut f, "plex-ext-med", include_bytes!("../assets/fonts/plexsans-latin-ext-500.ttf"));
    add(&mut f, "plex-ext-semi", include_bytes!("../assets/fonts/plexsans-latin-ext-600.ttf"));
    add(&mut f, "plex-mono-cyr", include_bytes!("../assets/fonts/plexmono-cyrillic-400.ttf"));
    add(&mut f, "plex-mono-ext", include_bytes!("../assets/fonts/plexmono-latin-ext-400.ttf"));
    let prop = f.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let mono = f.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();
    let with = |first: &[&str], rest: &[String]| {
        let mut v: Vec<String> = first.iter().map(|x| x.to_string()).collect();
        v.extend(rest.iter().cloned());
        v
    };
    f.families.insert(FontFamily::Proportional, with(&["plex", "plex-cyr", "plex-ext"], &prop));
    f.families.insert(semi(), with(&["plex-semi", "plex-cyr-semi", "plex-ext-semi"], &prop));
    f.families.insert(medium(), with(&["plex-med", "plex-cyr-med", "plex-ext-med"], &prop));
    f.families.insert(FontFamily::Monospace, with(&["plex-mono", "plex-mono-cyr", "plex-mono-ext"], &mono));
    ctx.set_fonts(f);
}

fn apply_theme(ctx: &egui::Context, p: &Pal, dark: bool) {
    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.panel_fill = p.bg;
    v.window_fill = p.surface;
    v.extreme_bg_color = p.well;
    v.faint_bg_color = p.surface;
    v.code_bg_color = p.raised;
    v.window_stroke = Stroke::new(1.0, p.line);
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = p.accent.gamma_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.widgets.noninteractive.bg_fill = p.surface;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.line);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = 5.into();
        w.fg_stroke = Stroke::new(1.0, p.text);
    }
    v.widgets.inactive.bg_fill = p.raised;
    v.widgets.inactive.weak_bg_fill = p.raised;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, p.line);
    v.widgets.hovered.bg_fill = p.line;
    v.widgets.hovered.weak_bg_fill = p.line;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.accent.gamma_multiply(0.6));
    v.widgets.active.bg_fill = p.accent.gamma_multiply(0.5);
    v.widgets.active.weak_bg_fill = p.accent.gamma_multiply(0.5);
    v.widgets.noninteractive.corner_radius = 5.into();
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        use egui::TextStyle::*;
        s.text_styles.insert(Body, FontId::new(12.5, FontFamily::Proportional));
        s.text_styles.insert(Button, FontId::new(12.5, FontFamily::Proportional));
        s.text_styles.insert(Small, FontId::new(11.0, FontFamily::Proportional));
        s.text_styles.insert(Heading, FontId::new(17.0, semi()));
        s.text_styles.insert(Monospace, FontId::new(11.5, FontFamily::Monospace));
        s.spacing.item_spacing = Vec2::new(6.0, 4.0);
        s.spacing.button_padding = Vec2::new(8.0, 3.0);
        s.spacing.interact_size.y = 22.0;
    });
}

fn status_col(p: &Pal, s: Status) -> Color32 {
    match s {
        Status::Ok => p.ok,
        Status::Info => p.accent,
        Status::Warn => p.warn,
        Status::Fail => p.bad,
        Status::Skipped => p.skip,
    }
}

fn status_glyph(s: Status) -> &'static str {
    match s {
        Status::Ok => "✔",
        Status::Info => "i",
        Status::Warn => "!",
        Status::Fail => "✖",
        Status::Skipped => "–",
    }
}

// ---------------------------------------------------------------- app state

#[derive(PartialEq, Clone, Copy)]
enum Page {
    Card,
    Sensors,
    Verify,
    MemTest,
    Advanced,
}

const PAGES: [(Page, &str); 5] = [
    (Page::Card, "Graphics Card"),
    (Page::Sensors, "Sensors"),
    (Page::Advanced, "Advanced"),
    (Page::Verify, "Validation"),
    (Page::MemTest, "Memory Test"),
];

enum JobResult {
    Mem(MemTestResult),
    Compute(ComputeResult),
    Error(String),
}

struct Job {
    gpu: usize,
    mem: bool,
    prog: gputest::Shared,
    cancel: Arc<AtomicBool>,
    result: Arc<Mutex<Option<JobResult>>>,
}

type ShotSlot = Arc<Mutex<Option<Arc<egui::ColorImage>>>>;

struct Shot {
    prefix: String,
    idx: usize,
    t: std::time::Instant,
    requested: bool,
    slot: ShotSlot,
}

struct App {
    gpus: Vec<Gpu>,
    sel: usize,
    page: Page,
    dark: bool,
    pal: Pal,
    sampler: Sampler,
    mt: Vec<Option<MemTestResult>>,
    cr: Vec<Option<ComputeResult>>,
    job: Option<Job>,
    last_log: Vec<String>,
    last_map: (Vec<u8>, u64),
    mt_fraction: f64,
    mt_passes: u32,
    patterns: [bool; 6],
    stress: bool,
    heat: bool,
    continue_on_error: bool,
    graph_unit: String,
    logging: bool,
    status: String,
    logos: Vec<(u16, egui::TextureHandle)>,
    shot: Option<Shot>,
    adv_sec: usize,
    status_seen: Option<(String, std::time::Instant)>,
}

pub fn run() {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([530.0, 610.0])
            .with_min_inner_size([500.0, 560.0])
            .with_title("GPU-R"),
        ..Default::default()
    };
    if let Err(e) = eframe::run_native("GPU-R", opts, Box::new(|cc| Ok(Box::new(App::new(cc))))) {
        eprintln!("GUI error: {e}");
    }
}

fn load_logo(ctx: &egui::Context, name: &str, bytes: &[u8]) -> Option<egui::TextureHandle> {
    let img = image::load_from_memory_with_format(bytes, image::ImageFormat::Png).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    let ci = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], img.as_raw());
    Some(ctx.load_texture(name, ci, egui::TextureOptions::LINEAR))
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_fonts(&cc.egui_ctx);
        let cfg = Settings::load();
        let dark = std::env::var("GPUR_THEME").map(|t| t != "light").unwrap_or(cfg.dark);
        let lang = std::env::var("GPUR_LANG").ok().and_then(|l| i18n::from_code(&l)).or(cfg.lang.as_deref().and_then(i18n::from_code)).unwrap_or_else(i18n::detect);
        i18n::set(lang);
        let pal = if dark { DARK } else { LIGHT };
        apply_theme(&cc.egui_ctx, &pal, dark);

        let mut logos = vec![];
        for (vid, name, bytes) in [
            (0x1002u16, "amd", &include_bytes!("../assets/logos/amd.png")[..]),
            (0x10DE, "nvidia", &include_bytes!("../assets/logos/nvidia.png")[..]),
            (0x8086, "intel", &include_bytes!("../assets/logos/intel.png")[..]),
        ] {
            if let Some(t) = load_logo(&cc.egui_ctx, name, bytes) {
                logos.push((vid, t));
            }
        }

        let gpus = gpu::collect_all();
        let sel = gpus
            .iter()
            .position(|g| g.pci.driver.is_some() && !g.pci.boot_vga)
            .or_else(|| gpus.iter().position(|g| g.pci.driver.is_some()))
            .unwrap_or(0);
        let src = gpus.get(sel).map(crate::source_of).unwrap_or(crate::sensors::Source {
            pci_addr: String::new(),
            sysfs: Default::default(),
            driver: String::new(),
        });
        let ctx = cc.egui_ctx.clone();
        let sampler = Sampler::start(src, move || ctx.request_repaint());
        let n = gpus.len();
        let shot = std::env::var("GPUR_SHOT").ok().map(|prefix| Shot {
            prefix,
            idx: 0,
            t: std::time::Instant::now(),
            requested: false,
            slot: Default::default(),
        });
        Self {
            gpus,
            sel,
            page: Page::Card,
            dark,
            pal,
            sampler,
            mt: vec![None; n],
            cr: vec![None; n],
            job: None,
            last_log: vec![],
            last_map: (vec![], 256 << 20),
            mt_fraction: 0.90,
            mt_passes: 1,
            patterns: [true, true, true, true, true, false],
            stress: std::env::var("GPUR_STRESS").is_ok(),
            heat: true,
            continue_on_error: false,
            graph_unit: "°C".into(),
            logging: false,
            status: String::new(),
            logos,
            shot,
            adv_sec: 0,
            status_seen: None,
        }
    }

    fn gpu(&self) -> Option<&Gpu> {
        self.gpus.get(self.sel)
    }

    fn checks(&self) -> Vec<Check> {
        self.gpu().map(|g| g.checks(self.mt[self.sel].as_ref(), self.cr[self.sel].as_ref())).unwrap_or_default()
    }

    fn select(&mut self, i: usize) {
        if i == self.sel || i >= self.gpus.len() {
            return;
        }
        self.sel = i;
        *self.sampler.source.lock().unwrap() = crate::source_of(&self.gpus[i]);
        *self.sampler.history.lock().unwrap() = Default::default();
        self.logging = false;
    }

    fn rescan(&mut self) {
        let addr = self.gpu().map(|g| g.pci.addr.clone());
        self.gpus = gpu::collect_all();
        let n = self.gpus.len();
        self.mt.resize(n, None);
        self.cr.resize(n, None);
        if let Some(a) = addr {
            self.sel = self.gpus.iter().position(|g| g.pci.addr == a).unwrap_or(0);
        }
        self.status = tr("Hardware rescanned").into();
    }

    fn toggle_theme(&mut self, ctx: &egui::Context) {
        self.dark = !self.dark;
        self.pal = if self.dark { DARK } else { LIGHT };
        apply_theme(ctx, &self.pal, self.dark);
        self.save_settings();
    }

    fn save_settings(&self) {
        Settings { dark: self.dark, lang: Some(i18n::code(i18n::get()).to_string()) }.save();
    }

    fn poll_job(&mut self) {
        let Some(job) = &self.job else { return };
        {
            let p = job.prog.lock().unwrap();
            self.last_log = p.log.clone();
            if job.mem {
                self.last_map = (p.chunks.clone(), p.chunk_bytes.max(1));
            }
        }
        let res = job.result.lock().unwrap().take();
        if let Some(r) = res {
            let g = job.gpu;
            match r {
                JobResult::Mem(m) => {
                    self.status = tr(if m.errors > 0 { "Memory test found errors" } else { "Memory test finished" }).into();
                    self.mt[g] = Some(m);
                }
                JobResult::Compute(c) => {
                    self.status = tr("FP32 benchmark finished").into();
                    self.cr[g] = Some(c);
                }
                JobResult::Error(e) => self.status = trf("Test could not start: {}", &[&e]),
            }
            self.job = None;
        }
    }

    fn start_job(&mut self, ctx: &egui::Context, mem: bool) {
        let Some(g) = self.gpu() else { return };
        let (vendor, device) = (g.pci.vendor, g.pci.device);
        let src = crate::source_of(g);
        let reported = g.vram_bytes().unwrap_or(0);
        let claimed = g.shaders().or(g.db.as_ref().map(|d| d.spec.shaders));
        let patterns: Vec<u32> = (0..6u32).filter(|&i| self.patterns[i as usize]).collect();
        let (fraction, passes) = (self.mt_fraction, self.mt_passes);
        let (stress, heat, continue_on_error) = (self.stress, self.stress && self.heat, self.continue_on_error);
        let log_path = stress.then(|| {
            let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            home_dir().join(format!("gpu-r-stress-{ts}.log"))
        });
        let job = Job {
            gpu: self.sel,
            mem,
            prog: Default::default(),
            cancel: Arc::new(AtomicBool::new(false)),
            result: Default::default(),
        };
        let (prog, cancel, result) = (job.prog.clone(), job.cancel.clone(), job.result.clone());
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let r = match gputest::open(vendor, device) {
                Err(e) => JobResult::Error(e),
                Ok(c) if mem => {
                    let opt = gputest::MemTestOptions {
                        reported_vram: reported,
                        used_vram: src.vram_used(),
                        fraction,
                        passes,
                        patterns: if patterns.is_empty() { vec![0] } else { patterns },
                        stress,
                        continue_on_error,
                        heat,
                        log_path,
                    };
                    JobResult::Mem(gputest::run_memtest(&c, &opt, &|| src.vram_used(), &|| crate::sensors_line(&src), &prog, &cancel))
                }
                Ok(c) => JobResult::Compute(gputest::run_compute(&c, claimed, &|| src.gpu_clock(), &prog)),
            };
            *result.lock().unwrap() = Some(r);
            ctx.request_repaint();
        });
        self.last_log.clear();
        if mem {
            self.last_map = (vec![], 256 << 20);
        }
        self.job = Some(job);
    }

    fn save_report(&mut self) {
        let Some(g) = self.gpu() else { return };
        let mt = self.mt[self.sel].as_ref();
        let cr = self.cr[self.sel].as_ref();
        let v = serde_json::json!({
            "gpu": g, "fields": g.fields(mt, cr), "checks": g.checks(mt, cr), "memtest": mt, "compute": cr,
        });
        let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let path = home_dir().join(format!("gpu-r-report-{ts}.json"));
        self.status = match std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap()) {
            Ok(_) => trf("Report saved to {}", &[&path.display()]),
            Err(e) => trf("Report not saved: {}", &[&e]),
        };
    }

    /// GPUR_SHOT=/prefix — screenshot every page, then quit (layout checks).
    fn debug_shot(&mut self, ctx: &egui::Context) {
        let Some(s) = self.shot.as_mut() else { return };
        if s.idx >= PAGES.len() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        self.page = PAGES[s.idx].0;
        if let Some(img) = s.slot.lock().unwrap().take() {
            let mut ppm = format!("P6\n{} {}\n255\n", img.size[0], img.size[1]).into_bytes();
            for px in &img.pixels {
                ppm.extend_from_slice(&[px.r(), px.g(), px.b()]);
            }
            let _ = std::fs::write(format!("{}{}.ppm", s.prefix, s.idx), ppm);
            s.idx += 1;
            s.t = std::time::Instant::now();
            s.requested = false;
        } else if !s.requested && s.t.elapsed().as_secs_f32() > if s.idx == 0 { 6.0 } else { 1.2 } {
            let slot = s.slot.clone();
            let c2 = ctx.clone();
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::ScreenshotCallback::new(move |img| {
                *slot.lock().unwrap() = Some(img);
                c2.request_repaint();
            })));
            s.requested = true;
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}

// ---------------------------------------------------------------- frame

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_job();
        self.debug_shot(&ctx);
        if self.job.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(120));
        }
        let p = self.pal;

        egui::Panel::top("tabs")
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(p.chrome).inner_margin(egui::Margin { left: 6, right: 6, top: 6, bottom: 0 }))
            .show(ui, |ui| self.tab_bar(ui, &ctx));
        egui::Panel::bottom("bar")
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(p.chrome).inner_margin(egui::Margin::symmetric(8, 6)))
            .show(ui, |ui| self.bottom_bar(ui, &ctx));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p.bg).inner_margin(egui::Margin { left: 8, right: 8, top: 8, bottom: 6 }))
            .show(ui, |ui| {
                if self.gpus.is_empty() {
                    ui.label(RichText::new(tr("No graphics adapter found on the PCI bus.")).color(p.text));
                    return;
                }
                let w = ui.available_width() - 6.0;
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    ui.set_width(w);
                    match self.page {
                        Page::Card => self.page_card(ui),
                        Page::Sensors => self.page_sensors(ui),
                        Page::Verify => self.page_verify(ui),
                        Page::MemTest => self.page_memtest(ui, &ctx),
                        Page::Advanced => self.page_advanced(ui),
                    }
                });
            });
    }
}

impl App {
    fn tab_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let p = self.pal;
        let checks = self.checks();
        let worst = if checks.iter().any(|c| c.status == Status::Fail) {
            Some(Status::Fail)
        } else if checks.iter().any(|c| c.status == Status::Warn) {
            Some(Status::Warn)
        } else {
            None
        };
        let job_frac = self.job.as_ref().map(|j| j.prog.lock().unwrap().frac);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 1.0;
            // fit the tabs next to the four tool buttons: shrink font and padding for long translations
            let avail = ui.available_width() - 150.0;
            let natural: f32 = PAGES
                .iter()
                .map(|(_, n)| ui.painter().layout_no_wrap(tr(n).to_string(), FontId::new(12.5, medium()), p.text).size().x + 17.0)
                .sum::<f32>()
                + 10.0;
            let (fsize, pad) = if natural <= avail { (12.5, 16.0) } else if natural - 25.0 <= avail { (12.5, 11.0) } else { (11.5, 9.0) };
            for (page, name) in PAGES {
                let sel = self.page == page;
                let col = if sel { p.text } else { p.muted };
                let label = match (page, job_frac) {
                    (Page::MemTest, Some(f)) => format!("{} {:.0}%", tr(name), f * 100.0),
                    _ => tr(name).to_string(),
                };
                let font = FontId::new(fsize, if sel { medium() } else { FontFamily::Proportional });
                let galley = ui.painter().layout_no_wrap(label, font, col);
                let dot = page == Page::Verify && worst.is_some();
                let size = Vec2::new(galley.size().x + pad + if dot { 10.0 } else { 0.0 }, 27.0);
                let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
                let painter = ui.painter();
                if sel {
                    painter.rect_filled(rect, egui::CornerRadius { nw: 5, ne: 5, sw: 0, se: 0 }, p.bg);
                    painter.rect_filled(Rect::from_min_size(rect.min + Vec2::new(4.0, 0.0), Vec2::new(rect.width() - 8.0, 2.0)), 1.0, p.accent);
                } else if resp.hovered() {
                    painter.rect_filled(rect.shrink2(Vec2::new(0.0, 3.0)), 4.0, p.raised);
                }
                let gh = galley.size().y;
                painter.galley(Pos2::new(rect.left() + pad / 2.0, rect.center().y - gh / 2.0 + 1.0), galley, col);
                if let (true, Some(w)) = (dot, worst) {
                    painter.circle_filled(Pos2::new(rect.right() - 11.0, rect.center().y + 1.0), 3.5, status_col(&p, w));
                }
                if resp.clicked() {
                    self.page = page;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let tool = |ui: &mut egui::Ui, icon: &str, tip: &str| {
                    ui.add(egui::Button::new(RichText::new(icon).size(13.5).color(p.text)).min_size(Vec2::new(26.0, 22.0)).corner_radius(4))
                        .on_hover_text(tr(tip))
                        .clicked()
                };
                let (icon, tip) = if self.dark { ("☀", "Switch to light theme") } else { ("☾", "Switch to dark theme") };
                if tool(ui, icon, tip) {
                    self.toggle_theme(ctx);
                }
                let cur = i18n::get();
                let mut picked = cur;
                egui::ComboBox::from_id_salt("lang")
                    .width(44.0)
                    .selected_text(RichText::new(i18n::code(cur).to_uppercase()).size(12.0))
                    .show_ui(ui, |ui| {
                        for (l, _, n) in i18n::ALL {
                            ui.selectable_value(&mut picked, l, n);
                        }
                    })
                    .response
                    .on_hover_text(tr("Language"));
                if picked != cur {
                    i18n::set(picked);
                    self.save_settings();
                }
                if tool(ui, "⟳", "Rescan hardware") {
                    self.rescan();
                }
                if tool(ui, "🖫", "Save a JSON report to your home folder") {
                    self.save_report();
                }
            });
        });
    }

    fn bottom_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let p = self.pal;
        if self.status_seen.as_ref().map(|s| &s.0) != Some(&self.status) {
            self.status_seen = Some((self.status.clone(), std::time::Instant::now()));
        }
        if let Some((s, t)) = &self.status_seen {
            if !s.is_empty() && t.elapsed().as_secs() < 10 {
                ui.label(RichText::new(s).size(11.0).color(p.muted));
                ctx.request_repaint_after(std::time::Duration::from_secs(1));
            }
        }
        ui.horizontal(|ui| {
            let close_w = 74.0;
            if !self.gpus.is_empty() {
                let names: Vec<String> = self.gpus.iter().map(|g| g.title()).collect();
                let mut sel = self.sel;
                egui::ComboBox::from_id_salt("gpu_sel")
                    .width(ui.available_width() - close_w - 10.0)
                    .selected_text(names[sel].clone())
                    .show_ui(ui, |ui| {
                        for (i, n) in names.iter().enumerate() {
                            ui.selectable_value(&mut sel, i, format!("{n}  ({})", self.gpus[i].pci.addr));
                        }
                    });
                self.select(sel);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add_sized([close_w, 24.0], egui::Button::new(tr("Close"))).clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        });
    }

    // ------------------------------------------------------------ graphics card

    fn page_card(&mut self, ui: &mut egui::Ui) {
        let p = self.pal;
        let g = &self.gpus[self.sel];
        let fields = g.fields(self.mt[self.sel].as_ref(), self.cr[self.sel].as_ref());
        let get = |k: &str| fields.iter().find(|(n, _)| *n == k).map(|x| x.1.clone());
        let info = |v: String, src: &str| Some(Field::new(v, Status::Info, src));
        let checks = self.checks();
        ui.spacing_mut().item_spacing = Vec2::new(4.0, 5.0);

        // Name + verdict (where GPU-Z has "Lookup")
        let (st, word) = verdict(&checks);
        let vcol = status_col(&p, st);
        let mut go_verify = false;
        ui.horizontal(|ui| {
            let total = ui.available_width();
            let btn_w = 104.0;
            let (rect, _) = ui.allocate_exact_size(Vec2::new(total - btn_w - 6.0, ROW_H), Sense::hover());
            cells_in(ui, &p, rect, &[("Name", get("Name"), 1.0)]);
            let b = egui::Button::new(RichText::new(tr(word)).size(11.5).color(vcol))
                .fill(vcol.gamma_multiply(0.13))
                .stroke(Stroke::new(1.0, vcol.gamma_multiply(0.6)));
            if ui.add_sized([btn_w, ROW_H], b).on_hover_text(tr(verdict_sentence(st))).clicked() {
                go_verify = true;
            }
        });

        // three rows beside the vendor logo
        let logo = self.logos.iter().find(|(v, _)| *v == g.pci.vendor).map(|x| x.1.clone());
        let reserve = if logo.is_some() { 104.0 } else { 0.0 };
        let left = ui.cursor().left();
        let full_w = ui.available_width();
        let top = ui.cursor().top();
        row(ui, &p, reserve, &[("GPU", get("GPU"), 1.15), ("Revision", get("Revision"), 0.85)]);
        row(ui, &p, reserve, &[("Technology", get("Technology"), 1.15), ("Die Size", get("Die Size"), 0.85)]);
        row(ui, &p, reserve, &[("Release Date", get("Release Date"), 1.15), ("Transistors", get("Transistors"), 0.85)]);
        let bottom = ui.cursor().top() - 5.0;
        if let Some(t) = logo {
            let area = Rect::from_min_max(Pos2::new(left + full_w - reserve + 10.0, top), Pos2::new(left + full_w, bottom));
            let sz = t.size_vec2();
            let s = (area.width() / sz.x).min(area.height() / sz.y);
            let r = Rect::from_center_size(area.center(), sz * s);
            ui.painter().image(t.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        }

        // BIOS version + UEFI checkbox
        {
            let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::hover());
            let chk_w = 64.0;
            let main = Rect::from_min_max(rect.min, Pos2::new(rect.right() - chk_w - 8.0, rect.bottom()));
            cells_in(ui, &p, main, &[("BIOS Version", get("BIOS Version"), 1.0)]);
            let uefi = get("UEFI");
            let state = uefi.as_ref().map(|f| {
                if f.value.starts_with("Yes") { Some(true) } else if f.value.starts_with("No") { Some(false) } else { None }
            }).unwrap_or(None);
            let cr = Rect::from_min_max(Pos2::new(rect.right() - chk_w, rect.top()), rect.right_bottom());
            let resp = check_mark(ui, &p, cr, "UEFI", state);
            if let Some(f) = uefi {
                resp.on_hover_text(evidence_tip(&f));
            }
        }
        row(ui, &p, 0.0, &[("Subvendor", get("Subvendor"), 0.8), ("Device ID", get("Device ID"), 1.2)]);
        row(ui, &p, 0.0, &[("ROPs/TMUs", get("ROPs/TMUs"), 0.8), ("Bus Interface", get("Bus Interface"), 1.2)]);
        let vk_api = g.vk.as_ref().map(|v| {
            let info = v.driver_info.split('-').next().unwrap_or("").trim();
            format!("{} ({})", info, v.driver).trim().to_string()
        });
        row(ui, &p, 0.0, &[("Shaders", get("Shaders"), 0.8), ("Vulkan", vk_api.and_then(|v| info(v, "Vulkan API version")).or(get("Vulkan")), 1.2)]);
        row(ui, &p, 0.0, &[("Pixel Fillrate", get("Pixel Fillrate"), 1.0), ("Texture Fillrate", get("Texture Fillrate"), 1.0)]);
        row(ui, &p, 0.0, &[("Memory Type", get("Memory Type"), 1.0), ("Bus Width", get("Bus Width"), 1.0)]);
        row(ui, &p, 0.0, &[("Memory Size", get("Memory Size"), 1.0), ("Bandwidth", get("Bandwidth"), 1.0)]);
        row(ui, &p, 0.0, &[("Driver Version", get("Driver Version"), 1.0)]);
        row(ui, &p, 0.0, &[("Driver Files", get("Driver Integrity"), 1.0)]);

        // clocks: live / reference
        let (now_gpu, now_mem) = {
            let h = self.sampler.history.lock().unwrap();
            let last = |n: &str| h.series.get(n).and_then(|s| s.last());
            (last("GPU Clock"), last("Memory Clock"))
        };
        let mhz = |v: Option<f64>, src: &str| info(v.map(|x| format!("{x:.0} MHz")).unwrap_or("N/A".into()), src);
        row(ui, &p, 0.0, &[
            ("GPU Clock", mhz(now_gpu, "live sensor"), 1.0),
            ("Memory", mhz(now_mem, "live sensor"), 1.0),
            ("Boost", mhz(g.max_gpu_clock().map(|x| x as f64), "driver DPM table (highest level)"), 1.0),
        ]);
        let spec = g.db.as_ref().map(|d| &d.spec);
        row(ui, &p, 0.0, &[
            ("Reference", spec.and_then(|s| info(s.name.trim_start_matches("Radeon ").trim_start_matches("GeForce ").to_string(), "reference DB")), 1.0),
            ("Memory", spec.and_then(|s| info(format!("{:.1} Gbps", s.mem_gbps), "reference DB")), 1.0),
            ("Boost", get("Reference Boost"), 1.0),
        ]);
        row(ui, &p, 0.0, &[("Max Memory", get("Memory Clock"), 1.0), ("Resizable BAR", get("Resizable BAR"), 0.8)]);

        // checkbox rows
        let comp = get("Computing").map(|f| f.value).unwrap_or_default();
        let has = |k: &str| comp.contains(k);
        checks_row(ui, &p, "Computing", &[
            ("Vulkan", Some(has("Vulkan"))),
            ("OpenCL", Some(has("OpenCL"))),
            ("ROCm/HIP", Some(has("ROCm"))),
            ("CUDA", Some(has("CUDA"))),
        ]);
        let rt = g.amd.as_ref().and_then(|a| a.gc_ip).map(|v| (v.0, v.1) >= (10, 3))
            .or(g.nv.as_ref().and_then(|n| n.cuda_cc).map(|c| c >= (7, 5)));
        let rebar = get("Resizable BAR").map(|f| f.value.starts_with("Enabled"));
        checks_row(ui, &p, "Technologies", &[
            ("Ray Tracing", rt),
            ("ReBAR", rebar),
            (if cfg!(windows) { "Admin rights" } else { "Root access" }, Some(crate::util::is_root())),
        ]);

        ui.add_space(4.0);
        legend(ui, &p);
        if go_verify {
            self.page = Page::Verify;
        }
    }

    // ------------------------------------------------------------ sensors

    fn page_sensors(&mut self, ui: &mut egui::Ui) {
        let p = self.pal;
        let hist = self.sampler.history.clone();
        let mut h = hist.lock().unwrap();
        ui.spacing_mut().item_spacing = Vec2::new(4.0, 3.0);

        let lines: Vec<(String, Vec<[f64; 2]>)> = h
            .order
            .iter()
            .filter_map(|n| h.series.get(n).filter(|s| s.unit == self.graph_unit).map(|s| (n.clone(), s.points.clone())))
            .collect();
        egui::Frame::new().fill(p.well).stroke(Stroke::new(1.0, p.line)).corner_radius(3).inner_margin(4).show(ui, |ui| {
            Plot::new("big")
                .height(150.0)
                .legend(Legend::default().position(egui_plot::Corner::LeftTop))
                .allow_scroll(false)
                .allow_zoom(false)
                .allow_drag(false)
                .allow_boxed_zoom(false)
                .include_y(0.0)
                .show_background(false)
                .y_axis_label(format!("{} ({})", tr(unit_title(&self.graph_unit)), self.graph_unit))
                .show(ui, |pu| {
                    for (i, (n, pts)) in lines.into_iter().enumerate() {
                        let c = p.series[i % p.series.len()];
                        pu.line(Line::new(tr(&n), PlotPoints::from(pts)).color(c).width(1.8).fill(0.0).fill_alpha(0.08));
                    }
                });
        });
        ui.add_space(4.0);

        let order = h.order.clone();
        let mut clicked = None;
        for name in &order {
            let Some(s) = h.series.get(name) else { continue };
            let active = s.unit == self.graph_unit;
            let color = if active {
                let idx = order.iter().filter(|n| h.series.get(*n).is_some_and(|x| x.unit == s.unit)).position(|n| n == name).unwrap_or(0);
                p.series[idx % p.series.len()]
            } else {
                p.bad.gamma_multiply(0.85)
            };
            if sensor_row(ui, &p, name, s, color, active).clicked() {
                clicked = Some(s.unit.clone());
            }
        }
        if let Some(u) = clicked {
            self.graph_unit = u;
        }

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr("Refresh")).color(p.muted));
            let mut ms = *self.sampler.interval_ms.lock().unwrap();
            egui::ComboBox::from_id_salt("int").selected_text(format!("{:.2} s", ms as f64 / 1000.0)).width(70.0).show_ui(ui, |ui| {
                for v in [250u64, 500, 1000, 2000, 5000] {
                    ui.selectable_value(&mut ms, v, format!("{:.2} s", v as f64 / 1000.0));
                }
            });
            *self.sampler.interval_ms.lock().unwrap() = ms;
            if ui.checkbox(&mut self.logging, tr("Log to file")).changed() {
                if self.logging {
                    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                    let path = home_dir().join(format!("gpu-r-sensors-{ts}.csv"));
                    h.log_file = std::fs::File::create(&path).ok();
                    h.log_header_written = false;
                    self.status = trf("Logging sensors to {}", &[&path.display()]);
                } else {
                    h.log_file = None;
                    self.status = tr("Sensor logging stopped").into();
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr("Reset")).on_hover_text(tr("Reset min / max / average")).clicked() {
                    h.reset_stats();
                }
            });
        });
        ui.label(RichText::new(tr("Click a sensor to plot its group above. Hover for min, max and average.")).size(11.0).color(p.muted));
    }

    // ------------------------------------------------------------ validation

    fn page_verify(&mut self, ui: &mut egui::Ui) {
        let p = self.pal;
        let checks = self.checks();
        let count = |s: Status| checks.iter().filter(|c| c.status == s).count();
        let (st, word) = verdict(&checks);
        let col = status_col(&p, st);
        ui.spacing_mut().item_spacing = Vec2::new(4.0, 3.0);

        egui::Frame::new()
            .fill(col.gamma_multiply(0.10))
            .stroke(Stroke::new(1.0, col.gamma_multiply(0.55)))
            .corner_radius(4)
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(tr(word)).family(semi()).size(17.0).color(col));
                ui.add(egui::Label::new(RichText::new(tr(verdict_sentence(st))).size(11.5).color(p.muted)).wrap());
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    for (n, label, s) in [
                        (count(Status::Ok), "passed", Status::Ok),
                        (count(Status::Warn), "differ", Status::Warn),
                        (count(Status::Fail), "contradict", Status::Fail),
                        (count(Status::Skipped), "not run", Status::Skipped),
                    ] {
                        let c = if n == 0 { p.muted } else { status_col(&p, s) };
                        ui.label(RichText::new(n.to_string()).family(semi()).size(13.0).color(c));
                        ui.label(RichText::new(tr(label)).size(11.5).color(p.muted));
                        ui.add_space(8.0);
                    }
                });
            });
        if cfg!(windows) {
            ui.add(egui::Label::new(RichText::new(tr("On Windows the BIOS chip and driver files cannot be read; identity, VRAM and the GPU tests still work.")).size(11.0).color(p.muted)).wrap());
        } else if !crate::util::is_root() {
            ui.add(egui::Label::new(RichText::new(tr("Run as root to also read the physical BIOS chip and the full PCI config space.")).size(11.0).color(p.muted)).wrap());
        }
        ui.add_space(4.0);

        let mut groups: Vec<&str> = vec![];
        for c in &checks {
            if !groups.contains(&c.group) {
                groups.push(c.group);
            }
        }
        for gname in groups {
            group_header(ui, &p, tr(group_title(gname)));
            for c in checks.iter().filter(|c| c.group == gname) {
                check_row(ui, &p, c);
            }
        }
    }

    // ------------------------------------------------------------ memory test

    fn page_memtest(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let p = self.pal;
        let running = self.job.is_some();
        let (frac, phase, errs, is_mem) = self
            .job
            .as_ref()
            .map(|j| {
                let g = j.prog.lock().unwrap();
                (g.frac, g.phase.clone(), g.errors, j.mem)
            })
            .unwrap_or((0.0, String::new(), 0, true));
        let last = self.mt[self.sel].clone();
        ui.spacing_mut().item_spacing = Vec2::new(5.0, 5.0);

        ui.add(egui::Label::new(RichText::new(tr(
            "Fills free VRAM with patterns unique to each block, then reads it back. Missing chips show up as blocks that overwrite each other, failing chips as flipped bits. Close games and 3D apps first."))
            .size(11.5).color(p.muted)).wrap());
        ui.add_space(2.0);
        ui.add_enabled_ui(!running, |ui| {
            egui::Grid::new("mt_opts").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                ui.label(RichText::new(tr("Free VRAM")).color(p.muted));
                ui.horizontal(|ui| {
                    ui.add(egui::Slider::new(&mut self.mt_fraction, 0.1..=0.98).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)));
                    ui.label(RichText::new(tr("Passes")).color(p.muted));
                    ui.add_enabled(!self.stress, egui::DragValue::new(&mut self.mt_passes).range(1..=100));
                });
                ui.end_row();
                ui.label(RichText::new(tr("Mode")).color(p.muted));
                ui.horizontal_wrapped(|ui| {
                    ui.radio_value(&mut self.stress, false, tr("Single run"));
                    ui.radio_value(&mut self.stress, true, tr("Stress test"))
                        .on_hover_text(tr("Repeats all patterns until you press Stop, an error appears or the GPU hangs. Each pass is written to a log file in your home folder, so the result survives a system freeze."));
                });
                ui.end_row();
                if self.stress {
                    ui.label("");
                    ui.horizontal_wrapped(|ui| {
                        ui.checkbox(&mut self.heat, tr("Load shaders too (heats the core)"));
                        ui.checkbox(&mut self.continue_on_error, tr("Keep going after errors"));
                    });
                    ui.end_row();
                }
                ui.label(RichText::new(tr("Patterns")).color(p.muted));
                ui.horizontal_wrapped(|ui| {
                    for (i, n) in ["address", "inverted", "random", "walking 1s", "checker", "solid"].iter().enumerate() {
                        ui.checkbox(&mut self.patterns[i], tr(n));
                    }
                });
                ui.end_row();
            });
        });
        ui.horizontal(|ui| {
            if running {
                if ui.button(tr("Stop")).clicked() {
                    if let Some(j) = &self.job {
                        j.cancel.store(true, Ordering::Relaxed);
                    }
                }
            } else {
                let start = egui::Button::new(RichText::new(tr(if self.stress { "Start stress test" } else { "Start memory test" })).family(medium()).color(if self.dark { p.bg } else { Color32::WHITE })).fill(p.accent);
                if ui.add(start).clicked() {
                    self.start_job(ctx, true);
                }
                if ui.button(tr("FP32 benchmark")).on_hover_text(tr("Measures real shader throughput to confirm the shader count")).clicked() {
                    self.start_job(ctx, false);
                }
            }
        });

        // progress
        let bar_col = if errs > 0 { p.bad } else { p.accent };
        let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 6.0), Sense::hover());
        ui.painter().rect_filled(r, 3.0, p.well);
        ui.painter().rect_stroke(r, 3.0, Stroke::new(1.0, p.line), egui::StrokeKind::Inside);
        let shown = if running { frac } else if last.is_some() { 1.0 } else { 0.0 };
        if shown > 0.0 {
            let done_col = if running { bar_col } else if last.as_ref().is_some_and(|m| m.errors > 0) { p.bad } else { p.ok };
            ui.painter().rect_filled(Rect::from_min_size(r.min, Vec2::new(r.width() * shown, r.height())), 3.0, done_col);
        }
        let line = if running {
            format!("{:.0}%  {}{}", frac * 100.0, if is_mem { "" } else { "benchmark: " }, phase)
        } else if let Some(m) = &last {
            if m.errors > 0 { trf("Last run: {} errors", &[&m.errors]) } else { tr("Last run: no errors").into() }
        } else {
            tr("Not run yet").into()
        };
        ui.label(RichText::new(line).size(11.5).color(if errs > 0 { p.bad } else { p.muted }));

        // stress status: elapsed, passes, hang watchdog
        let stress_info = self.job.as_ref().filter(|j| j.mem).map(|j| {
            let g = j.prog.lock().unwrap();
            (g.started.map(|t| t.elapsed().as_secs()), g.pass, g.heartbeat.map(|t| t.elapsed().as_secs()), g.device_lost.clone())
        });
        if let (true, Some((el, pass, hb, lost))) = (self.stress, stress_info) {
            let el = el.unwrap_or(0);
            egui::Frame::new().fill(p.well).stroke(Stroke::new(1.0, p.line)).corner_radius(3).inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    for (k, v, c) in [
                        (tr("Time"), format!("{:02}:{:02}:{:02}", el / 3600, el / 60 % 60, el % 60), p.text),
                        (tr("Pass"), pass.to_string(), p.text),
                        (tr("errors"), errs.to_string(), if errs > 0 { p.bad } else { p.ok }),
                    ] {
                        ui.label(RichText::new(k).size(11.5).color(p.muted));
                        ui.label(RichText::new(v).family(semi()).size(14.0).color(c));
                        ui.add_space(12.0);
                    }
                });
                if let Some(l) = lost {
                    ui.label(RichText::new(trf("GPU lost by the driver (hang or reset): {}", &[&l])).size(11.5).color(p.bad));
                } else if let Some(h) = hb.filter(|&h| h >= 20) {
                    ui.label(RichText::new(trf("GPU has not responded for {} s: it may have hung", &[&h])).size(11.5).color(p.warn));
                }
            });
        }

        // VRAM map
        let g = &self.gpus[self.sel];
        let chunk = self.last_map.1.max(1);
        let total_blocks = (g.vram_bytes().unwrap_or(0) / chunk).clamp(1, 1024) as usize;
        let states = &self.last_map.0;
        group_header(ui, &p, &trf("VRAM map, {} MB per block of {}", &[&(chunk >> 20), &crate::util::fmt_bytes(g.vram_bytes().unwrap_or(0))]));
        let (sq, gap) = (13.0, 3.0);
        let per_row = ((ui.available_width() + gap) / (sq + gap)).floor().max(1.0) as usize;
        let rows = total_blocks.div_ceil(per_row);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), rows as f32 * (sq + gap)), Sense::hover());
        for i in 0..total_blocks {
            let r = Rect::from_min_size(rect.min + Vec2::new((i % per_row) as f32 * (sq + gap), (i / per_row) as f32 * (sq + gap)), Vec2::splat(sq));
            let painter = ui.painter();
            match states.get(i).copied().unwrap_or(0) {
                0 => {
                    painter.rect_stroke(r, 2.0, Stroke::new(1.0, p.line), egui::StrokeKind::Inside);
                }
                1 => {
                    painter.rect_filled(r, 2.0, p.raised);
                }
                2 => {
                    painter.rect_filled(r, 2.0, p.accent.gamma_multiply(0.55));
                }
                3 => {
                    painter.rect_filled(r, 2.0, p.ok);
                }
                _ => {
                    painter.rect_filled(r, 2.0, p.bad);
                }
            }
        }
        ui.horizontal_wrapped(|ui| {
            for (c, n, outline) in [
                (p.line, "untested", true),
                (p.raised, "allocated", false),
                (p.accent.gamma_multiply(0.55), "written", false),
                (p.ok, "verified", false),
                (p.bad, "errors", false),
            ] {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                if outline {
                    ui.painter().rect_stroke(r, 2.0, Stroke::new(1.0, c), egui::StrokeKind::Inside);
                } else {
                    ui.painter().rect_filled(r, 2.0, c);
                }
                ui.label(RichText::new(tr(n)).size(11.0).color(p.muted));
                ui.add_space(4.0);
            }
        });

        if let Some(m) = &last {
            let st = if m.errors > 0 { Status::Fail } else if m.verified_capacity() { Status::Ok } else { Status::Warn };
            let st = if m.device_lost.is_some() { Status::Fail } else if m.stress && m.errors == 0 { Status::Ok } else { st };
            result_panel(ui, &p, st, tr(if m.stress { "Stress test" } else { "Memory test" }), &m.summary());
            if let Some(f) = &m.log_file {
                ui.label(RichText::new(trf("Log: {}", &[f])).size(11.0).color(p.muted));
            }
            if !m.error_samples.is_empty() {
                egui::CollapsingHeader::new(tr("Failing addresses")).show(ui, |ui| {
                    for (a, x) in &m.error_samples {
                        ui.monospace(format!("offset {:#014x}  bits {:#010x}", a, x));
                    }
                });
            }
        }
        if let Some(c) = &self.cr[self.sel] {
            let st = match c.verdict_ok {
                Some(true) => Status::Ok,
                Some(false) => Status::Warn,
                None => Status::Info,
            };
            result_panel(ui, &p, st, tr("FP32 benchmark"), &c.summary());
        }
        if !self.last_log.is_empty() {
            egui::CollapsingHeader::new(tr("Test log")).show(ui, |ui| {
                for l in &self.last_log {
                    ui.label(RichText::new(l).family(FontFamily::Monospace).size(11.0));
                }
            });
        }
    }

    // ------------------------------------------------------------ advanced

    fn page_advanced(&mut self, ui: &mut egui::Ui) {
        let p = self.pal;
        let g = &self.gpus[self.sel];
        let secs = advanced_sections(g);
        let mut names: Vec<String> = vec![];
        if g.amd.is_some() {
            names.push("Compute units".into());
        }
        names.extend(secs.iter().map(|s| s.0.clone()));
        if self.adv_sec >= names.len() {
            self.adv_sec = 0;
        }
        ui.spacing_mut().item_spacing = Vec2::new(4.0, 4.0);
        let mut sel = self.adv_sec;
        egui::ComboBox::from_id_salt("adv").width(ui.available_width() - 6.0).selected_text(tr(&names[sel])).show_ui(ui, |ui| {
            for (i, n) in names.iter().enumerate() {
                ui.selectable_value(&mut sel, i, tr(n));
            }
        });
        self.adv_sec = sel;
        ui.add_space(4.0);
        let name = names[sel].as_str();
        if name == "Compute units" {
            if let Some(a) = &g.amd {
                ui.add(egui::Label::new(RichText::new(trf(
                    "{} shader engines, {} arrays each, {} CU slots per array. {} of {} CUs active, read from the hardware harvest registers.",
                    &[&a.num_se, &a.num_sh_per_se, &a.num_cu_per_sh, &a.cu_active_number, &a.cu_slots()],
                )).size(11.5).color(p.muted)).wrap());
                ui.add_space(4.0);
                let sq = ((ui.available_width() - 40.0 - a.num_sh_per_se.min(4) as f32 * 8.0) / (a.num_sh_per_se.min(4) * a.num_cu_per_sh.min(32)).max(1) as f32).min(16.0);
                for se in 0..a.num_se.min(8) as usize {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.add_sized([40.0, sq], egui::Label::new(RichText::new(format!("SE{se}")).size(11.0).color(p.muted)));
                        for sh in 0..a.num_sh_per_se.min(4) as usize {
                            let mask = a.cu_bitmap.get(se).and_then(|r| r.get(sh)).copied().unwrap_or(0);
                            for cu in 0..a.num_cu_per_sh.min(32) {
                                let on = mask & (1 << cu) != 0;
                                let (r, resp) = ui.allocate_exact_size(Vec2::splat(sq), Sense::hover());
                                if on {
                                    ui.painter().rect_filled(r.shrink(1.5), 2.0, p.ok);
                                } else {
                                    ui.painter().rect_stroke(r.shrink(2.0), 2.0, Stroke::new(1.0, p.skip), egui::StrokeKind::Inside);
                                }
                                resp.on_hover_text(format!("SE{se} SH{sh} CU{cu}: {}", tr(if on { "active" } else { "disabled (harvested or fused off)" })));
                            }
                            ui.add_space(8.0);
                        }
                    });
                }
            }
            return;
        }
        if let Some((_, rows)) = secs.iter().find(|s| s.0 == name) {
            for (i, (k, v)) in rows.iter().enumerate() {
                kv(ui, &p, i, k, v);
            }
        }
    }
}

fn advanced_sections(g: &Gpu) -> Vec<(String, Vec<(String, String)>)> {
    let mut out: Vec<(String, Vec<(String, String)>)> = vec![];
    let s = |x: &str| x.to_string();
    if let Some(a) = &g.amd {
        let mut v = vec![
            (s("DRM"), a.drm_version.clone().unwrap_or_default()),
            (s("Graphics core IP"), a.gc_ip.map(|v| format!("{}.{}.{}", v.0, v.1, v.2)).unwrap_or("-".into())),
            (s("Family / chip rev / ext rev"), format!("{} / {} / {}", a.family, a.chip_rev, a.external_rev)),
            (s("Render backends"), format!("{} ({} ROPs), mask {:#x}", a.num_rb_pipes, a.rops(), a.enabled_rb_mask)),
            (s("Infinity Cache"), format!("{} MB", a.mall_size >> 20)),
            (s("VRAM total / usable / CPU-visible"), format!("{} / {} / {} MB", a.sysfs_vram_total.unwrap_or(0) >> 20, a.ioctl_vram_size >> 20, a.ioctl_vram_cpu_visible >> 20)),
            (s("GTT"), format!("{} MB", a.ioctl_gtt_size >> 20)),
            (s("GPU clock levels"), format!("{:?} MHz", a.sclk_levels)),
            (s("Memory clock levels"), format!("{:?} MHz", a.mclk_levels)),
            (s("VBIOS name"), a.vbios_name.clone().unwrap_or_default()),
            (s("VBIOS date"), a.vbios_date.clone().unwrap_or_default()),
        ];
        for (k, val) in &a.module_params {
            v.push((format!("amdgpu.{k}"), val.clone()));
        }
        out.push((s("amdgpu driver"), v));
        out.push((s("Firmware"), a.fw_versions.iter().map(|(k, v)| (k.clone(), v.clone())).collect()));
    }
    if let Some(n) = &g.nv {
        out.push((s("NVML"), vec![
            (s("Name"), n.name.clone().unwrap_or_default()),
            (s("Driver / NVML"), format!("{} / {}", n.driver_version.clone().unwrap_or_default(), n.nvml_version.clone().unwrap_or_default())),
            (s("CUDA cores"), n.cores.map(|x| x.to_string()).unwrap_or("-".into())),
            (s("Compute capability"), n.cuda_cc.map(|c| format!("{}.{}", c.0, c.1)).unwrap_or("-".into())),
            (s("Memory / BAR1"), format!("{} / {} MB", n.mem_total.unwrap_or(0) >> 20, n.bar1_total.unwrap_or(0) >> 20)),
            (s("Bus width"), n.bus_width.map(|x| format!("{x} bit")).unwrap_or("-".into())),
            (s("VBIOS"), n.vbios.clone().unwrap_or_default()),
        ]));
    }
    for v in [&g.vbios_driver, &g.vbios_rom].into_iter().flatten() {
        let mut rows = vec![
            (s("Source"), v.source.clone()),
            (s("Size"), format!("{} bytes", v.size)),
            (s("SHA-256"), v.sha256.clone()),
            (s("Part number"), v.part_number.clone().unwrap_or("-".into())),
        ];
        if let Some(ver) = &v.version {
            rows.push((s("Version"), ver.clone()));
        }
        if v.atom {
            rows.push((s("Board ID in header"), format!("{:04X}:{:04X}", v.atom_subsys_vendor.unwrap_or(0), v.atom_subsys_device.unwrap_or(0))));
        }
        rows.push((s("PowerPlay table"), v.powerplay_table.as_ref().map(|t| format!("{} bytes", t.len())).unwrap_or("-".into())));
        for i in &v.images {
            rows.push((format!("Image at {:#x}", i.offset), format!("{:04X}:{:04X}, {}, {} bytes{}{}",
                i.vendor, i.device, i.code_type_name(), i.length, if i.last { ", last" } else { "" },
                match i.checksum_ok { Some(true) => ", checksum valid", Some(false) => ", CHECKSUM INVALID", None => "" })));
        }
        let title = if v.source.contains("ROM") || v.source.contains("rom") { "VBIOS (physical ROM)" } else { "VBIOS (driver copy)" };
        out.push((s(title), rows));
    }
    if let Some(i) = &g.integrity {
        let mut rows = vec![
            (s("Module"), i.module.clone()),
            (s("Loaded / on-disk build"), format!("{} / {}", i.srcversion_loaded.clone().unwrap_or_default(), i.srcversion_disk.clone().unwrap_or_default())),
            (s("Signed by"), i.signer.clone().unwrap_or("-".into())),
            (s("Module taint"), i.module_taint.clone().unwrap_or("none".into())),
            (s("Kernel taint"), if i.kernel_taint == 0 { "none".into() } else { format!("{} ({})", i.kernel_taint, i.kernel_taint_flags.join("; ")) }),
        ];
        for f in &i.files {
            rows.push((f.what.clone(), format!("{}\n{} {}", f.path, f.package.clone().map(|p| format!("[{p}]")).unwrap_or_default(), f.detail)));
        }
        out.push((s("Driver files"), rows));
    }
    let d = &g.pci;
    let mut pci = vec![
        (s("Address"), d.addr.clone()),
        (s("Vendor"), format!("{:04X}  {}", d.vendor, d.vendor_name.clone().unwrap_or_default())),
        (s("Device"), format!("{:04X}  {}", d.device, d.device_name.clone().unwrap_or_default())),
        (s("Subsystem"), format!("{:04X}:{:04X}  {}", d.subsys_vendor, d.subsys_device, d.subsys_name.clone().unwrap_or_default())),
        (s("Config space read"), format!("{} bytes{}", d.cfg_len, if d.cfg_len < 256 { " (root reads all 4096)" } else { "" })),
    ];
    for b in &d.bars {
        pci.push((format!("BAR {}", b.index), format!("{}{}{}", crate::util::fmt_bytes(b.size),
            if b.prefetch { ", prefetchable" } else { "" }, if b.is64 { ", 64-bit" } else { "" })));
    }
    if !d.rebar_sizes.is_empty() {
        pci.push((s("Resizable BAR sizes"), d.rebar_sizes.iter().map(|s| format!("{} MB", s >> 20)).collect::<Vec<_>>().join(", ")));
    }
    out.push((s("PCI"), pci));
    if let Some(m) = &g.db {
        let sp = &m.spec;
        out.push((s("Reference spec"), vec![
            (s("Model"), format!("{} ({})", sp.name, sp.chip)),
            (s("Matched by"), m.how.to_string()),
            (s("Units"), format!("{} of {} on the full die", sp.units, sp.units_full_die)),
            (s("Shaders / TMUs / ROPs"), format!("{} / {} / {}", sp.shaders, sp.tmus, sp.rops)),
            (s("Memory"), format!("{} {}-bit, {}, {} Gbps", sp.mem.name(), sp.bus_width, sp.sizes_str(), sp.mem_gbps)),
            (s("Boost clock"), format!("{} MHz", sp.boost_mhz)),
            (s("Data checks"), if sp.verified { "passed".to_string() } else { "FAILED, treat reference values with care".to_string() }),
            (s("Check details"), sp.checks.join("\n")),
            (s("Spec source"), sp.spec_source.to_string()),
            (s("ID source"), sp.id_sources.join(", ")),
            (s("Other SKUs on this ID"), m.candidates.join(", ")),
        ]));
    }
    out
}

// ---------------------------------------------------------------- widgets

const ROW_H: f32 = 22.0;
const LBL: f32 = 96.0;
const GAP: f32 = 5.0;

/// One GPU-Z style line: right-aligned label, sunken value box, repeated.
fn row(ui: &mut egui::Ui, p: &Pal, reserve: f32, cells: &[(&str, Option<Field>, f32)]) {
    let total = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(total, ROW_H), Sense::hover());
    let r = Rect::from_min_max(rect.min, Pos2::new(rect.right() - reserve, rect.bottom()));
    cells_in(ui, p, r, cells);
}

fn cells_in(ui: &mut egui::Ui, p: &Pal, rect: Rect, cells: &[(&str, Option<Field>, f32)]) {
    let n = cells.len() as f32;
    let lbl = if cells.len() >= 3 { [LBL, 56.0, 46.0] } else { [LBL, LBL, LBL] };
    let lbl_sum: f32 = lbl.iter().take(cells.len()).sum();
    let boxes = rect.width() - lbl_sum - n * GAP - (n - 1.0) * GAP;
    let wsum: f32 = cells.iter().map(|c| c.2).sum();
    let mut x = rect.left();
    for (i, (label, f, wt)) in cells.iter().enumerate() {
        if i > 0 {
            x += GAP;
        }
        let lw = lbl[i.min(2)];
        let lg = one_line(ui, tr(label), FontId::new(12.0, FontFamily::Proportional), p.muted, lw - 2.0);
        let lsz = lg.size();
        ui.painter().galley(Pos2::new(x + lw - lsz.x, rect.center().y - lsz.y / 2.0), lg, p.muted);
        x += lw + GAP;
        let bw = boxes * wt / wsum;
        value_box(ui, p, Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(bw, rect.height())), f.as_ref(), label, i);
        x += bw;
    }
}

fn value_box(ui: &mut egui::Ui, p: &Pal, r: Rect, f: Option<&Field>, label: &str, i: usize) {
    let painter = ui.painter();
    painter.rect_filled(r, 2.0, p.well);
    painter.rect_stroke(r, 2.0, Stroke::new(1.0, p.line), egui::StrokeKind::Inside);
    let resp = ui.interact(r, ui.scope_id().with(("vb", label, i, r.top() as i32)), Sense::hover());
    let Some(f) = f else {
        let g = painter.layout_no_wrap(tr("N/A").into(), FontId::new(12.0, FontFamily::Proportional), p.muted);
        painter.galley(r.center() - g.size() / 2.0, g, p.muted);
        return;
    };
    if let Some(c) = match f.status {
        Status::Ok => Some(p.ok),
        Status::Warn => Some(p.warn),
        Status::Fail => Some(p.bad),
        _ => None,
    } {
        painter.rect_filled(Rect::from_min_size(r.min + Vec2::new(2.0, 3.0), Vec2::new(3.0, r.height() - 6.0)), 1.0, c);
    }
    let mono = label == "Device ID" || label == "Revision";
    let font = if mono { FontId::new(11.0, FontFamily::Monospace) } else { FontId::new(12.5, FontFamily::Proportional) };
    let col = match f.status {
        Status::Fail => p.bad,
        Status::Skipped => p.muted,
        _ => p.text,
    };
    // too long for the box: drop the parenthetical, the tooltip has the full text
    let value = tr(&f.value);
    let full = painter.layout_no_wrap(value.to_string(), font.clone(), col).size().x;
    let shown = match value.find(" (") {
        Some(i) if full > r.width() - 14.0 => &value[..i],
        _ => value,
    };
    let g = one_line(ui, shown, font, col, r.width() - 14.0);
    let pos = Pos2::new(r.center().x - g.size().x / 2.0 + 2.0, r.center().y - g.size().y / 2.0);
    ui.painter().galley(pos, g, col);
    resp.on_hover_text(evidence_tip(f));
}

fn evidence_tip(f: &Field) -> String {
    let mut tip = format!("{}\n\n", tr(&f.value));
    tip.push_str(tr(match f.status {
        Status::Ok => "Confirmed by independent sources:",
        Status::Info => "Single source, nothing to compare against:",
        Status::Warn => "Differs from the factory reference:",
        Status::Fail => "Sources contradict each other:",
        Status::Skipped => "Not available.",
    }));
    tip.push('\n');
    for (s, ok) in &f.evidence {
        tip.push_str(&format!("  {} {}\n", if *ok { "✔" } else { "✖" }, s));
    }
    let agree = f.evidence.iter().filter(|e| e.1).count();
    tip.push('\n');
    tip.push_str(&trf("{} of {} agree", &[&agree, &f.evidence.len()]));
    tip
}

/// Read-only checkbox: Some(true) ticked, Some(false) empty, None unknown.
fn check_mark(ui: &mut egui::Ui, p: &Pal, r: Rect, label: &str, state: Option<bool>) -> egui::Response {
    let resp = ui.interact(r, ui.scope_id().with(("ck", label, r.left() as i32, r.top() as i32)), Sense::hover());
    let painter = ui.painter();
    let b = Rect::from_center_size(Pos2::new(r.left() + 7.0, r.center().y), Vec2::splat(13.0));
    painter.rect_filled(b, 2.0, p.well);
    painter.rect_stroke(b, 2.0, Stroke::new(1.0, p.line), egui::StrokeKind::Inside);
    match state {
        Some(true) => {
            painter.line(vec![b.min + Vec2::new(3.0, 6.5), b.min + Vec2::new(5.5, 9.5), b.min + Vec2::new(10.0, 3.5)], Stroke::new(1.8, p.accent));
        }
        None => {
            painter.text(b.center(), egui::Align2::CENTER_CENTER, "?", FontId::new(10.0, FontFamily::Proportional), p.muted);
        }
        Some(false) => {}
    }
    painter.text(Pos2::new(b.right() + 5.0, r.center().y), egui::Align2::LEFT_CENTER, tr(label), FontId::new(12.0, FontFamily::Proportional),
        if state == Some(true) { p.text } else { p.muted });
    resp
}

fn checks_row(ui: &mut egui::Ui, p: &Pal, label: &str, items: &[(&str, Option<bool>)]) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H - 2.0), Sense::hover());
    let lg = one_line(ui, tr(label), FontId::new(12.0, FontFamily::Proportional), p.muted, LBL);
    let lsz = lg.size();
    ui.painter().galley(Pos2::new(rect.left() + LBL - lsz.x, rect.center().y - lsz.y / 2.0), lg, p.muted);
    let x0 = rect.left() + LBL + GAP;
    let w = (rect.right() - x0) / 4.0;
    for (i, (n, st)) in items.iter().enumerate() {
        let r = Rect::from_min_size(Pos2::new(x0 + i as f32 * w, rect.top()), Vec2::new(w, rect.height()));
        check_mark(ui, p, r, n, *st);
    }
}

fn legend(ui: &mut egui::Ui, p: &Pal) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (c, n) in [(p.ok, "cross-checked"), (p.warn, "differs from reference"), (p.bad, "sources contradict")] {
            let (r, _) = ui.allocate_exact_size(Vec2::new(4.0, 12.0), Sense::hover());
            ui.painter().rect_filled(r, 1.0, c);
            ui.label(RichText::new(tr(n)).size(11.0).color(p.muted));
            ui.add_space(6.0);
        }
        ui.label(RichText::new(tr("hover a value for its sources")).size(11.0).color(p.muted));
    });
}

fn group_header(ui: &mut egui::Ui, p: &Pal, title: &str) {
    ui.add_space(4.0);
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.0), Sense::hover());
    let g = ui.painter().layout_no_wrap(title.to_string(), FontId::new(12.0, medium()), p.text);
    let gw = g.size().x;
    ui.painter().galley(Pos2::new(r.left(), r.center().y - g.size().y / 2.0), g, p.text);
    ui.painter().hline(r.left() + gw + 8.0..=r.right(), r.center().y, Stroke::new(1.0, p.line));
}

fn one_line(ui: &egui::Ui, text: &str, font: FontId, color: Color32, width: f32) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap = egui::text::TextWrapping { max_width: width.max(8.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    ui.painter().layout_job(job)
}

fn check_row(ui: &mut egui::Ui, p: &Pal, c: &Check) {
    let col = status_col(p, c.status);
    ui.horizontal_top(|ui| {
        let (r, _) = ui.allocate_exact_size(Vec2::new(18.0, 18.0), Sense::hover());
        ui.painter().circle_filled(r.center(), 7.5, col.gamma_multiply(0.2));
        ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, status_glyph(c.status), FontId::new(10.0, semi()), col);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.label(RichText::new(tr(&c.title)).size(12.5).color(p.text));
            ui.add(egui::Label::new(RichText::new(&c.detail).size(11.0).color(p.muted)).wrap());
        });
    });
}

fn kv(ui: &mut egui::Ui, p: &Pal, i: usize, k: &str, v: &str) {
    let resp = ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(Vec2::new(150.0, 16.0), egui::Layout::left_to_right(egui::Align::Min), |ui| {
            ui.set_width(150.0);
            ui.add(egui::Label::new(RichText::new(tr(k)).size(11.5).color(p.muted)).truncate());
        });
        ui.add(egui::Label::new(RichText::new(v).size(12.0).color(p.text)).wrap());
    });
    if i % 2 == 1 {
        let r = resp.response.rect.expand2(Vec2::new(3.0, 1.5));
        ui.painter().rect_filled(r, 2.0, p.raised.gamma_multiply(0.35));
    }
}

fn sensor_row(ui: &mut egui::Ui, p: &Pal, name: &str, s: &Series, color: Color32, active: bool) -> egui::Response {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, ROW_H), Sense::click());
    let painter = ui.painter();
    if active || resp.hovered() {
        painter.rect_filled(rect.expand(1.5), 3.0, p.raised.gamma_multiply(if active { 0.8 } else { 0.45 }));
    }
    let (lw, vw) = (150.0, 88.0);
    let lab = one_line(ui, tr(name), FontId::new(12.0, FontFamily::Proportional), if active { p.text } else { p.muted }, lw - 8.0);
    let lh = lab.size().y;
    painter.galley(Pos2::new(rect.left() + 4.0, rect.center().y - lh / 2.0), lab, p.text);
    let vr = Rect::from_min_size(Pos2::new(rect.left() + lw, rect.top()), Vec2::new(vw, ROW_H));
    painter.rect_filled(vr, 2.0, p.well);
    painter.rect_stroke(vr, 2.0, Stroke::new(1.0, p.line), egui::StrokeKind::Inside);
    let v = s.last().unwrap_or(0.0);
    let val = if s.unit == "V" { format!("{v:.3} {}", s.unit) } else if v.abs() >= 100.0 || s.unit == "RPM" { format!("{v:.0} {}", s.unit) } else { format!("{v:.1} {}", s.unit) };
    painter.text(Pos2::new(vr.right() - 6.0, vr.center().y), egui::Align2::RIGHT_CENTER, val, FontId::new(12.5, FontFamily::Proportional), p.text);
    let gr = Rect::from_min_max(Pos2::new(vr.right() + 4.0, rect.top()), rect.right_bottom());
    painter.rect_filled(gr, 2.0, p.well);
    painter.rect_stroke(gr, 2.0, Stroke::new(1.0, p.line), egui::StrokeKind::Inside);
    spark(painter, gr.shrink(2.0), s, color);
    let f2 = |x: f64| format!("{x:.2}");
    resp.on_hover_text(format!("{}\n{}\n{}", tr(name),
        trf("now {} {}\nmin {}, max {}, avg {}", &[&f2(v), &s.unit, &f2(s.min), &f2(s.max), &f2(s.avg())]),
        tr("Click to plot this group")))
}

/// GPU-Z style history: newest sample at the right edge, filled to the baseline.
fn spark(painter: &egui::Painter, r: Rect, s: &Series, color: Color32) {
    let step = 2.0;
    let k = ((r.width() / step) as usize + 1).min(s.points.len());
    if k < 2 {
        return;
    }
    let pts = &s.points[s.points.len() - k..];
    let (mut lo, mut hi) = (f64::MAX, f64::MIN);
    for q in pts {
        lo = lo.min(q[1]);
        hi = hi.max(q[1]);
    }
    if s.unit == "%" {
        lo = 0.0;
        hi = 100.0;
    } else {
        lo = lo.min(0.0);
        hi = if hi <= lo { lo + 1.0 } else { hi * 1.1 };
    }
    let span = hi - lo;
    let line: Vec<Pos2> = pts
        .iter()
        .enumerate()
        .map(|(i, q)| Pos2::new(r.right() - (k - 1 - i) as f32 * step, r.bottom() - ((q[1] - lo) / span) as f32 * r.height()))
        .collect();
    let fill = color.gamma_multiply(0.35);
    for w2 in line.windows(2) {
        painter.add(egui::Shape::convex_polygon(
            vec![w2[0], w2[1], Pos2::new(w2[1].x, r.bottom()), Pos2::new(w2[0].x, r.bottom())],
            fill,
            Stroke::NONE,
        ));
    }
    painter.line(line, Stroke::new(1.3, color));
}

fn result_panel(ui: &mut egui::Ui, p: &Pal, st: Status, title: &str, text: &str) {
    let col = status_col(p, st);
    egui::Frame::new()
        .fill(col.gamma_multiply(0.08))
        .stroke(Stroke::new(1.0, col.gamma_multiply(0.55)))
        .corner_radius(4)
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(status_glyph(st)).family(semi()).color(col));
                ui.label(RichText::new(title).family(medium()).size(12.5).color(p.text));
            });
            ui.add(egui::Label::new(RichText::new(text).size(11.5).color(p.muted)).wrap());
        });
}

fn verdict(checks: &[Check]) -> (Status, &'static str) {
    let has = |s: Status| checks.iter().any(|c| c.status == s);
    if has(Status::Fail) {
        (Status::Fail, "Contradictions")
    } else if has(Status::Warn) {
        (Status::Warn, "Modified")
    } else if has(Status::Skipped) {
        (Status::Ok, "Consistent")
    } else {
        (Status::Ok, "Genuine")
    }
}

fn verdict_sentence(s: Status) -> &'static str {
    match s {
        Status::Fail => "Two or more sources report different values for the same thing. Treat this card as fake or faulty until explained.",
        Status::Warn => "Everything is self-consistent, but some values differ from the factory spec: modded BIOS, unlocked units, memory mod or a non-reference model.",
        _ => "Every source agrees with the others and with the factory spec. Run the memory test and FP32 benchmark to verify the hardware itself.",
    }
}

fn group_title(g: &str) -> &str {
    match g {
        "VBIOS" => "Video BIOS",
        "Configuration" => "Shader array",
        "Bus" => "PCI Express",
        o => o,
    }
}

fn unit_title(u: &str) -> &str {
    match u {
        "°C" => "Temperature",
        "MHz" => "Clock",
        "W" => "Power",
        "%" => "Load",
        "RPM" => "Fan",
        "MB" => "Memory",
        "V" => "Voltage",
        o => o,
    }
}

// ---------------------------------------------------------------- settings

pub fn home_dir() -> std::path::PathBuf {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(Into::into).unwrap_or_else(|| ".".into())
}

fn config_path() -> std::path::PathBuf {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(std::path::PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from).or_else(|| Some(home_dir().join(".config")))
    };
    base.unwrap_or_else(home_dir).join("gpu-r").join("settings.json")
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Settings {
    dark: bool,
    lang: Option<String>,
}

impl Settings {
    fn load() -> Self {
        std::fs::read(config_path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or(Settings { dark: true, lang: None })
    }

    fn save(&self) {
        let p = config_path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(j) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(p, j);
        }
    }
}
