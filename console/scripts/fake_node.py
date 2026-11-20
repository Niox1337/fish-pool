#!/usr/bin/env python3
"""Fake node agent for local smoke tests of fish-console.

Serves the signed node API under /v1 (HMAC verified per the console contract),
a fake OpenAI compatible endpoint under /llm/v1, a fake GitHub API and raw file host
under /gh-api and /gh-raw, a git ref advertisement under /gh-git, and debug helpers under /_debug.

Environment:
  FISH_NODE_SECRET   base64 of 32 bytes, HMAC key
  FISH_NODE_ID       default node-1
  FAKE_NODE_PORT     default 8701
  FISH_CONSOLE_URL   console origin used by /_debug/emit, default http://127.0.0.1:8787
"""

import base64
import hashlib
import hmac
import json
import os
import sys
import threading
import time
import urllib.parse
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

SECRET = base64.b64decode(os.environ["FISH_NODE_SECRET"])
NODE_ID = os.environ.get("FISH_NODE_ID", "node-1")
PORT = int(os.environ.get("FAKE_NODE_PORT", "8701"))
CONSOLE = os.environ.get("FISH_CONSOLE_URL", "http://127.0.0.1:8787")

LOCK = threading.Lock()
STATE = {
    "desired": {},
    "deleted": [],
    "restarts": [],
    "verified": 0,
    "rejected": 0,
    "qr": {},
    "commit": "a" * 40,
}
COMMIT_VERSIONS = {"a" * 40: "v1.0.0", "b" * 40: "v1.1.0"}

META = """name: astrbot_plugin_demo
display_name: 演示插件
desc: 一个用于冒烟测试的插件
version: {version}
author: fish
"""
SCHEMA = {
    "greeting": {"type": "string", "description": "问候语", "default": "你好"},
    "times": {"type": "int", "default": 1, "slider": {"min": 1, "max": 5, "step": 1}},
    "loud": {"type": "bool"},
}


def sign(ts, method, path, body):
    payload = f"{ts}\n{method.upper()}\n{path}\n{hashlib.sha256(body).hexdigest()}"
    return hmac.new(SECRET, payload.encode(), hashlib.sha256).hexdigest()


def usage_sources(user_id):
    sources = []
    with LOCK:
        docs = [d for d in STATE["desired"].values() if d["user_id"] == user_id]
    for d in docs:
        sources.append({"bot_id": d["bot_id"], "kind": "core", "plugin_id": None, "label": "核心连接", "memory_bytes": 4 << 20, "storage_bytes": 1 << 20, "cpu_percent": 0.3})
        sources.append({"bot_id": d["bot_id"], "kind": "conversations", "plugin_id": None, "label": "对话记录", "memory_bytes": 0, "storage_bytes": 2 << 20, "cpu_percent": 0.0})
        for pl in d["plugins"]:
            sources.append({"bot_id": d["bot_id"], "kind": "plugin", "plugin_id": pl["id"], "label": pl["name"], "memory_bytes": 30 << 20, "storage_bytes": 50 << 20, "cpu_percent": 1.0})
    return sources


def fake_system():
    with LOCK:
        docs = list(STATE["desired"].values())
    now = int(time.time() * 1000)
    procs = [
        {"pid": 1, "ppid": 0, "role": "node", "name": "fish-node", "rss_bytes": 30 << 20, "pss_bytes": 28 << 20, "uss_bytes": 26 << 20,
         "swap_bytes": 0, "cpu_percent": 0.5, "threads": 6, "started_at": now - 3_600_000},
        {"pid": 2, "ppid": 1, "role": "sidecar", "name": "python3", "rss_bytes": 40 << 20, "pss_bytes": 20 << 20, "uss_bytes": 12 << 20,
         "swap_bytes": 0, "cpu_percent": 0.1, "threads": 1, "started_at": now - 3_500_000},
    ]
    pid = 10
    users = sorted({d["user_id"] for d in docs})
    for d in docs:
        for pl in d["plugins"]:
            procs.append({"pid": pid, "ppid": 2, "role": "plugin", "name": "python3", "rss_bytes": 50 << 20, "pss_bytes": 30 << 20,
                          "uss_bytes": 25 << 20, "swap_bytes": 0, "cpu_percent": 1.0, "threads": 2, "started_at": now - 600_000,
                          "plugin": {"user_id": d["user_id"], "bot_id": d["bot_id"], "bot_name": d["name"], "plugin_id": pl["id"], "label": pl["name"]}})
            pid += 1
    history = [{"ts": now - k * 15_000, "host_cpu": 12.5, "container_cpu": 3.0,
                "host_memory_used_bytes": 1 << 30, "container_memory_bytes": 200 << 20} for k in range(4)][::-1]
    return {
        "node_id": NODE_ID, "version": "fake", "started_at": now - 3_600_000, "interval_s": 15, "sampled_at": now,
        "host": {"cpu_count": 2, "cpu_percent": 12.5, "load": [0.1, 0.2, 0.3], "uptime_s": 86_400, "memory_total_bytes": 4 << 30,
                 "memory_available_bytes": 3 << 30, "swap_total_bytes": 0, "swap_free_bytes": 0, "kernel": "6.8.0-fake"},
        "container": {"memory_current_bytes": 200 << 20, "memory_max_bytes": 1600 << 20, "memory_peak_bytes": 300 << 20,
                      "memory_anon_bytes": 150 << 20, "memory_file_bytes": 50 << 20, "oom_kills": 0, "cpu_percent": 3.0,
                      "cpu_limit_cores": None, "pids_current": 9, "pids_max": 512},
        "processes": procs,
        "history": history,
        "disk": {"path": "/data", "total_bytes": 69 << 30, "free_bytes": 59 << 30, "avail_bytes": 56 << 30, "inodes_total": 4_000_000, "inodes_free": 3_900_000},
        "storage": {"node_db_bytes": 1 << 20, "conversations_db_bytes": 2 << 20, "tenants_bytes": 50 << 20, "uv_cache_bytes": 300 << 20},
        "cgroups_enabled": True,
        "tenants": [{"user_id": u, "memory_current_bytes": 30 << 20, "memory_max_bytes": 128 << 20, "memory_peak_bytes": 40 << 20,
                     "oom_kills": 0, "procs": 1} for u in users],
        "bots": {"total": len(docs), "running": sum(1 for d in docs if d["running"])},
        "platforms": [{"kind": "qq_official", "total": 1, "connected": 1}],
        "plugins": {"total": sum(len(d["plugins"]) for d in docs)},
        "logs": {"window_s": 3600, "errors": 1, "warns": 0,
                 "bots": [{"bot_id": d["bot_id"], "bot_name": d["name"], "user_id": d["user_id"], "errors": 1, "warns": 0,
                           "last_error_at": now - 60_000, "last_error": "fake error"} for d in docs[:1]]},
    }


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):
        sys.stderr.write("[fake-node] " + (fmt % args) + "\n")

    def send_json(self, status, obj):
        data = json.dumps(obj, ensure_ascii=False).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def send_text(self, status, text):
        data = text.encode()
        self.send_response(status)
        self.send_header("content-type", "text/plain; charset=utf-8")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def read_body(self):
        n = int(self.headers.get("content-length") or 0)
        return self.rfile.read(n) if n else b""

    def verify(self, body):
        ts = self.headers.get("x-fish-ts")
        sig = self.headers.get("x-fish-sig")
        node = self.headers.get("x-fish-node")
        ok = False
        if ts and sig and node == NODE_ID:
            try:
                fresh = abs(time.time() * 1000 - int(ts)) <= 60_000
            except ValueError:
                fresh = False
            ok = fresh and hmac.compare_digest(sign(ts, self.command, self.path, body), sig)
        with LOCK:
            STATE["verified" if ok else "rejected"] += 1
        if not ok:
            self.send_json(401, {"error": {"code": "bad_signature", "message": "签名校验失败"}})
        return ok

    def do_GET(self):
        self.route(b"")

    def do_DELETE(self):
        self.route(self.read_body())

    def do_POST(self):
        self.route(self.read_body())

    def do_PUT(self):
        self.route(self.read_body())

    def route(self, body):
        url = urllib.parse.urlsplit(self.path)
        parts = [p for p in url.path.split("/") if p]
        query = urllib.parse.parse_qs(url.query)
        if parts[:1] == ["v1"]:
            if not self.verify(body):
                return
            return self.node_api(parts[1:], query, body)
        if parts[:2] == ["llm", "v1"]:
            return self.llm(parts[2:])
        if parts[:1] == ["gh-api"]:
            return self.gh_api(parts[1:])
        if parts[:1] == ["gh-raw"]:
            return self.gh_raw(parts[1:])
        if parts[:1] == ["gh-git"]:
            return self.gh_git(parts[1:], query)
        if parts[:1] == ["_debug"]:
            return self.debug(parts[1:], query)
        self.send_json(404, {"error": {"code": "not_found", "message": "not found"}})

    # ---- signed node API ----
    def node_api(self, p, query, body):
        m = self.command
        if m == "GET" and p == ["health"]:
            with LOCK:
                n = len(STATE["desired"])
            return self.send_json(200, {"node_id": NODE_ID, "version": "fake", "memory_total_bytes": 2 << 30, "memory_used_bytes": 1 << 30, "bots": n})
        if m == "GET" and p == ["system"]:
            return self.send_json(200, fake_system())
        if len(p) >= 2 and p[0] == "bots":
            bot_id = p[1]
            if m == "PUT" and len(p) == 2:
                doc = json.loads(body)
                assert doc["bot_id"] == bot_id
                with LOCK:
                    STATE["desired"][bot_id] = doc
                return self.send_json(200, {"ok": True})
            if m == "DELETE" and len(p) == 2:
                with LOCK:
                    STATE["desired"].pop(bot_id, None)
                    STATE["deleted"].append(bot_id)
                return self.send_json(200, {"ok": True})
            with LOCK:
                doc = STATE["desired"].get(bot_id)
            if doc is None:
                return self.send_json(404, {"error": {"code": "not_found", "message": "bot unknown"}})
            if m == "POST" and p[2:] == ["restart"]:
                with LOCK:
                    STATE["restarts"].append(bot_id)
                return self.send_json(200, {"ok": True})
            if m == "GET" and p[2:] == ["status"]:
                running = doc["running"]
                return self.send_json(200, {
                    "status": "running" if running else "stopped",
                    "status_detail": "",
                    "platforms": [{"id": x["id"], "status": "connected" if x["enabled"] and running else "disabled", "status_detail": ""} for x in doc["platforms"]],
                    "plugins": [{"id": x["id"], "status": "running" if x["enabled"] and running else "stopped", "status_detail": "", "version": x["version"]} for x in doc["plugins"]],
                })
            if m == "GET" and p[2:] == ["logs"]:
                after = int((query.get("after") or ["0"])[0] or 0)
                items = [{"ts": 1700000000000 + i, "level": "info", "source": "core", "message": f"log line {i}"} for i in range(after, after + 3)]
                return self.send_json(200, {"items": items, "cursor": str(after + 3)})
        if m == "GET" and p == ["usage", "summary"]:
            with LOCK:
                bots = {d["bot_id"]: d for d in STATE["desired"].values()}
            users = sorted({d["user_id"] for d in bots.values()})
            rows = []
            plugins = []
            for u in users:
                src = usage_sources(u)
                rows.append({"user_id": u, "memory_bytes": sum(s["memory_bytes"] for s in src), "storage_bytes": sum(s["storage_bytes"] for s in src)})
                for s in src:
                    if s["kind"] == "plugin":
                        plugins.append({**{k: s[k] for k in ("bot_id", "plugin_id", "label", "cpu_percent")},
                                        "user_id": u, "bot_name": bots[s["bot_id"]]["name"]})
            mem = sum(r["memory_bytes"] for r in rows)
            sto = sum(r["storage_bytes"] for r in rows)
            now = int(time.time() * 1000)
            history = [{"ts": now - k * 300_000, "memory_bytes": mem, "storage_bytes": sto} for k in range(3)]
            cpu = {"percent": sum(p["cpu_percent"] for p in plugins), "sampled_at": now, "plugins": plugins}
            return self.send_json(200, {"memory_bytes": mem, "storage_bytes": sto, "users": rows, "history": history, "plugin_cpu": cpu})
        if m == "GET" and p == ["usage"]:
            user_id = (query.get("user_id") or [""])[0]
            sources = usage_sources(user_id)
            now = int(time.time() * 1000)
            mem = sum(s["memory_bytes"] for s in sources)
            sto = sum(s["storage_bytes"] for s in sources)
            history = [{"ts": now - k * 300_000, "memory_bytes": mem, "storage_bytes": sto} for k in range(3)]
            return self.send_json(200, {"quota": {"memory_mb": 0, "storage_mb": 0}, "memory_bytes": mem, "storage_bytes": sto, "sources": sources, "history": history})
        if len(p) >= 2 and p[0] == "qr" and p[1] in ("qq", "weixin"):
            kind = p[1]
            if m == "POST" and len(p) == 2:
                task = f"{kind}-task-{int(time.time() * 1000)}"
                with LOCK:
                    STATE["qr"][task] = 0
                return self.send_json(200, {"task_id": task, "qr_url": f"https://example.invalid/qr/{task}", "interval_s": 2, "expires_at": int(time.time() * 1000) + 300_000})
            if m == "GET" and len(p) == 3:
                task = p[2]
                with LOCK:
                    if task not in STATE["qr"]:
                        return self.send_json(404, {"error": {"code": "not_found", "message": "task unknown"}})
                    STATE["qr"][task] += 1
                    n = STATE["qr"][task]
                if kind == "qq":
                    if n < 2:
                        return self.send_json(200, {"status": "pending"})
                    return self.send_json(200, {"status": "created", "app_id": "102030405", "secret": "qq-secret-abcdefgh"})
                if n < 2:
                    return self.send_json(200, {"status": "pending"})
                if n < 3:
                    return self.send_json(200, {"status": "scanned"})
                return self.send_json(200, {"status": "created", "bot_id": "wxbot@im.bot", "token": "wx-token-1234567890", "base_url": "https://ilinkai.weixin.qq.com", "user_id": "wxuser@im.wechat"})
        self.send_json(404, {"error": {"code": "not_found", "message": "unknown node route"}})

    # ---- fake LLM ----
    def llm(self, p):
        if p == ["models"]:
            auth = self.headers.get("authorization", "")
            if auth.startswith("Bearer sk-good"):
                return self.send_json(200, {"object": "list", "data": [{"id": "fake-chat", "object": "model"}]})
            return self.send_json(401, {"error": {"message": "Invalid API key provided", "type": "invalid_request_error"}})
        self.send_json(404, {"error": {"message": "not found"}})

    # ---- fake GitHub ----
    def gh_api(self, p):
        if self.headers.get("user-agent") != "fish-console":
            return self.send_json(403, {"message": "missing user agent"})
        if len(p) == 3 and p[0] == "repos" and p[1] == "fish-test":
            if p[2] in ("astrbot_plugin_demo", "not_a_plugin"):
                return self.send_json(200, {"default_branch": "main", "private": False, "full_name": f"fish-test/{p[2]}"})
        self.send_json(404, {"message": "Not Found"})

    def gh_git(self, p, query):
        if p == ["fish-test", "astrbot_plugin_demo.git", "info", "refs"] and query.get("service") == ["git-upload-pack"]:
            with LOCK:
                sha = STATE["commit"]

            def pkt(line):
                return f"{len(line) + 4:04x}{line}"

            body = pkt("# service=git-upload-pack\n") + "0000" + pkt(f"{sha} HEAD\0symref=HEAD:refs/heads/main\n") + pkt(f"{sha} refs/heads/main\n") + "0000"
            return self.send_text(200, body)
        self.send_text(401, "auth required")

    def gh_raw(self, p):
        with LOCK:
            sha = STATE["commit"]
        if len(p) == 4 and p[0] == "fish-test" and p[1] == "astrbot_plugin_demo" and (p[2] in ("main", "HEAD") or p[2] in COMMIT_VERSIONS):
            ref = sha if p[2] in ("main", "HEAD") else p[2]
            if p[3] == "metadata.yaml":
                return self.send_text(200, META.format(version=COMMIT_VERSIONS[ref]))
            if p[3] == "_conf_schema.json":
                return self.send_text(200, json.dumps(SCHEMA, ensure_ascii=False))
            if p[3] == "requirements.txt":
                return self.send_text(200, "httpx\n")
        self.send_text(404, "404: Not Found")

    # ---- debug ----
    def debug(self, p, query):
        if p == ["stats"]:
            with LOCK:
                return self.send_json(200, {k: STATE[k] for k in ("verified", "rejected", "deleted", "restarts")} | {"bots": list(STATE["desired"].keys())})
        if p == ["commit"]:
            with LOCK:
                STATE["commit"] = (query.get("sha") or [STATE["commit"]])[0]
                return self.send_json(200, {"commit": STATE["commit"]})
        if len(p) == 2 and p[0] == "desired":
            with LOCK:
                doc = STATE["desired"].get(p[1])
            return self.send_json(200 if doc else 404, doc or {})
        if p == ["emit"]:
            bot_id = (query.get("bot_id") or [""])[0]
            status = (query.get("status") or ["running"])[0]
            kind = (query.get("kind") or ["bot_status"])[0]
            ev_id = (query.get("id") or [bot_id])[0]
            body = json.dumps({"node_id": NODE_ID, "events": [{"bot_id": bot_id, "kind": kind, "id": ev_id, "status": status, "status_detail": "来自节点的事件", "ts": int(time.time() * 1000)}]}).encode()
            path = "/api/internal/node/events"
            ts = str(int(time.time() * 1000))
            req = urllib.request.Request(CONSOLE + path, data=body, method="POST", headers={
                "content-type": "application/json", "x-fish-node": NODE_ID, "x-fish-ts": ts, "x-fish-sig": sign(ts, "POST", path, body)})
            try:
                with urllib.request.urlopen(req, timeout=10) as r:
                    return self.send_json(200, {"console_status": r.status, "body": json.loads(r.read() or b"null")})
            except urllib.error.HTTPError as e:
                return self.send_json(200, {"console_status": e.code, "body": e.read().decode()})
        self.send_json(404, {})


if __name__ == "__main__":
    srv = ThreadingHTTPServer(("127.0.0.1", PORT), Handler)
    print(f"fake node {NODE_ID} on 127.0.0.1:{PORT}", flush=True)
    srv.serve_forever()
