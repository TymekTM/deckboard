//! Extension manager: scans `~/deckboard/extensions`, loads every package
//! into its own JS runtime, dispatches `execute` calls and drives timers.
//!
//! Boa's interpreter is not `Send` (it uses `Rc` internally), so each
//! extension runtime is owned by a dedicated OS thread that answers
//! requests over an `std::sync::mpsc` channel. This mirrors the original
//! Electron design where every extension ran in its own hidden window.

use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::sync::mpsc as tokio_mpsc;
use tracing::warn;

use crate::host::{ExtRuntime, HostEvent};
use thiserror::Error;

#[derive(Debug, Clone)]
pub enum ExtEvent {
    SetValue(Value),
}

/// Messages the manager sends to an extension thread.
pub enum ExtRequest {
    Execute {
        action: String,
        args: Value,
        reply: mpsc::Sender<Result<Vec<HostEvent>, crate::host::HostError>>,
    },
    Shutdown,
}

struct ExtEntry {
    package: String,
    name: String,
    actions: Vec<String>,
    /// Channel to the runtime thread; None while the extension failed to
    /// load (kept for reporting).
    dispatch: Option<mpsc::Sender<ExtRequest>>,
    error: Option<String>,
}

/// A flattened extension input declaration (`{label, value, icon, color,
/// fontIcon, mode, command}`) as used by the payload mapper for styles.
#[derive(Debug, Clone)]
pub struct ExtInputInfo {
    pub value: String,
    pub icon: Option<String>,
    pub color: Option<String>,
    pub font_icon: Option<String>,
    pub mode: Option<String>,
    pub command: Option<String>,
}

impl std::fmt::Debug for ExtEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtEntry")
            .field("package", &self.package)
            .field("name", &self.name)
            .field("actions", &self.actions)
            .field("loaded", &self.dispatch.is_some())
            .field("error", &self.error)
            .finish()
    }
}

#[derive(Error, Debug)]
pub enum ManagerError {
    #[error("host error: {0}")]
    Host(#[from] crate::host::HostError),
    #[error("extension thread unreachable: {0}")]
    Thread(String),
}

pub struct ExtManager {
    entries: Vec<ExtEntry>,
    inputs: Vec<ExtInputInfo>,
    events_tx: tokio_mpsc::UnboundedSender<ExtEvent>,
}

impl ExtManager {
    /// Scan `dir` for packages (directories and `*.asar` files), load each
    /// into its own runtime. Extension configs come from `settings.json`
    /// (same file the original desktop app writes: `{ "<package>": {...} }`).
    pub fn load(
        dir: &std::path::Path,
        settings: &Value,
    ) -> (Arc<ExtManager>, tokio_mpsc::UnboundedReceiver<ExtEvent>) {
        let (events_tx, events_rx) = tokio_mpsc::unbounded_channel();
        let mut entries = Vec::new();
        let mut all_inputs = Vec::new();

        let Some(list) = std::fs::read_dir(dir).ok() else {
            tracing::info!(dir = %dir.display(), "no extensions directory");
            return (
                Arc::new(ExtManager { entries, inputs: all_inputs, events_tx }),
                events_rx,
            );
        };

        let mut packages: Vec<(String, PathBuf)> = list
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                !name.starts_with('.') && name != "temp" && name != "database.db"
            })
            .map(|e| (e.path(), e.file_name().to_string_lossy().into_owned()))
            .map(|(p, name)| {
                if p.is_dir() {
                    (name, p)
                } else {
                    (name.trim_end_matches(".asar").to_string(), p)
                }
            })
            .collect();
        packages.sort();

        for (package, path) in packages {
            let configs = settings
                .get(&package)
                .cloned()
                .unwrap_or_else(|| Value::Object(Default::default()));
            match load_extension(&path, &package, &configs, events_tx.clone()) {
                Ok((name, actions, inputs, dispatch)) => {
                    tracing::info!(package = %package, name = %name, actions = ?actions, "extension loaded");
                    all_inputs.extend(inputs);
                    entries.push(ExtEntry { package, name, actions, dispatch: Some(dispatch), error: None });
                }
                Err(e) => {
                    warn!(package = %package, error = %e, "extension failed to load - disabled");
                    entries.push(ExtEntry {
                        name: package.clone(),
                        package,
                        actions: Vec::new(),
                        dispatch: None,
                        error: Some(e.to_string()),
                    });
                }
            }
        }

        (Arc::new(ExtManager { entries, inputs: all_inputs, events_tx }), events_rx)
    }

    /// Runtime threads are spawned at load time and self-manage their
    /// timers; nothing to start here anymore.
    pub fn start(&self) {}

    fn forward(tx: &tokio_mpsc::UnboundedSender<ExtEvent>, ev: HostEvent) {
        match ev {
            HostEvent::SetValue(v) => {
                tracing::debug!(target: "deckboard_ext", value = %v, "setValue event forwarded");
                let _ = tx.send(ExtEvent::SetValue(v));
            }
            HostEvent::Log(level, msg) => {
                match level.as_str() {
                    "warn" => warn!(target: "deckboard_ext", "{msg}"),
                    "error" => tracing::error!(target: "deckboard_ext", "{msg}"),
                    "debug" => tracing::debug!(target: "deckboard_ext", "{msg}"),
                    _ => tracing::info!(target: "deckboard_ext", "{msg}"),
                }
            }
            HostEvent::IntervalStart(..) | HostEvent::IntervalClear(_) => {}
        }
    }

    /// Does any extension handle this action type?
    pub fn has_action(&self, action: &str) -> bool {
        self.entries.iter().any(|e| e.actions.iter().any(|a| a == action))
    }

    /// Execute an action on whichever extension declared it. Blocks until
    /// the extension thread replies (it runs the JS synchronously).
    pub fn execute(&self, action: &str, command: Option<&str>) -> Result<(), ManagerError> {
        let args: Value = match command {
            Some(c) if !c.trim().is_empty() => serde_json::from_str(c)
                .unwrap_or_else(|_| Value::String(c.to_string())),
            _ => Value::Null,
        };
        for entry in &self.entries {
            if !entry.actions.iter().any(|a| a == action) {
                continue;
            }
            let Some(dispatch) = &entry.dispatch else {
                return Err(ManagerError::Host(crate::host::HostError::Other(format!(
                    "extension {} is disabled (load error: {})",
                    entry.package,
                    entry.error.as_deref().unwrap_or("unknown")
                ))));
            };
            let (reply_tx, reply_rx) = mpsc::channel();
            dispatch
                .send(ExtRequest::Execute {
                    action: action.to_string(),
                    args,
                    reply: reply_tx,
                })
                .map_err(|_| ManagerError::Thread(entry.package.clone()))?;
            let events = reply_rx
                .recv_timeout(Duration::from_secs(30))
                .map_err(|_| {
                    ManagerError::Thread(format!("{} did not answer in time", entry.package))
                })??;
            for ev in events {
                Self::forward(&self.events_tx, ev);
            }
            return Ok(());
        }
        Err(ManagerError::Host(crate::host::HostError::Other(format!(
            "no extension handles action '{action}'"
        ))))
    }

    /// Extension input declarations, flattened across loaded packages.
    pub fn inputs(&self) -> &[ExtInputInfo] {
        &self.inputs
    }

    pub fn summary(&self) -> Vec<(String, String, Option<String>)> {
        self.entries
            .iter()
            .map(|e| (e.package.clone(), e.name.clone(), e.error.clone()))
            .collect()
    }
}

/// Parse raw input JSON objects into mapper-ready style infos.
fn parse_inputs(raw: &[Value]) -> Vec<ExtInputInfo> {
    raw.iter()
        .filter_map(|i| {
            let value = i.get("value").and_then(Value::as_str)?.to_string();
            Some(ExtInputInfo {
                value,
                icon: i.get("icon").and_then(Value::as_str).map(str::to_string),
                color: i.get("color").and_then(Value::as_str).map(str::to_string),
                font_icon: i.get("fontIcon").and_then(Value::as_str).map(str::to_string),
                mode: i.get("mode").and_then(Value::as_str).map(str::to_string),
                command: i.get("command").and_then(Value::as_str).map(str::to_string),
            })
        })
        .collect()
}

fn load_extension(
    path: &std::path::Path,
    package: &str,
    configs: &Value,
    events_tx: tokio_mpsc::UnboundedSender<ExtEvent>,
) -> Result<(String, Vec<String>, Vec<ExtInputInfo>, mpsc::Sender<ExtRequest>), crate::host::HostError> {
    // extract to a temp dir before spawning (plain IO, thread-agnostic)
    let source = crate::source::PackageSource::open(path, package.to_string())
        .map_err(|e| crate::host::HostError::Other(e.to_string()))?;
    let root = source.root.clone();
    let package = package.to_string();
    let configs = configs.clone();

    // The interpreter is created inside the thread and never crosses a
    // thread boundary afterwards (Boa is !Send).
    let (req_tx, req_rx) = mpsc::channel::<ExtRequest>();
    let (res_tx, res_rx) = mpsc::channel();
    // Boa recurses deeply on big bundles; default thread stacks are too small
    std::thread::Builder::new()
        .name(format!("ext-{package}"))
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            let result = ExtRuntime::load(&root, &package, &configs);
            match result {
                Ok(rt) => {
                    let inputs = parse_inputs(&rt.inputs);
                    let ready = Ok((rt.name.clone(), rt.actions.clone(), inputs));
                    if res_tx.send(ready).is_ok() {
                        runtime_loop(rt, req_rx, events_tx);
                    }
                }
                Err(e) => {
                    let _ = res_tx.send(Err(e));
                }
            }
        })
        .map_err(|e| crate::host::HostError::Other(e.to_string()))?;

    let (name, actions, inputs) = res_rx
        .recv()
        .map_err(|_| crate::host::HostError::Other("extension thread died".into()))??;
    Ok((name, actions, inputs, req_tx))
}

fn runtime_loop(
    mut rt: ExtRuntime,
    req_rx: mpsc::Receiver<ExtRequest>,
    events_tx: tokio_mpsc::UnboundedSender<ExtEvent>,
) {
    let mut next_tick = Instant::now();
    loop {
        let now = Instant::now();
        if next_tick <= now {
            for ev in rt.tick_due() {
                ExtManager::forward(&events_tx, ev);
            }
            next_tick = now + rt
                .tick_granularity()
                .clamp(Duration::from_millis(50), Duration::from_secs(1));
        }
        match req_rx.recv_timeout(next_tick.saturating_duration_since(Instant::now())) {
            Ok(ExtRequest::Execute { action, args, reply }) => {
                // execute() drains setValue/interval events itself; the
                // manager forwards whatever the reply carries
                let _ = reply.send(rt.execute(&action, &args));
            }
            Ok(ExtRequest::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}
