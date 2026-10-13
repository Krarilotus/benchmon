//! The library's monitor against this machine's real probe (Windows: the local PowerShell one).
#![cfg(windows)]

use benchmon::probe::{Sample, Target};
use benchmon::Monitor;
use std::time::{Duration, Instant};

#[test]
fn a_local_monitor_reads_serialises_and_stops() {
    let monitor = Monitor::start([("here".to_string(), Target::Local)]);
    let deadline = Instant::now() + Duration::from_secs(30);
    let sample = loop {
        if let Some((sample, _)) = monitor.reading("here").and_then(|r| r.latest) {
            break sample;
        }
        assert!(Instant::now() < deadline, "no sample within 30s");
        std::thread::sleep(Duration::from_millis(200));
    };
    assert!(sample.ram_total_gb > 0.0 && !sample.disks.is_empty());
    let back: Sample = serde_json::from_str(&serde_json::to_string(&*sample).unwrap()).unwrap();
    assert_eq!(back.disks.len(), sample.disks.len());
    assert!(monitor.reading("elsewhere").is_none());

    let dropped = Instant::now();
    drop(monitor);
    assert!(dropped.elapsed() < Duration::from_secs(5), "stopping took {:?}", dropped.elapsed());
}
