//! One probe per machine: a long-lived process (PowerShell locally, `ssh` to the others) that
//! prints CPU/RAM/GPU and per-disk activity every two seconds, with a JSON disk suffix.
//! Disk capacity is refreshed every thirty seconds; nothing is installed on the machine.
//! Every state of a probe has a deadline: one that ends, or stays silent too long (a hung
//! collector, a dead link that keeps its pipe open), is ended with every process it started
//! and connected again, after a pause that doubles while attempts deliver nothing.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
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

/// What a machine's probe is doing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Status {
    /// Started; no sample from this connection yet.
    #[default]
    Connecting,
    /// Its samples arrive (the reading tells whether the last one is still fresh).
    Online,
    /// It ended or stayed silent past its deadline; it connects again after a pause.
    LinkLost,
    /// It could not be started.
    NoProbe(String),
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Status::Connecting => f.write_str("connecting"),
            Status::Online => f.write_str("online"),
            Status::LinkLost => f.write_str("link lost"),
            Status::NoProbe(error) => write!(f, "no probe: {error}"),
        }
    }
}

#[derive(Clone, Default)]
pub struct Reading {
    /// The latest sample and when it arrived; never refreshed without a new sample.
    pub latest: Option<(Arc<Sample>, Instant)>,
    pub status: Status,
}

/// A sample older than this is stale: the panel shows the probe's status instead of ONLINE.
pub const STALE_AFTER: Duration = Duration::from_secs(8);

impl Reading {
    pub fn stale(&self) -> bool {
        self.latest.as_ref().is_none_or(|(_, at)| at.elapsed() > STALE_AFTER)
    }

    /// What the panel shows for the machine: ONLINE while samples are fresh, else what the
    /// probe is doing (STALE while a running probe is silent, CONNECTING, LINK LOST,
    /// NO PROBE: ...) and the age of the values it last delivered.
    pub fn label(&self) -> String {
        if !self.stale() {
            return "ONLINE".into();
        }
        let state = match &self.status {
            Status::Online => "STALE".into(),
            other => other.to_string().to_uppercase(),
        };
        match &self.latest {
            Some((_, at)) => format!("{state} ({})", age(at.elapsed())),
            None => state,
        }
    }
}

fn age(elapsed: Duration) -> String {
    match elapsed.as_secs() {
        s @ 0..120 => format!("{s}s"),
        s @ 120..7200 => format!("{}m", s / 60),
        s => format!("{}h", s / 3600),
    }
}

pub type Shared = Arc<Mutex<Reading>>;

/// The deadlines that bound every state of a probe.
#[derive(Clone, Copy)]
pub(crate) struct Timing {
    /// For a new probe's first sample: ssh's connect and PowerShell's compile, under load.
    pub first_sample: Duration,
    /// Between samples, before a silent probe is ended.
    pub silence: Duration,
    /// The reconnect pause: `pause` after a probe that delivered, doubling up to `max_pause`
    /// while attempts deliver nothing.
    pub pause: Duration,
    pub max_pause: Duration,
}

pub(crate) const TIMING: Timing = Timing {
    first_sample: Duration::from_secs(45),
    silence: Duration::from_secs(30),
    pause: Duration::from_secs(2),
    max_pause: Duration::from_secs(60),
};

#[cfg(windows)]
const NO_WINDOW: u32 = 0x0800_0000;

const WINDOWS_PROBE: &str = include_str!("collectors/windows.ps1");
const LINUX_PROBE: &str = include_str!("collectors/linux.py");
// Keep the remote command short enough for cmd.exe's limit. The full collector
// travels over stdin and is compiled in memory; no remote file is installed.
const WINDOWS_BOOTSTRAP: &str =
    "$script = [Console]::In.ReadToEnd(); & ([ScriptBlock]::Create($script))";

/// What a probe's thread waits for: a line of a connection, a connection's end, or `Stop`.
enum Event {
    Line(u64, String),
    Ended(u64),
    Stop,
}

/// The thread reading one machine, from `start` until it is dropped, which ends it and its
/// process at once, wherever it waits.
pub(crate) struct Probe {
    events: Sender<Event>,
    thread: Option<JoinHandle<()>>,
}

impl Probe {
    /// Starts reading `target` into `shared`, its processes made by `spawn`.
    pub fn start(
        target: Target,
        shared: Shared,
        timing: Timing,
        spawn: impl Fn(&Target) -> std::io::Result<Child> + Send + 'static,
    ) -> Self {
        let (events, inbox) = mpsc::channel();
        let lines = events.clone();
        let thread = thread::spawn(move || {
            supervise(&target, &shared, timing, &spawn, &lines, &inbox);
        });
        Self { events, thread: Some(thread) }
    }

    /// Asks the probe to stop, without waiting for it (dropping it waits).
    pub fn stop(&self) {
        let _ = self.events.send(Event::Stop);
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        self.stop();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Connects again and again, until stopped (`None`).
fn supervise(
    target: &Target,
    shared: &Shared,
    timing: Timing,
    spawn: &dyn Fn(&Target) -> std::io::Result<Child>,
    lines: &Sender<Event>,
    inbox: &Receiver<Event>,
) -> Option<()> {
    let mut pause = timing.pause;
    for connection in 0.. {
        if connect(target, shared, timing, spawn, connection, lines, inbox)? {
            pause = timing.pause;
        }
        let resume = Instant::now() + pause;
        while let Some(event) = next(inbox, resume) {
            if let Event::Stop = event {
                return None;
            }
        }
        pause = (pause * 2).min(timing.max_pause);
    }
    Some(())
}

/// One probe process, from its start until it ends, misses its deadline or is stopped:
/// whether it delivered a sample, or `None` once stopped.
fn connect(
    target: &Target,
    shared: &Shared,
    timing: Timing,
    spawn: &dyn Fn(&Target) -> std::io::Result<Child>,
    connection: u64,
    lines: &Sender<Event>,
    inbox: &Receiver<Event>,
) -> Option<bool> {
    set_status(shared, Status::Connecting);
    let mut process = match spawn(target) {
        Ok(child) => Process::new(child),
        Err(error) => {
            set_status(shared, Status::NoProbe(error.to_string()));
            return Some(false);
        }
    };
    if let Some(out) = process.child.stdout.take() {
        pump(out, connection, lines.clone());
    }
    let mut delivered = false;
    let mut deadline = Instant::now() + timing.first_sample;
    while let Some(event) = next(inbox, deadline) {
        match event {
            Event::Line(c, line) if c == connection => {
                if let Some(sample) = parse(&line) {
                    let mut reading = shared.lock().unwrap();
                    reading.latest = Some((Arc::new(sample), Instant::now()));
                    reading.status = Status::Online;
                    delivered = true;
                    deadline = Instant::now() + timing.silence;
                }
            }
            Event::Ended(c) if c == connection => break,
            // An earlier connection's last words.
            Event::Line(..) | Event::Ended(_) => {}
            Event::Stop => return None,
        }
    }
    drop(process);
    set_status(shared, Status::LinkLost);
    Some(delivered)
}

/// The next event before `deadline`, if one comes.
fn next(inbox: &Receiver<Event>, deadline: Instant) -> Option<Event> {
    match inbox.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(event) => Some(event),
        Err(RecvTimeoutError::Timeout) => None,
        // Unreachable while the probe holds a sender itself; stop all the same.
        Err(RecvTimeoutError::Disconnected) => Some(Event::Stop),
    }
}

/// Forwards a probe's lines until its output closes, which ending its process does.
fn pump(out: ChildStdout, connection: u64, lines: Sender<Event>) {
    thread::spawn(move || {
        for line in BufReader::new(out).split(b'\n') {
            let Ok(line) = line else { break };
            let line = String::from_utf8_lossy(&line).into_owned();
            if lines.send(Event::Line(connection, line)).is_err() {
                return;
            }
        }
        let _ = lines.send(Event::Ended(connection));
    });
}

fn set_status(shared: &Shared, status: Status) {
    shared.lock().unwrap().status = status;
}

/// A probe process, ended with everything it started when dropped.
struct Process {
    child: Child,
    #[cfg(windows)]
    job: Option<job::Job>,
}

impl Process {
    fn new(child: Child) -> Self {
        Self {
            #[cfg(windows)]
            job: job::Job::holding(&child),
            child,
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        #[cfg(windows)]
        drop(self.job.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub(crate) fn spawn(target: &Target) -> std::io::Result<Child> {
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

/// Each probe process lives in a job of its own that Windows ends, with every process the
/// probe started (nvidia-smi, the compiler behind Add-Type), when its handle closes: when the
/// probe is ended, and however the app ends. No grandchild outlives it or holds its output open.
#[cfg(windows)]
mod job {
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    pub struct Job(HANDLE);

    impl Job {
        pub fn holding(child: &Child) -> Option<Self> {
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return None;
                }
                let job = Job(handle);
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let limited = SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                ) != 0;
                let held =
                    limited && AssignProcessToJobObject(handle, child.as_raw_handle() as _) != 0;
                held.then_some(job)
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
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

    /// Short deadlines, so a test sees every state end.
    const QUICK: Timing = Timing {
        first_sample: Duration::from_secs(4),
        silence: Duration::from_secs(4),
        pause: Duration::from_millis(200),
        max_pause: Duration::from_millis(800),
    };

    /// A fake probe: a PowerShell running `script`.
    #[cfg(windows)]
    fn fake(script: &str) -> std::io::Result<Child> {
        let mut c = Command::new("powershell");
        c.args(["-NoProfile", "-NonInteractive", "-Command", script]);
        c.stdout(Stdio::piped()).stderr(Stdio::null()).stdin(Stdio::null());
        c.creation_flags(NO_WINDOW);
        c.spawn()
    }

    #[cfg(windows)]
    const SAMPLE: &str =
        "[Console]::Out.WriteLine('S 12 8388608 16777216 | D []'); [Console]::Out.Flush()";

    /// Starts a probe whose `n`th attempt is `attempt(n)`; the probe and when each attempt began.
    fn probe(
        attempt: impl Fn(usize) -> std::io::Result<Child> + Send + 'static,
    ) -> (Probe, Shared, Arc<Mutex<Vec<Instant>>>) {
        let shared: Shared = Arc::default();
        let attempts: Arc<Mutex<Vec<Instant>>> = Arc::default();
        let log = attempts.clone();
        let probe = Probe::start(Target::Local, shared.clone(), QUICK, move |_| {
            let n = {
                let mut log = log.lock().unwrap();
                log.push(Instant::now());
                log.len() - 1
            };
            attempt(n)
        });
        (probe, shared, attempts)
    }

    /// Waits up to `limit` for `done`, checking every 50 ms; how long it took.
    fn within(limit: Duration, done: impl Fn() -> bool) -> Option<Duration> {
        let start = Instant::now();
        while start.elapsed() < limit {
            if done() {
                return Some(start.elapsed());
            }
            thread::sleep(Duration::from_millis(50));
        }
        None
    }

    /// The panel stuck at CONNECTING under load: a probe that starts and then never answers, or
    /// answers once and falls silent (a hung collector), is ended and started again.
    #[test]
    #[cfg(windows)]
    fn a_silent_probe_is_ended_and_started_again() {
        let (_probe, shared, attempts) = probe(|n| match n {
            0 => fake("Start-Sleep 3600"),
            1 => fake(&format!("{SAMPLE}; Start-Sleep 3600")),
            _ => fake(&format!("while ($true) {{ {SAMPLE}; Start-Sleep -Milliseconds 300 }}")),
        });
        let reading = || shared.lock().unwrap().clone();
        let count = || attempts.lock().unwrap().len();

        within(QUICK.first_sample + Duration::from_secs(5), || reading().label() == "ONLINE")
            .expect("the probe that never answered was replaced by one that does");
        assert_eq!(count(), 2);
        let silent_since = reading().latest.map(|(_, at)| at);
        within(QUICK.silence + Duration::from_secs(5), || {
            let r = reading();
            r.label() == "ONLINE" && r.latest.map(|(_, at)| at) != silent_since
        })
        .expect("the silent probe was replaced and fresh samples arrive");
        assert_eq!(count(), 3);
    }

    #[test]
    #[cfg(windows)]
    fn reconnects_back_off_while_they_fail_and_resume_when_the_machine_is_back() {
        const DOWN: usize = 5;
        let (probe, shared, attempts) = probe(|n| match n {
            0..DOWN => Err(std::io::Error::other("down")),
            _ => fake(&format!("while ($true) {{ {SAMPLE}; Start-Sleep -Milliseconds 300 }}")),
        });
        within(Duration::from_secs(15), || shared.lock().unwrap().label() == "ONLINE")
            .expect("readings resume once the machine is back");
        let at = attempts.lock().unwrap().clone();
        let pauses: Vec<Duration> = at.windows(2).take(DOWN).map(|w| w[1] - w[0]).collect();
        // 200 ms doubling, capped at 800 ms.
        for (pause, expected) in pauses.iter().zip([200, 400, 800, 800, 800]) {
            let expected = Duration::from_millis(expected);
            assert!(
                *pause >= expected && *pause < expected + Duration::from_millis(400),
                "pauses {pauses:?}"
            );
        }

        let dropped = Instant::now();
        drop(probe);
        assert!(
            dropped.elapsed() < Duration::from_secs(3),
            "stopping took {:?}",
            dropped.elapsed()
        );
    }

    /// A silent but running probe's values are not shown as live: stale, with their age.
    #[test]
    fn stale_values_are_labelled_stale_with_their_age() {
        let old = Instant::now().checked_sub(Duration::from_secs(20)).unwrap();
        let reading = |at: Option<Instant>, status| Reading {
            latest: at.map(|at| (Arc::new(parse("S 1 1 2").unwrap()), at)),
            status,
        };
        assert_eq!(reading(Some(Instant::now()), Status::Online).label(), "ONLINE");
        assert_eq!(reading(Some(old), Status::Online).label(), "STALE (20s)");
        assert_eq!(reading(Some(old), Status::Connecting).label(), "CONNECTING (20s)");
        assert_eq!(reading(None, Status::Connecting).label(), "CONNECTING");
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
