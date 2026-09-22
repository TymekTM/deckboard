//! Native Voicemeeter remote integration.
//!
//! The original `voicemeeter-control` extension talks to the
//! VoicemeeterRemote DLL through ffi-napi, which cannot run in our JS host.
//! This crate ports its action semantics 1:1 against the DLL directly:
//!
//! - `Strip[i].<param>` / `Bus[i].<param>` reads go through
//!   `VBVMR_GetParameterFloat`, writes through `VBVMR_SetParameters`
//!   text commands (`Strip[5].Gain=3;`), like voicemeeter-connector does.
//! - toggles map `round(current) == 0` to `1`, anything else to `0`.
//! - `vm-restart` sends `Command.Restart=1;`.
//! - `vm-set-output` sends `Bus[0].Device.<type>="<name>";`.
//!
//! The DLL is loaded lazily; if Voicemeeter is not installed every call
//! returns [`VmError::Unavailable`] and buttons degrade to log lines, the
//! same way the original extension failed to load.

use std::ffi::c_void;
use std::time::Duration;

use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum VmError {
    #[error("Voicemeeter remote DLL not found (is Voicemeeter installed?)")]
    Unavailable,
    #[error("DLL symbol missing: {0}")]
    Symbol(&'static str),
    #[error("Voicemeeter call failed: {0}")]
    Call(&'static str),
    #[error("bad payload for {0}: {1}")]
    BadPayload(&'static str, String),
    #[error("not logged in")]
    NotLoggedIn,
}

pub type Result<T> = std::result::Result<T, VmError>;

/// C API signatures from the Voicemeeter Remote API (ANSI string variants).
type LoginFn = extern "system" fn() -> i32;
type LogoutFn = extern "system" fn() -> i32;
type IsParametersDirtyFn = extern "system" fn() -> i32;
type GetParameterFloatFn = extern "system" fn(name: *const u8, value: *mut f32) -> i32;
type SetParametersFn = extern "system" fn(param: *const u8) -> i32;

struct Symbols {
    login: LoginFn,
    logout: LogoutFn,
    is_parameters_dirty: IsParametersDirtyFn,
    get_parameter_float: GetParameterFloatFn,
    set_parameters: SetParametersFn,
    module: *mut c_void,
}

// SAFETY: the DLL handles its own synchronization; every call goes through
// the handle stored behind the connection mutex and the function pointers
// are immutable once resolved.
unsafe impl Send for Symbols {}

unsafe fn load_symbols(path: &std::ffi::CStr) -> Result<Symbols> {
    extern "system" {
        fn LoadLibraryA(name: *const u8) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    }
    // SAFETY: path is a NUL-terminated C string
    let module = unsafe { LoadLibraryA(path.as_ptr() as *const u8) };
    if module.is_null() {
        return Err(VmError::Unavailable);
    }
    let resolve = |name: &'static str| -> Result<*mut c_void> {
        let symbol = std::ffi::CString::new(name).expect("symbol names contain no NUL");
        // SAFETY: symbol is NUL-terminated and module is a live handle
        let sym = unsafe { GetProcAddress(module, symbol.as_ptr() as *const u8) };
        if sym.is_null() {
            Err(VmError::Symbol(name))
        } else {
            Ok(sym)
        }
    };
    // SAFETY: each pointer is validated non-null above and has the
    // documented signature for that export
    Ok(Symbols {
        login: unsafe { std::mem::transmute::<*mut c_void, LoginFn>(resolve("VBVMR_Login")?) },
        logout: unsafe { std::mem::transmute::<*mut c_void, LogoutFn>(resolve("VBVMR_Logout")?) },
        is_parameters_dirty: unsafe {
            std::mem::transmute::<*mut c_void, IsParametersDirtyFn>(resolve(
                "VBVMR_IsParametersDirty",
            )?)
        },
        get_parameter_float: unsafe {
            std::mem::transmute::<*mut c_void, GetParameterFloatFn>(resolve(
                "VBVMR_GetParameterFloat",
            )?)
        },
        set_parameters: unsafe {
            std::mem::transmute::<*mut c_void, SetParametersFn>(resolve("VBVMR_SetParameters")?)
        },
        module,
    })
}

/// Candidate DLL locations, mirroring the install layouts Voicemeeter uses.
pub fn dll_candidates() -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let base =
        std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| r"C:\Program Files (x86)".into());
    // Potato installs into a versioned subfolder first
    for entry in [
        r"\VB\Voicemeeter\VoicemeeterRemote64.dll",
        r"\VB\Voicemeeter\Potato\VoicemeeterRemote64.dll",
        r"\VB\Voicemeeter\Banana\VoicemeeterRemote64.dll",
    ] {
        out.push(std::path::PathBuf::from(&base).join(entry.trim_start_matches('\\')));
    }
    out
}

/// Logged-in handle to the remote API. Connection state lives behind the
/// mutex in [`VoicemeeterState`]; this struct is just resolved symbols.
pub struct Remote {
    symbols: Symbols,
}

impl Remote {
    /// Locate and load VoicemeeterRemote64.dll.
    pub fn open() -> Result<Remote> {
        for path in dll_candidates() {
            if !path.exists() {
                continue;
            }
            let c = std::ffi::CString::new(path.to_string_lossy().as_bytes())
                .map_err(|_| VmError::Unavailable)?;
            // SAFETY: c is NUL-terminated
            let symbols = unsafe { load_symbols(&c) }?;
            return Ok(Remote { symbols });
        }
        Err(VmError::Unavailable)
    }

    pub fn login(&self) -> Result<()> {
        // 0 = logged in; the API also starts Voicemeeter when needed
        if (self.symbols.login)() < 0 {
            return Err(VmError::Call("VBVMR_Login"));
        }
        Ok(())
    }

    pub fn logout(&self) -> Result<()> {
        if (self.symbols.logout)() < 0 {
            return Err(VmError::Call("VBVMR_Logout"));
        }
        Ok(())
    }

    pub fn get_parameter_float(&self, param: &str) -> Result<f32> {
        let name = std::ffi::CString::new(param)
            .map_err(|_| VmError::BadPayload("parameter", param.into()))?;
        // the DLL refreshes its parameter bank only when asked; without
        // this call reads return stale values (and toggles read the old
        // state forever)
        let mut syncs = 0;
        while (self.symbols.is_parameters_dirty)() == 1 && syncs < 20 {
            syncs += 1;
        }
        let mut value = 0f32;
        // SAFETY: name outlives the call, value is a valid out-param
        if (self.symbols.get_parameter_float)(name.as_ptr() as *const u8, &mut value) < 0 {
            return Err(VmError::Call("VBVMR_GetParameterFloat"));
        }
        Ok(value)
    }

    pub fn set_parameters(&self, text: &str) -> Result<()> {
        let c = std::ffi::CString::new(text)
            .map_err(|_| VmError::BadPayload("parameters", text.into()))?;
        // SAFETY: c is NUL-terminated
        if (self.symbols.set_parameters)(c.as_ptr() as *const u8) < 0 {
            return Err(VmError::Call("VBVMR_SetParameters"));
        }
        // the engine applies text commands asynchronously; voicemeeter-
        // connector waited 200 ms so a following read sees the new state
        std::thread::sleep(Duration::from_millis(200));
        Ok(())
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        extern "system" {
            fn FreeLibrary(module: *mut c_void) -> i32;
        }
        if !self.symbols.module.is_null() {
            // SAFETY: module is a live handle we own
            unsafe { FreeLibrary(self.symbols.module) };
        }
    }
}

/// Lazy connection holder for the server backend.
#[derive(Default)]
pub struct VoicemeeterState {
    remote: Option<Remote>,
    logged_in: bool,
}

impl VoicemeeterState {
    pub fn new() -> VoicemeeterState {
        VoicemeeterState::default()
    }

    fn with_remote<T>(&mut self, f: impl FnOnce(&Remote) -> Result<T>) -> Result<T> {
        if self.remote.is_none() {
            self.remote = Some(Remote::open()?);
        }
        let remote = self.remote.as_ref().expect("remote just set");
        if !self.logged_in {
            remote.login()?;
            self.logged_in = true;
        }
        f(remote)
    }

    /// Execute one `vm-*` action with the extension's argument shape
    /// (`{"param": "...", "number": N, "value": "..."}`).
    pub fn execute(&mut self, action: &str, args: &Value) -> Result<()> {
        if action == "vm-restart" {
            return self.with_remote(|r| r.set_parameters("Command.Restart=1;"));
        }
        let kind: &'static str = match action {
            "vm-set-strip" | "vm-toggle-strip" | "vm-increase-strip" | "vm-decrease-strip" => {
                "Strip"
            }
            "vm-set-bus" | "vm-toggle-bus" | "vm-increase-bus" | "vm-decrease-bus" => "Bus",
            "vm-set-output" => return self.set_output(args),
            other => return Err(VmError::BadPayload("action", other.into())),
        };
        let args = args
            .as_object()
            .ok_or_else(|| VmError::BadPayload("args", "expected object".into()))?;
        let number = args
            .get("number")
            .and_then(Value::as_i64)
            .ok_or_else(|| VmError::BadPayload("number", "missing".into()))?;
        let param = args
            .get("param")
            .and_then(Value::as_str)
            .ok_or_else(|| VmError::BadPayload("param", "missing".into()))?
            .to_string();
        let index = format!("{kind}[{number}].{param}");
        // action shape: vm-{op}-{strip|bus}
        let base = action
            .strip_suffix("-strip")
            .or_else(|| action.strip_suffix("-bus"))
            .ok_or_else(|| VmError::BadPayload("action", action.to_string()))?;

        match base {
            "vm-set" => {
                let value = parse_value(args)?;
                self.with_remote(|r| r.set_parameters(&param_text(&index, value as f32)))
            }
            "vm-toggle" => self.with_remote(|r| {
                let current = r.get_parameter_float(&index)?;
                let target = if current.round() == 0.0 { 1.0 } else { 0.0 };
                r.set_parameters(&param_text(&index, target))
            }),
            "vm-increase" | "vm-decrease" => {
                let delta = parse_value(args)?;
                let delta = if base == "vm-decrease" { -delta } else { delta };
                self.with_remote(|r| {
                    let current = r.get_parameter_float(&index)?;
                    r.set_parameters(&param_text(&index, current + delta as f32))
                })
            }
            other => Err(VmError::BadPayload("action", other.into())),
        }
    }

    /// Read one Strip parameter - used by live diagnostics and tests.
    pub fn read_strip(&mut self, number: i64, param: &str) -> Result<f32> {
        let index = format!("Strip[{number}].{param}");
        self.with_remote(|r| r.get_parameter_float(index.as_str()))
    }

    /// `vm-set-output`: device is `"TYPE: name"` (MME/WDM/KS); the switch
    /// lands on bus 0 like the original extension.
    fn set_output(&mut self, args: &Value) -> Result<()> {
        let device = args
            .get("device")
            .and_then(Value::as_str)
            .ok_or_else(|| VmError::BadPayload("device", "missing".into()))?;
        let (kind, name) = device
            .split_once(": ")
            .ok_or_else(|| VmError::BadPayload("device", device.into()))?;
        let index = format!("Bus[0].Device.{}", kind.to_lowercase());
        self.with_remote(|r| r.set_parameters(&string_param_text(&index, name)))
    }
}

/// Values from the editor arrive as JSON strings ("1"), the extension
/// used parseFloat - numbers and numeric strings both pass.
fn parse_value(args: &serde_json::Map<String, Value>) -> Result<f64> {
    match args.get("value") {
        Some(Value::Number(n)) => n
            .as_f64()
            .ok_or_else(|| VmError::BadPayload("value", "not a number".into())),
        Some(Value::String(s)) => s
            .trim()
            .parse::<f64>()
            .map_err(|_| VmError::BadPayload("value", s.clone())),
        _ => Err(VmError::BadPayload("value", "missing".into())),
    }
}

/// `Strip[5].Gain=3;` - the text command form voicemeeter-connector builds.
fn param_text(index: &str, value: f32) -> String {
    format!("{index}={};", format_value(value))
}

/// Values with a fraction keep one decimal, integers stay integers (the JS
/// side concatenated numbers, which never produce trailing zeros).
fn format_value(value: f32) -> String {
    if (value - value.round()).abs() < f32::EPSILON {
        format!("{}", value.round() as i64)
    } else {
        format!("{value}")
    }
}

/// `Bus[0].Device.wdm="Speakers";` - quoted string parameter form.
fn string_param_text(index: &str, value: &str) -> String {
    format!("{index}=\"{value}\";")
}

/// Extension input declarations for the style resolver (same colors and
/// icons as the original voicemeeter-control package declares).
pub fn input_declarations() -> Vec<(
    &'static str,
    Option<&'static str>,
    &'static str,
    &'static str,
)> {
    // (value, icon, fontIcon, color)
    vec![
        ("vm-set-strip", Some("headphones"), "fas", "#171A21"),
        (
            "vm-toggle-strip",
            Some("microphone-slash"),
            "fas",
            "#171A21",
        ),
        ("vm-increase-strip", Some("volume-up"), "fas", "#171A21"),
        ("vm-decrease-strip", Some("volume-down"), "fas", "#171A21"),
        ("vm-set-bus", Some("headphones"), "fas", "#171A21"),
        ("vm-toggle-bus", Some("volume-mute"), "fas", "#171A21"),
        ("vm-increase-bus", Some("volume-up"), "fas", "#171A21"),
        ("vm-decrease-bus", Some("volume-down"), "fas", "#171A21"),
        ("vm-restart", Some("sync"), "fas", "#171A21"),
        ("vm-set-output", Some("headphones"), "fas", "#171A21"),
    ]
}

/// Is this action one of ours?
pub fn is_vm_action(kind: &str) -> bool {
    kind.starts_with("vm-")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn param_text_matches_connector_output() {
        assert_eq!(param_text("Strip[5].Gain", 3.0), "Strip[5].Gain=3;");
        assert_eq!(param_text("Strip[2].A1", 1.0), "Strip[2].A1=1;");
        assert_eq!(param_text("Bus[0].Gain", -10.5), "Bus[0].Gain=-10.5;");
    }

    #[test]
    fn string_params_are_quoted() {
        assert_eq!(
            string_param_text("Bus[0].Device.wdm", "Speakers"),
            "Bus[0].Device.wdm=\"Speakers\";"
        );
    }

    #[test]
    fn payload_errors_come_before_connection() {
        let mut vm = VoicemeeterState::new();
        // missing number -> payload error even when Voicemeeter is absent
        let err = vm
            .execute("vm-toggle-strip", &json!({"param": "Mute"}))
            .unwrap_err();
        assert!(matches!(err, VmError::BadPayload("number", _)));
        let err = vm
            .execute("vm-set-output", &json!({"device": "no-colon"}))
            .unwrap_err();
        assert!(matches!(err, VmError::BadPayload("device", _)));
    }

    #[test]
    fn values_parse_like_the_extension() {
        // shapes exactly as stored in the user's database
        assert_eq!(
            parse_value(json!({"value": "1"}).as_object().unwrap()).unwrap(),
            1.0
        );
        assert_eq!(
            parse_value(json!({"value": "0.5"}).as_object().unwrap()).unwrap(),
            0.5
        );
        assert_eq!(
            parse_value(json!({"value": 2}).as_object().unwrap()).unwrap(),
            2.0
        );
        assert!(parse_value(json!({"value": "loud"}).as_object().unwrap()).is_err());
        assert!(parse_value(json!({}).as_object().unwrap()).is_err());
    }

    #[test]
    fn unknown_actions_are_rejected() {
        let mut vm = VoicemeeterState::new();
        let err = vm.execute("vm-explode", &Value::Null).unwrap_err();
        assert!(matches!(err, VmError::BadPayload("action", _)));
        let err = vm
            .execute(
                "vm-toggle-something",
                &json!({"param": "Mute", "number": 0}),
            )
            .unwrap_err();
        assert!(matches!(err, VmError::BadPayload("action", _)));
    }

    /// The remote API is single-client: parallel live tests in one process
    /// would race their logins and crash the harness (0xc0000005). In the
    /// app the shared `Mutex<VoicemeeterState>` in the backend plays this
    /// role - the tests must not run the FFI concurrently either.
    fn live_lock() -> std::sync::MutexGuard<'static, ()> {
        static LIVE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LIVE_LOCK.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Live check: reads the real Strip[2].A1 routing state. Run explicitly:
    /// `cargo test -p deckboard-vm -- --ignored live_get`
    #[test]
    #[ignore = "reads the live Voicemeeter state"]
    fn live_get_strip2_a1() {
        let _guard = live_lock();
        let mut vm = VoicemeeterState::new();
        let v = vm.read_strip(2, "A1").expect("read Strip[2].A1");
        println!("Strip[2].A1 = {v}");
    }

    /// Live probe of the full toggle round trip on Strip[2].A1: read,
    /// toggle, read, toggle back, read. Run explicitly.
    #[test]
    #[ignore = "flips live audio routing twice"]
    fn live_toggle_probe() {
        let _guard = live_lock();
        let mut vm = VoicemeeterState::new();
        let v0 = vm.read_strip(2, "A1").unwrap();
        vm.execute(
            "vm-toggle-strip",
            &serde_json::json!({"param": "A1", "number": 2}),
        )
        .unwrap();
        let v1 = vm.read_strip(2, "A1").unwrap();
        vm.execute(
            "vm-toggle-strip",
            &serde_json::json!({"param": "A1", "number": 2}),
        )
        .unwrap();
        let v2 = vm.read_strip(2, "A1").unwrap();
        println!("toggle round trip: {v0} -> {v1} -> {v2}");
    }

    /// Live test: touches the real Voicemeeter instance. Run explicitly:
    /// `cargo test -p deckboard-vm -- --ignored`
    #[test]
    #[ignore = "fires Command.Restart on the live audio engine"]
    fn live_restart() {
        let _guard = live_lock();
        let mut vm = VoicemeeterState::new();
        vm.execute("vm-restart", &Value::Null).unwrap();
    }

    #[test]
    fn action_routing() {
        assert!(is_vm_action("vm-toggle-strip"));
        assert!(!is_vm_action("vol"));
    }

    #[test]
    fn dll_candidates_cover_standard_layout() {
        let cands = dll_candidates();
        assert!(cands.iter().any(|p| p
            .to_string_lossy()
            .ends_with("Voicemeeter\\VoicemeeterRemote64.dll")));
    }
}
