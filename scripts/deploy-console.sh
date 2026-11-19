#!/usr/bin/env bash
# Checks production configuration before deploying the Worker and its static assets.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CONFIG="$ROOT/console/wrangler.toml"
PYTHON="${PYTHON:-python3}"
WRANGLER="${WRANGLER:-wrangler}"

if [ "$#" -ne 1 ]; then
  echo "Usage: $0 --check | --deploy" >&2
  exit 2
fi
case "$1" in
  --check|--deploy) MODE="$1" ;;
  *) echo "Usage: $0 --check | --deploy" >&2; exit 2 ;;
esac

if [ "${FISH_DEV+x}" = x ]; then
  echo "Refusing deployment: unset FISH_DEV in the shell." >&2
  exit 1
fi

REQUIRED="$("$PYTHON" - "$CONFIG" <<'PY'
import re
import sys

try:
    import tomllib
except ImportError:
    sys.exit("Python 3.11+ is required. Set PYTHON to its executable.")

with open(sys.argv[1], "rb") as source:
    config = tomllib.load(source)
errors = []
databases = [d for d in config.get("d1_databases", []) if d.get("binding") == "DB"]
if len(databases) != 1:
    errors.append("Exactly one D1 binding named DB is required.")
else:
    database_id = databases[0].get("database_id", "")
    if (not isinstance(database_id, str)
            or not re.fullmatch(r"[0-9a-fA-F]{8}(?:-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12}", database_id)
            or database_id == "00000000-0000-0000-0000-000000000000"):
        errors.append("Replace the D1 database_id placeholder with the created database UUID.")
variables = config.get("vars", {})
if "FISH_DEV" in variables:
    errors.append("Remove FISH_DEV from wrangler.toml [vars], including values 0 and false.")
required = ["DATA_KEY", "NODE_SECRET"]
provider = variables.get("EMAIL_PROVIDER")
if provider == "resend":
    required.append("RESEND_API_KEY")
elif provider != "cloudflare":
    errors.append("Production EMAIL_PROVIDER must be cloudflare or resend.")
for name in required:
    if name in variables:
        errors.append(f"Remove {name} from [vars] and use a Worker secret.")
if not any(r.get("pattern") == "fish.niox.lol" and r.get("custom_domain") is True
           for r in config.get("routes", [])):
    errors.append("Enable the top-level fish.niox.lol custom domain route.")
if errors:
    sys.exit("\n".join(errors))
print(" ".join(required))
PY
)"

if [ "$MODE" = --check ]; then
  echo "Local configuration checks passed. Required secrets: $REQUIRED"
  echo "No remote calls made. Secret existence and values have not been checked."
  exit 0
fi

for tool in "$WRANGLER" npm worker-build clang; do
  command -v "$tool" >/dev/null || { echo "Missing executable: $tool" >&2; exit 1; }
done
if [ ! -d "$ROOT/web/node_modules" ]; then
  echo "Install frontend dependencies with npm --prefix web ci from the project root." >&2
  exit 1
fi

SECRET_LIST="$(mktemp "${TMPDIR:-/tmp}/fish-console-secrets.XXXXXX")"
trap 'rm -f "$SECRET_LIST"' EXIT
cd "$ROOT/console"
export WRANGLER_SEND_METRICS=false
"$WRANGLER" secret list --config "$CONFIG" --format json > "$SECRET_LIST"
"$PYTHON" - "$SECRET_LIST" "$REQUIRED" <<'PY'
import json
import sys

with open(sys.argv[1]) as source:
    secrets = json.load(source)
if not isinstance(secrets, list) or any(not isinstance(s, dict) or not isinstance(s.get("name"), str) for s in secrets):
    sys.exit("Unexpected wrangler secret list JSON. Deployment stopped.")
names = {s["name"] for s in secrets}
if "FISH_DEV" in names:
    sys.exit("Remove the FISH_DEV Worker secret before deployment.")
missing = [name for name in sys.argv[2].split() if name not in names]
if missing:
    sys.exit("Missing Worker secrets: " + ", ".join(missing))
PY

# Wrangler runs the configured frontend and Rust build before uploading.
"$WRANGLER" deploy --config "$CONFIG"
