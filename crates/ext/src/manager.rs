//! Extension manager: scans `~/pulpitApp/extensions`, loads every package
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
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc as tokio_mpsc;
use tracing::warn;

use crate::host::{ExtRuntime, HostError, HostEvent};
use thiserror::Error;

/// One probe answer: metadata plus the channel to a spawned runtime, or the
/// load error. A `type` alias keeps the plan enum below readable.
type ProbeResponseRx =
    mpsc::Receiver<Result<(ProbeMeta, Option<mpsc::Sender<ExtRequest>>), HostError>>;

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
    /// Human action label from the extension declaration.
    pub label: Option<String>,
    /// Display name of the extension package that declared this action;
    /// the editor groups extension actions under it.
    pub extension: String,
    /// Per-action parameter fields declared by the extension (`inputs`).
    pub fields: Vec<ExtFieldInfo>,
}

/// One parameter field of an extension action: how the editor (and the
/// original renderer) builds the button dialog for that action.
#[derive(Debug, Clone)]
pub struct ExtFieldInfo {
    /// INPUT_METHOD string, e.g. "input:text" / "input:select".
    pub kind: String,
    pub label: String,
    /// JSON key of the value inside the button's command object.
    pub key: String,
    /// Choices for `input:select` fields.
    pub items: Vec<(String, String)>,
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
#[derive(Serialize, Deserialize, Clone)]
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
    /// Connected tablet clients, updated by the embedding app. Timer
    /// threads read it to stretch their tick cadence while nobody is
    /// watching: zero clients means the pushed values go unseen, so
    /// ticking at full speed would be pure idle CPU.
    active_clients: Arc<AtomicUsize>,
    /// How long a runtime spawn may take before the package is reported
    /// failed (see [`JS_LOAD_TIMEOUT`]).
    spawn_timeout: Duration,
}

/// When no clients are connected, timer ticks wait at least this long,
/// whatever the extension requested (see `runtime_loop`).
pub const IDLE_TICK_FLOOR: Duration = Duration::from_secs(15);

impl ExtManager {
    /// Scan `dir` for packages (directories and `*.asar` files). Extension
    /// configs come from `settings.json` (same file the original desktop
    /// app writes: `{ "<package>": {...} }`). `skip` names packages
    /// replaced by native implementations - their JS never loads.
    ///
    /// A runtime spawn may take up to [`JS_LOAD_TIMEOUT`] before the
    /// package is reported failed.
    pub fn load(
        dir: &std::path::Path,
        settings: &Value,
        skip: &[String],
    ) -> (Arc<ExtManager>, tokio_mpsc::UnboundedReceiver<ExtEvent>) {
        Self::load_with_spawn_timeout(dir, settings, skip, JS_LOAD_TIMEOUT)
    }

    /// [`load`][Self::load] with an explicit spawn timeout. The 30 s
    /// production bound is impractical in tests, which use milliseconds
    /// to prove the wedged-package paths.
    pub fn load_with_spawn_timeout(
        dir: &std::path::Path,
        settings: &Value,
        skip: &[String],
        spawn_timeout: Duration,
    ) -> (Arc<ExtManager>, tokio_mpsc::UnboundedReceiver<ExtEvent>) {
        let (events_tx, events_rx) = tokio_mpsc::unbounded_channel();
        let entries: Vec<ExtEntry> = Vec::new();
        let mut all_inputs = Vec::new();
        let mut cache = MetadataCache::load();
        let active_clients = Arc::new(AtomicUsize::new(0));

        let Some(list) = std::fs::read_dir(dir).ok() else {
            tracing::info!(dir = %dir.display(), "no extensions directory");
            return (
                Arc::new(ExtManager {
                    entries,
                    inputs: all_inputs,
                    events_tx,
                    active_clients,
                    spawn_timeout,
                }),
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

        // One JS probe per cold package runs on its own thread, all in
        // parallel; a single wedged extension must not stall the fleet.
        // The deadline bounds the whole wait, matching the original app's
        // tolerance for slow extensions without hanging startup forever.
        const LOAD_DEADLINE: Duration = Duration::from_secs(30);
        let deadline = Instant::now() + LOAD_DEADLINE;

        enum Plan {
            /// Cache hit, no timers: metadata only, runtime spawns lazily.
            Lazy { meta: ProbeMeta },
            /// Cache hit with timers: runtime spawned during planning.
            Resident {
                meta: ProbeMeta,
                dispatch: Option<mpsc::Sender<ExtRequest>>,
            },
            Failed {
                meta: Option<ProbeMeta>,
                error: String,
            },
            /// Cold cache: probe thread still running.
            Pending {
                rx: ProbeResponseRx,
                signature: Option<u64>,
            },
        }

        let mut plans: Vec<(String, PathBuf, Plan)> = Vec::new();
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
            let plan = if let Some(cached) = cached {
                let meta = ProbeMeta {
                    name: cached.name,
                    actions: cached.actions,
                    inputs: cached.inputs,
                    has_timers: cached.has_timers,
                };
                if meta.has_timers {
                    // metadata known; still need the live runtime (thread
                    // spawn, no JS on this thread)
                    tracing::info!(package = %package, "timer extension from metadata cache - loading runtime");
                    match spawn_runtime(
                        &path,
                        &package,
                        &configs,
                        &events_tx,
                        &active_clients,
                        spawn_timeout,
                    ) {
                        Ok(dispatch) => Plan::Resident {
                            meta,
                            dispatch: Some(dispatch),
                        },
                        Err(e) => {
                            warn!(package = %package, error = %e, "extension failed to load - disabled");
                            Plan::Failed {
                                meta: Some(meta),
                                error: e.to_string(),
                            }
                        }
                    }
                } else {
                    tracing::info!(package = %package, "extension metadata cached - JS not loaded until first execute");
                    Plan::Lazy { meta }
                }
            } else {
                // first run for this source version: one JS load (on its
                // own thread) to learn what the package is
                let (res_tx, res_rx) = mpsc::channel();
                let spawned = std::thread::Builder::new()
                    .name(format!("ext-probe-{package}"))
                    .stack_size(64 * 1024 * 1024)
                    .spawn({
                        let path = path.clone();
                        let package = package.clone();
                        let events_tx = events_tx.clone();
                        let active_clients = active_clients.clone();
                        move || {
                            let _ = res_tx.send(load_extension(
                                &path,
                                &package,
                                &configs,
                                &events_tx,
                                &active_clients,
                            ));
                        }
                    });
                match spawned {
                    Ok(_) => Plan::Pending {
                        rx: res_rx,
                        signature,
                    },
                    Err(e) => Plan::Failed {
                        meta: None,
                        error: format!("probe thread spawn: {e}"),
                    },
                }
            };
            plans.push((package, path, plan));
        }

        let mut entries: Vec<ExtEntry> = Vec::new();
        for (package, path, plan) in plans {
            let (meta, dispatch) = match plan {
                Plan::Lazy { meta } => (meta, None),
                Plan::Resident { meta, dispatch } => (meta, dispatch),
                Plan::Failed { meta, error } => {
                    entries.push(ExtEntry {
                        name: meta
                            .as_ref()
                            .map(|m| m.name.clone())
                            .unwrap_or_else(|| package.clone()),
                        package,
                        actions: meta.map(|m| m.actions).unwrap_or_default(),
                        residence: std::sync::Mutex::new(Residence::Failed(error)),
                    });
                    continue;
                }
                Plan::Pending { rx, signature } => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    let result = match rx.recv_timeout(remaining) {
                        Ok(result) => result,
                        Err(_) => {
                            warn!(package = %package, "load timed out after 30s - disabled");
                            entries.push(ExtEntry {
                                name: package.clone(),
                                package,
                                actions: Vec::new(),
                                residence: std::sync::Mutex::new(Residence::Failed(
                                    "load timed out after 30s".into(),
                                )),
                            });
                            continue;
                        }
                    };
                    match result {
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
                }
            };

            all_inputs.extend(parse_inputs(&meta.inputs, &meta.name));
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
                    let configs = settings
                        .get(&package)
                        .cloned()
                        .unwrap_or_else(|| Value::Object(Default::default()));
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
        (
            Arc::new(ExtManager {
                entries,
                inputs: all_inputs,
                events_tx,
                active_clients: active_clients.clone(),
                spawn_timeout,
            }),
            events_rx,
        )
    }

    /// Report the number of connected clients. Timer threads stretch
    /// their cadence to `IDLE_TICK_FLOOR` while it reads zero, so an
    /// idle tray app does not burn CPU pushing values nobody reads.
    pub fn set_activity(&self, clients: usize) {
        self.active_clients.store(clients, Ordering::Relaxed);
    }

    /// Runtime threads are spawned at load time and self-manage their
    /// timers; nothing to start here anymore.
    pub fn start(&self) {}

    fn forward(tx: &tokio_mpsc::UnboundedSender<ExtEvent>, ev: HostEvent) {
        match ev {
            HostEvent::SetValue(v) => {
                tracing::debug!(target: "pulpit_ext", value = %v, "setValue event forwarded");
                let _ = tx.send(ExtEvent::SetValue(v));
            }
            HostEvent::Log(level, msg) => match level.as_str() {
                "warn" => warn!(target: "pulpit_ext", "{msg}"),
                "error" => tracing::error!(target: "pulpit_ext", "{msg}"),
                "debug" => tracing::debug!(target: "pulpit_ext", "{msg}"),
                _ => tracing::info!(target: "pulpit_ext", "{msg}"),
            },
            HostEvent::IntervalStart(..) | HostEvent::IntervalClear(_) => {}
        }
    }

    /// Does any extension handle this action type?
    pub fn has_action(&self, action: &str) -> bool {
        self.entries
            .iter()
            .any(|e| e.actions.iter().any(|a| a == action))
    }

    /// Execute an action on whichever extension declared it. Blocks until
    /// the extension thread replies (it runs the JS synchronously). Lazy
    /// extensions pay a one-time runtime spawn here. The residence lock is
    /// never held across the blocking dispatch or the spawn: a wedged
    /// extension stalls only its own action, not `summary` or another
    /// package's promotion.
    pub fn execute(&self, action: &str, command: Option<&str>) -> Result<(), ManagerError> {
        let args: Value = match command {
            Some(c) if !c.trim().is_empty() => {
                serde_json::from_str(c).unwrap_or_else(|_| Value::String(c.to_string()))
            }
            _ => Value::Null,
        };
        for entry in &self.entries {
            if !entry.actions.iter().any(|a| a == action) {
                continue;
            }
            // snapshot what to do without holding the lock through the
            // slow parts below
            enum Snap {
                Live(mpsc::Sender<ExtRequest>),
                Lazy { path: PathBuf, configs: Value },
                Failed(String),
            }
            let snap = {
                let mut residence = entry.residence.lock().unwrap();
                match &mut *residence {
                    Residence::Resident(dispatch) => Snap::Live(dispatch.clone()),
                    Residence::Lazy { path, configs } => Snap::Lazy {
                        path: path.clone(),
                        configs: configs.clone(),
                    },
                    Residence::Failed(error) => Snap::Failed(error.clone()),
                }
            };
            let dispatch = match snap {
                Snap::Live(dispatch) => dispatch,
                Snap::Failed(error) => {
                    return Err(ManagerError::Host(HostError::Other(format!(
                        "extension {} is disabled (load error: {})",
                        entry.package, error
                    ))));
                }
                Snap::Lazy { path, configs } => {
                    tracing::info!(package = %entry.package, "first execute - spawning extension runtime");
                    let dispatch = match spawn_runtime(
                        &path,
                        &entry.package,
                        &configs,
                        &self.events_tx,
                        &self.active_clients,
                        self.spawn_timeout,
                    ) {
                        Ok(dispatch) => dispatch,
                        Err(e) => {
                            // A failed spawn disables the package for the
                            // rest of the session: the load already had
                            // its timeout, so a later execute must fail
                            // fast instead of spawning (and waiting)
                            // again. A racing execute that promoted the
                            // entry meanwhile keeps its live runtime.
                            let mut residence = entry.residence.lock().unwrap();
                            if matches!(&*residence, Residence::Lazy { .. }) {
                                warn!(package = %entry.package, error = %e, "extension failed to load - disabled");
                                *residence = Residence::Failed(e.to_string());
                            }
                            return Err(e.into());
                        }
                    };
                    // a racing execute may have promoted the entry first;
                    // whoever lost drops its extra channel and the loser
                    // thread exits when its receiver disconnects
                    let mut residence = entry.residence.lock().unwrap();
                    match &mut *residence {
                        Residence::Resident(existing) => existing.clone(),
                        _ => {
                            *residence = Residence::Resident(dispatch.clone());
                            dispatch
                        }
                    }
                }
            };
            return self.dispatch_to(&dispatch, action, args);
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

/// Parse raw input JSON objects into mapper-ready style infos. `extension`
/// is the declaring package's display name, stamped on every input.
fn parse_inputs(raw: &[Value], extension: &str) -> Vec<ExtInputInfo> {
    raw.iter()
        .filter_map(|i| {
            let value = i.get("value").and_then(Value::as_str)?.to_string();
            // field declarations come as "inputs" (pulpit-extension-kit)
            // or "input" (older pulpit-kit) - accept both spellings
            let fields = i
                .get("inputs")
                .or_else(|| i.get("input"))
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|f| {
                            let kind = f.get("type").and_then(Value::as_str)?.to_string();
                            let label = f.get("label").and_then(Value::as_str)?.to_string();
                            let key = f.get("ref").and_then(Value::as_str)?.to_string();
                            let items = f
                                .get("items")
                                .and_then(Value::as_array)
                                .map(|items| {
                                    items
                                        .iter()
                                        .filter_map(|it| {
                                            let v = it.get("value").and_then(Value::as_str)?;
                                            let l = it.get("label").and_then(Value::as_str)?;
                                            Some((v.to_string(), l.to_string()))
                                        })
                                        .collect()
                                })
                                .unwrap_or_default();
                            Some(ExtFieldInfo {
                                kind,
                                label,
                                key,
                                items,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(ExtInputInfo {
                value,
                icon: i.get("icon").and_then(Value::as_str).map(str::to_string),
                color: i.get("color").and_then(Value::as_str).map(str::to_string),
                font_icon: i
                    .get("fontIcon")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                mode: i.get("mode").and_then(Value::as_str).map(str::to_string),
                command: i.get("command").and_then(Value::as_str).map(str::to_string),
                label: i.get("label").and_then(Value::as_str).map(str::to_string),
                extension: extension.to_string(),
                fields,
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
    active_clients: &Arc<AtomicUsize>,
) -> Result<(ProbeMeta, Option<mpsc::Sender<ExtRequest>>), HostError> {
    // extract to a temp dir before spawning (plain IO, thread-agnostic)
    let source = crate::source::PackageSource::open(path, package.to_string())
        .map_err(|e| HostError::Other(e.to_string()))?;
    let root = source.root;
    let package = package.to_string();
    let configs = configs.clone();
    let events_tx = events_tx.clone();
    let active_clients = active_clients.clone();

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
                        runtime_loop(
                            ExtSlot {
                                root,
                                package,
                                configs,
                                live: Some(rt),
                            },
                            req_rx,
                            events_tx,
                            active_clients,
                        );
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

/// How long one extension's JS may take to load (probe at startup and
/// lazy first-execute spawns). Beyond it the package is reported failed:
/// a wedged extension must not stall startup or a tile tap forever.
const JS_LOAD_TIMEOUT: Duration = Duration::from_secs(30);

/// Start a persistent runtime thread (cached timer extensions and lazy
/// first-executes). Blocks until the runtime is ready, at most
/// `load_timeout`: a package whose top-level JS never finishes loading
/// errors instead of pinning the caller forever.
fn spawn_runtime(
    path: &Path,
    package: &str,
    configs: &Value,
    events_tx: &tokio_mpsc::UnboundedSender<ExtEvent>,
    active_clients: &Arc<AtomicUsize>,
    load_timeout: Duration,
) -> Result<mpsc::Sender<ExtRequest>, HostError> {
    let source = crate::source::PackageSource::open(path, package.to_string())
        .map_err(|e| HostError::Other(e.to_string()))?;
    let root = source.root;
    let package = package.to_string();
    let configs = configs.clone();
    let events_tx = events_tx.clone();
    let active_clients = active_clients.clone();

    let (req_tx, req_rx) = mpsc::channel::<ExtRequest>();
    let (res_tx, res_rx) = mpsc::channel();
    std::thread::Builder::new()
        .name(format!("ext-{package}"))
        .stack_size(64 * 1024 * 1024)
        .spawn(move || match ExtRuntime::load(&root, &package, &configs) {
            Ok(rt) => {
                if res_tx.send(Ok(())).is_ok() {
                    runtime_loop(
                        ExtSlot {
                            root,
                            package,
                            configs,
                            live: Some(rt),
                        },
                        req_rx,
                        events_tx,
                        active_clients,
                    );
                }
            }
            Err(e) => {
                let _ = res_tx.send(Err(e));
            }
        })
        .map_err(|e| HostError::Other(e.to_string()))?;

    res_rx
        .recv_timeout(load_timeout)
        .map_err(|_| HostError::Other("extension runtime did not start in time".into()))??;
    Ok(req_tx)
}

/// Interpreter slot owned by an extension thread: the runtime is held only
/// while timers need it; stateless extensions get one per action.
struct ExtSlot {
    root: std::path::PathBuf,
    package: String,
    configs: Value,
    live: Option<ExtRuntime>,
}

/// Execute an action on the slot's interpreter, (re)creating it on demand.
/// The interpreter is released right after the run unless the action
/// registered timers, so resident cost between actions is zero. Re-running
/// the entry module is safe because extensions keep their own state on
/// disk (see the module docs).
fn run_action(
    slot: &mut ExtSlot,
    action: &str,
    args: &Value,
) -> Result<Vec<crate::host::HostEvent>, crate::host::HostError> {
    if slot.live.is_none() {
        tracing::info!(package = %slot.package, action, "extension runtime loaded on demand");
        slot.live = Some(ExtRuntime::load(&slot.root, &slot.package, &slot.configs)?);
    }
    let rt = slot.live.as_mut().expect("runtime loaded above");
    let events = rt.execute(action, args)?;
    if !rt.has_timers() {
        slot.live = None;
    }
    Ok(events)
}

fn runtime_loop(
    mut slot: ExtSlot,
    req_rx: mpsc::Receiver<ExtRequest>,
    events_tx: tokio_mpsc::UnboundedSender<ExtEvent>,
    active_clients: Arc<AtomicUsize>,
) {
    loop {
        // Timers only run while a live interpreter is held; without one
        // the thread blocks on recv() with no wakeups at all. With no
        // clients connected the pushed values go unseen, so the wait
        // stretches to `IDLE_TICK_FLOOR` instead of full speed.
        let wait = if let Some(rt) = slot.live.as_mut() {
            for ev in rt.tick_due() {
                ExtManager::forward(&events_tx, ev);
            }
            let granularity = rt
                .tick_granularity()
                .clamp(Duration::from_millis(50), Duration::from_secs(1));
            if active_clients.load(Ordering::Relaxed) == 0 {
                granularity.max(IDLE_TICK_FLOOR)
            } else {
                granularity
            }
        } else {
            Duration::ZERO
        };
        let request = if wait.is_zero() {
            req_rx
                .recv()
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
        } else {
            req_rx.recv_timeout(wait)
        };
        match request {
            Ok(ExtRequest::Execute {
                action,
                args,
                reply,
            }) => {
                // execute() drains setValue/interval events itself; the
                // manager forwards whatever the reply carries
                let _ = reply.send(run_action(&mut slot, &action, &args));
            }
            Ok(ExtRequest::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

/// mtime-based source signature: covers asar edits and directory
/// packages. Sub-second granularity, so an edit inside the same
/// wall-clock second still invalidates the metadata cache.
fn source_signature(path: &Path) -> Option<u64> {
    fn mtime_nanos(p: &Path) -> Option<u64> {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as u64)
    }
    if path.is_dir() {
        let index = mtime_nanos(&path.join("index.js")).unwrap_or(0);
        let manifest = mtime_nanos(&path.join("package.json")).unwrap_or(0);
        Some(index.max(manifest))
    } else {
        mtime_nanos(path)
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
        let path = dirs::cache_dir().map(|d| d.join("pulpit-server").join("extmeta.json"));
        let entries = path
            .as_deref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| serde_json::from_slice::<Vec<CachedMeta>>(&bytes).ok())
            .map(|list| list.into_iter().map(|m| (m.package.clone(), m)).collect())
            .unwrap_or_default();
        MetadataCache {
            path,
            entries,
            dirty: false,
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Pin index.js's mtime inside one wall-clock second, then bump it by
    /// 100 ms (still the same second): a seconds-granularity signature
    /// cannot tell the two apart, so an edited package keeps its stale
    /// cached metadata.
    fn set_index_mtime(dir: &Path, offset: Duration) {
        let file = std::fs::File::options()
            .write(true)
            .open(dir.join("index.js"))
            .expect("open index.js");
        let base = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        file.set_times(
            std::fs::FileTimes::new()
                .set_accessed(base)
                .set_modified(base + offset),
        )
        .expect("set mtime");
    }

    #[test]
    fn signatures_distinguish_edits_within_the_same_second() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("pkg")).unwrap();
        std::fs::write(dir.path().join("pkg/index.js"), "module.exports = 1").unwrap();

        let pkg = dir.path().join("pkg");
        set_index_mtime(&pkg, Duration::ZERO);
        let first = source_signature(&pkg).expect("signature");
        set_index_mtime(&pkg, Duration::from_millis(100));
        let second = source_signature(&pkg).expect("signature");

        assert_ne!(
            first, second,
            "an edit within the same wall-clock second must invalidate the cache"
        );
    }
}
