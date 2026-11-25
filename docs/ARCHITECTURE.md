# 大肥鱼托管班 (fish.niox.lol) Architecture and Contracts

This file is the single source of truth shared by every component. Change it before changing a contract.

## 1. Product summary

Users register with email + password + email code (with a self generated image captcha). Each user gets a resource quota. A user creates a bot ("小鱼"), connects QQ Official Bot and/or WeChat iLink by scanning a QR code (credentials are filled in automatically), supplies their own OpenAI compatible LLM API key, edits persona and parameters visually, and installs AstrBot plugins with one click from the AstrBot plugin market catalogue. Users see resource usage broken down by source (bot core, conversation storage, each plugin's memory and storage).

## 2. Components and directories

```
fish/
  docs/ARCHITECTURE.md        this file
  web/                        React SPA (Vite, TS, Tailwind v4, shadcn/ui, Motion, TanStack Query, Recharts)
  console/                    Cloudflare Worker in Rust (workers-rs), D1, serves web/dist as static assets
  node/                       Rust node agent (tokio, axum, reqwest, rusqlite), runs all bots of all tenants on the Linux node host
  sidecar/                    Python plugin host: fork server running AstrBot plugins (AGPL)
  deploy/                     node Dockerfile, compose, cloudflared config
```

Runtime topology:

```
Browser ──HTTPS──> fish.niox.lol  (Worker "fish-console": /api/* handled by Rust, everything else = static SPA assets, SPA fallback to index.html)
                        │  HMAC signed HTTPS
                        ▼
                  fish-node-1.niox.lol (Cloudflare Tunnel) ──> node agent :8700 on the Linux node host
                        │  unix socket JSON lines
                        ▼
                  sidecar fork server ──fork──> one Python process per (bot, plugin)
```

The web build prerenders `/` (`index.html`) and `/about` (`about.html`) to static HTML with their titles, descriptions, Open Graph tags and JSON-LD (`web/src/seo.ts`, `web/scripts/prerender.mjs`), and writes `sitemap.xml`. The browser hydrates a prerendered page when its path matches the root's `data-route`. On fallback paths an inline script empties the root first and the app renders from scratch.

The console Worker is the source of truth for users, bots, credentials and plugin lists. The node is an executor that caches the desired state it receives and reports live status and usage. The node persists its cache in SQLite so it can restart without the console.

## 3. Shared conventions

- IDs: ULID strings, lowercase. Timestamps: integer unix milliseconds.
- JSON field names: snake_case.
- Errors from every HTTP API: status 4xx/5xx with body `{"error": {"code": "snake_case_code", "message": "中文 human readable"}}`.
- Secrets at rest in D1 (LLM api keys, QQ AppSecret, WeChat bot token) are encrypted with AES-256-GCM using Worker secret `DATA_KEY` (base64 32 bytes). Stored format: base64(nonce12 || ciphertext || tag16).
- Secrets are never returned to the browser. The browser only sees masked forms such as `sk-…a1b2` (first 3 and last 4 chars).

### 3.1 Console to node and node to console authentication

Both directions sign requests with the shared secret `NODE_SECRET` (per node, base64 32 bytes). The HMAC key is the 32 bytes obtained by base64 decoding the secret. The node also accepts the raw secret string as key for compatibility. The console reads `NODE_SECRET_<ID>` (node id uppercased, non alphanumeric characters replaced by `_`, e.g. `NODE_SECRET_NODE_1`) and falls back to `NODE_SECRET`.

Headers:
- `X-Fish-Node`: node id (e.g. `node-1`)
- `X-Fish-Ts`: unix ms
- `X-Fish-Sig`: lowercase hex of HMAC-SHA256(secret, `{ts}\n{METHOD}\n{path_with_query}\n{hex(sha256(body))}`)

Receivers reject when |now - ts| > 60s or signature mismatch. Body for GET is the empty string.

## 4. Data model (D1, console)

```
users(id PK, email UNIQUE, password_hash, role TEXT 'user'|'admin', created_at,
      quota_memory_mb INT, quota_storage_mb INT, quota_bots INT, quota_plugins INT,
      service_until INT NULL, last_login_at INT NULL)   -- ms, null means no service yet
invite_codes(code PK, days INT, max_uses INT DEFAULT 1, used_count INT DEFAULT 0, note TEXT,
             created_by, created_at, expires_at INT NULL, revoked_at INT NULL, quota_json TEXT NULL,
             gateway_fen INT DEFAULT 0)   -- gengwiki gateway credit per redemption
invite_redemptions(code, user_id, days, redeemed_at)
gateway_accounts(user_id PK, remote_key_id, llm_key_id, created_at)   -- one gateway key per user
gateway_credits(id PK, user_id, amount_fen, expires_at INT NULL, source 'invite'|'admin', ref UNIQUE,
                note, created_by NULL, created_at, remote_id NULL, error NULL)   -- remote_id once on the gateway
sessions(token_hash PK, user_id, created_at, expires_at, user_agent)
email_codes(email, purpose 'register'|'reset', code_hash, expires_at, attempts, created_at)
captchas(id PK, answer_hash, expires_at)
llm_keys(id PK, user_id, name, base_url, model, api_key_enc, created_at)
bots(id PK, user_id, name, avatar TEXT, node_id, status TEXT, status_detail TEXT,
     desired_running INT, needs_push INT, created_at, updated_at,
     settings_json)                        -- see BotSettings
platforms(id PK, bot_id, kind 'qq_official'|'weixin_ilink', display_name, app_id,
          secret_enc, extra_json, enabled INT, status, status_detail, created_at)
plugins(id PK, bot_id, repo_url, name, display_name, version, author, description,
        enabled INT, config_json, schema_json, status, status_detail, install_rev INT,
        commit_sha, created_at, updated_at)            -- UNIQUE(bot_id, name)
plugin_repos(repo_url PK, commit_sha, checked_at)  -- last upstream check per installed repository
nodes(id PK, base_url, region, capacity_memory_mb, capacity_storage_mb, created_at,
      last_seen_at INT NULL)
rate_events(bucket, created_at)            -- sliding window counters for throttling
qr_tasks(kind, task_id, bot_id, node_id, platform_id NULL, expires_at, created_at)  -- PK(kind, task_id)
node_tombstones(bot_id PK, node_id, created_at)  -- bot deletes the node has not confirmed yet
```

- Registration defaults to 128 MiB of memory, 128 MiB of storage and unlimited bots and plugins. The API represents unlimited counts as `null`, stored as `-1` in `users`. A count of `0` allows no slots. An invite can override the defaults at registration. Redeeming any invite on an existing account extends the service period and preserves its quotas.
- `desired_running` is the state the user asked for, `status` is the last known live state. `needs_push = 1` marks a desired state the node has not accepted yet (node unreachable). The console retries on `GET /api/bots/:id`, on node events and from the 5 minute cron, and shows status `error` with a `status_detail` meanwhile.
- `install_rev` is bumped by `POST /api/bots/:id/plugins/:pid/update` and by the automatic update, and tells the node to reinstall.
- `commit_sha` is the commit the plugin is pinned to, resolved at install and update. Empty when GitHub did not report a commit, then the node installs the head of the branch.
- Automatic updates: the 5 minute cron checks up to 5 repositories from `plugin_repos` that were not checked in the last hour. It reads the latest commit of the repository's branch (or default branch) from `https://github.com/{o}/{r}.git/info/refs?service=git-upload-pack`, which is not subject to the REST API rate limit, and falls back to `GET https://api.github.com/repos/{o}/{r}/commits/{branch or HEAD}`. Every plugin of that repository whose `commit_sha` differs is updated like a manual update: metadata and schema are read at the new commit, settings are kept and missing defaults added, `commit_sha` and `install_rev` change and the bot's desired state is pushed. A plugin whose name changed upstream is not updated. Plugins installed before commits were recorded have an empty `commit_sha` and are pinned on the first check, which reinstalls them once.
- `qr_tasks` makes QR polling idempotent: the platform created for a task is returned again on later polls.
- `node_tombstones` keeps failed node deletes so the cron can retry them.

Password hashing: PBKDF2-SHA256 via WebCrypto, 100000 iterations, 16 byte salt, stored `pbkdf2$100000$salt_b64$hash_b64`.
Session: random 32 bytes token in cookie `fish_session` (HttpOnly, Secure, SameSite=Lax, Path=/, 30 days). D1 stores sha256(token).
CSRF: mutating requests must carry header `X-Fish-Csrf: 1` (custom header + SameSite=Lax is enough for same origin SPA).

### BotSettings (settings_json)

```json
{
  "persona": {"name": "大肥鱼", "system_prompt": "...", "greeting": ""},
  "llm": {"key_id": "01H...", "model": "deepseek-chat", "temperature": 0.7, "max_tokens": 1024, "context_turns": 12},
  "behavior": {
    "wake_prefixes": ["/"], "reply_when_at": true, "reply_in_private": true,
    "reply_in_group_without_at": false, "admins": ["qq:12345"], "blocklist": [],
    "rate_limit_per_minute": 20
  }
}
```

## 5. Console HTTP API (browser to Worker), prefix `/api`

All responses JSON. Authenticated routes require the session cookie, otherwise 401 `unauthorized`.

Auth
- `GET  /api/captcha` → `{"id": "...", "svg": "<svg ...>"}` (image captcha, 4 to 5 chars, 5 min TTL, single use)
- `POST /api/auth/email-code` `{email, purpose: "register"|"reset", captcha_id, captcha_answer, invite_code?}` → `{"ok": true, "cooldown_s": 60}`. In dev (no mail binding) the code is logged. For `register` the invite code is required and checked first (no mail is sent for a bad code).
- `POST /api/auth/register` `{email, password, code, invite_code, agree: true}` → `{"user": User}` + cookie. The invite is redeemed in the same transaction: `service_until = now + days`.
- Addresses listed in `ADMIN_EMAILS` may request a register code and register without an invite code, so the first admin can bootstrap the service. `console/scripts/make-invite.sh <days> [--remote]` inserts one code directly into D1 and prints it.
- `POST /api/auth/login` `{email, password, captcha_id?, captcha_answer?}` → `{"user": User}` + cookie. After 5 failures per email in 15 min the captcha becomes required, reported via error code `captcha_required`.
- `POST /api/auth/logout` → `{"ok": true}`
- `POST /api/auth/reset` `{email, code, password}` → `{"ok": true}`
- `GET  /api/me` → `{"user": User}`

`User = {id, email, role, created_at, quota: {memory_mb, storage_mb, bots, plugins}, service_until: ms|null, service_active: bool}`

Subscription and invite codes
- The service is subscription based. At this stage every subscription period comes from an invite code made by an admin. A code carries a service length in days. Prices are not shown anywhere.
- Code format `FISH-XXXX-XXXX` (Crockford base32, case insensitive, dashes optional on input).
- `POST /api/me/redeem` `{code}` → `{"user": User}`. Extends `service_until` to `max(now, service_until) + days`.
- Register and redeem with an invite whose `gateway_fen > 0` add a gateway credit that expires at the user's `service_until` right after that redemption (see gengwiki gateway below). Later extensions do not move it. The redemption succeeds even when the gateway call fails.
- Invite errors: `invite_invalid` (unknown or revoked), `invite_used` (used_count reached max_uses), `invite_expired` (past expires_at).
- `service_active` is `role == 'admin' || service_until > now`. While inactive: `POST /api/bots`, `/start`, `/restart` and `POST /api/bots/:id/plugins` fail with 403 `service_expired`. Everything else stays readable and editable.
- A 5 minute cron stops the bots of users whose service ran out (DesiredBot `running: false`, bot `status_detail` = `服务已到期，兑换新的邀请码后可以重新启动`). Data is kept. Redeeming a code does not auto start bots.

LLM keys
- `GET /api/llm-keys` → `{"items": [{id, name, base_url, model, key_masked, created_at, via_gateway, has_balance}]}`. `via_gateway` is true when the node reaches this provider through the Cloudflare AI Gateway. `has_balance` is true for keys on the gengwiki gateway, `rotatable` for the user's own gateway key.
- `GET /api/llm-keys/:id/balance` → `{currency, available, budget, spent, expires_at, disabled, credits: [{available, amount, spent, expires_at}]}` in yuan strings, read from the gateway with the key itself. `credits` are the usable credits in billing order. Other keys give `invalid_input`, a gateway failure gives `upstream_error` (502).
- `GET /api/me/gateway` → `{account, key: item|null, balance: Balance|null, balance_error?, pending: [{amount, expires_at}]}`. `key` is null when the user deleted the model key. `pending` lists credits not yet on the gateway.
- `POST /api/me/gateway/rotate` → `{key: item}` with a new secret. Credits and spend stay, a deleted model key is added again, and bots using it are pushed. 10 per hour per user, users without a gateway key get `not_found`.
- `POST /api/llm-keys` `{name, base_url, model, api_key}` → item. Worker validates by calling `{base_url}/models` with 8s timeout, failure returns `llm_key_invalid` with the upstream message.
- `DELETE /api/llm-keys/:id`
- Presets offered by the UI (base_url): DeepSeek `https://api.deepseek.com/v1`, OpenAI `https://api.openai.com/v1`, SiliconFlow `https://api.siliconflow.cn/v1`, Moonshot `https://api.moonshot.cn/v1`, 智谱 `https://open.bigmodel.cn/api/paas/v4`, custom.

Bots
- `GET /api/bots` → `{"items": [Bot]}`
- `POST /api/bots` `{name, key_id, model}` → Bot. The Key must belong to the caller. Creation saves the selected model, assigns a node and pushes the desired state with status `starting`. Fails with `quota_exceeded` when over `quota_bots`.
- `GET /api/bots/:id` → Bot
- `PATCH /api/bots/:id` `{name?, avatar?, settings?}` → Bot (pushes to node)
- `POST /api/bots/:id/start|stop|restart` → Bot
- `DELETE /api/bots/:id`
- `GET /api/bots/:id/logs?after=<cursor>` → `{"items": [{ts, level, source, message}], "cursor": "..."}` (proxied from node, sources: `core`, `qq_official`, `weixin_ilink`, `llm`, `plugin:<name>`)

`Bot = {id, name, avatar, status: "starting"|"running"|"stopped"|"error", status_detail, created_at, updated_at, settings: BotSettings, platforms: [Platform], plugins: [Plugin]}`
`Platform = {id, kind, display_name, app_id, secret_masked, enabled, status: "connected"|"connecting"|"error"|"disabled", status_detail}`

Platform binding (QR flows run on the node, the Worker proxies and stores the result)
- `POST /api/bots/:id/platforms/qq/qr` → `{"task_id", "qr_url", "interval_s", "expires_at"}`
- `GET  /api/bots/:id/platforms/qq/qr/:task_id` → `{"status": "pending"|"expired"|"created"|"error", "message"?, "platform"?: Platform}`. On `created` the Worker stores app_id + encrypted secret, creates the platform row, pushes desired state.
- `POST /api/bots/:id/platforms/qq/manual` `{app_id, secret, mode?}` → Platform (fallback)
- `POST /api/bots/:id/platforms/weixin/qr` → `{"task_id", "qr_url", "interval_s", "expires_at"}`
- `GET  /api/bots/:id/platforms/weixin/qr/:task_id` → `{"status": "pending"|"scanned"|"expired"|"created"|"error", "platform"?}`
- `PATCH /api/bots/:id/platforms/:pid` `{enabled?, display_name?, mode?}`. `mode` is `websocket` or `webhook`, QQ only, stored in `extra_json.mode`. Platform views of QQ carry `mode` (default `websocket`).
- `POST /api/qq/webhook/:pid` public QQ callback address, no session or CSRF header. The Worker accepts it only for an enabled QQ platform in `webhook` mode and forwards `{timestamp, signature, app_id, body}` (from `X-Signature-Timestamp`, `X-Signature-Ed25519`, `X-Bot-Appid` and the raw body) to the node, then returns the node's JSON. Unsigned requests other than op 13 are rejected with 401.
- `DELETE /api/bots/:id/platforms/:pid`

Plugins
- `GET /api/plugins/market` → `{"items": [{"id","name","display_name","author","description","repo_url","category","tags"}]}`, sorted by download count. The Worker reads `https://cloud.astrbot.app/api/v1/market/plugins.json` (schema version 1), keeps entries whose `id` is `author/name` or the legacy bare `name` and whose `repo` is an https GitHub repository, and caches the result at the edge for 5 minutes. `ASTRBOT_MARKET_URL` replaces the registry URL in dev. Error: 502 `plugin_market_unavailable`.
- `POST /api/bots/:id/plugins` `{plugin_id}` → Plugin (status `installing`, node installs asynchronously). `plugin_id` is a market `id`. Plugins install only from the market catalogue, there is no install by repository URL. The Worker takes the repository from the catalogue entry, resolves the latest commit as described for automatic updates and reads `metadata.yaml` and `_conf_schema.json` from `https://raw.githubusercontent.com/{o}/{r}/{commit, or branch or HEAD when no commit}/...`. Only when no metadata file is found does it ask `https://api.github.com/repos/{o}/{r}` whether the repository exists, because unauthenticated API calls from Workers are usually rate limited. Errors: 404 `market_plugin_not_found`, `repo_invalid`, `not_astrbot_plugin`, `quota_exceeded` over `quota_plugins` (counted across all bots of the user).
- `PATCH /api/bots/:id/plugins/:pid` `{enabled?, config?}` → Plugin
- `POST /api/bots/:id/plugins/:pid/update` → Plugin (reinstall latest)
- `DELETE /api/bots/:id/plugins/:pid`

`Plugin = {id, repo_url, name, display_name, version, author, description, enabled, config, schema, status: "installing"|"running"|"sleeping"|"stopped"|"error"|"over_quota", status_detail, updated_at}`

Usage
- `GET /api/usage` → aggregated over the user's bots, proxied from the node:
```json
{
  "quota": {"memory_mb": 128, "storage_mb": 128},
  "memory_bytes": 123, "storage_bytes": 456,
  "sources": [
    {"bot_id": "...", "kind": "core"|"conversations"|"plugin", "plugin_id": null, "label": "核心连接",
     "memory_bytes": 0, "storage_bytes": 0, "cpu_percent": 0.0}
  ],
  "history": [{"ts": 0, "memory_bytes": 0, "storage_bytes": 0}]
}
```
`history` holds 5 minute samples for the last 24 hours.

Admin (role `admin` only, otherwise 403 `forbidden`; read routes never return secrets, only masked values)
- Admin accounts: the Worker var `ADMIN_EMAILS` (comma separated). A user whose email is in the list gets `role = 'admin'` on register and on every login. `console/scripts/make-admin.sh <email> [--remote]` sets the role directly in D1.
- `GET /api/admin/overview` →
```json
{
  "users": {"total": 0, "new_7d": 0, "active": 0, "expiring_7d": 0, "expired": 0},
  "invites": {"unused": 0, "used_7d": 0},
  "bots": {"total": 0, "running": 0, "starting": 0, "stopped": 0, "error": 0},
  "plugins": {"total": 0, "running": 0, "sleeping": 0, "installing": 0, "error": 0, "over_quota": 0},
  "platforms": {"qq_official": 0, "weixin_ilink": 0, "error": 0},
  "usage": {"memory_bytes": 0, "storage_bytes": 0, "quota_memory_mb": 0, "quota_storage_mb": 0},
  "plugin_cpu": {"percent": null, "reporting_nodes": 0, "items": []},
  "nodes": [{"id", "base_url", "online": true, "version", "memory_total_bytes", "memory_used_bytes", "bots": 0, "last_seen": 0, "error"?: "..."}],
  "problems": [{"user_id", "user_email", "bot_id", "bot_name", "kind": "bot"|"platform"|"plugin", "id", "label", "status", "status_detail", "updated_at"}]
}
```
  `usage` sums the node usage summaries, `quota_*` sums every user's quota. `problems` lists at most 50 entries in status `error` or `over_quota`, newest first.
  `plugin_cpu` sums reported CPU usage of plugin processes, where 100% is one logical CPU. `items` contains up to 20 plugins sorted by usage and includes `node_id` and `sampled_at` for each. Nodes without a CPU sample are excluded from `reporting_nodes`.
- `GET /api/admin/system` →
```json
{
  "generated_at": 0,
  "console": {"colo": "HKG"|null, "d1": {"size_bytes": 0|null, "query_ms": 0.0|null, "round_trip_ms": 0, "tables": [{"name", "rows"}], "users_admin": 0, "users_login_24h": 0, "sessions_active": 0, "bots_desired_running": 0, "bots_needs_push": 0, "qr_tasks_active": 0}},
  "nodes": [{"id", "base_url", "online": true, "latency_ms": 0, "last_seen": 0, "error"?: "...", "system": <GET /v1/system>|null}],
  "users": {"<user_id>": "<email>"}
}
```
  The Worker probes every node's `/v1/system` with an 8 second timeout and records `latency_ms`. A failed probe yields `online: false` and `system: null` while the D1 part is still returned. `tables` lists row counts of every D1 table. `users` maps every user id that appears in node processes, tenants and logs to its email.
- `GET /api/admin/users?q=<email substring>&cursor=` → `{"items": [AdminUser], "cursor": "..."|null}` (50 per page, newest first)
  `AdminUser = User + {bots: n, plugins: n, running: n, errors: n, memory_bytes: n|null, storage_bytes: n|null, last_login_at: n|null}` (usage is null when the node is unreachable)
- `GET /api/admin/users/:id` → `{"user": AdminUser, "bots": [Bot]}`
- `GET /api/admin/users/:id/credits` → `{configured, account, items: [Credit], remote_error}`, newest first.
  `Credit = {id, source, amount, expires_at: ms|null, expired, note, created_at, created_by_email, state: pending|failed|granted, error, spent, remaining}`. `spent` and `remaining` come from the gateway and are null when it cannot be read.
- `POST /api/admin/users/:id/credits` `{amount: "0.01".."1000", expires_at: ms|null, note?}` → 201 with the list. The credit is sent right away and retried by the maintenance cron on failure.
- `PATCH /api/admin/users/:id/credits/:credit_id` `{amount?, expires_at?}` → the list. The gateway refuses an amount below what is spent.
- `GET /api/admin/users/:id/usage` → the Usage object of `GET /api/usage`, for that user
- `GET /api/admin/users/:id/bots/:bot_id/logs?after=` → logs of that bot
- `PATCH /api/admin/users/:id` `{quota?: {memory_mb?, storage_mb?, bots?, plugins?}, service_until?: ms|null, extend_days?: n}` → AdminUser (pushes the new quota to the nodes of that user's bots, `extend_days` adds to `max(now, service_until)`)
- `GET /api/admin/invites?status=unused|used|revoked|all&cursor=` → `{"items": [Invite], "cursor", "default_quota"}` (50 per page, newest first)
  `Invite = {code, days, max_uses, used_count, note, created_by_email, created_at, expires_at, revoked_at, quota, gateway_budget, redemptions: [{user_id, user_email, days, redeemed_at, gateway, gateway_error}]}`. `gateway_budget` is yuan or null, `gateway` is null, `pending`, `failed` or `granted`.
- `POST /api/admin/invites` `{days: 1..3650, count: 1..100, max_uses?: 1..1000 (default 1), note?, expires_at?, quota?: {memory_mb, storage_mb, bots, plugins}, gateway_budget?: "0".."1000" yuan}` → `{"items": [Invite]}`. Legacy invite codes with `quota: null` use the registration defaults. A gateway budget needs `GENGWIKI_GATEWAY_TOKEN`.
- `DELETE /api/admin/invites/:code` → `{"ok": true}` (sets revoked_at, already granted time stays)
- The overview gains `"users": {"total", "new_7d", "active", "expiring_7d", "expired"}` and `"invites": {"unused", "used_7d"}`.
- `AdminUser` includes `service_until` and `service_active` through `User`.

Error codes (field `error.code`)
- Generic: `invalid_input` (400), `unauthorized` (401), `forbidden` (403, also non admins on `/api/admin/*`), `not_found` (404), `conflict` (409), `rate_limited` (429), `csrf_failed` (403), `internal` (500).
- Auth: `captcha_invalid`, `captcha_required`, `code_invalid`, `code_expired`, `email_taken` (409), `email_invalid`, `weak_password`, `terms_not_accepted`, `invalid_credentials` (401), `email_send_failed` (502).
- Subscription: `invite_invalid`, `invite_used`, `invite_expired` (400), `service_expired` (403).
- Resources: `quota_exceeded` (403), `llm_key_invalid`, `repo_invalid`, `not_astrbot_plugin`, `config_invalid` (400), `plugin_exists` (409), `github_unavailable` (502).
- Node: `node_unreachable` (502, also from `GET /api/usage`), `node_error` (502, the node answered with an error), `no_node` (503, no node configured).
- Node to console callbacks: `bad_signature` (401).

gengwiki gateway
- An OpenAI compatible DeepSeek endpoint with prepaid keys. Worker vars `GENGWIKI_GATEWAY` (origin, default `https://gateway.gengwiki.com`) and `GENGWIKI_GATEWAY_MODEL` (default `deepseek-flash`), secret `GENGWIKI_GATEWAY_TOKEN` (the service token gengwiki lists for fish).
- Each user has one key, created on the first credit: `POST {origin}/manage/keys` `{name: "大肥鱼 <email>", budget: "0", external_ref: "user:<user_id>"}` with the service token. A repeated `external_ref` reissues the same key, so a retry never makes a second key.
- The key is stored as an `llm_keys` row named `活动获取` with base URL `{origin}/v1`, and bots of that user whose key is unset or deleted switch to it and are pushed.
- Credits: `POST {origin}/manage/keys/{id}/credits` `{amount, expires_at, external_ref, note}`. `external_ref` is `invite:<code>:<user_id>` or `admin:<credit id>`, a repeated one returns the credit already there. Each credit expires on its own, the gateway bills the unexpired credit that expires soonest first, and an expired remainder is forfeited.
- A failed call stores `gateway_credits.error`. The maintenance cron retries credits older than 2 minutes and younger than 7 days, 5 per run.
- Balance: `GET {origin}/v1/balance` with the key as bearer.

Development mode (Worker var `FISH_DEV = "1"`, never in production)
- Captcha answers and email codes are written to the Worker log as `[dev] captcha <id> answer <answer>` and `[dev] email code for <email>: <code>`. `EMAIL_PROVIDER = "log"` is only accepted in this mode.
- LLM base URLs may use `http://`.
- `GITHUB_API_BASE`, `GITHUB_RAW_BASE` and `GITHUB_GIT_BASE` override `https://api.github.com`, `https://raw.githubusercontent.com` and `https://github.com` for plugin inspection and update checks.

Plugin config schema (`_conf_schema.json`) supported field types for the UI form: `string`, `text`, `int`, `float`, `bool`, `list` (of strings), `object` (nested `items`), `dict`, `template_list` (render as JSON editor). Field keys used: `description`, `hint`, `obvious_hint`, `default`, `options` (select), `labels` (option labels), `slider` `{min,max,step}`, `invisible`, `condition` `{other_key: value}` (show field only when matched), `editor_mode`.

## 6. Node HTTP API (console Worker to node), prefix `/v1`, HMAC signed

- `GET  /v1/health` → `{"node_id", "version", "memory_total_bytes", "memory_used_bytes", "bots": n}`
- `PUT  /v1/bots/:bot_id` body `DesiredBot` → `{"ok": true}` (idempotent full desired state, node reconciles: starts/stops adapters, installs/removes plugins)
- `DELETE /v1/bots/:bot_id` → stops everything and deletes bot data
- `POST /v1/bots/:bot_id/restart`
- `GET  /v1/bots/:bot_id/status` → `{"status", "status_detail", "platforms": [{"id","status","status_detail"}], "plugins": [{"id","status","status_detail","version"}]}`
- `GET  /v1/bots/:bot_id/logs?after=` → same shape as console logs
- `GET  /v1/usage?user_id=` → usage object of section 5 (node aggregates the bots belonging to that user_id)
- `GET /v1/usage/summary` reports node memory, storage, per-user totals, history and `plugin_cpu`. The CPU sample contains `percent`, `sampled_at` and the highest-usage `plugins`, each with `user_id`, `bot_id`, `bot_name`, `plugin_id`, `label` and `cpu_percent`. The sample is `null` before the first sampling round.
- `GET /v1/system` → host and container metrics sampled every 15 seconds by a background task. Keys: `node_id`, `version`, `started_at`, `interval_s`, `sampled_at` (`null` before the first round), `host` (`cpu_count`, `cpu_percent`, `load`, `uptime_s`, `memory_total_bytes`, `memory_available_bytes`, swap, `kernel`), `container` (cgroup v2 `memory_current_bytes`, `memory_max_bytes`, `memory_peak_bytes`, anon and file bytes, `oom_kills`, `cpu_percent`, `cpu_limit_cores`, `pids_current`, `pids_max`, or `null` without cgroup), `processes` (`pid`, `ppid`, `role` node|sidecar|plugin|task|other, `name`, `rss_bytes`, `pss_bytes`, `uss_bytes`, `swap_bytes`, `cpu_percent`, `threads`, `started_at`, and `plugin` with `user_id`, `bot_id`, `bot_name`, `plugin_id`, `label` for plugin processes), `history` (up to 240 points of `ts`, `host_cpu`, `container_cpu`, `host_memory_used_bytes`, `container_memory_bytes`), `disk` (statvfs of the data dir), `storage` (`node_db_bytes`, `conversations_db_bytes`, `tenants_bytes`, `uv_cache_bytes`), `cgroups_enabled`, `tenants` (per user cgroup memory current, max, peak, `oom_kills`, `procs`), `bots` and `plugins` status counts, `platforms` status counts per `kind`, and `logs` (error and warn counts of the last hour with the top 10 bots). CPU percent is 100 per logical core everywhere. Without `/proc` the node falls back to `ps` and reports RSS only.
- `POST /v1/qr/qq` → `{"task_id","qr_url","interval_s","expires_at"}`
- `GET  /v1/qr/qq/:task_id` → `{"status", "app_id"?, "secret"?}` (plaintext over the signed TLS tunnel, the Worker encrypts it before storing)
- `POST /v1/bots/:bot_id/platforms/:platform_id/qq-webhook` `{timestamp, signature, app_id, body}` → op 13 answer `{plain_token, signature}` or `{"op": 12}`. 401 for a bad signature or AppID, 400 when the platform is not running in webhook mode.
- `POST /v1/qr/weixin` → same shape
- `GET  /v1/qr/weixin/:task_id` → `{"status", "bot_id"?, "token"?, "base_url"?, "user_id"?}`

```
DesiredBot = {
  bot_id, user_id, name, running: bool,
  quota: {memory_mb, storage_mb},
  settings: BotSettings,
  llm: {base_url, model, api_key, endpoint?, gateway_token?} | null,  // plaintext resolved from settings.llm.key_id
  platforms: [{id, kind, enabled, app_id, secret, extra}],
  plugins:   [{id, name, repo_url, version, enabled, config, install_rev, commit}]
}
```

`endpoint` and `gateway_token` are set only for OpenAI, Anthropic, Google Gemini and xAI when the Worker has `AI_GATEWAY` and the `AI_GATEWAY_TOKEN` secret. The node then calls `{endpoint}/chat/completions` (Anthropic `{endpoint}/messages`) with `cf-aig-authorization: Bearer <gateway_token>` and `cf-aig-collect-log: false`, and keeps provider specific parameters keyed on `base_url`. Mapping: `api.openai.com/v1` to `<gateway>/openai`, `api.anthropic.com/v1` to `<gateway>/anthropic/v1`, `generativelanguage.googleapis.com/v1beta/openai` to `<gateway>/google-ai-studio/v1beta/openai`, `api.x.ai/v1` to `<gateway>/grok/v1`. Key validation runs in the Worker and stays direct. Anthropic keys go in `x-api-key`.

`install_rev` (ms) changes when the user asks for a plugin update or an automatic update runs. `commit` is a 40 hex commit id or empty. A node that sees a new `install_rev`, `commit`, `version`, `name` or `repo_url` reinstalls the plugin and keeps its config and data. With a commit the node downloads `zip/<commit>`, otherwise the head of the branch.

Node to console callbacks (optional, best effort, HMAC signed): `POST https://fish.niox.lol/api/internal/node/events` with `{"node_id", "events": [{"bot_id","kind":"bot_status"|"platform_status"|"plugin_status","id","status","status_detail","ts"}]}`. The console updates D1 status columns.

Plugin archive proxy: `GET https://fish.niox.lol/api/internal/plugin-archive/{owner}/{repo}/zip/{ref...}`, signed by the node like the events callback (method `GET`, empty body). The Worker streams `https://codeload.github.com/{owner}/{repo}/zip/{ref...}` back. Only `[A-Za-z0-9._-]` path segments are accepted. GitHub 404 passes through as 404, other upstream failures return 502 or 504, archives over 64 MB return 413. The node tries each candidate archive URL through this proxy first and then directly against codeload. Each request has a 60 s total timeout, network errors, 429 and 5xx are retried once per source, and a 404 moves on to the next candidate ref.

## 7. Node internals

- One process, tokio. Each bot is a task group: platform adapters, message pipeline, LLM client, plugin bridge.
- Message pipeline: platform event → normalize to `MessageEvent` → behavior filters (blocklist, rate limit, wake rules) → plugin dispatch (section 8) → if no plugin handled it and the message woke the bot → default LLM chat with persona and the last `context_turns` turns → send reply.
- Conversation history in SQLite `data/conversations.db`, table per bot rows, attributed as storage source `conversations`.
- Recently seen sessions and senders in table `seen` of `data/conversations.db`, at most 100 of each per bot, written at most once per 5 min per entry. `GET /v1/bots/{bot_id}/recent` (console `GET /api/bots/{id}/recent`) returns `sessions` and `users`, each `[{id, origin, last_ts}]` newest first, where `id` is the unified origin or the sender account and `origin` is the session it was last seen in. The console offers them when filling plugin session lists and admin lists.
- Built in command `/sid` (any wake prefix followed by `sid`, or `/sid`) replies with the session origin, the sender account and the group id, before plugins and the default chat see the message.
- QQ Official: credentials (app_id, secret) → access token via `https://bots.qq.com/app/getAppAccessToken`, passive reply with msg_id/msg_seq while the last inbound message of that session is under 60 minutes old (groups and private chats), active message otherwise. A private active message that QQ rejects is sent again with `is_wakeup` (interaction recall, one per period for 30 days after the user last chatted), and the rest of that chain is dropped. Rebuild from public QQ bot docs. Two ways to receive events, chosen by `extra.mode`:
  - `websocket` (default): the node opens the gateway with intents for group @ messages and C2C messages.
  - `webhook`: QQ posts events to `https://fish.niox.lol/api/qq/webhook/<platform id>`. The Ed25519 key comes from the secret repeated to at least 32 bytes and cut to 32. Pushes are verified over `timestamp || body`. The op 13 validation is answered with a signature over `event_ts || plain_token`, and it is signed only for alphanumeric tokens and numeric timestamps, so the answer can never cover a JSON event. A signed op 13 request must carry a valid signature. Retried pushes are dropped by event id. The platform reports `connecting` until the first validation or event, then `connected`.
- QQ QR bind: random 32 byte key (base64) → `POST https://q.qq.com/lite/create_bind_task {"key"}` → `task_id`, QR content `https://q.qq.com/qqbot/openclaw/connect.html?task_id=...`, poll `POST https://q.qq.com/lite/poll_bind_result` every 2s, status 1 pending, 2 completed, 3 expired. On completion decrypt `bot_encrypt_secret` (base64 of nonce12 || ciphertext || tag16, AES-256-GCM with the key) and return `bot_appid` + secret.
- WeChat iLink: base `https://ilinkai.weixin.qq.com`, QR via `ilink/bot/get_bot_qrcode` and `ilink/bot/get_qrcode_status`, then long poll for updates and send messages. Private chat only, passive reply only, 24 hour window. Study the protocol and implement independently.
- When the default LLM reply fails for a known reason, the bot replies with a short notice instead of staying silent: no key configured, insufficient balance (HTTP 402 or balance error codes and messages), invalid or expired key (401, `invalid_api_key`, `key_disabled`, `key_expired`), unavailable model (404, `model_not_found`), busy provider (429, 529), unreachable provider (no response or 5xx) and refused content (content filter errors). A notice of one kind is sent at most once per chat session every 10 minutes, every minute for busy and unreachable providers, and every 10 seconds for refused content. Other errors are only logged.
- Usage: memory of plugin processes measured as private memory (private clean plus private dirty) from `/proc/<pid>/smaps_rollup` (Linux), fallback RSS on macOS. Pages still shared with the fork server are not charged to plugins. A process shared by several plugins (group mode, section 8) is measured once and split evenly across its plugins. Core memory per bot is an estimate maintained by counters (buffers, caches, session state). Storage measured by walking `data/tenants/<user_id>/<bot_id>/plugins/<plugin>/` (data) plus the shared install it uses (code + venv) every 5 min, so every bot is charged the full shared install, conversations by SQLite row bytes. Samples kept 24h in SQLite.
- The node applies the tenant's `memory.max` before the plugin loads on Linux with writable cgroup v2. Every 10 seconds it reads each tenant's working set (`memory.current` minus `inactive_file`) and the free memory of its own cgroup, and sets `memory.max` to the working set plus half of the node memory above a 256 MiB reserve, kept between the quota and twice the quota. A tenant can therefore use more than its quota for a short time while the node has memory to spare. A tenant that stays above its quota for 3 minutes has its largest plugin stopped with status `over_quota`. Without writable cgroups the node falls back to periodic memory checks, and three consecutive five-minute samples over quota stop the largest plugin. A tenant over its storage quota cannot install new plugins. Storage accounting for plugin writes is best effort.
- The node sends `idle_check` after `FISH_PLUGIN_IDLE_SECS` of inactivity (default 1800, 0 disables it). General plugins track all inbound messages. A plugin that opts in with `fish_pool.command_only_idle: true` and reports eligible commands tracks messages matching those commands or their aliases. A plugin with no background work shuts down and reports `sleeping`. The node starts sleeping plugins that may handle a message and waits up to 30 seconds before dispatching. Background work postpones sleep for at least another idle period. In a group process each plugin sleeps on its own: the node unloads that plugin from the process, and the process exits once it holds no plugins.
- The sidecar decodes knowledge-base `.txt` uploads without loading converters for other document formats. Linux fork servers can share the default Jieba dictionary across plugin processes with `FISH_SIDECAR_WARM_JIEBA=1`. After the preload, and in a template after the plugin import, the process makes every object reachable from garbage collected objects and loaded modules immortal (CPython 3.12+ default build) and then freezes the garbage collector, so plugin processes that read these objects do not write their reference counts and keep sharing the pages. These objects are never freed, and their `__del__` methods and weak reference callbacks do not run. `FISH_SIDECAR_IMMORTALIZE=0` disables this.
- Isolation: plugin processes run as an unprivileged uid, inside `bwrap` when available (read only root, write access only to the plugin's data dir, network allowed). Shortcut: no bwrap on macOS dev, upgrade to bwrap/nsjail on the Linux node.
- Data layout: `data/node.db`, `data/conversations.db`, `data/tenants/<user_id>/<bot_id>/plugins/<plugin_name>/{data,.fish_install.json}`, `data/shared/plugins/<share key>/{src,venv,.fish_shared.json}`. The sandbox mounts `data/shared` read only.
- Config via env: `FISH_NODE_ID`, `FISH_NODE_SECRET`, `FISH_CONSOLE_URL`, `FISH_LISTEN` (default `0.0.0.0:8700`), `FISH_DATA_DIR`, `FISH_SIDECAR_PYTHON`, `FISH_SIDECAR_DIR`, `FISH_PLUGIN_IDLE_SECS`.

## 8. Node to sidecar protocol

Transport: the node starts the fork server `python -m fish_sidecar.server --socket <path>` once. Messages are JSON objects, one per line (UTF-8, `\n` terminated) on unix sockets.

Control socket (node ↔ fork server):
- `{"op":"spawn","proc_id","plugin_dir","venv_site_packages","data_dir","plugin_name","bot_id","socket","wait_for_memory_limit","share_key","config_on_socket":true}` → fork server forks a child that loads the plugin and connects to `socket` (a per process socket the node listens on). Reply `{"op":"spawned","proc_id","pid"}` or `{"op":"error","proc_id","message"}`. The node writes the byte `0x01` after it moved the child into the tenant memory group (only when `wait_for_memory_limit`), then `{"op":"config","config"}` as the first line on `socket`. The config never passes through the fork server, whose memory every forked process inherits.
- Templates (Linux): for a spawn with `share_key` the fork server keeps one template process per share key and plugin name. It sends `{"op":"template","key","pid"}`, the node moves the pid into the `templates` memory group (512 MB, no swap) and answers `{"op":"template_ready","key","pid","ok","message"?}`. The template then imports the plugin once in an empty working directory and forks every plugin process of that install twice, so the plugin process is adopted by the fork server (a child subreaper) and reported with `spawned` like a direct child. Each plugin process enters its own data dir and runs the constructor itself. The import is not shared, and the plugin is forked directly from the fork server, when the import fails, takes over 90 s, or does something that would differ per process: file system access through a relative path or inside the working directory or `HOME`, randomness (`os.urandom`, the `random` functions, a new `random.Random`, except `numpy.random`, which each plugin process seeds again), signal handlers, threads, subprocesses, open files, files written to the working directory, or values that contain the working directory path, a reading of `time.time`, `time.monotonic`, `time.perf_counter` (seconds, ms or ns) or a `datetime` taken during the import, or the template's pid after the import called `os.getpid` (calls from `logging` excluded). Values are searched in the namespaces of the newly imported modules and their classes and in the attributes of module level objects whose class comes from those modules, one container level deep. The plugin log then says why. A template is not dumpable, so plugin processes of the same uid cannot trace it, and it stops after 300 s without plugin processes. `FISH_SIDECAR_TEMPLATES=0` disables templates.
- `{"op":"kill","proc_id"}`
- Group mode: a spawn with `"mode":"group"` starts a group process that can hold several plugins of the same bot. Its `socket` is a control socket. After the optional `0x01` byte the group process sends `{"op":"ready","pid"}`. The node then sends `{"op":"load","proc_id","plugin_dir","plugin_name","data_dir","socket"}` to add a plugin, `{"op":"unload","proc_id"}` to stop one and `{"op":"exit"}` to end the process. Each loaded plugin connects to its own `socket` and speaks the plugin process protocol below, starting with the `config` line. When a plugin ends the group process sends `{"op":"slot_exited","proc_id","code","drain"}`, where `drain` asks the node to stop placing new plugins there. Each plugin runs in its own `contextvars` context with its own AstrBot root, databases, `StarTools` context, log sink and standard streams. Handlers, tools and idle checks only see the plugin's own modules and tasks. Only plugins without their own installed packages are placed in a group. Before importing, the group process scans the plugin source (tests and docs skipped) for threads, background tasks, schedulers, blocking sleeps, synchronous HTTP, subprocesses, environment or process state changes, relative paths, session waiters and the knowledge base, and checks that its top level module names do not resolve elsewhere. After loading it checks for new threads and leftover tasks. A plugin that fails any check ends with code `4` before `hello`, and the node starts it again at once in its own process. A plugin that blocks the event loop for more than 5 s also ends with code `4`. The node remembers these plugins per bot until it restarts. A group process holds at most 16 plugins and accepts at most 32 loads in its lifetime.
- Child exit notification `{"op":"exited","proc_id","pid","code"}`. A child that exits with code 2 before sending `hello` failed to load the plugin (import error or unsupported feature), and the node marks the plugin `error` with the last logged message instead of restarting it.

Install: the node downloads the archive and extracts it to `data/shared/plugins/.staging-<random>/src`. When the plugin has `requirements.txt` the node creates a throwaway venv there and runs `uv pip install --dry-run -r requirements.txt`, which reports the packages a fresh install would add or replace now. The shared install is named `<plugin name>-<first 16 hex of sha256(source + resolved package list)>-py<major.minor>`, where the source is the pinned commit or, without one, the archive sha256. When that install is complete (it has `.fish_shared.json`), the bot reuses it. Otherwise the node moves the extracted source into it and runs `uv venv --system-site-packages` + `uv pip install --compile-bytecode -r requirements.txt` (shared uv cache) and `compileall` inside `data/shared/plugins/<share key>/`, writing `.fish_shared.json` last. The staging directory is removed afterwards. Installs run one at a time. Install output goes to the bot logs with source `plugin:<name>`. The bot's install marker `.fish_install.json` records repo, version, name, commit, `install_rev`, the `major.minor` of `FISH_SIDECAR_PYTHON` and the share key, so a node running another interpreter reinstalls the plugin (`data/` is kept). Markers written before shared installs have no share key, so those plugins are downloaded again once and their per bot `src` and `venv` are removed. Shared installs that no marker names are removed at node start, after an install and after a plugin or bot is removed. Because the key includes the resolved package list, a bot reuses a shared install only when a fresh install would pick the same dependency versions, and a manual update of a plugin resolves its dependencies again.

Plugin process socket (node ↔ one plugin process):
- child → node `{"op":"hello","proc_id","plugin":{"name","version","wake_commands"?,"handlers":[{"id","kind":"command"|"command_group"|"regex"|"event_message_type"|"llm_request"|"decorating_result"|"on_loaded"|"platform_loaded"|"llm_tool","pattern"?,"priority","tool"?}]}}`. `wake_commands` lists the command names and aliases for an eligible opt-in plugin, or is null. `llm_tool` handlers carry `tool: {"name","description","parameters"}`, where `parameters` is a JSON schema built from the Google style docstring and the signature.
- node → child `{"op":"event","event_id","event": MessageEvent}`
- child → node `{"op":"matched","event_id","matched": bool}` (sent quickly after filter matching, before running handlers)
- child → node `{"op":"send","event_id"?,"session","chain":[Component]}` (replies, may be several)
- child → node `{"op":"done","event_id","stop": bool, "call_llm": bool}`
- node → child `{"op":"llm_hook","event_id","request":{"prompt","system_prompt","contexts"}}` → child `{"op":"llm_hook_result","event_id","request":{...}}`
- node → child `{"op":"tool_call","call_id","event_id","event","name","arguments"}` → child `{"op":"tool_result","call_id","text","sent"}` or `{"op":"tool_result","call_id","error"}`. Returned and yielded strings form `text`. Message results the tool yields are sent to the user with `send` ops for `event_id` before the result, and `sent` is true. The node's default LLM reply offers the tools of all running plugins of the bot (OpenAI `tools` or Claude `tools`), runs each call with a 60 s limit, feeds results back (truncated to 8000 characters) and allows up to 5 tool rounds, after which the model must answer without tools. If the first request with tools gets a 4xx other than 401 or 429, it is retried once without tools. Duplicate tool names keep the plugin that sorts first by name.
- child → node `{"op":"llm","req_id","prompt"?,"system_prompt"?,"contexts"?,"image_urls"?,"tools"?}` → node `{"op":"llm_result","req_id","text"?,"tool_calls"?,"error"?}`. The node uses the bot's configured model. The child can run the returned tool calls and submit the next turn with the tool results in `contexts`.
- child → node `{"op":"send_to","session","chain"}` (proactive send by unified_msg_origin)
- child → node `{"op":"log","level","message"}`
- node → child `{"op":"config","config"}` (config update) and `{"op":"shutdown"}`
- node → child `{"op":"idle_check"}` → child `{"op":"idle_result","busy":[...]}`. `busy` describes work that stopping the process would lose: asyncio tasks other than the child's own, pending loop timers, file descriptors registered with the loop besides the node socket, threads not named `asyncio*`, and pending LLM requests. An empty list means the plugin may sleep.

```
MessageEvent = {platform: "qq_official"|"weixin_ilink", bot_id, self_id,
  message_type: "group"|"private", group_id?, session_id, unified_msg_origin: "<platform_id>:<GroupMessage|FriendMessage>:<session_id>",
  message_id, sender: {user_id, nickname}, message_str, chain: [Component], is_at_or_wake: bool, is_admin: bool, admins?: [account id], timestamp, raw?}
Component = {"type":"plain","text"} | {"type":"image","url"?,"path"?,"base64"?} | {"type":"at","user_id","name"?}
          | {"type":"reply","id"} | {"type":"record","url"?,"path"?} | {"type":"face","id"}
```

`message_str` arrives with the matched wake prefix already removed by the node, so the sidecar passes it to plugins unchanged.

Plugins may run handlers concurrently. The node waits up to 2s for `matched` from every plugin, then runs the default LLM only if nobody matched or every matched plugin ended with `call_llm: true`.

## 9. AstrBot in the sidecar

The sidecar runs plugins against the real AstrBot package (`astrbot` from PyPI, pinned in `sidecar/uv.lock`), so plugins import AstrBot's own `astrbot.api` and `astrbot.core` modules. The fork server imports AstrBot once and warms its asynchronous SQLite start-up paths with disposable databases. It closes those connections and joins their workers before forking. `fish_sidecar/astrbot_boot.py` replaces the console writer and preference scheduler threads with inline versions and keeps OpenTelemetry's fork handler from starting threads. Light plugins of the same bot share a group process and other plugins run in their own forked process (section 8). Every plugin keeps its own data directory and databases.

`fish_sidecar/adapter.py` connects message events and platform entries to the node `send` and `send_to` ops. It connects the `fish` chat provider to the node `llm` op. The plugin `Context` uses AstrBot's own conversation, message history, persona, configuration profile, knowledge base and cron managers. Each plugin instance stores their data in its own AstrBot root. See `sidecar/COMPAT.md` for the supported provider and job types.

## 10. License boundary

AstrBot is AGPL-3.0. The sidecar imports and runs it, so the sidecar (`sidecar/`) is distributed under AGPL-3.0-or-later, with the license text in `sidecar/LICENSE`, and its source stays public in this repository. The console, node and web front end do not include or link AstrBot code. They talk to the sidecar only through the plugin protocol (section 8) over a Unix socket. The [README](../README.md) records the whale sprite assets' source, modifications and licence.
