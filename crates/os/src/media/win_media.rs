//! Windows implementation of SMTC media transport controls.

use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use serde_json::Value;
use windows::core::PCWSTR;
use windows::Foundation::TypedEventHandler;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession, GlobalSystemMediaTransportControlsSessionManager,
    GlobalSystemMediaTransportControlsSessionMediaProperties,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus,
};
use windows::Media::MediaPlaybackAutoRepeatMode;
use windows::Storage::Streams::DataReader;
use windows::Win32::Foundation::WIN32_ERROR;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ,
    REG_SZ,
};

use super::{
    aumid_to_friendly_name, build_snapshot, AutoRepeatMode, MediaProperties, PlaybackInfo,
    PlaybackStatus, TimelineProperties,
};
use crate::{OsError, Result};

/// Global waker used to notify the poller loop when external control
/// calls execute or consumers reconnect.
static WAKER: OnceLock<Arc<Waker>> = OnceLock::new();

fn global_waker() -> Arc<Waker> {
    WAKER.get_or_init(|| Arc::new(Waker::default())).clone()
}

#[derive(Default)]
struct Waker {
    mutex: Mutex<bool>,
    condvar: Condvar,
}

impl Waker {
    fn wake(&self) {
        let mut triggered = self.mutex.lock().unwrap();
        *triggered = true;
        self.condvar.notify_all();
    }

    fn wait_timeout(&self, timeout: Duration) {
        let mut triggered = self.mutex.lock().unwrap();
        if *triggered {
            *triggered = false;
            return;
        }
        let (mut guard, _) = self.condvar.wait_timeout(triggered, timeout).unwrap();
        *guard = false;
    }
}

/// Look up DisplayName from HKCU\Software\Classes\AppUserModelId registry keys.
pub(crate) fn lookup_registry_display_name(aumid: &str) -> Option<String> {
    let subkey_path: Vec<u16> = "Software\\Classes\\AppUserModelId\0"
        .encode_utf16()
        .collect();
    let mut root_key = HKEY::default();
    unsafe {
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey_path.as_ptr()),
            0,
            KEY_READ,
            &mut root_key,
        ) != WIN32_ERROR(0)
        {
            return None;
        }
    }

    let aumid_lower = aumid.to_lowercase();
    let mut index = 0u32;
    let mut name_buf = [0u16; 260];
    let mut matched_display_name = None;

    loop {
        let mut name_len = name_buf.len() as u32;
        let res = unsafe {
            RegEnumKeyExW(
                root_key,
                index,
                windows::core::PWSTR(name_buf.as_mut_ptr()),
                &mut name_len,
                None,
                windows::core::PWSTR::null(),
                None,
                None,
            )
        };
        if res != WIN32_ERROR(0) {
            break;
        }
        index += 1;

        let subkey_name = String::from_utf16_lossy(&name_buf[..name_len as usize]);
        let subkey_lower = subkey_name.to_lowercase();
        if subkey_lower.contains(&aumid_lower) || aumid_lower.contains(&subkey_lower) {
            // Read "DisplayName"
            let mut sub_key = HKEY::default();
            let sub_path: Vec<u16> = format!("{subkey_name}\0").encode_utf16().collect();
            let opened = unsafe {
                RegOpenKeyExW(
                    root_key,
                    PCWSTR(sub_path.as_ptr()),
                    0,
                    KEY_READ,
                    &mut sub_key,
                )
            };
            if opened == WIN32_ERROR(0) {
                let val_name: Vec<u16> = "DisplayName\0".encode_utf16().collect();
                let mut val_type = REG_SZ;
                let mut data_buf = [0u16; 260];
                let mut data_size = (data_buf.len() * 2) as u32;
                let queried = unsafe {
                    RegQueryValueExW(
                        sub_key,
                        PCWSTR(val_name.as_ptr()),
                        None,
                        Some(&mut val_type),
                        Some(data_buf.as_mut_ptr() as *mut u8),
                        Some(&mut data_size),
                    )
                };
                unsafe {
                    let _ = RegCloseKey(sub_key);
                };
                if queried == WIN32_ERROR(0) && data_size > 2 {
                    let chars = (data_size / 2) as usize;
                    let end = chars.saturating_sub(1); // strip trailing null
                    let display_name = String::from_utf16_lossy(&data_buf[..end])
                        .trim()
                        .to_string();
                    if !display_name.is_empty() {
                        matched_display_name = Some(display_name);
                        break;
                    }
                }
            }
        }
    }

    unsafe {
        let _ = RegCloseKey(root_key);
    };
    matched_display_name
}

fn init_mta() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

fn get_manager() -> Result<GlobalSystemMediaTransportControlsSessionManager> {
    init_mta();
    let op = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
        .map_err(|e| OsError::Failed(format!("request smtc manager: {e}")))?;
    op.get()
        .map_err(|e| OsError::Failed(format!("get smtc manager: {e}")))
}

fn read_media_properties(
    session: &GlobalSystemMediaTransportControlsSession,
) -> (MediaProperties, Option<(Vec<u8>, &'static str)>) {
    let mut props = MediaProperties::default();
    let mut art = None;

    if let Ok(op) = session.TryGetMediaPropertiesAsync() {
        if let Ok(p) = op.get() {
            props.title = p.Title().map(|s| s.to_string()).unwrap_or_default();
            props.artist = p.Artist().map(|s| s.to_string()).unwrap_or_default();
            props.album_title = p.AlbumTitle().map(|s| s.to_string()).unwrap_or_default();
            props.album_artist = p.AlbumArtist().map(|s| s.to_string()).unwrap_or_default();
            props.track_number = p.TrackNumber().ok();

            art = read_thumbnail(&p);
        }
    }

    (props, art)
}

fn read_thumbnail(
    p: &GlobalSystemMediaTransportControlsSessionMediaProperties,
) -> Option<(Vec<u8>, &'static str)> {
    let thumb_ref = p.Thumbnail().ok()?;
    let stream_op = thumb_ref.OpenReadAsync().ok()?;
    let stream = stream_op.get().ok()?;
    let size = stream.Size().ok()?;
    if size == 0 || size > 5 * 1024 * 1024 {
        return None;
    }
    let reader = DataReader::CreateDataReader(&stream).ok()?;
    reader.LoadAsync(size as u32).ok()?.get().ok()?;
    let mut bytes = vec![0u8; size as usize];
    reader.ReadBytes(&mut bytes).ok()?;

    let ext = if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "jpg"
    } else if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        "png"
    } else if bytes.len() > 12 && bytes.starts_with(b"RIFF") && bytes[8..].starts_with(b"WEBP") {
        "webp"
    } else {
        "jpg"
    };

    Some((bytes, ext))
}

fn read_playback_info(session: &GlobalSystemMediaTransportControlsSession) -> PlaybackInfo {
    let mut info = PlaybackInfo::default();
    if let Ok(p) = session.GetPlaybackInfo() {
        let status = match p.PlaybackStatus() {
            Ok(GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing) => {
                PlaybackStatus::Playing
            }
            Ok(GlobalSystemMediaTransportControlsSessionPlaybackStatus::Paused) => {
                PlaybackStatus::Paused
            }
            Ok(GlobalSystemMediaTransportControlsSessionPlaybackStatus::Stopped) => {
                PlaybackStatus::Stopped
            }
            Ok(GlobalSystemMediaTransportControlsSessionPlaybackStatus::Changing) => {
                PlaybackStatus::Changing
            }
            Ok(GlobalSystemMediaTransportControlsSessionPlaybackStatus::Opened) => {
                PlaybackStatus::Opened
            }
            _ => PlaybackStatus::Closed,
        };
        info.status = status;
        info.is_playing = status == PlaybackStatus::Playing;

        if let Ok(shuffle_ref) = p.IsShuffleActive() {
            info.shuffle = shuffle_ref.GetBoolean().ok();
        }
        if let Ok(repeat_ref) = p.AutoRepeatMode() {
            info.repeat = match repeat_ref.Value() {
                Ok(MediaPlaybackAutoRepeatMode::Track) => Some(AutoRepeatMode::Track),
                Ok(MediaPlaybackAutoRepeatMode::List) => Some(AutoRepeatMode::List),
                _ => Some(AutoRepeatMode::None),
            };
        }

        if let Ok(c) = p.Controls() {
            info.can_play = c.IsPlayEnabled().unwrap_or(false);
            info.can_pause = c.IsPauseEnabled().unwrap_or(false);
            info.can_skip_next = c.IsNextEnabled().unwrap_or(false);
            info.can_skip_prev = c.IsPreviousEnabled().unwrap_or(false);
            info.can_stop = c.IsStopEnabled().unwrap_or(false);
            info.can_seek = c.IsPlaybackPositionEnabled().unwrap_or(false);
        }
    }
    info
}

fn read_timeline_properties(
    session: &GlobalSystemMediaTransportControlsSession,
) -> TimelineProperties {
    let mut timeline = TimelineProperties::default();
    if let Ok(t) = session.GetTimelineProperties() {
        if let Ok(pos) = t.Position() {
            timeline.position_ms = (pos.Duration.max(0) / 10_000) as u64;
        }
        if let Ok(end) = t.EndTime() {
            let start = t.StartTime().map(|s| s.Duration).unwrap_or(0);
            let dur = (end.Duration - start).max(0);
            timeline.duration_ms = (dur / 10_000) as u64;
        }
        if let Ok(min) = t.MinSeekTime() {
            timeline.min_seek_ms = (min.Duration.max(0) / 10_000) as u64;
        }
        if let Ok(max) = t.MaxSeekTime() {
            timeline.max_seek_ms = (max.Duration.max(0) / 10_000) as u64;
        }
    }
    timeline
}

fn find_matching_session(
    manager: &GlobalSystemMediaTransportControlsSessionManager,
    target_app: Option<&str>,
) -> Option<GlobalSystemMediaTransportControlsSession> {
    // An explicit target wins: no matching session is an error, never a
    // silent fallback to whatever the system currently focuses (the tile
    // promised to control that one app).
    if let Some(target) = target_app.filter(|s| !s.trim().is_empty()) {
        let pattern = target.trim().to_lowercase();
        if let Ok(sessions) = manager.GetSessions() {
            let count = sessions.Size().unwrap_or(0);
            for i in 0..count {
                if let Ok(s) = sessions.GetAt(i) {
                    if let Ok(aumid) = s.SourceAppUserModelId() {
                        let aumid_str = aumid.to_string();
                        let friendly = aumid_to_friendly_name(&aumid_str);
                        if aumid_str.to_lowercase().contains(&pattern)
                            || friendly.to_lowercase().contains(&pattern)
                        {
                            return Some(s);
                        }
                    }
                }
            }
        }
        return None;
    }
    manager.GetCurrentSession().ok()
}

pub(crate) fn control(
    target_app: Option<&str>,
    action: &str,
    slider_value: Option<f64>,
) -> Result<bool> {
    let manager = get_manager()?;
    let session = find_matching_session(&manager, target_app)
        .ok_or_else(|| OsError::Failed("no active media session found".to_string()))?;

    let act = action.trim().to_lowercase();
    let res = match act.as_str() {
        "play-pause" => session
            .TryTogglePlayPauseAsync()
            .map_err(|e| OsError::Failed(format!("play-pause: {e}")))?
            .get()
            .unwrap_or(false),
        "play" => session
            .TryPlayAsync()
            .map_err(|e| OsError::Failed(format!("play: {e}")))?
            .get()
            .unwrap_or(false),
        "pause" => session
            .TryPauseAsync()
            .map_err(|e| OsError::Failed(format!("pause: {e}")))?
            .get()
            .unwrap_or(false),
        "next" => session
            .TrySkipNextAsync()
            .map_err(|e| OsError::Failed(format!("next: {e}")))?
            .get()
            .unwrap_or(false),
        "previous" | "prev" => session
            .TrySkipPreviousAsync()
            .map_err(|e| OsError::Failed(format!("previous: {e}")))?
            .get()
            .unwrap_or(false),
        "stop" => session
            .TryStopAsync()
            .map_err(|e| OsError::Failed(format!("stop: {e}")))?
            .get()
            .unwrap_or(false),
        "seek" => {
            let val = slider_value.unwrap_or(0.0).clamp(0.0, 1.0);
            let timeline = session
                .GetTimelineProperties()
                .map_err(|e| OsError::Failed(format!("get timeline: {e}")))?;
            let start = timeline.StartTime().map(|s| s.Duration).unwrap_or(0);
            let end = timeline.EndTime().map(|s| s.Duration).unwrap_or(0);
            let duration = (end - start).max(0);
            let target_pos = start + (duration as f64 * val) as i64;
            session
                .TryChangePlaybackPositionAsync(target_pos)
                .map_err(|e| OsError::Failed(format!("seek: {e}")))?
                .get()
                .unwrap_or(false)
        }
        other => {
            return Err(OsError::Failed(format!(
                "unsupported media action: {other}"
            )))
        }
    };

    global_waker().wake();
    Ok(res)
}

pub(crate) fn list_sessions() -> Result<Vec<String>> {
    let manager = get_manager()?;
    let mut names = Vec::new();
    if let Ok(sessions) = manager.GetSessions() {
        let count = sessions.Size().unwrap_or(0);
        for i in 0..count {
            if let Ok(s) = sessions.GetAt(i) {
                if let Ok(aumid) = s.SourceAppUserModelId() {
                    let friendly = aumid_to_friendly_name(&aumid.to_string());
                    if !friendly.is_empty() && !names.contains(&friendly) {
                        names.push(friendly);
                    }
                }
            }
        }
    }
    Ok(names)
}

pub(crate) fn spawn_push(
    consumers: Arc<dyn Fn() -> usize + Send + Sync + 'static>,
) -> tokio::sync::mpsc::UnboundedReceiver<Value> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let waker = global_waker();

    std::thread::Builder::new()
        .name("smtc-push".to_string())
        .spawn(move || {
            init_mta();
            let manager = match GlobalSystemMediaTransportControlsSessionManager::RequestAsync() {
                Ok(op) => match op.get() {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::warn!(error = %e, "failed to get SMTC session manager");
                        return;
                    }
                },
                Err(e) => {
                    tracing::warn!(error = %e, "failed to request SMTC session manager");
                    return;
                }
            };

            // Register Manager events
            let waker_sessions = waker.clone();
            let _ = manager.SessionsChanged(&TypedEventHandler::new(move |_, _| {
                waker_sessions.wake();
                Ok(())
            }));

            let waker_current = waker.clone();
            let _ = manager.CurrentSessionChanged(&TypedEventHandler::new(move |_, _| {
                waker_current.wake();
                Ok(())
            }));

            let mut last_session_id = String::new();
            let mut last_snapshot_str = String::new();

            loop {
                let num_consumers = consumers();
                if num_consumers == 0 {
                    // Plan 004 quiet idle loops: wait when nobody renders state
                    waker.wait_timeout(Duration::from_secs(5));
                    continue;
                }

                let current_session = manager.GetCurrentSession().ok();
                let current_id = current_session
                    .as_ref()
                    .and_then(|s| s.SourceAppUserModelId().ok())
                    .map(|h| h.to_string())
                    .unwrap_or_default();

                // Attach session events when session changes
                if current_id != last_session_id {
                    last_session_id = current_id.clone();
                    if let Some(session) = &current_session {
                        let w = waker.clone();
                        let _ =
                            session.MediaPropertiesChanged(&TypedEventHandler::new(move |_, _| {
                                w.wake();
                                Ok(())
                            }));
                        let w = waker.clone();
                        let _ =
                            session.PlaybackInfoChanged(&TypedEventHandler::new(move |_, _| {
                                w.wake();
                                Ok(())
                            }));
                        let w = waker.clone();
                        let _ = session.TimelinePropertiesChanged(&TypedEventHandler::new(
                            move |_, _| {
                                w.wake();
                                Ok(())
                            },
                        ));
                    }
                }

                let (props, playback, timeline, app_name, art) = match &current_session {
                    Some(session) => {
                        let (p, art) = read_media_properties(session);
                        let pb = read_playback_info(session);
                        let tl = read_timeline_properties(session);
                        let name = aumid_to_friendly_name(&current_id);
                        (Some(p), Some(pb), Some(tl), name, art)
                    }
                    None => (None, None, None, String::new(), None),
                };

                let is_playing = playback.as_ref().is_some_and(|p| p.is_playing);
                let art_ref = art.as_ref().map(|(b, ext)| (b.as_slice(), *ext));
                let snapshot = build_snapshot(
                    props.as_ref(),
                    playback.as_ref(),
                    timeline.as_ref(),
                    &app_name,
                    art_ref,
                );

                // Change detection: compare serialized string without art data
                let mut compare_val = snapshot.clone();
                if let Some(obj) = compare_val.as_object_mut() {
                    obj.remove(super::ART_DATA_KEY);
                }
                let snapshot_str = compare_val.to_string();
                if snapshot_str != last_snapshot_str {
                    last_snapshot_str = snapshot_str;
                    if tx.send(snapshot).is_err() {
                        break; // receiver dropped
                    }
                }

                // If playing, sleep with 2.5s timeout for natural track changes;
                // if paused/idle, sleep up to 10s or until event wakes us
                let sleep_duration = if is_playing {
                    Duration::from_millis(2500)
                } else {
                    Duration::from_secs(10)
                };
                waker.wait_timeout(sleep_duration);
            }
        })
        .expect("spawn smtc worker thread");

    rx
}
