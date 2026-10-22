//! In-app updates: `tauri-plugin-updater` wiring, commands and events.
//!
//! Distribution model (see `docs/UPDATING.md`):
//!   * GitHub Releases serve `latest.json` plus a signed `Agent Monitor.app`
//!     tarball (`Agent-Monitor-macos-arm64.app.tar.gz` + `.sig`),
//!   * the plugin verifies the **minisign signature** before anything touches
//!     the bundle on disk (signatures are mandatory; the endpoint manifest
//!     itself is not signed, only the artifact),
//!   * `download_and_install` swaps the `.app` in place; macOS keeps the old
//!     process alive, so the caller must restart to actually run the new binary.
//!
//! The whole module is deliberately UI-free: the frontend polls nothing and
//! never talks to the JS plugin — it calls our commands and listens to events.
//!
//! Everything network/update related is best-effort at launch: being offline,
//! behind a proxy or pointed at a dead endpoint must never disturb the pet.

use serde::{Deserialize, Serialize};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Url};
use tauri_plugin_updater::UpdaterExt;

/// Delay before the silent launch-time check. The pet and the first task
/// snapshot must be on screen first; the check is background noise.
const STARTUP_CHECK_DELAY: Duration = Duration::from_secs(4);
/// Debug-only auto-install start delay (nobody is watching the UI in that mode).
const AUTO_INSTALL_DELAY: Duration = Duration::from_secs(2);

/// Comma-separated endpoint override for tests/mirrors (see `docs/UPDATING.md`).
pub const ENDPOINT_ENV: &str = "AGENT_MONITOR_UPDATE_ENDPOINT";
/// Debug/acceptance only: `=1` turns a normal launch into
/// check → download → install → restart so a signed swap can be verified on a
/// copied `.app` without clicking. Never set for real users.
pub const AUTO_INSTALL_ENV: &str = "AGENT_MONITOR_UPDATE_AUTO_INSTALL";

pub const EVENT_AVAILABLE: &str = "update-available";
pub const EVENT_PROGRESS: &str = "update-progress";
pub const EVENT_INSTALLED: &str = "update-installed";

/// Result of a check, serialised both for the `check_for_update` command and
/// the `update-available` event. `null` fields stay present on purpose: the
/// settings UI reads the keys directly.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateCheck {
    pub current_version: String,
    pub available: bool,
    pub version: Option<String>,
    pub notes: Option<String>,
    pub pub_date: Option<String>,
}

impl UpdateCheck {
    fn up_to_date(current_version: String) -> Self {
        Self {
            current_version,
            available: false,
            version: None,
            notes: None,
            pub_date: None,
        }
    }
}

/// Payload of `update-progress`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct UpdateProgress {
    pub downloaded: u64,
    pub total: Option<u64>,
}

/// Payload of `update-installed`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UpdateInstalled {
    pub version: String,
}

// ── Commands ────────────────────────────────────────────────────

/// `check_for_update`: manual check (Settings → Updates). Returns the result to
/// the caller instead of broadcasting it — `update-available` belongs to the
/// silent startup check only.
#[tauri::command]
pub async fn check_for_update(app: AppHandle) -> Result<UpdateCheck, String> {
    check(&app).await
}

/// `install_update`: re-check, download, verify the signature, install over the
/// running bundle, emit progress + `update-installed`. Deliberately does **not**
/// restart; the UI decides when (the user may still be typing elsewhere).
#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    install(&app).await
}

/// `restart_app`: exit and relaunch the (now updated) bundle.
#[tauri::command]
pub fn restart_app(app: AppHandle) {
    app.restart();
}

// ── Core ────────────────────────────────────────────────────────

/// Build the updater, honouring `AGENT_MONITOR_UPDATE_ENDPOINT` when set.
fn updater_builder(app: &AppHandle) -> Result<tauri_plugin_updater::UpdaterBuilder, String> {
    let builder = app.updater_builder();
    match endpoint_override()? {
        Some(endpoints) => builder
            .endpoints(endpoints)
            .map_err(|err| format!("invalid updater endpoint: {err}")),
        None => Ok(builder),
    }
}

/// Run one check against the configured (or overridden) endpoints.
async fn check(app: &AppHandle) -> Result<UpdateCheck, String> {
    let current_version = app.package_info().version.to_string();
    let updater = updater_builder(app)?
        .build()
        .map_err(|err| format!("updater unavailable: {err}"))?;
    let update = updater
        .check()
        .await
        .map_err(|err| format!("update check failed: {err}"))?;

    Ok(match update {
        Some(update) => UpdateCheck {
            current_version,
            available: true,
            version: Some(update.version.clone()),
            notes: update.body.clone(),
            pub_date: update
                .date
                .map(|date| rfc3339_from_unix(date.unix_timestamp())),
        },
        None => UpdateCheck::up_to_date(current_version),
    })
}

/// Check again, then download + install. Progress is emitted while the bytes
/// arrive (throttled), `update-installed` after the bundle was swapped.
async fn install(app: &AppHandle) -> Result<(), String> {
    let updater = updater_builder(app)?
        .build()
        .map_err(|err| format!("updater unavailable: {err}"))?;
    let update = updater
        .check()
        .await
        .map_err(|err| format!("update check failed: {err}"))?
        .ok_or_else(|| "already up to date".to_string())?;

    let version = update.version.clone();
    let handle = app.clone();
    let mut downloaded: u64 = 0;
    let mut emitted: u64 = 0;
    let mut total: Option<u64> = None;

    update
        .download_and_install(
            |chunk_len, content_length| {
                if content_length.is_some() {
                    total = content_length;
                }
                downloaded += chunk_len as u64;
                let step = progress_step(total);
                if downloaded.saturating_sub(emitted) >= step {
                    emitted = downloaded;
                    let _ = handle.emit(
                        EVENT_PROGRESS,
                        UpdateProgress {
                            downloaded,
                            total,
                        },
                    );
                }
            },
            || {},
        )
        .await
        .map_err(|err| format!("update install failed: {err}"))?;

    // Final 100 % notification: the server may not send Content-Length at all.
    let _ = app.emit(
        EVENT_PROGRESS,
        UpdateProgress {
            downloaded: total.unwrap_or(downloaded),
            total,
        },
    );
    let _ = app.emit(EVENT_INSTALLED, UpdateInstalled { version });
    Ok(())
}

// ── Startup auto-check + debug auto-install ─────────────────────

/// Silent launch-time check (`auto_check_updates`, on by default): ~4 s after
/// start, check once and emit `update-available` only when there is something
/// newer. Failures (offline, 404, DNS, TLS) are logged and otherwise ignored.
pub fn spawn_startup_check(app: &AppHandle) {
    if auto_check_updates(app) && !auto_install_requested() {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            sleep_async(STARTUP_CHECK_DELAY).await;
            match check(&app).await {
                Ok(found) if found.available => {
                    eprintln!(
                        "update: {} available (running {})",
                        found.version.as_deref().unwrap_or("?"),
                        found.current_version
                    );
                    let _ = app.emit(EVENT_AVAILABLE, &found);
                }
                Ok(_) => {}
                Err(err) => eprintln!("update: startup check failed (ignored): {err}"),
            }
        });
    }
}

/// Debug/acceptance only (`AGENT_MONITOR_UPDATE_AUTO_INSTALL=1`): launch,
/// check, download, install and restart without user interaction so the whole
/// signed swap can be verified on a copied `.app`. When no update is available
/// it exits 0 instead of starting the UI, which keeps scripted runs short.
pub fn spawn_auto_install_if_requested(app: &AppHandle) {
    if !auto_install_requested() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        sleep_async(AUTO_INSTALL_DELAY).await;
        match check(&app).await {
            Ok(found) if found.available => match install(&app).await {
                Ok(()) => {
                    eprintln!("update: installed {}; restarting", found.version.as_deref().unwrap_or("?"));
                    app.restart();
                }
                Err(err) => {
                    eprintln!("update: auto-install failed: {err}");
                    std::process::exit(1);
                }
            },
            Ok(found) => {
                eprintln!(
                    "update: auto-install: already at {} (nothing to do)",
                    found.current_version
                );
                std::process::exit(0);
            }
            Err(err) => {
                eprintln!("update: auto-install check failed: {err}");
                std::process::exit(1);
            }
        }
    });
}

fn auto_check_updates(app: &AppHandle) -> bool {
    crate::settings_snapshot(app).auto_check_updates
}

fn auto_install_requested() -> bool {
    std::env::var(AUTO_INSTALL_ENV).as_deref() == Ok("1")
}

/// Await a delay without parking an async worker: the sleep runs on the
/// blocking pool (Tauri does not re-export the runtime's timer).
async fn sleep_async(delay: Duration) {
    let _ = tauri::async_runtime::spawn_blocking(move || std::thread::sleep(delay)).await;
}

// ── Headless entry (`--check-update`) ───────────────────────────

/// `agent-monitor --check-update`: print one `UpdateCheck` as JSON and exit.
///
/// Builds a minimal Tauri context (no windows, so nothing flashes) purely to get
/// the plugin's configured endpoints/pubkey; `AGENT_MONITOR_UPDATE_ENDPOINT` is
/// honoured like in the GUI. Exit codes: 0 = checked (available or not),
/// 1 = check failed, 2 = could not initialise the app context.
pub fn check_update_headless() {
    let app = match tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .build(tauri::generate_context!())
    {
        Ok(app) => app,
        Err(err) => {
            eprintln!("update: failed to initialise app context: {err}");
            std::process::exit(2);
        }
    };
    match tauri::async_runtime::block_on(check(app.handle())) {
        Ok(found) => {
            match serde_json::to_string_pretty(&found) {
                Ok(json) => println!("{json}"),
                Err(err) => {
                    eprintln!("update: failed to serialise check result: {err}");
                    std::process::exit(2);
                }
            }
            std::process::exit(0);
        }
        Err(err) => {
            eprintln!("update: {err}");
            std::process::exit(1);
        }
    }
}

// ── Pure helpers (unit-tested) ──────────────────────────────────

/// Parse the endpoint override: comma-separated absolute `http(s)` URLs.
///
/// Blank entries are skipped, everything else must parse and use `http`/`https`
/// — a typo should produce a readable error instead of silently falling back to
/// the GitHub endpoint.
pub fn parse_endpoints(raw: &str) -> Result<Vec<Url>, String> {
    let mut endpoints = Vec::new();
    for part in raw.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let url = Url::parse(part).map_err(|err| format!("invalid update endpoint `{part}`: {err}"))?;
        match url.scheme() {
            "http" | "https" => {}
            other => {
                return Err(format!(
                    "unsupported update endpoint scheme `{other}` in `{part}` (expected http or https)"
                ))
            }
        }
        endpoints.push(url);
    }
    if endpoints.is_empty() {
        return Err("no update endpoint given (set at least one http(s) URL)".into());
    }
    Ok(endpoints)
}

/// `Some(endpoints)` when `AGENT_MONITOR_UPDATE_ENDPOINT` is set and non-blank.
fn endpoint_override() -> Result<Option<Vec<Url>>, String> {
    let Ok(raw) = std::env::var(ENDPOINT_ENV) else {
        return Ok(None);
    };
    if raw.trim().is_empty() {
        return Ok(None);
    }
    parse_endpoints(&raw).map(Some)
}

/// Minimum byte delta between two `update-progress` events: ~20 events for a
/// normal download, never less than 128 KiB (a chunk per event would flood IPC).
fn progress_step(total: Option<u64>) -> u64 {
    const MIN_STEP: u64 = 128 * 1024;
    total.map(|total| (total / 20).max(MIN_STEP)).unwrap_or(MIN_STEP)
}

/// RFC 3339 UTC timestamp (`2026-10-22T09:00:00Z`) from a unix timestamp.
///
/// The plugin returns `time::OffsetDateTime`; formatting it without pulling the
/// `time` formatting features in as a new direct dependency keeps this crate's
/// dependency surface at exactly one new crate (the updater plugin) and makes
/// the format unit-testable.
pub fn rfc3339_from_unix(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (hour, minute, second) = (rem / 3600, (rem % 3600) / 60, rem % 60);

    // Howard Hinnant's civil_from_days: days since 1970-01-01 to y/m/d.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

// ── Tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_check_serialises_the_documented_shape() {
        let available = UpdateCheck {
            current_version: "0.6.0".into(),
            available: true,
            version: Some("0.7.0".into()),
            notes: Some("Tray fixes".into()),
            pub_date: Some("2026-10-22T09:00:00Z".into()),
        };
        let json = serde_json::to_value(&available).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "current_version": "0.6.0",
                "available": true,
                "version": "0.7.0",
                "notes": "Tray fixes",
                "pub_date": "2026-10-22T09:00:00Z",
            })
        );

        // Up to date: every key stays present (null), the UI reads them directly.
        let current = UpdateCheck::up_to_date("0.6.0".into());
        let json = serde_json::to_value(&current).unwrap();
        assert_eq!(json["available"], false);
        assert!(json.get("version").is_some() && json["version"].is_null());
        assert!(json.get("notes").is_some() && json["notes"].is_null());
        assert!(json.get("pub_date").is_some() && json["pub_date"].is_null());
    }

    #[test]
    fn update_check_roundtrips_through_json() {
        let found = UpdateCheck {
            current_version: "0.6.0".into(),
            available: true,
            version: Some("0.7.0".into()),
            notes: None,
            pub_date: None,
        };
        let raw = serde_json::to_string(&found).unwrap();
        assert_eq!(serde_json::from_str::<UpdateCheck>(&raw).unwrap(), found);
    }

    #[test]
    fn endpoint_override_parses_a_comma_separated_list() {
        let endpoints = parse_endpoints(
            " http://127.0.0.1:8765/latest.json ,, https://mirror.example.com/am/latest.json ",
        )
        .expect("valid endpoints");
        assert_eq!(endpoints.len(), 2);
        assert_eq!(endpoints[0].as_str(), "http://127.0.0.1:8765/latest.json");
        assert_eq!(
            endpoints[1].as_str(),
            "https://mirror.example.com/am/latest.json"
        );
    }

    #[test]
    fn endpoint_override_rejects_blank_typo_and_foreign_schemes() {
        assert!(parse_endpoints("").is_err());
        assert!(parse_endpoints(" , , ").is_err());
        assert!(parse_endpoints("not a url").is_err());
        assert!(parse_endpoints("ftp://example.com/latest.json").is_err());
        assert!(parse_endpoints("file:///tmp/latest.json").is_err());
        // One bad entry rejects the whole override instead of half-applying it.
        assert!(
            parse_endpoints("https://ok.example.com/latest.json,ftp://nope/latest.json").is_err()
        );
    }

    #[test]
    fn progress_is_throttled_to_about_twenty_events() {
        assert_eq!(progress_step(None), 128 * 1024);
        assert_eq!(progress_step(Some(1024)), 128 * 1024);
        assert_eq!(progress_step(Some(20 * 1024 * 1024)), 1024 * 1024);
    }

    #[test]
    fn pub_dates_format_as_rfc3339_utc() {
        assert_eq!(rfc3339_from_unix(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_from_unix(1_704_164_645), "2024-01-02T03:04:05Z");
        // Leap day of a leap year, and a date before the epoch.
        assert_eq!(rfc3339_from_unix(1_709_164_800), "2024-02-29T00:00:00Z");
        assert_eq!(rfc3339_from_unix(-86_400), "1969-12-31T00:00:00Z");
    }

    #[test]
    fn release_manifest_shape_is_accepted_by_the_updater() {
        // Exactly what scripts/make-updater-artifact.sh writes (and what the
        // release workflow attaches): pins the schema the plugin parses.
        let manifest = r#"{
          "version": "0.7.1",
          "notes": "Tray flicker + jump fixes",
          "pub_date": "2026-10-22T09:00:00Z",
          "platforms": {
            "darwin-aarch64": {
              "signature": "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNpZ25lcgpsYWI=",
              "url": "https://github.com/Hoemr/agent-monitor/releases/download/v0.7.1/Agent-Monitor-macos-arm64.app.tar.gz"
            }
          }
        }"#;
        let release: tauri_plugin_updater::RemoteRelease =
            serde_json::from_str(manifest).expect("latest.json must parse");
        assert_eq!(release.version.to_string(), "0.7.1");
        assert_eq!(release.notes.as_deref(), Some("Tray flicker + jump fixes"));
        assert!(release.pub_date.is_some());

        // macOS resolves the target as `darwin-aarch64` (with an optional
        // `-app` bundle-type suffix tried first).
        assert_eq!(tauri_plugin_updater::target().as_deref(), Some("darwin-aarch64"));
        assert_eq!(
            release.download_url("darwin-aarch64").unwrap().as_str(),
            "https://github.com/Hoemr/agent-monitor/releases/download/v0.7.1/Agent-Monitor-macos-arm64.app.tar.gz"
        );
        assert!(release
            .signature("darwin-aarch64")
            .unwrap()
            .starts_with("dW50cnVz"));
        // A manifest without the platform must fail loudly, not silently.
        assert!(release.download_url("darwin-x86_64").is_err());
    }
}
