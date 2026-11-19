# Wire protocol notes

Working notes for the platform protocols the node speaks. Field names and values below are the ones the node sends and expects.

## QQ Official Bot

### Access token

```
POST https://bots.qq.com/app/getAppAccessToken
{"appId": "<app_id>", "clientSecret": "<secret>"}
-> {"access_token": "...", "expires_in": "7200"}
```

`expires_in` arrives as a string of seconds (a number is accepted too). The node refreshes 60 seconds before expiry and drops the cached token after an HTTP 401 or a gateway close code 4004.

Every OpenAPI call carries `Authorization: QQBot <access_token>` and `X-Union-Appid: <app_id>`. Base URL `https://api.sgroup.qq.com`.

### Gateway

1. `GET /gateway` returns `{"url": "wss://..."}`.
2. Connect. The first frame is Hello: `{"op": 10, "d": {"heartbeat_interval": 41250}}`.
3. Send Identify `{"op": 2, "d": {"token": "QQBot <token>", "intents": 33554432, "shard": [0, 1], "properties": {}}}` or, when a session exists, Resume `{"op": 6, "d": {"token": "QQBot <token>", "session_id": "...", "seq": <last s>}}`.
4. Heartbeat every interval: `{"op": 1, "d": <last s or null>}`. The server answers with op 11.
5. Dispatch frames are `{"op": 0, "s": <seq>, "t": "<EVENT>", "d": {...}}`. `READY` carries `session_id` and `user.id`. `RESUMED` confirms a resume.

Other opcodes: 7 asks the client to reconnect and resume, 9 means the session is invalid and a fresh Identify is needed.

Close codes handled: 4004 invalid token (refresh and identify), 4006, 4007 and 4009 (identify again), 4013 and 4014 (intents not allowed, fatal), 4914 (bot offline, sandbox only, fatal), 4915 (bot banned, fatal). Anything else reconnects with backoff.

Intent bit `1 << 25` (GROUP_AND_C2C_EVENT) delivers group @ messages and single chat messages.

### Inbound events

`GROUP_AT_MESSAGE_CREATE`

```json
{"id": "<msg id>", "content": " text", "timestamp": "...", "group_openid": "<group>",
 "author": {"member_openid": "<user in group>"},
 "attachments": [{"content_type": "image/png", "url": "gchat.qpic.cn/...", "filename": "..."}]}
```

`C2C_MESSAGE_CREATE`

```json
{"id": "<msg id>", "content": "text", "timestamp": "...", "author": {"user_openid": "<user>"}, "attachments": []}
```

Attachment URLs may come without a scheme and get `https://` prepended. Content can hold `<@...>` mention markers which are stripped.

### Replies

```
POST /v2/groups/{group_openid}/messages
POST /v2/users/{user_openid}/messages
{"content": "text", "msg_type": 0, "msg_id": "<inbound id>", "msg_seq": 1}
```

A passive reply carries the inbound `msg_id`. Each reply to the same `msg_id` needs a distinct `msg_seq`, the node counts up from 1 per `msg_id`. The passive window is 5 minutes for groups and 60 minutes for single chats. Without a known `msg_id` the message goes out as an active message, which the platform may reject.

Images use rich media upload first:

```
POST /v2/groups/{group_openid}/files    or    POST /v2/users/{user_openid}/files
{"file_type": 1, "url": "https://..." , "srv_send_msg": false}
{"file_type": 1, "file_data": "<base64>", "srv_send_msg": false}
-> {"file_uuid": "...", "file_info": "...", "ttl": 0}
```

then a message with `{"msg_type": 7, "content": " ", "media": {"file_info": "..."}, "msg_id": ..., "msg_seq": ...}`. When upload fails the node sends the image URL as text and logs the failure.

### QR bind

1. Generate a random 32 byte key, base64 encoded.
2. `POST https://q.qq.com/lite/create_bind_task` with `{"key": "<key>"}` and `Accept: application/json`. Response `{"retcode": 0, "data": {"task_id": "..."}}`. A non zero `retcode` is an error with `msg` or `message`.
3. QR content: `https://q.qq.com/qqbot/openclaw/connect.html?task_id=<url encoded task_id>&_wv=2`.
4. Poll `POST https://q.qq.com/lite/poll_bind_result` with `{"task_id": "..."}` every 2 seconds. `data.status`: 1 pending, 2 completed, 3 expired.
5. On completion `data.bot_appid` is the AppID and `data.bot_encrypt_secret` is base64 of `nonce(12) || ciphertext || tag(16)`, AES-256-GCM with the key from step 1. The plaintext is the AppSecret.

## WeChat iLink bot

Base URL `https://ilinkai.weixin.qq.com` unless the login result returns another `baseurl`.

Headers on every call:

```
Content-Type: application/json
AuthorizationType: ilink_bot_token
X-WECHAT-UIN: base64(<random u32 as decimal string>)
Authorization: Bearer <bot_token>        (after login only)
```

Request bodies carry `"base_info": {"channel_version": "<client version string>"}`.

Responses report errors through `ret` and `errcode` (both 0 on success) plus `errmsg`. `errcode` -14 means the login session timed out and the account must scan again.

### QR login

```
GET ilink/bot/get_bot_qrcode?bot_type=3
-> {"qrcode": "<opaque id>", "qrcode_img_content": "<URL to encode as QR>"}

GET ilink/bot/get_qrcode_status?qrcode=<qrcode>     header iLink-App-ClientVersion: 1
-> {"status": "wait" | "scaned" | "confirmed" | "expired", ...}
```

The status call is a long poll of up to about 35 seconds. On `confirmed` the body holds `bot_token`, `ilink_bot_id`, `ilink_user_id` and optionally `baseurl`. The QR code lives about 5 minutes.

### Receiving

```
POST ilink/bot/getupdates
{"base_info": {...}, "get_updates_buf": "<cursor or empty>"}
-> {"ret": 0, "get_updates_buf": "<next cursor>", "msgs": [ ... ]}
```

Long poll, the client timeout is a little above 35 seconds and a client side timeout simply starts the next poll. The cursor is persisted so a restart does not replay messages.

Message fields used: `from_user_id`, `to_user_id`, `message_type` (1 user, 2 bot), `message_id`, `create_time_ms`, `context_token`, `item_list`. Item types: 1 text (`text_item.text`), 2 image (`image_item.media`), 3 voice (`voice_item.text` holds a transcript when present), 4 file, 5 video. Media is stored encrypted on the CDN.

### Sending

```
POST ilink/bot/sendmessage
{"base_info": {...}, "msg": {"from_user_id": "", "to_user_id": "<user>", "client_id": "<unique>",
  "message_type": 2, "message_state": 2, "context_token": "<latest token from this user>",
  "item_list": [{"type": 1, "text_item": {"text": "..."}}]}}
```

Only private chats exist and only passive replies work. A reply needs the most recent `context_token` seen from that user, which stays usable for about 24 hours. The node keeps the latest token per user in `node.db` so replies survive a restart.

Outbound media would need `ilink/bot/getuploadurl`, AES-128-ECB encryption with PKCS7 padding and an upload to the CDN. The node does not implement this and sends image URLs as text.

## Plugin host (sidecar)

The node and the fork server in `sidecar/` speak the JSON lines protocol of the architecture document. Behaviour observed between the two that the document does not spell out:

- Wake prefix. The node removes the first matching wake prefix (default `/`) and surrounding whitespace before it fills `message_str`, and sets `is_at_or_wake`. The sidecar then removes one more leading `/` when `is_at_or_wake` is true. With the default prefix `/echo hi` reaches the plugin as `echo hi`. Only text that still starts with `/` after the node strip, such as `//echo hi`, loses a second slash.
- Load failure. A plugin that fails to import or initialize sends an `error` level `log` line and its process exits with code 2 before `hello`. The node treats exit code 2 before `hello` and an `error` op before `hello` as a load failure. The plugin status becomes `error` with the last error log line as `status_detail`, and the node does not relaunch it until the plugin config changes, the plugin is updated or the bot restarts. Any other exit is a crash and is relaunched with backoff.
- Platform names. The node sends `platform` as `qq_official` or `weixin_ilink`. The sidecar shows `weixin_ilink` to plugins as `weixin_oc`. Plugins address sessions only through `unified_msg_origin`, which starts with the platform id and is returned unchanged, so the name a plugin sees is never read back by the node.
- `message_id` carries the platform message id of the inbound message.
