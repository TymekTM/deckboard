//! Extension manager: scans `~/deckboard/extensions`, loads every package
//! into its own JS runtime, dispatches `execute` calls and drives timers.
//!
//! Boa's interpreter is not `Send` (it uses `Rc` internally), so each
//! extension runtime is owned by a dedicated OS thread that answers
//! requests over an `std::sync::mpsc` channel. This mirrors the original
//! Electron design where every extension ran in its own hidden window.
//!
//! Memory model: a runtime costs its committed heap for the life of the
//! process (the OS heap never returns freed JS pages), so packages are
//! classified at load time. Extensions that registered timers while
//! loading stay resident (their ticks push tile values); execute-only
//! ones drop their runtime and spawn it on the first `execute`. Metadata
//! is cached on disk keyed by the source mtime, so after the first run a
//! package that stays lazy never evals JS at startup at all.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc as tokio_mpsc;
use tracing::warn;

use crate::host::{ExtRuntime, HostError, HostEvent};
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

/// Where a scanned package's runtime lives. Shared behind a mutex so a
/// lazy entry can promote itself to resident on the first execute.
enum Residence {
    /// Runtime thread is up since load (timer-driven extensions) or since
    /// the first execute promoted this entry.
    Resident(mpsc::Sender<ExtRequest>),
    /// Metadata is known but no runtime exists; spawn on first execute.
    Lazy { path: PathBuf, configs: Value },
    /// Load failed; kept for reporting only.
    Failed(String),
}

struct ExtEntry {
    package: String,
    name: String,
    actions: Vec<String>,
    residence: std::sync::Mutex<Residence>,
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
        let kind = match &*self.residence.lock().unwrap() {
            Residence::Resident(_) => "resident",
            Residence::Lazy { .. } => "lazy",
            Residence::Failed(_) => "failed",
        };
        f.debug_struct("ExtEntry")
            .field("package", &self.package)
            .field("name", &self.name)
            .field("actions", &self.actions)
            .field("residence", &kind)
            .finish()
    }
}

#[derive(Error, Debug)]
pub enum ManagerError {
    #[error("host error: {0}")]
    Host(#[from] HostError),
    #[error("extension thread unreachable: {0}")]
    Thread(String),
}

/// What the metadata cache stores per package (see `load` docs).
#[derive(Serialize, Deserialize)]
#[derive(Clone)]
struct CachedMeta {
    package: String,
    signature: u64,
    name: String,
    actions: Vec<String>,
    inputs: Vec<Value>,
    has_timers: bool,
}

/// (name, actions, raw inputs, registered timers?)
struct ProbeMeta {
    name: String,
    actions: Vec<String>,
    inputs: Vec<Value>,
    has_timers: bool,
}

pub struct ExtManager {
    entries: Vec<ExtEntry>,
    inputs: Vec<ExtInputInfo>,
    events_tx: tokio_mpsc::UnboundedSender<ExtEvent>,
}

impl ExtManager {
    /// Scan `dir` for packages (directories and `*.asar` files). Extension
    /// configs come from `settings.json` (same file the original desktop
    /// app writes: `{ "<package>": {...} }`). `skip` names packages
    /// replaced by native implementations - their JS never loads.
    pub fn load(
        dir: &std::path::Path,
        settings: &Value,
        skip: &[String],
    ) -> (Arc<ExtManager>, tokio_mpsc::UnboundedReceiver<ExtEvent>) {
        let (events_tx, events_rx) = tokio_mpsc::unbounded_channel();
        let mut entries = Vec::new();
        let mut all_inputs = Vec::new();
        let mut cache = MetadataCache::load();

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
            if skip.iter().any(|s| s == &package) {
                tracing::info!(package = %package, "extension replaced by native implementation - JS not loaded");
                continue;
            }
            let configs = settings
                .get(&package)
                .cloned()
                .unwrap_or_else(|| Value::Object(Default::default()));

            let signature = source_signature(&path);
            let cached = signature.and_then(|sig| cache.valid(&package, sig));
            let (meta, dispatch) = if let Some(cached) = cached {
                let meta = ProbeMeta {
                    name: cached.name,
                    actions: cached.actions,
                    inputs: cached.inputs,
                    has_timers: cached.has_timers,
                };
                if meta.has_timers {
                    // metadata known; still need the live runtime
                    tracing::info!(package = %package, "timer extension from metadata cache - loading runtime");
                    match spawn_runtime(&path, &package, &configs, &events_tx) {
                        Ok(dispatch) => (meta, Some(dispatch)),
                        Err(e) => {
                            warn!(package = %package, error = %e, "extension failed to load - disabled");
                            entries.push(ExtEntry {
                                name: meta.name.clone(),
                                package,
                                actions: meta.actions,
                                residence: std::sync::Mutex::new(Residence::Failed(e.to_string())),
                            });
                            continue;
                        }
                    }
                } else {
                    tracing::info!(package = %package, "extension metadata cached - JS not loaded until first execute");
                    (meta, None)
                }
            } else {
                // first run for this source version: one JS load to learn
                // what the package is and whether it needs to stay resident
                match load_extension(&path, &package, &configs, &events_tx) {
                    Ok((meta, dispatch)) => {
                        if let Some(sig) = signature {
                            cache.store(CachedMeta {
                                package: package.clone(),
                                signature: sig,
                                name: meta.name.clone(),
                                actions: meta.actions.clone(),
                                inputs: meta.inputs.clone(),
                                has_timers: meta.has_timers,
                            });
                        }
                        (meta, dispatch)
                    }
                    Err(e) => {
                        warn!(package = %package, error = %e, "extension failed to load - disabled");
                        entries.push(ExtEntry {
                            name: package.clone(),
                            package,
                            actions: Vec::new(),
                            residence: std::sync::Mutex::new(Residence::Failed(e.to_string())),
                        });
                        continue;
                    }
                }
            };

            all_inputs.extend(parse_inputs(&meta.inputs));
            match dispatch {
                Some(dispatch) => {
                    tracing::info!(package = %package, name = %meta.name, actions = ?meta.actions, "extension resident (timers)");
                    entries.push(ExtEntry {
                        package,
                        name: meta.name,
                        actions: meta.actions,
                        residence: std::sync::Mutex::new(Residence::Resident(dispatch)),
                    });
                }
                None => {
                    tracing::info!(package = %package, name = %meta.name, actions = ?meta.actions, "extension lazy (runtime loads on first execute)");
                    entries.push(ExtEntry {
                        package,
                        name: meta.name,
                        actions: meta.actions,
                        residence: std::sync::Mutex::new(Residence::Lazy { path, configs }),
                    });
                }
            }
        }

        cache.flush();
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
    /// the extension thread replies (it runs the JS synchronously). Lazy
    /// extensions pay a one-time runtime spawn here.
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
            let mut residence = entry.residence.lock().unwrap();
            match &mut *residence {
                Residence::Resident(dispatch) => {
                    return self.dispatch_to(dispatch, action, args);
                }
                Residence::Lazy { path, configs } => {
                    tracing::info!(package = %entry.package, "first execute - spawning extension runtime");
                    let dispatch = spawn_runtime(path, &entry.package, configs, &self.events_tx)?;
                    *residence = Residence::Resident(dispatch);
                    let Residence::Resident(dispatch) = &*residence else {
                        unreachable!("just assigned Resident")
                    };
                    return self.dispatch_to(dispatch, action, args);
                }
                Residence::Failed(error) => {
                    return Err(ManagerError::Host(HostError::Other(format!(
                        "extension {} is disabled (load error: {})",
                        entry.package, error
                    ))));
                }
            }
        }
        Err(ManagerError::Host(HostError::Other(format!(
            "no extension handles action '{action}'"
        ))))
    }

    fn dispatch_to(
        &self,
        dispatch: &mpsc::Sender<ExtRequest>,
        action: &str,
        args: Value,
    ) -> Result<(), ManagerError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        dispatch
            .send(ExtRequest::Execute {
                action: action.to_string(),
                args,
                reply: reply_tx,
            })
            .map_err(|_| ManagerError::Thread("runtime thread gone".into()))?;
        let events = reply_rx
            .recv_timeout(Duration::from_secs(30))
            .map_err(|_| ManagerError::Thread("extension did not answer in time".into()))??;
        for ev in events {
            Self::forward(&self.events_tx, ev);
        }
        Ok(())
    }

    /// Extension input declarations, flattened across scanned packages.
    pub fn inputs(&self) -> &[ExtInputInfo] {
        &self.inputs
    }

    pub fn summary(&self) -> Vec<(String, String, Option<String>)> {
        self.entries
            .iter()
            .map(|e| {
                let error = match &*e.residence.lock().unwrap() {
                    Residence::Failed(err) => Some(err.clone()),
                    _ => None,
                };
                (e.package.clone(), e.name.clone(), error)
            })
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

/// Load the package once: learn its metadata and, when it registered
/// timers, leave the runtime running on its thread (returned sender);
/// otherwise the runtime drops and `None` comes back.
fn load_extension(
    path: &Path,
    package: &str,
    configs: &Value,
    events_tx: &tokio_mpsc::UnboundedSender<ExtEvent>,
) -> Result<(ProbeMeta, Option<mpsc::Sender<ExtRequest>>), HostError> {
    // extract to a temp dir before spawning (plain IO, thread-agnostic)
    let source = crate::source::PackageSource::open(path, package.to_string())
        .map_err(|e| HostError::Other(e.to_string()))?;
    let root = source.root;
    let package = package.to_string();
    let configs = configs.clone();
    let events_tx = events_tx.clone();

    // The interpreter is created inside the thread and never crosses a
    // thread boundary afterwards (Boa is !Send).
    let (res_tx, res_rx) = mpsc::channel();
    // Boa recurses deeply on big bundles; default thread stacks are too small
    std::thread::Builder::new()
        .name(format!("ext-{package}"))
        .stack_size(64 * 1024 * 1024)
        .spawn(move || match ExtRuntime::load(&root, &package, &configs) {
            Ok(mut rt) => {
                let has_timers = rt.has_timers();
                let meta = ProbeMeta {
                    name: rt.name.clone(),
                    actions: rt.actions.clone(),
                    inputs: rt.inputs.clone(),
                    has_timers,
                };
                // drain setValue/interval events the load itself produced
                for ev in rt.drain() {
                    ExtManager::forward(&events_tx, ev);
                }
                if has_timers {
                    let (req_tx, req_rx) = mpsc::channel();
                    if res_tx.send(Ok((meta, Some(req_tx.clone())))).is_ok() {
                        runtime_loop(rt, req_rx, events_tx);
                    }
                } else {
                    let _ = res_tx.send(Ok((meta, None)));
                    // runtime drops: freed JS pages stay committed in the
                    // OS heap, which is exactly why lazy packages must not
                    // be re-probed on every start (the metadata cache)
                }
            }
            Err(e) => {
                let _ = res_tx.send(Err(e));
            }
        })
        .map_err(|e| HostError::Other(e.to_string()))?;

    res_rx
        .recv()
        .map_err(|_| HostError::Other("extension thread died".into()))?
}

/// Start a persistent runtime thread (cached timer extensions and lazy
/// first-executes). Blocks until the runtime is ready.
fn spawn_runtime(
    path: &Path,
    package: &str,
    configs: &Value,
    events_tx: &tokio_mpsc::UnboundedSender<ExtEvent>,
) -> Result<mpsc::Sender<ExtRequest>, HostError> {
    let source = crate::source::PackageSource::open(path, package.to_string())
        .map_err(|e| HostError::Other(e.to_string()))?;
    let root = source.root;
    let package = package.to_string();
    let configs = configs.clone();
    let events_tx = events_tx.clone();

    let (req_tx, req_rx) = mpsc::channel::<ExtRequest>();
    let (res_tx, res_rx) = mpsc::channel();
    std::thread::Builder::new()
        .name(format!("ext-{package}"))
        .stack_size(64 * 1024 * 1024)
        .spawn(move || match ExtRuntime::load(&root, &package, &configs) {
            Ok(rt) => {
                if res_tx.send(Ok(())).is_ok() {
                    runtime_loop(rt, req_rx, events_tx);
                }
            }
            Err(e) => {
                let _ = res_tx.send(Err(e));
            }
        })
        .map_err(|e| HostError::Other(e.to_string()))?;

    res_rx
        .recv()
        .map_err(|_| HostError::Other("extension thread died".into()))??;
    Ok(req_tx)
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

/// mtime-based source signature: covers asar edits and directory packages.
fn source_signature(path: &Path) -> Option<u64> {
    fn mtime_secs(p: &Path) -> Option<u64> {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
    }
    if path.is_dir() {
        let index = mtime_secs(&path.join("index.js")).unwrap_or(0);
        let manifest = mtime_secs(&path.join("package.json")).unwrap_or(0);
        Some(index.max(manifest))
    } else {
        mtime_secs(path)
    }
}

/// On-disk store of `CachedMeta`, so after the first run the manager can
/// classify packages without evaluating any JS. Best-effort: an unreadable
/// or missing cache only costs a probe.
struct MetadataCache {
    path: Option<PathBuf>,
    entries: HashMap<String, CachedMeta>,
    dirty: bool,
}

impl MetadataCache {
    fn load() -> Self {
        let path = dirs::cache_dir().map(|d| d.join("deckboard-server").join("extmeta.json"));
        let entries = path
            .as_deref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| serde_json::from_slice::<Vec<CachedMeta>>(&bytes).ok())
            .map(|list| list.into_iter().map(|m| (m.package.clone(), m)).collect())
            .unwrap_or_default();
        MetadataCache { path, entries, dirty: false }
    }

    fn valid(&self, package: &str, signature: u64) -> Option<CachedMeta> {
        let meta = self.entries.get(package)?;
        if meta.signature == signature {
            Some(meta.clone())
        } else {
            None
        }
    }

    fn store(&mut self, meta: CachedMeta) {
        self.entries.insert(meta.package.clone(), meta);
        self.dirty = true;
    }

    fn flush(&mut self) {
        if !self.dirty {
            return;
        }
        let Some(path) = &self.path else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_vec_pretty(&self.entries.values().collect::<Vec<_>>()) {
            Ok(bytes) => {
                if let Err(e) = std::fs::write(path, bytes) {
                    tracing::debug!(error = %e, "could not write extension metadata cache");
                }
            }
            Err(e) => tracing::debug!(error = %e, "could not serialize extension metadata cache"),
        }
    }
}
