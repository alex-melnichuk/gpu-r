//! Background sensor sampling with history (for graphs and CSV logging).

use crate::{amd, nvidia};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct Source {
    pub pci_addr: String,
    pub sysfs: PathBuf,
    pub driver: String,
}

impl Source {
    pub fn read(&self) -> Vec<(String, String, f64)> {
        let mut v = vec![];
        match self.driver.as_str() {
            "amdgpu" => amd::read_sensors(&self.sysfs, amd::hwmon_dir(&self.sysfs).as_deref(), &mut v),
            "nvidia" => {
                if let Some(n) = nvidia::nvml() {
                    n.read_sensors(&self.pci_addr, &mut v);
                }
            }
            _ => {
                // generic hwmon (i915/xe/nouveau/radeon)
                if let Some(h) = amd::hwmon_dir(&self.sysfs) {
                    if let Some(t) = crate::util::read_num(h.join("temp1_input")) {
                        v.push(("GPU Temperature".into(), "°C".into(), t as f64 / 1000.0));
                    }
                    if let Some(f) = crate::util::read_num(h.join("fan1_input")) {
                        v.push(("Fan Speed".into(), "RPM".into(), f as f64));
                    }
                    if let Some(p) = crate::util::read_num(h.join("power1_input")) {
                        v.push(("Board Power".into(), "W".into(), p as f64 / 1e6));
                    }
                }
                if let Some(f) = crate::util::read_num(self.sysfs.join("drm").join("card0/gt_act_freq_mhz")) {
                    v.push(("GPU Clock".into(), "MHz".into(), f as f64));
                }
            }
        }
        v
    }

    pub fn vram_used(&self) -> Option<u64> {
        match self.driver.as_str() {
            "amdgpu" => amd::vram_used(&self.sysfs),
            "nvidia" => nvidia::nvml().and_then(|n| n.mem_used(&self.pci_addr)),
            _ => None,
        }
    }

    pub fn gpu_clock(&self) -> Option<f64> {
        self.read().into_iter().find(|(n, _, _)| n == "GPU Clock").map(|x| x.2)
    }
}

#[derive(Default, Clone)]
pub struct Series {
    pub unit: String,
    pub points: Vec<[f64; 2]>,
    pub min: f64,
    pub max: f64,
    pub sum: f64,
    pub n: u64,
}

impl Series {
    pub fn last(&self) -> Option<f64> {
        self.points.last().map(|p| p[1])
    }
    pub fn avg(&self) -> f64 {
        if self.n == 0 { 0.0 } else { self.sum / self.n as f64 }
    }
}

#[derive(Default)]
pub struct History {
    pub series: BTreeMap<String, Series>,
    pub order: Vec<String>,
    pub start: Option<Instant>,
    pub log_file: Option<std::fs::File>,
    pub log_header_written: bool,
}

pub const MAX_POINTS: usize = 3600;

impl History {
    pub fn push(&mut self, sample: Vec<(String, String, f64)>) {
        let t0 = *self.start.get_or_insert_with(Instant::now);
        let t = t0.elapsed().as_secs_f64();
        if let Some(f) = self.log_file.as_mut() {
            if !self.log_header_written {
                let names: Vec<String> = sample.iter().map(|(n, u, _)| format!("{n} [{u}]")).collect();
                let _ = writeln!(f, "time_s,{}", names.join(","));
                self.log_header_written = true;
            }
            let vals: Vec<String> = sample.iter().map(|(_, _, v)| format!("{v:.2}")).collect();
            let _ = writeln!(f, "{t:.1},{}", vals.join(","));
        }
        for (name, unit, v) in sample {
            if !self.series.contains_key(&name) {
                self.order.push(name.clone());
            }
            let s = self.series.entry(name).or_insert_with(|| Series {
                unit,
                min: f64::MAX,
                max: f64::MIN,
                ..Default::default()
            });
            s.points.push([t, v]);
            if s.points.len() > MAX_POINTS {
                s.points.drain(..s.points.len() - MAX_POINTS);
            }
            s.min = s.min.min(v);
            s.max = s.max.max(v);
            s.sum += v;
            s.n += 1;
        }
    }

    pub fn reset_stats(&mut self) {
        for s in self.series.values_mut() {
            s.min = f64::MAX;
            s.max = f64::MIN;
            s.sum = 0.0;
            s.n = 0;
        }
    }
}

pub struct Sampler {
    pub history: Arc<Mutex<History>>,
    pub interval_ms: Arc<Mutex<u64>>,
    pub source: Arc<Mutex<Source>>,
}

impl Sampler {
    pub fn start(src: Source, on_sample: impl Fn() + Send + 'static) -> Self {
        let history = Arc::new(Mutex::new(History::default()));
        let interval_ms = Arc::new(Mutex::new(1000u64));
        let source = Arc::new(Mutex::new(src));
        let (h, i, s) = (history.clone(), interval_ms.clone(), source.clone());
        std::thread::spawn(move || loop {
            let src = s.lock().unwrap().clone();
            let sample = src.read();
            h.lock().unwrap().push(sample);
            on_sample();
            let ms = *i.lock().unwrap();
            std::thread::sleep(Duration::from_millis(ms));
        });
        Self { history, interval_ms, source }
    }
}
