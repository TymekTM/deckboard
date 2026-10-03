//! Plan-limit providers. HTTP APIs are polled only when a credential is
//! available (config file or the tool's conventional env var); providers
//! without access are simply absent from the tile instead of showing a wall
//! of errors. Generic JSON endpoints from the config cover anything the
//! built-ins do not.
//!
//! Lanes and their sources:
//! - Codex: the `rate_limits` embedded in local rollout files - no API;
//! - Claude: the OAuth usage API with the CLI's own token, falling back
//!   to local sums against configured ceilings;
//! - GLM (ZCode + OpenCode, one plan): local sums against ceilings;
//! - Antigravity: the running IDE's local quota endpoint;
//! - OpenRouter / Anthropic admin / custom endpoints: opt-in API keys.

use std::io::Read;

use crate::util::truncate;
use crate::{local_usage::fmt_tokens, local_usage::Sums, Apikey, Config, Paths};

/// One display row of the plan-limits tile. `state` colors the dot:
/// ok | warn | high | error. `percent` is the usage fill of the bar; the
/// value text speaks in the remaining limit and, when the source reports
/// it, the reset countdown (`reset_at`, unix epoch seconds).
#[derive(Clone)]
pub struct ProviderRow {
    pub name: String,
    pub state: String,
    pub text: String,
    pub percent: Option<f64>,
    pub reset_at: Option<i64>,
}

pub fn row_json(row: &ProviderRow) -> serde_json::Value {
    serde_json::json!({
        "label": row.name,
        "value": row.text,
        "state": row.state,
        "percent": row.percent,
        "reset_at": row.reset_at,
    })
}

#[derive(Default)]
pub struct HttpState {
    pub rows: Vec<ProviderRow>,
    /// Claude 5h/weekly utilization from the OAuth usage API; `None` when
    /// the token is missing, scope-less or the call fails - the tile then
    /// falls back to local sums against configured ceilings.
    pub claude_oauth: Option<(Option<f64>, Option<f64>)>,
    /// Codex windows from the wham/usage API; `None` when the lane is not
    /// configured, the CLI is not logged in, or the call fails - the tile
    /// then falls back to the limits embedded in local rollouts.
    pub codex_wham: Option<crate::codex::Limits>,
}

impl HttpState {
    /// Poll every network-backed provider. Runs on the dedicated push
    /// thread at the slow cadence, so blocking here only delays this
    /// source, never the app.
    pub fn refresh(&mut self, config: &Config, paths: &Paths) {
        self.rows = poll_all(config);
        self.rows.extend(antigravity_quota_rows());
        self.claude_oauth = claude_oauth_usage(paths);
        self.codex_wham = codex_wham_limits(config, paths);
    }
}

fn poll_all(config: &Config) -> Vec<ProviderRow> {
    let mut rows = Vec::new();
    let openrouter = resolve_key(config.providers.openrouter.as_ref(), "OPENROUTER_API_KEY");
    if let Some(key) = openrouter {
        rows.push(openrouter_row(&key));
    }
    let anthropic = resolve_key(config.providers.anthropic.as_ref(), "ANTHROPIC_API_KEY");
    if let Some(key) = anthropic {
        rows.push(anthropic_row(&key));
    }
    if let Some(zai) = config.providers.zai.as_ref() {
        let key = resolve_key(
            Some(&Apikey {
                api_key: zai.api_key.clone(),
            }),
            "Z_AI_API_KEY",
        );
        if let Some(key) = key {
            rows.extend(zai_rows(&key, zai.host.as_deref()));
        }
    }
    for custom in &config.custom {
        rows.push(custom_row(custom));
    }
    rows
}

/// All plan-limits rows for this push: the slow network lanes plus the
/// cheap local ones (Codex embedded limits, GLM ceilings, Claude fallback).
pub fn plan_rows(
    config: &Config,
    http: &HttpState,
    sums: &[(&'static str, Sums)],
    paths: &Paths,
    now: i64,
) -> Vec<ProviderRow> {
    let mut rows = http.rows.clone();
    // a live GLM quota lane from the z.ai monitor API replaces the local
    // ceilings stopgap - real limits beat configured guesses
    let zai_live = rows.iter().any(|r| r.name.starts_with(GLM_LANE));

    if let Some(limits) = http
        .codex_wham
        .clone()
        // the API answers only when the lane is enabled and logged in; the
        // rollout-embedded limits are the always-there fallback
        .or_else(|| crate::codex::limits(&paths.codex_sessions, now))
    {
        rows.extend(codex_rows("Codex", &limits));
    }

    let provider_sums = |name: &str| {
        sums.iter()
            .find(|(n, _)| *n == name)
            .map(|(_, s)| *s)
            .unwrap_or_default()
    };
    let claude = provider_sums("Claude");
    if !zai_live {
        let glm_five = provider_sums("Zcode").five_hour + provider_sums("OpenCode").five_hour;
        let glm_week = provider_sums("Zcode").week + provider_sums("OpenCode").week;
        rows.extend(local_rows(
            GLM_LANE,
            glm_five,
            glm_week,
            config.glm_five_hour_tokens,
            config.glm_week_tokens,
        ));
    }
    match http.claude_oauth {
        Some((five, week)) => rows.extend(percent_rows("Claude", five, week)),
        None => rows.extend(local_rows(
            "Claude",
            claude.five_hour,
            claude.week,
            config.claude_five_hour_tokens,
            config.claude_week_tokens,
        )),
    }
    rows
}

fn resolve_key(cfg: Option<&Apikey>, env: &str) -> Option<String> {
    if let Some(key) = cfg.and_then(|a| a.api_key.as_deref()) {
        if !key.is_empty() {
            return Some(key.to_string());
        }
    }
    std::env::var(env).ok().filter(|k| !k.is_empty())
}

/// ---- HTTP plumbing -----------------------------------------------------------

pub(crate) fn http_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(10)))
        .http_status_as_error(false)
        .build()
        .new_agent()
}

fn get_json(url: &str, headers: &[(&str, String)]) -> Result<serde_json::Value, String> {
    let mut req = http_agent().get(url);
    for (name, value) in headers {
        req = req.header(*name, value.as_str());
    }
    let resp = req.call().map_err(|e| format!("network: {e}"))?;
    let status = resp.status().as_u16();
    let mut body = String::new();
    resp.into_body()
        .into_reader()
        .read_to_string(&mut body)
        .map_err(|e| format!("body: {e}"))?;
    if !(200..300).contains(&status) {
        return Err(format!("HTTP {status} {}", http_error_note(&body)));
    }
    serde_json::from_str(&body).map_err(|e| format!("json: {e}"))
}

/// One-line hint for a failed request: the API's own error message when the
/// body is JSON (`{"error":{"message": ...}}` or `{"message": ...}`), else
/// the collapsed body - short enough for one tile row.
fn http_error_note(body: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        for key in ["/error/message", "/message"] {
            if let Some(msg) = v.pointer(key).and_then(|m| m.as_str()) {
                if !msg.trim().is_empty() {
                    return truncate(msg.trim(), 48);
                }
            }
        }
    }
    let collapsed: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate(&collapsed, 48)
}

/// ---- built-in providers --------------------------------------------------------

/// OpenRouter key info: `GET /api/v1/key` -> `{data: {usage, limit}}`
/// (limit is null when no per-key spending cap is set).
fn openrouter_row(key: &str) -> ProviderRow {
    let auth = ("Authorization", format!("Bearer {key}"));
    match get_json("https://openrouter.ai/api/v1/key", &[auth]) {
        Ok(v) => {
            let usage = v.pointer("/data/usage").and_then(|x| x.as_f64());
            let limit = v.pointer("/data/limit").and_then(|x| x.as_f64());
            match usage {
                Some(used) => usd_row("OpenRouter", used, limit),
                None => error_row("OpenRouter", "no usage in response"),
            }
        }
        Err(e) => error_row("OpenRouter", &e),
    }
}

/// Anthropic Admin usage report: `GET /v1/organization/usage/report`
/// summed as tokens for the current UTC day (the report exposes no spend
/// limit, so the row shows consumption without a percentage).
fn anthropic_row(key: &str) -> ProviderRow {
    let now = crate::unix_now();
    let start = crate::local_usage::utc_day_start(now);
    let url = format!(
        "https://api.anthropic.com/v1/organization/usage/report?start_time={start}&end_time={now}"
    );
    let headers = [
        ("x-api-key", key.to_string()),
        ("anthropic-version", "2023-06-01".to_string()),
    ];
    match get_json(&url, &headers) {
        Ok(v) => {
            let tokens = anthropic_totals(&v);
            match tokens {
                Some(t) if t > 0 => ProviderRow {
                    name: "Anthropic API".into(),
                    state: "ok".into(),
                    text: format!("{} today", fmt_tokens(t)),
                    percent: None,
                    reset_at: None,
                },
                _ => error_row("Anthropic API", "no totals in response"),
            }
        }
        Err(e) => error_row("Anthropic API", &e),
    }
}

/// The report's `totals` is either a flat token object or a per-model
/// array; either way the row only needs the day's grand total.
fn anthropic_totals(v: &serde_json::Value) -> Option<u64> {
    let totals = v.get("totals")?;
    let fields = [
        "input_tokens",
        "output_tokens",
        "cache_creation_input_tokens",
    ];
    if let Some(map) = totals.as_object() {
        let sum: u64 = fields
            .iter()
            .map(|k| map.get(*k).and_then(|x| x.as_u64()).unwrap_or(0))
            .sum();
        return Some(sum);
    }
    Some(
        totals
            .as_array()?
            .iter()
            .map(|row| {
                fields
                    .iter()
                    .map(|k| row.get(*k).and_then(|x| x.as_u64()).unwrap_or(0))
                    .sum::<u64>()
            })
            .sum(),
    )
}

/// Config-declared endpoint: GET `url`, read `used_path` / `limit_path`
/// from the response. Numbers render with the optional unit.
fn custom_row(custom: &crate::CustomProvider) -> ProviderRow {
    let header_refs: Vec<(&str, String)> = custom
        .headers
        .iter()
        .map(|(k, v)| (k.as_str(), v.clone()))
        .collect();
    match get_json(&custom.url, &header_refs) {
        Ok(v) => {
            let used = json_path(&v, &custom.used_path).and_then(|x| x.as_f64());
            let limit = custom
                .limit_path
                .as_deref()
                .and_then(|p| json_path(&v, p))
                .and_then(|x| x.as_f64());
            match used {
                Some(used) => numeric_row(&custom.name, used, limit, custom.unit.as_deref()),
                None => error_row(&custom.name, "used path not found"),
            }
        }
        Err(e) => error_row(&custom.name, &e),
    }
}

/// ---- lanes as per-window bars ------------------------------------------------

const GLM_LANE: &str = "GLM";

/// One window of one provider as a bar row: the fill is the usage
/// percentage, the value text says how much is left and (when the source
/// reports it) when the window resets.
fn window_row(provider: &str, window: &str, percent: f64, reset_at: Option<i64>) -> ProviderRow {
    let percent = percent.clamp(0.0, 100.0);
    ProviderRow {
        name: format!("{provider} {window}"),
        state: threshold_state(Some(percent)).to_string(),
        text: limit_text(percent, reset_at, crate::unix_now()),
        percent: Some((percent * 10.0).round() / 10.0),
        reset_at,
    }
}

/// Value text of a window bar: remaining limit ("66% left"), plus the
/// countdown to the reset when the source carries one. `now` is injectable
/// so tests can pin the countdown wording.
fn limit_text(percent: f64, reset_at: Option<i64>, now: i64) -> String {
    let left = 100.0 - percent.clamp(0.0, 100.0);
    let left = if left.fract() == 0.0 {
        format!("{left:.0}%")
    } else {
        format!("{left:.1}%")
    };
    match reset_at.and_then(|r| countdown(r, now)) {
        Some(c) => format!("{left} left, reset {c}"),
        None => format!("{left} left"),
    }
}

/// Human countdown to a unix epoch: "1h 23m", "45m", "3d 4h"; zero tails
/// are dropped ("2h", "1w"), and a reset that already passed yields None -
/// the stale countdown is dropped until fresh data arrives.
fn countdown(reset_at: i64, now: i64) -> Option<String> {
    let secs = reset_at - now;
    if secs <= 0 {
        return None;
    }
    if secs < 60 {
        return Some("<1m".into());
    }
    let mins = secs / 60;
    if mins < 60 {
        return Some(format!("{mins}m"));
    }
    let hours = mins / 60;
    if hours < 24 {
        return Some(match mins % 60 {
            0 => format!("{hours}h"),
            m => format!("{hours}h {m}m"),
        });
    }
    let days = hours / 24;
    if days < 7 {
        return Some(match hours % 24 {
            0 => format!("{days}d"),
            h => format!("{days}d {h}h"),
        });
    }
    Some(match days % 7 {
        0 => format!("{}w", days / 7),
        d => format!("{}w {d}d", days / 7),
    })
}

/// Codex rows from the rate limits embedded in the rollout files: one bar
/// per reported window (primary is the session lane, secondary the weekly
/// one; labels come from the API's own window minutes). Without windows
/// the plan name is all the API gives up.
fn codex_rows(name: &str, limits: &crate::codex::Limits) -> Vec<ProviderRow> {
    let mut rows = Vec::new();
    if let Some(primary) = &limits.primary {
        rows.push(window_row(
            name,
            &primary.window_label(),
            primary.used_percent,
            primary.resets_at,
        ));
    }
    if let Some(secondary) = &limits.secondary {
        rows.push(window_row(
            name,
            &secondary.window_label(),
            secondary.used_percent,
            secondary.resets_at,
        ));
    }
    if rows.is_empty() {
        rows.push(ProviderRow {
            name: name.to_string(),
            state: "ok".into(),
            text: format!("{} plan", limits.plan_type.as_deref().unwrap_or("limited")),
            percent: None,
            reset_at: None,
        });
    }
    rows
}

/// GLM Coding Plan via the z.ai monitor quota endpoint (CodexBar's
/// mapping): `data.limits[]` entries of type TOKENS_LIMIT or
/// CREDIT_LIMIT, each with `unit`+`number` for the window and a
/// percentage the API computes itself.
fn zai_rows(key: &str, host: Option<&str>) -> Vec<ProviderRow> {
    let base = match zai_base_url(host) {
        Ok(base) => base,
        Err(e) => return vec![error_row(GLM_LANE, &e)],
    };
    let url = format!("{base}/api/monitor/usage/quota/limit");
    let headers = [
        ("Authorization", format!("Bearer {key}")),
        ("accept", "application/json".to_string()),
    ];
    let v = match get_json(&url, &headers) {
        Ok(v) => v,
        Err(e) => return vec![error_row(GLM_LANE, &e)],
    };
    zai_rows_from(v)
}

/// Base URL for the z.ai monitor endpoint. `host` may be a bare host or
/// a full URL; plain `http://` is refused for anything but loopback -
/// the request carries the Bearer key, which must not travel in
/// plaintext off-machine.
fn zai_base_url(host: Option<&str>) -> std::result::Result<String, String> {
    let Some(h) = host.map(str::trim).filter(|h| !h.is_empty()) else {
        return Ok("https://api.z.ai".into());
    };
    let base = if h.starts_with("http://") || h.starts_with("https://") {
        h.trim_end_matches('/').to_string()
    } else {
        format!("https://{h}")
    };
    if let Some(rest) = base.strip_prefix("http://") {
        if !is_loopback_host(hostname_of(rest)) {
            return Err("plain http:// host refused (bearer key would be sent unencrypted)".into());
        }
    }
    Ok(base)
}

/// Hostname of a URL string's authority part (path and port dropped,
/// bracketed IPv6 kept intact).
fn hostname_of(authority: &str) -> &str {
    let authority = authority.split('/').next().unwrap_or_default();
    if let Some(stripped) = authority.strip_prefix('[') {
        return stripped.split(']').next().unwrap_or_default();
    }
    authority.split(':').next().unwrap_or_default()
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost") || host == "::1" || host.starts_with("127.")
}

/// Map a quota response onto bar rows; split from the HTTP call so tests
/// can feed fixtures directly.
fn zai_rows_from(v: serde_json::Value) -> Vec<ProviderRow> {
    if v.get("success").and_then(|s| s.as_bool()) != Some(true) {
        let msg = v
            .get("msg")
            .and_then(|m| m.as_str())
            .unwrap_or("quota request rejected");
        return vec![error_row(GLM_LANE, msg)];
    }
    let Some(limits) = v.pointer("/data/limits").and_then(|l| l.as_array()) else {
        return vec![error_row(GLM_LANE, "no limits in response")];
    };
    let plan = ["level", "planName", "plan_type", "packageName"]
        .iter()
        .find_map(|k| {
            v.pointer("/data")
                .and_then(|d| d.get(k))
                .and_then(|p| p.as_str())
                .map(str::to_string)
        });
    let mut windows: Vec<(u64, ProviderRow)> =
        limits.iter().filter_map(|raw| zai_window(raw)).collect();
    windows.sort_by_key(|(minutes, _)| *minutes);
    if windows.is_empty() {
        return vec![ProviderRow {
            name: GLM_LANE.into(),
            state: "ok".into(),
            text: match &plan {
                Some(p) => format!("{p} plan"),
                None => "no quota windows".into(),
            },
            percent: None,
            reset_at: None,
        }];
    }
    windows.into_iter().map(|(_, row)| row).collect()
}

/// One `data.limits[]` entry -> (window_minutes, row); unsupported types
/// and window shapes are skipped, mirroring the CodexBar parser.
fn zai_window(raw: &serde_json::Value) -> Option<(u64, ProviderRow)> {
    let kind = raw.get("type")?.as_str()?;
    if kind != "TOKENS_LIMIT" && kind != "CREDIT_LIMIT" {
        return None;
    }
    let unit = raw.get("unit")?.as_u64()?;
    let number = raw.get("number")?.as_u64()?;
    // unit codes: 1 day, 3 hour, 5 minute, 6 week
    let multiplier = match unit {
        1 => 1440,
        3 => 60,
        5 => 1,
        6 => 10_080,
        _ => return None,
    };
    let minutes = number.checked_mul(multiplier)?;
    let api_percent = raw.get("percentage")?.as_f64()?;
    let usage = raw.get("usage").and_then(|u| u.as_f64());
    let current = raw.get("currentValue").and_then(|c| c.as_f64());
    let remaining = raw.get("remaining").and_then(|r| r.as_f64());
    let mut percent = api_percent;
    // the counts exist only to keep the percentage honest: the API's own
    // figure can be stale or zeroed on credit plans
    if let Some(usage) = usage.filter(|u| *u > 0.0) {
        let used = match (remaining, current) {
            (Some(remaining), Some(current)) => Some((usage - remaining).max(current)),
            (Some(remaining), None) => Some(usage - remaining),
            (None, Some(current)) => Some(current),
            _ => None,
        };
        if let Some(used) = used {
            percent = 100.0 * used.clamp(0.0, usage) / usage;
        }
    }
    let window = crate::codex::RateLimit {
        used_percent: 0.0,
        window_minutes: minutes,
        resets_at: None,
    }
    .window_label();
    Some((minutes, window_row(GLM_LANE, &window, percent, None)))
}

/// Local lanes from token sums, one row per window: a window with a
/// configured ceiling becomes a percentage bar, without a ceiling the raw
/// sum is all there is (no invented percentages).
fn local_rows(
    name: &str,
    five_hour: u64,
    week: u64,
    five_hour_ceiling: Option<u64>,
    week_ceiling: Option<u64>,
) -> Vec<ProviderRow> {
    let mut rows = Vec::new();
    for (window, used, ceiling) in [
        ("5h", five_hour, five_hour_ceiling),
        ("week", week, week_ceiling),
    ] {
        if used == 0 && ceiling.filter(|c| *c > 0).is_none() {
            continue; // nothing used and no limit known: nothing to show
        }
        match ceiling.filter(|c| *c > 0) {
            Some(ceiling) => rows.push(window_row(
                name,
                window,
                100.0 * used as f64 / ceiling as f64,
                None,
            )),
            None => rows.push(ProviderRow {
                name: format!("{name} {window}"),
                state: "ok".into(),
                text: fmt_tokens(used),
                percent: None,
                reset_at: None,
            }),
        }
    }
    if rows.is_empty() {
        rows.push(ProviderRow {
            name: name.to_string(),
            state: "ok".into(),
            text: "no usage".into(),
            percent: None,
            reset_at: None,
        });
    }
    rows
}

/// Claude via OAuth: per-window bar rows; utilization values arrive as
/// 0..1 fractions or straight percentages, both normalized here.
fn percent_rows(name: &str, five_hour: Option<f64>, week: Option<f64>) -> Vec<ProviderRow> {
    let normalize = |v: Option<f64>| v.map(|p| if p <= 1.0 { p * 100.0 } else { p });
    let five = normalize(five_hour);
    let week = normalize(week);
    let mut rows = Vec::new();
    if let Some(p) = five {
        rows.push(window_row(name, "5h", p, None));
    }
    if let Some(p) = week {
        rows.push(window_row(name, "week", p, None));
    }
    if rows.is_empty() {
        rows.push(ProviderRow {
            name: name.to_string(),
            state: "ok".into(),
            text: "no data".into(),
            percent: None,
            reset_at: None,
        });
    }
    rows
}

/// What the CLI's credentials file yields for the OAuth usage lane: the
/// access token and whether it has already expired.
pub(crate) struct ClaudeOauthCredentials {
    pub(crate) token: String,
    pub(crate) expired: bool,
}

/// Extract the Claude CLI's OAuth token from the credentials file JSON.
/// The file nests the grant under `claudeAiOauth` with camelCase keys
/// (`accessToken`, `expiresAt`, `scopes`); older writers used snake_case
/// `access_token`, kept as fallback. `expiresAt` is ms since epoch - a
/// past expiry means the usage call would 401, so the credentials are
/// flagged and the caller skips the call. A scope list without
/// `user:profile` cannot see usage at all.
pub(crate) fn claude_oauth_credentials(
    v: &serde_json::Value,
    now_ms: i64,
) -> Option<ClaudeOauthCredentials> {
    let oauth = v.get("claudeAiOauth").unwrap_or(v);
    if let Some(scopes) = oauth.get("scopes").and_then(|s| s.as_array()) {
        let has_profile = scopes
            .iter()
            .filter_map(|s| s.as_str())
            .any(|s| s == "user:profile");
        if !has_profile {
            return None; // token cannot see usage; fall back to ceilings
        }
    }
    let token = oauth
        .get("accessToken")
        .or_else(|| oauth.get("access_token"))?
        .as_str()?;
    if token.is_empty() {
        return None;
    }
    let expired = oauth
        .get("expiresAt")
        .and_then(|e| e.as_i64())
        .map(|ms| ms <= now_ms)
        .unwrap_or(false);
    Some(ClaudeOauthCredentials {
        token: token.to_string(),
        expired,
    })
}

/// Claude OAuth usage: `GET /api/oauth/usage` with the CLI's own bearer
/// token; utilization values arrive as 0..1 fractions or straight
/// percentages, both normalized by [`percent_rows`]. Any failure just
/// drops the lane and the tile falls back to local sums.
fn claude_oauth_usage(paths: &Paths) -> Option<(Option<f64>, Option<f64>)> {
    let credentials = paths.claude_projects.parent()?.join(".credentials.json");
    let text = std::fs::read_to_string(credentials).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let creds = claude_oauth_credentials(&v, crate::unix_now() * 1000)?;
    if creds.expired {
        tracing::debug!("claude oauth token expired, skipping usage api");
        return None;
    }
    let headers = [
        ("Authorization", format!("Bearer {}", creds.token)),
        ("anthropic-beta", "oauth-2025-04-20".to_string()),
    ];
    // the error text carries only the HTTP status, the API's own message
    // or a transport error - never the token or the auth header
    let v = match get_json("https://api.anthropic.com/api/oauth/usage", &headers) {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!(error = %e, "claude oauth usage call failed, using local sums");
            return None;
        }
    };
    let lane = |key: &str| {
        v.get(key)
            .and_then(|l| l.get("utilization"))
            .and_then(|u| u.as_f64())
    };
    Some((lane("five_hour"), lane("seven_day")))
}

/// ---- Codex wham/usage lane -----------------------------------------------------

/// Codex OAuth usage lane: read the CLI's own `auth.json` and poll
/// `wham/usage` so limits and resets stay fresh even when no Codex session
/// has run lately. Read-only on the token file - the CLI owns it and a
/// parallel refresh would corrupt its token rotation - so any failure just
/// drops the lane and the rollout-embedded limits keep the bar alive.
fn codex_wham_limits(config: &Config, paths: &Paths) -> Option<crate::codex::Limits> {
    let provider = config.providers.codex.as_ref()?;
    let home = match provider.home.as_deref().filter(|h| !h.is_empty()) {
        Some(h) => std::path::PathBuf::from(h),
        None => paths.codex_sessions.parent()?.to_path_buf(),
    };
    let (token, account) = codex_auth(&home)?;
    let mut headers = vec![
        ("Authorization", format!("Bearer {token}")),
        ("accept", "application/json".to_string()),
        ("User-Agent", "pulpit-aidev".to_string()),
    ];
    if let Some(acc) = account {
        headers.push(("ChatGPT-Account-Id", acc));
    }
    let v = match get_json("https://chatgpt.com/backend-api/wham/usage", &headers) {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!(error = %e, "codex wham/usage failed, using rollout limits");
            return None;
        }
    };
    let limits = codex_wham_limits_from(v);
    if limits.is_none() {
        tracing::debug!("codex wham/usage response had no rate_limit windows");
    }
    limits
}

/// `tokens.access_token` + `tokens.account_id` from the Codex CLI's
/// credential file. No login, no refresh: the lane exists only while the
/// CLI's own tokens are valid.
fn codex_auth(codex_home: &std::path::Path) -> Option<(String, Option<String>)> {
    let text = std::fs::read_to_string(codex_home.join("auth.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let tokens = v.get("tokens")?;
    let access = tokens.get("access_token")?.as_str()?;
    let account = tokens
        .get("account_id")
        .and_then(|x| x.as_str())
        .map(str::to_string);
    Some((access.to_string(), account))
}

/// Map a wham/usage response onto `Limits`; split from the HTTP call so
/// tests can feed fixtures directly. Window shape: `used_percent`,
/// `reset_at` (epoch seconds) and `limit_window_seconds` (5h session and
/// one-week windows on paid plans).
fn codex_wham_limits_from(v: serde_json::Value) -> Option<crate::codex::Limits> {
    let rl = v.get("rate_limit")?;
    let window = |key: &str| {
        let w = rl.get(key)?;
        let used_percent = w.get("used_percent")?.as_f64()?;
        let resets_at = w
            .get("reset_at")
            .or_else(|| w.get("resets_at"))
            .and_then(|x| x.as_i64());
        let window_minutes = w
            .get("limit_window_seconds")
            .and_then(|x| x.as_u64())
            .map(|s| s / 60)
            .unwrap_or(0);
        Some(crate::codex::RateLimit {
            used_percent,
            window_minutes,
            resets_at,
        })
    };
    Some(crate::codex::Limits {
        plan_type: v
            .get("plan_type")
            .and_then(|x| x.as_str())
            .map(str::to_string),
        primary: window("primary_window"),
        secondary: window("secondary_window"),
    })
}

/// Antigravity lane from the running IDE's local quota endpoint, as
/// per-window bar rows.
fn antigravity_quota_rows() -> Vec<ProviderRow> {
    let Some(quota) = crate::antigravity::quota() else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    if let Some(p) = quota.five_hour_used {
        rows.push(window_row("Antigravity", "5h", p, None));
    }
    if let Some(p) = quota.weekly_used {
        rows.push(window_row("Antigravity", "week", p, None));
    }
    rows
}

/// ---- row shaping -----------------------------------------------------------

fn usd_row(name: &str, used: f64, limit: Option<f64>) -> ProviderRow {
    numeric_row(name, used, limit, Some("$"))
}

fn numeric_row(name: &str, used: f64, limit: Option<f64>, unit: Option<&str>) -> ProviderRow {
    let suffix = unit.unwrap_or("");
    let fmt = |v: f64| {
        if unit == Some("$") {
            format!("{suffix}{v:.2}")
        } else {
            format!("{v:.1} {suffix}").trim().to_string()
        }
    };
    let percent = limit.filter(|l| *l > 0.0).map(|l| 100.0 * used / l);
    let text = match limit {
        Some(l) => format!("used {} / {}", fmt(used), fmt(l)),
        None => format!("used {}", fmt(used)),
    };
    ProviderRow {
        name: name.to_string(),
        state: threshold_state(percent).to_string(),
        text,
        percent: percent.map(|p| (p * 10.0).round() / 10.0),
        reset_at: None,
    }
}

fn error_row(name: &str, error: &str) -> ProviderRow {
    ProviderRow {
        name: name.to_string(),
        state: "error".into(),
        text: error.to_string(),
        percent: None,
        reset_at: None,
    }
}

pub(crate) fn threshold_state(percent: Option<f64>) -> &'static str {
    match percent {
        Some(p) if p >= 85.0 => "high",
        Some(p) if p >= 60.0 => "warn",
        Some(_) => "ok",
        None => "ok",
    }
}

/// Dotted JSON path with numeric segments indexing arrays
/// (`data.rows.0.used`).
fn json_path<'a>(v: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let mut cur = v;
    for seg in path.split('.') {
        cur = match seg.parse::<usize>() {
            Ok(idx) => cur.get(idx)?,
            Err(_) => cur.get(seg)?,
        };
    }
    Some(cur)
}

/// ---- local transcript rows ---------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usd_row_states_by_threshold() {
        let r = usd_row("X", 4.0, Some(10.0));
        assert_eq!(r.state, "ok");
        assert_eq!(r.text, "used $4.00 / $10.00");
        assert_eq!(r.percent, Some(40.0));

        let r = usd_row("X", 7.0, Some(10.0));
        assert_eq!(r.state, "warn");

        let r = usd_row("X", 9.0, Some(10.0));
        assert_eq!(r.state, "high");

        let r = usd_row("X", 4.0, None);
        assert_eq!(r.state, "ok");
        assert_eq!(r.text, "used $4.00");
        assert_eq!(r.percent, None);
    }

    #[test]
    fn json_path_walks_objects_and_arrays() {
        let v: serde_json::Value =
            serde_json::from_str(r#"{"data":{"rows":[{"used":1},{"used":2}]}}"#).unwrap();
        assert_eq!(
            json_path(&v, "data.rows.1.used").and_then(|x| x.as_i64()),
            Some(2)
        );
        assert!(json_path(&v, "data.nope").is_none());
    }

    #[test]
    fn openrouter_response_shapes_a_row() {
        let v: serde_json::Value =
            serde_json::from_str(r#"{"data":{"label":"k","usage":4.0,"limit":8.0}}"#).unwrap();
        let used = v.pointer("/data/usage").and_then(|x| x.as_f64()).unwrap();
        let limit = v.pointer("/data/limit").and_then(|x| x.as_f64());
        let row = usd_row("OpenRouter", used, limit);
        assert_eq!(row.percent, Some(50.0));
        assert_eq!(row.text, "used $4.00 / $8.00");
    }

    #[test]
    fn custom_unit_not_dollars() {
        let r = numeric_row("Credits", 3.0, Some(10.0), Some("cr"));
        assert_eq!(r.text, "used 3.0 cr / 10.0 cr");
    }

    #[test]
    fn http_error_note_prefers_the_api_message() {
        let body = r#"{ "error": { "message": "Incorrect API key provided: sk-proj-abc123, but the key was issued to a different org." } }"#;
        let note = http_error_note(body);
        assert!(note.starts_with("Incorrect API key provided"));
        assert!(note.chars().count() <= 48);

        // non-JSON bodies collapse to one short line
        assert_eq!(
            http_error_note("Bad  gateway\n  try again"),
            "Bad gateway try again"
        );
        // top-level message shape also works
        assert_eq!(
            http_error_note(r#"{"message":"quota exceeded"}"#),
            "quota exceeded"
        );
    }

    #[test]
    fn local_rows_bar_ceilinged_windows() {
        // no ceilings: raw sums per window, no invented percentage
        let rows = local_rows("GLM", 100_000, 1_000_000, None, None);
        let labels: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(labels, ["GLM 5h", "GLM week"]);
        assert_eq!(rows[0].text, "100.0K");
        assert_eq!(rows[0].percent, None);

        // a ceiling turns its window into a percentage bar
        let rows = local_rows("GLM", 100_000, 1_000_000, Some(1_000_000), None);
        assert_eq!(rows[0].name, "GLM 5h");
        assert_eq!(rows[0].text, "90% left");
        assert_eq!(rows[0].percent, Some(10.0));
        assert_eq!(rows[1].text, "1.0M");

        // both ceilings: each window gets its own bar and its own dot
        let rows = local_rows("GLM", 100_000, 950_000, Some(1_000_000), Some(1_000_000));
        assert_eq!(rows[0].text, "90% left");
        assert_eq!(rows[1].text, "5% left");
        assert_eq!(rows[1].state, "high");

        // zero usage without a ceiling collapses to a quiet row
        let rows = local_rows("GLM", 0, 0, None, None);
        assert_eq!(rows[0].text, "no usage");

        // a ceiling with zero usage still shows the limit exists
        let rows = local_rows("GLM", 0, 0, Some(1_000_000), None);
        assert_eq!(rows[0].text, "100% left");
        assert_eq!(rows[0].percent, Some(0.0));
    }

    #[test]
    fn limit_text_speaks_in_remaining_and_resets() {
        // no reset: the remaining share is the whole message
        assert_eq!(limit_text(34.0, None, 0), "66% left");
        // sub-10% remaining keeps one decimal
        assert_eq!(limit_text(93.4, None, 0), "6.6% left");
        assert_eq!(limit_text(100.0, None, 0), "0% left");
        assert_eq!(limit_text(0.0, None, 0), "100% left");

        let now = 1_800_000_000;
        assert_eq!(
            limit_text(34.0, Some(now + 3600), now),
            "66% left, reset 1h"
        );
        assert_eq!(
            limit_text(34.0, Some(now + 3600 + 60 * 23), now),
            "66% left, reset 1h 23m"
        );
        assert_eq!(
            limit_text(34.0, Some(now + 60 * 45), now),
            "66% left, reset 45m"
        );
        assert_eq!(limit_text(34.0, Some(now + 30), now), "66% left, reset <1m");
        assert_eq!(
            limit_text(34.0, Some(now + 86_400 * 3), now),
            "66% left, reset 3d"
        );
        assert_eq!(
            limit_text(34.0, Some(now + 86_400 * 3 + 3600 * 4), now),
            "66% left, reset 3d 4h"
        );
        assert_eq!(
            limit_text(34.0, Some(now + 86_400 * 14), now),
            "66% left, reset 2w"
        );
        // a reset in the past drops the stale countdown instead of lying
        assert_eq!(limit_text(34.0, Some(now - 10), now), "66% left");
    }

    #[test]
    fn window_rows_carry_the_bar_data() {
        let now = crate::unix_now();
        let rows = codex_rows(
            "Codex",
            &crate::codex::Limits {
                plan_type: Some("pro".into()),
                primary: Some(crate::codex::RateLimit {
                    used_percent: 65.0,
                    window_minutes: 300,
                    resets_at: Some(now + 3600 * 2),
                }),
                secondary: Some(crate::codex::RateLimit {
                    used_percent: 12.0,
                    window_minutes: 10_080,
                    resets_at: None,
                }),
            },
        );
        // one bar row per window, API-provided window labels; the value
        // text says what is left and when the window resets
        let labels: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(labels, ["Codex 5h", "Codex week"]);
        assert_eq!(rows[0].text, "35% left, reset 2h");
        assert_eq!(rows[0].percent, Some(65.0));
        assert_eq!(rows[0].state, "warn");
        assert_eq!(rows[0].reset_at, Some(now + 3600 * 2));
        assert_eq!(rows[1].percent, Some(12.0));
        assert_eq!(rows[1].text, "88% left");

        // a plan with no windows still shows something readable
        let bare = crate::codex::Limits {
            plan_type: Some("free".into()),
            primary: None,
            secondary: None,
        };
        assert_eq!(codex_rows("Codex", &bare)[0].text, "free plan");
    }

    #[test]
    fn wham_usage_response_maps_to_limits() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"plan_type":"pro","rate_limit":{
                "primary_window":{"used_percent":34,"reset_at":1790000000,"limit_window_seconds":18000},
                "secondary_window":{"used_percent":12,"reset_at":1790900000,"limit_window_seconds":604800}
            }}"#,
        )
        .unwrap();
        let limits = codex_wham_limits_from(v).expect("windows in response");
        assert_eq!(limits.plan_type.as_deref(), Some("pro"));
        let primary = limits.primary.expect("primary window");
        assert_eq!(primary.used_percent, 34.0);
        assert_eq!(primary.window_label(), "5h");
        assert_eq!(primary.resets_at, Some(1_790_000_000));
        let secondary = limits.secondary.expect("secondary window");
        assert_eq!(secondary.window_label(), "week");
        assert_eq!(secondary.resets_at, Some(1_790_900_000));

        // a response without rate_limit windows drops the lane
        assert!(codex_wham_limits_from(serde_json::json!({"plan_type":"free"})).is_none());
    }

    #[test]
    fn codex_auth_reads_token_and_account() {
        let dir = std::env::temp_dir().join(format!("aidev-wham-auth-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("auth.json"),
            r#"{"tokens":{"access_token":"abc","refresh_token":"r","account_id":"acc-1"},"last_refresh":"2026-09-20T00:00:00Z"}"#,
        )
        .unwrap();
        let (token, account) = codex_auth(&dir).expect("auth file parsed");
        assert_eq!(token, "abc");
        assert_eq!(account.as_deref(), Some("acc-1"));

        // no auth.json: the lane stays off
        assert!(codex_auth(&dir.join("nope")).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn claude_oauth_credentials_read_camel_case_keys() {
        // fixture shaped like the CLI's credentials file: claudeAiOauth
        // with camelCase keys and an obvious fake token
        let v: serde_json::Value = serde_json::from_str(
            r#"{"claudeAiOauth":{
                "accessToken":"test-oauth-token",
                "refreshToken":"test-refresh-token",
                "expiresAt":1900000000000,
                "scopes":["user:profile","user:inference"]
            }}"#,
        )
        .unwrap();
        let now_ms = 1_800_000_000_000;
        let creds = claude_oauth_credentials(&v, now_ms).expect("credentials parsed");
        assert_eq!(creds.token, "test-oauth-token");
        assert!(!creds.expired);

        // a past expiry must flag the credentials so the caller skips the call
        let creds = claude_oauth_credentials(&v, 1_900_000_000_000).expect("credentials parsed");
        assert!(creds.expired);

        // older writers used snake_case access_token
        let v: serde_json::Value = serde_json::from_str(
            r#"{"claudeAiOauth":{"access_token":"test-oauth-token","scopes":["user:profile"]}}"#,
        )
        .unwrap();
        let creds = claude_oauth_credentials(&v, 0).expect("credentials parsed");
        assert_eq!(creds.token, "test-oauth-token");

        // a scope list without user:profile cannot see usage
        let v: serde_json::Value = serde_json::from_str(
            r#"{"claudeAiOauth":{"accessToken":"test-oauth-token","scopes":["user:inference"]}}"#,
        )
        .unwrap();
        assert!(claude_oauth_credentials(&v, 0).is_none());

        // no scope list at all: the grant is unknown, the call decides
        let v: serde_json::Value =
            serde_json::from_str(r#"{"claudeAiOauth":{"accessToken":"test-oauth-token"}}"#)
                .unwrap();
        assert!(claude_oauth_credentials(&v, 0).is_some());

        // no token in either spelling
        let v: serde_json::Value = serde_json::from_str(r#"{"claudeAiOauth":{}}"#).unwrap();
        assert!(claude_oauth_credentials(&v, 0).is_none());
    }

    #[test]
    fn percent_rows_normalize_fractions_and_percentages() {
        let rows = percent_rows("Claude", Some(0.42), Some(75.0));
        assert_eq!(rows[0].name, "Claude 5h");
        assert_eq!(rows[0].percent, Some(42.0));
        assert_eq!(rows[1].percent, Some(75.0));
    }

    #[test]
    fn zai_windows_map_units_and_counts() {
        // a coding plan reports a 5h token window and a weekly one; the
        // API's own percentage wins when the counts are unusable
        let limits: serde_json::Value = serde_json::from_str(
            r#"{"success":true,"code":200,"data":{"level":"pro","limits":[
                {"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":34,
                 "usage":120000,"currentValue":40800},
                {"type":"TOKENS_LIMIT","unit":6,"number":1,"percentage":0,
                 "usage":600000,"remaining":540000,"currentValue":0},
                {"type":"TIME_LIMIT","unit":3,"number":1,"percentage":10}
            ]}}"#,
        )
        .unwrap();
        let rows = zai_rows_from(limits);
        // TIME_LIMIT is not a coding-plan window; windows sort by length
        let labels: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(labels, ["GLM 5h", "GLM week"]);
        // rows speak in the remaining share; the counts only keep them honest
        assert_eq!(rows[0].text, "66% left");
        assert_eq!(rows[0].percent, Some(34.0));
        // remaining is honored when currentValue is zeroed
        assert_eq!(rows[1].text, "90% left");
        assert_eq!(rows[1].percent, Some(10.0));
    }

    #[test]
    fn zai_base_url_refuses_plaintext_non_loopback_hosts() {
        // defaults and bare hosts upgrade to https
        assert_eq!(zai_base_url(None).unwrap(), "https://api.z.ai");
        assert_eq!(
            zai_base_url(Some(" open.bigmodel.cn ")).unwrap(),
            "https://open.bigmodel.cn"
        );
        assert_eq!(
            zai_base_url(Some("https://api.z.ai/")).unwrap(),
            "https://api.z.ai"
        );
        // plain http would send the Bearer key in plaintext: loopback only
        assert!(zai_base_url(Some("http://api.z.ai")).is_err());
        assert!(zai_base_url(Some("http://example.com/x")).is_err());
        assert_eq!(
            zai_base_url(Some("http://127.0.0.1:8080")).unwrap(),
            "http://127.0.0.1:8080"
        );
        assert_eq!(
            zai_base_url(Some("http://localhost:9/")).unwrap(),
            "http://localhost:9"
        );
        assert_eq!(zai_base_url(Some("http://[::1]")).unwrap(), "http://[::1]");
    }

    #[test]
    fn zai_errors_and_empty_plans_stay_readable() {
        let rejected: serde_json::Value =
            serde_json::from_str(r#"{"success":false,"code":401,"msg":"invalid api key"}"#)
                .unwrap();
        let rows = zai_rows_from(rejected);
        assert_eq!(rows[0].state, "error");
        assert_eq!(rows[0].text, "invalid api key");

        let no_limits: serde_json::Value = serde_json::from_str(
            r#"{"success":true,"code":200,"data":{"level":"lite","limits":[]}}"#,
        )
        .unwrap();
        let rows = zai_rows_from(no_limits);
        assert_eq!(rows[0].text, "lite plan");
        assert_eq!(rows[0].percent, None);
    }

    #[test]
    fn plan_rows_combine_http_and_local_lanes() {
        let config = Config::default();
        let http = HttpState {
            rows: vec![ProviderRow {
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
        let sums = [
            (
                "Zcode",
                Sums {
                    today: 500_000,
                    ..Default::default()
                },
            ),
            (
                "Claude",
                Sums {
                    five_hour: 100_000,
                    week: 1_000_000,
                    ..Default::default()
                },
            ),
            (
                "OpenCode",
                Sums {
                    five_hour: 50_000,
                    ..Default::default()
                },
            ),
        ];
        let rows = plan_rows(&config, &http, &sums, &paths, crate::unix_now());
        // no codex sessions on this fake path: the lane stays absent; the
        // no-ceiling GLM sums show raw tokens, the empty Claude week is dropped
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["OpenRouter", "GLM 5h", "Claude 5h", "Claude week"]);
        let glm = rows.iter().find(|r| r.name == "GLM 5h").unwrap();
        // zcode + opencode together
        assert_eq!(glm.text, "50.0K");

        // a live GLM quota lane replaces the local ceilings lane
        let http = HttpState {
            rows: vec![
                ProviderRow {
                    name: "GLM 5h".into(),
                    state: "ok".into(),
                    text: "66% left".into(),
                    percent: Some(34.0),
                    reset_at: None,
                },
                ProviderRow {
                    name: "GLM week".into(),
                    state: "ok".into(),
                    text: "90% left".into(),
                    percent: Some(10.0),
                    reset_at: None,
                },
            ],
            claude_oauth: None,
            codex_wham: None,
        };
        let rows = plan_rows(&config, &http, &sums, &paths, crate::unix_now());
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["GLM 5h", "GLM week", "Claude 5h", "Claude week"]);
    }
}
