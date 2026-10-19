//! Compact Windows monitor. Configuration and collection live in the library; drawing here.
#![windows_subsystem = "windows"]

mod ui;

use benchmon::{config, probe, Monitor, Watched};
use eframe::egui::{self, FontId};
use std::collections::VecDeque;
use std::time::{Duration, Instant};
use ui::{BG, GREEN, HISTORY, RED};

/// What the panel shows of a machine: smoothed bars and history.
#[derive(Default)]
struct Host {
    shown: [f32; 3],
    history: [VecDeque<Option<f32>>; 4],
    last_seen: Option<Instant>,
}

/// The monitor drops with the app, and its probes end with it.
struct App {
    monitor: Monitor,
    hosts: Vec<Host>,
    config_error: Option<String>,
}

impl App {
    fn new() -> Self {
        let (configs, config_error) = match config::load() {
            Ok(configs) => (configs, None),
            Err(error) => (Vec::new(), Some(error)),
        };
        let monitor = Monitor::start(configs.into_iter().map(|c| (c.name, c.target)));
        let hosts = monitor.hosts().iter().map(|_| Host::default()).collect();
        Self { monitor, hosts, config_error }
    }

    fn host(ui: &mut egui::Ui, watched: &Watched, host: &mut Host, t: f64, dt: f32) {
        let reading = watched.reading();
        let (stale, label) = (reading.stale(), reading.label());
        let sample = reading.latest;
        let mut values = [None; 4];
        let mut details = [String::new(), String::new(), String::new()];
        if let Some((sample, at)) = &sample {
            values[0] = Some(sample.cpu);
            values[1] = Some(100.0 * sample.ram_used_gb / sample.ram_total_gb.max(0.001));
            details[1] = format!("{:4.1}/{:.0}G", sample.ram_used_gb, sample.ram_total_gb);
            if let Some(gpu) = sample.gpu {
                values[2] = Some(gpu.util);
                details[2] = format!("{:4.1}/{:.0}G", gpu.vram_used_gb, gpu.vram_total_gb);
            }
            // Capacity is stable; the graph tracks the busiest mounted disk's activity.
            values[3] = probe::busiest_disk(&sample.disks);
            if host.last_seen != Some(*at) {
                host.last_seen = Some(*at);
                for (history, value) in host.history.iter_mut().zip(values) {
                    history.push_back(value);
                    if history.len() > HISTORY {
                        history.pop_front();
                    }
                }
            }
        }
        for (shown, target) in host.shown.iter_mut().zip(values) {
            *shown += (target.unwrap_or(0.0) - *shown) * (1.0 - (-6.0 * dt).exp());
        }
        let (state, color) = if stale { (label, RED) } else { (label, GREEN) };
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("[{}]", watched.name))
                    .font(FontId::monospace(12.0))
                    .color(GREEN)
                    .strong(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(state).font(FontId::monospace(10.0)).color(color));
            });
        });
        ui::bar(ui, ui::Metric::Cpu, host.shown[0], "", t);
        ui::bar(ui, ui::Metric::Ram, host.shown[1], &details[1], t + 0.33);
        if values[2].is_some() {
            ui::bar(ui, ui::Metric::Gpu, host.shown[2], &details[2], t + 0.66);
        }
        ui::disks(ui, sample.as_ref().map_or(&[], |(s, _)| s.disks.as_slice()), t);
        ui::trace(ui, &host.history);
        ui.add_space(8.0);
    }

    /// Reuse the GUI's configuration, collectors and cleanup for one-sample diagnostics.
    fn check(self) -> i32 {
        if let Some(error) = &self.config_error {
            println!("{}", serde_json::json!({"status": "error", "message": error}));
            return 1;
        }
        let deadline = Instant::now() + Duration::from_secs(25);
        let mut pending: Vec<_> = self.monitor.hosts().iter().collect();
        let mut failed = false;
        while !pending.is_empty() {
            pending.retain(|host| {
                let reading = host.reading();
                if let Some((sample, _)) = &reading.latest {
                    let ok = !sample.disks.is_empty();
                    failed |= !ok;
                    println!(
                        "{}",
                        serde_json::json!({"name": host.name,
                        "status": if ok { "ok" } else { "error: no disk readings" },
                        "sample": sample.as_ref()})
                    );
                } else if Instant::now() >= deadline {
                    failed = true;
                    println!(
                        "{}",
                        serde_json::json!({"name": host.name, "status": "error",
                        "message": format!("No sample within 25s ({})", reading.status)})
                    );
                } else {
                    return true;
                }
                false
            });
            if !pending.is_empty() {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        i32::from(failed)
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let t = ctx.input(|input| input.time);
        let dt = ctx.input(|input| input.stable_dt).min(2.0 * ui::FRAME.as_secs_f32());
        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(BG).inner_margin(10.0))
            .show(ctx, |ui| {
                if let Some(error) = &self.config_error {
                    ui.colored_label(RED, error);
                    ui.label("Edit hosts.txt beside benchmon.exe, then restart.");
                }
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for (watched, host) in self.monitor.hosts().iter().zip(&mut self.hosts) {
                        Self::host(ui, watched, host, t, dt);
                    }
                });
            });
        ctx.request_repaint_after(ui::FRAME);
    }
}

fn main() -> eframe::Result {
    let app = App::new();
    if std::env::args().any(|arg| arg == "--check") {
        std::process::exit(app.check());
    }
    let height = (20.0 + 155.0 * app.monitor.hosts().len() as f32).clamp(200.0, 900.0);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("benchmon")
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!("../assets/benchmon.png"))
                    .expect("built-in app icon"),
            )
            .with_inner_size([440.0, height])
            .with_min_inner_size([400.0, 180.0])
            .with_always_on_top(),
        ..Default::default()
    };
    eframe::run_native("benchmon", options, Box::new(|_cc| Ok(Box::new(app))))
}
