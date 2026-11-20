# AstrBot plugins in the fish sidecar

The sidecar runs plugins against the real AstrBot package (`astrbot` 4.29.4 from PyPI, pinned in `uv.lock`). Every `astrbot.*` module a plugin imports is AstrBot's own code. The fork server imports AstrBot once, each plugin runs in its own forked process, and the fish adapter (`fish_sidecar/adapter.py`) connects AstrBot's message event, chat provider, platform and plugin `Context` to the node protocol in `docs/ARCHITECTURE.md` section 8.

A plugin that works in AstrBot loads the same way here. What differs is the set of AstrBot services behind `Context`, listed below.

## Loading

- AstrBot's own loading rules: `@register` or a `Star` subclass in `main.py`, metadata from `metadata.yaml`, `_conf_schema.json` defaults merged with the config the node sends, `__init__(self, context)` or `__init__(self, context, config)`, then `async initialize()`. `async terminate()` runs on shutdown (5 s limit). Saving the config in the console reloads the plugin.
- The plugin module is imported on the plugin process's event loop thread, as AstrBot does, so module level code can create tasks and asyncio schedulers.
- Each plugin process has its own AstrBot root, the plugin data dir. `data` inside it points back at the data dir, so AstrBot paths (`data/plugin_data`, `data/config`, `data/data_v4.db`) and `StarTools.get_data_dir()` all stay in the plugin's own storage. The working directory is the data dir and `data/plugins/<name>` points at the plugin source. `HOME` is `<data dir>/home` with the XDG dirs under it.
- AstrBot's database (`data_v4.db`) holds preferences, conversations, personas and cron jobs. Values a plugin stored in `fish_kv.json` under the earlier fish sidecar are moved into it on the first start and the file is renamed to `fish_kv.json.migrated`.
- A plugin process stops when its bot has received no message for 30 minutes and the plugin has no background work, and starts again on the next message. The constructor, `initialize()` and the loaded hooks run again after each wake, and state kept only in memory is lost. Running asyncio tasks, scheduled timers, open connections and extra threads count as background work and keep the plugin running.
- Plugins run on CPython 3.13. Dependencies need a cp313 wheel or a pure Python release, since the node image has no compiler.
- Plugin code and its venv are read only and shared by every bot that installed the same archive. On the Linux node, a plugin whose import does not depend on the working directory, `HOME`, randomness or a running event loop is imported once and its processes are forked from that import. Other plugins load on their own and the log says why. Values computed at import time, such as timestamps, are shared by all processes forked from one import.
- Each plugin install has a venv with `requirements.txt` installed. The venv also sees the sidecar's libraries: AstrBot and its dependencies, plus Pillow, aiofiles, aiosqlite, APScheduler, beautifulsoup4, lxml, chardet, colorlog, cryptography, defusedxml, Jinja2, openai, psutil, PyJWT, websockets, aiohttp, httpx, pydantic and PyYAML. Libraries the fork server imports before forking are shared in memory and the sidecar's version wins over a pin in `requirements.txt`.

## Events and hooks

- Message handlers (`command`, `command_group`, `regex`, `event_message_type`, `permission_type`, `platform_adapter_type`, `custom_filter`, priorities) are matched with AstrBot's filter objects. `session_waiter` multi turn sessions work inside one plugin.
- Hooks that run: `on_llm_request`, `on_llm_response`, `on_decorating_result`, `after_message_sent`, `on_astrbot_loaded`, `on_platform_loaded`, `on_waiting_llm_request`, `on_plugin_error`.
- Hooks that are registered but never called: `on_agent_begin`, `on_agent_done`, `on_using_llm_tool`, `on_llm_tool_respond`, `on_plugin_loaded`, `on_plugin_unloaded`. The console lists them on the plugin.
- `on_llm_request` hooks on the bot's default LLM reply see the prompt, system prompt and contexts. Text in `extra_user_content_parts` is appended to the prompt and images there are dropped. `on_decorating_result` and `on_llm_response` run only on replies produced in the plugin.
- `get_platform_name()` returns `qq_official`, or `weixin_oc` for the node platform `weixin_ilink`.

## Function tools

- Active `FunctionTool`s in AstrBot's tool manager, including `@filter.llm_tool` functions, are offered to the bot's default LLM reply. Name, description and JSON schema are AstrBot's own (parameters come from the docstring). Tools run the way AstrBot's local executor runs them: the handler with the event, an overridden `call` with an agent run context, or `run`.
- Returned or yielded text goes back to the model. Yielded message results are sent to the user directly. MCP style results with text content are read as text.
- Sub agent handoff tools are not offered.

## Context

- `send_message(umo, chain)` and `StarTools.send_message` go to the node `send_to` op. `event.send` goes to the node `send` op. Message components that the node cannot send are dropped with a warning. `send_streaming` collects the stream and sends it once. `send_typing` does nothing.
- `fish` is the default chat provider, backed by the bot's configured model. Pass `fish` as the provider ID for `llm_generate` or `tool_loop_agent`. `llm_generate` returns requested tool calls to the plugin. `tool_loop_agent` and `event.request_llm` with a tool set run the tools in the plugin process and return the result to the model. Audio and per-call model overrides are ignored by the `fish` provider.
- `persona_manager`, `astrbot_config_mgr`, `kb_manager`, `conversation_manager`, `message_history_manager` and `cron_manager` use AstrBot's implementations and the plugin's own storage. `get_config(umo)` resolves the plugin's configuration profile for that session.
- Knowledge bases are stored under the plugin's `knowledge_base` directory. A plugin using a knowledge base can call `context.register_provider` in its constructor to register its embedding provider and optional reranker before stored knowledge bases load. The `fish` provider supplies chat completions.
- The cron manager loads persisted active-agent jobs after plugin initialisation and shuts down with the plugin. Basic jobs use in-process callbacks and must be registered by the plugin each time it starts.
