//! Windows master audio endpoint: volume + mute read/write, output device
//! list/switch. This is the original app's "Multimedia" backend
//! (speaker-volume slider tiles, the 5 s status poll that drives mute-tile
//! state, Set Audio Device buttons).

#[cfg(windows)]
use windows::core::PCWSTR;
#[cfg(windows)]
use windows::Win32::Media::Audio::{eConsole, eRender, DEVICE_STATE_ACTIVE};
#[cfg(windows)]
use windows::Win32::System::Com::CLSCTX_ALL;

#[cfg(windows)]
pub fn master_status() -> Option<(f32, bool)> {
    with_endpoint(|ep| unsafe {
        let volume = ep.GetMasterVolumeLevelScalar()?;
        let muted = ep.GetMute()?.as_bool();
        Ok((volume, muted))
    })
    .ok()
}

#[cfg(windows)]
pub fn set_master_volume(volume: f32) -> Result<(), String> {
    let volume = volume.clamp(0.0, 1.0);
    with_endpoint(|ep| unsafe { ep.SetMasterVolumeLevelScalar(volume, std::ptr::null()) })
        .map_err(|e| format!("set master volume failed: {e}"))
}

// ---- output devices (list / default / switch) ------------------------------
//
// The original shelled out to PowerShell's AudioDeviceCmdlets module for
// these; we talk to Core Audio directly. Switching the default endpoint
// has no documented API - every switcher (SoundSwitch, AudioDeviceCmdlets
// itself) calls the undocumented IPolicyConfig, so the vtable is declared
// by hand below.

/// Active render endpoints: `(endpoint id, friendly name)`.
#[cfg(windows)]
pub fn output_devices() -> Vec<(String, String)> {
    with_enumerator(|enumerator| unsafe {
        let collection = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
        let count = collection.GetCount()?;
        let mut out = Vec::new();
        for i in 0..count {
            let device = collection.Item(i)?;
            let id = device.GetId()?.to_string()?;
            out.push((id, endpoint_name(&device)?));
        }
        Ok(out)
    })
    .unwrap_or_default()
}

/// Endpoint id of the current default console device.
#[cfg(windows)]
pub fn default_output_device_id() -> Option<String> {
    with_enumerator(|enumerator| unsafe {
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let id = device.GetId()?;
        Ok(id.to_string()?)
    })
    .ok()
}

/// Make `id` the default endpoint for all roles (console, multimedia,
/// communications), like the original's `Set-AudioDevice -ID`.
#[cfg(windows)]
pub fn set_default_output_device(id: &str) -> Result<(), String> {
    with_enumerator(|_| unsafe {
        let mut raw: *mut core::ffi::c_void = std::ptr::null_mut();
        let hr = CoCreateInstance(
            &CLSID_CPolicyConfigClient,
            std::ptr::null_mut(),
            CLSCTX_ALL.0,
            &IID_IPolicyConfig,
            &mut raw,
        );
        if hr.is_err() {
            return Err(windows::core::Error::from(hr));
        }
        let result = (|| {
            let vtable = (*(raw as *mut PolicyConfig)).vtable;
            let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
            // 0/1/2 = console, multimedia, communications (ERole)
            for role in [0i32, 1, 2] {
                let hr = ((*vtable).set_default_endpoint)(raw, PCWSTR(wide.as_ptr()), role);
                if hr.is_err() {
                    return Err(windows::core::Error::from(hr));
                }
            }
            Ok(())
        })();
        ((*(*(raw as *mut PolicyConfig)).vtable).release)(raw);
        result
    })
    .map_err(|e| format!("set default device failed: {e}"))
}

#[cfg(windows)]
unsafe fn endpoint_name(
    device: &windows::Win32::Media::Audio::IMMDevice,
) -> windows::core::Result<String> {
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::System::Com::StructuredStorage::PropVariantClear;
    use windows::Win32::System::Com::STGM_READ;
    use windows::Win32::System::Variant::VT_LPWSTR;
    use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;

    let store: IPropertyStore = device.OpenPropertyStore(STGM_READ)?;
    let mut prop = store.GetValue(&PKEY_Device_FriendlyName)?;
    // name lives in the VT_LPWSTR arm of the nested union
    let inner = &prop.Anonymous.Anonymous;
    let name = if inner.vt == VT_LPWSTR {
        let pwsz = inner.Anonymous.pwszVal;
        if pwsz.is_null() {
            String::new()
        } else {
            PCWSTR(pwsz.0).to_string().unwrap_or_default()
        }
    } else {
        String::new()
    };
    let _ = PropVariantClear(&mut prop);
    Ok(name)
}

// CoCreateInstance over the raw ABI: PolicyConfig is not a windows-rs
// interface, so the typed helper cannot instantiate it.
#[cfg(windows)]
#[link(name = "ole32")]
extern "system" {
    fn CoCreateInstance(
        rclsid: *const windows::core::GUID,
        punk_outer: *mut core::ffi::c_void,
        cls_context: u32,
        riid: *const windows::core::GUID,
        ppv: *mut *mut core::ffi::c_void,
    ) -> windows::core::HRESULT;
}

#[cfg(windows)]
#[allow(non_upper_case_globals)]
const CLSID_CPolicyConfigClient: windows::core::GUID = windows::core::GUID::from_u128(
    0x870af99c_171d_4f9e_af0d_e63df40c2bc9,
);
#[cfg(windows)]
#[allow(non_upper_case_globals)]
const IID_IPolicyConfig: windows::core::GUID =
    windows::core::GUID::from_u128(0xf8679f50_850a_41cf_9c72_430f290290c8);

/// Hand-declared IPolicyConfig vtable. Only `set_default_endpoint` is ever
/// called; the other slots exist to keep the layout aligned. Slot order is
/// the binary contract: the interface carries 12 methods after IUnknown,
/// and a shorter layout silently calls SetPropertyValue instead of
/// SetDefaultEndpoint (it still returns success).
#[cfg(windows)]
#[repr(C)]
struct PolicyConfigVtable {
    query_interface: usize,
    add_ref: usize,
    release: unsafe extern "system" fn(*mut core::ffi::c_void) -> u32,
    _get_mix_format: usize,
    _get_device_format: usize,
    _reset_device_format: usize,
    _set_device_format: usize,
    _get_processing_period: usize,
    _set_processing_period: usize,
    _get_share_mode: usize,
    _set_share_mode: usize,
    _get_property_value: usize,
    _set_property_value: usize,
    set_default_endpoint: unsafe extern "system" fn(
        *mut core::ffi::c_void,
        windows::core::PCWSTR,
        i32,
    ) -> windows::core::HRESULT,
    _set_endpoint_visibility: usize,
}

#[cfg(windows)]
#[repr(C)]
struct PolicyConfig {
    vtable: *const PolicyConfigVtable,
}

// Each call opens its own COM session: exec arrives on arbitrary worker
// threads and the 5 s poll is far too slow for COM setup to matter.
#[cfg(windows)]
fn with_enumerator<T>(
    f: impl FnOnce(
        &windows::Win32::Media::Audio::IMMDeviceEnumerator,
    ) -> windows::core::Result<T>,
) -> windows::core::Result<T> {
    use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
    use windows::Win32::Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
    };

    unsafe {
        let init = CoInitializeEx(None, COINIT_MULTITHREADED);
        // another apartment model on this thread still serves our purpose;
        // S_FALSE means the thread already had this apartment - someone
        // else owns it and must not be torn down here
        let must_uninit = init == windows::core::HRESULT(0);
        if init.is_err() && init != RPC_E_CHANGED_MODE {
            return Err(init.into());
        }
        let result = (|| {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            f(&enumerator)
        })();
        if must_uninit {
            CoUninitialize();
        }
        result
    }
}

#[cfg(windows)]
fn with_endpoint<T>(
    f: impl FnOnce(
        &windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume,
    ) -> windows::core::Result<T>,
) -> windows::core::Result<T> {
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::Media::Audio::{eConsole, eRender};

    with_enumerator(|enumerator| unsafe {
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let endpoint: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None)?;
        f(&endpoint)
    })
}

#[cfg(not(windows))]
pub fn master_status() -> Option<(f32, bool)> {
    None
}

#[cfg(not(windows))]
pub fn set_master_volume(_volume: f32) -> Result<(), String> {
    Err("audio backend is Windows-only".into())
}

#[cfg(all(test, windows))]
mod tests {
    // restores the machine's master volume even if an assert panics -
    // this test fiddles with real system audio
    struct VolumeGuard(f32);
    impl Drop for VolumeGuard {
        fn drop(&mut self) {
            let _ = super::set_master_volume(self.0);
        }
    }

    #[test]
    fn master_volume_reports_in_unit_range() {
        let (volume, _muted) = super::master_status().expect("core audio endpoint");
        assert!((0.0..=1.0).contains(&volume));
    }

    #[test]
    fn output_devices_listed_with_default_among_them() {
        let devices = super::output_devices();
        assert!(!devices.is_empty(), "at least one active render endpoint");
        let default = super::default_output_device_id().expect("default endpoint id");
        assert!(
            devices.iter().any(|(id, _)| *id == default),
            "default {default} should be one of the listed endpoints"
        );
    }

    #[test]
    fn set_volume_roundtrips_within_driver_steps() {
        let (prev, _) = super::master_status().expect("core audio endpoint");
        let _restore = VolumeGuard(prev);
        super::set_master_volume(0.42).expect("set volume");
        let (volume, _) = super::master_status().expect("read back");
        // drivers quantize the scalar to hardware steps (often 1/50)
        assert!(
            (volume - 0.42).abs() < 0.06,
            "read back {volume} after setting 0.42"
        );
    }
}
