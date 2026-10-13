//! A set of probes owned together: started at once, read at any time, stopped when dropped.
use crate::probe::{self, Children, Reading, Shared, Target};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// One machine a `Monitor` reads.
pub struct Watched {
    pub name: String,
    shared: Shared,
}

impl Watched {
    /// Its latest sample (with when it arrived) and the probe's status.
    pub fn reading(&self) -> Reading {
        self.shared.lock().unwrap().clone()
    }
}

pub struct Monitor {
    hosts: Vec<Watched>,
    children: Children,
    stop: Arc<AtomicBool>,
}

impl Monitor {
    /// Starts one probe per named machine; each reconnects on its own until the monitor drops.
    pub fn start(hosts: impl IntoIterator<Item = (String, Target)>) -> Self {
        let children: Children = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let hosts = hosts
            .into_iter()
            .map(|(name, target)| {
                let shared: Shared = Arc::new(Mutex::new(Reading::default()));
                probe::start(target, shared.clone(), children.clone(), stop.clone());
                Watched { name, shared }
            })
            .collect();
        Self { hosts, children, stop }
    }

    pub fn hosts(&self) -> &[Watched] {
        &self.hosts
    }

    /// The reading of the machine named `name`, if this monitor watches it.
    pub fn reading(&self, name: &str) -> Option<Reading> {
        self.hosts.iter().find(|host| host.name == name).map(Watched::reading)
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        for child in self.children.lock().unwrap().iter_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
