//! Windows implementation of [`crate::Speaker`]: WASAPI for volume and
//! enumeration, the undocumented-but-stable IPolicyConfig COM interface
//! (the same one SoundSwitch uses) for default-device switching - no
//! PowerShell dependency, unlike the original app.

use windows::core::{GUID, HRESULT, PCWSTR, PWSTR};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    eMultimedia, eRender, IMMDevice, IMMDeviceCollection, IMMDeviceEnumerator, MMDeviceEnumerator,
    DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
};

use crate::{AudioDevice, OsError, Result, Speaker};

/// `CLSID_PolicyConfigClient` - the PolicyConfig COM class every audio
/// device switcher drives.
const POLICY_CONFIG_CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

/// RPC_E_CHANGED_MODE: the thread already runs another apartment model,
/// which is fine - COM is usable either way.
const RPC_E_CHANGED_MODE: HRESULT = HRESULT(0x8001_0106u32 as i32);

pub struct WinSpeaker;

impl WinSpeaker {
    pub fn new() -> Self {
        Self
    }
}

/// COM is per-thread: initialize on each blocking call and leave it
/// initialized for the thread's lifetime (cheap; spawn_blocking threads
/// are reused).
fn ensure_com() -> Result<()> {
    let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if hr.is_err() && hr != RPC_E_CHANGED_MODE {
        return Err(OsError::Failed(format!("CoInitializeEx: {hr}")));
    }
    Ok(())
}

fn enumerator() -> Result<IMMDeviceEnumerator> {
    ensure_com()?;
    unsafe {
        CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
            .map_err(|e| OsError::Failed(format!("device enumerator: {e}")))
    }
}

/// Copy a CoTask-allocated wide string owned by WASAPI, then free it.
pub(crate) unsafe fn take_pwstr(ptr: PWSTR) -> String {
    let mut len = 0usize;
    while *ptr.0.add(len) != 0 {
        len += 1;
    }
    let s = String::from_utf16_lossy(std::slice::from_raw_parts(ptr.0, len));
    unsafe { CoTaskMemFree(Some(ptr.0.cast())) };
    s
}

fn endpoint_id(device: &IMMDevice) -> Result<String> {
    let id = unsafe { device.GetId() }.map_err(|e| OsError::Failed(format!("endpoint id: {e}")))?;
    Ok(unsafe { take_pwstr(id) })
}

/// Friendly device name from the endpoint property store.
fn device_name(device: &IMMDevice) -> String {
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    let Ok(store) = (unsafe { device.OpenPropertyStore(STGM_READ) }) else {
        return String::new();
    };
    let Ok(value) = (unsafe { store.GetValue(&PKEY_Device_FriendlyName) }) else {
        return String::new();
    };
    // PROPVARIANT renders its string payload via BSTR
    value.to_string()
}

fn endpoint_volume(device: &IMMDevice) -> Result<IAudioEndpointVolume> {
    unsafe { device.Activate(CLSCTX_ALL, None) }
        .map_err(|e| OsError::Failed(format!("endpoint volume: {e}")))
}

fn default_device(enumr: &IMMDeviceEnumerator) -> Result<IMMDevice> {
    unsafe { enumr.GetDefaultAudioEndpoint(eRender, eMultimedia) }
        .map_err(|e| OsError::Failed(format!("default device: {e}")))
}

/// Raw `IPolicyConfig` vtable. The interface is undocumented; this layout
/// (IUnknown plus twelve methods, `SetDefaultEndpoint` at slot 13) is the
/// one SoundSwitch/AudioSwitcher have shipped for a decade.
#[repr(C)]
struct PolicyConfigVtbl {
    query_interface:
        unsafe fn(*mut std::ffi::c_void, *const GUID, *mut *mut std::ffi::c_void) -> i32,
    add_ref: unsafe fn(*mut std::ffi::c_void) -> u32,
    release: unsafe fn(*mut std::ffi::c_void) -> u32,
    get_mix_format: *mut std::ffi::c_void,
    get_device_format: *mut std::ffi::c_void,
    reset_device_format: *mut std::ffi::c_void,
    set_device_format: *mut std::ffi::c_void,
    get_processing_period: *mut std::ffi::c_void,
    set_processing_period: *mut std::ffi::c_void,
    get_share_mode: *mut std::ffi::c_void,
    set_share_mode: *mut std::ffi::c_void,
    get_property_value: *mut std::ffi::c_void,
    set_property_value: *mut std::ffi::c_void,
    set_default_endpoint: unsafe fn(*mut std::ffi::c_void, PCWSTR, i32) -> i32,
    set_endpoint_visibility: *mut std::ffi::c_void,
}

#[repr(C)]
struct PolicyConfig {
    vtbl: *const PolicyConfigVtbl,
}

fn policy_config() -> Result<PolicyConfig> {
    ensure_com()?;
    let unk: windows::core::IUnknown = unsafe {
        CoCreateInstance(&POLICY_CONFIG_CLIENT, None, CLSCTX_ALL)
            .map_err(|e| OsError::Failed(format!("policy config: {e}")))?
    };
    use windows::core::Interface;
    // the interface pointer's first field IS the vtable pointer
    let object = unk.as_raw();
    Ok(PolicyConfig {
        vtbl: unsafe { *(object as *const *const PolicyConfigVtbl) },
    })
}

impl Speaker for WinSpeaker {
    fn volume(&mut self) -> Result<f32> {
        let enumr = enumerator()?;
        let vol = endpoint_volume(&default_device(&enumr)?)?;
        let value = unsafe { vol.GetMasterVolumeLevelScalar() }
            .map_err(|e| OsError::Failed(format!("get volume: {e}")))?;
        Ok((value * 100.0).clamp(0.0, 100.0))
    }

    fn muted(&mut self) -> Result<bool> {
        let enumr = enumerator()?;
        let vol = endpoint_volume(&default_device(&enumr)?)?;
        let mute = unsafe { vol.GetMute() }.map_err(|e| OsError::Failed(format!("get mute: {e}")))?;
        Ok(mute.as_bool())
    }

    fn set_volume(&mut self, percent: f32) -> Result<()> {
        let enumr = enumerator()?;
        let vol = endpoint_volume(&default_device(&enumr)?)?;
        unsafe { vol.SetMasterVolumeLevelScalar(percent.clamp(0.0, 100.0) / 100.0, std::ptr::null()) }
            .map_err(|e| OsError::Failed(format!("set volume: {e}")))
    }

    fn devices(&mut self) -> Result<Vec<AudioDevice>> {
        let enumr = enumerator()?;
        let default_id = default_device(&enumr).and_then(|d| endpoint_id(&d))?;
        let collection: IMMDeviceCollection = unsafe {
            enumr
                .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)
                .map_err(|e| OsError::Failed(format!("enum endpoints: {e}")))?
        };
        let count = unsafe { collection.GetCount() }
            .map_err(|e| OsError::Failed(format!("device count: {e}")))?;
        let mut out = Vec::with_capacity(count as usize);
        for i in 0..count {
            let Ok(device) = (unsafe { collection.Item(i) }) else {
                continue;
            };
            let id = endpoint_id(&device)?;
            let name = device_name(&device);
            out.push(AudioDevice {
                is_default: id == default_id,
                id,
                name,
            });
        }
        Ok(out)
    }

    fn active_device(&mut self) -> Result<String> {
        endpoint_id(&default_device(&enumerator()?)?)
    }

    fn set_active_device(&mut self, id: &str) -> Result<()> {
        let config = policy_config()?;
        let mut wide: Vec<u16> = id.encode_utf16().collect();
        wide.push(0);
        // roles: 0 = console, 1 = multimedia, 2 = communications - set all
        // three so every consumer follows the switch (what the Settings
        // app and SoundSwitch do)
        for role in 0..3i32 {
            let hr = HRESULT(unsafe {
                ((*config.vtbl).set_default_endpoint)(
                    std::mem::transmute::<&PolicyConfig, *mut std::ffi::c_void>(&config),
                    PCWSTR(wide.as_ptr()),
                    role,
                )
            });
            if hr.is_err() {
                return Err(OsError::Failed(format!("set default endpoint: {hr}")));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "live: touches the real audio device"]
    fn live_volume_round_trip() {
        let mut sp = WinSpeaker::new();
        let before = sp.volume().expect("volume read");
        sp.set_volume(23.0).expect("volume set");
        let now = sp.volume().expect("volume read after set");
        assert!((now - 23.0).abs() < 1.5, "expected ~23, got {now}");
        sp.set_volume(before).expect("restore");
    }

    #[test]
    #[ignore = "live: touches the real audio device"]
    fn live_device_list_and_noop_switch() {
        let mut sp = WinSpeaker::new();
        let devices = sp.devices().expect("devices");
        assert!(!devices.is_empty());
        assert!(devices.iter().any(|d| d.is_default));
        assert!(devices.iter().all(|d| !d.name.is_empty()));
        let active = sp.active_device().expect("active");
        // switching to the already-active endpoint exercises the COM path
        // without an audible change
        sp.set_active_device(&active).expect("same-device switch");
        assert_eq!(sp.active_device().expect("active after"), active);
    }
}
