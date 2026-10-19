//! A set of probes owned together: started at once, read at any time, stopped when dropped.
use crate::probe::{self, Probe, Reading, Shared, Target};
use std::sync::{Arc, Mutex};

/// One machine a `Monitor` reads.
pub struct Watched {
    pub name: String,
    shared: Shared,
    probe: Probe,
}

impl Watched {
    /// Its latest sample (with when it arrived) and the probe's status.
    pub fn reading(&self) -> Reading {
        self.shared.lock().unwrap().clone()
    }
}

pub struct Monitor {
    hosts: Vec<Watched>,
}

impl Monitor {
    /// Starts one probe per named machine; each reconnects on its own until the monitor drops.
    pub fn start(hosts: impl IntoIterator<Item = (String, Target)>) -> Self {
        let hosts = hosts
            .into_iter()
            .map(|(name, target)| {
                let shared: Shared = Arc::new(Mutex::new(Reading::default()));
                let probe = Probe::start(target, shared.clone(), probe::TIMING, probe::spawn);
                Watched { name, shared, probe }
            })
            .collect();
        Self { hosts }
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
    /// Stops every probe at once; each waits for its own as the hosts drop.
    fn drop(&mut self) {
        for host in &self.hosts {
            host.probe.stop();
        }
    }
}
