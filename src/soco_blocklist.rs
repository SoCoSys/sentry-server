// SoCo Sentry — server-side device blocklist.
//
// Polls the portal's /api/blocklist (the single source of truth) and holds the
// blocked RustDesk IDs in memory. hbbs consults this in the RegisterPk and
// PunchHoleRequest paths so a blocked device can neither register (come online)
// nor be connected to.
//
// Fails OPEN: any fetch/parse error keeps the last known set (empty at boot),
// so a portal outage never takes the relay down. Enabled only when both
// BLOCKLIST_URL and BLOCKLIST_TOKEN env vars are set.

use hbb_common::log;
use std::{collections::HashSet, sync::RwLock, time::Duration};

lazy_static::lazy_static! {
    static ref ID_BLOCKLIST: RwLock<HashSet<String>> = RwLock::new(HashSet::new());
}

/// Is this device id currently blocked? Cheap, lock-guarded, no await.
pub fn is_blocked(id: &str) -> bool {
    ID_BLOCKLIST.read().map(|s| s.contains(id)).unwrap_or(false)
}

/// Start the background poller (no-op if not configured). Safe to call once at
/// server startup; spawns its own thread with a blocking HTTP client so it is
/// independent of the tokio runtime.
pub fn start() {
    let url = std::env::var("BLOCKLIST_URL").unwrap_or_default();
    let token = std::env::var("BLOCKLIST_TOKEN").unwrap_or_default();
    if url.is_empty() || token.is_empty() {
        log::info!("SoCo blocklist disabled (set BLOCKLIST_URL + BLOCKLIST_TOKEN to enable).");
        return;
    }
    let interval = std::env::var("BLOCKLIST_INTERVAL_SEC")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(60)
        .max(10);
    log::info!("SoCo blocklist enabled: polling every {}s.", interval);

    std::thread::spawn(move || {
        let client = match reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                log::error!("SoCo blocklist: HTTP client build failed: {}", e);
                return;
            }
        };
        loop {
            match client.get(&url).header("X-Blocklist-Token", &token).send() {
                Ok(resp) if resp.status().is_success() => match resp.text() {
                    Ok(text) => {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                            if let Some(arr) = v.get("ids").and_then(|x| x.as_array()) {
                                let set: HashSet<String> = arr
                                    .iter()
                                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                                    .collect();
                                let n = set.len();
                                if let Ok(mut w) = ID_BLOCKLIST.write() {
                                    *w = set;
                                }
                                log::debug!("SoCo blocklist updated: {} id(s).", n);
                            }
                        } else {
                            log::warn!("SoCo blocklist: bad JSON, keeping last set.");
                        }
                    }
                    Err(e) => log::warn!("SoCo blocklist: read error, keeping last set: {}", e),
                },
                Ok(resp) => log::warn!("SoCo blocklist: HTTP {}, keeping last set.", resp.status()),
                Err(e) => log::warn!("SoCo blocklist: fetch failed, keeping last set: {}", e),
            }
            std::thread::sleep(Duration::from_secs(interval));
        }
    });
}
