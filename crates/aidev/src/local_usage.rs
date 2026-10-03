//! Incremental scanning of local AI-tool transcripts for token usage sums.
//!
//! ZCode writes one JSONL per session under `~/.zcode/cli/rollout/`
//! (`model-io-sess_*.jsonl`, each line carrying `completedAt` and a
//! usage object); Claude Code writes per-session JSONL files under
//! `~/.claude/projects/<project>/*.jsonl` (assistant lines carrying
//! `timestamp` and `message.usage`); OpenAI Codex writes rollout JSONLs
//! under `~/.codex/sessions/YYYY/MM/DD/` (event_msg records of type
//! `token_count`). All are append-only in practice, so each file is
//! parsed once: the scanner keeps a byte offset and only decodes what was
//! appended since the last pass.
//!
//! Counted tokens are full context processing: input (cache reads
//! included) + output + cache writes. A single turn on a long conversation
//! re-sends the whole context, and those numbers are the reality the tile
//! is meant to show. Claude Code streams one message as several assistant
//! lines sharing `message.id` - those chunks are deduplicated.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

/// Which transcript flavor a scanner reads; only the line shapes differ.
#[derive(Clone, Copy)]
pub enum Format {
    Zcode,
    Claude,
    Codex,
}

pub(crate) struct Sample {
    pub ts: i64,
    pub tokens: u64,
}

/// A parsed usage line plus an optional streaming-chunk identity: Claude
/// Code writes one message as several assistant lines sharing the same
/// message id, and only the first occurrence counts.
struct Parsed {
    sample: Sample,
    dedup_key: Option<String>,
}

struct FileState {
    offset: u64,
    samples: Vec<Sample>,
    seen_keys: HashSet<String>,
}

/// Token sums over the windows the tiles show. `hour` is the rolling
/// last-60-minutes window, `today` is since UTC midnight, `five_hour` is
/// the rolling Claude Code window, `week` starts Monday 00:00 UTC (the
/// Anthropic weekly reset).
#[derive(Default, Clone, Copy)]
pub struct Sums {
    pub hour: u64,
    pub today: u64,
    pub five_hour: u64,
    pub week: u64,
}

pub struct Scanner {
    history_secs: i64,
    files: HashMap<PathBuf, FileState>,
}

impl Scanner {
    pub fn new(history_days: i64) -> Self {
        Self {
            history_secs: history_days.max(1) * 86_400,
            files: HashMap::new(),
        }
    }

    /// Walk `root` for `*.jsonl` (up to 3 levels deep - Claude nests
    /// sub-agent transcripts one level below the project dir) and consume
    /// whatever was appended since the last pass. Missing root is not an
    /// error: the tool simply is not installed.
    pub fn scan(&mut self, format: Format, root: &Path) {
        let cutoff = crate::unix_now() - self.history_secs;
        let mut files = Vec::new();
        collect_jsonl(root, 0, &mut files);
        let live: Vec<PathBuf> = files
            .into_iter()
            .filter(|p| file_age_ok(p, cutoff))
            .collect();
        self.files.retain(|path, _| live.contains(path));
        for path in live {
            self.ingest(format, &path);
        }
    }

    fn ingest(&mut self, format: Format, path: &Path) {
        let Ok(meta) = std::fs::metadata(path) else {
            return;
        };
        let len = meta.len();
        let state = self
            .files
            .entry(path.to_path_buf())
            .or_insert_with(|| FileState {
                offset: 0,
                samples: Vec::new(),
                seen_keys: HashSet::new(),
            });
        if len < state.offset {
            // truncated/rewritten file: start over rather than lose it
            state.offset = 0;
            state.samples.clear();
            state.seen_keys.clear();
        }
        if len == state.offset {
            return;
        }
        let Ok(mut file) = std::fs::File::open(path) else {
            return;
        };
        if let Err(e) = file.seek(std::io::SeekFrom::Start(state.offset)) {
            tracing::warn!(path = %path.display(), error = %e, "aidev transcript seek failed");
            return;
        }
        let mut raw = Vec::new();
        if let Err(e) = file.read_to_end(&mut raw) {
            tracing::warn!(path = %path.display(), error = %e, "aidev transcript read failed");
            return;
        }
        // lossy decode: invalid bytes anywhere in the chunk would fail
        // read_to_string as a whole and re-stall on the same offset
        // forever. `consumed` counts raw bytes so the offset stays
        // aligned with the file even when lossy decoding changes lengths
        let consumed = raw.len() - trailing_partial(&raw);
        let chunk = String::from_utf8_lossy(&raw[..consumed]);
        for line in chunk.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(parsed) = parse_line(format, line) {
                if let Some(key) = parsed.dedup_key {
                    if !state.seen_keys.insert(key) {
                        continue; // another streaming chunk of a counted message
                    }
                }
                state.samples.push(parsed.sample);
            }
        }
        state.offset += consumed as u64;
    }

    /// Sums per window over all ingested samples. `boundary` decides
    /// where "today" and "week" start (UTC or local midnight).
    pub fn sums(&self, now: i64, boundary: DayBoundary) -> Sums {
        let today_start = boundary.day_start(now);
        let week_start = boundary.week_start(now);
        let mut sums = Sums::default();
        for state in self.files.values() {
            for s in &state.samples {
                if s.ts > now {
                    continue; // clock skew: never count the future
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
        }
        sums
    }
}

fn file_age_ok(path: &Path, cutoff: i64) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    let Ok(modified) = meta.modified() else {
        return false;
    };
    let ts = modified
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    ts >= cutoff
}

fn collect_jsonl(dir: &Path, depth: u8, out: &mut Vec<PathBuf>) {
    if depth > 3 {
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

/// Bytes of a trailing partial line (no newline terminator yet).
fn trailing_partial(chunk: &[u8]) -> usize {
    match chunk.iter().rposition(|b| *b == b'\n') {
        Some(pos) => chunk.len() - pos - 1,
        None => chunk.len(),
    }
}

fn parse_line(format: Format, line: &str) -> Option<Parsed> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    match format {
        Format::Zcode => {
            let ts = parse_iso_rfc3339(v.get("completedAt")?.as_str()?)?;
            // two mirrors of the same numbers: the provider-native
            // snake_case block and the SDK's camelCase summary
            let usage = v
                .pointer("/response/providerMetadata/anthropic/usage")
                .or_else(|| v.pointer("/response/usage"))?;
            let pick = |snake: &str, camel: &str| {
                usage
                    .get(snake)
                    .or_else(|| usage.get(camel))
                    .and_then(|x| x.as_u64())
            };
            let tokens = tokens_from(
                pick("input_tokens", "inputTokens"),
                pick("output_tokens", "outputTokens"),
                pick("cache_creation_input_tokens", "cacheWriteTokens"),
                pick("cache_read_input_tokens", "cacheReadTokens"),
            )?;
            Some(Parsed {
                sample: Sample { ts, tokens },
                dedup_key: None,
            })
        }
        Format::Claude => {
            let message = v.get("message")?;
            let usage = message.get("usage")?;
            let ts = parse_iso_rfc3339(v.get("timestamp")?.as_str()?)?;
            let pick = |key: &str| usage.get(key).and_then(|x| x.as_u64());
            let tokens = tokens_from(
                pick("input_tokens"),
                pick("output_tokens"),
                pick("cache_creation_input_tokens"),
                pick("cache_read_input_tokens"),
            )?;
            // one streamed message can be written as several assistant
            // lines with the same id; requestId disambiguates requests
            let dedup_key = match (message.get("id"), v.get("requestId")) {
                (Some(id), Some(req)) => Some(format!(
                    "{}\u{1f}{}",
                    id.as_str().unwrap_or_default(),
                    req.as_str().unwrap_or_default()
                )),
                (Some(id), None) => Some(id.as_str().unwrap_or_default().to_string()),
                _ => None,
            };
            Some(Parsed {
                sample: Sample { ts, tokens },
                dedup_key,
            })
        }
        Format::Codex => {
            // event_msg records of type token_count; last_token_usage is
            // the per-request delta, total_token_usage the running sum -
            // counting the delta per event reassembles the total exactly
            let payload = v.get("payload")?;
            if payload.get("type")?.as_str()? != "token_count" {
                return None;
            }
            let ts = parse_iso_rfc3339(v.get("timestamp")?.as_str()?)?;
            let info = payload.get("info")?;
            let last = info
                .get("last_token_usage")
                .or_else(|| info.get("total_token_usage"))?;
            let pick = |key: &str| last.get(key).and_then(|x| x.as_u64());
            let tokens = tokens_from(
                pick("input_tokens"),
                pick("output_tokens"),
                pick("cache_write_input_tokens"),
                pick("cached_input_tokens"),
            )?;
            Some(Parsed {
                sample: Sample { ts, tokens },
                dedup_key: None,
            })
        }
    }
}

fn tokens_from(
    input: Option<u64>,
    output: Option<u64>,
    cache_write: Option<u64>,
    cache_read: Option<u64>,
) -> Option<u64> {
    // a line without any token fields is not a usage record (queue ops,
    // user turns, ...): skip it rather than count a zero sample
    if input.is_none() && output.is_none() && cache_write.is_none() && cache_read.is_none() {
        return None;
    }
    Some(
        input.unwrap_or(0)
            + output.unwrap_or(0)
            + cache_write.unwrap_or(0)
            + cache_read.unwrap_or(0),
    )
}

/// ---- time helpers (no chrono: the shapes here are fixed enough) --------------

/// Parse `2026-09-22T12:17:09.815Z` / `...+02:00` into Unix seconds.
pub(crate) fn parse_iso_rfc3339(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() < 19 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
        return None;
    }
    let num = |a: usize, b: usize| -> Option<i64> { text.get(a..b)?.parse().ok() };
    let (year, month, day) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hour, minute, second) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let mut unix = days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second;
    let rest = text[19..].trim();
    let rest = if let Some(stripped) = rest.strip_prefix('.') {
        let digits = stripped.chars().take_while(|c| c.is_ascii_digit()).count();
        if digits == 0 {
            return None;
        }
        &stripped[digits..]
    } else {
        rest
    };
    if rest.is_empty() || rest == "Z" {
        return Some(unix);
    }
    let sign = match rest.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let (h, m) = rest[1..].split_once(':')?;
    unix -= sign * (h.parse::<i64>().ok()? * 3600 + m.parse::<i64>().ok()? * 60);
    Some(unix)
}

/// Days from 1970-01-01 (Howard Hinnant's `days_from_civil`).
pub(crate) fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Midnight of the day `now` falls into, in a zone `offset` seconds east
/// of UTC. Pure in the offset so both boundary modes share one code path
/// and tests can pin exact instants without a real timezone.
pub(crate) fn day_start_in(offset: i64, now: i64) -> i64 {
    (now + offset).div_euclid(86_400) * 86_400 - offset
}

/// Monday 00:00 of the ISO week `now` falls into, in a zone `offset`
/// seconds east of UTC.
pub(crate) fn week_start_in(offset: i64, now: i64) -> i64 {
    let day = day_start_in(offset, now);
    // weekday of the local day, computed in the shifted domain
    // (1970-01-01 was a Thursday: shift by 3 so Monday maps to 0)
    let day_no = (day + offset).div_euclid(86_400);
    day - ((day_no + 3).rem_euclid(7)) * 86_400
}

pub(crate) fn utc_day_start(now: i64) -> i64 {
    day_start_in(0, now)
}

/// Monday 00:00 UTC of the current ISO week.
pub(crate) fn utc_week_start(now: i64) -> i64 {
    week_start_in(0, now)
}

/// Window boundary mode for the "today" and "week" token sums: UTC
/// midnight (the historical behavior) or the machine's local midnight.
/// East of Greenwich the UTC boundaries reset "today" in the middle of
/// the local night-to-morning (02:00 in Poland under UTC+1... 03:00 in
/// summer), which is why local midnight is configurable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DayBoundary {
    Utc,
    Local,
}

impl DayBoundary {
    /// Start of the current day under this boundary mode.
    pub(crate) fn day_start(self, now: i64) -> i64 {
        match self {
            DayBoundary::Utc => utc_day_start(now),
            DayBoundary::Local => day_start_in(local_offset_secs(now), now),
        }
    }

    /// Start of the current week (Monday 00:00) under this boundary mode.
    pub(crate) fn week_start(self, now: i64) -> i64 {
        match self {
            DayBoundary::Utc => utc_week_start(now),
            DayBoundary::Local => week_start_in(local_offset_secs(now), now),
        }
    }
}

/// The machine's timezone offset at `now`, seconds east of UTC, DST
/// included. Ambiguous or gapped instants (DST transitions) fall back to
/// UTC - a one-hour mis-attribution twice a year beats a panic.
fn local_offset_secs(now: i64) -> i64 {
    use chrono::{Offset, TimeZone};
    chrono::Local
        .timestamp_opt(now, 0)
        .single()
        .map(|dt| dt.offset().fix().local_minus_utc() as i64)
        .unwrap_or(0)
}

/// Compact token counts: 1.2K, 3.4M.
pub(crate) fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("aidev-test-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn iso_parse_matches_known_unix_values() {
        assert_eq!(parse_iso_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_iso_rfc3339("2026-09-22T12:17:09.815Z"),
            Some(1_790_079_429)
        );
        // +02:00 offset shifts back two hours
        assert_eq!(
            parse_iso_rfc3339("2026-09-22T12:17:09+02:00"),
            Some(1_790_072_229)
        );
        assert_eq!(parse_iso_rfc3339("not a date"), None);
        assert_eq!(parse_iso_rfc3339("2026-13-40T99:99:99Z"), None);
    }

    #[test]
    fn week_start_is_monday_utc() {
        // 2026-09-22 is a Tuesday; the week started Monday 2026-09-21
        let tuesday = 1_790_035_200; // 2026-09-22T00:00:00Z
        assert_eq!(utc_day_start(tuesday), tuesday);
        assert_eq!(utc_week_start(tuesday), tuesday - 86_400);
        // 1970-01-01 (Thursday) belongs to the week of Monday 1969-12-29
        assert_eq!(utc_week_start(0), -3 * 86_400);
    }

    #[test]
    fn day_boundaries_follow_the_zone_offset() {
        // 2026-09-22T21:30Z is 23:30 the same day in UTC+2: the LOCAL day
        // began at 22:00Z the evening before, while the UTC day starts at
        // 00:00Z - exactly the gap that made Poland's "today" reset at
        // 02:00 (01:00 UTC+1 / 03:00 DST) local time.
        let now = 1_790_112_600; // 2026-09-22T21:30:00Z
        assert_eq!(day_start_in(0, now), 1_790_035_200); // UTC midnight
        assert_eq!(day_start_in(7_200, now), 1_790_028_000); // local midnight
        assert_eq!(
            day_start_in(7_200, now),
            utc_day_start(now) - 2 * 3_600,
            "UTC+2 local midnight is two hours earlier in absolute time"
        );
        // west of UTC the local midnight comes later instead
        assert_eq!(
            day_start_in(-6 * 3_600, now),
            utc_day_start(now) + 6 * 3_600
        );
        // the day boundary math also holds before the epoch: one second
        // before midnight UTC is 00:59 local (UTC+1), so the local day
        // began at 23:00Z the evening before
        assert_eq!(day_start_in(3_600, -1), -3_600);
    }

    #[test]
    fn week_boundaries_follow_the_zone_offset() {
        // Tuesday evening 2026-09-22T21:30Z, UTC+2: still Tuesday locally,
        // so the local week began Monday 2026-09-21T00:00+02:00, which is
        // Sunday 22:00Z - the same calendar Monday, two hours earlier.
        let now = 1_790_112_600;
        assert_eq!(week_start_in(0, now), 1_789_948_800); // Mon 2026-09-21T00:00Z
        assert_eq!(week_start_in(7_200, now), 1_789_941_600); // Mon 00:00+02:00
        assert_eq!(
            week_start_in(7_200, now),
            week_start_in(0, now) - 2 * 3_600,
            "local Monday 00:00+02:00 is Sunday 22:00Z, two hours earlier"
        );
        // west of UTC the local Monday 00:00 lands after the UTC one:
        // UTC-5 Monday 00:00 is Monday 05:00Z
        assert_eq!(week_start_in(-5 * 3_600, now), 1_789_948_800 + 5 * 3_600);
    }

    #[test]
    fn local_mode_routes_through_the_machine_offset() {
        // the Local branch must delegate to the offset math with the
        // machine's real offset (guards against swapped match arms); the
        // offset itself is whatever this machine is set to
        let now = 1_790_112_600;
        let off = local_offset_secs(now);
        assert_eq!(DayBoundary::Local.day_start(now), day_start_in(off, now));
        assert_eq!(DayBoundary::Local.week_start(now), week_start_in(off, now));
        assert_eq!(DayBoundary::Utc.day_start(now), day_start_in(0, now));
    }

    #[test]
    fn day_boundary_mode_matches_its_setting() {
        // the config switch maps onto the two modes; the UTC default keeps
        // the historical boundary exactly
        let cfg = crate::Config::default();
        assert!(!cfg.local_midnight);
        let now = 1_790_112_600;
        assert_eq!(
            cfg.day_boundary().day_start(now),
            utc_day_start(now),
            "default (UTC) boundaries must not move"
        );
    }

    #[test]
    fn local_midnight_config_parses() {
        let cfg: crate::Config = serde_json::from_str(r#"{"local_midnight": true}"#).unwrap();
        assert!(cfg.local_midnight);
        assert_eq!(cfg.day_boundary(), DayBoundary::Local);
        // an absent key keeps the default
        let cfg: crate::Config = serde_json::from_str(r#"{}"#).unwrap();
        assert!(!cfg.local_midnight);
        assert_eq!(cfg.day_boundary(), DayBoundary::Utc);
    }

    #[test]
    fn fmt_tokens_compacts() {
        assert_eq!(fmt_tokens(942), "942");
        assert_eq!(fmt_tokens(12_345), "12.3K");
        assert_eq!(fmt_tokens(1_234_567), "1.2M");
    }

    #[test]
    fn zcode_claude_and_codex_lines_parse_to_samples() {
        // full context processing: cache reads count with everything else
        let zline = r#"{"completedAt":"2026-09-22T12:17:09.815Z","response":{"providerMetadata":{"anthropic":{"usage":{"input_tokens":6952,"output_tokens":112,"cache_read_input_tokens":41280}}},"usage":{"inputTokens":1,"outputTokens":1}}}"#;
        let s = parse_line(Format::Zcode, zline).unwrap();
        assert_eq!(s.sample.tokens, 6952 + 112 + 41280);
        assert_eq!(s.sample.ts, 1_790_079_429);

        // without metadata the SDK's camelCase mirror is used
        let zline2 = r#"{"completedAt":"2026-09-22T12:00:00Z","response":{"usage":{"inputTokens":100,"outputTokens":50,"cacheWriteTokens":10,"cacheReadTokens":999}}}"#;
        let s = parse_line(Format::Zcode, zline2).unwrap();
        assert_eq!(s.sample.tokens, 100 + 50 + 10 + 999);

        let cline = r#"{"type":"assistant","timestamp":"2026-09-22T12:00:00Z","message":{"id":"msg_1","usage":{"input_tokens":100,"output_tokens":50,"cache_creation_input_tokens":10,"cache_read_input_tokens":999}}}"#;
        let s = parse_line(Format::Claude, cline).unwrap();
        assert_eq!(s.sample.tokens, 100 + 50 + 10 + 999);
        assert_eq!(s.dedup_key.as_deref(), Some("msg_1"));

        // codex token_count event: the per-request delta is counted
        let xline = r#"{"timestamp":"2026-08-03T14:03:03.038Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":25288,"cached_input_tokens":17920,"cache_write_input_tokens":0,"output_tokens":606,"total_tokens":43814},"last_token_usage":{"input_tokens":12644,"cached_input_tokens":8960,"cache_write_input_tokens":0,"output_tokens":303,"reasoning_output_tokens":105,"total_tokens":21907}}}}"#;
        let s = parse_line(Format::Codex, xline).unwrap();
        assert_eq!(s.sample.tokens, 12644 + 303 + 8960);

        // non-usage lines yield nothing
        let op = r#"{"type":"queue-operation","operation":"enqueue","timestamp":"2026-09-22T12:00:00Z"}"#;
        assert!(parse_line(Format::Claude, op).is_none());
        let zop = r#"{"completedAt":"2026-09-22T12:00:00Z","requestId":"x"}"#;
        assert!(parse_line(Format::Zcode, zop).is_none());
        let nop = r#"{"timestamp":"2026-08-03T14:03:03.038Z","type":"event_msg","payload":{"type":"task_complete"}}"#;
        assert!(parse_line(Format::Codex, nop).is_none());
    }

    #[test]
    fn claude_streaming_chunks_dedupe_by_message_id() {
        let dir = tmp_dir("dedup");
        let f = dir.join("proj").join("s.jsonl");
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        let t0 = 1_700_000_000;
        // three lines, two of them chunks of the same message
        let mut body = String::new();
        for tokens in [100, 200, 300] {
            body.push_str(&format!(
                r#"{{"type":"assistant","timestamp":"{}","message":{{"id":"msg_1","usage":{{"input_tokens":{tokens},"output_tokens":0}}}}}}"#,
                iso(t0)
            ));
            body.push('\n');
        }
        std::fs::write(&f, &body).unwrap();
        let mut scanner = Scanner::new(8);
        scanner.scan(Format::Claude, &dir);
        assert_eq!(scanner.sums(t0 + 10, DayBoundary::Utc).today, 100);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hour_window_is_rolling_sixty_minutes() {
        let dir = tmp_dir("hour");
        let f = dir.join("s.jsonl");
        let now = 1_790_079_429;
        let mut body = String::new();
        for (i, age) in [30i64, 500, 3700].iter().enumerate() {
            body.push_str(&format!(
                r#"{{"completedAt":"{}","response":{{"usage":{{"inputTokens":{},"outputTokens":0}}}}}}"#,
                iso(now - age),
                1000 + i as u64
            ));
            body.push('\n');
        }
        std::fs::write(&f, &body).unwrap();
        let mut scanner = Scanner::new(8);
        scanner.scan(Format::Zcode, &dir);
        let s = scanner.sums(now, DayBoundary::Utc);
        assert_eq!(s.hour, 1000 + 1001); // 3700s old is outside the hour
        assert_eq!(s.five_hour, 1000 + 1001 + 1002); // but inside 5h

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scanner_is_incremental_and_windowed() {
        let dir = tmp_dir("incr");
        let f = dir.join("model-io-sess_x.jsonl");
        let t0 = 1_700_000_000;
        let mut body = String::new();
        for i in 0..5 {
            body.push_str(&format!(
                r#"{{"completedAt":"{}","response":{{"usage":{{"inputTokens":1000,"outputTokens":0}}}}}}"#,
                iso(t0 + i)
            ));
            body.push('\n');
        }
        std::fs::write(&f, &body).unwrap();

        let mut scanner = Scanner::new(8);
        scanner.scan(Format::Zcode, &dir);
        let now = t0 + 10;
        let s = scanner.sums(now, DayBoundary::Utc);
        assert_eq!(s.today, 5000); // all samples same UTC day
        assert_eq!(s.five_hour, 5000);

        // append two more records and rescan: only the new bytes are read
        let mut body2 = body.clone();
        for i in 5..7 {
            body2.push_str(&format!(
                r#"{{"completedAt":"{}","response":{{"usage":{{"inputTokens":1000,"outputTokens":0}}}}}}"#,
                iso(t0 + i)
            ));
            body2.push('\n');
        }
        std::fs::write(&f, &body2).unwrap();
        scanner.scan(Format::Zcode, &dir);
        assert_eq!(scanner.sums(now, DayBoundary::Utc).today, 7000);
        assert_eq!(scanner.files[&f].offset, body2.len() as u64);

        // a torn trailing line waits for its newline
        std::fs::write(&f, format!("{body2}{{\"completedAt\":\"20")).unwrap();
        scanner.scan(Format::Zcode, &dir);
        assert_eq!(scanner.files[&f].offset, body2.len() as u64);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_files_drop_out() {
        let dir = tmp_dir("stale");
        let f = dir.join("old.jsonl");
        std::fs::write(
            &f,
            format!(
                r#"{{"completedAt":"{}","response":{{"providerMetadata":{{"anthropic":{{"usage":{{"input_tokens":100,"output_tokens":0}}}}}}}}}}"#,
                iso(1_700_000_000)
            ),
        )
        .unwrap();
        let mut scanner = Scanner::new(8);
        scanner.scan(Format::Zcode, &dir);
        assert!(scanner.files.contains_key(&f));
        // backdate the file beyond the history window and rescan
        set_mtime_old(&f);
        scanner.scan(Format::Zcode, &dir);
        assert!(!scanner.files.contains_key(&f));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_utf8_does_not_stall_the_scanner() {
        let dir = tmp_dir("badutf8");
        let f = dir.join("s.jsonl");
        // a crashed writer can leave raw invalid bytes in a transcript;
        // the valid usage line after them must still count and the byte
        // offset must advance past the damage
        let mut body: Vec<u8> = b"{\"completedAt\":\"2026-09-22T12:00".to_vec();
        body.extend_from_slice(&[0xFF, 0xFE]);
        body.extend_from_slice(b"00Z\",\"response\":{}}\n");
        body.extend_from_slice(
            br#"{"completedAt":"2026-09-22T12:00:00Z","response":{"usage":{"inputTokens":1000,"outputTokens":0}}}"#,
        );
        body.push(b'\n');
        std::fs::write(&f, &body).unwrap();
        let mut scanner = Scanner::new(8);
        scanner.scan(Format::Zcode, &dir);
        assert_eq!(scanner.sums(1_790_079_429, DayBoundary::Utc).today, 1000);
        assert_eq!(scanner.files[&f].offset, body.len() as u64);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn iso(t: i64) -> String {
        let days = t.div_euclid(86_400);
        let secs = t.rem_euclid(86_400);
        let (y, m, d) = civil_from_days(days);
        format!(
            "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
            secs / 3600,
            (secs % 3600) / 60,
            secs % 60
        )
    }

    fn civil_from_days(z: i64) -> (i64, i64, i64) {
        let z = z + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        (if m <= 2 { y + 1 } else { y }, m, d)
    }

    fn set_mtime_old(path: &Path) {
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(9 * 86_400);
        let f = std::fs::File::options().append(true).open(path).unwrap();
        f.set_modified(old).unwrap();
    }
}
