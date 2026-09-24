//! Agent progress detection, provider-agnostic.
//!
//! Every tool leaves its own trail and the tile merges them:
//! - ZCode journals conversations in `~/.zcode/cli/db/db.sqlite` (title,
//!   project directory, `time_updated` heartbeat);
//! - Claude Code writes per-session JSONL transcripts under
//!   `~/.claude/projects/<project>/`;
//! - OpenAI Codex rollouts under `~/.codex/sessions/` carry the working
//!   directory in their first `session_meta` record (see [`crate::codex`]);
//! - OpenCode keeps a `session` table in `opencode.db` shaped like
//!   ZCode's journal (see [`crate::opencode`]);
//! - Google Antigravity persists one SQLite database per conversation
//!   under `~/.gemini/antigravity/conversations/` (see [`crate::antigravity`]).
//!
//! Fresh writes mean "working"; a session quiet for minutes means "may
//! need attention"; older means "done". A provider is shown only while it
//! has at least one working or attention session, and rows are grouped by
//! project with the provider glyph on every row. When the tile cannot fit
//! the detail, the renderer switches to the per-provider `compact` counts.

use std::collections::{BTreeMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::antigravity;
use crate::codex;
use crate::opencode;
use crate::{Config, Paths};
use crate::util::{mtime, truncate};

/// How many sessions of one project to show before the group is trimmed.
const PER_PROJECT_CAP: usize = 2;
/// Hard row budget (headers included) before the renderer is expected to
/// fall back to the compact counts.
const MAX_ROWS: usize = 12;
/// Canonical provider order for compact rows.
const PROVIDERS: [&str; 5] = ["zcode", "claude", "codex", "opencode", "antigravity"];

/// One conversation with recent activity, as the agents tile sees it.
pub(crate) struct AgentSession {
    pub provider: &'static str,
    pub project: String,
    pub title: String,
    pub last_ts: i64,
}

/// Build the `ai-agent-status` snapshot payload: detailed project-grouped
/// rows plus per-provider compact counts.
pub fn snapshot(config: &Config, paths: &Paths, now: i64) -> serde_json::Value {
    let mut sessions = Vec::new();
    zcode_sessions(
        &paths.zcode_cli.join("db").join("db.sqlite"),
        config,
        now,
        &mut sessions,
    );
    if sessions.is_empty() {
        // no journal (other ZCode build, schema drift) - degrade to rollout
        // file mtimes so the tile still shows liveness, just without titles
        rollout_fallback(&paths.zcode_cli.join("rollout"), &mut sessions);
    }
    claude_sessions(&paths.claude_projects, &mut sessions);
    codex::sessions(&paths.codex_sessions, config, now, &mut sessions);
    opencode::sessions(&paths.opencode_db, config, now, &mut sessions);
    antigravity::sessions(
        &paths.antigravity_conversations,
        config,
        now,
        &mut sessions,
    );

    let classified: Vec<(&AgentSession, &'static str)> = sessions
        .iter()
        .filter_map(|s| classify((now - s.last_ts).max(0), config).map(|state| (s, state)))
        .collect();

    // a provider is on the tile only while something of it is alive:
    // working or needing attention - finished sessions alone stay hidden
    let active: HashSet<&str> = classified
        .iter()
        .filter(|(_, state)| *state != "done")
        .map(|(session, _)| session.provider)
        .collect();
    let visible: Vec<(&AgentSession, &'static str)> = classified
        .into_iter()
        .filter(|(session, _)| active.contains(session.provider))
        .collect();

    let count = |want: &'static str| {
        visible
            .iter()
            .filter(|(_, state)| *state == want)
            .count() as u32
    };
    let (working, attention, done) = (count("working"), count("attention"), count("done"));

    let mut rows = grouped_rows(&visible, now);
    if rows.is_empty() {
        rows.push(serde_json::json!({
            "label": "no agent activity",
            "value": "",
            "state": "off",
        }));
    }

    let mut summary = String::new();
    for (count, label) in [(working, "working"), (attention, "attention"), (done, "done")] {
        if count > 0 {
            if !summary.is_empty() {
                summary.push_str(" · ");
            }
            summary.push_str(&format!("{count} {label}"));
        }
    }
    if summary.is_empty() {
        summary = "idle".into();
    }

    serde_json::json!({
        "title": "Agent progress",
        "rows": rows,
        "compact": compact_rows(&visible),
        "summary": summary,
    })
}

/// Project-grouped display rows: each project appears once with a header,
/// most urgent project first, sessions within a project by urgency then
/// freshness, at most [`PER_PROJECT_CAP`] sessions per project. Every
/// session row carries its provider so the renderer can draw the glyph.
fn grouped_rows(classified: &[(&AgentSession, &'static str)], now: i64) -> Vec<serde_json::Value> {
    let mut buckets: BTreeMap<&str, Vec<&(&AgentSession, &'static str)>> = BTreeMap::new();
    for entry in classified {
        buckets
            .entry(entry.0.project.as_str())
            .or_default()
            .push(entry);
    }
    // projects ordered by their most urgent session, freshest activity as
    // the tie-break so active projects float to the top
    let mut projects: Vec<(&str, Vec<&(&AgentSession, &'static str)>)> = buckets.into_iter().collect();
    projects.sort_by_key(|(_, sessions)| {
        (
            sessions.iter().map(|(_, state)| rank(state)).min().unwrap_or(2),
            std::cmp::Reverse(
                sessions
                    .iter()
                    .map(|(session, _)| session.last_ts)
                    .max()
                    .unwrap_or(0),
            ),
        )
    });

    let mut rows = Vec::new();
    for (project, sessions) in projects {
        if rows.len() + 2 > MAX_ROWS {
            break;
        }
        rows.push(serde_json::json!({
            "label": project,
            "state": "header",
        }));
        let mut ordered = sessions;
        ordered.sort_by_key(|(session, state)| (rank(state), std::cmp::Reverse(session.last_ts)));
        for (session, state) in ordered.into_iter().take(PER_PROJECT_CAP) {
            let age = (now - session.last_ts).max(0);
            rows.push(serde_json::json!({
                "label": session.title,
                "value": format!("{} · {}", state_label(state), fmt_age(age)),
                "state": state,
                "provider": session.provider,
            }));
        }
    }
    rows
}

/// Per-provider counts for the compact fallback: one line per active
/// provider in canonical order, done sessions included for context.
fn compact_rows(classified: &[(&AgentSession, &'static str)]) -> Vec<serde_json::Value> {
    let mut rows = Vec::new();
    for provider in PROVIDERS {
        let mut counts = [0u32; 3]; // working, attention, done
        let mut any = false;
        for (session, state) in classified {
            if session.provider != provider {
                continue;
            }
            any = true;
            counts[rank(state) as usize] += 1;
        }
        if !any {
            continue;
        }
        let mut value = String::new();
        for (count, label) in [
            (counts[0], "working"),
            (counts[1], "attention"),
            (counts[2], "done"),
        ] {
            if count > 0 {
                if !value.is_empty() {
                    value.push_str(" · ");
                }
                value.push_str(&format!("{count} {label}"));
            }
        }
        rows.push(serde_json::json!({
            "provider": provider,
            "value": value,
        }));
    }
    rows
}

pub(crate) fn classify(age: i64, config: &Config) -> Option<&'static str> {
    if age <= config.agent_fresh_secs {
        Some("working")
    } else if age <= config.agent_attention_secs {
        Some("attention")
    } else if age <= config.agent_done_secs {
        Some("done")
    } else {
        None
    }
}

fn rank(state: &str) -> u8 {
    match state {
        "working" => 0,
        "attention" => 1,
        _ => 2,
    }
}

fn state_label(state: &str) -> &'static str {
    match state {
        "working" => "working",
        "attention" => "check?",
        _ => "done",
    }
}

fn fmt_age(secs: i64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h", secs / 3600)
    }
}

/// ---- session sources ---------------------------------------------------------

/// ZCode conversations from the SQLite journal, read-only alongside the
/// running CLI (WAL allows that). `parent_id IS NULL` keeps main
/// conversations only - subagent rows share the parent's heartbeat and
/// their prompt-quoting titles would just add noise.
fn zcode_sessions(db_path: &Path, config: &Config, now: i64, out: &mut Vec<AgentSession>) {
    let cutoff_ms = (now - config.agent_done_secs - crate::util::DONE_GRACE_SECS).max(0) * 1000;
    let read: Vec<(String, String, i64)> = journal_query(
        db_path,
        "SELECT title, directory, time_updated FROM session
         WHERE parent_id IS NULL AND time_archived IS NULL AND time_updated > ?1",
        [cutoff_ms],
    );
    for (title, directory, ts_ms) in read {
        out.push(AgentSession {
            provider: "zcode",
            project: if directory.is_empty() {
                "zcode".into()
            } else {
                basename(&directory)
            },
            title: if title.trim().is_empty() {
                "session".into()
            } else {
                truncate(title.trim(), 34)
            },
            last_ts: ts_ms / 1000,
        });
    }
}

/// Shared read pattern for the SQLite journals: (title, directory,
/// time_updated). Rows are collected inside a block so every borrow of
/// the connection drops before the connection itself.
fn journal_query(
    db_path: &Path,
    sql: &str,
    params: impl rusqlite::Params,
) -> Vec<(String, String, i64)> {
    let flags =
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let Ok(conn) = rusqlite::Connection::open_with_flags(db_path, flags) else {
        return Vec::new();
    };
    let read: Vec<(String, String, i64)> = {
        let Ok(mut stmt) = conn.prepare(sql) else {
            return Vec::new();
        };
        let rows = match stmt.query_map(params, |row| {
            Ok((
                row.get::<_, Option<String>>(0)?.unwrap_or_default(),
                row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                row.get::<_, Option<i64>>(2)?.unwrap_or_default(),
            ))
        }) {
            Ok(rows) => rows.flatten().collect::<Vec<_>>(),
            Err(_) => return Vec::new(),
        };
        rows
    };
    read
}

/// Fallback when the journal is unreadable: rollout file mtimes only,
/// labeled by the id fragment. No titles, project "zcode".
fn rollout_fallback(rollout: &Path, out: &mut Vec<AgentSession>) {
    let Ok(entries) = std::fs::read_dir(rollout) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let Some(rest) = name.strip_prefix("model-io-sess_") else {
            continue;
        };
        let Some(id) = rest.strip_suffix(".jsonl") else {
            continue;
        };
        let Some(last_ts) = mtime(&path) else {
            continue;
        };
        let short: String = id.chars().take(4).collect();
        out.push(AgentSession {
            provider: "zcode",
            project: "zcode".into(),
            title: format!("Zcode {short}"),
            last_ts,
        });
    }
}

/// Claude Code sessions: `~/.claude/projects/<project>/<session>.jsonl`.
/// The transcript head names the session: its user records carry the real
/// `cwd` (project basename) and the opening prompt (title). The munged
/// directory name and the session id fragment stay as fallbacks.
fn claude_sessions(root: &Path, out: &mut Vec<AgentSession>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let project_dir = entry.path();
        if !project_dir.is_dir() {
            continue;
        }
        let Some((last_ts, transcript)) = freshest_transcript(&project_dir) else {
            continue;
        };
        let (cwd_project, prompt) = transcript_head_info(&transcript);
        // "f--projects-deckboard-clone" -> "projects-deckboard-clone"
        let raw = project_dir.file_name().unwrap_or_default().to_string_lossy();
        let cleaned = raw
            .trim_start_matches(|c: char| c.is_ascii_alphabetic())
            .trim_start_matches('-');
        let stem = transcript
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let short: String = stem
            .chars()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        out.push(AgentSession {
            provider: "claude",
            project: cwd_project.unwrap_or_else(|| truncate(cleaned, 24)),
            title: prompt.unwrap_or_else(|| format!("claude {short}")),
            last_ts,
        });
    }
}

/// Freshest `.jsonl` transcript in a Claude project dir, with its mtime.
fn freshest_transcript(project_dir: &Path) -> Option<(i64, PathBuf)> {
    let mut best: Option<(i64, PathBuf)> = None;
    let entries = std::fs::read_dir(project_dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        if let Some(ts) = mtime(&path) {
            if best.as_ref().map_or(true, |(b, _)| ts > *b) {
                best = Some((ts, path));
            }
        }
    }
    best
}

/// Project and title from the transcript head: the first user record
/// carries the session's `cwd` (project basename) and its text content is
/// the opening prompt. Bounded read; SDK wrappers and torn lines yield
/// None parts, handled by the caller's fallbacks.
fn transcript_head_info(path: &Path) -> (Option<String>, Option<String>) {
    let Ok(mut file) = std::fs::File::open(path) else {
        return (None, None);
    };
    let mut head = vec![0u8; 256 * 1024];
    let read = file.read(&mut head).unwrap_or(0);
    let text = String::from_utf8_lossy(&head[..read]);
    let mut cwd = None;
    let mut prompt = None;
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if cwd.is_none() {
            cwd = v
                .get("cwd")
                .and_then(|c| c.as_str())
                .map(basename)
                .filter(|s| !s.is_empty());
        }
        if prompt.is_none() && v.get("type").and_then(|t| t.as_str()) == Some("user") {
            let content = v.pointer("/message/content");
            let text = match content {
                Some(serde_json::Value::String(s)) => Some(s.clone()),
                Some(serde_json::Value::Array(items)) => items.iter().find_map(|item| {
                    let t = item.get("text")?.as_str()?;
                    (item.get("type")?.as_str()? == "text").then(|| t.to_string())
                }),
                _ => None,
            };
            // leading markup is tooling, not a human prompt
            prompt = text
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty() && !s.starts_with('<'));
        }
        if cwd.is_some() && prompt.is_some() {
            break;
        }
    }
    (cwd, prompt)
}

/// ---- helpers -----------------------------------------------------------------

fn basename(path: &str) -> String {
    path.rsplit(['\\', '/'])
        .find(|s| !s.is_empty())
        .unwrap_or(path)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_usage::fmt_tokens;

    fn config() -> Config {
        Config {
            agent_fresh_secs: 180,
            agent_attention_secs: 900,
            agent_done_secs: 14_400,
            ..Config::default()
        }
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// Rows are (marker, title, directory, ts); marker "sub" parents the
    /// row off the first session, which the reader must exclude.
    fn seed_journal(db_path: &Path, rows: &[(&str, &str, &str, i64)]) {
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let conn = rusqlite::Connection::open(db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (id TEXT, parent_id TEXT, title TEXT,
             directory TEXT, time_updated INTEGER, time_archived INTEGER);",
        )
        .unwrap();
        for (i, (marker, title, directory, ts)) in rows.iter().enumerate() {
            let id = format!("s{i}");
            let parent: Option<&str> = if *marker == "sub" { Some("s0") } else { None };
            conn.execute(
                "INSERT INTO session VALUES (?1, ?2, ?3, ?4, ?5, NULL)",
                rusqlite::params![id, parent, title, directory, ts * 1000],
            )
            .unwrap();
        }
    }

    #[test]
    fn classify_by_age_thresholds() {
        let cfg = config();
        assert_eq!(classify(0, &cfg), Some("working"));
        assert_eq!(classify(180, &cfg), Some("working"));
        assert_eq!(classify(181, &cfg), Some("attention"));
        assert_eq!(classify(900, &cfg), Some("attention"));
        assert_eq!(classify(901, &cfg), Some("done"));
        assert_eq!(classify(14_400, &cfg), Some("done"));
        assert_eq!(classify(14_401, &cfg), None);
    }

    #[test]
    fn snapshot_without_sources_is_idle() {
        let cfg = config();
        let paths = Paths {
            config: "unused".into(),
            zcode_cli: std::env::temp_dir().join("aidev-test-nope-z"),
            claude_projects: std::env::temp_dir().join("aidev-test-nope-c"),
            codex_sessions: std::env::temp_dir().join("aidev-test-nope-x"),
            opencode_db: std::env::temp_dir().join("aidev-test-nope-o").join("db.sqlite"),
            antigravity_conversations: std::env::temp_dir().join("aidev-test-nope-a"),
        };
        let v = snapshot(&cfg, &paths, 1_800_000_000);
        assert_eq!(v["summary"], "idle");
        assert_eq!(v["rows"][0]["state"], "off");
        assert!(v["compact"].as_array().unwrap().is_empty());
    }

    #[test]
    fn zcode_sessions_come_from_the_journal() {
        let dir = scratch("aidev-agents");
        let db_path = dir.join("db").join("db.sqlite");
        let now = crate::unix_now();
        seed_journal(
            &db_path,
            &[
                ("s1", "Fix the grid", "F:\\projects\\deckboard clone", now - 120),
                ("sub", "You are a reviewer", "F:\\x", now - 30),
                ("s3", "Ancient", "F:\\y", now - 90_000),
            ],
        );

        let mut out = Vec::new();
        zcode_sessions(&db_path, &config(), now, &mut out);
        assert_eq!(out.len(), 1, "subagents and stale rows excluded");
        assert_eq!(out[0].provider, "zcode");
        assert_eq!(out[0].title, "Fix the grid");
        assert_eq!(out[0].project, "deckboard clone");
        assert!((out[0].last_ts - (now - 120)).abs() <= 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rows_are_grouped_by_project_with_provider_rows() {
        let dir = scratch("aidev-groups");
        let db_path = dir.join("db").join("db.sqlite");
        let now = crate::unix_now();
        seed_journal(
            &db_path,
            &[
                ("s1", "Fix the grid", "F:\\projects\\deckboard clone", now - 120),
                ("s3", "Rebrand README", "F:\\projects\\deckboard clone", now - 1_620),
                ("s2", "Deep dive", "F:\\projects\\AIJobSearchDLL", now - 3_600),
            ],
        );

        let v = snapshot(
            &config(),
            &Paths {
                config: "unused".into(),
                zcode_cli: dir.clone(),
                claude_projects: dir.join("claude"),
                codex_sessions: dir.join("codex"),
                opencode_db: dir.join("nope").join("db.sqlite"),
                antigravity_conversations: dir.join("antigravity"),
            },
            now,
        );
        let rows = v["rows"].as_array().unwrap();
        // the project with mixed states still gets exactly one header, its
        // working session above the done one
        let headers: Vec<_> = rows
            .iter()
            .filter(|r| r["state"] == "header")
            .map(|r| r["label"].as_str().unwrap())
            .collect();
        assert_eq!(headers, ["deckboard clone", "AIJobSearchDLL"]);
        assert_eq!(rows[0]["state"], "header");
        assert_eq!(rows[1]["label"], "Fix the grid");
        assert_eq!(rows[1]["provider"], "zcode");
        assert!(rows[1]["value"].as_str().unwrap().starts_with("working · "));
        assert_eq!(rows[2]["label"], "Rebrand README");
        assert!(rows[2]["value"].as_str().unwrap().starts_with("done · "));
        assert_eq!(rows[3]["state"], "header");
        assert_eq!(rows[3]["label"], "AIJobSearchDLL");
        assert!(rows[4]["value"].as_str().unwrap().starts_with("done · "));
        assert_eq!(v["summary"], "1 working · 2 done");
        // compact counts mirror the visible sessions per provider
        assert_eq!(v["compact"][0]["provider"], "zcode");
        assert_eq!(v["compact"][0]["value"], "1 working · 2 done");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn done_only_providers_stay_hidden() {
        let dir = scratch("aidev-hidden");
        let db_path = dir.join("db").join("db.sqlite");
        let now = crate::unix_now();
        seed_journal(
            &db_path,
            &[
                ("s1", "Old work", "F:\\projects\\deckboard clone", now - 3_600),
                ("s2", "Other old", "F:\\projects\\AIJobSearchDLL", now - 7_200),
            ],
        );

        let v = snapshot(
            &config(),
            &Paths {
                config: "unused".into(),
                zcode_cli: dir.clone(),
                claude_projects: dir.join("claude"),
                codex_sessions: dir.join("codex"),
                opencode_db: dir.join("nope").join("db.sqlite"),
                antigravity_conversations: dir.join("antigravity"),
            },
            now,
        );
        // every session is done: the provider is hidden, tile is idle
        assert_eq!(v["summary"], "idle");
        assert_eq!(v["rows"][0]["state"], "off");
        assert!(v["compact"].as_array().unwrap().is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ages_and_names_format() {
        assert_eq!(fmt_age(45), "45s");
        assert_eq!(fmt_age(300), "5m");
        assert_eq!(fmt_age(7_200), "2h");
        assert_eq!(truncate("abcdefghij", 5), "abcd…");
        assert_eq!(basename(r"F:\projects\deckboard clone"), "deckboard clone");
        assert_eq!(basename(r"F:\projects\deckboard clone\"), "deckboard clone");
        assert_eq!(fmt_tokens(1_500), "1.5K");
    }

    #[test]
    fn claude_head_names_the_session() {
        let dir = scratch("aidev-claude-head");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("transcript.jsonl");
        std::fs::write(
            &f,
            concat!(
                r#"{"type":"queue-operation","operation":"enqueue","sessionId":"t"}"#,
                "\n",
                r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"Fix the login bug"}]},"cwd":"F:\\projects\\demo app"}"#,
                "\n",
                r#"{"type":"assistant","message":{"role":"assistant","content":[]}}"#,
                "\n"
            ),
        )
        .unwrap();

        let (project, prompt) = transcript_head_info(&f);
        assert_eq!(project.as_deref(), Some("demo app"));
        assert_eq!(prompt.as_deref(), Some("Fix the login bug"));

        // SDK wrapper content and tool markup are not prompts; the munged
        // dir name covers a missing cwd
        let f2 = dir.join("sdk.jsonl");
        std::fs::write(
            &f2,
            concat!(
                r#"{"type":"user","message":{"role":"user","content":"<command-name>/clear</command-name>"},"cwd":""}"#,
                "\n",
                r#"{"type":"user","message":{"role":"user","content":"real prompt"},"cwd":"F:\\projects\\demo app"}"#,
                "\n"
            ),
        )
        .unwrap();
        let (project, prompt) = transcript_head_info(&f2);
        assert_eq!(project.as_deref(), Some("demo app"));
        assert_eq!(prompt.as_deref(), Some("real prompt"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
