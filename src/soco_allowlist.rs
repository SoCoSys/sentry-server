// SoCo Sentry — server-side allowlist (default-deny).
//
// Reads the allowlist file the portal writes (the allowed device IDs) on a
// shared volume, every few seconds. hbbs denies RegisterPk + PunchHoleRequest
// for any id NOT on the list. Because the file lives on disk — independent of
// the portal *process* — enforcement survives a portal outage AND a relay
// restart (the file is still there to read on boot).
//
// Fail-open ONLY when no allowlist file exists yet (fresh / unconfigured):
// allow everything until the portal writes one, so a deploy never locks the
// fleet out. Once a file is present it is authoritative — an empty list denies
// everyone, which is real default-deny. If an existing file later disappears,
// the last known allowlist is kept (we never silently open up at runtime).

use hbb_common::log;
use std::{collections::HashSet, sync::RwLock, time::Duration};

struct AllowState {
    active: bool,
    ids: HashSet<String>,
}

lazy_static::lazy_static! {
    static ref ALLOW: RwLock<AllowState> = RwLock::new(AllowState { active: false, ids: HashSet::new() });
}

fn allowlist_path() -> String {
    std::env::var("ALLOWLIST_FILE").unwrap_or_else(|_| "/shared/allowlist.json".to_string())
}

/// Should this id be DENIED? True only when an allowlist is active and the id
/// is not on it. Cheap, lock-guarded, no await. Fails open on a poisoned lock.
pub fn is_denied(id: &str) -> bool {
    match ALLOW.read() {
        Ok(s) => s.active && !s.ids.contains(id),
        Err(_) => false,
    }
}

fn load_once(path: &str) {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(arr) = v.get("ids").and_then(|x| x.as_array()) {
                    let ids: HashSet<String> = arr
                        .iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                        .collect();
                    let n = ids.len();
                    if let Ok(mut w) = ALLOW.write() {
                        *w = AllowState { active: true, ids };
                    }
                    log::debug!("SoCo allowlist loaded: {} id(s) allowed.", n);
                    return;
                }
            }
            log::warn!("SoCo allowlist: unparseable {}, keeping last state.", path);
        }
        Err(_) => {
            // No file: fresh install → stay inactive (allow all). If we were
            // already active, keep the last allowlist rather than opening up.
            if let Ok(w) = ALLOW.read() {
                if w.active {
                    log::warn!("SoCo allowlist file {} missing; keeping last allowlist.", path);
                }
            }
        }
    }
}

/// Start the background file watcher. Safe to call once at server startup.
pub fn start() {
    let path = allowlist_path();
    let interval = std::env::var("ALLOWLIST_INTERVAL_SEC")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(30)
        .max(5);
    log::info!("SoCo allowlist (default-deny) watching {} every {}s.", path, interval);
    std::thread::spawn(move || loop {
        load_once(&path);
        std::thread::sleep(Duration::from_secs(interval));
    });
}
