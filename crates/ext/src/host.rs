//! Boa-backed JS runtime for one extension package.
//!
//! Native functions are stateless free functions (all needed context is
//! passed through arguments); Rust-visible state (set_value calls, interval
//! registrations) is drained from JS globals after every eval. This avoids
//! holding GC'd JS objects on the Rust side.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use boa_engine::{Context, JsResult, JsValue, NativeFunction, Source};
use serde_json::Value;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum HostError {
    #[error("js error: {0}")]
    Js(String),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, HostError>;

/// Events produced while the extension runs.
#[derive(Debug, Clone)]
pub enum HostEvent {
    /// Extension pushed a custom value (graph/button state) to clients.
    SetValue(Value),
    /// A `setInterval(cb, ms)` was registered; id is stable per runtime.
    IntervalStart(u64, u64),
    /// A `clearInterval(id)`.
    IntervalClear(u64),
    Log(String, String),
}

pub struct ExtRuntime {
    context: Context,
    root: PathBuf,
    pub package: String,
    pub name: String,
    /// action `value` strings this extension handles
    pub actions: Vec<String>,
    /// raw `inputs` declarations (button styles + actions for the mapper)
    pub inputs: Vec<Value>,
    intervals: Vec<(u64, u64, Instant)>, // (id, ms, due)
}

impl ExtRuntime {
    pub fn load(root: &Path, package: &str, configs: &Value) -> Result<ExtRuntime> {
        let mut context = Context::default();
        register_natives(&mut context);
        let prelude = include_str!("prelude.js")
            .replace("__EXT_ROOT__", &escape_js(&root.to_string_lossy()))
            .replace("__EXT_PACKAGE__", &escape_js(package));
        run(&mut context, &prelude)?;

        // load the entry module and normalize to an instance
        let setup = r#"
            (function () {
                try {
                    __require_entry();
                } catch (e) {
                    return { error: String(e && e.message || e) + (e && e.stack ? "\\n" + e.stack : "") };
                }
                var inst = __ext_instance;
                return {
                    name: String(inst.name || __EXT_PACKAGE),
                    inputs: inst.inputs || [],
                };
            })()
        "#.replace("__EXT_PACKAGE", &escape_js(package));
        let info_value = run(&mut context, &setup)?;
        // boa's to_json yields None for undefined
        let info_json = info_value
            .to_json(&mut context)
            .map_err(|e| HostError::Js(e.to_string()))?
            .unwrap_or(Value::Null);
        let info_obj = info_json.as_object().cloned().unwrap_or_default();
        if let Some(err) = info_obj.get("error").and_then(Value::as_str) {
            return Err(HostError::Other(err.to_string()));
        }
        let name = info_obj
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(package)
            .to_string();
        let actions = info_obj
            .get("inputs")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|i| i.get("value").and_then(Value::as_str).map(str::to_string))
            .collect();
        let inputs = info_obj
            .get("inputs")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        // inject user configs from ~/deckboard/settings.json
        let cfg = serde_json::to_string(configs).unwrap_or_else(|_| "{}".into());
        run(
            &mut context,
            &format!("__ext_instance.configs = JSON.parse({});", escape_js(&cfg)),
        )?;

        let mut rt = ExtRuntime {
            context,
            root: root.to_path_buf(),
            package: package.to_string(),
            name,
            actions,
            inputs,
            intervals: Vec::new(),
        };

        // old-style init hook
        run(&mut rt.context, "if (typeof __ext_instance.initExtension === 'function') __ext_instance.initExtension();")?;
        rt.drain(); // collect intervals started by initExtension
        Ok(rt)
    }

    /// Execute an action (`runCommand` default case):
    /// `instance.execute(action, JSON.parse(command))`.
    pub fn execute(&mut self, action: &str, args: &Value) -> Result<Vec<HostEvent>> {
        let args_json = serde_json::to_string(args).unwrap_or_else(|_| "null".into());
        let code = format!(
            "__ext_instance.execute({}, JSON.parse({} || 'null'));",
            escape_js(action),
            escape_js(&args_json)
        );
        run(&mut self.context, &code)?;
        Ok(self.drain())
    }

    /// Run one registered timer callback by id (host-driven ticking).
    pub fn tick(&mut self, id: u64) -> Vec<HostEvent> {
        let code = format!("__run_timer({id});");
        if run(&mut self.context, &code).is_err() {
            return Vec::new();
        }
        self.drain()
    }

    /// Collect pending events (set values, interval registrations) and
    /// update the host-side tick schedule.
    pub fn drain(&mut self) -> Vec<HostEvent> {
        let mut events = Vec::new();
        if let Ok(v) = run(&mut self.context, "JSON.stringify(__flush_set_values())") {
            if let Ok(list) = serde_json::from_str::<Vec<Value>>(
                &v.as_string()
                    .map(|s| s.to_std_string_escaped())
                    .unwrap_or_default(),
            ) {
                for obj in list {
                    events.push(HostEvent::SetValue(obj));
                }
            }
        }
        if let Ok(v) = run(
            &mut self.context,
            "JSON.stringify({n: __new_intervals.splice(0), c: __cleared_intervals.splice(0)})",
        ) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(
                &v.as_string()
                    .map(|s| s.to_std_string_escaped())
                    .unwrap_or_default(),
            ) {
                for iv in json
                    .get("n")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                {
                    let id = iv.get("id").and_then(Value::as_u64).unwrap_or(0);
                    let ms = iv
                        .get("ms")
                        .and_then(Value::as_u64)
                        .unwrap_or(1000)
                        .max(100);
                    self.intervals.retain(|(eid, _, _)| *eid != id);
                    self.intervals
                        .push((id, ms, Instant::now() + Duration::from_millis(ms)));
                    events.push(HostEvent::IntervalStart(id, ms));
                }
                for id in json
                    .get("c")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                {
                    let id = id.as_u64().unwrap_or(0);
                    self.intervals.retain(|(eid, _, _)| *eid != id);
                    events.push(HostEvent::IntervalClear(id));
                }
            }
        }
        events
    }

    /// Fire every interval whose deadline passed. Returns produced events.
    pub fn tick_due(&mut self) -> Vec<HostEvent> {
        let now = Instant::now();
        let due: Vec<u64> = self
            .intervals
            .iter()
            .filter(|(_, _, d)| *d <= now)
            .map(|(id, _, _)| *id)
            .collect();
        let mut events = Vec::new();
        for id in due {
            tracing::debug!(package = %self.package, timer = id, intervals = self.intervals.len(), "timer tick");
            events.extend(self.tick(id));
            if let Some(entry) = self.intervals.iter_mut().find(|(eid, _, _)| *eid == id) {
                let ms = entry.1;
                entry.2 = Instant::now() + Duration::from_millis(ms);
            }
        }
        events
    }

    /// Minimum tick granularity needed by this runtime.
    pub fn tick_granularity(&self) -> Duration {
        let min = self
            .intervals
            .iter()
            .map(|(_, ms, _)| *ms)
            .min()
            .unwrap_or(1000);
        Duration::from_millis(min.clamp(100, 60_000))
    }

    #[allow(dead_code)]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

fn run(context: &mut Context, code: &str) -> Result<JsValue> {
    let value = context
        .eval(Source::from_bytes(code))
        .map_err(|e| HostError::Js(e.to_string()))?;
    // flush any promise microtasks the shims created (fetch etc.)
    let _ = context.run_jobs();
    Ok(value)
}

fn escape_js(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

// ------------------------------------------------------------- native fns

/// Argument extraction helpers (all natives are string-in, string/bool-out).
fn arg_str(args: &[JsValue], idx: usize) -> String {
    args.get(idx)
        .and_then(|v| v.as_string())
        .map(|s| s.to_std_string_escaped())
        .unwrap_or_default()
}

macro_rules! native {
    ($name:ident, |$args:ident, $ctx:ident| $body:expr) => {
        fn $name(_this: &JsValue, $args: &[JsValue], $ctx: &mut Context) -> JsResult<JsValue> {
            $body
        }
    };
}

native!(host_read_file, |args, _ctx| {
    let p = arg_str(args, 0);
    match std::fs::read_to_string(&p) {
        Ok(text) => Ok(JsValue::from(boa_engine::JsString::from(text))),
        Err(_) => {
            // binary reads come back as null; the extensions we target read text
            if Path::new(&p).exists() {
                tracing::debug!(path = %p, "binary file read unsupported, returned null");
            }
            Ok(JsValue::null())
        }
    }
});

native!(host_read_file_base64, |args, _ctx| {
    let p = arg_str(args, 0);
    match std::fs::read(&p) {
        Ok(bytes) => {
            let table = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            let mut b64 = String::new();
            for chunk in bytes.chunks(3) {
                let b = [
                    chunk[0],
                    *chunk.get(1).unwrap_or(&0),
                    *chunk.get(2).unwrap_or(&0),
                ];
                let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
                b64.push(table[(n >> 18) as usize & 63] as char);
                b64.push(table[(n >> 12) as usize & 63] as char);
                b64.push(if chunk.len() > 1 {
                    table[(n >> 6) as usize & 63] as char
                } else {
                    '='
                });
                b64.push(if chunk.len() > 2 {
                    table[n as usize & 63] as char
                } else {
                    '='
                });
            }
            Ok(JsValue::from(boa_engine::JsString::from(b64)))
        }
        Err(_) => Ok(JsValue::null()),
    }
});

native!(host_file_exists, |args, _ctx| {
    Ok(JsValue::from(Path::new(&arg_str(args, 0)).exists()))
});

native!(host_list_dir, |args, ctx| {
    let p = arg_str(args, 0);
    match std::fs::read_dir(&p) {
        Ok(entries) => {
            let names: Vec<Value> = entries
                .filter_map(|e| e.ok())
                .map(|e| Value::String(e.file_name().to_string_lossy().into_owned()))
                .collect();
            JsValue::from_json(&Value::Array(names), ctx)
        }
        Err(_) => Ok(JsValue::null()),
    }
});

native!(host_write_file, |args, _ctx| {
    let p = arg_str(args, 0);
    let content = arg_str(args, 1);
    Ok(JsValue::from(std::fs::write(&p, content).is_ok()))
});

native!(host_shell_exec, |args, ctx| {
    let cmd = arg_str(args, 0);
    let output = if cfg!(windows) {
        std::process::Command::new("cmd")
            .args(["/C", &cmd])
            .output()
    } else {
        std::process::Command::new("sh").args(["-c", &cmd]).output()
    };
    let value = match output {
        Ok(out) => Value::Object(
            [
                (
                    "stdout".into(),
                    Value::String(String::from_utf8_lossy(&out.stdout).into_owned()),
                ),
                (
                    "stderr".into(),
                    Value::String(String::from_utf8_lossy(&out.stderr).into_owned()),
                ),
                (
                    "error".into(),
                    if out.status.success() {
                        Value::Null
                    } else {
                        Value::String(format!("exit code {}", out.status))
                    },
                ),
            ]
            .into_iter()
            .collect(),
        ),
        Err(e) => Value::Object(
            [
                ("stdout".into(), Value::String(String::new())),
                ("stderr".into(), Value::String(String::new())),
                ("error".into(), Value::String(e.to_string())),
            ]
            .into_iter()
            .collect(),
        ),
    };
    JsValue::from_json(&value, ctx)
});

native!(host_spawn, |args, _ctx| {
    let cmd = arg_str(args, 0);
    let result = if cfg!(windows) {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("cmd")
            .args(["/C", &cmd])
            .creation_flags(0x00000008) // DETACHED_PROCESS
            .spawn()
    } else {
        std::process::Command::new("sh").args(["-c", &cmd]).spawn()
    };
    if let Err(e) = &result {
        tracing::warn!(command = %cmd, err = %e, "extension spawn failed");
    }
    Ok(JsValue::from(result.is_ok()))
});

native!(host_open, |args, _ctx| {
    let target = arg_str(args, 0);
    let result = open::that(&target);
    if let Err(e) = &result {
        tracing::warn!(target = %target, err = %e, "extension open failed");
    }
    Ok(JsValue::from(result.is_ok()))
});

native!(host_http, |args, ctx| {
    let spec = arg_str(args, 0);
    let parsed: Value = serde_json::from_str(&spec).unwrap_or(Value::Null);
    let url = parsed
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let method = parsed
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("GET")
        .to_string();
    let body = parsed
        .get("body")
        .and_then(Value::as_str)
        .map(str::to_string);
    let value = match do_http(&url, &method, body) {
        Ok((status, text)) => Value::Object(
            [
                ("status".into(), Value::from(status)),
                ("body".into(), Value::String(text)),
                ("error".into(), Value::Null),
            ]
            .into_iter()
            .collect(),
        ),
        Err(e) => Value::Object(
            [
                ("status".into(), Value::from(0)),
                ("body".into(), Value::String(String::new())),
                ("error".into(), Value::String(e)),
            ]
            .into_iter()
            .collect(),
        ),
    };
    JsValue::from_json(&value, ctx)
});

fn do_http(
    url: &str,
    method: &str,
    body: Option<String>,
) -> std::result::Result<(u16, String), String> {
    let agent = ureq::Agent::new_with_defaults();
    let method = method.to_uppercase();
    let result = match (method.as_str(), body) {
        ("POST", Some(b)) => agent
            .post(url)
            .content_type("application/json")
            .send(b.as_bytes()),
        ("PUT", Some(b)) => agent
            .put(url)
            .content_type("application/json")
            .send(b.as_bytes()),
        ("POST", None) => agent.post(url).send(&b""[..]),
        ("PUT", None) => agent.put(url).send(&b""[..]),
        ("DELETE", _) => agent.delete(url).call(),
        _ => agent.get(url).call(),
    };
    let resp = result.map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    let text = resp
        .into_body()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    Ok((status, text))
}

native!(host_dialog_error, |args, _ctx| {
    tracing::warn!(title = %arg_str(args, 0), msg = %arg_str(args, 1), "extension dialog");
    Ok(JsValue::undefined())
});

native!(host_log, |args, _ctx| {
    let level = arg_str(args, 0);
    let msg = arg_str(args, 1);
    match level.as_str() {
        "warn" => tracing::warn!(target: "deckboard_ext", "{msg}"),
        "error" => tracing::error!(target: "deckboard_ext", "{msg}"),
        "debug" => tracing::debug!(target: "deckboard_ext", "{msg}"),
        _ => tracing::info!(target: "deckboard_ext", "{msg}"),
    }
    Ok(JsValue::undefined())
});

native!(host_hostname, |_args, _ctx| {
    Ok(JsValue::from(boa_engine::JsString::from(hostname())))
});

/// Aggregate CPU time counters in 100ns units: (idle, kernel, user).
/// `kernel` includes idle, like Win32 GetSystemTimes.
#[cfg(windows)]
fn system_cpu_times() -> (u64, u64, u64) {
    #[repr(C)]
    #[derive(Default)]
    struct Filetime {
        low: u32,
        high: u32,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetSystemTimes(idle: *mut Filetime, kernel: *mut Filetime, user: *mut Filetime) -> i32;
    }
    let (mut idle, mut kernel, mut user) = (
        Filetime::default(),
        Filetime::default(),
        Filetime::default(),
    );
    // SAFETY: three distinct out-parameters of the documented struct size
    let ok = unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) };
    if ok == 0 {
        return (0, 0, 0);
    }
    let raw = |f: &Filetime| ((f.high as u64) << 32) | f.low as u64;
    (raw(&idle), raw(&kernel), raw(&user))
}

#[cfg(not(windows))]
fn system_cpu_times() -> (u64, u64, u64) {
    (0, 0, 0)
}

/// Physical memory in bytes: (total, available).
#[cfg(windows)]
fn system_mem_info() -> (u64, u64) {
    // must mirror MEMORYSTATUSEX exactly: 64 bytes, dwLength = 64, or the
    // API rejects the call
    #[repr(C)]
    struct MemoryStatus {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page: u64,
        avail_page: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_virtual_extended: u64,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalMemoryStatusEx(buf: *mut MemoryStatus) -> i32;
    }
    let mut buf = MemoryStatus {
        length: std::mem::size_of::<MemoryStatus>() as u32,
        memory_load: 0,
        total_phys: 0,
        avail_phys: 0,
        total_page: 0,
        avail_page: 0,
        total_virtual: 0,
        avail_virtual: 0,
        avail_virtual_extended: 0,
    };
    debug_assert_eq!(buf.length, 64);
    // SAFETY: buf is initialized with the expected dwLength
    let ok = unsafe { GlobalMemoryStatusEx(&mut buf) };
    if ok == 0 {
        return (0, 0);
    }
    (buf.total_phys, buf.avail_phys)
}

#[cfg(not(windows))]
fn system_mem_info() -> (u64, u64) {
    (0, 0)
}

native!(host_cpu_times, |_args, ctx| {
    let (idle, kernel, user) = system_cpu_times();
    JsValue::from_json(
        &serde_json::json!({ "idle": idle, "kernel": kernel, "user": user }),
        ctx,
    )
});

native!(host_cpu_count, |_args, _ctx| {
    let n = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    Ok(JsValue::from(n as u32))
});

native!(host_mem_info, |_args, ctx| {
    let (total, free) = system_mem_info();
    JsValue::from_json(&serde_json::json!({ "total": total, "free": free }), ctx)
});

fn hostname() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "PC".into())
}

native!(host_home_dir, |_args, _ctx| {
    Ok(JsValue::from(boa_engine::JsString::from(
        dirs::home_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
    )))
});

native!(host_tmp_dir, |_args, _ctx| {
    Ok(JsValue::from(boa_engine::JsString::from(
        std::env::temp_dir().to_string_lossy().into_owned(),
    )))
});

native!(host_spawn_capture, |args, ctx| {
    let cmd = arg_str(args, 0);
    let output = if cfg!(windows) {
        std::process::Command::new("cmd")
            .args(["/C", &cmd])
            .output()
    } else {
        std::process::Command::new("sh").args(["-c", &cmd]).output()
    };
    let value = match output {
        Ok(out) => Value::Object(
            [
                (
                    "stdout".into(),
                    Value::String(String::from_utf8_lossy(&out.stdout).into_owned()),
                ),
                (
                    "stderr".into(),
                    Value::String(String::from_utf8_lossy(&out.stderr).into_owned()),
                ),
                ("code".into(), Value::from(out.status.code().unwrap_or(0))),
                ("error".into(), Value::Null),
            ]
            .into_iter()
            .collect(),
        ),
        Err(e) => Value::Object(
            [
                ("stdout".into(), Value::String(String::new())),
                ("stderr".into(), Value::String(String::new())),
                ("code".into(), Value::from(-1)),
                ("error".into(), Value::String(e.to_string())),
            ]
            .into_iter()
            .collect(),
        ),
    };
    JsValue::from_json(&value, ctx)
});

fn register_natives(context: &mut Context) {
    let fns: Vec<(&str, NativeFunction)> = vec![
        (
            "__host_read_file",
            NativeFunction::from_fn_ptr(host_read_file),
        ),
        (
            "__host_read_file_base64",
            NativeFunction::from_fn_ptr(host_read_file_base64),
        ),
        (
            "__host_file_exists",
            NativeFunction::from_fn_ptr(host_file_exists),
        ),
        (
            "__host_list_dir",
            NativeFunction::from_fn_ptr(host_list_dir),
        ),
        (
            "__host_write_file",
            NativeFunction::from_fn_ptr(host_write_file),
        ),
        (
            "__host_shell_exec",
            NativeFunction::from_fn_ptr(host_shell_exec),
        ),
        ("__host_spawn", NativeFunction::from_fn_ptr(host_spawn)),
        (
            "__host_spawn_capture",
            NativeFunction::from_fn_ptr(host_spawn_capture),
        ),
        ("__host_open", NativeFunction::from_fn_ptr(host_open)),
        ("__host_http", NativeFunction::from_fn_ptr(host_http)),
        (
            "__host_dialog_error",
            NativeFunction::from_fn_ptr(host_dialog_error),
        ),
        ("__host_log", NativeFunction::from_fn_ptr(host_log)),
        (
            "__host_hostname",
            NativeFunction::from_fn_ptr(host_hostname),
        ),
        (
            "__host_home_dir",
            NativeFunction::from_fn_ptr(host_home_dir),
        ),
        ("__host_tmp_dir", NativeFunction::from_fn_ptr(host_tmp_dir)),
        (
            "__host_cpu_times",
            NativeFunction::from_fn_ptr(host_cpu_times),
        ),
        (
            "__host_cpu_count",
            NativeFunction::from_fn_ptr(host_cpu_count),
        ),
        (
            "__host_mem_info",
            NativeFunction::from_fn_ptr(host_mem_info),
        ),
    ];
    for (name, f) in fns {
        let _ = context.register_global_callable(boa_engine::JsString::from(name), 1, f);
    }
}
