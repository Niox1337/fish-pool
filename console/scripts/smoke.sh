#!/usr/bin/env bash
# End to end smoke test of fish-console against a local D1 and the fake node.
#
# Usage: scripts/smoke.sh [--build] [--keep]
#   --build  rebuild the wasm bundle with worker-build first (done automatically when missing)
#   --keep   keep the temp directory with logs and local state
#
# Needs: wrangler, python3, jq, curl, openssl. Nothing on the Cloudflare account is touched:
# the database is local (--local, --persist-to a temp dir) and nothing is deployed.

set -uo pipefail

CONSOLE_DIR="$(cd "$(dirname "$0")/.." && pwd)"
PORT="${SMOKE_PORT:-8799}"
NODE_PORT="${SMOKE_NODE_PORT:-8701}"
BASE="http://127.0.0.1:$PORT"
NODE="http://127.0.0.1:$NODE_PORT"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/fish-smoke.XXXXXX")"
BUILD=0
KEEP=0
for a in "$@"; do
  case "$a" in
    --build) BUILD=1 ;;
    --keep) KEEP=1 ;;
  esac
done

PASS=0
FAIL=0
WRANGLER_PID=""
NODE_PID=""

cleanup() {
  [ -n "$WRANGLER_PID" ] && kill "$WRANGLER_PID" 2>/dev/null
  # wrangler starts node and workerd children that outlive the subshell.
  pkill -f "persist-to $TMP/state" 2>/dev/null
  [ -n "$NODE_PID" ] && kill "$NODE_PID" 2>/dev/null
  wait 2>/dev/null
  if [ "$KEEP" = 1 ] || [ "$FAIL" != 0 ]; then
    echo "logs and state kept in $TMP"
  else
    rm -rf "$TMP"
  fi
}
trap cleanup EXIT

ok() { PASS=$((PASS + 1)); printf '  \033[32mok\033[0m   %s\n' "$1"; }
bad() { FAIL=$((FAIL + 1)); printf '  \033[31mFAIL\033[0m %s\n' "$1"; [ -n "${2:-}" ] && printf '       %s\n' "$2"; }

# ---------- build and config ----------

if [ "$BUILD" = 1 ] || [ ! -f "$CONSOLE_DIR/build/index.js" ]; then
  echo "building wasm bundle"
  (cd "$CONSOLE_DIR" && CC=clang worker-build --release) || exit 1
fi

ASSETS="$CONSOLE_DIR/../web/dist"
if [ ! -f "$ASSETS/index.html" ]; then
  ASSETS="$TMP/dist"
  mkdir -p "$ASSETS"
  echo '<!doctype html><title>fish placeholder</title><div id="root">placeholder</div>' > "$ASSETS/index.html"
fi

DATA_KEY="$(openssl rand -base64 32)"
NODE_SECRET="$(openssl rand -base64 32)"

cat > "$TMP/wrangler.toml" <<EOF
name = "fish-console-smoke"
main = "$CONSOLE_DIR/build/index.js"
compatibility_date = "2026-01-01"
assets = { directory = "$ASSETS", binding = "ASSETS", not_found_handling = "single-page-application", run_worker_first = ["/api", "/api/*", "/__scheduled"] }

[[d1_databases]]
binding = "DB"
database_name = "fish"
database_id = "00000000-0000-0000-0000-000000000000"
migrations_dir = "$CONSOLE_DIR/migrations"
EOF

cat > "$TMP/dev.env" <<EOF
FISH_DEV=1
EMAIL_PROVIDER=log
EMAIL_FROM=noreply@niox.lol
DEFAULT_NODE_ID=node-1
DEFAULT_NODE_URL=$NODE
DATA_KEY=$DATA_KEY
NODE_SECRET=$NODE_SECRET
GITHUB_API_BASE=$NODE/gh-api
GITHUB_RAW_BASE=$NODE/gh-raw
GITHUB_GIT_BASE=$NODE/gh-git
GENGWIKI_GATEWAY=$NODE/gw
GENGWIKI_GATEWAY_TOKEN=gw-service-token
ADMIN_EMAILS=admin@example.com
EOF

echo "applying migrations to local D1"
(cd "$TMP" && CI=1 wrangler d1 migrations apply DB --local --config "$TMP/wrangler.toml" --persist-to "$TMP/state" > "$TMP/migrate.log" 2>&1) || {
  cat "$TMP/migrate.log"; exit 1;
}

start_node() {
  FISH_NODE_SECRET="$NODE_SECRET" FISH_NODE_ID=node-1 FAKE_NODE_PORT="$NODE_PORT" FISH_CONSOLE_URL="$BASE" \
    python3 "$CONSOLE_DIR/scripts/fake_node.py" >> "$TMP/node.log" 2>&1 &
  NODE_PID=$!
  for _ in $(seq 1 50); do
    curl -s "$NODE/_debug/stats" > /dev/null && return 0
    sleep 0.1
  done
  echo "fake node did not start"; cat "$TMP/node.log"; exit 1
}

stop_node() {
  kill "$NODE_PID" 2>/dev/null
  wait "$NODE_PID" 2>/dev/null
  NODE_PID=""
}

for port in "$PORT" "$NODE_PORT"; do
  if curl -s -o /dev/null --max-time 2 "http://127.0.0.1:$port/"; then
    echo "port $port is already in use, stop the process listening there first"; exit 1
  fi
done

start_node
echo "starting wrangler dev on $BASE"
(cd "$TMP" && wrangler dev --config "$TMP/wrangler.toml" --local --ip 127.0.0.1 --port "$PORT" \
  --persist-to "$TMP/state" --env-file "$TMP/dev.env" --test-scheduled --show-interactive-dev-session=false > "$TMP/wrangler.log" 2>&1) &
WRANGLER_PID=$!
for _ in $(seq 1 120); do
  code=$(curl -s -o /dev/null -w '%{http_code}' "$BASE/api/me" || true)
  [ "$code" = 401 ] && break
  sleep 0.5
done
if [ "$code" != 401 ]; then
  echo "wrangler dev did not become ready"; tail -50 "$TMP/wrangler.log"; exit 1
fi

# ---------- helpers ----------

COOKIE=""
STATUS=""
BODY=""

# req METHOD PATH [JSON] [--no-csrf]
req() {
  local m="$1" p="$2" d="${3:-}" csrf="${4:-}"
  local args=(-s -o "$TMP/body" -D "$TMP/headers" -w '%{http_code}' -X "$m")
  [ "$csrf" != "--no-csrf" ] && args+=(-H 'X-Fish-Csrf: 1')
  [ -n "$COOKIE" ] && args+=(-H "Cookie: $COOKIE")
  [ -n "$d" ] && args+=(-H 'Content-Type: application/json' --data-binary "$d")
  STATUS=$(curl "${args[@]}" "$BASE$p")
  BODY=$(cat "$TMP/body")
}

# check NAME EXPECTED_STATUS [JQ_BOOLEAN_EXPR]
check() {
  local name="$1" want="$2" expr="${3:-true}"
  if [ "$STATUS" != "$want" ]; then
    bad "$name" "status $STATUS, want $want, body: ${BODY:0:300}"
    return 1
  fi
  if [ "$(printf '%s' "$BODY" | jq -r "$expr" 2>/dev/null)" != "true" ]; then
    bad "$name" "jq '$expr' not true, body: ${BODY:0:300}"
    return 1
  fi
  ok "$name"
}

# node_check NAME PATH JQ_BOOLEAN_EXPR (against the fake node debug API)
node_check() {
  local out
  out=$(curl -s "$NODE$2")
  if [ "$(printf '%s' "$out" | jq -r "$3" 2>/dev/null)" = "true" ]; then ok "$1"; else bad "$1" "${out:0:300}"; fi
}

j() { printf '%s' "$BODY" | jq -r "$1"; }

grab_cookie() {
  local c
  c=$(grep -i '^set-cookie: fish_session=' "$TMP/headers" | head -1 | sed -E 's/^[Ss]et-[Cc]ookie: (fish_session=[^;]*).*/\1/' | tr -d '\r')
  [ -n "$c" ] && COOKIE="$c"
}

wait_log() {
  local pattern="$1" line
  for _ in $(seq 1 40); do
    line=$(grep -a -E "$pattern" "$TMP/wrangler.log" | tail -1)
    [ -n "$line" ] && { printf '%s' "$line"; return 0; }
    sleep 0.25
  done
  return 1
}

new_captcha() {
  req GET /api/captcha
  CAPTCHA_ID=$(j .id)
  CAPTCHA_ANSWER=$(wait_log "\[dev\] captcha $CAPTCHA_ID answer" | sed -E 's/.*answer ([A-Z0-9]+).*/\1/')
}

email_code_for() {
  wait_log "\[dev\] email code for $1: [0-9]{6}" | sed -E 's/.*: ([0-9]{6}).*/\1/'
}

# d1 SQL: runs a statement against the local database used by wrangler dev
d1() {
  (cd "$TMP" && CI=1 wrangler d1 execute DB --local --config "$TMP/wrangler.toml" --persist-to "$TMP/state" --json --command "$1" 2> "$TMP/d1.err")
}

make_invite() {
  CI=1 "$CONSOLE_DIR/scripts/make-invite.sh" "$1" --config "$TMP/wrangler.toml" --persist-to "$TMP/state" 2> "$TMP/make-invite.log" | tail -1
}

# no_secrets NAME TEXT: TEXT must not contain plaintext secrets or stored secret columns
no_secrets() {
  if printf '%s' "$2" | grep -q -E 'sk-good|gwk_ai_|qq-secret|wx-token|manual-secret|password_hash|pbkdf2|secret_enc|api_key_enc|"secret"|"token"'; then
    bad "$1" "$(printf '%s' "$2" | grep -o -E '.{40}(sk-good|gwk_ai_|qq-secret|wx-token|manual-secret|password_hash|pbkdf2|secret_enc|api_key_enc|"secret"|"token").{20}' | head -3)"
  else
    ok "$1"
  fi
}

EMAIL="smoke$(date +%s)@example.com"
EMAIL2="second$(date +%s)@example.com"
PASSWORD="fishfish123"
DAY_MS=86400000

echo "static assets"
code=$(curl -s -o "$TMP/page" -w '%{http_code}' "$BASE/")
[ "$code" = 200 ] && grep -q '<' "$TMP/page" && ok "GET / serves the SPA" || bad "GET / serves the SPA" "status $code"
code=$(curl -s -o "$TMP/page" -w '%{http_code}' "$BASE/bots/some/deep/link")
[ "$code" = 200 ] && grep -q '<' "$TMP/page" && ok "SPA fallback for deep links" || bad "SPA fallback for deep links" "status $code"
for path in /login /app/bots/some/deep/link; do
  code=$(curl -s -o "$TMP/page" -w '%{http_code}' "$BASE$path")
  [ "$code" = 200 ] && cmp -s "$TMP/page" "$ASSETS/index.html" && ok "SPA index fallback for $path" || bad "SPA index fallback for $path" "status $code"
done
req GET /api/does-not-exist
check "unknown API route is a JSON 404" 404 '.error.code == "not_found"'
req GET /api
check "bare API route is a JSON 404" 404 '.error.code == "not_found"'
for path in /api /api/does-not-exist; do
  STATUS=$(curl -s -o "$TMP/body" -w '%{http_code}' -H 'Sec-Fetch-Mode: navigate' "$BASE$path")
  BODY=$(cat "$TMP/body")
  check "API navigation stays in Rust for $path" 404 '.error.code == "not_found"'
done

echo "auth"
req GET /api/me
check "me without session" 401 '.error.code == "unauthorized" and (.error.message | length > 0)'
req POST /api/auth/logout '{}' --no-csrf
check "mutation without CSRF header" 403 '.error.code == "csrf_failed"'

INVITE=$(make_invite 30)
if [[ "$INVITE" =~ ^FISH-[0-9A-HJKMNP-TV-Z]{4}-[0-9A-HJKMNP-TV-Z]{4}$ ]]; then ok "make-invite.sh prints a code"; else bad "make-invite.sh prints a code" "$INVITE $(tail -5 "$TMP/make-invite.log")"; fi
# The same code typed in lowercase without dashes.
INVITE_TYPED=$(printf '%s' "$INVITE" | tr -d '-' | tr 'A-Z' 'a-z')

req GET /api/captcha
check "captcha issued" 200 '(.id | length) == 26 and (.svg | startswith("<svg")) and (.svg | contains("<text") | not) and (.svg | contains("<path"))'

req POST /api/auth/email-code "{\"email\":\"$EMAIL\",\"purpose\":\"register\",\"captcha_id\":\"$(j .id)\",\"captcha_answer\":\"WRONG\"}"
check "email code rejects wrong captcha" 400 '.error.code == "captcha_invalid"'

new_captcha
req POST /api/auth/email-code "{\"email\":\"$EMAIL\",\"purpose\":\"register\",\"captcha_id\":\"$CAPTCHA_ID\",\"captcha_answer\":\"$CAPTCHA_ANSWER\"}"
check "register code needs an invite" 400 '.error.code == "invite_invalid"'
new_captcha
req POST /api/auth/email-code "{\"email\":\"$EMAIL\",\"purpose\":\"register\",\"captcha_id\":\"$CAPTCHA_ID\",\"captcha_answer\":\"$CAPTCHA_ANSWER\",\"invite_code\":\"FISH-0000-0000\"}"
check "unknown invite rejected" 400 '.error.code == "invite_invalid"'
sleep 0.3
grep -a -q "\[dev\] email code for $EMAIL" "$TMP/wrangler.log" && bad "no mail for a bad invite" || ok "no mail for a bad invite"

new_captcha
req POST /api/auth/email-code "{\"email\":\"$EMAIL\",\"purpose\":\"register\",\"captcha_id\":\"$CAPTCHA_ID\",\"captcha_answer\":\"$(echo "$CAPTCHA_ANSWER" | tr 'A-Z' 'a-z')\",\"invite_code\":\"$INVITE_TYPED\"}"
check "email code sent (captcha case insensitive, invite normalized)" 200 '.ok == true and .cooldown_s == 60'
REG_CODE=$(email_code_for "$EMAIL")
[ -n "$REG_CODE" ] && ok "dev log shows the email code" || bad "dev log shows the email code"

req POST /api/auth/email-code "{\"email\":\"$EMAIL\",\"purpose\":\"register\",\"captcha_id\":\"$CAPTCHA_ID\",\"captcha_answer\":\"$CAPTCHA_ANSWER\",\"invite_code\":\"$INVITE\"}"
check "captcha is single use" 400 '.error.code == "captcha_invalid"'

new_captcha
req POST /api/auth/email-code "{\"email\":\"$EMAIL\",\"purpose\":\"register\",\"captcha_id\":\"$CAPTCHA_ID\",\"captcha_answer\":\"$CAPTCHA_ANSWER\",\"invite_code\":\"$INVITE\"}"
check "email code cooldown" 429 '.error.code == "rate_limited"'

req POST /api/auth/register "{\"email\":\"$EMAIL\",\"password\":\"$PASSWORD\",\"code\":\"$REG_CODE\",\"agree\":false}"
check "register requires agreement" 400 '.error.code == "terms_not_accepted"'
req POST /api/auth/register "{\"email\":\"$EMAIL\",\"password\":\"$PASSWORD\",\"code\":\"000000x\",\"invite_code\":\"$INVITE\",\"agree\":true}"
check "register rejects wrong code" 400 '.error.code == "code_invalid"'
req POST /api/auth/register "{\"email\":\"$EMAIL\",\"password\":\"short\",\"code\":\"$REG_CODE\",\"agree\":true}"
check "register rejects weak password" 400 '.error.code == "weak_password"'
req POST /api/auth/register "{\"email\":\"$EMAIL\",\"password\":\"$PASSWORD\",\"code\":\"$REG_CODE\",\"agree\":true}"
check "register without invite fails" 400 '.error.code == "invite_invalid"'
T0=$(($(date +%s) * 1000))
req POST /api/auth/register "{\"email\":\"$EMAIL\",\"password\":\"$PASSWORD\",\"code\":\"$REG_CODE\",\"invite_code\":\"$INVITE_TYPED\",\"agree\":true}"
check "register succeeds and sets service_until" 200 ".user.email == \"$EMAIL\" and .user.role == \"user\" and .user.quota.bots == null and .user.quota.plugins == null and .user.quota.memory_mb == 128 and .user.quota.storage_mb == 128 and .user.service_active == true and (.user.service_until - $T0 - 30 * $DAY_MS | fabs) < 120000"
USER_ID=$(j .user.id)
grab_cookie
grep -i '^set-cookie: fish_session=' "$TMP/headers" | grep -q 'HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=2592000' \
  && ok "session cookie attributes" || bad "session cookie attributes" "$(grep -i set-cookie "$TMP/headers")"
req GET /api/me
check "me with session" 200 ".user.email == \"$EMAIL\""

req POST /api/auth/logout '{}'
check "logout" 200 '.ok == true'
req GET /api/me
check "session revoked after logout" 401 '.error.code == "unauthorized"'
COOKIE=""

for i in 1 2 3 4; do
  req POST /api/auth/login "{\"email\":\"$EMAIL\",\"password\":\"wrong-pass-$i\"}"
done
check "wrong password" 401 '.error.code == "invalid_credentials"'
req POST /api/auth/login "{\"email\":\"$EMAIL\",\"password\":\"wrong-pass-5\"}"
check "fifth failure asks for captcha" 400 '.error.code == "captcha_required"'
req POST /api/auth/login "{\"email\":\"$EMAIL\",\"password\":\"$PASSWORD\"}"
check "captcha required even with right password" 400 '.error.code == "captcha_required"'
new_captcha
req POST /api/auth/login "{\"email\":\"$EMAIL\",\"password\":\"$PASSWORD\",\"captcha_id\":\"$CAPTCHA_ID\",\"captcha_answer\":\"$CAPTCHA_ANSWER\"}"
check "login with captcha" 200 ".user.email == \"$EMAIL\""
grab_cookie
OLD_COOKIE="$COOKIE"

new_captcha
req POST /api/auth/email-code "{\"email\":\"$EMAIL\",\"purpose\":\"reset\",\"captcha_id\":\"$CAPTCHA_ID\",\"captcha_answer\":\"$CAPTCHA_ANSWER\"}"
check "reset code sent" 200 '.ok == true'
sleep 0.5
RESET_CODE=$(grep -a -E "\[dev\] email code for $EMAIL: [0-9]{6}" "$TMP/wrangler.log" | tail -1 | sed -E 's/.*: ([0-9]{6}).*/\1/')
NEW_PASSWORD="newfish456!"
req POST /api/auth/reset "{\"email\":\"$EMAIL\",\"code\":\"$RESET_CODE\",\"password\":\"$NEW_PASSWORD\"}"
check "password reset" 200 '.ok == true'
req GET /api/me
check "reset revokes sessions" 401 'true'
req POST /api/auth/login "{\"email\":\"$EMAIL\",\"password\":\"$NEW_PASSWORD\"}"
check "login with new password, failures cleared" 200 ".user.email == \"$EMAIL\""
grab_cookie

new_captcha
req POST /api/auth/email-code "{\"email\":\"$EMAIL2\",\"purpose\":\"register\",\"captcha_id\":\"$CAPTCHA_ID\",\"captcha_answer\":\"$CAPTCHA_ANSWER\",\"invite_code\":\"$INVITE\"}"
check "used invite rejected" 400 '.error.code == "invite_used"'
used=$(d1 "SELECT used_count FROM invite_codes WHERE code = '$INVITE'" | jq -r '.[0].results[0].used_count')
[ "$used" = 1 ] && ok "invite used_count is 1" || bad "invite used_count is 1" "$used"

echo "llm keys"
req POST /api/llm-keys "{\"name\":\"bad\",\"base_url\":\"$NODE/llm/v1\",\"model\":\"fake-chat\",\"api_key\":\"sk-bad-123456\"}"
check "invalid key rejected with upstream message" 400 '.error.code == "llm_key_invalid" and (.error.message | contains("Invalid API key"))'
req POST /api/llm-keys "{\"name\":\"bad\",\"base_url\":\"ftp://x\",\"model\":\"m\",\"api_key\":\"k\"}"
check "bad base url rejected" 400 '.error.code == "invalid_input"'
req POST /api/llm-keys "{\"name\":\"测试\",\"base_url\":\"$NODE/llm/v1/\",\"model\":\"fake-chat\",\"api_key\":\"sk-good-abcdef1234\"}"
check "valid key stored" 200 '.key_masked == "sk-…1234" and (.base_url | endswith("/llm/v1"))'
KEY_ID=$(j .id)
req GET /api/llm-keys
check "keys listed without plaintext" 200 '(.items | length) == 1 and (tostring | contains("sk-good") | not)'

echo "bots"
req POST /api/bots "{\"name\":\"小鱼一号\",\"key_id\":\"$KEY_ID\",\"model\":\"fake-chat\"}"
check "bot created" 200 ".status == \"starting\" and .settings.llm.key_id == \"$KEY_ID\" and .platforms == [] and .plugins == []"
BOT=$(j .id)
node_check "node received DesiredBot with decrypted key" "/_debug/desired/$BOT" ".running == true and .llm.api_key == \"sk-good-abcdef1234\" and .llm.model == \"fake-chat\" and .quota.memory_mb == 128 and .name == \"小鱼一号\""
req POST /api/bots "{\"name\":\"小鱼二号\",\"key_id\":\"$KEY_ID\",\"model\":\"fake-chat\"}"
check "default quota allows another bot" 200 '.name == "小鱼二号"'
SECOND_BOT=$(j .id)
req DELETE "/api/bots/$SECOND_BOT"
check "extra bot deleted" 200 '.ok == true'
req GET /api/bots
check "bot list" 200 "(.items | length) == 1 and .items[0].id == \"$BOT\""
req PATCH "/api/bots/$BOT" '{"settings":{"persona":{"system_prompt":"你是测试鱼"},"llm":{"temperature":1.1}}}'
check "settings patched" 200 '.settings.persona.system_prompt == "你是测试鱼" and .settings.llm.temperature == 1.1 and .settings.llm.max_tokens == 1024'
node_check "settings pushed" "/_debug/desired/$BOT" '.settings.persona.system_prompt == "你是测试鱼"'
req PATCH "/api/bots/$BOT" '{"settings":{"llm":{"temperature":5}}}'
check "invalid settings rejected" 400 '.error.code == "invalid_input"'
req PATCH "/api/bots/$BOT" '{"settings":{"llm":{"key_id":"nope"}}}'
check "foreign key id rejected" 400 '.error.code == "invalid_input"'
req GET "/api/bots/$BOT"
check "bot status refreshed from node" 200 '.status == "running"'

curl -s "$NODE/_debug/emit?bot_id=$BOT&status=error" > "$TMP/emit.json"
[ "$(jq -r .console_status "$TMP/emit.json")" = 200 ] && ok "signed node event accepted" || bad "signed node event accepted" "$(cat "$TMP/emit.json")"
req GET /api/bots
check "node event updated status" 200 '.items[0].status == "error" and .items[0].status_detail == "来自节点的事件"'
code=$(curl -s -o "$TMP/body" -w '%{http_code}' -X POST -H 'X-Fish-Node: node-1' -H "X-Fish-Ts: $(($(date +%s) * 1000))" -H 'X-Fish-Sig: deadbeef' \
  --data-binary '{"node_id":"node-1","events":[]}' "$BASE/api/internal/node/events")
[ "$code" = 401 ] && [ "$(jq -r .error.code "$TMP/body")" = bad_signature ] && ok "forged node event rejected" || bad "forged node event rejected" "$code $(cat "$TMP/body")"

req POST "/api/bots/$BOT/stop" '{}'
check "stop" 200 '.status == "stopped"'
node_check "stop pushed" "/_debug/desired/$BOT" '.running == false'
req POST "/api/bots/$BOT/start" '{}'
check "start" 200 '.status == "starting"'
req POST "/api/bots/$BOT/restart" '{}'
check "restart" 200 '.status == "starting"'
node_check "restart reached node" "/_debug/stats" "(.restarts | index(\"$BOT\")) != null"
req GET "/api/bots/$BOT/logs"
check "logs proxied" 200 '(.items | length) == 3 and .cursor == "3" and .items[0].source == "core"'
req GET "/api/bots/$BOT/logs?after=3"
check "logs cursor" 200 '.cursor == "6"'

echo "platforms"
req POST "/api/bots/$BOT/platforms/qq/qr" '{}'
check "qq qr started" 200 '(.task_id | length) > 0 and (.qr_url | length) > 0 and .interval_s == 2'
TASK=$(j .task_id)
req GET "/api/bots/$BOT/platforms/qq/qr/$TASK"
check "qq qr pending" 200 '.status == "pending"'
req GET "/api/bots/$BOT/platforms/qq/qr/$TASK"
check "qq qr created" 200 '.status == "created" and .platform.kind == "qq_official" and .platform.app_id == "102030405" and .platform.secret_masked == "qq-…efgh"'
QQ_PID=$(j .platform.id)
req GET "/api/bots/$BOT/platforms/qq/qr/$TASK"
check "qq qr result is stable" 200 ".status == \"created\" and .platform.id == \"$QQ_PID\""
req GET "/api/bots/$BOT/platforms/qq/qr/unknown-task"
check "unknown qr task" 404 '.error.code == "not_found"'
node_check "qq credentials pushed" "/_debug/desired/$BOT" '.platforms[0].secret == "qq-secret-abcdefgh" and .platforms[0].enabled == true'

req POST "/api/bots/$BOT/platforms/weixin/qr" '{}'
WX_TASK=$(j .task_id)
req GET "/api/bots/$BOT/platforms/weixin/qr/$WX_TASK"
check "weixin pending" 200 '.status == "pending"'
req GET "/api/bots/$BOT/platforms/weixin/qr/$WX_TASK"
check "weixin scanned" 200 '.status == "scanned"'
req GET "/api/bots/$BOT/platforms/weixin/qr/$WX_TASK"
check "weixin created" 200 '.status == "created" and .platform.kind == "weixin_ilink" and .platform.app_id == "wxbot@im.bot"'
node_check "weixin extra pushed" "/_debug/desired/$BOT" '[.platforms[] | select(.kind == "weixin_ilink")][0].extra.base_url == "https://ilinkai.weixin.qq.com"'

req POST "/api/bots/$BOT/platforms/qq/manual" '{"app_id":"999","secret":"manual-secret-xyz1"}'
check "manual qq" 200 '.app_id == "999" and .secret_masked == "man…xyz1" and .enabled == true'
MANUAL_PID=$(j .id)
req PATCH "/api/bots/$BOT/platforms/$QQ_PID" '{"enabled":false,"display_name":"主号"}'
check "platform disabled" 200 '.enabled == false and .status == "disabled" and .display_name == "主号"'
node_check "disable pushed" "/_debug/desired/$BOT" "[.platforms[] | select(.id == \"$QQ_PID\")][0].enabled == false"
req DELETE "/api/bots/$BOT/platforms/$MANUAL_PID"
check "platform deleted" 200 '.ok == true'
node_check "platform removal pushed" "/_debug/desired/$BOT" '(.platforms | length) == 2'

echo "plugins"
req POST /api/plugins/inspect '{"repo_url":"https://gitlab.com/a/b"}'
check "non GitHub url rejected" 400 '.error.code == "repo_invalid"'
req POST /api/plugins/inspect '{"repo_url":"https://github.com/fish-test/missing"}'
check "missing repo rejected" 400 '.error.code == "repo_invalid"'
req POST /api/plugins/inspect '{"repo_url":"https://github.com/fish-test/not_a_plugin"}'
check "repo without metadata rejected" 400 '.error.code == "not_astrbot_plugin"'
req POST /api/plugins/inspect '{"repo_url":"https://github.com/fish-test/astrbot_plugin_demo.git/"}'
check "plugin inspected" 200 '.name == "astrbot_plugin_demo" and .display_name == "演示插件" and .version == "v1.0.0" and .has_requirements == true and .schema.times.type == "int" and .repo_url == "https://github.com/fish-test/astrbot_plugin_demo"'
req POST "/api/bots/$BOT/plugins" '{"repo_url":"https://github.com/fish-test/astrbot_plugin_demo"}'
check "plugin installed" 200 '.status == "installing" and .config == {"greeting":"你好","times":1,"loud":false} and .enabled == true'
PLUG=$(j .id)
node_check "plugin pushed" "/_debug/desired/$BOT" '(.plugins | length) == 1 and .plugins[0].name == "astrbot_plugin_demo" and .plugins[0].config.times == 1'
REV1=$(curl -s "$NODE/_debug/desired/$BOT" | jq .plugins[0].install_rev)
req POST "/api/bots/$BOT/plugins" '{"repo_url":"https://github.com/fish-test/astrbot_plugin_demo"}'
check "duplicate plugin rejected" 409 '.error.code == "plugin_exists"'
req PATCH "/api/bots/$BOT/plugins/$PLUG" '{"config":{"greeting":"嗨","times":9,"loud":true}}'
check "config out of range rejected" 400 '.error.code == "config_invalid"'
req PATCH "/api/bots/$BOT/plugins/$PLUG" '{"config":{"greeting":"嗨","times":3,"loud":true}}'
check "config saved" 200 '.config.times == 3 and .config.greeting == "嗨"'
node_check "config pushed" "/_debug/desired/$BOT" '.plugins[0].config.times == 3'
sleep 0.01
req POST "/api/bots/$BOT/plugins/$PLUG/update" '{}'
check "plugin update keeps config" 200 '.status == "installing" and .config.times == 3'
node_check "update bumps install_rev" "/_debug/desired/$BOT" ".plugins[0].install_rev > $REV1"
req PATCH "/api/bots/$BOT/plugins/$PLUG" '{"enabled":false}'
check "plugin disabled" 200 '.enabled == false and .status == "stopped"'
req PATCH "/api/bots/$BOT/plugins/$PLUG" '{"enabled":true}'
check "plugin enabled" 200 '.enabled == true'
node_check "plugin pinned to the upstream commit" "/_debug/desired/$BOT" ".plugins[0].commit == \"$(printf 'a%.0s' $(seq 40))\""
REV2=$(curl -s "$NODE/_debug/desired/$BOT" | jq .plugins[0].install_rev)
NEW_COMMIT=$(printf 'b%.0s' $(seq 40))
curl -s "$NODE/_debug/commit?sha=$NEW_COMMIT" > /dev/null
sleep 0.01
code=$(curl -s -o /dev/null -w '%{http_code}' "$BASE/__scheduled?cron=*/5+*+*+*+*")
node_check "cron moves plugins to a new upstream commit" "/_debug/desired/$BOT" ".plugins[0].commit == \"$NEW_COMMIT\" and .plugins[0].version == \"v1.1.0\" and .plugins[0].install_rev > $REV2 and .plugins[0].config.times == 3"
req GET "/api/bots/$BOT"
check "automatic update shows the new version and commit" 200 "[.plugins[] | select(.id == \"$PLUG\")][0] | .version == \"v1.1.0\" and .commit == \"$NEW_COMMIT\""
REV3=$(curl -s "$NODE/_debug/desired/$BOT" | jq .plugins[0].install_rev)
curl -s -o /dev/null "$BASE/__scheduled?cron=*/5+*+*+*+*"
node_check "a checked repository waits for the next interval" "/_debug/desired/$BOT" ".plugins[0].install_rev == $REV3"

echo "usage"
req GET /api/usage
check "usage aggregated" 200 '.quota.memory_mb == 128 and .memory_bytes > 0 and ([.sources[] | select(.kind == "plugin")] | length) == 1 and (.history | length) == 3'

echo "admin"
USER_COOKIE="$COOKIE"
req GET /api/admin/overview
check "non-admin gets 403 on overview" 403 '.error.code == "forbidden"'
req GET /api/admin/users
check "non-admin gets 403 on users" 403 '.error.code == "forbidden"'
req GET /api/admin/system
check "non-admin gets 403 on system" 403 '.error.code == "forbidden"'
req POST /api/admin/invites '{"days":30}'
check "non-admin cannot create invites" 403 '.error.code == "forbidden"'

COOKIE=""
ADMIN_EMAIL="admin@example.com"
new_captcha
req POST /api/auth/email-code "{\"email\":\"$ADMIN_EMAIL\",\"purpose\":\"register\",\"captcha_id\":\"$CAPTCHA_ID\",\"captcha_answer\":\"$CAPTCHA_ANSWER\"}"
check "ADMIN_EMAILS address gets a code without invite" 200 '.ok == true'
ADMIN_CODE=$(email_code_for "$ADMIN_EMAIL")
req POST /api/auth/register "{\"email\":\"$ADMIN_EMAIL\",\"password\":\"$PASSWORD\",\"code\":\"$ADMIN_CODE\",\"agree\":true}"
check "admin registers and is promoted" 200 '.user.role == "admin" and .user.service_active == true and .user.service_until == null'
grab_cookie
ADMIN_COOKIE="$COOKIE"
ADMIN_BODIES=""

curl -s "$NODE/_debug/emit?bot_id=$BOT&kind=plugin_status&id=$PLUG&status=over_quota" > /dev/null
req GET /api/admin/overview
check "overview" 200 '.users.total >= 2 and .users.active >= 2 and .bots.total >= 1 and .plugins.total >= 1 and .platforms.qq_official >= 1 and .platforms.weixin_ilink >= 1 and .nodes[0].online == true and .nodes[0].version == "fake" and .nodes[0].last_seen > 0 and .usage.memory_bytes > 0 and .usage.quota_memory_mb >= 256 and (.plugins | has("sleeping")) and (.invites | has("unused") and has("used_7d")) and .invites.used_7d >= 1'
check "overview reports plugin CPU from node" 200 ".plugin_cpu.reporting_nodes == 1 and .plugin_cpu.percent == 1 and (.plugin_cpu.items | any(.plugin_id == \"$PLUG\" and .node_id == \"node-1\" and .cpu_percent == 1 and .sampled_at > 0))"
check "overview lists problems" 200 "[.problems[] | select(.kind == \"plugin\" and .id == \"$PLUG\" and .status == \"over_quota\" and .user_email == \"$EMAIL\")] | length == 1"
ADMIN_BODIES="$ADMIN_BODIES$BODY"
req GET /api/admin/system
check "admin system view reports D1 tables" 200 '(.console.d1.tables | length) == 16 and ([.console.d1.tables[] | select(.name == "users")][0].rows >= 2) and .console.d1.round_trip_ms >= 0 and .generated_at > 0'
check "admin system view embeds node metrics" 200 ".nodes[0].online == true and .nodes[0].latency_ms >= 0 and .nodes[0].system.host.cpu_count == 2 and .nodes[0].system.container.memory_max_bytes > 0 and (.nodes[0].system.processes | any(.role == \"plugin\" and .plugin.plugin_id == \"$PLUG\")) and .nodes[0].last_seen > 0"
check "admin system view maps user emails" 200 ".users[\"$USER_ID\"] == \"$EMAIL\""
ADMIN_BODIES="$ADMIN_BODIES$BODY"
req GET /api/admin/users
check "admin lists users" 200 "(.items | length) >= 2 and .cursor == null and ([.items[] | select(.email == \"$EMAIL\")][0] | .bots == 1 and .plugins == 1 and .memory_bytes > 0 and .storage_bytes > 0 and .last_login_at > 0 and .service_active == true)"
check "users newest first" 200 ".items[0].email == \"$ADMIN_EMAIL\""
ADMIN_BODIES="$ADMIN_BODIES$BODY"
req GET "/api/admin/users?q=SMOKE"
check "user search" 200 "(.items | length) == 1 and .items[0].email == \"$EMAIL\""
req GET "/api/admin/users?q=%25"
check "user search escapes LIKE wildcards" 200 '(.items | length) == 0'
req GET "/api/admin/users/$USER_ID"
check "admin reads user detail" 200 ".user.email == \"$EMAIL\" and (.bots | length) == 1 and .bots[0].id == \"$BOT\" and (.bots[0].platforms | length) == 2"
ADMIN_BODIES="$ADMIN_BODIES$BODY"
req GET "/api/admin/users/$USER_ID/usage"
check "admin reads user usage" 200 '.memory_bytes > 0 and .quota.memory_mb == 128'
ADMIN_BODIES="$ADMIN_BODIES$BODY"
req GET "/api/admin/users/$USER_ID/bots/$BOT/logs"
check "admin reads user bot logs" 200 '(.items | length) == 3'
req GET "/api/admin/users/$USER_ID/bots/nope/logs"
check "logs of a foreign bot id" 404 '.error.code == "not_found"'
req PATCH "/api/admin/users/$USER_ID" '{"quota":{"memory_mb":1}}'
check "quota out of range rejected" 400 '.error.code == "invalid_input"'
req PATCH "/api/admin/users/$USER_ID" '{"quota":{"memory_mb":512,"plugins":10}}'
check "quota patched" 200 '.quota.memory_mb == 512 and .quota.plugins == 10 and .quota.bots == null and .bots == 1'
ADMIN_BODIES="$ADMIN_BODIES$BODY"
node_check "new quota pushed to node" "/_debug/desired/$BOT" '.quota.memory_mb == 512'
no_secrets "admin responses carry no secrets" "$ADMIN_BODIES"

echo "invites"
req POST /api/admin/invites '{"days":0}'
check "invite days validated" 400 '.error.code == "invalid_input"'
req POST /api/admin/invites '{"days":7,"quota":{"memory_mb":1,"storage_mb":128,"bots":null,"plugins":null}}'
check "invite quota validated" 400 '.error.code == "invalid_input"'
req POST /api/admin/invites '{"days":7,"count":4,"note":"smoke","quota":{"memory_mb":192,"storage_mb":256,"bots":0,"plugins":2}}'
check "admin creates invites" 200 "(.items | length) == 4 and (.items | all(.code | test(\"^FISH-[0-9A-HJKMNP-TV-Z]{4}-[0-9A-HJKMNP-TV-Z]{4}$\"))) and .items[0].created_by_email == \"$ADMIN_EMAIL\" and .items[0].days == 7 and .items[0].max_uses == 1 and .items[0].redemptions == []"
INV_A=$(j '.items[0].code'); INV_B=$(j '.items[1].code'); INV_C=$(j '.items[2].code'); INV_D=$(j '.items[3].code')
req GET "/api/admin/invites?status=unused"
check "unused invites listed" 200 "[.items[].code] | index(\"$INV_A\") != null and index(\"$INV_B\") != null"
check "invite list returns stored and default quotas" 200 '(.default_quota == {memory_mb:128,storage_mb:128,bots:null,plugins:null}) and (.items | any(.quota == {memory_mb:192,storage_mb:256,bots:0,plugins:2}))'
req DELETE "/api/admin/invites/$INV_A"
check "invite revoked" 200 '.ok == true'
req GET "/api/admin/invites?status=revoked"
check "revoked invite listed" 200 "[.items[] | select(.code == \"$INV_A\" and .revoked_at > 0)] | length == 1"
req DELETE "/api/admin/invites/FISH-0000-0000"
check "revoking an unknown invite" 404 '.error.code == "not_found"'

COOKIE="$USER_COOKIE"
req POST /api/me/redeem "{\"code\":\"$INV_A\"}"
check "revoked invite gives invite_invalid" 400 '.error.code == "invite_invalid"'
req GET /api/me
UNTIL0=$(j .user.service_until)
req POST /api/me/redeem "{\"code\":\"$(printf '%s' "$INV_B" | tr 'A-Z' 'a-z')\"}"
check "redeem extends service by 7 days and preserves quotas" 200 ".user.service_until == ($UNTIL0 + 7 * $DAY_MS) and .user.service_active == true and .user.quota.memory_mb == 512 and .user.quota.plugins == 10 and .user.quota.bots == null"
req POST /api/me/redeem "{\"code\":\"$INV_B\"}"
check "redeemed invite gives invite_used" 400 '.error.code == "invite_used"'
COOKIE="$ADMIN_COOKIE"
req GET "/api/admin/invites?status=used"
check "used invite shows redemption" 200 "[.items[] | select(.code == \"$INV_B\")][0] | .used_count == 1 and .redemptions[0].user_email == \"$EMAIL\" and .redemptions[0].days == 7"

echo "gateway credits"
req POST /api/admin/invites '{"days":1,"gateway_budget":"1000.01"}'
check "gateway credit validated" 400 '.error.code == "invalid_input"'
req POST /api/admin/invites '{"days":1,"count":2,"gateway_budget":"2.5"}'
check "invite carries gateway credit" 200 '(.items | length) == 2 and .items[0].gateway_budget == "2.50"'
INV_G1=$(j '.items[0].code'); INV_G2=$(j '.items[1].code')
COOKIE="$USER_COOKIE"
req POST /api/me/redeem "{\"code\":\"$INV_G1\"}"
check "gateway invite redeemed" 200 '.user.service_active == true'
UNTIL_G1=$(j '.user.service_until')
EXPIRY_G1=$(python3 -c "import datetime,sys;print(datetime.datetime.fromtimestamp(int(sys.argv[1])//1000,datetime.timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ'))" "$UNTIL_G1")
node_check "one gateway key per user" "/_debug/gw" "[.keys[] | select(.external_ref == \"user:$USER_ID\")] | length == 1 and .[0].budget == \"0\" and .[0].issued == 1 and (.[0].name | startswith(\"大肥鱼 \"))"
node_check "invite credit expires with the service" "/_debug/gw" "[.keys[] | select(.external_ref == \"user:$USER_ID\")][0].credits | length == 1 and .[0].external_ref == \"invite:$INV_G1:$USER_ID\" and .[0].amount_nano == 2500000000 and .[0].expires_at == \"$EXPIRY_G1\""
req GET /api/llm-keys
check "gateway key added to the user's keys" 200 '[.items[] | select(.name == "活动获取" and .has_balance == true and .rotatable == true and .model == "deepseek-flash" and (.base_url | endswith("/gw/v1")))] | length == 1'
no_secrets "key list hides the gateway key" "$BODY"
GW_KEY_ID=$(j '[.items[] | select(.name == "活动获取")][0].id')
GW_MASKED=$(j '[.items[] | select(.name == "活动获取")][0].key_masked')
req GET "/api/llm-keys/$GW_KEY_ID/balance"
check "gateway key reads its balance" 200 '.available == "2.5000" and (.credits | length) == 1'
req GET "/api/llm-keys/$KEY_ID/balance"
check "other keys have no balance" 400 '.error.code == "invalid_input"'
req GET /api/me/gateway
check "account view lists the credits" 200 ".account == true and .key.id == \"$GW_KEY_ID\" and .balance.available == \"2.5000\" and .balance.credits[0].expires_at == \"$EXPIRY_G1\" and .pending == []"
no_secrets "account view hides the key" "$BODY"
req POST /api/me/gateway/rotate '{}'
check "gateway key rotated" 200 ".key.id == \"$GW_KEY_ID\" and .key.rotatable == true and .key.key_masked != \"$GW_MASKED\""
no_secrets "rotate reply hides the key" "$BODY"
node_check "rotation keeps the same gateway key" "/_debug/gw" "[.keys[] | select(.external_ref == \"user:$USER_ID\")] | length == 1 and .[0].issued == 2"
req GET "/api/llm-keys/$GW_KEY_ID/balance"
check "rotated key reads its balance" 200 '.available == "2.5000"'
node_check "bot with a working key keeps it" "/_debug/desired/$BOT" '.llm.api_key == "sk-good-abcdef1234"'
COOKIE="$ADMIN_COOKIE"
req PATCH "/api/admin/users/$USER_ID" '{"extend_days":5}'
check "admin extends the service" 200 ".service_until > $UNTIL_G1"
node_check "service extension leaves the credit expiry" "/_debug/gw" "[.keys[] | select(.external_ref == \"user:$USER_ID\")][0].credits[0].expires_at == \"$EXPIRY_G1\""
req POST "/api/admin/users/$USER_ID/credits" '{"amount":"0","expires_at":null}'
check "admin credit amount validated" 400 '.error.code == "invalid_input"'
req POST "/api/admin/users/$USER_ID/credits" '{"amount":"3","expires_at":1000}'
check "admin credit expiry must be in the future" 400 '.error.code == "invalid_input"'
req POST "/api/admin/users/$USER_ID/credits" '{"amount":"3","expires_at":null,"note":"补偿"}'
check "admin adds a credit" 201 '(.items | length) == 2 and .items[0].source == "admin" and .items[0].amount == "3.00" and .items[0].state == "granted" and .items[0].remaining == "3.0000" and .items[0].created_by_email != null and .items[1].source == "invite"'
ADMIN_CREDIT=$(j '.items[0].id')
req PATCH "/api/admin/users/$USER_ID/credits/$ADMIN_CREDIT" "{\"amount\":\"4.5\",\"expires_at\":$((UNTIL_G1 + 86400000))}"
check "admin adjusts a credit" 200 "[.items[] | select(.id == \"$ADMIN_CREDIT\")][0] | .amount == \"4.50\" and .expires_at == $((UNTIL_G1 + 86400000)) and .remaining == \"4.5000\""
node_check "adjustment reaches the gateway" "/_debug/gw" "[.keys[] | select(.external_ref == \"user:$USER_ID\")][0].credits | map(select(.note == \"补偿\"))[0].amount_nano == 4500000000"
req PATCH "/api/admin/users/$USER_ID/credits/$ADMIN_CREDIT" '{}'
check "empty adjustment rejected" 400 '.error.code == "invalid_input"'
req GET /api/me
ADMIN_ID=$(j '.user.id')
req GET "/api/admin/users/$ADMIN_ID/credits"
check "user without credits lists none" 200 '.account == false and .items == []'
COOKIE="$USER_COOKIE"
req GET /api/me/gateway
check "balance lists credits soonest expiry first" 200 ".balance.available == \"7.0000\" and (.balance.credits | length) == 2 and .balance.credits[0].expires_at == \"$EXPIRY_G1\""
req GET "/api/admin/users/$USER_ID/credits"
check "users cannot read admin credits" 403 '.error.code == "forbidden"'
curl -s "$NODE/_debug/gw?fail=1" > /dev/null
req POST /api/me/redeem "{\"code\":\"$INV_G2\"}"
check "redeem succeeds while the gateway fails" 200 '.user.service_active == true'
req GET /api/me/gateway
check "failed credit shows as pending" 200 '(.pending | length) == 1 and .pending[0].amount == "2.50"'
COOKIE="$ADMIN_COOKIE"
req GET "/api/admin/invites?status=used"
check "redemptions show the gateway credit state" 200 "([.items[] | select(.code == \"$INV_G1\")][0].redemptions[0].gateway == \"granted\") and ([.items[] | select(.code == \"$INV_G2\")][0].redemptions[0] | .gateway == \"failed\" and (.gateway_error | contains(\"503\")))"
d1 "UPDATE gateway_credits SET created_at = created_at - 600000 WHERE remote_id IS NULL" > /dev/null
code=$(curl -s -o /dev/null -w '%{http_code}' "$BASE/__scheduled?cron=*/5+*+*+*+*")
sleep 1
req GET "/api/admin/invites?status=used"
check "maintenance resends the failed credit" 200 "[.items[] | select(.code == \"$INV_G2\")][0].redemptions[0].gateway == \"granted\""
new_captcha
COOKIE=""
req POST /api/auth/email-code "{\"email\":\"$EMAIL2\",\"purpose\":\"register\",\"captcha_id\":\"$CAPTCHA_ID\",\"captcha_answer\":\"$CAPTCHA_ANSWER\",\"invite_code\":\"$INV_A\"}"
check "registration with revoked invite gives invite_invalid" 400 '.error.code == "invite_invalid"'
new_captcha
req POST /api/auth/email-code "{\"email\":\"$EMAIL2\",\"purpose\":\"register\",\"captcha_id\":\"$CAPTCHA_ID\",\"captcha_answer\":\"$CAPTCHA_ANSWER\",\"invite_code\":\"$INV_D\"}"
check "configured invite sends registration code" 200 '.ok == true'
sleep 0.5
CODE2=$(grep -a -E "\[dev\] email code for $EMAIL2: [0-9]{6}" "$TMP/wrangler.log" | tail -1 | sed -E 's/.*: ([0-9]{6}).*/\1/')
req POST /api/auth/register "{\"email\":\"$EMAIL2\",\"password\":\"$PASSWORD\",\"code\":\"$CODE2\",\"invite_code\":\"$INV_D\",\"agree\":true}"
check "configured invite sets the new account quotas" 200 '.user.quota == {memory_mb:192,storage_mb:256,bots:0,plugins:2}'
grab_cookie
req POST /api/bots '{"name":"没有名额"}'
check "zero bot slots still prevents creation" 403 '.error.code == "quota_exceeded"'


echo "subscription"
COOKIE="$ADMIN_COOKIE"
req PATCH "/api/admin/users/$USER_ID" '{"service_until":1000}'
check "admin expires the user" 200 '.service_until == 1000 and .service_active == false'
node_check "expired user's bot stopped on the node" "/_debug/desired/$BOT" '.running == false'
COOKIE="$USER_COOKIE"
req GET /api/me
check "me shows inactive service" 200 '.user.service_active == false'
req POST "/api/bots/$BOT/start" '{}'
check "expired user gets service_expired on start" 403 '.error.code == "service_expired"'
req POST "/api/bots/$BOT/restart" '{}'
check "expired user gets service_expired on restart" 403 '.error.code == "service_expired"'
req POST /api/bots '{"name":"过期"}'
check "expired user gets service_expired on create" 403 '.error.code == "service_expired"'
req POST "/api/bots/$BOT/plugins" '{"repo_url":"https://github.com/fish-test/astrbot_plugin_demo"}'
check "expired user gets service_expired on plugin install" 403 '.error.code == "service_expired"'
req GET "/api/bots/$BOT"
check "stopped bot explains the expiry" 200 '.status == "stopped" and .status_detail == "服务已到期，兑换新的邀请码后可以重新启动"'
req PATCH "/api/bots/$BOT" '{"settings":{"persona":{"greeting":"到期也能改"}}}'
check "expired user can still edit" 200 '.settings.persona.greeting == "到期也能改"'
req POST /api/me/redeem "{\"code\":\"$INV_C\"}"
check "redeem after expiry starts from now" 200 '.user.service_active == true and (.user.service_until - (now * 1000) - 7 * 86400000 | fabs) < 120000'
req GET "/api/bots/$BOT"
check "redeem does not auto start bots" 200 '.status == "stopped"'
req POST "/api/bots/$BOT/start" '{}'
check "start after redeem" 200 '.status == "starting"'

COOKIE="$ADMIN_COOKIE"
req PATCH "/api/admin/users/$USER_ID" '{"extend_days":3}'
check "admin extend_days" 200 ".service_until > (now * 1000 + 9 * $DAY_MS)"
d1 "UPDATE users SET service_until = 5000 WHERE id = '$USER_ID'" > /dev/null
code=$(curl -s -o /dev/null -w '%{http_code}' "$BASE/__scheduled?cron=*/5+*+*+*+*")
node_check "cron stops bots of expired users" "/_debug/desired/$BOT" '.running == false'
COOKIE="$USER_COOKIE"
req GET "/api/bots/$BOT"
check "cron stop keeps data and notes the expiry" 200 '.status == "stopped" and .status_detail == "服务已到期，兑换新的邀请码后可以重新启动" and (.platforms | length) == 2 and (.plugins | length) == 1'
COOKIE="$ADMIN_COOKIE"
req PATCH "/api/admin/users/$USER_ID" '{"service_until":null,"extend_days":30}'
check "admin restores service" 200 '.service_active == true'
COOKIE="$USER_COOKIE"
req POST "/api/bots/$BOT/start" '{}'
check "user starts again" 200 '.status == "starting"'

echo "node outage"
stop_node
req PATCH "/api/bots/$BOT" '{"name":"离线改名"}'
check "mutation persists while node is down" 200 '.name == "离线改名" and .status == "error" and (.status_detail | test("节点"))'
req GET /api/usage
check "usage reports node_unreachable" 502 '.error.code == "node_unreachable"'
req GET /api/bots
check "D1 kept the change" 200 '.items[0].name == "离线改名" and .items[0].status == "error"'
COOKIE="$ADMIN_COOKIE"
req GET /api/admin/system
check "system view survives node outage" 200 '.nodes[0].online == false and .nodes[0].system == null and (.nodes[0].error | length) > 0 and (.console.d1.tables | length) == 16'
req GET /api/admin/overview
check "overview survives node outage" 200 '.nodes[0].online == false and (.nodes[0].error | length) > 0 and .nodes[0].version == null and .usage.memory_bytes == 0'
req GET /api/admin/users
check "unknown usage is null while node is down" 200 "[.items[] | select(.email == \"$EMAIL\")][0] | .memory_bytes == null and .storage_bytes == null"
check "user without bots has zero usage" 200 "[.items[] | select(.email == \"$ADMIN_EMAIL\")][0] | .memory_bytes == 0"
COOKIE="$USER_COOKIE"
start_node
req GET "/api/bots/$BOT"
check "recovered node gets re-push" 200 '.status == "running" and .name == "离线改名"'
node_check "re-pushed DesiredBot complete" "/_debug/desired/$BOT" '.name == "离线改名" and (.plugins | length) == 1 and (.platforms | length) == 2'

echo "scheduled maintenance"
code=$(curl -s -o "$TMP/body" -w '%{http_code}' "$BASE/__scheduled?cron=*/5+*+*+*+*")
if [ "$code" = 200 ] && ! grep -a -q "maintenance failed" "$TMP/wrangler.log"; then ok "cron handler runs"; else bad "cron handler runs" "status $code"; fi

echo "cleanup"
req DELETE "/api/bots/$BOT/plugins/$PLUG"
check "plugin deleted" 200 '.ok == true'
node_check "plugin removal pushed" "/_debug/desired/$BOT" '(.plugins | length) == 0'
req DELETE "/api/llm-keys/$KEY_ID"
check "key deleted" 200 '.ok == true'
node_check "key removal pushed" "/_debug/desired/$BOT" '.llm == null'
req DELETE "/api/bots/$BOT"
check "bot deleted" 200 '.ok == true'
node_check "node removed bot" "/_debug/stats" "(.deleted | index(\"$BOT\")) != null and (.bots | index(\"$BOT\")) == null"
req GET "/api/bots/$BOT"
check "deleted bot is gone" 404 '.error.code == "not_found"'
node_check "node requests after restart all carried valid signatures" "/_debug/stats" '.rejected == 0 and .verified > 3'

echo
echo "passed $PASS, failed $FAIL"
[ "$FAIL" = 0 ]
