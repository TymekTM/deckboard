# AI dev work source (`deckboard-aidev`)

Native read-only display tiles for AI coding-agent activity: plan limits,
agent progress and token burn. Ships four catalog entries under
**AI dev work**:

| Tile | Mode | Shows |
|---|---|---|
| `ai-plan-limits` | `status` | One bar row per provider window (5h, week): fill is the usage percentage, the value says how much is left and - when the source reports it - when the window resets |
| `ai-agent-status` | `status` | Active agents grouped by project: provider glyph, session title, state |
| `ai-tokens-today` | `graph` | Sparkline of tokens consumed today, all providers combined |
| `ai-tokens-hour` | `graph` | Rolling last-60-minutes window, refreshed with every poll (~15 s); a tap flips it to the per-provider breakdown |

Data is produced by a background thread in the desktop app (and the
headless server), pushed as `APP_CUSTOM_VALUE` like every other live
value, and also fed to protocol v2 channels `ext.ai-*`. Presses on the
tiles are no-ops except the hour tile's tap, which toggles its breakdown
client-side.

## Providers

Five coding agents are understood; a provider appears on the agents tile
only while it has at least one working or attention session, and a plan
lane only when its limit source is available:

| Provider | Sessions read from | Token usage | Limits |
|---|---|---|---|
| `zcode` | `~/.zcode/cli/db/db.sqlite` (`session` table: title, directory, `time_updated`; main conversations only) | rollout JSONLs `~/.zcode/cli/rollout/` | GLM Coding Plan quota from the z.ai monitor API (`providers.zai`); falls back to local sums vs `glm_five_hour_tokens` / `glm_week_tokens` ceilings |
| `claude` | `~/.claude/projects/<project>/*.jsonl` mtimes; the freshest transcript's head gives the real cwd and opening prompt | the same JSONLs (`message.usage`) | OAuth `api.anthropic.com/api/oauth/usage` with the CLI's own token (`user:profile` scope); falls back to `claude_five_hour_tokens` / `claude_week_tokens` |
| `codex` | `~/.codex/sessions/YYYY/MM/DD/*.jsonl` (`session_meta` → cwd, first `user_message` → title) | the same rollouts (`token_count` events, `last_token_usage` deltas) | the `rate_limits` embedded in every `token_count` event - no API call |
| `opencode` | `opencode.db` `session` table (title, directory, `time_updated`) | the `message` table's assistant rows (`tokens` object) | shares the GLM ceilings |
| `antigravity` | `~/.gemini/antigravity/conversations/*.db` file mtimes | protobuf `gen_metadata` blobs (CodexBar field layout: 1+2 input, 5 cache read, 9+10 output), stamped with the `steps` row's Timestamp | the running IDE's local `RetrieveUserQuotaSummary` endpoint (port discovered by probing Antigravity's loopback listeners) |

ZCode fallback: when the journal cannot be opened, the rollout file mtimes
still drive liveness (id fragments instead of titles). Claude and Codex
rows fall back to id fragments too when their transcripts carry no usable
prompt (SDK sessions, wrapper content).

OpenRouter (`OPENROUTER_API_KEY`), the Anthropic admin report
(`ANTHROPIC_API_KEY`) and arbitrary config-declared endpoints remain
opt-in extra lanes. The old OpenAI Costs lane is gone - consumer keys are
not accepted there and Codex usage is subscription-based.

## Plan-limit bars

Every window renders as its own row: label (`GLM 5h`), the bar whose fill
is the usage percentage (colored by the threshold palette: green ok, amber
over 60%, red over 85%), and a value text spoken in the remaining limit -
`67% left`, or `35% left, reset 1h 23m` when the source carries a reset
time (a reset that already passed drops its countdown until fresh data
arrives). Providers whose sources expose only raw sums keep the sum as the
value; nothing invents a percentage.

Reset times come only from sources that report them: Codex (the
`resets_at` embedded in every rollout `token_count` event, or the
`reset_at` of the optional `wham/usage` lane). Claude's OAuth usage API,
the z.ai monitor endpoint and Antigravity's quota endpoint carry
utilization only, so their rows have no countdown.

Percentage sources per provider: Codex's embedded `rate_limits` (or the
opt-in `wham/usage` API lane, which wins when it answers - real limits
beat locally cached ones), Claude OAuth utilization, Antigravity's quota
endpoint, and the z.ai monitor API for GLM (CodexBar's mapping:
`data.limits[]`, TOKENS_LIMIT / CREDIT_LIMIT windows; `usage` +
`currentValue`/`remaining` counts only refine the percentage the API
reports). Without any of those, GLM and Claude fall back to token sums
against ceilings configured in `~/deckboard/aidev.json`
(`glm_five_hour_tokens` / `glm_week_tokens` and the `claude_*` pair) - a
ceiling turns the local sum into a percentage bar. With neither source nor
ceiling there is no percentage to show, so the row stays a raw sum; ZCode
does not expose its plan quota locally (its own protocol, credentials stay
in the encrypted keystore).

The optional Codex API lane reads the CLI's own `~/.codex/auth.json`
(`tokens.access_token` + `tokens.account_id`) and polls
`https://chatgpt.com/backend-api/wham/usage` at the `http_poll_secs`
cadence; it is enabled by listing an (empty) `codex` object under
`providers`. The token file is read-only for this lane - the CLI owns it
and a parallel refresh would corrupt its token rotation - so on a missing
or expired token the lane silently drops back to the rollout-embedded
limits:

```json
{
  "providers": {
    "codex": {}
  }
}
```

The z.ai lane needs the plan's API key pasted into `aidev.json` - it
cannot be pulled out of ZCode's keystore:

```json
{
  "providers": {
    "zai": { "api_key": "..." }
  }
}
```

Mainland (BigModel) plans add `"host": "open.bigmodel.cn"`. While this
lane answers, the local-sums-vs-ceilings stopgap is suppressed - real
limits beat configured guesses.

## Agent states

- **working** - a session was active within `agent_fresh_secs`
  (default 180 s).
- **check? (needs attention)** - activity stopped `agent_attention_secs`
  (default 15 min) ago: a permission prompt, a long build, or a finished
  turn waiting for review.
- **done** - quiet for up to `agent_done_secs` (default 4 h), then the
  session drops off. Done sessions are never displayed: the tile carries
  only what is running or waiting, so a provider whose sessions are all
  done disappears entirely.

Rows are grouped under an uppercase project header (the session's
working-directory basename), freshest and most urgent project first, up
to 2 sessions per project. Every session row carries its provider glyph.
When the tile cannot fit the detail (small tile, too many rows) the
renderer switches to the producer's `compact` view: a vertical stack of
provider logos, each with a dot and the number of working or waiting
sessions below it (the dot turns amber when any session needs
attention).

## Token accounting

Full context processing: input (cache reads included) + output + cache
writes. A turn on a long conversation re-sends the whole context - those
are real tokens processed and the tiles show them as such, so numbers grow
by the context size on every turn. Claude Code streams one message as
several assistant lines sharing `message.id`; only the first chunk counts.
Windows: `hour` is the rolling last 60 minutes, `today` starts at UTC
midnight, `five_hour` is the rolling Claude window and `week` starts
Monday 00:00 UTC. History comes from whatever the tools still keep on
disk - they prune their own logs, so "today" counts what is retrievable.

## Configuration

`~/deckboard/aidev.json`, all keys optional (defaults shown):

```json
{
  "poll_secs": 15,
  "http_poll_secs": 300,
  "agent_fresh_secs": 180,
  "agent_attention_secs": 900,
  "agent_done_secs": 14400,
  "history_days": 8,
  "claude_five_hour_tokens": null,
  "claude_week_tokens": null,
  "glm_five_hour_tokens": null,
  "glm_week_tokens": null,
  "providers": {
    "openrouter": { "api_key": "" },
    "anthropic": { "api_key": "" },
    "zai": { "api_key": "", "host": null }
  },
  "custom": []
}
```

Token ceilings turn the local GLM / Claude sums into percentages; without
a ceiling the lane shows raw tokens. Poll cadence: `poll_secs` drives the
local tick (the tokens-per-hour tile refreshes at this rate),
`http_poll_secs` the network lanes. `providers.zai.api_key` enables the
GLM Coding Plan quota lane (see "Plan-limit bars" above); `host` is only
needed for mainland BigModel plans.

Live smoke check against the real user directories:

```
cargo test -p deckboard-aidev -- --ignored live --nocapture
```
