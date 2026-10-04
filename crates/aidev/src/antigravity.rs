//! Google Antigravity as a data source (`~/.gemini/antigravity`).
//!
//! Agent trajectories live in one SQLite database per conversation
//! (`conversations/<uuid>.db`, table `gen_metadata`) whose `data` blobs
//! are protobuf messages. CodexBar's reverse-engineered layout reads the
//! top-level varint fields of each blob as token counters: fields 1 and 2
//! are input, 5 is cache read, 9 is text output, 10 is thinking output.
//! Blobs are immutable, so a per-database high-water mark over `idx`
//! keeps polling incremental; their sample time comes from the `steps`
//! row at the same idx (a protobuf Timestamp), not from discovery.
//!
//! Plan quota is not stored on disk: the running IDE exposes it on a
//! local Connect-RPC endpoint (`RetrieveUserQuotaSummary`), discovered by
//! probing the loopback ports Antigravity listens on. No IDE, no quota.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::agents::AgentSession;
use crate::local_usage::Sample;
use crate::util::mtime;

/// Recent conversations; activity is the database file mtime, the title
/// is the conversation uuid fragment. Protobuf carries no cwd, so the
/// project lane is fixed.
pub fn sessions(
    conversations_dir: &Path,
    config: &crate::Config,
    now: i64,
    out: &mut Vec<AgentSession>,
) {
    let cutoff = now - config.agent_done_secs - crate::util::DONE_GRACE_SECS;
    let Ok(entries) = std::fs::read_dir(conversations_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("db") {
            continue;
        }
        let Some(last_ts) = mtime(&path) else {
            continue;
        };
        if last_ts < cutoff {
            continue;
        }
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let frag: String = stem
            .chars()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        out.push(AgentSession {
            provider: "antigravity",
            project: "antigravity".into(),
            title: format!("antigravity {frag}"),
            last_ts,
        });
    }
}

// ---- token usage ------------------------------------------------------------

/// Incremental reader over all conversation databases.
pub struct Usage {
    samples: Vec<Sample>,
    // per database: the highest gen_metadata idx already parsed
    watermark: HashMap<PathBuf, i64>,
    history_secs: i64,
}

impl Usage {
    pub fn new(history_days: i64) -> Self {
        Self {
            samples: Vec::new(),
            watermark: HashMap::new(),
            history_secs: history_days.max(1) * 86_400,
        }
    }

    pub fn scan(&mut self, conversations_dir: &Path, now: i64) {
        let Ok(entries) = std::fs::read_dir(conversations_dir) else {
            return;
        };
        let mut live: HashSet<PathBuf> = HashSet::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("db") {
                continue;
            }
            self.scan_db(&path, now);
            live.insert(path);
        }
        // databases the IDE deleted must not pin their watermark entries
        // forever (one PathBuf per conversation adds up over weeks);
        // dropping an entry only re-reads that db once
        self.watermark.retain(|p, _| live.contains(p));
        let keep_from = (now - self.history_secs).max(0);
        self.samples.retain(|s| s.ts >= keep_from);
    }

    fn scan_db(&mut self, db_path: &Path, now: i64) {
        let from_idx = *self.watermark.get(db_path).unwrap_or(&-1);
        let read: Vec<(i64, Vec<u8>, Option<Vec<u8>>)> = query_blobs(db_path, from_idx);
        let mut max_idx = from_idx;
        for (idx, blob, step_meta) in read {
            max_idx = max_idx.max(idx);
            if let Some(tokens) = count_blob(&blob) {
                if tokens > 0 {
                    // the gen_metadata blob carries no time; the matching
                    // steps row does (protobuf Timestamp), discovery time
                    // is the last-resort fallback
                    let ts = step_meta.as_deref().and_then(step_timestamp).unwrap_or(now);
                    self.samples.push(Sample { ts, tokens });
                }
            }
        }
        self.watermark.insert(db_path.to_path_buf(), max_idx);
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

fn query_blobs(db_path: &Path, from_idx: i64) -> Vec<(i64, Vec<u8>, Option<Vec<u8>>)> {
    let flags =
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let Ok(conn) = rusqlite::Connection::open_with_flags(db_path, flags) else {
        return Vec::new();
    };
    // every gen_metadata row has a steps row at the same idx whose
    // `metadata` blob holds the step's protobuf Timestamp
    let read: Vec<(i64, Vec<u8>, Option<Vec<u8>>)> = {
        let Ok(mut stmt) = conn.prepare(
            "SELECT g.idx, g.data, s.metadata \
             FROM gen_metadata g LEFT JOIN steps s ON s.idx = g.idx \
             WHERE g.idx > ?1 ORDER BY g.idx",
        ) else {
            return Vec::new();
        };
        let rows = match stmt.query_map([from_idx], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        }) {
            Ok(rows) => rows.flatten().collect::<Vec<_>>(),
            Err(_) => return Vec::new(),
        };
        rows
    };
    read
}

/// Epoch seconds from a steps `metadata` blob: field 1 wraps a protobuf
/// Timestamp whose field 1 is the seconds varint. Anything else -> None.
fn step_timestamp(meta: &[u8]) -> Option<i64> {
    // locate top-level field 1 (length-delimited): key byte, len, payload
    let mut wrapped = None;
    let mut i = 0usize;
    while i < meta.len() {
        let (key, consumed) = read_varint(&meta[i..])?;
        i += consumed;
        let wire = (key & 7) as u8;
        if wire == 0 {
            let (_, consumed) = read_varint(&meta[i..])?;
            i += consumed;
        } else if wire == 1 {
            i += 8;
        } else if wire == 5 {
            i += 4;
        } else if wire == 2 {
            let (len, consumed) = read_varint(&meta[i..])?;
            // untrusted length: checked advance only, a wrap could loop
            let next = i
                .checked_add(consumed)?
                .checked_add(usize::try_from(len).ok()?)?;
            if next > meta.len() {
                return None;
            }
            if ((key >> 3) & 0xFFFFF) as u32 == 1 && wrapped.is_none() {
                wrapped = Some(&meta[i + consumed..next]);
            }
            i = next;
        } else {
            return None;
        }
    }
    let ts = wrapped?;
    let (key, consumed) = read_varint(ts)?;
    if (key & 7) != 0 || ((key >> 3) & 0xFFFFF) as u32 != 1 {
        return None;
    }
    let (seconds, _) = read_varint(&ts[consumed..])?;
    i64::try_from(seconds).ok()
}

/// Sum the documented token fields of one protobuf blob (top-level
/// varints only: 1 + 2 = input, 5 = cache read, 9 + 10 = output).
pub(crate) fn count_blob(blob: &[u8]) -> Option<u64> {
    let mut f1 = 0u64;
    let mut f2 = 0u64;
    let mut f5 = 0u64;
    let mut f9 = 0u64;
    let mut f10 = 0u64;
    let mut seen = false;
    walk_top_level(blob, &mut |field, value| match field {
        1 => {
            f1 += value;
            seen = true;
        }
        2 => {
            f2 += value;
            seen = true;
        }
        5 => {
            f5 += value;
            seen = true;
        }
        9 => {
            f9 += value;
            seen = true;
        }
        10 => {
            f10 += value;
            seen = true;
        }
        _ => {}
    });
    seen.then_some(f1 + f2 + f5 + f9 + f10)
}

/// Walk top-level protobuf fields: varints visit the callback, other wire
/// types are skipped by their fixed size. Malformed bytes end the walk.
fn walk_top_level(buf: &[u8], visit: &mut impl FnMut(u32, u64)) {
    let mut i = 0usize;
    while i < buf.len() {
        // read_varint counts bytes consumed from the slice start, so every
        // advance here is relative and adds to i
        let Some((key, consumed)) = read_varint(&buf[i..]) else {
            return;
        };
        i += consumed;
        let wire = (key & 7) as u8;
        match wire {
            0 => {
                let Some((value, consumed)) = read_varint(&buf[i..]) else {
                    return;
                };
                i += consumed;
                visit(((key >> 3) & 0xFFFFF) as u32, value);
            }
            1 => i += 8,
            2 => {
                let Some((len, consumed)) = read_varint(&buf[i..]) else {
                    return;
                };
                // untrusted length: checked advance, never overflow or
                // wrap (a wrap could loop forever)
                let Some(next) = i
                    .checked_add(consumed)
                    .and_then(|i| i.checked_add(usize::try_from(len).ok()?))
                else {
                    return;
                };
                if next > buf.len() {
                    return;
                }
                i = next;
            }
            5 => i += 4,
            _ => return,
        }
    }
}

fn read_varint(buf: &[u8]) -> Option<(u64, usize)> {
    let mut value = 0u64;
    for (i, byte) in buf.iter().take(10).enumerate() {
        value |= ((byte & 0x7F) as u64) << (7 * i);
        if byte & 0x80 == 0 {
            return Some((value, i + 1));
        }
    }
    None
}

/// ---- plan quota via the running IDE ------------------------------------------

#[derive(Default, Clone, Debug)]
pub struct Quota {
    pub five_hour_used: Option<f64>,
    pub weekly_used: Option<f64>,
}

/// Ask the running IDE for its quota summary. Any failure (IDE closed,
/// port moved, protocol drift) yields `None` - the tile simply shows no
/// Antigravity lane.
pub fn quota() -> Option<Quota> {
    for port in ide_ports() {
        if let Some(quota) = ask_port(port) {
            return Some(quota);
        }
    }
    None
}

/// Console probes run on every producer tick; without CREATE_NO_WINDOW
/// each tasklist/netstat pops a visible cmd window on the desktop (see
/// the matching flag in pulpit_ext's shell_command).
#[cfg(windows)]
fn quiet(mut c: std::process::Command) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    c
}

#[cfg(not(windows))]
fn quiet(c: std::process::Command) -> std::process::Command {
    c
}

fn ide_ports() -> Vec<u16> {
    let pids = antigravity_pids();
    if pids.is_empty() {
        return Vec::new();
    }
    let output = quiet(std::process::Command::new("netstat"))
        .args(["-ano"])
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut ports = Vec::new();
    for line in text.lines() {
        // TCP    127.0.0.1:4123    0.0.0.0:0    LISTENING    12345
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() >= 5 && fields[3].eq_ignore_ascii_case("LISTENING") {
            if let Ok(pid) = fields[4].parse::<u32>() {
                if pids.contains(&pid) {
                    if let Some(port) = fields[1].rsplit(':').next().and_then(|p| p.parse().ok()) {
                        if !ports.contains(&port) {
                            ports.push(port);
                        }
                    }
                }
            }
        }
    }
    ports
}

#[cfg(windows)]
fn antigravity_pids() -> Vec<u32> {
    let output = quiet(std::process::Command::new("tasklist"))
        .args(["/FO", "CSV", "/NH"])
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut pids = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split("\",\"").collect();
        if fields.len() >= 2 {
            let exe = fields[0].trim_start_matches('"').to_ascii_lowercase();
            if exe.starts_with("antigravity") {
                if let Ok(pid) = fields[1].trim_end_matches('"').parse() {
                    pids.push(pid);
                }
            }
        }
    }
    pids
}

#[cfg(not(windows))]
fn antigravity_pids() -> Vec<u32> {
    Vec::new()
}

fn ask_port(port: u16) -> Option<Quota> {
    let agent = crate::limits::http_agent();
    let url = format!(
        "http://127.0.0.1:{port}/exa.language_server_pb.LanguageServerService/RetrieveUserQuotaSummary"
    );
    let mut req = agent.post(url);
    req = req.header("content-type", "application/json");
    let resp = req.send("{}").ok()?;
    // the quota payload is tiny; any loopback port that streams more than
    // the cap (ide_ports probes every LISTENING port of the IDE's PIDs)
    // must not stream into the tray process unbounded
    let body = resp
        .into_body()
        .into_with_config()
        .limit(crate::limits::MAX_JSON_BODY_BYTES)
        .lossy_utf8(true)
        .read_to_string()
        .ok()?;
    let v: serde_json::Value = serde_json::from_str(&body).ok()?;
    parse_quota(&v)
}

/// `groups[].buckets[]` with `remaining.remainingFraction`; the window is
/// named somewhere in the bucket (limit type, id or window key).
fn parse_quota(v: &serde_json::Value) -> Option<Quota> {
    let mut quota = Quota::default();
    let empty: [serde_json::Value; 0] = [];
    for group in v.get("groups")?.as_array()? {
        for bucket in group
            .get("buckets")
            .and_then(|b| b.as_array())
            .map(|b| b.as_slice())
            .unwrap_or(&empty)
        {
            let fraction = bucket
                .pointer("/remaining/remainingFraction")
                .and_then(|x| x.as_f64());
            let Some(fraction) = fraction else { continue };
            let used = ((1.0 - fraction).clamp(0.0, 1.0)) * 100.0;
            let naming = bucket_to_string(bucket);
            let lower = naming.to_ascii_lowercase();
            if lower.contains("week") {
                quota.weekly_used = Some(used);
            } else if lower.contains("five") || lower.contains("session") {
                quota.five_hour_used = Some(used);
            }
        }
    }
    (quota.five_hour_used.is_some() || quota.weekly_used.is_some()).then_some(quota)
}

/// Concatenate every string in the bucket (depth-first) so the window
/// name can be matched wherever the schema keeps it.
fn bucket_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(items) => items.iter().map(bucket_to_string).collect(),
        serde_json::Value::Object(map) => map.values().map(bucket_to_string).collect(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn varint(value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        let mut v = value;
        loop {
            let byte = (v & 0x7F) as u8;
            v >>= 7;
            if v == 0 {
                out.push(byte);
                break;
            }
            out.push(byte | 0x80);
        }
        out
    }

    fn field(num: u32, value: u64) -> Vec<u8> {
        let mut out = varint((num << 3) as u64); // key: field << 3 | wire type 0 (varint)
        out.extend(varint(value));
        out
    }

    #[test]
    fn blob_fields_sum_to_tokens() {
        // fields: 1=100 (input part a), 2=200 (input part b), 5=4000
        // (cache read), 9=300 (text output), 10=50 (thinking)
        let mut blob = Vec::new();
        blob.extend(field(1, 100));
        blob.extend(field(2, 200));
        blob.extend(field(5, 4000));
        blob.extend(field(9, 300));
        blob.extend(field(10, 50));
        blob.extend(field(11, 999)); // undocumented field: ignored
        assert_eq!(count_blob(&blob), Some(100 + 200 + 4000 + 300 + 50));
    }

    #[test]
    fn garbage_is_rejected_safely() {
        assert_eq!(count_blob(&[]), None);
        // a length-delimited field announcing more bytes than exist ends
        // the walk without panicking
        let mut blob = varint((2 << 3) | 2);
        blob.extend(varint(999));
        blob.extend_from_slice(&[0xFF; 4]);
        assert_eq!(count_blob(&blob), None);
    }

    #[test]
    fn hostile_varint_lengths_end_the_walk() {
        // a hostile length varint (u64::MAX) must end the walk through
        // checked arithmetic, not overflow-panic or wrap into a loop
        let mut blob = varint((2 << 3) | 2);
        blob.extend(varint(u64::MAX));
        blob.extend_from_slice(&[0xFF; 8]);
        assert_eq!(count_blob(&blob), None);
        assert_eq!(step_timestamp(&blob), None);

        // same hostility inside the steps metadata walk
        let mut meta = varint((1 << 3) | 2);
        meta.extend(varint(u64::MAX));
        meta.extend_from_slice(&[0xFF; 8]);
        assert_eq!(step_timestamp(&meta), None);
    }

    #[test]
    fn step_metadata_yields_epoch_seconds() {
        // steps.metadata: field 1 wraps a Timestamp whose field 1 is
        // seconds, field 2 nanoseconds
        let mut inner = field(1, 1_790_402_011);
        inner.extend(field(2, 999_999_999));
        let mut meta = varint((1 << 3) | 2);
        meta.extend(varint(inner.len() as u64));
        meta.extend(inner);
        assert_eq!(step_timestamp(&meta), Some(1_790_402_011));

        // no wrapper, wrong field number, truncated payload: all None
        assert_eq!(step_timestamp(&field(2, 7)), None);
        assert_eq!(step_timestamp(&[]), None);
        let mut short = varint((1 << 3) | 2);
        short.extend(varint(999));
        short.extend_from_slice(&[0x08]);
        assert_eq!(step_timestamp(&short), None);
    }

    #[test]
    fn watermark_forgets_deleted_databases() {
        // one watermark entry per conversation db must not outlive the db:
        // the IDE garbage-collects old conversations and a weeks-old tray
        // process would otherwise accumulate thousands of PathBuf keys
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.db");
        let b = dir.path().join("b.db");
        std::fs::write(&a, b"").unwrap();
        std::fs::write(&b, b"").unwrap();
        let mut usage = Usage::new(7);
        let now = 1_800_000_000;
        usage.scan(dir.path(), now);
        assert_eq!(usage.watermark.len(), 2);
        std::fs::remove_file(&b).unwrap();
        usage.scan(dir.path(), now);
        assert_eq!(usage.watermark.len(), 1);
        assert!(usage.watermark.contains_key(&a));
    }

    #[test]
    fn quota_reads_fractions_from_buckets() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"groups":[{"displayName":"Gemini","buckets":[
                {"id":"five_hour","remaining":{"remainingFraction":0.7}},
                {"limitType":"WEEKLY","remaining":{"remainingFraction":0.25}}]}]}"#,
        )
        .unwrap();
        let quota = parse_quota(&v).unwrap();
        assert!((quota.five_hour_used.unwrap() - 30.0).abs() < 1e-9);
        assert!((quota.weekly_used.unwrap() - 75.0).abs() < 1e-9);
    }
}
