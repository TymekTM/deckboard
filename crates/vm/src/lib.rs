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

/// Voicemeeter gain fader range in dB, the same span the Voicemeeter UI
/// sliders cover for Strip and Bus Gain.
pub const GAIN_MIN: f32 = -60.0;
pub const GAIN_MAX: f32 = 12.0;

/// Slider position 0..1 -> gain in dB across the fader range.
pub fn slider_gain(value: f64) -> f32 {
    let v = value.clamp(0.0, 1.0) as f32;
    (GAIN_MIN + (GAIN_MAX - GAIN_MIN) * v).clamp(GAIN_MIN, GAIN_MAX)
}

/// Inverse of [`slider_gain`]: gain in dB -> slider position 0..1.
pub fn gain_to_slider(gain: f32) -> f64 {
    let g = gain.clamp(GAIN_MIN, GAIN_MAX);
    ((g - GAIN_MIN) / (GAIN_MAX - GAIN_MIN)) as f64
}

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
        self.set_parameters_nowait(text)?;
        // the engine applies text commands asynchronously; voicemeeter-
        // connector waited 200 ms so a following read sees the new state
        std::thread::sleep(Duration::from_millis(200));
        Ok(())
    }

    /// Fire one text command and return immediately, for write-only paths
    /// like the gain slider that stream many updates and never read back.
    pub fn set_parameters_nowait(&self, text: &str) -> Result<()> {
        let c = std::ffi::CString::new(text)
            .map_err(|_| VmError::BadPayload("parameters", text.into()))?;
        // SAFETY: c is NUL-terminated
        if (self.symbols.set_parameters)(c.as_ptr() as *const u8) < 0 {
            return Err(VmError::Call("VBVMR_SetParameters"));
        }
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
            "vm-set-strip" | "vm-toggle-strip" | "vm-increase-strip" | "vm-decrease-strip"
            | "vm-slider-strip" => "Strip",
            "vm-set-bus" | "vm-toggle-bus" | "vm-increase-bus" | "vm-decrease-bus"
            | "vm-slider-bus" => "Bus",
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
        check_target(number, &param)?;
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
            // slider position 0..1 (injected by the backend dispatcher)
            // mapped straight onto the gain fader range; streamed without
            // the settle wait so dragging stays realtime
            "vm-slider" => {
                let value = parse_value(args)?;
                let gain = slider_gain(value);
                self.with_remote(|r| r.set_parameters_nowait(&param_text(&index, gain)))
            }
            other => Err(VmError::BadPayload("action", other.into())),
        }
    }

    /// Read one Strip parameter - used by live diagnostics and tests.
    pub fn read_strip(&mut self, number: i64, param: &str) -> Result<f32> {
        check_target(number, param)?;
        let index = format!("Strip[{number}].{param}");
        self.with_remote(|r| r.get_parameter_float(index.as_str()))
    }

    /// Read one Bus parameter - used by live diagnostics and tests.
    pub fn read_bus(&mut self, number: i64, param: &str) -> Result<f32> {
        check_target(number, param)?;
        let index = format!("Bus[{number}].{param}");
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
        let kind = kind.to_lowercase();
        // both halves land inside one text command; validate before it
        // is ever built
        if !is_plain_token(&kind) || !is_quotable_text(name) {
            return Err(VmError::BadPayload("device", device.into()));
        }
        let index = format!("Bus[0].Device.{kind}");
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

/// Strip/Bus count bound for arguments. Voicemeeter's largest layout
/// (Potato) has 8 of each; 32 leaves margin while keeping indices sane.
const MAX_INDEX: i64 = 32;

/// Parameter-name token Voicemeeter understands: ASCII letters, digits,
/// dot, underscore. Anything else - in particular `;`, `=`, quotes and
/// newlines - could splice extra commands into the `;`-separated command
/// language of `VBVMR_SetParameters`.
fn is_plain_token(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_')
}

/// Text that is safe to place inside a quoted command: no quote (breaks
/// out), no semicolon (splices), no control characters.
fn is_quotable_text(s: &str) -> bool {
    !s.is_empty()
        && !s
            .bytes()
            .any(|b| b == b'"' || b == b';' || b.is_ascii_control())
}

/// Validate the `(kind, number, param)` triple used to address a bus or
/// strip parameter.
fn check_target(number: i64, param: &str) -> Result<()> {
    if !(0..=MAX_INDEX).contains(&number) {
        return Err(VmError::BadPayload(
            "number",
            format!("{number} outside 0..={MAX_INDEX}"),
        ));
    }
    if !is_plain_token(param) {
        return Err(VmError::BadPayload("param", param.into()));
    }
    Ok(())
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
        ("vm-slider-strip", Some("sliders-h"), "fas", "#171A21"),
        ("vm-slider-bus", Some("sliders-h"), "fas", "#171A21"),
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

    #[test]
    fn params_that_could_splice_commands_are_rejected() {
        // VBVMR_SetParameters speaks a `;`-separated command language: a
        // param carrying `;`, `=` or quotes could splice extra commands.
        // The toggle path is used so the pre-fix run only ever READS with
        // the poisoned name (inert on a live Voicemeeter).
        let mut vm = VoicemeeterState::new();
        for evil in [
            "Mute=0;Strip[0].kilo",
            "Mute;",
            "Mu\"te",
            "Mu\nte",
            "",
            "Mu te",
        ] {
            let err = vm
                .execute("vm-toggle-strip", &json!({ "param": evil, "number": 1 }))
                .unwrap_err();
            assert!(
                matches!(err, VmError::BadPayload("param", _)),
                "param {evil:?} must be rejected as a payload error, got {err:?}"
            );
        }
    }

    #[test]
    fn out_of_range_strip_and_bus_numbers_are_rejected() {
        let mut vm = VoicemeeterState::new();
        for bad in [-1, 999] {
            let err = vm
                .execute(
                    "vm-toggle-strip",
                    &json!({ "param": "Mute", "number": bad }),
                )
                .unwrap_err();
            assert!(
                matches!(err, VmError::BadPayload("number", _)),
                "number {bad} must be rejected as a payload error, got {err:?}"
            );
        }
    }

    #[test]
    fn set_output_rejects_quote_and_semicolon_breakout() {
        // the device name lands inside a quoted text command: a quote can
        // break out and splice further commands. The fixture's spliced
        // text is deliberately inert (`=;` parses as nothing).
        let mut vm = VoicemeeterState::new();
        for evil in ["WDM: x\";=;", "WD;M: y", "WDM: a\nb", ""] {
            let err = vm
                .execute("vm-set-output", &json!({ "device": evil }))
                .unwrap_err();
            assert!(
                matches!(err, VmError::BadPayload("device", _)),
                "device {evil:?} must be rejected as a payload error, got {err:?}"
            );
        }
    }

    #[test]
    fn plain_tokens_and_quotable_text_are_strict() {
        assert!(is_plain_token("Gain"));
        assert!(is_plain_token("A1"));
        assert!(is_plain_token("mode.center"));
        assert!(!is_plain_token(""));
        assert!(!is_plain_token("Gain;"));
        assert!(!is_plain_token("Gain=1"));
        assert!(!is_plain_token("Ga\"in"));
        assert!(!is_plain_token("Ga\nin"));
        assert!(!is_plain_token("Ga in"));

        assert!(is_quotable_text("Speakers (Realtek Audio)"));
        assert!(!is_quotable_text("x\";=;"));
        assert!(!is_quotable_text("a;b"));
        assert!(!is_quotable_text("a\nb"));
        assert!(!is_quotable_text(""));
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
    /// `cargo test -p pulpit-vm -- --ignored live_get`
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
    /// `cargo test -p pulpit-vm -- --ignored`
    #[test]
    #[ignore = "fires Command.Restart on the live audio engine"]
    fn live_restart() {
        let _guard = live_lock();
        let mut vm = VoicemeeterState::new();
        vm.execute("vm-restart", &Value::Null).unwrap();
    }

    /// Live round trip of the bus gain slider on A3 (Bus[2]): read the
    /// current gain, move the slider to the middle (-24 dB), verify, then
    /// restore. Run explicitly:
    /// `cargo test -p pulpit-vm -- --ignored --nocapture live_slider_bus_a3`
    #[test]
    #[ignore = "moves the live A3 bus fader twice"]
    fn live_slider_bus_a3() {
        let _guard = live_lock();
        let mut vm = VoicemeeterState::new();
        // the slider path fires without the settle wait, so a verifying
        // read must give the engine a moment to apply the command
        let settle = || std::thread::sleep(Duration::from_millis(250));
        let before = vm.read_bus(2, "Gain").expect("read Bus[2].Gain");
        vm.execute(
            "vm-slider-bus",
            &json!({"param": "Gain", "number": 2, "value": 0.5}),
        )
        .unwrap();
        settle();
        let mid = vm.read_bus(2, "Gain").expect("read Bus[2].Gain");
        assert!((mid - (-24.0)).abs() < 0.6, "expected -24 dB, got {mid}");
        vm.execute(
            "vm-slider-bus",
            &json!({"param": "Gain", "number": 2, "value": gain_to_slider(before)}),
        )
        .unwrap();
        settle();
        let after = vm.read_bus(2, "Gain").expect("read Bus[2].Gain");
        println!("A3 gain: {before} -> {mid} -> {after}");
        assert!(
            (after - before).abs() < 1.0,
            "restore drifted: {before} vs {after}"
        );
    }

    #[test]
    fn action_routing() {
        assert!(is_vm_action("vm-toggle-strip"));
        assert!(is_vm_action("vm-slider-bus"));
        assert!(!is_vm_action("vol"));
    }

    #[test]
    fn slider_gain_maps_fader_range() {
        assert_eq!(slider_gain(0.0), -60.0);
        assert_eq!(slider_gain(1.0), 12.0);
        assert_eq!(slider_gain(0.5), -24.0);
        // out-of-range positions clamp instead of overshooting the fader
        assert_eq!(slider_gain(1.7), 12.0);
        assert_eq!(slider_gain(-0.3), -60.0);
        // round trip: gain back to a slider position lands on the same gain
        for g in [-60.0f32, -24.0, 0.0, 12.0] {
            assert!((slider_gain(gain_to_slider(g)) - g).abs() < 1e-4);
        }
    }

    #[test]
    fn slider_actions_reject_missing_value_before_connecting() {
        let mut vm = VoicemeeterState::new();
        // args straight from the tile command JSON: no slider value yet
        let err = vm
            .execute("vm-slider-bus", &json!({"param": "Gain", "number": 2}))
            .unwrap_err();
        assert!(matches!(err, VmError::BadPayload("value", _)));
    }

    #[test]
    fn dll_candidates_cover_standard_layout() {
        let cands = dll_candidates();
        assert!(cands.iter().any(|p| p
            .to_string_lossy()
            .ends_with("Voicemeeter\\VoicemeeterRemote64.dll")));
    }
}
