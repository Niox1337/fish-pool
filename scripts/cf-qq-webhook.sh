#!/usr/bin/env bash
# Checks and allows QQ open platform callbacks to /api/qq/webhook/* on the niox.lol zone.
#
#   scripts/cf-qq-webhook.sh --check   read security level, browser integrity check, Bot Fight Mode and custom rules
#   scripts/cf-qq-webhook.sh --apply   also add a skip rule for POST /api/qq/webhook/* (idempotent)
#
# Token: CLOUDFLARE_API_TOKEN in the environment or in the project .env.
# Required permissions: Zone > Zone Read, Zone > Zone Settings Read, Zone > Bot Management Read, Zone > Zone WAF Edit.
set -euo pipefail

ZONE_NAME="niox.lol"
HOST="fish.niox.lol"
RULE_REF="fish_qq_webhook_skip"
EXPR="(http.host eq \"$HOST\" and http.request.method eq \"POST\" and starts_with(http.request.uri.path, \"/api/qq/webhook/\"))"

mode="${1:---check}"
case "$mode" in --check|--apply) ;; *) echo "usage: $0 --check|--apply" >&2; exit 2 ;; esac

root="$(cd "$(dirname "$0")/.." && pwd)"
token="${CLOUDFLARE_API_TOKEN:-}"
if [ -z "$token" ] && [ -f "$root/.env" ]; then
  token="$(grep -E '^CLOUDFLARE_API_TOKEN=' "$root/.env" | tail -1 | cut -d= -f2- | tr -d '"'"'"' ')"
fi
[ -n "$token" ] || { echo "CLOUDFLARE_API_TOKEN missing (environment or .env)" >&2; exit 1; }

api() {
  local method="$1" path="$2" body="${3:-}"
  if [ -n "$body" ]; then
    curl -sS -X "$method" "https://api.cloudflare.com/client/v4$path" -H "Authorization: Bearer $token" -H "Content-Type: application/json" --data "$body"
  else
    curl -sS -X "$method" "https://api.cloudflare.com/client/v4$path" -H "Authorization: Bearer $token"
  fi
}

field() { python3 -c "import sys,json
d=json.load(sys.stdin)
if not d.get('success'):
    print('ERROR ' + '; '.join(e.get('message','') for e in d.get('errors') or []))
else:
    r=d['result']
    for k in sys.argv[1].split('.'):
        r=r.get(k) if isinstance(r,dict) else None
    print(r)" "$1"; }

zone="$(api GET "/zones?name=$ZONE_NAME" | python3 -c "import sys,json;d=json.load(sys.stdin);print(d['result'][0]['id'] if d.get('success') and d['result'] else '')")"
[ -n "$zone" ] || { echo "zone $ZONE_NAME not visible to this token" >&2; exit 1; }

echo "security_level:     $(api GET "/zones/$zone/settings/security_level" | field value)"
echo "browser_check:      $(api GET "/zones/$zone/settings/browser_check" | field value)"
fight="$(api GET "/zones/$zone/bot_management" | field fight_mode)"
echo "bot_fight_mode:     $fight"

entry="$(api GET "/zones/$zone/rulesets/phases/http_request_firewall_custom/entrypoint")"
ruleset_id="$(printf '%s' "$entry" | python3 -c "import sys,json;d=json.load(sys.stdin);print(d['result']['id'] if d.get('success') else '')")"
entry_missing="$(printf '%s' "$entry" | python3 -c "import sys,json;d=json.load(sys.stdin);print(any(e.get('code')==10003 for e in d.get('errors') or []))")"
has_rule="$(printf '%s' "$entry" | python3 -c "import sys,json;d=json.load(sys.stdin);print(any(r.get('ref')=='$RULE_REF' for r in (d.get('result') or {}).get('rules') or []))")"
printf '%s' "$entry" | python3 -c "import sys,json
d=json.load(sys.stdin)
for r in (d.get('result') or {}).get('rules') or []:
    print('custom rule:        %s %s %s' % (r.get('action'), 'on' if r.get('enabled') else 'off', r.get('expression')))"
echo "webhook skip rule:  $has_rule"

if [ "$fight" = "True" ]; then
  echo "Bot Fight Mode is on. Skip rules cannot exempt it on the Free plan, so QQ callbacks may be challenged. Turn it off in Security > Bots." >&2
fi

[ "$mode" = "--apply" ] || exit 0
if [ "$has_rule" = "True" ]; then
  echo "skip rule already present"
  exit 0
fi

rule=$(python3 -c "import json,sys
print(json.dumps({
  'ref': '$RULE_REF',
  'description': 'QQ open platform webhook callbacks',
  'expression': sys.argv[1],
  'action': 'skip',
  'action_parameters': {
    'ruleset': 'current',
    'phases': ['http_ratelimit', 'http_request_firewall_managed'],
    'products': ['bic', 'hot', 'securityLevel', 'uaBlock', 'zoneLockdown'],
  },
  'logging': {'enabled': True},
  'enabled': True,
}))" "$EXPR")

if [ -n "$ruleset_id" ]; then
  body=$(python3 -c "import json,sys;r=json.loads(sys.argv[1]);r['position']={'index':1};print(json.dumps(r))" "$rule")
  result="$(api POST "/zones/$zone/rulesets/$ruleset_id/rules" "$body")"
elif [ "$entry_missing" = "True" ]; then
  body=$(python3 -c "import json,sys;print(json.dumps({'rules':[json.loads(sys.argv[1])]}))" "$rule")
  result="$(api PUT "/zones/$zone/rulesets/phases/http_request_firewall_custom/entrypoint" "$body")"
else
  echo "cannot read custom rules: $(printf '%s' "$entry" | field id)" >&2
  exit 1
fi
printf '%s' "$result" | python3 -c "import sys,json
d=json.load(sys.stdin)
print('skip rule added' if d.get('success') else 'failed: ' + '; '.join(e.get('message','') for e in d.get('errors') or []))
sys.exit(0 if d.get('success') else 1)"
