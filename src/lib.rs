//! benchmon's probes as a library: the panel's own readings for other programs.
//!
//! ```no_run
//! use benchmon::{probe::Target, Monitor};
//!
//! let monitor = Monitor::start([("server".to_string(), Target::Linux("my-server".into()))]);
//! std::thread::sleep(std::time::Duration::from_secs(5));
//! if let Some((sample, _at)) = monitor.reading("server").and_then(|r| r.latest) {
//!     println!("{:.0}% CPU, {:.1} GB RAM used", sample.cpu, sample.ram_used_gb);
//! }
//! // Dropping the monitor stops its probes, locally and over SSH.
//! ```

pub mod config;
mod monitor;
pub mod probe;

pub use monitor::{Monitor, Watched};
