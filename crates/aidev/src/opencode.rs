//! OpenCode as a data source (`~/.local/share/opencode/opencode.db`).
//!
//! One SQLite database, two reads:
//! - *agent sessions*: the `session` table carries `title`, `directory`
//!   and `time_updated` per conversation - the same shape as the ZCode
//!   journal, read with the same freshness classification.
//! - *token usage*: the `message` table stores one row per message whose
//!   `data` column is the assistant-message JSON (`tokens` object with
//!   input/output/reasoning and `cache.read/write`). Rows are immutable,
//!   so a high-water mark over `time_created` keeps every poll incremental.
//!
//! The database can be large (gigabytes); every query is time-bounded and
//! touches only the small `session` table or a recent slice of `message`.

use std::collections::HashSet;
use std::path::Path;

use crate::agents::AgentSession;
use crate::local_usage::Sample;
use crate::util::truncate;

/// Recent agent conversations, same classification input as ZCode's.
pub fn sessions(db_path: &Path, config: &crate::Config, now: i64, out: &mut Vec<AgentSession>) {
    let cutoff_ms = (now - config.agent_done_secs - crate::util::DONE_GRACE_SECS).max(0) * 1000;
    let read: Vec<(String, String, i64)> = query(
        db_path,
        "SELECT title, directory, time_updated FROM session WHERE time_updated > ?1",
        [cutoff_ms],
        |row| {
            Ok((
                row.get::<_, Option<String>>(0)?.unwrap_or_default(),
                row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                row.get::<_, Option<i64>>(2)?.unwrap_or_default(),
            ))
        },
    );
    for (title, directory, ts_ms) in read {
        out.push(AgentSession {
            provider: "opencode",
            project: if directory.is_empty() {
                "opencode".into()
            } else {
                directory
                    .rsplit(['\\', '/'])
                    .find(|s| !s.is_empty())
                    .unwrap_or(&directory)
                    .to_string()
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

fn query<T, P, F>(db_path: &Path, sql: &str, params: P, map: F) -> Vec<T>
where
    P: rusqlite::Params,
    F: FnMut(&rusqlite::Row) -> rusqlite::Result<T>,
{
    let flags =
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let Ok(conn) = rusqlite::Connection::open_with_flags(db_path, flags) else {
        return Vec::new();
    };
    // rows are collected inside the block so every borrow of the
    // connection is dropped before the connection itself
    let read: Vec<T> = {
        let Ok(mut stmt) = conn.prepare(sql) else {
            return Vec::new();
        };
        let rows = match stmt.query_map(params, map) {
            Ok(rows) => rows.flatten().collect::<Vec<T>>(),
            Err(_) => return Vec::new(),
        };
        rows
    };
    read
}

// ---- token usage ------------------------------------------------------------

/// Incremental usage reader: message rows are immutable, so a high-water
/// mark over `time_created` feeds only new rows into the sample list.
pub struct Usage {
    samples: Vec<Sample>,
    seen_ids: HashSet<String>,
    watermark_ms: i64,
    history_secs: i64,
}

impl Usage {
    pub fn new(history_days: i64) -> Self {
        Self {
            samples: Vec::new(),
            seen_ids: HashSet::new(),
            watermark_ms: 0,
            history_secs: history_days.max(1) * 86_400,
        }
    }

    pub fn scan(&mut self, db_path: &Path, now: i64) {
        let cutoff_ms = (now - self.history_secs).max(0) * 1000;
        let from_ms = self.watermark_ms.max(cutoff_ms);
        let read: Vec<(String, i64, String)> = query(
            db_path,
            "SELECT id, time_created, data FROM message WHERE time_created > ?1 ORDER BY time_created",
            [from_ms],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?.unwrap_or_default(),
                    row.get::<_, Option<i64>>(1)?.unwrap_or_default(),
                    row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                ))
            },
        );
        for (id, ts_ms, data) in read {
            if ts_ms > self.watermark_ms {
                self.watermark_ms = ts_ms;
            }
            if !self.seen_ids.insert(id) {
                continue;
            }
            if let Some(sample) = parse_message(&data, ts_ms) {
                self.samples.push(sample);
            }
        }
        self.prune(now);
    }

    fn prune(&mut self, now: i64) {
        let keep_from = (now - self.history_secs).max(0);
        self.samples.retain(|s| s.ts >= keep_from);
        if self.seen_ids.len() > 50_000 {
            self.seen_ids.clear(); // coarse cap: worst case a few re-counts
        }
    }

    pub fn sums(
        &self,
        now: i64,
        boundary: crate::local_usage::DayBoundary,
    ) -> crate::local_usage::Sums {
        let mut sums = crate::local_usage::Sums::default();
        let today_start = boundary.day_start(now);
        let week_start = boundary.week_start(now);
        for s in &self.samples {
            if s.ts > now {
                continue;
            }
            if s.ts >= now - 3600 {
                sums.hour += s.tokens;
            }
            if s.ts >= now - 5 * 3600 {
                sums.five_hour += s.tokens;
            }
            if s.ts >= today_start {
                sums.today += s.tokens;
            }
            if s.ts >= week_start {
                sums.week += s.tokens;
            }
        }
        sums
    }
}

/// Assistant messages carry the usage: `tokens.input/output/reasoning` and
/// `tokens.cache.read/write`, with `time.created` in epoch milliseconds.
fn parse_message(data: &str, ts_ms: i64) -> Option<Sample> {
    let v: serde_json::Value = serde_json::from_str(data).ok()?;
    if v.get("role")?.as_str()? != "assistant" {
        return None;
    }
    let tokens = v.get("tokens")?;
    let pick = |key: &str| tokens.get(key).and_then(|x| x.as_u64());
    let cache = tokens.get("cache");
    let cache_read = cache
        .and_then(|c| c.get("read"))
        .and_then(|x| x.as_u64())
        .unwrap_or(0);
    let cache_write = cache
        .and_then(|c| c.get("write"))
        .and_then(|x| x.as_u64())
        .unwrap_or(0);
    let total = pick("input")? + pick("output")? + cache_read + cache_write;
    // reasoning tokens are part of the output on counting models
    let _ = pick("reasoning");
    Some(Sample {
        ts: ts_ms / 1000,
        tokens: total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_db(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("aidev-opencode-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("opencode.db")
    }

    fn message_json(id: &str, input: u64, cache_read: u64) -> String {
        format!(
            r#"{{"id":"{id}","sessionID":"ses_1","role":"assistant","time":{{"created":0}},"modelID":"glm-5","providerID":"zai-coding-plan","tokens":{{"total":{total},"input":{input},"output":5,"reasoning":0,"cache":{{"read":{cache_read},"write":0}}}}}}"#,
            total = input + 5 + cache_read
        )
    }

    #[test]
    fn sessions_come_from_the_session_table() {
        let db = tmp_db("sess");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (id TEXT, title TEXT, directory TEXT, time_updated INTEGER);",
        )
        .unwrap();
        let now = crate::unix_now();
        conn.execute(
            "INSERT INTO session VALUES ('ses_1', 'Refactor the parser', 'F:\\projects\\demo', ?1)",
            [(now - 60) * 1000],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session VALUES ('ses_2', 'Old one', 'F:\\other', ?1)",
            [(now - 90_000) * 1000],
        )
        .unwrap();

        let mut out = Vec::new();
        sessions(&db, &crate::Config::default(), now, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].provider, "opencode");
        assert_eq!(out[0].title, "Refactor the parser");
        assert_eq!(out[0].project, "demo");

        let _ = std::fs::remove_dir_all(db.parent().unwrap());
    }

    #[test]
    fn usage_is_incremental_and_windowed() {
        let db = tmp_db("usage");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE message (id TEXT PRIMARY KEY, time_created INTEGER, data TEXT);",
        )
        .unwrap();
        let now = crate::unix_now();
        for (i, (age, input, cache)) in [(30i64, 100u64, 500u64), (3700, 200, 900)]
            .iter()
            .enumerate()
        {
            conn.execute(
                "INSERT INTO message VALUES (?1, ?2, ?3)",
                rusqlite::params![
                    format!("msg_{i}"),
                    (now - age) * 1000,
                    message_json(&format!("msg_{i}"), *input, *cache)
                ],
            )
            .unwrap();
        }

        let mut usage = Usage::new(8);
        usage.scan(&db, now);
        let s = usage.sums(now, crate::local_usage::DayBoundary::Utc);
        assert_eq!(s.hour, 100 + 5 + 500);
        assert_eq!(s.five_hour, 100 + 5 + 500 + 200 + 5 + 900);

        // a new row after the watermark is picked up, old ones not re-counted
        conn.execute(
            "INSERT INTO message VALUES ('msg_new', ?1, ?2)",
            rusqlite::params![(now - 10) * 1000, message_json("msg_new", 70, 0)],
        )
        .unwrap();
        usage.scan(&db, now);
        assert_eq!(
            usage.sums(now, crate::local_usage::DayBoundary::Utc).hour,
            100 + 5 + 500 + 70 + 5
        );

        let _ = std::fs::remove_dir_all(db.parent().unwrap());
    }
}
