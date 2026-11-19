# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Stack

React + Vite + TypeScript SPA with Tailwind CSS v4, shadcn/ui (Radix), Motion and TanStack Query, Recharts for usage charts. The user rejected SvelteKit and asked for the framework with the strongest visual ecosystem. Built to static files and served by the Rust Cloudflare Worker `fish-console` on the same origin as `/api`. Backend: Rust Worker + D1 for the console, Rust node agent plus a Python plugin host on a 2 vCPU / 2 GB server for running bots.

## Users

Two groups, both first class:

- People who are not technical and want an AI chat bot for themselves or their QQ / WeChat group without running a server. They need plain spoken guidance and no jargon on the default path.
- AstrBot users who know plugins and API keys and only want to stop maintaining a server. They need the advanced options (plugin config, LLM parameters, logs, per source usage) reachable without hunting.

The default experience is friendly to beginners and advanced options are folded away for people who know what they are doing.

## Product Purpose

大肥鱼托管班 hosts chat bots ("小鱼") for its users. A user registers, deploys a bot with one click, connects QQ Official Bot or WeChat by scanning a QR code with the credentials filled in automatically, brings their own LLM API key, tunes persona and behavior visually, and installs AstrBot plugins by pasting a GitHub repository URL. Success means a newcomer goes from sign up to a bot answering in their QQ group in a few minutes, and an AstrBot user can move their plugin setup over without touching a server.

## Positioning

A small daycare for bots: the platform looks after hosting, restarts, isolation and resource accounting, and the user only decides who the bot is and what it can do. It runs real AstrBot plugins on a lightweight Rust core, so the plugin ecosystem comes along without running a full AstrBot per user.

## Operating Context

- Site: fish.niox.lol. Same origin API under `/api`.
- QQ Official Bot binding through QQ's QR bind flow. A bot created this way can only join groups where the user is group owner.
- WeChat via the iLink bot protocol: private chats only, replies only to incoming messages, 24 hour window, still in gradual rollout on WeChat's side.
- Plugins are found by the user on https://cloud.astrbot.app/plugin and added by repository URL. Plugin settings come from each plugin's `_conf_schema.json`.
- Every user has a resource quota (memory, storage, bot count, plugin count). Usage is shown per source: bot core, conversation storage, and each plugin's memory and storage.
- Users must supply their own OpenAI compatible LLM API key. The platform never provides model credit.

## Capabilities and Constraints

- Email + password registration with an invite code, an email code and a self generated image captcha (Cloudflare Turnstile is not used because it is unreliable in mainland China).
- Bots: create, start, stop, restart, delete, logs.
- Platforms: QQ Official (QR or manual AppID/Secret), WeChat iLink (QR).
- Plugin compatibility covers a documented subset of the AstrBot plugin API. Unsupported features surface as a plugin error with a reason.
- The service is subscription based. At this stage a subscription period comes only from an invite code made by an admin, and each code carries a service length in days. Registration requires a code. Prices are undecided: do not show prices, plans, upgrade offers or promises that the service is free forever.
- Admin accounts can see every user, bot, node and invite, read any user's usage page, adjust quotas and service time, and create or revoke invite codes.

## Brand Commitments

- Name: 大肥鱼托管班. The bot instances are affectionately called 小鱼.
- Theme illustration supplied by the user: a cozy chibi daycare where a pale blue haired teacher (the 梗Wiki mascot, speech bubble hair clip) reads a picture book to four small whale-girl pupils (大肥鱼) in a warm reading corner. The user requires the site to feel warm and cute.
- Character assets from YunYueSama's 大肥鱼 project require visible attribution: `作者：YunYueSama` and `仓库：https://github.com/YunYueSama/codex-deepseek-pet`, plus a note that the assets were modified. Place it on the about section and in the README.
- Interface language: Simplified Chinese.

## Evidence on Hand

- Theme illustration: `web/src/assets/brand/daycare-hero.webp` (copied from the user's upload) and `/Users/niox/project/gengwiki/assets/whale-daycare.png`.
- 大肥鱼 sprites and expressions: `/Users/niox/project/gengwiki/assets/references/whale/`, emoji set in `/Users/niox/project/gengwiki/assets/emojis/`.
- No users, testimonials, usage numbers or press exist yet. Do not fabricate any.

## Product Principles

1. Scan and go. Every connection step that can be a QR code or one click is one.
2. Beginners first, experts not blocked. Plain words up front, power folded one step away.
3. Honest resources. Show exactly what uses memory and storage and never hide a limit until it bites.
4. The user's keys stay the user's. Secrets are masked after saving and never shown again.
5. Never promise what the platforms do not allow. QQ and WeChat limits are stated where the user makes the choice.

## Accessibility & Inclusion

WCAG 2.2 AA contrast and keyboard access for the console. Respect reduced motion for all character animation.
