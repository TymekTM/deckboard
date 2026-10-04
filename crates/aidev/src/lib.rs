//! Native "AI dev work" source: read-only display tiles for AI plan limits
//! and coding-agent progress, built the same way as `deckboard-sysinfo` -
//! [`input_declarations`] styles the tiles, a background thread pushes values
//! under stable keys, and the shell forwards them to both protocols as
//! `APP_CUSTOM_VALUE`.
//!
//! Two data families feed the tiles:
//! - *plan limits*: HTTP providers with locally available credentials
//!   (OpenRouter, Anthropic admin usage, plus arbitrary JSON endpoints
//!   from the config file), Codex's embedded rate limits, Claude's OAuth
//!   usage API and token counts summed from local transcripts (ZCode,
//!   Claude Code, Codex, OpenCode, Antigravity);
//! - *agent progress*: session freshness across the five providers
//!   (ZCode journal, Claude Code, Codex and OpenCode transcripts,
//!   Antigravity conversation databases) - actively written means
//!   "working", recently gone quiet means "may need attention", older
//!   means "done" - grouped by project with the conversation title and
//!   provider glyph on each row.

use std::path::PathBuf;

use tokio::sync::mpsc as tokio_mpsc;

mod agents;
mod antigravity;
mod codex;
mod limits;
mod local_usage;
mod opencode;
mod util;

/// Tile inputs this source serves: (value, icon, color, mode). The `status`
/// mode is the editor's multi-row display class; `graph` reuses the
/// sparkline tile like the si-* inputs do.
pub fn input_declarations() -> Vec<(&'static str, &'static str, &'static str, &'static str)> {
    vec![
        ("ai-plan-limits", "tachometer-alt", "#171A21", "status"),
        ("ai-agent-status", "robot", "#171A21", "status"),
        ("ai-tokens-today", "coins", "#171A21", "graph"),
        ("ai-tokens-hour", "clock", "#171A21", "graph"),
    ]
}

/// Does this action kind belong to the native AI dev-work source?
pub fn is_aidev_action(kind: &str) -> bool {
    input_declarations()
        .iter()
        .any(|(value, ..)| *value == kind)
}

/// Display tiles have nothing to press; keep the claimed no-op parity with
/// `deckboard-sysinfo` so tile taps succeed without changing anything.
pub fn execute(kind: &str) {
    tracing::debug!(kind, "ai dev-work action accepted (no-op)");
}

/// Where the source reads its config and the tools write their state. The
/// shell passes its home-derived paths so the crate stays testable.
#[derive(Clone)]
pub struct Paths {
    /// `deckboard/aidev.json` - optional provider keys and tuning.
    pub config: PathBuf,
    /// `~/.zcode/cli` - the db.sqlite journal and rollout token logs.
    pub zcode_cli: PathBuf,
    /// `~/.claude/projects` - Claude Code transcripts; the parent dir also
    /// holds `.credentials.json` for the OAuth usage API.
    pub claude_projects: PathBuf,
    /// `~/.codex/sessions` - Codex rollout JSONLs.
    pub codex_sessions: PathBuf,
    /// `~/.local/share/opencode/opencode.db` - OpenCode journal.
    pub opencode_db: PathBuf,
    /// `~/.gemini/antigravity/conversations` - Antigravity databases.
    pub antigravity_conversations: PathBuf,
}

/// ---- config ----------------------------------------------------------------

#[derive(serde::Deserialize, Clone)]
#[serde(default)]
pub struct Config {
    /// Local tick: transcripts and agent status are re-read this often.
    pub poll_secs: u64,
    /// Remote plan APIs are polled this often; they move slowly and each
    /// poll costs a request against the provider.
    pub http_poll_secs: u64,
    /// Session activity younger than this counts as "working".
    pub agent_fresh_secs: i64,
    /// Activity this old (while the last turn never completed) counts as
    /// "may need attention" - a long tool call, a permission prompt, or a
    /// finished turn waiting for review.
    pub agent_attention_secs: i64,
    /// Sessions that went quiet longer ago drop off the tile entirely.
    pub agent_done_secs: i64,
    /// Days of transcript samples kept in memory for the window sums.
    pub history_days: i64,
    /// Optional token ceilings turning local sums into percentages: the
    /// Claude plan pair and the GLM pair shared by ZCode + OpenCode.
    pub claude_five_hour_tokens: Option<u64>,
    pub claude_week_tokens: Option<u64>,
    pub glm_five_hour_tokens: Option<u64>,
    pub glm_week_tokens: Option<u64>,
    /// Count the "today" / "week" token windows from LOCAL midnight
    /// instead of UTC. Default false keeps the historical UTC boundaries:
    /// east of Greenwich those reset "today" at a confusing local wall
    /// time (02:00 in Poland on UTC+1), which this switch moves to local
    /// midnight.
    pub local_midnight: bool,
    /// Plan-limits tile presentation (Ustawienia -> AI usage): which
    /// detected rows to show and whether the summary line is wanted.
    pub status: StatusConfig,
    pub providers: Providers,
    /// Generic JSON endpoints: name, url, headers, used/limit JSON paths.
    /// This is the "support practically everything" escape hatch.
    pub custom: Vec<CustomProvider>,
}

#[derive(serde::Deserialize, Clone, Default)]
pub struct Providers {
    pub openrouter: Option<Apikey>,
    pub anthropic: Option<Apikey>,
    /// GLM Coding Plan (z.ai / BigModel): the same API key the plan page
    /// issues, queried against the monitor quota endpoint.
    pub zai: Option<ZaiProvider>,
    /// Codex OAuth usage lane (`wham/usage`): reads the CLI's own
    /// auth.json, no key to configure - presence under `providers.codex`
    /// (as `{}`) enables it.
    pub codex: Option<CodexProvider>,
}

#[derive(serde::Deserialize, Clone)]
pub struct Apikey {
    pub api_key: Option<String>,
}

/// Codex wham/usage lane knobs; all optional, the empty object is the
/// normal configuration.
#[derive(serde::Deserialize, Clone, Default)]
pub struct CodexProvider {
    /// Override of the Codex CLI home (defaults to the parent of the
    /// sessions directory, usually `~/.codex`).
    pub home: Option<String>,
}

#[derive(serde::Deserialize, Clone, Default)]
pub struct ZaiProvider {
    pub api_key: Option<String>,
    /// Host override (`open.bigmodel.cn` for mainland plans) or a full
    /// URL; defaults to the global `https://api.z.ai`.
    pub host: Option<String>,
}

/// Plan-limits tile presentation, edited from Ustawienia -> AI usage.
/// `show` lists the row ids that survive; empty means "everything the
/// producers detect" (the historical behavior). `summary` keeps the
/// one-line headline under the row list. `row_style` picks the row
/// identifier: "name" renders the text label, "logo" the provider mark
/// (falling back to the name when no mark exists) - never both.
#[derive(serde::Deserialize, Clone, PartialEq)]
pub struct StatusConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub show: Vec<String>,
    #[serde(default = "default_true")]
    pub summary: bool,
    #[serde(default)]
    pub row_style: RowStyle,
}

#[derive(serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum RowStyle {
    #[default]
    Name,
    Logo,
}

impl RowStyle {
    pub fn as_str(self) -> &'static str {
        match self {
            RowStyle::Name => "name",
            RowStyle::Logo => "logo",
        }
    }
}

impl Default for StatusConfig {
    fn default() -> Self {
        // the derive would give summary=false; the line is ON unless
        // the user turns it off
        Self {
            show: Vec::new(),
            summary: true,
            row_style: RowStyle::default(),
        }
    }
}

fn default_true() -> bool {
    true
}

/// One detected plan-limits row, published for the settings UI: `id` is
/// the stable selection key ("glm:5h", "codex:week", ...), `label` the
/// human-readable row name ("GLM 5h").
#[derive(Clone, serde::Serialize)]
pub struct DetectedRow {
    pub id: String,
    pub label: String,
}

static DETECTED: std::sync::OnceLock<std::sync::Mutex<Vec<DetectedRow>>> =
    std::sync::OnceLock::new();

/// Remember the rows this cycle's producers built, before any
/// user-selection filtering, so the settings UI can offer every limit
/// the machine actually reports (auto-detection).
pub(crate) fn record_detected(rows: &[crate::limits::ProviderRow]) {
    let cell = DETECTED.get_or_init(|| std::sync::Mutex::new(Vec::new()));
    let mut known = cell.lock().expect("detected registry lock");
    for row in rows {
        if row.id.is_empty() || known.iter().any(|d| d.id == row.id) {
            continue;
        }
        known.push(DetectedRow {
            id: row.id.clone(),
            label: row.name.clone(),
        });
    }
}

/// Every plan-limits row detected so far (deduplicated by id, insertion
/// order kept). The settings UI reads this to render the checkboxes.
pub fn detected_rows() -> Vec<DetectedRow> {
    DETECTED
        .get_or_init(|| std::sync::Mutex::new(Vec::new()))
        .lock()
        .expect("detected registry lock")
        .clone()
}

#[derive(serde::Deserialize, Clone)]
pub struct CustomProvider {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub headers: std::collections::BTreeMap<String, String>,
    /// Dotted JSON path to the used value, e.g. "data.usage".
    pub used_path: String,
    /// Dotted JSON path to the limit value; without it the row shows the
    /// used value only.
    pub limit_path: Option<String>,
    /// Free-form unit suffix ("USD", "credits", ...).
    pub unit: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            // short enough for the tokens-per-hour tile to feel live
            poll_secs: 15,
            http_poll_secs: 300,
            agent_fresh_secs: 180,
            agent_attention_secs: 900,
            agent_done_secs: 4 * 3600,
            history_days: 8,
            claude_five_hour_tokens: None,
            claude_week_tokens: None,
            glm_five_hour_tokens: None,
            glm_week_tokens: None,
            local_midnight: false,
            status: StatusConfig::default(),
            providers: Providers::default(),
            custom: Vec::new(),
        }
    }
}

impl Config {
    /// Where the "today" / "week" token windows start: local midnight
    /// when `local_midnight` is set, UTC midnight (the historical
    /// behavior) otherwise.
    pub(crate) fn day_boundary(&self) -> local_usage::DayBoundary {
        if self.local_midnight {
            local_usage::DayBoundary::Local
        } else {
            local_usage::DayBoundary::Utc
        }
    }

    /// Missing or broken config file falls back to defaults - the tiles must
    /// not die because of a typo in an optional file.
    pub fn load(path: &std::path::Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                tracing::warn!(path = %path.display(), error = %e, "aidev config invalid, using defaults");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }
}

// ---- push loop ---------------------------------------------------------------

/// Spawn the poll loop; values arrive as one object with the ai-* keys.
pub fn spawn_push(paths: Paths) -> tokio_mpsc::UnboundedReceiver<serde_json::Value> {
    let (tx, rx) = tokio_mpsc::unbounded_channel();
    std::thread::Builder::new()
        .name("aidev-push".into())
        .spawn(move || push_loop(&tx, paths))
        .expect("spawn aidev thread");
    rx
}

fn push_loop(tx: &tokio_mpsc::UnboundedSender<serde_json::Value>, paths: Paths) {
    let config = Config::load(&paths.config);
    let mut zcode = local_usage::Scanner::new(config.history_days);
    let mut claude = local_usage::Scanner::new(config.history_days);
    let mut codex = local_usage::Scanner::new(config.history_days);
    let mut opencode = opencode::Usage::new(config.history_days);
    let mut antigravity = antigravity::Usage::new(config.history_days);
    let mut http = limits::HttpState::default();
    let mut last_http: Option<std::time::Instant> = None;
    loop {
        // the config is re-read every cycle so edits from Ustawienia ->
        // AI usage apply live, without restarting the app; the scanners
        // keep their history, only the knobs move
        let config = Config::load(&paths.config);
        let http_every = std::time::Duration::from_secs(config.http_poll_secs.max(30));
        let now = unix_now();
        zcode.scan(local_usage::Format::Zcode, &paths.zcode_cli.join("rollout"));
        claude.scan(local_usage::Format::Claude, &paths.claude_projects);
        codex.scan(local_usage::Format::Codex, &paths.codex_sessions);
        opencode.scan(&paths.opencode_db, now);
        antigravity.scan(&paths.antigravity_conversations, now);
        if last_http.is_none_or(|t| t.elapsed() >= http_every) {
            http.refresh(&config, &paths);
            last_http = Some(std::time::Instant::now());
        }
        let payload = assemble(
            &config,
            &http,
            &zcode,
            &claude,
            &codex,
            &opencode,
            &antigravity,
            &paths,
            now,
        );
        if tx.send(payload).is_err() {
            tracing::debug!("aidev push channel closed, stopping");
            return;
        }
        std::thread::sleep(std::time::Duration::from_secs(config.poll_secs.max(5)));
    }
}

/// One snapshot of every ai-* key. `rows` are display-ready so the tile
/// renderer stays dumb; `percent` (0..100) drives the thin usage bars.
#[allow(clippy::too_many_arguments)] // nine read-only inputs into one private fn; a params struct is churn in this perf-frozen crate
fn assemble(
    config: &Config,
    http: &limits::HttpState,
    zcode: &local_usage::Scanner,
    claude: &local_usage::Scanner,
    codex: &local_usage::Scanner,
    opencode: &opencode::Usage,
    antigravity: &antigravity::Usage,
    paths: &Paths,
    now: i64,
) -> serde_json::Value {
    let boundary = config.day_boundary();
    let sums = [
        ("Zcode", zcode.sums(now, boundary)),
        ("Claude", claude.sums(now, boundary)),
        ("Codex", codex.sums(now, boundary)),
        ("OpenCode", opencode.sums(now, boundary)),
        ("Antigravity", antigravity.sums(now, boundary)),
    ];
    let total = |key: fn(&local_usage::Sums) -> u64| sums.iter().map(|(_, s)| key(s)).sum::<u64>();

    let plan_rows = limits::plan_rows(config, http, &sums, paths, now);
    // one-line headline: whichever window is closest to its limit; the
    // AI-usage settings can turn the whole line off
    let plan_summary = if config.status.summary {
        plan_rows
            .iter()
            .filter_map(|r| r.percent.map(|p| (p, r)))
            .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(p, r)| format!("{} {:.0}%", r.name, p))
            .unwrap_or_default()
    } else {
        String::new()
    };

    let agents = agents::snapshot(config, paths, now);
    let hour_rows: Vec<_> = sums
        .iter()
        .filter(|(_, s)| s.hour > 0)
        .map(|(name, s)| {
            serde_json::json!({
                "label": name,
                "provider": name.to_lowercase(),
                "value": local_usage::fmt_tokens(s.hour),
            })
        })
        .collect();

    serde_json::json!({
        "ai-plan-limits": {
            "title": "AI plan limits",
            "rows": plan_rows.iter().map(limits::row_json).collect::<Vec<_>>(),
            // percent summary only when at least one lane knows its ceiling;
            // an empty string keeps the renderer from drawing the line.
            // `hide_summary` tells renderers that recompute their own
            // headline from the visible rows to keep quiet too, and
            // `row_style` picks name vs brand mark as the row identifier.
            "summary": plan_summary,
            "hide_summary": !config.status.summary,
            "row_style": config.status.row_style.as_str(),
        },
        "ai-agent-status": agents,
        // graph tile: the client keeps the last 10 samples as a sparkline;
        // `value_label` is the compact display form ("11.0M") of `value`
        "ai-tokens-today": {
            "title": "AI tokens today",
            "value": total(|s| s.today),
            "value_label": local_usage::fmt_tokens(total(|s| s.today)),
            "suffix": "",
        },
        // same shape, rolling 60-minute window; `rows` carries the
        // per-provider breakdown the renderer shows on tap
        "ai-tokens-hour": {
            "title": "AI tokens / hour",
            "value": total(|s| s.hour),
            "value_label": local_usage::fmt_tokens(total(|s| s.hour)),
            "suffix": "",
            "rows": hour_rows,
        },
    })
}

/// Wall-clock seconds since the Unix epoch (second precision is plenty).
pub(crate) fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live smoke check against the real user directories (run explicitly:
    /// `cargo test -p deckboard-aidev -- --ignored live`). Prints the first
    /// pushed snapshot so a human can eyeball plan limits and agent rows.
    #[test]
    #[ignore]
    fn live_snapshot_prints_real_payload() {
        let Some(home) = dirs_home() else {
            return; // no home on this machine: nothing to smoke-test
        };
        let paths = Paths {
            config: home.join("deckboard/aidev.json"),
            zcode_cli: home.join(".zcode/cli"),
            claude_projects: home.join(".claude/projects"),
            codex_sessions: home.join(".codex/sessions"),
            opencode_db: home.join(".local/share/opencode/opencode.db"),
            antigravity_conversations: home.join(".gemini/antigravity/conversations"),
        };
        let mut rx = spawn_push(paths);
        let payload = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("test runtime")
            .block_on(async {
                tokio::time::timeout(std::time::Duration::from_secs(30), rx.recv())
                    .await
                    .expect("first push within 30 s")
                    .expect("push channel open")
            });
        println!("{}", serde_json::to_string_pretty(&payload).unwrap());
        assert!(payload.get("ai-agent-status").is_some());
    }

    #[cfg(windows)]
    fn dirs_home() -> Option<PathBuf> {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    }

    #[cfg(not(windows))]
    fn dirs_home() -> Option<PathBuf> {
        std::env::var_os("HOME").map(PathBuf::from)
    }

    #[test]
    fn declarations_cover_exactly_the_pushed_keys() {
        let decls = input_declarations();
        let values: Vec<_> = decls.iter().map(|(v, ..)| *v).collect();
        assert_eq!(
            values,
            [
                "ai-plan-limits",
                "ai-agent-status",
                "ai-tokens-today",
                "ai-tokens-hour"
            ]
        );
        // every declaration must be a claimable action kind
        assert!(is_aidev_action("ai-tokens-hour"));
        assert!(!is_aidev_action("si-cpu"));
    }

    #[test]
    fn config_defaults_when_file_missing() {
        let cfg = Config::load(std::path::Path::new("Z:/definitely/not/here.json"));
        assert_eq!(cfg.poll_secs, 15);
        assert_eq!(cfg.history_days, 8);
        assert_eq!(cfg.glm_five_hour_tokens, None);
        // no status section: everything detected is shown, summary on,
        // rows identified by name
        assert_eq!(cfg.status.show, Vec::<String>::new());
        assert!(cfg.status.summary);
        assert_eq!(cfg.status.row_style, RowStyle::Name);
    }

    #[test]
    fn config_parses_status_selection() {
        let cfg: Config = serde_json::from_str(
            r#"{"status": {"show": ["glm:5h", "claude:week"], "summary": false, "row_style": "logo"}}"#,
        )
        .expect("status config parses");
        assert_eq!(cfg.status.show, ["glm:5h", "claude:week"]);
        assert!(!cfg.status.summary);
        assert_eq!(cfg.status.row_style, RowStyle::Logo);
    }

    #[test]
    fn config_parses_provider_keys_and_custom_endpoints() {
        let text = r#"{
            "poll_secs": 30,
            "providers": {"openrouter": {"api_key": "sk-or-1"}, "codex": {}},
            "custom": [{
                "name": "My gateway",
                "url": "https://gw.example/usage",
                "used_path": "data.used",
                "unit": "credits"
            }]
        }"#;
        let cfg: Config = serde_json::from_str(text).expect("parse");
        assert_eq!(cfg.poll_secs, 30);
        assert_eq!(
            cfg.providers.openrouter.and_then(|a| a.api_key).as_deref(),
            Some("sk-or-1")
        );
        // an empty codex object turns the wham/usage lane on
        assert!(cfg.providers.codex.is_some());
        assert_eq!(cfg.custom.len(), 1);
        assert_eq!(cfg.custom[0].unit.as_deref(), Some("credits"));
        // unspecified knobs keep their defaults
        assert_eq!(cfg.agent_fresh_secs, 180);
    }

    #[test]
    fn payload_carries_display_ready_rows() {
        let config = Config::default();
        let http = limits::HttpState {
            rows: vec![limits::ProviderRow {
                id: limits::row_id("OpenRouter", None),
                name: "OpenRouter".into(),
                state: "ok".into(),
                text: "used $4.00 / $10.00".into(),
                percent: Some(40.0),
                reset_at: None,
            }],
            claude_oauth: None,
            codex_wham: None,
        };
        let paths = Paths {
            config: "unused".into(),
            zcode_cli: "Z:/nope".into(),
            claude_projects: "Z:/nope".into(),
            codex_sessions: "Z:/nope".into(),
            opencode_db: "Z:/nope".into(),
            antigravity_conversations: "Z:/nope".into(),
        };
        let payload = assemble(
            &config,
            &http,
            &local_usage::Scanner::new(8),
            &local_usage::Scanner::new(8),
            &local_usage::Scanner::new(8),
            &opencode::Usage::new(8),
            &antigravity::Usage::new(8),
            &paths,
            1_800_000_000,
        );
        let plan = &payload["ai-plan-limits"];
        assert_eq!(plan["title"], "AI plan limits");
        assert!(plan["rows"][0]["label"] == "OpenRouter");
        assert!(plan["summary"].as_str().unwrap().contains("40%"));
        assert!(payload["ai-agent-status"]["rows"].is_array());
        assert!(payload["ai-tokens-today"]["value"].is_u64());
        // the hour tile carries the shared window and per-provider rows
        assert!(payload["ai-tokens-hour"]["value"].is_u64());
        assert!(payload["ai-tokens-hour"]["rows"].is_array());
    }
}
