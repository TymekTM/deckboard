//! Windows master audio endpoint: volume + mute read/write. This is the
//! original app's "Multimedia" backend (speaker-volume slider tiles, the
//! 5 s status poll that drives mute-tile state).

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

// Each call opens its own COM session: exec arrives on arbitrary worker
// threads and the 5 s poll is far too slow for COM setup to matter.
#[cfg(windows)]
fn with_endpoint<T>(
    f: impl FnOnce(
        &windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume,
    ) -> windows::core::Result<T>,
) -> windows::core::Result<T> {
    use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::Media::Audio::{
        eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
    };

    unsafe {
        let init = CoInitializeEx(None, COINIT_MULTITHREADED);
        // another apartment model on this thread still serves our purpose
        let must_uninit = init.is_ok();
        if init.is_err() && init != RPC_E_CHANGED_MODE {
            return Err(init.into());
        }
        let result = (|| {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
            let endpoint: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None)?;
            f(&endpoint)
        })();
        if must_uninit {
            CoUninitialize();
        }
        result
    }
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
