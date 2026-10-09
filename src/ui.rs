//! Shared segmented bars, capacity-proportional disk layout and metric history.
use crate::probe::Disk;
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Shape, Stroke, Vec2};
use std::collections::VecDeque;

pub const HISTORY: usize = 90;
pub const BG: Color32 = Color32::from_rgb(4, 9, 6);
pub const GREEN: Color32 = Color32::from_rgb(57, 255, 136);
pub const DIM: Color32 = Color32::from_rgb(20, 70, 40);
pub const AMBER: Color32 = Color32::from_rgb(255, 196, 0);
pub const RED: Color32 = Color32::from_rgb(255, 64, 80);

#[derive(Clone, Copy)]
pub enum Metric {
    Cpu,
    Ram,
    Gpu,
    Disk,
}

impl Metric {
    fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::Ram => "RAM",
            Self::Gpu => "GPU",
            Self::Disk => "DISK",
        }
    }

    fn base(self) -> Color32 {
        match self {
            Self::Cpu => GREEN,
            Self::Ram => Color32::from_rgb(180, 48, 62),
            Self::Gpu => Color32::from_rgb(255, 164, 58),
            Self::Disk => Color32::from_rgb(117, 209, 255),
        }
    }

    fn color(self, value: f32) -> Color32 {
        let base = self.base();
        if value <= 80.0 {
            let fade = ((value - 50.0) / 30.0).clamp(0.0, 1.0);
            base.gamma_multiply(0.55 + 0.20 * fade)
        } else {
            // Above 80%, brighten much faster while keeping the metric's own hue.
            let fade = ((value - 80.0) / 20.0).clamp(0.0, 1.0);
            blend(base.gamma_multiply(0.75), lighter(base), 1.0 - (1.0 - fade).powi(2))
        }
    }
}

fn blend(from: Color32, to: Color32, amount: f32) -> Color32 {
    let channel = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * amount).round() as u8;
    Color32::from_rgb(
        channel(from.r(), to.r()),
        channel(from.g(), to.g()),
        channel(from.b(), to.b()),
    )
}

fn lighter(color: Color32) -> Color32 {
    blend(color, Color32::WHITE, 0.5)
}

fn text(
    p: &egui::Painter,
    at: Pos2,
    align: Align2,
    label: impl ToString,
    size: f32,
    color: Color32,
) {
    p.text(at, align, label, FontId::monospace(size), color);
}

const SEGMENTS: usize = 32;
const CPU_SCAN_SPEED: f64 = 48.0;
const DISK_SCAN_SPEED: f64 = 1.0;
const LABEL_WIDTH: f32 = 38.0;
const VALUE_WIDTH: f32 = 118.0;

fn lit_segments(value: f32) -> usize {
    (value.clamp(0.0, 100.0) / 100.0 * SEGMENTS as f32).round() as usize
}

fn scan_index(time: f64, speed: f64, filled: usize) -> Option<usize> {
    (filled > 0).then(|| (time * speed) as usize % filled)
}

/// Shared bar geometry for CPU/RAM/GPU and both disk rows, including scaled gaps.
fn paint_segments(p: &egui::Painter, rect: Rect, metric: Metric, value: f32, scan: Option<usize>) {
    let gap = 2.0_f32.min(rect.width() / (SEGMENTS as f32 * 3.0));
    let width = (rect.width() - gap * (SEGMENTS - 1) as f32) / SEGMENTS as f32;
    let lit = lit_segments(value);
    let color = metric.color(value);
    let flash = lighter(color);
    for index in 0..SEGMENTS {
        let cell = Rect::from_min_size(
            Pos2::new(rect.left() + index as f32 * (width + gap), rect.top()),
            Vec2::new(width, rect.height()),
        );
        let color = if index >= lit {
            metric.base().gamma_multiply(0.2)
        } else if scan == Some(index) {
            flash
        } else {
            color
        };
        p.rect_filled(cell, 0.0, color);
    }
}

pub fn bar(ui: &mut egui::Ui, metric: Metric, value: f32, detail: &str, t: f64) {
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.0), egui::Sense::hover());
    let p = ui.painter();
    text(p, rect.left_center(), Align2::LEFT_CENTER, metric.label(), 11.0, metric.base());
    let track = Rect::from_min_max(
        Pos2::new(rect.left() + LABEL_WIDTH, rect.top() + 3.0),
        Pos2::new(rect.right() - VALUE_WIDTH, rect.bottom() - 3.0),
    );
    paint_segments(p, track, metric, value, scan_index(t, CPU_SCAN_SPEED, lit_segments(value)));
    text(
        p,
        rect.right_center(),
        Align2::RIGHT_CENTER,
        format!("{value:5.1}% {detail}"),
        11.0,
        metric.base(),
    );
}

/// Per-disk widths follow capacity. Every disk scales its own 32 segments into that width.
/// Each row's slow scan loops through filled segments across all disks without an empty tail.
pub fn disks(ui: &mut egui::Ui, disks: &[Disk], t: f64) {
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 45.0), egui::Sense::hover());
    let p = ui.painter();
    text(p, rect.min, Align2::LEFT_TOP, "DISK", 11.0, Metric::Disk.base());
    for (label, y) in [("I/O", 20.0), ("SPACE", 36.0)] {
        text(
            p,
            Pos2::new(rect.left(), rect.top() + y),
            Align2::LEFT_CENTER,
            label,
            9.0,
            Metric::Disk.base(),
        );
    }
    let left = rect.left() + LABEL_WIDTH;
    let width = rect.right() - VALUE_WIDTH - left;
    if disks.is_empty() {
        for y in [20.0, 36.0] {
            text(
                p,
                Pos2::new(rect.right(), rect.top() + y),
                Align2::RIGHT_CENTER,
                "n/a",
                9.0,
                AMBER,
            );
        }
        return;
    }
    let filled = [
        disks.iter().map(|disk| lit_segments(disk.busy().unwrap_or(0.0))).sum(),
        disks.iter().map(|disk| lit_segments(disk.percent())).sum(),
    ];
    let scans = filled.map(|count| scan_index(t, DISK_SCAN_SPEED, count));
    let mut offsets = [0; 2];
    for (index, (disk, (start, end))) in disks.iter().zip(disk_ranges(disks)).enumerate() {
        let region = Rect::from_min_max(
            Pos2::new(left + width * start, rect.top()),
            Pos2::new(left + width * end, rect.bottom()),
        );
        let painter = p.with_clip_rect(region);
        text(
            &painter,
            region.center_top(),
            Align2::CENTER_TOP,
            &disk.name,
            9.0,
            Metric::Disk.base(),
        );
        for (row, value) in [disk.busy(), Some(disk.percent())].into_iter().enumerate() {
            let track = Rect::from_min_max(
                Pos2::new(region.left(), rect.top() + 14.0 + row as f32 * 16.0),
                Pos2::new(region.right(), rect.top() + 26.0 + row as f32 * 16.0),
            );
            let value = value.unwrap_or(0.0);
            let lit = lit_segments(value);
            let local_scan = scans[row]
                .and_then(|scan| scan.checked_sub(offsets[row]))
                .filter(|scan| *scan < lit);
            paint_segments(&painter, track, Metric::Disk, value, local_scan);
            offsets[row] += lit;
            // Delimit volumes without changing their capacity-proportional allocation.
            if index > 0 {
                painter.line_segment(
                    [track.left_top(), track.left_bottom()],
                    Stroke::new(1.0_f32, BG),
                );
            }
        }
        ui.interact(region, ui.id().with(("disk", index)), egui::Sense::hover()).on_hover_ui(
            |ui| {
                ui.label(disk_details(disk));
            },
        );
    }
    let busiest = crate::probe::busiest_disk(disks);
    let (io_text, io_color) =
        busiest.map_or(("n/a".into(), AMBER), |v| (format!("{v:5.1}%"), Metric::Disk.base()));
    let total: f64 = disks.iter().map(|disk| disk.total_bytes).sum();
    let used: f64 = disks.iter().map(|disk| disk.used_bytes).sum();
    let percent = (100.0 * used / total) as f32;
    let capacity = if total >= 1024_f64.powi(4) {
        format!("{:.1}/{:.1}T", used / 1024_f64.powi(4), total / 1024_f64.powi(4))
    } else {
        format!("{:.1}/{:.0}G", used / 1024_f64.powi(3), total / 1024_f64.powi(3))
    };
    for (row, (label, color)) in
        [(io_text, io_color), (format!("{percent:5.1}% {capacity}"), Metric::Disk.base())]
            .into_iter()
            .enumerate()
    {
        text(
            p,
            Pos2::new(rect.right(), rect.top() + 20.0 + row as f32 * 16.0),
            Align2::RIGHT_CENTER,
            label,
            11.0,
            color,
        );
    }
}

fn disk_details(disk: &Disk) -> String {
    let gib = 1024_f64.powi(3);
    let io = match (disk.read_percent, disk.write_percent) {
        (Some(read), Some(write)) => {
            format!("I/O: {:.1}% (read {read:.1}%, write {write:.1}%)", read.max(write))
        }
        _ => "I/O: unavailable".into(),
    };
    format!("{}\n{io}\nSpace: {:.1}% used\n{:.1} / {:.1} GiB occupied, {:.1} GiB free\nCapacity refreshed every 30 seconds",
        disk.name, disk.percent(), disk.used_bytes / gib, disk.total_bytes / gib, (disk.total_bytes - disk.used_bytes) / gib)
}

fn disk_ranges(disks: &[Disk]) -> impl Iterator<Item = (f32, f32)> + '_ {
    let total: f64 = disks.iter().map(|d| d.total_bytes).sum();
    let mut start = 0.0;
    disks.iter().map(move |disk| {
        let end = start + disk.total_bytes / total;
        let range = (start as f32, end as f32);
        start = end;
        range
    })
}

pub fn trace(ui: &mut egui::Ui, histories: &[VecDeque<Option<f32>>; 4]) {
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_stroke(rect, 0.0, Stroke::new(1.0_f32, DIM), egui::StrokeKind::Inside);
    let colors = [Metric::Cpu, Metric::Ram, Metric::Gpu, Metric::Disk].map(Metric::base);
    for (history, color) in histories.iter().zip(colors) {
        let step = rect.width() / (HISTORY - 1) as f32;
        let start = rect.right() - step * history.len().saturating_sub(1) as f32;
        let mut points = Vec::new();
        for (index, value) in history.iter().enumerate() {
            if let Some(value) = value {
                points.push(Pos2::new(
                    start + index as f32 * step,
                    rect.bottom() - 2.0 - (rect.height() - 4.0) * value.clamp(0.0, 100.0) / 100.0,
                ));
            } else {
                if points.len() >= 2 {
                    p.add(Shape::line(std::mem::take(&mut points), Stroke::new(1.2_f32, color)));
                }
                points.clear();
            }
        }
        if points.len() >= 2 {
            p.add(Shape::line(points, Stroke::new(1.2_f32, color)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_restarts_at_the_last_filled_segment_instead_of_waiting_for_empty_space() {
        assert_eq!(scan_index(0.0, 1.0, 3), Some(0));
        assert_eq!(scan_index(2.0, 1.0, 3), Some(2));
        assert_eq!(scan_index(3.0, 1.0, 3), Some(0));
        assert_eq!(scan_index(0.25, 48.0, 3), Some(0));
        assert_eq!(scan_index(100.0, 1.0, 0), None);
    }

    #[test]
    fn widths_follow_capacity_while_each_disks_metrics_remain_independent() {
        let disks = vec![
            Disk {
                name: "small".into(),
                used_bytes: 25.0,
                total_bytes: 100.0,
                read_percent: Some(20.0),
                write_percent: Some(80.0),
            },
            Disk {
                name: "large".into(),
                used_bytes: 225.0,
                total_bytes: 300.0,
                read_percent: Some(5.0),
                write_percent: Some(10.0),
            },
        ];
        assert_eq!(disk_ranges(&disks).collect::<Vec<_>>(), vec![(0.0, 0.25), (0.25, 1.0)]);
        assert_eq!(disks[0].busy(), Some(80.0));
        assert_eq!(disks[0].percent(), 25.0);
        assert_eq!(disks[1].busy(), Some(10.0));
        assert_eq!(disks[1].percent(), 75.0);
    }
}
