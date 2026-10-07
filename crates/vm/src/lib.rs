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

/// Voicemeeter edition identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum VoicemeeterType {
    Basic,
    Banana,
    Potato,
    Unknown(i32),
}

impl VoicemeeterType {
    pub fn from_raw(code: i32) -> Self {
        match code {
            1 | 4 => VoicemeeterType::Basic,
            2 | 5 => VoicemeeterType::Banana,
            3 | 6 => VoicemeeterType::Potato,
            other => VoicemeeterType::Unknown(other),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            VoicemeeterType::Basic => "Voicemeeter",
            VoicemeeterType::Banana => "Voicemeeter Banana",
            VoicemeeterType::Potato => "Voicemeeter Potato",
            VoicemeeterType::Unknown(_) => "Nieznany",
        }
    }

    pub fn strip_count(&self) -> usize {
        match self {
            VoicemeeterType::Basic => 3,
            VoicemeeterType::Banana => 5,
            VoicemeeterType::Potato => 8,
            VoicemeeterType::Unknown(_) => 8,
        }
    }

    pub fn bus_count(&self) -> usize {
        match self {
            VoicemeeterType::Basic => 2,
            VoicemeeterType::Banana => 5,
            VoicemeeterType::Potato => 8,
            VoicemeeterType::Unknown(_) => 8,
        }
    }
}

/// Unpack 32-bit Voicemeeter version into `v1.v2.v3.v4`.
pub fn format_version(ver: i32) -> String {
    let v1 = (ver >> 24) & 0xFF;
    let v2 = (ver >> 16) & 0xFF;
    let v3 = (ver >> 8) & 0xFF;
    let v4 = ver & 0xFF;
    format!("{v1}.{v2}.{v3}.{v4}")
}

/// Status of the Voicemeeter integration.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VoicemeeterStatus {
    pub installed: bool,
    pub dll_path: Option<String>,
    pub logged_in: bool,
    pub vm_type: Option<String>,
    pub version: Option<String>,
    pub strip_count: usize,
    pub bus_count: usize,
    pub dll_override: Option<String>,
}

/// Selectable item for catalog dropdowns (`devices: "vm-strip"` / `"vm-bus"`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DeviceItem {
    pub value: i64,
    pub label: String,
}

/// Bus indices map onto Voicemeeter's A/B output channels (A1..A5, then
/// B1..B3); the same `i (A1)` text the static catalog lists use.
const DEFAULT_BUS_LABELS: [&str; 8] = [
    "0 (A1)", "1 (A2)", "2 (A3)", "3 (A4)", "4 (A5)", "5 (B1)", "6 (B2)", "7 (B3)",
];

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
type GetVoicemeeterTypeFn = extern "system" fn(p_type: *mut i32) -> i32;
type GetVoicemeeterVersionFn = extern "system" fn(p_version: *mut i32) -> i32;
type GetParameterStringAFn = extern "system" fn(param: *const u8, value: *mut u8) -> i32;
type RunVoicemeeterFn = extern "system" fn(v_type: i32) -> i32;

struct Symbols {
    login: LoginFn,
    logout: LogoutFn,
    is_parameters_dirty: IsParametersDirtyFn,
    get_parameter_float: GetParameterFloatFn,
    set_parameters: SetParametersFn,
    get_voicemeeter_type: Option<GetVoicemeeterTypeFn>,
    get_voicemeeter_version: Option<GetVoicemeeterVersionFn>,
    get_parameter_string: Option<GetParameterStringAFn>,
    run_voicemeeter: Option<RunVoicemeeterFn>,
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
    let resolve_opt = |name: &'static str| -> Option<*mut c_void> {
        let symbol = std::ffi::CString::new(name).ok()?;
        let sym = unsafe { GetProcAddress(module, symbol.as_ptr() as *const u8) };
        if sym.is_null() {
            None
        } else {
            Some(sym)
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
        get_voicemeeter_type: unsafe {
            resolve_opt("VBVMR_GetVoicemeeterType")
                .map(|s| std::mem::transmute::<*mut c_void, GetVoicemeeterTypeFn>(s))
        },
        get_voicemeeter_version: unsafe {
            resolve_opt("VBVMR_GetVoicemeeterVersion")
                .map(|s| std::mem::transmute::<*mut c_void, GetVoicemeeterVersionFn>(s))
        },
        get_parameter_string: unsafe {
            resolve_opt("VBVMR_GetParameterStringA")
                .map(|s| std::mem::transmute::<*mut c_void, GetParameterStringAFn>(s))
        },
        run_voicemeeter: unsafe {
            resolve_opt("VBVMR_RunVoicemeeter")
                .map(|s| std::mem::transmute::<*mut c_void, RunVoicemeeterFn>(s))
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

/// Candidate DLL locations consulting any user override path first.
pub fn dll_candidates_with_override(
    custom_path: Option<&std::path::Path>,
) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Some(custom) = custom_path {
        if !custom.as_os_str().is_empty() {
            out.push(custom.to_path_buf());
        }
    }
    out.extend(dll_candidates());
    out
}

/// Logged-in handle to the remote API. Connection state lives behind the
/// mutex in [`VoicemeeterState`]; this struct is just resolved symbols.
pub struct Remote {
    symbols: Symbols,
    path: std::path::PathBuf,
}

impl Remote {
    /// Locate and load VoicemeeterRemote64.dll.
    pub fn open() -> Result<Remote> {
        Self::open_with_override(None)
    }

    /// Locate and load VoicemeeterRemote64.dll consulting custom override first.
    pub fn open_with_override(custom_path: Option<&std::path::Path>) -> Result<Remote> {
        for path in dll_candidates_with_override(custom_path) {
            if !path.exists() {
                continue;
            }
            let c = std::ffi::CString::new(path.to_string_lossy().as_bytes())
                .map_err(|_| VmError::Unavailable)?;
            // SAFETY: c is NUL-terminated
            let symbols = unsafe { load_symbols(&c) }?;
            return Ok(Remote { symbols, path });
        }
        Err(VmError::Unavailable)
    }

    pub fn dll_path(&self) -> &std::path::Path {
        &self.path
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

    pub fn get_voicemeeter_type(&self) -> Result<VoicemeeterType> {
        let Some(func) = self.symbols.get_voicemeeter_type else {
            return Err(VmError::Symbol("VBVMR_GetVoicemeeterType"));
        };
        let mut raw = 0i32;
        if (func)(&mut raw) < 0 {
            return Err(VmError::Call("VBVMR_GetVoicemeeterType"));
        }
        Ok(VoicemeeterType::from_raw(raw))
    }

    pub fn get_voicemeeter_version(&self) -> Result<String> {
        let Some(func) = self.symbols.get_voicemeeter_version else {
            return Err(VmError::Symbol("VBVMR_GetVoicemeeterVersion"));
        };
        let mut raw = 0i32;
        if (func)(&mut raw) < 0 {
            return Err(VmError::Call("VBVMR_GetVoicemeeterVersion"));
        }
        Ok(format_version(raw))
    }

    pub fn get_parameter_string(&self, param: &str) -> Result<String> {
        let Some(func) = self.symbols.get_parameter_string else {
            return Err(VmError::Symbol("VBVMR_GetParameterStringA"));
        };
        let c_param = std::ffi::CString::new(param)
            .map_err(|_| VmError::BadPayload("parameter", param.into()))?;
        let mut buffer = [0u8; 512];
        if (func)(c_param.as_ptr() as *const u8, buffer.as_mut_ptr()) < 0 {
            return Err(VmError::Call("VBVMR_GetParameterStringA"));
        }
        let nul_pos = buffer.iter().position(|&b| b == 0).unwrap_or(buffer.len());
        let s = String::from_utf8_lossy(&buffer[..nul_pos]).trim().to_string();
        Ok(s)
    }

    pub fn run_voicemeeter(&self, v_type: i32) -> Result<()> {
        let Some(func) = self.symbols.run_voicemeeter else {
            return Err(VmError::Symbol("VBVMR_RunVoicemeeter"));
        };
        if (func)(v_type) < 0 {
            return Err(VmError::Call("VBVMR_RunVoicemeeter"));
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
pub struct VoicemeeterState {
    remote: Option<Remote>,
    logged_in: bool,
    override_path: Option<std::path::PathBuf>,
}

impl Default for VoicemeeterState {
    fn default() -> Self {
        Self::new()
    }
}

impl VoicemeeterState {
    pub fn new() -> VoicemeeterState {
        VoicemeeterState {
            remote: None,
            logged_in: false,
            override_path: None,
        }
    }

    pub fn with_override(override_path: Option<std::path::PathBuf>) -> VoicemeeterState {
        VoicemeeterState {
            remote: None,
            logged_in: false,
            override_path,
        }
    }

    pub fn set_override_path(&mut self, path: Option<std::path::PathBuf>) {
        if self.override_path != path {
            self.override_path = path;
            self.drop_session();
        }
    }

    pub fn get_override_path(&self) -> Option<&std::path::Path> {
        self.override_path.as_deref()
    }

    pub fn drop_session(&mut self) {
        if let Some(remote) = &self.remote {
            if self.logged_in {
                let _ = remote.logout();
            }
        }
        self.logged_in = false;
        self.remote = None;
    }

    pub fn reconnect(&mut self) -> Result<()> {
        self.drop_session();
        self.ensure_connected()?;
        Ok(())
    }

    fn ensure_connected(&mut self) -> Result<&Remote> {
        if self.remote.is_none() {
            self.remote = Some(Remote::open_with_override(self.override_path.as_deref())?);
        }
        let remote = self.remote.as_ref().expect("remote just set");
        if !self.logged_in {
            remote.login()?;
            self.logged_in = true;
        }
        Ok(self.remote.as_ref().unwrap())
    }

    fn with_remote<T>(&mut self, f: impl FnOnce(&Remote) -> Result<T>) -> Result<T> {
        self.ensure_connected()?;
        let remote = self.remote.as_ref().expect("remote connected");
        match f(remote) {
            Ok(v) => Ok(v),
            Err(e) => {
                // Voicemeeter may have exited or restarted since the
                // login: drop the cached session so the NEXT call starts
                // with a fresh open + login instead of replaying a stale
                // one forever.
                self.logged_in = false;
                self.remote = None;
                Err(e)
            }
        }
    }

    /// Query current status for desktop settings.
    pub fn status(&mut self) -> VoicemeeterStatus {
        let override_str = self
            .override_path
            .as_ref()
            .map(|p| p.to_string_lossy().to_string());
        let existing_path = dll_candidates_with_override(self.override_path.as_deref())
            .into_iter()
            .find(|p| p.exists())
            .map(|p| p.to_string_lossy().to_string());

        let installed = existing_path.is_some();
        if !installed {
            return VoicemeeterStatus {
                installed: false,
                dll_path: None,
                logged_in: false,
                vm_type: None,
                version: None,
                strip_count: 0,
                bus_count: 0,
                dll_override: override_str,
            };
        }

        let details = self.with_remote(|r| {
            let vm_type = r.get_voicemeeter_type().ok();
            let version = r.get_voicemeeter_version().ok();
            let strip_count = vm_type.as_ref().map(|t| t.strip_count()).unwrap_or(8);
            let bus_count = vm_type.as_ref().map(|t| t.bus_count()).unwrap_or(8);
            let type_str = vm_type.map(|t| t.as_str().to_string());
            Ok((type_str, version, strip_count, bus_count))
        });

        match details {
            Ok((vm_type, version, strip_count, bus_count)) => VoicemeeterStatus {
                installed: true,
                dll_path: existing_path,
                logged_in: self.logged_in,
                vm_type,
                version,
                strip_count,
                bus_count,
                dll_override: override_str,
            },
            Err(_) => VoicemeeterStatus {
                installed: true,
                dll_path: existing_path,
                logged_in: false,
                vm_type: None,
                version: None,
                strip_count: 0,
                bus_count: 0,
                dll_override: override_str,
            },
        }
    }

    /// Read strip and bus labels for the catalog dropdowns
    /// (`devices: "vm-strip"` / `"vm-bus"`). Only an already-live session
    /// is read: VBVMR_Login starts Voicemeeter, and merely opening the
    /// tile editor must not launch the app. Without a session the lists
    /// come back empty and the editor falls back to the static catalog
    /// indices.
    pub fn devices(&mut self) -> (Vec<DeviceItem>, Vec<DeviceItem>) {
        if self.remote.is_none() || !self.logged_in {
            return (Vec::new(), Vec::new());
        }
        let res = self.with_remote(|r| {
            let vm_type = r.get_voicemeeter_type()?;
            let strip_max = vm_type.strip_count();
            let bus_max = vm_type.bus_count();

            let mut strips = Vec::with_capacity(strip_max);
            for i in 0..strip_max as i64 {
                let label = r
                    .get_parameter_string(&format!("Strip[{i}].Label"))
                    .unwrap_or_default();
                let display = if label.is_empty() {
                    i.to_string()
                } else {
                    format!("{i} ({label})")
                };
                strips.push(DeviceItem {
                    value: i,
                    label: display,
                });
            }

            let mut buses = Vec::with_capacity(bus_max);
            for i in 0..bus_max as i64 {
                // bus labels default to the A/B output names, so an empty
                // custom label still renders `0 (A1)` style text
                let custom_label = r
                    .get_parameter_string(&format!("Bus[{i}].Label"))
                    .unwrap_or_default();
                let fallback = DEFAULT_BUS_LABELS.get(i as usize).copied().unwrap_or("");
                let display = if custom_label.is_empty() {
                    fallback.to_string()
                } else {
                    format!("{i} ({custom_label})")
                };
                buses.push(DeviceItem {
                    value: i,
                    label: display,
                });
            }

            Ok((strips, buses))
        });

        // a session that died between the check and the read degrades to
        // the static lists the same way an absent Voicemeeter does
        res.unwrap_or((Vec::new(), Vec::new()))
    }

    /// Launch Voicemeeter (`VBVMR_RunVoicemeeter`).
    pub fn run_voicemeeter(&mut self, v_type: Option<i32>) -> Result<()> {
        let code = v_type.unwrap_or(2); // Banana by default
        self.with_remote(|r| r.run_voicemeeter(code))
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
        if !is_plain_token(&kind) || !is_quotable_text(name) {
            return Err(VmError::BadPayload("device", device.into()));
        }
        let index = format!("Bus[0].Device.{kind}");
        self.with_remote(|r| r.set_parameters(&string_param_text(&index, name)))
    }
}

/// Read optional DLL path override from `settings.json`.
pub fn load_dll_override(settings: &serde_json::Value) -> Option<std::path::PathBuf> {
    let vm = settings.get("voicemeeter")?;
    let path_val = vm.get("dllPath").or_else(|| vm.get("dll_path"))?;
    let str_val = if let Some(s) = path_val.as_str() {
        s
    } else {
        path_val.get("value").and_then(serde_json::Value::as_str)?
    };
    if str_val.trim().is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(str_val.trim()))
    }
}

/// Persist optional DLL path override into `settings.json`, preserving all foreign keys.
pub fn save_dll_override(path: &std::path::Path, dll_path: Option<&str>) -> std::io::Result<()> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "{}".into(),
        Err(e) => return Err(e),
    };
    let mut settings: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("settings.json exists but is not valid JSON: {e}"),
            ))
        }
    };
    let obj = settings.as_object_mut().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "settings.json is not an object",
        )
    })?;
    let vm = obj
        .entry("voicemeeter")
        .or_insert_with(|| serde_json::Value::Object(Default::default()));
    let vm_obj = vm.as_object_mut().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "voicemeeter entry is not an object",
        )
    })?;

    match dll_path.filter(|s| !s.trim().is_empty()) {
        Some(override_str) => {
            let entry = vm_obj.entry("dllPath").or_insert_with(|| {
                serde_json::json!({
                    "descriptions": "VoicemeeterRemote DLL path override",
                    "name": "dllPath",
                    "type": "text",
                    "value": "",
                })
            });
            if let Some(f) = entry.as_object_mut() {
                f.insert("value".into(), serde_json::json!(override_str.trim()));
            }
        }
        None => {
            vm_obj.remove("dllPath");
            vm_obj.remove("dll_path");
        }
    }

    let json = serde_json::to_string_pretty(&settings)
        .map_err(|e| std::io::Error::other(format!("settings serialization failed: {e}")))?;
    pulpit_db::write_atomic(path, json.as_bytes())
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
    fn type_counts_mapping() {
        let basic = VoicemeeterType::from_raw(1);
        assert_eq!(basic, VoicemeeterType::Basic);
        assert_eq!(basic.strip_count(), 3);
        assert_eq!(basic.bus_count(), 2);

        let basic_64 = VoicemeeterType::from_raw(4);
        assert_eq!(basic_64, VoicemeeterType::Basic);
        assert_eq!(basic_64.strip_count(), 3);
        assert_eq!(basic_64.bus_count(), 2);

        let banana = VoicemeeterType::from_raw(2);
        assert_eq!(banana, VoicemeeterType::Banana);
        assert_eq!(banana.strip_count(), 5);
        assert_eq!(banana.bus_count(), 5);

        let banana_64 = VoicemeeterType::from_raw(5);
        assert_eq!(banana_64, VoicemeeterType::Banana);
        assert_eq!(banana_64.strip_count(), 5);
        assert_eq!(banana_64.bus_count(), 5);

        let potato = VoicemeeterType::from_raw(3);
        assert_eq!(potato, VoicemeeterType::Potato);
        assert_eq!(potato.strip_count(), 8);
        assert_eq!(potato.bus_count(), 8);

        let potato_64 = VoicemeeterType::from_raw(6);
        assert_eq!(potato_64, VoicemeeterType::Potato);
        assert_eq!(potato_64.strip_count(), 8);
        assert_eq!(potato_64.bus_count(), 8);

        let unknown = VoicemeeterType::from_raw(99);
        assert_eq!(unknown, VoicemeeterType::Unknown(99));
        assert_eq!(unknown.strip_count(), 8);
        assert_eq!(unknown.bus_count(), 8);
    }

    #[test]
    fn format_version_unpacks_bytes() {
        let v = (2 << 24) | (6 << 8) | 8;
        assert_eq!(format_version(v), "2.0.6.8");
    }

    #[test]
    fn settings_read_merge_write_preserves_foreign_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{
                "foreign-package": { "key": { "value": "secret" } },
                "discord-deckboard": { "discordClientId": { "value": "123" } }
            }"#,
        )
        .unwrap();

        // 1. Save override
        save_dll_override(&path, Some(r"C:\Custom\VoicemeeterRemote64.dll")).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        let val: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            val["foreign-package"]["key"]["value"],
            "secret",
            "foreign key must be preserved"
        );
        assert_eq!(
            val["discord-deckboard"]["discordClientId"]["value"],
            "123",
            "discord key must be preserved"
        );
        assert_eq!(
            load_dll_override(&val),
            Some(std::path::PathBuf::from(r"C:\Custom\VoicemeeterRemote64.dll"))
        );

        // 2. Clear override (None)
        save_dll_override(&path, None).unwrap();
        let raw2 = std::fs::read_to_string(&path).unwrap();
        let val2: serde_json::Value = serde_json::from_str(&raw2).unwrap();
        assert_eq!(val2["foreign-package"]["key"]["value"], "secret");
        assert_eq!(load_dll_override(&val2), None);
    }

    /// The remote API is single-client: parallel live tests in one process
    /// would race their logins and crash the harness (0xc0000005). In the
    /// app the shared `Mutex<VoicemeeterState>` in the backend plays this
    /// role - the tests must not run the FFI concurrently either.
    fn live_lock() -> std::sync::MutexGuard<'static, ()> {
        static LIVE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LIVE_LOCK.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Live check of the settings panel data: status plus strip/bus
    /// labels. Run explicitly:
    /// `cargo test -p pulpit-vm -- --ignored live_query_status_and_labels --nocapture`
    #[test]
    #[ignore = "needs live Voicemeeter installation"]
    fn live_query_status_and_labels() {
        let _guard = live_lock();
        let mut vm = VoicemeeterState::new();
        let status = vm.status();
        println!("Voicemeeter status: {status:?}");
        let (strips, buses) = vm.devices();
        println!("Strips: {strips:?}");
        println!("Buses: {buses:?}");
    }

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
    fn failed_calls_drop_the_cached_session() {
        let mut vm = VoicemeeterState::new();
        // Outcome depends on the machine: without a running Voicemeeter
        // the restart errors - and the failed session must then be
        // dropped so the NEXT call re-opens and re-logs-in instead of
        // replaying a stale one forever. With a live Voicemeeter the
        // restart succeeds and there is nothing to assert.
        if vm.execute("vm-restart", &Value::Null).is_err() {
            assert!(
                vm.remote.is_none(),
                "a failed call must drop the cached remote"
            );
            assert!(!vm.logged_in, "a failed call must clear the login flag");
        }
    }

    #[test]
    fn values_parse_like_the_extension() {
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
        assert!(!is_quotable_text(""));
        assert!(!is_quotable_text("Speakers \"extra"));
        assert!(!is_quotable_text("Speakers; extra"));
        assert!(!is_quotable_text("Speakers\nextra"));
    }

    #[test]
    fn slider_gain_maps_fader_range() {
        assert!((slider_gain(0.0) - GAIN_MIN).abs() < f32::EPSILON);
        assert!((slider_gain(1.0) - GAIN_MAX).abs() < f32::EPSILON);
        assert!((slider_gain(-1.0) - GAIN_MIN).abs() < f32::EPSILON);
        assert!((slider_gain(2.0) - GAIN_MAX).abs() < f32::EPSILON);
        let mid_gain = slider_gain(0.5);
        assert!((gain_to_slider(mid_gain) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn slider_actions_reject_missing_value_before_connecting() {
        let mut vm = VoicemeeterState::new();
        let err = vm
            .execute(
                "vm-slider-strip",
                &json!({ "param": "Gain", "number": 0 }),
            )
            .unwrap_err();
        assert!(matches!(err, VmError::BadPayload("value", _)));
    }

    #[test]
    fn dll_candidates_cover_standard_layout() {
        let list = dll_candidates();
        assert!(list.len() >= 3);
        assert!(list
            .iter()
            .any(|p| p.ends_with(r"VB\Voicemeeter\VoicemeeterRemote64.dll")));
    }

    #[test]
    fn action_routing() {
        assert!(is_vm_action("vm-set-strip"));
        assert!(is_vm_action("vm-toggle-bus"));
        assert!(is_vm_action("vm-restart"));
        assert!(!is_vm_action("obs-scene"));
    }

    /// Live check: reads the real Strip[2].A1 routing state. Run explicitly:
    /// `cargo test -p pulpit-vm -- --ignored live_get_strip2_a1`
    #[test]
    #[ignore = "reads the live Voicemeeter state"]
    fn live_get_strip2_a1() {
        let _guard = live_lock();
        let mut vm = VoicemeeterState::new();
        let val = vm.read_strip(2, "A1").expect("strip read failed");
        assert!(val == 0.0 || val == 1.0, "unexpected A1 state: {val}");
    }

    /// Live probe of the full toggle round trip on Strip[2].A1: read,
    /// toggle, read, toggle back, read. Run explicitly.
    #[test]
    #[ignore = "flips live audio routing twice"]
    fn live_toggle_probe() {
        let _guard = live_lock();
        let mut vm = VoicemeeterState::new();
        let initial = vm.read_strip(2, "A1").expect("read 1");
        vm.execute(
            "vm-toggle-strip",
            &json!({"param": "A1", "number": 2}),
        )
        .expect("toggle 1");
        let flipped = vm.read_strip(2, "A1").expect("read 2");
        assert_ne!(initial, flipped);
        vm.execute(
            "vm-toggle-strip",
            &json!({"param": "A1", "number": 2}),
        )
        .expect("toggle 2");
        let restored = vm.read_strip(2, "A1").expect("read 3");
        assert_eq!(initial, restored);
    }

    /// Live test: touches the real Voicemeeter instance. Run explicitly:
    /// `cargo test -p pulpit-vm -- --ignored live_restart`
    #[test]
    #[ignore = "fires Command.Restart on the live audio engine"]
    fn live_restart() {
        let _guard = live_lock();
        let mut vm = VoicemeeterState::new();
        vm.execute("vm-restart", &Value::Null).expect("restart failed");
    }

    /// Live round trip of the bus gain slider on A3 (Bus[2]): move to the
    /// middle, verify, restore. Run explicitly:
    /// `cargo test -p pulpit-vm -- --ignored live_slider_bus_a3`
    #[test]
    #[ignore = "moves the live A3 bus fader twice"]
    fn live_slider_bus_a3() {
        let _guard = live_lock();
        let mut vm = VoicemeeterState::new();
        let initial = vm.read_bus(2, "Gain").expect("read gain");
        vm.execute(
            "vm-slider-bus",
            &json!({"param": "Gain", "number": 2, "value": 0.5}),
        )
        .expect("slider set");
        let mid = vm.read_bus(2, "Gain").expect("read mid");
        assert!((mid - slider_gain(0.5)).abs() < 0.2);
        vm.execute(
            "vm-set-bus",
            &json!({"param": "Gain", "number": 2, "value": initial}),
        )
        .expect("restore");
    }
}
