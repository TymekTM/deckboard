//! OpenAI Codex CLI as a data source (`~/.codex/sessions/YYYY/MM/DD/*.jsonl`).
//!
//! Two reads, both local-only:
//! - *agent sessions*: the first record of every rollout (`session_meta`)
//!   carries the working directory, so fresh files map to projects; the
//!   file mtime is the heartbeat.
//! - *plan rate limits*: every `token_count` event embeds the
//!   server-reported `rate_limits` (primary/secondary with used_percent,
//!   window length and reset time) - the freshest record across files is
//!   the current state. No API call, no OAuth handling.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::agents::AgentSession;
use crate::util::{mtime, truncate};

const TAIL_BYTES: u64 = 64 * 1024;

/// A session with recent activity, as the agents tile sees it.
pub fn sessions(sessions_dir: &Path, config: &crate::Config, now: i64, out: &mut Vec<AgentSession>) {
    let cutoff = now - config.agent_done_secs - crate::util::DONE_GRACE_SECS;
    let mut files = Vec::new();
    collect_jsonl(sessions_dir, 0, &mut files);
    for path in files {
        let Some(last_ts) = mtime(&path) else {
            continue;
        };
        if last_ts < cutoff {
            continue;
        }
        let project = session_meta_cwd(&path)
            .map(|cwd| {
                let base = cwd.rsplit(['\\', '/']).find(|s| !s.is_empty()).unwrap_or(&cwd);
                base.to_string()
            })
            .unwrap_or_else(|| "codex".into());
        // rollouts carry no title record; the first user prompt is what
        // the session is about, the stem fragment is the fallback
        let frag = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|stem| stem.rsplit('-').next())
            .unwrap_or("ses")
            .to_string();
        let title = first_user_message(&path).unwrap_or_else(|| format!("codex {frag}"));
        out.push(AgentSession {
            provider: "codex",
            project,
            title: truncate(&title, 60),
            last_ts,
        });
    }
}

/// `session_meta` is the first record of a rollout; reading a bounded head
/// keeps huge transcripts cheap - a torn first line simply yields no cwd.
fn session_meta_cwd(path: &Path) -> Option<String> {
    let Ok(mut file) = std::fs::File::open(path) else {
        return None;
    };
    let mut head = vec![0u8; 256 * 1024];
    let read = file.read(&mut head).unwrap_or(0);
    let text = String::from_utf8_lossy(&head[..read]);
    let line = text.lines().next()?;
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? != "session_meta" {
        return None;
    }
    v.pointer("/payload/cwd")?
        .as_str()
        .map(|s| s.to_string())
}

/// The first `user_message` event of a rollout, scanned over the same
/// bounded head. The prompt is the closest thing to a session title the
/// format carries; transcripts without one fall back to the stem label.
fn first_user_message(path: &Path) -> Option<String> {
    let Ok(mut file) = std::fs::File::open(path) else {
        return None;
    };
    let mut head = vec![0u8; 256 * 1024];
    let read = file.read(&mut head).unwrap_or(0);
    let text = String::from_utf8_lossy(&head[..read]);
    text.lines().find_map(|line| {
        let v: serde_json::Value = serde_json::from_str(line).ok()?;
        if v.get("type")?.as_str()? != "event_msg" {
            return None;
        }
        if v.pointer("/payload/type")?.as_str()? != "user_message" {
            return None;
        }
        v.pointer("/payload/message")?
            .as_str()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    })
}

/// ---- rate limits -----------------------------------------------------------

#[derive(Default, Clone)]
pub struct Limits {
    pub plan_type: Option<String>,
    pub primary: Option<RateLimit>,
    pub secondary: Option<RateLimit>,
}

#[derive(Clone)]
pub struct RateLimit {
    pub used_percent: f64,
    pub window_minutes: u64,
}

impl RateLimit {
    /// Human window label: the API reports minutes (300 = 5h session
    /// window on paid plans, 10080 = week, 43200 = 30 days on free).
    pub fn window_label(&self) -> String {
        match self.window_minutes {
            300 => "5h".into(),
            1_440 => "24h".into(),
            10_080 => "week".into(),
            43_200 => "30d".into(),
            m if m >= 1_440 => format!("{}d", m / 1_440),
            m => format!("{m}m"),
        }
    }
}

/// The freshest embedded rate limits across recent rollout files. Every
/// `token_count` event carries them, so reading each file's tail and
/// keeping the newest record reconstructs the current state without
/// replaying full transcripts.
pub fn limits(sessions_dir: &Path, now: i64) -> Option<Limits> {
    let cutoff = now - 8 * 86_400;
    let mut files = Vec::new();
    collect_jsonl(sessions_dir, 0, &mut files);
    let mut best: Option<(i64, Limits)> = None;
    for path in files {
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        let mtime = modified
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        if mtime < cutoff {
            continue;
        }
        if let Some((ts, limits)) = tail_rate_limits(&path) {
            if best.as_ref().map_or(true, |(b, _)| ts > *b) {
                best = Some((ts, limits));
            }
        }
    }
    best.map(|(_, limits)| limits)
}

/// Parse the last `rate_limits`-bearing token_count record in the file's
/// tail. Returns the record timestamp with the limits for ordering.
fn tail_rate_limits(path: &Path) -> Option<(i64, Limits)> {
    let Ok(mut file) = std::fs::File::open(path) else {
        return None;
    };
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut tail = String::new();
    file.read_to_string(&mut tail).ok()?;
    // the first line after a mid-file seek may be torn: drop it
    if start > 0 {
        if let Some(pos) = tail.find('\n') {
            tail.drain(..pos + 1);
        }
    }
    let mut newest: Option<(i64, Limits)> = None;
    for line in tail.lines().rev() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let payload = v.get("payload")?;
        if payload.get("type")?.as_str()? != "token_count" {
            continue;
        }
        let Some(rl) = payload.get("rate_limits") else {
            continue;
        };
        let Some(limits) = parse_limits(rl) else {
            continue;
        };
        let ts = crate::local_usage::parse_iso_rfc3339(v.get("timestamp")?.as_str()?).unwrap_or(0);
        if newest.as_ref().map_or(true, |(b, _)| ts > *b) {
            newest = Some((ts, limits));
        }
        break; // scanning backwards, the first hit is the file's latest
    }
    newest
}

fn parse_limits(rl: &serde_json::Value) -> Option<Limits> {
    let window = |key: &str| {
        let w = rl.get(key)?;
        let used_percent = w.get("used_percent")?.as_f64()?;
        Some(RateLimit {
            used_percent,
            window_minutes: w.get("window_minutes").and_then(|x| x.as_u64()).unwrap_or(0),
        })
    };
    Some(Limits {
        plan_type: rl
            .get("plan_type")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string()),
        primary: window("primary"),
        secondary: window("secondary"),
    })
}

/// ---- helpers ----------------------------------------------------------------

fn collect_jsonl(dir: &Path, depth: u8, out: &mut Vec<std::path::PathBuf>) {
    if depth > 4 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(path);
        } else if path.is_dir() {
            collect_jsonl(&path, depth + 1, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("aidev-codex-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn token_count_line(ts: &str, input: u64, cached: u64, used: f64, window: u64) -> String {
        format!(
            r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{input},"cached_input_tokens":{cached},"output_tokens":10,"total_tokens":0}},"last_token_usage":{{"input_tokens":{input},"cached_input_tokens":{cached},"output_tokens":10}}}},"rate_limits":{{"limit_id":"codex","primary":{{"used_percent":{used},"window_minutes":{window},"resets_at":1790000000}},"secondary":null,"plan_type":"free"}}}}}}"#
        )
    }

    #[test]
    fn sessions_map_fresh_rollouts_to_projects() {
        let dir = tmp_dir("sess");
        let day = dir.join("2026").join("09").join("24");
        std::fs::create_dir_all(&day).unwrap();
        let f = day.join("rollout-20260924T1200-abc-def.jsonl");
        std::fs::write(
            &f,
            concat!(
                r#"{"timestamp":"2026-09-24T12:00:00.000Z","type":"session_meta","payload":{"session_id":"abc","cwd":"F:\\projects\\deckboard clone","originator":"codex-tui"}}"#,
                "\n",
                r#"{"timestamp":"2026-09-24T12:00:01.000Z","type":"event_msg","payload":{"type":"task_started"}}"#,
                "\n",
                r#"{"timestamp":"2026-09-24T12:00:02.000Z","type":"event_msg","payload":{"type":"user_message","message":"  Add the preset editor  "}}"#,
                "\n"
            ),
        )
        .unwrap();

        let mut out = Vec::new();
        let now = crate::unix_now();
        sessions(&dir, &crate::Config::default(), now, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].provider, "codex");
        assert_eq!(out[0].project, "deckboard clone");
        // the first user prompt is the session title, trimmed
        assert_eq!(out[0].title, "Add the preset editor");
        assert!((out[0].last_ts - now).abs() < 5);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn limits_pick_the_freshest_embedded_record() {
        let dir = tmp_dir("lim");
        let day = dir.join("2026").join("09").join("24");
        std::fs::create_dir_all(&day).unwrap();
        let f = day.join("rollout-x.jsonl");
        std::fs::write(
            &f,
            format!(
                "{}\n{}\n",
                token_count_line("2026-09-24T11:00:00Z", 100, 0, 10.0, 43200),
                token_count_line("2026-09-24T12:00:00Z", 200, 50, 59.0, 43200),
            ),
        )
        .unwrap();

        let now = crate::unix_now();
        let limits = limits(&dir, now).expect("limits from fresh file");
        assert_eq!(limits.plan_type.as_deref(), Some("free"));
        let primary = limits.primary.expect("primary window");
        assert_eq!(primary.used_percent, 59.0);
        assert_eq!(primary.window_label(), "30d");
        assert!(limits.secondary.is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_files_are_ignored() {
        let dir = tmp_dir("old");
        let f = dir.join("rollout-old.jsonl");
        std::fs::write(
            &f,
            token_count_line("2026-08-03T14:03:03Z", 100, 0, 10.0, 300),
        )
        .unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(20 * 86_400);
        let file = std::fs::File::options().append(true).open(&f).unwrap();
        file.set_modified(old).unwrap();
        assert!(limits(&dir, crate::unix_now()).is_none());
        let mut out = Vec::new();
        sessions(&dir, &crate::Config::default(), crate::unix_now(), &mut out);
        assert!(out.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
