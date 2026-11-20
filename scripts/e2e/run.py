"""Local full-stack assertions and deterministic HTTP fixtures."""

import base64
import hashlib
import hmac
import io
import json
import os
from pathlib import Path
import re
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
import traceback
import urllib.error
from urllib.parse import parse_qs, urlsplit
import urllib.request
import zipfile
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = Path(__file__).resolve().parents[2]
PERSONA = "You are the local test fish. Reply with the deterministic test response."
TEMPERATURE = 0.5
PROMPT = "Hello from the full stack test"
REPLY = "local fish reply"
TOOL_PROMPT = "Please use the echo tool"
API_KEY = "sk-local-e2e-1234"
MODEL = "fish-e2e"
REPO = "https://github.com/fish-e2e/cmd_plugin"
METADATA = b"name: cmd_plugin\nauthor: fish\ndesc: Local command fixture\nversion: 1.2.3\n"
CONFIG_SCHEMA = {"echo_prefix": {"type": "string", "description": "Echo prefix", "default": "echo"}}
QQ_APP_ID = "102384756"
QQ_SECRET = "naOC0ocQE3shWLAfffVLB1rhYPG7"
QQ_PANEL_TOKEN = "local-qq-panel-token"
# Ed25519 signature of "1725442341Arq0D5A61EgUu4OxUvOp" under the key derived from QQ_SECRET.
QQ_VALIDATION_SIG = ("3be3ed93973d9d382e3c4f0a65b57cabc9e48e3f7fcbca6c238e349f51bb5263"
                     "cb24e3723053f44af5540143a04450bdbdd4824453d483c58ad82e4fb039350a")
HTTP = urllib.request.build_opener(urllib.request.ProxyHandler({}))


class Fixture(ThreadingHTTPServer):
    def __init__(self, record):
        super().__init__(("127.0.0.1", 0), Handler)
        self.record = record
        self.requests = []
        self.archive_downloads = 0
        self.panels = {}
        self.lock = threading.Lock()
        archive = io.BytesIO()
        with zipfile.ZipFile(archive, "w") as z:
            z.writestr("cmd_plugin/main.py", (ROOT / "sidecar/tests/plugins/cmd_plugin/main.py").read_bytes())
            z.writestr("cmd_plugin/metadata.yaml", METADATA)
            z.writestr("cmd_plugin/_conf_schema.json", json.dumps(CONFIG_SCHEMA))
        self.archive = archive.getvalue()


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def reply(self, status, value, content_type="application/json"):
        data = value if isinstance(value, bytes) else json.dumps(value).encode()
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        if self.path == "/astrbot-market":
            return self.reply(200, {
                "$meta": {"schema_version": 1},
                "fish/cmd_plugin": {
                    "name": "cmd_plugin", "author": "fish", "repo": REPO,
                    "display_name": "本地命令插件", "desc": "Local command fixture",
                    "version": "1.2.3", "category": "工具",
                },
            })
        if self.path.startswith("/v2/panels"):
            return self.qq_panels("GET")
        if self.path == "/v1/models":
            if self.headers.get("Authorization") != f"Bearer {API_KEY}":
                return self.reply(401, {"error": {"message": "invalid local credential"}})
            return self.reply(200, {"object": "list", "data": [{"id": MODEL, "object": "model"}]})
        if self.path == "/gh-api/repos/fish-e2e/cmd_plugin":
            return self.reply(200, {"private": False, "default_branch": "main"})
        if self.path in ("/gh-raw/fish-e2e/cmd_plugin/main/metadata.yaml", "/gh-raw/fish-e2e/cmd_plugin/HEAD/metadata.yaml"):
            return self.reply(200, METADATA, "text/plain")
        if self.path in ("/gh-raw/fish-e2e/cmd_plugin/main/_conf_schema.json", "/gh-raw/fish-e2e/cmd_plugin/HEAD/_conf_schema.json"):
            return self.reply(200, CONFIG_SCHEMA)
        if self.path == "/codeload/fish-e2e/cmd_plugin/zip/HEAD":
            with self.server.lock:
                self.server.archive_downloads += 1
            return self.reply(200, self.server.archive, "application/zip")
        self.reply(404, {"error": "local fixture route not found"})

    def do_POST(self):
        if self.path == "/qq-token":
            credentials = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            if credentials != {"appId": QQ_APP_ID, "clientSecret": QQ_SECRET}:
                return self.reply(401, {"message": "invalid QQ fixture credentials"})
            return self.reply(200, {"access_token": QQ_PANEL_TOKEN, "expires_in": 7200})
        if self.path == "/v2/panels":
            return self.qq_panels("POST")
        if self.path != "/v1/chat/completions":
            return self.reply(404, {"error": "local fixture route not found"})
        if self.headers.get("Authorization") != f"Bearer {API_KEY}":
            return self.reply(401, {"error": {"message": "invalid local credential"}})
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        with self.server.lock:
            self.server.requests.append(body)
            with self.server.record.open("a") as f:
                f.write(json.dumps(body, ensure_ascii=False) + "\n")
        last = body["messages"][-1]
        message = {"role": "assistant", "content": REPLY}
        if last.get("content") == TOOL_PROMPT and body.get("tools"):
            message = {"role": "assistant", "content": None, "tool_calls": [{
                "id": "call-local", "type": "function",
                "function": {"name": "echo_tool", "arguments": json.dumps({"text": "from-llm"})},
            }]}
        elif last.get("role") == "tool":
            message = {"role": "assistant", "content": f"tool said {last['content']}"}
        self.reply(200, {
            "id": "chatcmpl-local", "object": "chat.completion", "model": MODEL,
            "choices": [{"index": 0, "message": message, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 12, "completion_tokens": 3, "total_tokens": 15},
        })

    def do_PUT(self):
        self.qq_panels("PUT")

    def do_DELETE(self):
        self.qq_panels("DELETE")

    def qq_panels(self, method):
        if self.headers.get("Authorization") != f"QQBot {QQ_PANEL_TOKEN}" or self.headers.get("X-Union-Appid") != QQ_APP_ID:
            return self.reply(401, {"message": "invalid panel token or AppID"})
        url = urlsplit(self.path)
        panel_id = url.path.rsplit("/", 1)[-1]
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"]))) if method in ("POST", "PUT") else None
        with self.server.lock:
            panels = self.server.panels
            record = panels.get(panel_id)
            if method == "GET" and url.path == "/v2/panels":
                scope = parse_qs(url.query).get("scope", [None])[0]
                records = [p for p in panels.values() if p["scope"] == scope]
                if not records:
                    return self.reply(200, {"is_end": True})
                return self.reply(200, {"records": records, "next_cursor": "", "is_end": True})
            if method == "POST" and body["target_type"] == "all":
                panel_id = "p_fixture_1"
                panels[panel_id] = {"panel_id": panel_id, **body, "version": 1}
                return self.reply(200, {"panel_id": panel_id})
            if record is None:
                return self.reply(404, {"message": "unknown fixture panel"})
            if method == "GET":
                return self.reply(200, record)
            if method == "PUT" and set(body) == {"panel"}:
                record["panel"] = body["panel"]
                record["version"] += 1
                return self.reply(200, {"version": record["version"]})
            if method == "DELETE":
                del panels[panel_id]
                return self.reply(200, {})
            return self.reply(400, {"message": "unexpected QQ panel request"})


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


class Run:
    def __init__(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="fish-e2e.", dir=os.environ["TMPDIR"]))
        self.processes = []
        self.passed = 0
        self.cookie = ""
        self.secret = os.urandom(32)
        self.fixture = None
        self.env = dict(os.environ, CI="1", WRANGLER_SEND_METRICS="false", UV_OFFLINE="1",
                        UV_PYTHON_DOWNLOADS="never", OBJC_DISABLE_INITIALIZE_FORK_SAFETY="YES",
                        PYTHONDONTWRITEBYTECODE="1", NO_PROXY="127.0.0.1,localhost")
        for name in ("HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy"):
            self.env.pop(name, None)

    def check(self, name, condition, evidence=""):
        if not condition:
            raise AssertionError(f"{name}: {evidence}")
        self.passed += 1
        print(f"  ok   {name}", flush=True)

    def start(self, name, args, env=None):
        with (self.tmp / f"{name}.log").open("ab") as log:
            process = subprocess.Popen(args, cwd=self.tmp, env=env or self.env,
                                       stdout=log, stderr=log, start_new_session=True)
        self.processes.append(process)
        return process

    def stop(self, process):
        for sig in (signal.SIGTERM, signal.SIGKILL):
            try:
                os.killpg(process.pid, sig)
            except ProcessLookupError:
                break
            try:
                process.wait(timeout=20 if sig == signal.SIGTERM else 5)
            except subprocess.TimeoutExpired:
                continue
            # Descendants can outlive the process that launched them.
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            except PermissionError:
                # A process group can disappear between wait() and killpg().
                groups = subprocess.check_output(["ps", "-axo", "pgid="], text=True)
                if str(process.pid) in groups.split():
                    raise
            break

    def request(self, base, method, path, data=None, signed=False):
        body = json.dumps(data, ensure_ascii=False).encode() if data is not None else b""
        headers = {"Content-Type": "application/json", "X-Fish-Csrf": "1"}
        if signed:
            ts = str(time.time_ns() // 1_000_000)
            canonical = f"{ts}\n{method}\n{path}\n{hashlib.sha256(body).hexdigest()}"
            headers.update({"X-Fish-Node": "node-1", "X-Fish-Ts": ts,
                            "X-Fish-Sig": hmac.new(self.secret, canonical.encode(), hashlib.sha256).hexdigest()})
        elif self.cookie:
            headers["Cookie"] = self.cookie
        req = urllib.request.Request(base + path, data=body if data is not None else None, method=method, headers=headers)
        try:
            response = HTTP.open(req, timeout=10)
        except urllib.error.HTTPError as e:
            response = e
        with response:
            raw = response.read().decode()
            value = json.loads(raw) if raw else None
            cookie = response.headers.get("Set-Cookie", "")
            if cookie.startswith("fish_session="):
                self.cookie = cookie.split(";", 1)[0]
            return response.status, value

    def callback(self, base, path, raw, headers):
        req = urllib.request.Request(base + path, data=raw.encode(), method="POST",
                                     headers={"Content-Type": "application/json", "User-Agent": "QQBot-Callback", **headers})
        try:
            response = HTTP.open(req, timeout=10)
        except urllib.error.HTTPError as e:
            response = e
        with response:
            text = response.read().decode()
            return response.status, json.loads(text) if text else None

    def expect(self, name, base, method, path, data=None, signed=False, status=200):
        code, body = self.request(base, method, path, data, signed)
        self.check(name, code == status, f"HTTP {code}: {body}")
        return body

    def until(self, name, predicate, seconds=60):
        deadline = time.monotonic() + seconds
        last = None
        while time.monotonic() < deadline:
            exited = [p.returncode for p in self.processes if p.poll() is not None]
            if exited:
                raise AssertionError(f"{name}: child process exited: {exited}")
            try:
                last = predicate()
                if last:
                    return last
            except (OSError, ValueError) as e:
                last = str(e)
            time.sleep(0.2)
        raise AssertionError(f"{name}: timed out: {last}")

    def log_value(self, pattern):
        def read():
            found = re.findall(pattern, (self.tmp / "wrangler.log").read_text(errors="replace"))
            return found[-1] if found else None
        return self.until("development log value", read)

    def run(self):
        python = ROOT / "sidecar/.venv/bin/python"
        if not python.is_file():
            raise RuntimeError("sidecar/.venv/bin/python is required. Run uv sync in sidecar/ first")
        subprocess.run([str(python), "-c", "import aiohttp, httpx, pydantic, yaml"], check=True, env=self.env)
        self.fixture = Fixture(self.tmp / "llm-requests.jsonl")
        threading.Thread(target=self.fixture.serve_forever, daemon=True).start()
        external = f"http://127.0.0.1:{self.fixture.server_port}"
        node_port, console_port = free_port(), free_port()
        while console_port == node_port:
            console_port = free_port()
        node, console = f"http://127.0.0.1:{node_port}", f"http://127.0.0.1:{console_port}"
        secret = base64.b64encode(self.secret).decode()
        node_env = dict(self.env, FISH_NODE_ID="node-1", FISH_NODE_SECRET=secret,
                        FISH_CONSOLE_URL=console, FISH_LISTEN=f"127.0.0.1:{node_port}",
                        FISH_DATA_DIR=str(self.tmp / "node"), FISH_SIDECAR_DIR=str(ROOT / "sidecar"),
                        FISH_SIDECAR_PYTHON=str(python), FISH_UV=shutil.which("uv"),
                        FISH_CODELOAD_BASE=external + "/codeload")
        node_env.pop("FISH_DEV", None)
        binary = str(ROOT / "node/target/debug/fish-node")
        production = self.start("node-without-fish-dev", [binary, "--dev"], node_env)
        self.until("node without FISH_DEV", lambda: self.request(node, "GET", "/v1/health", signed=True)[0] == 200)
        for method, suffix in (("POST", "inject"), ("GET", "outbox")):
            self.expect(f"{suffix} rejected without FISH_DEV even with --dev", node, method,
                        f"/v1/dev/bots/missing/{suffix}", signed=True, status=404)
        self.stop(production)
        self.processes.remove(production)
        node_env["FISH_DEV"] = "1"
        node_env.update(FISH_QQ_PANEL_API_BASE=external, FISH_QQ_PANEL_TOKEN_URL=external + "/qq-token")
        self.start("node", [binary], node_env)
        self.until("real node ready", lambda: self.request(node, "GET", "/v1/health", signed=True)[0] == 200)
        self.check("real node accepts decoded-secret HMAC", True)
        self.expect("loopback requires a signature", node, "POST", "/v1/dev/bots/missing/inject", status=401)

        assets = self.tmp / "assets"
        assets.mkdir()
        (assets / "index.html").write_text("<!doctype html><title>fish local test</title>")
        config = self.tmp / "wrangler.toml"
        config.write_text(f'''name = "fish-console-e2e"
main = "{ROOT}/console/build/index.js"
compatibility_date = "2026-01-01"
assets = {{ directory = "{assets}", binding = "ASSETS", not_found_handling = "single-page-application", run_worker_first = ["/api/*"] }}
[[d1_databases]]
binding = "DB"
database_name = "fish"
database_id = "00000000-0000-0000-0000-000000000000"
migrations_dir = "{ROOT}/console/migrations"
''')
        (self.tmp / "dev.env").write_text(f'''FISH_DEV=1
EMAIL_PROVIDER=log
EMAIL_FROM=noreply@niox.lol
DEFAULT_NODE_ID=node-1
DEFAULT_NODE_URL={node}
DATA_KEY={base64.b64encode(os.urandom(32)).decode()}
NODE_SECRET={secret}
GITHUB_API_BASE={external}/gh-api
GITHUB_RAW_BASE={external}/gh-raw
GITHUB_GIT_BASE={external}/gh-git
ASTRBOT_MARKET_URL={external}/astrbot-market
GITHUB_TOKEN=
ADMIN_EMAILS=admin@example.com
''')
        state_args = ["--config", str(config), "--persist-to", str(self.tmp / "state")]
        with (self.tmp / "migrate.log").open("w") as log:
            subprocess.run(["wrangler", "d1", "migrations", "apply", "DB", "--local", *state_args],
                           cwd=self.tmp, env=self.env, stdout=log, stderr=log, check=True, timeout=60)
        self.start("wrangler", ["wrangler", "dev", "--local", *state_args, "--ip", "127.0.0.1", "--port", str(console_port),
                               "--env-file", str(self.tmp / "dev.env"), "--show-interactive-dev-session=false"])
        self.until("console ready", lambda: self.request(console, "GET", "/api/me")[0] == 401)
        with (self.tmp / "invite.log").open("w") as log:
            invite = subprocess.check_output([str(ROOT / "console/scripts/make-invite.sh"), "30", *state_args],
                                             cwd=self.tmp, env=self.env, stderr=log, text=True, timeout=60).strip().splitlines()[-1]
        self.check("local D1 invite created", re.fullmatch(r"FISH-[0-9A-HJKMNP-TV-Z]{4}-[0-9A-HJKMNP-TV-Z]{4}", invite))
        captcha = self.expect("captcha issued", console, "GET", "/api/captcha")
        answer = self.log_value(r"\[dev\] captcha " + re.escape(captcha["id"]) + r" answer ([A-Z0-9]+)")
        email = "e2e@example.com"
        self.expect("registration email code sent", console, "POST", "/api/auth/email-code", {
            "email": email, "purpose": "register", "captcha_id": captcha["id"],
            "captcha_answer": answer, "invite_code": invite,
        })
        code = self.log_value(r"\[dev\] email code for " + re.escape(email) + r": ([0-9]{6})")
        user = self.expect("user registered through console", console, "POST", "/api/auth/register", {
            "email": email, "password": "local-fish-123", "code": code, "invite_code": invite, "agree": True,
        })
        self.check("registered session and service are active", bool(self.cookie) and user["user"]["service_active"])
        self.expect("first LLM credential saved", console, "POST", "/api/llm-keys", {
            "name": "Earlier key", "base_url": external + "/v1", "model": "earlier-default", "api_key": API_KEY,
        })
        key = self.expect("selected LLM credential validated by local models endpoint", console, "POST", "/api/llm-keys", {
            "name": "Local fixture", "base_url": external + "/v1", "model": "saved-default", "api_key": API_KEY,
        })
        self.expect("bot creation requires a key", console, "POST", "/api/bots", {"name": "Local test fish"}, status=400)
        self.expect("bot creation requires a model", console, "POST", "/api/bots", {"name": "Local test fish", "key_id": key["id"]}, status=400)
        self.expect("bot creation rejects an unavailable key", console, "POST", "/api/bots", {
            "name": "Local test fish", "key_id": "missing", "model": MODEL,
        }, status=404)
        created = self.expect("bot created with chosen key and model", console, "POST", "/api/bots", {
            "name": "Local test fish", "key_id": key["id"], "model": MODEL,
        })
        self.check("the chosen key and model were saved", created["settings"]["llm"]["key_id"] == key["id"]
                   and created["settings"]["llm"]["model"] == MODEL)
        bot = created["id"]
        path = f"/api/bots/{bot}"
        self.expect("persona and temperature set through console", console, "PATCH", path, {"settings": {
            "persona": {"system_prompt": PERSONA}, "llm": {"temperature": TEMPERATURE},
        }})
        status = self.expect("real bot status proxied through console", console, "GET", path)
        self.check("real bot is running", status["status"] == "running", status)
        market = self.expect("console reads the AstrBot market catalogue", console, "GET", "/api/plugins/market")
        item, = market["items"]
        self.check("catalogue provides the plugin identity and repository", item["id"] == "fish/cmd_plugin"
                   and item["repo_url"] == REPO and item["display_name"] == "本地命令插件", item)
        plugin = self.expect("market plugin installed through console", console, "POST", path + "/plugins", {"plugin_id": item["id"]})
        self.check("console inspected fixture metadata", plugin["name"] == "cmd_plugin" and plugin["version"] == "1.2.3", plugin)
        self.until("real sidecar plugin running", lambda: any(p["id"] == plugin["id"] and p["status"] == "running"
                   for p in self.request(console, "GET", path)[1]["plugins"]))
        self.check("real sidecar loaded installed command plugin", True)
        inject = f"/v1/dev/bots/{bot}/inject"
        outbox = f"/v1/dev/bots/{bot}/outbox"
        self.expect("chat injected into real pipeline", node, "POST", inject,
                    {"text": PROMPT, "session_id": "local-user", "message_id": "chat-1"}, signed=True)
        def reply(message_id, text):
            items = self.request(node, "GET", outbox, signed=True)[1]["items"]
            return any(i["reply_to"] == message_id and i["session_id"] == "local-user" and
                       i["chain"] == [{"type": "plain", "text": text}] for i in items)
        self.until("LLM reply returned to platform", lambda: reply("chat-1", REPLY))
        self.check("deterministic LLM reply returned through platform outbox", True)
        with self.fixture.lock:
            requests = list(self.fixture.requests)
        self.check("LLM received persona, temperature, user prompt and selected model", len(requests) == 1 and
                   requests[0]["model"] == MODEL and requests[0]["temperature"] == TEMPERATURE and requests[0]["messages"] == [
                       {"role": "system", "content": PERSONA}, {"role": "user", "content": PROMPT}], requests)
        self.expect("plugin command injected", node, "POST", inject,
                    {"text": "/echo local-command", "session_id": "local-user", "message_id": "command-1"}, signed=True)
        self.until("plugin reply returned to platform", lambda: reply("command-1", "echo:local-command"))
        self.check("real plugin handled command and returned reply", True)
        plugin_path = path + f"/plugins/{plugin['id']}"
        edited = self.expect("plugin configuration saved through console", console, "PATCH", plugin_path, {
            "config": {"echo_prefix": "updated"}, "config_revision": plugin["config_revision"],
        })
        self.check("plugin configuration and revision changed", edited["config"] == {"echo_prefix": "updated"}
                   and edited["config_revision"] != plugin["config_revision"])
        code, result = self.request(console, "PATCH", plugin_path, {
            "config": {"echo_prefix": "stale"}, "config_revision": plugin["config_revision"],
        })
        self.check("stale edit cannot overwrite plugin configuration", code == 409 and
                   result["error"]["code"] == "plugin_config_changed", result)
        self.until("plugin process started again after saving settings", lambda: sum(
            line["source"] == "plugin:cmd_plugin" and line["message"].startswith("插件已启动 pid=")
            for line in self.request(console, "GET", path + "/logs")[1]["items"]) >= 2)
        self.expect("command injected after saving settings", node, "POST", inject,
                    {"text": "/echo after-save", "session_id": "local-user", "message_id": "command-config"}, signed=True)
        self.until("plugin replied with settings read in its constructor", lambda: reply("command-config", "updated:after-save"))
        code, _ = self.request(console, "POST", path + "/plugins/missing/reload")
        self.check("unknown plugin cannot be reloaded", code == 404)
        self.expect("reload installed plugin through console", console, "POST", plugin_path + "/reload")
        self.until("manual reload started the installed plugin again", lambda: sum(
            line["source"] == "plugin:cmd_plugin" and line["message"].startswith("插件已启动 pid=")
            for line in self.request(console, "GET", path + "/logs")[1]["items"]) >= 3)
        with self.fixture.lock:
            self.check("manual reload reused installed code", self.fixture.archive_downloads == 1)
        self.expect("follow-up chat injected", node, "POST", inject,
                    {"text": "Follow-up", "session_id": "local-user", "message_id": "chat-2"}, signed=True)
        self.until("follow-up reply", lambda: reply("chat-2", REPLY))
        with self.fixture.lock:
            requests = list(self.fixture.requests)
        self.check("command bypassed LLM and conversation history persisted", len(requests) == 2 and requests[1]["messages"] == [
            {"role": "system", "content": PERSONA}, {"role": "user", "content": PROMPT},
            {"role": "assistant", "content": REPLY}, {"role": "user", "content": "Follow-up"}], requests)
        self.expect("tool chat injected", node, "POST", inject,
                    {"text": TOOL_PROMPT, "session_id": "local-user", "message_id": "chat-tool"}, signed=True)
        self.until("LLM answered with the plugin tool result", lambda: reply("chat-tool", "tool said updated:from-llm"))
        with self.fixture.lock:
            requests = list(self.fixture.requests)
        offered = requests[-2].get("tools") or []
        self.check("LLM was offered the plugin tool with its docstring schema", any(
            t["function"]["name"] == "echo_tool"
            and t["function"]["parameters"].get("properties", {}).get("text", {}).get("type") == "string"
            for t in offered), offered)
        self.check("tool call and result were sent back to the LLM", requests[-1]["messages"][-2:] == [
            {"role": "assistant", "content": None, "tool_calls": [{"id": "call-local", "type": "function",
             "function": {"name": "echo_tool", "arguments": json.dumps({"text": "from-llm"})}}]},
            {"role": "tool", "tool_call_id": "call-local", "content": "updated:from-llm"}], requests[-1]["messages"])
        self.expect("plugin agent command injected", node, "POST", inject,
                    {"text": "/ask_tool", "session_id": "local-user", "message_id": "plugin-tool"}, signed=True)
        self.until("plugin agent answered after calling its own tool", lambda: reply("plugin-tool", "tool said updated:from-llm"))
        with self.fixture.lock:
            first, followup = self.fixture.requests[-2:]
        self.check("plugin agent sent the tool definition and continued from its result",
                   first["messages"][-1] == {"role": "user", "content": TOOL_PROMPT}
                   and [tool["function"]["name"] for tool in first.get("tools", [])] == ["echo_tool"]
                   and followup["messages"][-1] == {"role": "tool", "tool_call_id": "call-local",
                                                    "content": "updated:from-llm"})
        usage = self.expect("console usage endpoint responds", console, "GET", "/api/usage")
        self.check("console reports nonzero bot and plugin memory", usage["memory_bytes"] > 0 and
                   any(s["bot_id"] == bot and s["kind"] == "core" and s["memory_bytes"] > 0 for s in usage["sources"]) and
                   any(s["bot_id"] == bot and s["plugin_id"] == plugin["id"] and s["memory_bytes"] > 0 for s in usage["sources"]), usage)
        self.expect("real node system report", node, "GET", "/v1/system", signed=True)

        def system_sees_plugin():
            system = self.request(node, "GET", "/v1/system", signed=True)[1]
            return (system["host"]["cpu_count"] >= 1 and
                    system["host"]["memory_total_bytes"] > 0 and system["disk"]["total_bytes"] > 0 and
                    any(p["role"] == "node" and p["rss_bytes"] > 0 for p in system["processes"]) and
                    any(p["role"] == "plugin" and p["plugin"]["plugin_id"] == plugin["id"] and p["plugin"]["bot_id"] == bot
                        for p in system["processes"]) and
                    system["bots"]["total"] == 1 and system["plugins"].get("running") == 1 and len(system["history"]) >= 1)
        self.until("node system report sees the node and the plugin process", system_sees_plugin, seconds=45)
        self.expect("system report requires a signature", node, "GET", "/v1/system", status=401)
        self.expect("non-admin cannot read the admin system view", console, "GET", "/api/admin/system", status=403)
        qq = self.expect("QQ platform bound in Webhook mode", console, "POST", path + "/platforms/qq/manual",
                         {"app_id": QQ_APP_ID, "secret": QQ_SECRET, "mode": "webhook"})
        self.check("platform view reports Webhook mode", qq["mode"] == "webhook", qq)
        panels = path + f"/platforms/{qq['id']}/qq-panels"
        panel = {"remark": "Local menu", "items": [{"type": "command", "name": "/echo", "desc": "Local command"}]}
        empty = self.expect("QQ panel list read via the node", console, "GET", panels + "?scope=group")
        self.check("QQ group list starts empty", empty["records"] == [])
        empty = self.expect("QQ direct-message panel list read via the node", console, "GET", panels + "?scope=c2c")
        self.check("QQ direct-message list starts empty", empty["records"] == [])
        created = self.expect("QQ group panel published", console, "POST", panels, {"scope": "group", "panel": panel})
        panel_path = panels + "/" + created["panel_id"]
        record = self.expect("QQ group panel read back", console, "GET", panels + "?scope=group")["records"][0]
        self.check("QQ panel saved the command and global scope", record["target_type"] == "all"
                   and record["panel"]["items"][0]["name"] == "/echo")
        self.expect("QQ panel changed with the current version", console, "PUT", panel_path,
                    {"panel": {**panel, "remark": "Updated"}, "expected_version": record["version"]})
        self.expect("stale QQ panel edit rejected", console, "PUT", panel_path,
                    {"panel": panel, "expected_version": record["version"]}, status=409)
        self.expect("QQ panel removed", console, "DELETE", panel_path)
        hook = f"/api/qq/webhook/{qq['id']}"
        validate = json.dumps({"d": {"plain_token": "Arq0D5A61EgUu4OxUvOp", "event_ts": "1725442341"}, "op": 13})
        code, answer = self.callback(console, hook, validate, {"X-Bot-Appid": QQ_APP_ID})
        self.check("callback validation answered by real node through console", code == 200 and
                   answer == {"plain_token": "Arq0D5A61EgUu4OxUvOp", "signature": QQ_VALIDATION_SIG}, f"HTTP {code}: {answer}")
        self.until("Webhook platform connected after validation", lambda: any(
            x["id"] == qq["id"] and x["status"] == "connected" for x in self.request(console, "GET", path)[1]["platforms"]))
        event = json.dumps({"id": "ev-1", "op": 0, "t": "C2C_MESSAGE_CREATE",
                            "d": {"id": "m-1", "content": "hi", "author": {"user_openid": "u-1"}}})
        code, _ = self.callback(console, hook, event, {"X-Bot-Appid": QQ_APP_ID})
        self.check("unsigned event push rejected", code == 401, code)
        code, _ = self.callback(console, hook, event, {"X-Bot-Appid": QQ_APP_ID, "X-Signature-Timestamp": "1725442341",
                                                       "X-Signature-Ed25519": "00" * 64})
        self.check("forged event signature rejected by node", code == 401, code)
        code, _ = self.callback(console, "/api/qq/webhook/unknown", validate, {"X-Bot-Appid": QQ_APP_ID})
        self.check("unknown callback address is 404", code == 404, code)
        self.expect("Webhook platform removed", console, "DELETE", path + f"/platforms/{qq['id']}")
        self.expect("bot stopped through console", console, "POST", path + "/stop", {})
        self.expect("stopped bot rejects injection", node, "POST", inject,
                    {"text": "stopped", "session_id": "local-user", "message_id": "stopped"}, signed=True, status=400)
        self.expect("bot deleted through console", console, "DELETE", path)

    def close(self, failed):
        for process in reversed(self.processes):
            self.stop(process)
        if self.fixture:
            self.fixture.shutdown()
            self.fixture.server_close()
        if failed:
            for path in sorted(self.tmp.glob("*.log")):
                print(f"\n{path.name} (last 30 lines)")
                print("\n".join(path.read_text(errors="replace").splitlines()[-30:]))
        if "--keep" in sys.argv:
            print(f"logs and state kept in {self.tmp}")
        else:
            shutil.rmtree(self.tmp)


def interrupted(*_):
    raise KeyboardInterrupt("interrupted")


if __name__ == "__main__":
    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)
    run = Run()
    failed = 0
    try:
        run.run()
    except (Exception, KeyboardInterrupt) as e:
        failed = 1
        print(f"  FAIL {e}", flush=True)
        traceback.print_exc()
    finally:
        run.close(failed)
        print(f"passed {run.passed}, failed {failed}", flush=True)
    sys.exit(failed)
