//! One probe per machine: a long-lived process (PowerShell locally, `ssh` to the others) that
//! prints CPU/RAM/GPU and per-disk activity every two seconds, with a JSON disk suffix.
//! Disk capacity is refreshed every thirty seconds;
//! nothing is installed on the machine. A probe that ends is restarted after ten
//! seconds, and every probe process is killed when its `Monitor` (or the app) closes.

use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// Where a machine is and how it's read.
#[derive(Clone, Debug)]
pub enum Target {
    Local,
    Windows(String),
    Linux(String),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Sample {
    pub cpu: f32,
    pub ram_used_gb: f32,
    pub ram_total_gb: f32,
    pub gpu: Option<Gpu>,
    pub disks: Vec<Disk>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Disk {
    pub name: String,
    pub used_bytes: f64,
    pub total_bytes: f64,
    pub read_percent: Option<f32>,
    pub write_percent: Option<f32>,
}

impl Disk {
    pub fn percent(&self) -> f32 {
        (100.0 * self.used_bytes / self.total_bytes) as f32
    }

    pub fn busy(&self) -> Option<f32> {
        Some(self.read_percent?.max(self.write_percent?))
    }
}

pub fn busiest_disk(disks: &[Disk]) -> Option<f32> {
    disks.iter().filter_map(Disk::busy).reduce(f32::max)
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct Gpu {
    pub util: f32,
    pub vram_used_gb: f32,
    pub vram_total_gb: f32,
}

#[derive(Clone, Default)]
pub struct Reading {
    pub latest: Option<(Arc<Sample>, Instant)>,
    pub status: String,
}

/// A sample older than this is stale: the panel shows the probe's status instead of ONLINE.
pub const STALE_AFTER: Duration = Duration::from_secs(8);

/// How long a lost probe waits before it connects again (and again, until its monitor stops).
pub const RECONNECT: Duration = Duration::from_secs(10);

impl Reading {
    pub fn stale(&self) -> bool {
        self.latest.as_ref().is_none_or(|(_, at)| at.elapsed() > STALE_AFTER)
    }

    /// What the panel shows for the machine: ONLINE while samples are fresh, else the
    /// probe's status (CONNECTING, LINK LOST, NO PROBE: ...).
    pub fn label(&self) -> String {
        if self.stale() {
            self.status.to_uppercase()
        } else {
            "ONLINE".into()
        }
    }
}

pub type Shared = Arc<Mutex<Reading>>;
pub type Children = Arc<Mutex<Vec<Child>>>;

#[cfg(windows)]
const NO_WINDOW: u32 = 0x0800_0000;

const WINDOWS_PROBE: &str = include_str!("collectors/windows.ps1");
const LINUX_PROBE: &str = include_str!("collectors/linux.py");
// Keep the remote command short enough for cmd.exe's limit. The full collector
// travels over stdin and is compiled in memory; no remote file is installed.
const WINDOWS_BOOTSTRAP: &str =
    "$script = [Console]::In.ReadToEnd(); & ([ScriptBlock]::Create($script))";

/// Starts the probe thread of one machine. It ends once `stop` is set and its probe killed
/// (`Monitor` does both): a probe is never started or restarted after `stop`.
pub fn start(target: Target, shared: Shared, children: Children, stop: Arc<AtomicBool>) {
    start_with(target, shared, children, stop, spawn);
}

/// `start` with the probe process made by `spawn` (a test's fake probe).
fn start_with(
    target: Target,
    shared: Shared,
    children: Children,
    stop: Arc<AtomicBool>,
    spawn: fn(&Target) -> std::io::Result<Child>,
) {
    thread::spawn(move || {
        // Reconnecting as ever: a probe that ends is started again after RECONNECT, for as
        // long as the app (or its monitor) runs; `stop` alone ends the loop.
        while !stop.load(Ordering::SeqCst) {
            run_once(&target, &shared, &children, &stop, spawn);
            let resume = Instant::now() + RECONNECT;
            while Instant::now() < resume {
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
    });
}

/// One probe process, from its start until its output ends.
fn run_once(
    target: &Target,
    shared: &Shared,
    children: &Children,
    stop: &AtomicBool,
    spawn: fn(&Target) -> std::io::Result<Child>,
) {
    set_status(shared, "connecting");
    match spawn(target) {
        Ok(mut child) => {
            let pid = child.id();
            let stdout = child.stdout.take();
            {
                // Checked under the lock `Monitor::drop` takes after setting `stop`: no
                // probe starts unseen by it.
                let mut active = children.lock().unwrap();
                if stop.load(Ordering::SeqCst) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return;
                }
                active.push(child);
            }
            if let Some(out) = stdout {
                for line in BufReader::new(out).lines() {
                    let Ok(line) = line else { break };
                    if let Some(sample) = parse(&line) {
                        let mut r = shared.lock().unwrap();
                        r.latest = Some((Arc::new(sample), Instant::now()));
                        r.status = "online".into();
                    }
                }
            }
            // Reap failed probes rather than retaining handles on every reconnect.
            let finished = {
                let mut active = children.lock().unwrap();
                active
                    .iter()
                    .position(|child| child.id() == pid)
                    .map(|index| active.swap_remove(index))
            };
            if let Some(mut child) = finished {
                let _ = child.kill();
                let _ = child.wait();
            }
            set_status(shared, "link lost");
        }
        Err(e) => set_status(shared, &format!("no probe: {e}")),
    }
}

fn set_status(shared: &Shared, status: &str) {
    shared.lock().unwrap().status = status.into();
}

fn spawn(target: &Target) -> std::io::Result<Child> {
    let mut cmd = Command::new(if matches!(target, Target::Local) { "powershell" } else { "ssh" });
    match target {
        Target::Local => {
            cmd.args([
                "-NoProfile",
                "-NonInteractive",
                "-EncodedCommand",
                &encode_powershell(WINDOWS_PROBE),
            ]);
        }
        Target::Windows(host) => {
            cmd.args(ssh_options()).arg(host).arg(format!(
                "powershell -NoProfile -NonInteractive -EncodedCommand {}",
                encode_powershell(WINDOWS_BOOTSTRAP)
            ));
        }
        Target::Linux(host) => {
            cmd.args(ssh_options()).arg(host).arg("python3 -u -");
        }
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::null());
    cmd.stdin(if matches!(target, Target::Local) { Stdio::null() } else { Stdio::piped() });
    #[cfg(windows)]
    cmd.creation_flags(NO_WINDOW);
    let mut child = cmd.spawn()?;
    #[cfg(windows)]
    job::adopt(&child);
    if let Some(mut stdin) = child.stdin.take() {
        let script = match target {
            Target::Windows(_) => WINDOWS_PROBE,
            Target::Linux(_) => LINUX_PROBE,
            Target::Local => unreachable!(),
        };
        if let Err(error) = stdin.write_all(script.replace('\r', "").as_bytes()) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    }
    Ok(child)
}

/// Probe processes live in a job that Windows kills when the app's handle to it closes, so
/// none outlives the app, however it ends.
#[cfg(windows)]
mod job {
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;
    use std::sync::OnceLock;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    struct Job(isize);

    fn job() -> isize {
        static JOB: OnceLock<Job> = OnceLock::new();
        JOB.get_or_init(|| unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            Job(handle as isize)
        })
        .0
    }

    pub fn adopt(child: &Child) {
        unsafe {
            AssignProcessToJobObject(job() as _, child.as_raw_handle() as _);
        }
    }
}

fn ssh_options() -> [&'static str; 10] {
    [
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=8",
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ServerAliveCountMax=3",
        "-o",
        "StrictHostKeyChecking=yes",
    ]
}

/// Legacy readings remain valid; disk capacity is a separate optional suffix.
fn parse(line: &str) -> Option<Sample> {
    let rest = line.trim().strip_prefix("S ")?;
    let (rest, disks) = match rest.split_once(" | D ") {
        Some((metrics, disks)) => (metrics, parse_disks(disks)),
        None => (rest, Vec::new()),
    };
    let mut fields = rest.splitn(4, ' ');
    let cpu: f32 = fields.next()?.replace(',', ".").parse().ok()?;
    let used: f32 = fields.next()?.parse().ok()?;
    let total: f32 = fields.next()?.parse().ok()?;
    let gpu = fields.next().and_then(|g| {
        let v: Vec<f32> = g.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        (v.len() == 3).then(|| Gpu {
            util: v[0],
            vram_used_gb: v[1] / 1024.0,
            vram_total_gb: v[2] / 1024.0,
        })
    });
    Some(Sample {
        cpu,
        ram_used_gb: used / 1_048_576.0,
        ram_total_gb: total / 1_048_576.0,
        gpu,
        disks,
    })
}

fn parse_disks(text: &str) -> Vec<Disk> {
    serde_json::from_str::<Vec<Disk>>(text)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|mut disk| {
            if disk.name.is_empty()
                || !disk.used_bytes.is_finite()
                || !disk.total_bytes.is_finite()
                || disk.total_bytes <= 0.0
                || disk.used_bytes < 0.0
                || disk.used_bytes > disk.total_bytes
            {
                return None;
            }
            for value in [&mut disk.read_percent, &mut disk.write_percent] {
                *value = value.filter(|v| v.is_finite() && *v >= 0.0).map(|v| v.min(100.0));
            }
            Some(disk)
        })
        .collect()
}

/// PowerShell's -EncodedCommand: the script as UTF-16LE in base64, so no quoting survives
/// three shells.
fn encode_powershell(script: &str) -> String {
    let bytes: Vec<u8> = script.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, &b)| n | (b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    /// Whether the fake machine is there: its probe streams samples; else it fails at once,
    /// as ssh does when its host is gone.
    static UP: AtomicBool = AtomicBool::new(true);

    #[cfg(windows)]
    fn fake(_: &Target) -> std::io::Result<Child> {
        let script = if UP.load(Ordering::SeqCst) {
            "while ($true) { [Console]::Out.WriteLine('S 12 8388608 16777216 | D []'); [Console]::Out.Flush(); Start-Sleep -Milliseconds 300 }"
        } else {
            "exit 255"
        };
        let mut c = Command::new("powershell");
        c.args(["-NoProfile", "-NonInteractive", "-Command", script]);
        c.stdout(Stdio::piped()).stderr(Stdio::null()).stdin(Stdio::null());
        c.creation_flags(NO_WINDOW);
        c.spawn()
    }

    /// Waits up to `limit` for `done`, checking every 100 ms; how long it took.
    fn within(limit: Duration, done: impl Fn() -> bool) -> Option<Duration> {
        let start = Instant::now();
        while start.elapsed() < limit {
            if done() {
                return Some(start.elapsed());
            }
            thread::sleep(Duration::from_millis(100));
        }
        None
    }

    #[test]
    #[cfg(windows)]
    fn a_lost_probe_reconnects_when_its_machine_is_back() {
        let shared: Shared = Arc::new(Mutex::new(Reading::default()));
        let children: Children = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        start_with(Target::Local, shared.clone(), children.clone(), stop.clone(), fake);
        let reading = || shared.lock().unwrap().clone();
        within(Duration::from_secs(20), || reading().label() == "ONLINE").expect("online at first");

        // The machine goes away: its connection drops, and reconnecting fails for a while.
        UP.store(false, Ordering::SeqCst);
        for child in children.lock().unwrap().iter_mut() {
            let _ = child.kill();
        }
        within(Duration::from_secs(5), || reading().status == "link lost").expect("link lost");
        within(STALE_AFTER + Duration::from_secs(3), || reading().label() == "LINK LOST")
            .expect("the panel shows LINK LOST once the last sample is stale");
        // At least one reconnect fails while it is away.
        thread::sleep(RECONNECT + Duration::from_secs(2));
        assert_eq!(reading().label(), "LINK LOST");
        let lost_at = reading().latest.map(|(_, at)| at);

        // Back: within one reconnect pause, fresh samples, and the panel says ONLINE again.
        UP.store(true, Ordering::SeqCst);
        let back = within(RECONNECT + Duration::from_secs(5), || {
            let r = reading();
            r.label() == "ONLINE" && r.latest.map(|(_, at)| at) != lost_at
        })
        .expect("readings resume after the machine is back");
        assert!(back <= RECONNECT + Duration::from_secs(5), "took {back:?}");
        eprintln!(
            "back online {back:?} after the machine returned (reconnect pause {RECONNECT:?})"
        );

        // Only `stop` ends it.
        stop.store(true, Ordering::SeqCst);
        for child in children.lock().unwrap().iter_mut() {
            let _ = child.kill();
        }
    }

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(encode_powershell("a"), "YQA=");
        assert_eq!(encode_powershell("ab"), "YQBiAA==");
    }

    #[test]
    fn lines_parse_with_and_without_a_gpu() {
        let s = parse("S 12 8388608 16777216 37, 2048, 8192").unwrap();
        assert_eq!(s.cpu, 12.0);
        assert!((s.ram_total_gb - 16.0).abs() < 1e-3);
        let g = s.gpu.unwrap();
        assert_eq!(g.util, 37.0);
        assert!((g.vram_total_gb - 8.0).abs() < 1e-3);
        assert!(parse("S 3.5 100 200 ").unwrap().gpu.is_none());
        assert!(parse("garbage").is_none());
    }

    #[test]
    fn disk_capacity_and_io_parse_independently_of_gpu() {
        for gpu in ["", "37, 2048, 8192"] {
            let json = r#"[{"name":"C:","used_bytes":6597069766656,"total_bytes":8796093022208,"read_percent":12,"write_percent":35}]"#;
            let s = parse(&format!("S 12 8388608 16777216 {gpu} | D {json}")).unwrap();
            assert_eq!(s.disks[0].total_bytes, 8796093022208.0);
            assert_eq!(s.disks[0].percent(), 75.0);
            assert_eq!(s.disks[0].busy(), Some(35.0));
            assert_eq!(s.gpu.is_some(), !gpu.is_empty());
        }
    }

    #[test]
    fn invalid_disks_do_not_discard_valid_disks_or_cpu_ram() {
        let json = r#"[{"name":"bad","used_bytes":101,"total_bytes":100},{"name":"good","used_bytes":0,"total_bytes":100,"read_percent":250,"write_percent":40},{"name":"unknown","used_bytes":50,"total_bytes":100}]"#;
        let s = parse(&format!("S 12 100 200 | D {json}")).unwrap();
        assert_eq!(s.cpu, 12.0);
        assert_eq!(s.disks.len(), 2);
        assert_eq!(s.disks[0].percent(), 0.0);
        assert_eq!(s.disks[0].busy(), Some(100.0));
        assert_eq!(s.disks[1].busy(), None);
        for malformed in ["oops", "{}", "[]"] {
            assert!(parse(&format!("S 12 100 200 | D {malformed}")).unwrap().disks.is_empty());
        }
    }
}
