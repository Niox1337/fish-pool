# Build status

## Done

### Local full-stack test

Run `scripts/e2e.sh` from the project root. It builds with the required C compilers, starts local Wrangler and D1, the real node and Python sidecar, and local HTTP fixtures for the LLM and plugin repository. It checks invite registration, persona delivery, LLM replies, plugin commands, conversation history and bot memory usage. The signed loopback endpoints and `FISH_CODELOAD_BASE` override require `FISH_DEV`. No console UI configuration is added.

Requires `cargo`, `worker-build`, `wrangler`, `uv`, `python3` and `sidecar/.venv` with the sidecar dependencies. Run `uv sync` in `sidecar/` to prepare that environment. Processes and temporary state are removed on exit. Use `scripts/e2e.sh --keep` to retain logs, recorded LLM requests and D1 state for inspection.

- Contract: docs/ARCHITECTURE.md.
- console/ (Rust Worker + D1): 36 unit tests, smoke.sh passes 94/94 locally. Build with `CC=clang`.
- node/ (Rust node agent) and deploy/ (Docker, compose with cloudflared): tests pass. Build with `CC=cc CXX=c++`.
- sidecar/ (Python fork server + astrbot shim): 10 tests pass. Coverage in sidecar/COMPAT.md.
- Landing comp B approved. All 7 plates in assets/plates/ accepted in review round 2. Impeccable build phase is now `hero`.
- Plates exported as webp to web/src/assets/plates/.
- Display font: Resource Han Rounded CN (OFL), static 700 and 900 instances in assets/fonts/.
  M PLUS Rounded 1c was dropped because it lacks simplified glyphs (扫 码 带 问 题 录 账 们 务 and more).
- web/scripts/subset-display-font.py writes web/src/assets/fonts/display-{700,900}.woff2. Run it after the page copy exists:
  `uv run --with fonttools --with brotli python scripts/subset-display-font.py`

## Next
1. Replace web/index.html, src/index.css, src/App.tsx, src/main.tsx (template leftovers) with tokens, @font-face for the display face, router and QueryClient.
2. Landing hero at 1536x1024 from .impeccable/build/scaffold/layout.css. Place the plates first, then capture .impeccable/review/hero-repro.png, run `build-phase record hero`, then the text layer, then advance. Then sections, motion, responsive.
3. Console pages (auth, bots, bot tabs, usage, keys, account, about with YunYueSama attribution).
4. Reconcile the contract proposals from the three agents:
   - `install_rev`.
   - HMAC key is the base64-decoded NODE_SECRET.
   - Fork server listens on `--socket`.
   - The node expects `{"op":"error"}` on plugin load failure, but the sidecar sends an error `log` and exits with code 2.
   - New D1 columns and tables, plus error codes.
5. End to end test, then impeccable optimize, distill, adapt, polish, then deploy to fish.niox.lol.
