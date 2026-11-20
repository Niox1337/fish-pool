# astrbot compatibility in the fish sidecar

The `astrbot` package in this directory is a new implementation of the plugin API surface that AstrBot plugins import. It is not AstrBot. A plugin runs in its own process, receives events from the fish node and talks back over the protocol in `docs/ARCHITECTURE.md` section 8.

Anything marked unsupported raises `NotImplementedError` (class `astrbot.UnsupportedFeature`, which is also an `ImportError`) with a message that names the feature. Importing any `astrbot.*` module that does not exist here yields a stub module: importing it works, but reading a name from it raises the same error, so `from astrbot.core.x import Y` fails at load time with the module and name in the message.

## Supported

Plugin loading
- `main.py` with a `Star` subclass, with or without `@register(name, author, desc, version, repo)`. Name, author, description and version fall back to `metadata.yaml`.
- `__init__(self, context)` and `__init__(self, context, config)`.
- `_conf_schema.json` defaults (nested `object` items included) merged with the config the node sends. Saving changes in the console reloads the plugin so its constructor and `initialize()` receive the new values.
- Relative imports inside the plugin (`from .lib import x`). The plugin is imported as `data.plugins.<name>`.
- `async initialize()` after construction and `async terminate()` on shutdown (5 s limit).
- A plugin process stops when its bot has received no message for 30 minutes and the plugin has no background work, and starts again on the next message. The constructor, `initialize()` and the loaded hooks run again after each wake, and state kept only in memory is lost. Running asyncio tasks, scheduled timers, open connections and extra threads count as background work and keep the plugin running.
- Working directory is the plugin data dir. `data/plugins/<name>` points at the plugin source. `HOME` is `<data dir>/home`, and `XDG_CONFIG_HOME`, `XDG_CACHE_HOME` and `XDG_DATA_HOME` live under it, so files written to `~` stay in the plugin's own storage.
- Plugins run on CPython 3.13. Dependencies need a cp313 wheel or a pure Python release, since the node image has no compiler.
- Plugin code and its venv are read only and shared by every bot that installed the same archive. On the Linux node, a plugin whose import does not depend on the working directory, `HOME` or randomness is imported once and its processes are forked from that import. Import time code that reads relative paths, writes files, starts threads or uses `random` still works, the plugin just loads on its own and the log says why. Values computed at import time, such as timestamps, are shared by all processes forked from one import.
- Each plugin install has a venv with `requirements.txt` installed. The venv also sees the sidecar's libraries, which include the ones AstrBot ships and plugins often import without declaring: Pillow, aiofiles, aiosqlite, APScheduler, beautifulsoup4, lxml, chardet, colorlog, cryptography, defusedxml, Jinja2, openai, psutil, PyJWT, websockets, plus aiohttp, httpx, pydantic and PyYAML. aiohttp, httpx, pydantic, PyYAML and Pillow are imported by the fork server before it forks, so every plugin shares one copy in memory and uses the sidecar's version even when `requirements.txt` pins another.

Filters (`astrbot.api.event.filter`)
- `command(name, alias=...)` with typed parameters (`str`, `int`, `float`, `bool`, defaults, `GreedyStr`) and Chinese parameter errors.
- `command_group(name, alias=...)`, `group.command(...)`, nested `group.group(...)`. A bare group name replies with its sub command tree.
- `regex`, `event_message_type` (`ALL`, `GROUP_MESSAGE`, `PRIVATE_MESSAGE`, `OTHER_MESSAGE`), `permission_type` (`ADMIN`, `MEMBER`, with `raise_error`), `platform_adapter_type`, `custom_filter`, `priority=` on every decorator.
- Hooks: `on_llm_request`, `on_llm_response`, `on_decorating_result`, `after_message_sent`, `on_astrbot_loaded`, `on_platform_loaded`, `on_waiting_llm_request`, `on_plugin_error`.
- `@filter.llm_tool(name=...)` function tools are offered to the bot's default LLM reply. Description and parameters come from the Google style docstring (`Args:` with `name(type): text`, types `string`, `number`, `integer`, `boolean`, `object`, `array[...]`) and the signature, where parameters without a default are required. Returned or yielded strings go back to the model. Yielded message results such as `event.image_result(...)` are sent to the user directly. The tool receives the same event object as the `on_llm_request` hooks of that reply, so `set_extra` values carry over.

`AstrMessageEvent`
- `message_str`, `message_obj`, `get_messages`, `get_message_str`, `get_message_outline`, `get_sender_id`, `get_sender_name`, `get_group_id`, `get_self_id`, `get_session_id`, `get_platform_name`, `get_platform_id`, `get_message_type`, `is_private_chat`, `is_admin`, `is_wake_up`, `is_at_or_wake_command`, `unified_msg_origin`, `role`, `session`.
- `plain_result`, `image_result`, `chain_result`, `make_result`, `set_result`, `get_result`, `clear_result`, `stop_event`, `continue_event`, `is_stopped`, `should_call_llm`, `set_extra`, `get_extra`, `clear_extra`.
- `await event.send(...)` and handlers that `return`, `yield` results, `yield` a `MessageChain`, or yield `event.request_llm(...)`.
- `event.request_llm(prompt, system_prompt, contexts, image_urls)` runs `on_waiting_llm_request`, `on_llm_request`, the node `llm` op, `on_llm_response`, `on_decorating_result` and sends the reply.

`Context` and helpers
- `get_using_provider().text_chat(prompt, system_prompt, contexts, image_urls)` returns `LLMResponse` with `completion_text`. `get_using_provider_async`, `get_provider_by_id`, `get_all_providers`, `llm_generate` use the same provider.
- `send_message(umo, chain)` (node `send_to` op), `get_config()`, `register_task`, `get_registered_star`, `get_all_stars`.
- `StarTools.get_data_dir()`, `StarTools.send_message`.
- `Star.put_kv_data`, `get_kv_data`, `delete_kv_data` and `astrbot.core.sp`, stored in `fish_kv.json` in the data dir.
- `session_waiter` multi turn sessions inside one plugin.
- Message components: `Plain`, `Image`, `At`, `Reply`, `Record`, `Face` go over the wire. `AtAll` is sent as `at` with `user_id` `all`. `Image.fromURL`, `fromFileSystem`, `fromBase64`, `fromBytes`, `convert_to_file_path`, `convert_to_base64`.
- `MessageChain` builders (`message`, `at`, `url_image`, `file_image`, `base64_image`), `MessageEventResult`, `ProviderRequest` (with `extra_user_content_parts`), `LLMResponse`, `AstrBotConfig`, `MessageType`, `AstrBotMessage`, `astrbot.core.utils.astrbot_path` helpers, `astrbot.api.logger`.

## Partial

- `get_platform_name()` returns `qq_official` or `weixin_oc` (for the node platform `weixin_ilink`).
- `AstrBotConfig.save_config()` keeps changes in memory only.
- `request_llm(conversation=...)` and `func_tool_manager` are accepted but ignored. The node keeps conversations.
- `context.conversation_manager`: reads return `None` or `[]`, writes raise.
- `llm_generate(tools=...)` ignores tools.
- `on_llm_request` hooks on the default LLM reply see the prompt, system prompt and contexts. Text in `extra_user_content_parts` is appended to the prompt. Images added there are dropped.
- `on_decorating_result` and `on_llm_response` do not run on the node default LLM reply, only on replies produced in the plugin.
- `Video`, `File`, `Poke`, `Forward`, `Node`, `Json`, `Share`, `Music` and other components can be built but are dropped with a warning when sent.
- `send_streaming` collects the stream and sends it once. `send_typing` does nothing. `react` sends the emoji as text.
- `get_group()` returns only what the event carries.

## Unsupported (registered or importable, never active, or raising)

- `FunctionTool` objects, `register_agent` sub agents and `Context.register_llm_tool`: recorded, never offered to the LLM. Tool activation methods return `False`.
- Plugin initiated LLM calls (`event.request_llm`, `provider.text_chat`) carry no tools. Only the bot's default reply offers them.
- `on_agent_begin`, `on_agent_done`, `on_using_llm_tool`, `on_llm_tool_respond`, `on_plugin_loaded`, `on_plugin_unloaded`: recorded, never called.
- `button_action`, `ui_action`: never match.
- Raising: `Star.text_to_image`, `Star.html_render`, `Context.tool_loop_agent`, `get_llm_tool_manager`, `register_web_api`, `register_commands`, `register_provider`, `get_event_queue`, `get_platform`, `get_platform_inst`, `get_db`, `persona_manager`, `provider_manager`, `platform_manager`, `message_history_manager`, `kb_manager`, `cron_manager`, `html_renderer`, `StarTools.create_message`, `create_event`, `send_message_by_id`, TTS, STT, embedding and rerank providers (`get_using_tts_provider` and `get_using_stt_provider` return `None`), `register_platform_adapter`, `ConversationManager` writes, any other `astrbot.core.*` module.
