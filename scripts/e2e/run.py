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
import urllib.request
import zipfile
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = Path(__file__).resolve().parents[2]
PERSONA = "You are the local test fish. Reply with the deterministic test response."
PROMPT = "Hello from the full stack test"
REPLY = "local fish reply"
API_KEY = "sk-local-e2e-1234"
MODEL = "fish-e2e"
REPO = "https://github.com/fish-e2e/cmd_plugin"
METADATA = b"name: cmd_plugin\nauthor: fish\ndesc: Local command fixture\nversion: 1.2.3\n"
HTTP = urllib.request.build_opener(urllib.request.ProxyHandler({}))


class Fixture(ThreadingHTTPServer):
    def __init__(self, record):
        super().__init__(("127.0.0.1", 0), Handler)
        self.record = record
        self.requests = []
        self.lock = threading.Lock()
        archive = io.BytesIO()
        with zipfile.ZipFile(archive, "w") as z:
            z.writestr("cmd_plugin/main.py", (ROOT / "sidecar/tests/plugins/cmd_plugin/main.py").read_bytes())
            z.writestr("cmd_plugin/metadata.yaml", METADATA)
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
        if self.path == "/v1/models":
            if self.headers.get("Authorization") != f"Bearer {API_KEY}":
                return self.reply(401, {"error": {"message": "invalid local credential"}})
            return self.reply(200, {"object": "list", "data": [{"id": MODEL, "object": "model"}]})
        if self.path == "/gh-api/repos/fish-e2e/cmd_plugin":
            return self.reply(200, {"private": False, "default_branch": "main"})
        if self.path == "/gh-raw/fish-e2e/cmd_plugin/main/metadata.yaml":
            return self.reply(200, METADATA, "text/plain")
        if self.path == "/codeload/fish-e2e/cmd_plugin/zip/HEAD":
            return self.reply(200, self.server.archive, "application/zip")
        self.reply(404, {"error": "local fixture route not found"})

    def do_POST(self):
        if self.path != "/v1/chat/completions":
            return self.reply(404, {"error": "local fixture route not found"})
        if self.headers.get("Authorization") != f"Bearer {API_KEY}":
            return self.reply(401, {"error": {"message": "invalid local credential"}})
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        with self.server.lock:
            self.server.requests.append(body)
            with self.server.record.open("a") as f:
                f.write(json.dumps(body, ensure_ascii=False) + "\n")
        self.reply(200, {
            "id": "chatcmpl-local", "object": "chat.completion", "model": MODEL,
            "choices": [{"index": 0, "message": {"role": "assistant", "content": REPLY}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 12, "completion_tokens": 3, "total_tokens": 15},
        })


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
        self.expect("LLM credential validated by local models endpoint", console, "POST", "/api/llm-keys", {
            "name": "Local fixture", "base_url": external + "/v1", "model": MODEL, "api_key": API_KEY,
        })
        bot = self.expect("bot created through console", console, "POST", "/api/bots", {"name": "Local test fish"})["id"]
        path = f"/api/bots/{bot}"
        self.expect("persona set through console", console, "PATCH", path, {"settings": {"persona": {"system_prompt": PERSONA}}})
        status = self.expect("real bot status proxied through console", console, "GET", path)
        self.check("real bot is running", status["status"] == "running", status)
        plugin = self.expect("local command plugin installed through console", console, "POST", path + "/plugins", {"repo_url": REPO})
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
        self.check("LLM received persona, user prompt and selected model", len(requests) == 1 and
                   requests[0]["model"] == MODEL and requests[0]["messages"] == [
                       {"role": "system", "content": PERSONA}, {"role": "user", "content": PROMPT}], requests)
        self.expect("plugin command injected", node, "POST", inject,
                    {"text": "/echo local-command", "session_id": "local-user", "message_id": "command-1"}, signed=True)
        self.until("plugin reply returned to platform", lambda: reply("command-1", "echo:local-command"))
        self.check("real plugin handled command and returned reply", True)
        self.expect("follow-up chat injected", node, "POST", inject,
                    {"text": "Follow-up", "session_id": "local-user", "message_id": "chat-2"}, signed=True)
        self.until("follow-up reply", lambda: reply("chat-2", REPLY))
        with self.fixture.lock:
            requests = list(self.fixture.requests)
        self.check("command bypassed LLM and conversation history persisted", len(requests) == 2 and requests[1]["messages"] == [
            {"role": "system", "content": PERSONA}, {"role": "user", "content": PROMPT},
            {"role": "assistant", "content": REPLY}, {"role": "user", "content": "Follow-up"}], requests)
        usage = self.expect("console usage endpoint responds", console, "GET", "/api/usage")
        self.check("console reports nonzero bot and plugin memory", usage["memory_bytes"] > 0 and
                   any(s["bot_id"] == bot and s["kind"] == "core" and s["memory_bytes"] > 0 for s in usage["sources"]) and
                   any(s["bot_id"] == bot and s["plugin_id"] == plugin["id"] and s["memory_bytes"] > 0 for s in usage["sources"]), usage)
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
