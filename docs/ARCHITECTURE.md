# 大肥鱼托管班 (fish.niox.lol) Architecture and Contracts

This file is the single source of truth shared by every component. Change it before changing a contract.

## 1. Product summary

Users register with email + password + email code (with a self generated image captcha). Each user gets a resource quota. A user creates a bot ("小鱼"), connects QQ Official Bot and/or WeChat iLink by scanning a QR code (credentials are filled in automatically), supplies their own OpenAI compatible LLM API key, edits persona and parameters visually, and installs AstrBot plugins by pasting a GitHub repository URL (found on https://cloud.astrbot.app/plugin). Users see resource usage broken down by source (bot core, conversation storage, each plugin's memory and storage).

## 2. Components and directories

```
fish/
  docs/ARCHITECTURE.md        this file
  web/                        React SPA (Vite, TS, Tailwind v4, shadcn/ui, Motion, TanStack Query, Recharts)
  console/                    Cloudflare Worker in Rust (workers-rs), D1, serves web/dist as static assets
  node/                       Rust node agent (tokio, axum, reqwest, rusqlite), runs all bots of all tenants on the 2C2G host
  sidecar/                    Python plugin host: fork server + `astrbot` compatibility package
  deploy/                     node Dockerfile, compose, cloudflared config
```

Runtime topology:

```
Browser ──HTTPS──> fish.niox.lol  (Worker "fish-console": /api/* handled by Rust, everything else = static SPA assets, SPA fallback to index.html)
                        │  HMAC signed HTTPS
                        ▼
                  node-1.fish.niox.lol (Cloudflare Tunnel) ──> node agent :8700 on the 2C2G host
                        │  unix socket JSON lines
                        ▼
                  sidecar fork server ──fork──> one Python process per (bot, plugin)
```

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
      quota_memory_mb INT DEFAULT 256, quota_storage_mb INT DEFAULT 1024,
      quota_bots INT DEFAULT 1, quota_plugins INT DEFAULT 8,
      service_until INT NULL, last_login_at INT NULL)   -- ms, null means no service yet
invite_codes(code PK, days INT, max_uses INT DEFAULT 1, used_count INT DEFAULT 0, note TEXT,
             created_by, created_at, expires_at INT NULL, revoked_at INT NULL)
invite_redemptions(code, user_id, days, redeemed_at)
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
        created_at, updated_at)            -- UNIQUE(bot_id, name)
nodes(id PK, base_url, region, capacity_memory_mb, capacity_storage_mb, created_at,
      last_seen_at INT NULL)
rate_events(bucket, created_at)            -- sliding window counters for throttling
qr_tasks(kind, task_id, bot_id, node_id, platform_id NULL, expires_at, created_at)  -- PK(kind, task_id)
node_tombstones(bot_id PK, node_id, created_at)  -- bot deletes the node has not confirmed yet
```

- `desired_running` is the state the user asked for, `status` is the last known live state. `needs_push = 1` marks a desired state the node has not accepted yet (node unreachable). The console retries on `GET /api/bots/:id`, on node events and from the 5 minute cron, and shows status `error` with a `status_detail` meanwhile.
- `install_rev` is bumped by `POST /api/bots/:id/plugins/:pid/update` and tells the node to reinstall.
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
- Invite errors: `invite_invalid` (unknown or revoked), `invite_used` (used_count reached max_uses), `invite_expired` (past expires_at).
- `service_active` is `role == 'admin' || service_until > now`. While inactive: `POST /api/bots`, `/start`, `/restart` and `POST /api/bots/:id/plugins` fail with 403 `service_expired`. Everything else stays readable and editable.
- A 5 minute cron stops the bots of users whose service ran out (DesiredBot `running: false`, bot `status_detail` = `服务已到期，兑换新的邀请码后可以重新启动`). Data is kept. Redeeming a code does not auto start bots.

LLM keys
- `GET /api/llm-keys` → `{"items": [{id, name, base_url, model, key_masked, created_at}]}`
- `POST /api/llm-keys` `{name, base_url, model, api_key}` → item. Worker validates by calling `{base_url}/models` with 8s timeout, failure returns `llm_key_invalid` with the upstream message.
- `DELETE /api/llm-keys/:id`
- Presets offered by the UI (base_url): DeepSeek `https://api.deepseek.com/v1`, OpenAI `https://api.openai.com/v1`, SiliconFlow `https://api.siliconflow.cn/v1`, Moonshot `https://api.moonshot.cn/v1`, 智谱 `https://open.bigmodel.cn/api/paas/v4`, custom.

Bots
- `GET /api/bots` → `{"items": [Bot]}`
- `POST /api/bots` `{name}` → Bot (one click deploy: assigns a node, pushes desired state, status `starting`). Fails with `quota_exceeded` when over `quota_bots`.
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
- `POST /api/bots/:id/platforms/qq/manual` `{app_id, secret}` → Platform (fallback)
- `POST /api/bots/:id/platforms/weixin/qr` → `{"task_id", "qr_url", "interval_s", "expires_at"}`
- `GET  /api/bots/:id/platforms/weixin/qr/:task_id` → `{"status": "pending"|"scanned"|"expired"|"created"|"error", "platform"?}`
- `PATCH /api/bots/:id/platforms/:pid` `{enabled?, display_name?}`
- `DELETE /api/bots/:id/platforms/:pid`

Plugins
- `POST /api/plugins/inspect` `{repo_url}` → `{"name","display_name","version","author","description","repo_url","schema": {...}|null, "has_requirements": bool}`. Accepts `https://github.com/owner/repo` (optional `.git`, trailing slash, `/tree/<branch>`). Worker reads `metadata.yaml` and `_conf_schema.json` from the default branch via `https://api.github.com/repos/{o}/{r}` and `https://raw.githubusercontent.com/...`. Errors: `repo_invalid`, `not_astrbot_plugin`.
- `POST /api/bots/:id/plugins` `{repo_url}` → Plugin (status `installing`, node installs asynchronously). Fails with `quota_exceeded` over `quota_plugins` (counted across all bots of the user).
- `PATCH /api/bots/:id/plugins/:pid` `{enabled?, config?}` → Plugin
- `POST /api/bots/:id/plugins/:pid/update` → Plugin (reinstall latest)
- `DELETE /api/bots/:id/plugins/:pid`

`Plugin = {id, repo_url, name, display_name, version, author, description, enabled, config, schema, status: "installing"|"running"|"stopped"|"error"|"over_quota", status_detail, updated_at}`

Usage
- `GET /api/usage` → aggregated over the user's bots, proxied from the node:
```json
{
  "quota": {"memory_mb": 256, "storage_mb": 1024},
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
  "plugins": {"total": 0, "running": 0, "installing": 0, "error": 0, "over_quota": 0},
  "platforms": {"qq_official": 0, "weixin_ilink": 0, "error": 0},
  "usage": {"memory_bytes": 0, "storage_bytes": 0, "quota_memory_mb": 0, "quota_storage_mb": 0},
  "nodes": [{"id", "base_url", "online": true, "version", "memory_total_bytes", "memory_used_bytes", "bots": 0, "last_seen": 0, "error"?: "..."}],
  "problems": [{"user_id", "user_email", "bot_id", "bot_name", "kind": "bot"|"platform"|"plugin", "id", "label", "status", "status_detail", "updated_at"}]
}
```
  `usage` sums the node usage summaries, `quota_*` sums every user's quota. `problems` lists at most 50 entries in status `error` or `over_quota`, newest first.
- `GET /api/admin/users?q=<email substring>&cursor=` → `{"items": [AdminUser], "cursor": "..."|null}` (50 per page, newest first)
  `AdminUser = User + {bots: n, plugins: n, running: n, errors: n, memory_bytes: n|null, storage_bytes: n|null, last_login_at: n|null}` (usage is null when the node is unreachable)
- `GET /api/admin/users/:id` → `{"user": AdminUser, "bots": [Bot]}`
- `GET /api/admin/users/:id/usage` → the Usage object of `GET /api/usage`, for that user
- `GET /api/admin/users/:id/bots/:bot_id/logs?after=` → logs of that bot
- `PATCH /api/admin/users/:id` `{quota?: {memory_mb?, storage_mb?, bots?, plugins?}, service_until?: ms|null, extend_days?: n}` → AdminUser (pushes the new quota to the nodes of that user's bots, `extend_days` adds to `max(now, service_until)`)
- `GET /api/admin/invites?status=unused|used|revoked|all&cursor=` → `{"items": [Invite], "cursor"}` (50 per page, newest first)
  `Invite = {code, days, max_uses, used_count, note, created_by_email, created_at, expires_at, revoked_at, redemptions: [{user_id, user_email, days, redeemed_at}]}`
- `POST /api/admin/invites` `{days: 1..3650, count: 1..100, max_uses?: 1..1000 (default 1), note?, expires_at?}` → `{"items": [Invite]}`
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

Development mode (Worker var `FISH_DEV = "1"`, never in production)
- Captcha answers and email codes are written to the Worker log as `[dev] captcha <id> answer <answer>` and `[dev] email code for <email>: <code>`. `EMAIL_PROVIDER = "log"` is only accepted in this mode.
- LLM base URLs may use `http://`.
- `GITHUB_API_BASE` and `GITHUB_RAW_BASE` override `https://api.github.com` and `https://raw.githubusercontent.com` for plugin inspection.

Plugin config schema (`_conf_schema.json`) supported field types for the UI form: `string`, `text`, `int`, `float`, `bool`, `list` (of strings), `object` (nested `items`), `dict`, `template_list` (render as JSON editor). Field keys used: `description`, `hint`, `obvious_hint`, `default`, `options` (select), `labels` (option labels), `slider` `{min,max,step}`, `invisible`, `condition` `{other_key: value}` (show field only when matched), `editor_mode`.

## 6. Node HTTP API (console Worker to node), prefix `/v1`, HMAC signed

- `GET  /v1/health` → `{"node_id", "version", "memory_total_bytes", "memory_used_bytes", "bots": n}`
- `PUT  /v1/bots/:bot_id` body `DesiredBot` → `{"ok": true}` (idempotent full desired state, node reconciles: starts/stops adapters, installs/removes plugins)
- `DELETE /v1/bots/:bot_id` → stops everything and deletes bot data
- `POST /v1/bots/:bot_id/restart`
- `GET  /v1/bots/:bot_id/status` → `{"status", "status_detail", "platforms": [{"id","status","status_detail"}], "plugins": [{"id","status","status_detail","version"}]}`
- `GET  /v1/bots/:bot_id/logs?after=` → same shape as console logs
- `GET  /v1/usage?user_id=` → usage object of section 5 (node aggregates the bots belonging to that user_id)
- `GET  /v1/usage/summary` → `{"memory_bytes", "storage_bytes", "users": [{"user_id", "memory_bytes", "storage_bytes"}], "history": [{"ts", "memory_bytes", "storage_bytes"}]}` (whole node, used by the admin overview)
- `POST /v1/qr/qq` → `{"task_id","qr_url","interval_s","expires_at"}`
- `GET  /v1/qr/qq/:task_id` → `{"status", "app_id"?, "secret"?}` (plaintext over the signed TLS tunnel, the Worker encrypts it before storing)
- `POST /v1/qr/weixin` → same shape
- `GET  /v1/qr/weixin/:task_id` → `{"status", "bot_id"?, "token"?, "base_url"?, "user_id"?}`

```
DesiredBot = {
  bot_id, user_id, name, running: bool,
  quota: {memory_mb, storage_mb},
  settings: BotSettings,
  llm: {base_url, model, api_key} | null,          // plaintext resolved from settings.llm.key_id
  platforms: [{id, kind, enabled, app_id, secret, extra}],
  plugins:   [{id, name, repo_url, version, enabled, config, install_rev}]
}
```

`install_rev` (ms) changes when the user asks for a plugin update, a node that sees a new value reinstalls the plugin and keeps its config and data.

Node to console callbacks (optional, best effort, HMAC signed): `POST https://fish.niox.lol/api/internal/node/events` with `{"node_id", "events": [{"bot_id","kind":"bot_status"|"platform_status"|"plugin_status","id","status","status_detail","ts"}]}`. The console updates D1 status columns.

## 7. Node internals

- One process, tokio. Each bot is a task group: platform adapters, message pipeline, LLM client, plugin bridge.
- Message pipeline: platform event → normalize to `MessageEvent` → behavior filters (blocklist, rate limit, wake rules) → plugin dispatch (section 8) → if no plugin handled it and the message woke the bot → default LLM chat with persona and the last `context_turns` turns → send reply.
- Conversation history in SQLite `data/conversations.db`, table per bot rows, attributed as storage source `conversations`.
- QQ Official: credentials (app_id, secret) → access token via `https://bots.qq.com/app/getAppAccessToken`, WebSocket gateway, intents for group @ messages and C2C messages, passive reply with msg_id/msg_seq. Rebuild from public QQ bot docs.
- QQ QR bind: random 32 byte key (base64) → `POST https://q.qq.com/lite/create_bind_task {"key"}` → `task_id`, QR content `https://q.qq.com/qqbot/openclaw/connect.html?task_id=...`, poll `POST https://q.qq.com/lite/poll_bind_result` every 2s, status 1 pending, 2 completed, 3 expired. On completion decrypt `bot_encrypt_secret` (base64 of nonce12 || ciphertext || tag16, AES-256-GCM with the key) and return `bot_appid` + secret.
- WeChat iLink: base `https://ilinkai.weixin.qq.com`, QR via `ilink/bot/get_bot_qrcode` and `ilink/bot/get_qrcode_status`, then long poll for updates and send messages. Private chat only, passive reply only, 24 hour window. Study the protocol and implement independently.
- Usage: memory of plugin processes measured as PSS from `/proc/<pid>/smaps_rollup` (Linux), fallback RSS on macOS. Core memory per bot is an estimate maintained by counters (buffers, caches, session state). Storage measured by walking `data/tenants/<user_id>/<bot_id>/plugins/<plugin>/` (code + venv + data) every 5 min, conversations by SQLite row bytes. Samples kept 24h in SQLite.
- Quota enforcement: when a bot's user total memory exceeds quota for 3 consecutive samples, the largest plugin process is stopped with status `over_quota`. Storage over quota blocks new installs and plugin writes are best effort. On Linux use cgroup v2 `memory.max` per tenant when the cgroup is writable.
- Isolation: plugin processes run as an unprivileged uid, inside `bwrap` when available (read only root, write access only to the plugin's data dir, network allowed). Shortcut: no bwrap on macOS dev, upgrade to bwrap/nsjail on the Linux node.
- Data layout: `data/node.db`, `data/conversations.db`, `data/tenants/<user_id>/<bot_id>/plugins/<plugin_name>/{src,venv,data}`.
- Config via env: `FISH_NODE_ID`, `FISH_NODE_SECRET`, `FISH_CONSOLE_URL`, `FISH_LISTEN` (default `0.0.0.0:8700`), `FISH_DATA_DIR`, `FISH_SIDECAR_PYTHON`, `FISH_SIDECAR_DIR`.

## 8. Node to sidecar protocol

Transport: the node starts the fork server `python -m fish_sidecar.server --socket <path>` once. Messages are JSON objects, one per line (UTF-8, `\n` terminated) on unix sockets.

Control socket (node ↔ fork server):
- `{"op":"spawn","proc_id","plugin_dir","venv_site_packages","data_dir","config","plugin_name","bot_id","socket"}` → fork server forks a child that loads the plugin and connects to `socket` (a per process socket the node listens on). Reply `{"op":"spawned","proc_id","pid"}` or `{"op":"error","proc_id","message"}`.
- `{"op":"kill","proc_id"}`
- Child exit notification `{"op":"exited","proc_id","pid","code"}`. A child that exits with code 2 before sending `hello` failed to load the plugin (import error or unsupported feature), and the node marks the plugin `error` with the last logged message instead of restarting it.

Install: the node runs `uv venv --system-site-packages` + `uv pip install -r requirements.txt` (shared uv cache) inside the plugin dir before spawning. Install output goes to the bot logs with source `plugin:<name>`.

Plugin process socket (node ↔ one plugin process):
- child → node `{"op":"hello","proc_id","plugin":{"name","version","handlers":[{"id","kind":"command"|"command_group"|"regex"|"event_message_type"|"llm_request"|"decorating_result"|"on_loaded"|"platform_loaded","pattern"?,"priority"}]}}`
- node → child `{"op":"event","event_id","event": MessageEvent}`
- child → node `{"op":"matched","event_id","matched": bool}` (sent quickly after filter matching, before running handlers)
- child → node `{"op":"send","event_id"?,"session","chain":[Component]}` (replies, may be several)
- child → node `{"op":"done","event_id","stop": bool, "call_llm": bool}`
- node → child `{"op":"llm_hook","event_id","request":{"prompt","system_prompt","contexts"}}` → child `{"op":"llm_hook_result","event_id","request":{...}}`
- child → node `{"op":"llm","req_id","prompt","system_prompt"?,"contexts"?,"image_urls"?}` → node `{"op":"llm_result","req_id","text"?,"error"?}` (uses the bot's LLM key)
- child → node `{"op":"send_to","session","chain"}` (proactive send by unified_msg_origin)
- child → node `{"op":"log","level","message"}`
- node → child `{"op":"config","config"}` (config update) and `{"op":"shutdown"}`

```
MessageEvent = {platform: "qq_official"|"weixin_ilink", bot_id, self_id,
  message_type: "group"|"private", group_id?, session_id, unified_msg_origin: "<platform_id>:<GroupMessage|FriendMessage>:<session_id>",
  message_id, sender: {user_id, nickname}, message_str, chain: [Component], is_at_or_wake: bool, is_admin: bool, timestamp, raw?}
Component = {"type":"plain","text"} | {"type":"image","url"?,"path"?,"base64"?} | {"type":"at","user_id","name"?}
          | {"type":"reply","id"} | {"type":"record","url"?,"path"?} | {"type":"face","id"}
```

`message_str` arrives with the matched wake prefix already removed by the node, so the sidecar passes it to plugins unchanged.

Plugins may run handlers concurrently. The node waits up to 2s for `matched` from every plugin, then runs the default LLM only if nobody matched or every matched plugin ended with `call_llm: true`.

## 9. `astrbot` compatibility package scope (sidecar)

Reimplement from scratch the public plugin API names that common plugins import, without copying AstrBot source: `astrbot.api` (`logger`, `AstrBotConfig`, `llm_tool` optional), `astrbot.api.star` (`Star`, `Context`, `register`, `StarTools`), `astrbot.api.event` (`AstrMessageEvent`, `MessageEventResult`, `MessageChain`, `filter` with `command`, `command_group`, `regex`, `event_message_type`, `EventMessageType`, `permission_type`, `PermissionType`, `on_llm_request`, `on_decorating_result`, `on_astrbot_loaded`, `platform_adapter_type`), `astrbot.api.message_components` (`Plain`, `Image`, `At`, `Reply`, `Record`, `Face`), `astrbot.api.platform` (`MessageType`, `AstrBotMessage`), `astrbot.api.provider` (`ProviderRequest`, `LLMResponse`), `astrbot.core.utils.astrbot_path` helpers. Unsupported features raise a clear `NotImplementedError` naming the feature, and the plugin status becomes `error` with that message. The supported list lives in `sidecar/COMPAT.md`.

## 10. License boundary

AstrBot is AGPL-3.0. Read it to learn protocols and the plugin API surface, then write new code. Do not translate or copy AstrBot source. The [README](../README.md) records the whale sprite assets' source, modifications and licence.
