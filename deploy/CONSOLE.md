# 控制台部署

本说明用于把大肥鱼托管班控制台部署到 `fish.niox.lol`。Rust Worker `fish-console` 处理 API，D1 保存数据，同一个 Worker 提供 `web/dist` 的前端资源。节点部署见 [README.md](README.md)。

下面的登录、建库、远程迁移、上传 secret 和部署命令都由站点所有者手动执行。`scripts/deploy-console.sh --check` 只检查本地配置，不访问 Cloudflare。只有明确传入 `--deploy` 才会查询远程 secret 并发布。

## 准备环境

需要以下工具：

- Node.js 20.19.x、20 系列更高版本，或 22.12.0 及以上版本，以及 npm。当前安装的 Vite 声明运行范围为 `^20.19.0 || >=22.12.0`。
- Rust stable、Cargo、`wasm32-unknown-unknown` target 和 clang。
- `worker-build` 和 Wrangler 4.20.0 或更新版本。
- Python 3.11 或更新版本，部署脚本用标准库 `tomllib` 解析 TOML。
- OpenSSL。冒烟测试另需 jq、curl 和 Python 3。
- 拥有 `niox.lol` 的 Cloudflare 账号，域名在该账号内处于启用状态，账号可管理 Workers、D1 和 Email Service。

在项目根目录安装依赖：

```sh
cd /Users/niox/project/fish
npm install -g wrangler
rustup target add wasm32-unknown-unknown
cargo install --locked worker-build
npm --prefix web ci
```

部署脚本默认调用 `python3`。如果 macOS 的 `python3` 仍是 3.9，而已经安装了 `python3.14`，先执行 `export PYTHON=python3.14`。脚本不会自动安装工具或前端依赖。

## 登录并创建 D1

由所有者执行：

```sh
cd /Users/niox/project/fish/console
wrangler login
wrangler d1 create fish
```

把返回的 `database_id` 原样填入 `console/wrangler.toml` 的 `[[d1_databases]]`，替换 `00000000-0000-0000-0000-000000000000`。保留 `binding = "DB"`、`database_name = "fish"` 和 `migrations_dir = "migrations"`。不要把账号 id 填成数据库 id。

迁移文件位于 `console/migrations/`：

- `0001_init.sql` 创建基础表。
- `0002_admin_subscription.sql` 增加管理员、服务期限和邀请码相关字段及表。
- `0003_invite_quota.sql` 增加邀请码的注册配额字段。发布使用该字段的 Worker 前先应用迁移。

仍在 `console/` 执行远程迁移，并核对列表：

```sh
wrangler d1 migrations apply DB --remote
wrangler d1 migrations list DB --remote
```

确认迁移目标是刚创建的 `fish`。后续更新也用 `migrations apply`，不要手工重复执行迁移文件。部署脚本不会创建数据库或执行迁移。

## 配置 secret

Worker 从环境读取以下名称。随机密钥都需要分别生成，不要共用一份。

| 名称 | 是否必需 | 用途和取得方式 |
| --- | --- | --- |
| `DATA_KEY` | 必需 | 用 `openssl rand -base64 32` 生成。base64 解码后为 32 字节，用于 AES-256-GCM 加密 D1 中的模型 API 密钥和平台凭据。 |
| `NODE_SECRET` | 必需 | 另用 `openssl rand -base64 32` 生成。与节点双向请求的 HMAC-SHA256 签名密钥，节点的 `FISH_NODE_SECRET` 必须与它完全一致。 |
| `NODE_SECRET_NODE_1` | 可选 | 给 `node-1` 单独配置密钥时，用 `openssl rand -base64 32` 生成。它优先于 `NODE_SECRET`，对应节点的 `FISH_NODE_SECRET` 此时必须与它一致。 |
| `ADMIN_EMAILS` | 可选 | 输入允许直接注册为管理员的邮箱，多个地址用英文逗号分隔。邮箱由所有者提供，不能用 OpenSSL 生成。 |
| `RESEND_API_KEY` | 仅 `EMAIL_PROVIDER = "resend"` 时必需 | 在 Resend 后台创建发送邮件的 API 凭据，不能用 OpenSSL 生成。默认 Cloudflare 邮件方案不需要它。 |
| `GITHUB_TOKEN` | 可选 | 在 GitHub 创建访问插件仓库所需的 token，用于提高插件检查的 API 限额，不能用 OpenSSL 生成。缺省时使用未认证的 GitHub API。 |
| `AI_GATEWAY_TOKEN` | 可选 | 在 Cloudflare 后台创建带 AI Gateway Run 权限的 API token。配置后，OpenAI、Anthropic、Google Gemini 和 xAI 的请求由节点经 `AI_GATEWAY` 指向的网关转发。缺省时节点直连这些服务商。 |

在 `console/` 逐项上传，命令会提示输入内容：

```sh
openssl rand -base64 32
wrangler secret put DATA_KEY
openssl rand -base64 32
wrangler secret put NODE_SECRET
```

每次生成后，将该次输出保存在密码管理器，再粘贴到对应的输入提示中。将 `NODE_SECRET` 的同一份内容填入节点主机的 `deploy/.env` 中的 `FISH_NODE_SECRET`。节点配置文件仅保留在所有者管理的主机上。

新建 Worker 尚不存在时，Wrangler 会提示创建 Worker，所有者确认后继续上传 secret。不要先绕过部署脚本发布缺少 secret 的应用。

需要可选配置时，再执行对应命令：

```sh
wrangler secret put ADMIN_EMAILS
wrangler secret put NODE_SECRET_NODE_1
wrangler secret put RESEND_API_KEY
wrangler secret put GITHUB_TOKEN
wrangler secret put AI_GATEWAY_TOKEN
```

`AI_GATEWAY` 写在 `wrangler.toml` 的 `[vars]` 里，格式是 `https://gateway.ai.cloudflare.com/v1/<账户 ID>/<网关 ID>`。当前使用自建网关 `fish`，网关上已关闭日志、缓存、限速和重试，并开启鉴权。节点请求带 `cf-aig-collect-log: false`，网关不保存用户的对话内容。改动后需要重新推送小鱼（启动、重启或修改设置）才会生效。

`ADMIN_EMAILS` 也能作为普通环境变量读取，但这里使用 secret，避免把个人邮箱写入文件。不要在 `[vars]` 再声明同名变量。它只影响注册时的管理员赋权和免邀请码注册，不会自动提升已有账号。已有账号用后面的 `make-admin.sh`。

其他节点的独立密钥名称按 `console/src/app.rs` 的规则处理：节点 id 的 ASCII 字母转大写，非 ASCII 字母数字字符替换为 `_`，前面加 `NODE_SECRET_`。未配置对应名称时回退到 `NODE_SECRET`。不要配置空值或格式错误的独立密钥，它们不会回退到有效的通用密钥。

`DATA_KEY` 必须单独备份。直接换掉它会使已有加密数据无法解密，当前部署流程不包含数据密钥轮换。不要把任何 secret 放到前端的 `VITE_*` 环境变量、文档或命令行参数里。

## 开通验证码邮件

生产环境目前通过 Resend 从 `noreply@gengwiki.niox.lol` 发信。本机项目根目录的 `.env` 用 `RESEND` 保存密钥，Worker 则读取 `RESEND_API_KEY` Secret。

如要改用 Cloudflare，配置示例如下：

```toml
[[send_email]]
name = "EMAIL"
allowed_sender_addresses = ["noreply@niox.lol"]

[vars]
EMAIL_PROVIDER = "cloudflare"
EMAIL_FROM = "noreply@niox.lol"
EMAIL_FROM_NAME = "大肥鱼托管班"
```

这是配置片段，用来核对现有内容，不要重复追加一份 `[vars]`。Worker 使用 `EMAIL` 绑定发送注册和密码重置验证码，不需要额外的 Cloudflare 邮件 API token。

1. 在 Cloudflare 的 Compute → Email Service → Email Sending 中选择 Onboard Domain，选择 `niox.lol`。
2. 核对并添加面板给出的 DNS 记录，完成发送域验证。记录涉及退信子域的 MX、SPF、DKIM，以及 DMARC。具体记录值使用面板返回的内容，不要凭空填写，也不要覆盖现有邮件服务的 DNS 配置。
3. 确认发送域已启用，`EMAIL_FROM` 与 `allowed_sender_addresses` 都保持 `noreply@niox.lol`。
4. 部署后用真实注册流程确认邮件到达收件箱，同时检查垃圾邮件和 Email Service 的发送记录。

只开通收件转发不能代替发送域验证。发送域未开通前只能向账号内已验证的目标地址发送，不能据此认为普通用户也能收到验证码。当前绑定只限制发件人，不设置收件人白名单，供注册用户接收验证码。参见 [Cloudflare 发送邮件说明](https://developers.cloudflare.com/email-service/get-started/send-emails/) 和 [send_email 绑定限制](https://developers.cloudflare.com/email-service/configuration/send-bindings/)。

如果选用 Resend，先在 Resend 验证发件域，上传 `RESEND_API_KEY`，再把 `EMAIL_PROVIDER` 改成 `resend`。不要在生产使用 `EMAIL_PROVIDER = "log"`。

## 启用域名并检查变量

取消 `console/wrangler.toml` 顶层 `routes` 的注释，保持在 `[build]` 之前：

```toml
routes = [
  { pattern = "fish.niox.lol", custom_domain = true },
]
```

这是同一个 Worker 的自定义域名，覆盖整个站点，不是仅给 `/api/*` 配一条代理路由。发布时 Cloudflare 会配置 DNS 和证书，已有同名 CNAME 会阻止创建自定义域名，需要所有者先核对并处理冲突。见 [Custom Domains](https://developers.cloudflare.com/workers/configuration/routing/custom-domains/)。

核对 `[vars]` 的 `DEFAULT_NODE_ID = "node-1"`、`DEFAULT_NODE_URL = "https://fish-node-1.niox.lol"`，与节点的 Tunnel 地址一致。定时任务 `*/5 * * * *` 每 5 分钟执行清理、重试推送和到期处理。

**生产环境绝不能设置 `FISH_DEV`。** 不要在 `wrangler.toml`、Worker secret 或部署终端环境中设置它，包括 `"0"`。开发模式会把验证码和图形验证码答案写入日志，还会允许开发用 HTTP 地址和 GitHub 请求地址覆盖。部署脚本拒绝任何 `[vars].FISH_DEV`，也拒绝远程 secret 列表中的 `FISH_DEV`。

`GITHUB_API_BASE` 和 `GITHUB_RAW_BASE` 只用于开发测试，生产不配置它们。

## 构建与发布

在项目根目录先检查本地配置：

```sh
./scripts/deploy-console.sh --check
```

数据库 id 尚未填写或自定义域名仍被注释时，检查会失败并给出原因。它不会构建、登录或查询远程 secret。

可以先独立运行本地构建和测试：

```sh
npm --prefix web run build
cd console
CC=clang cargo test
TMPDIR=/private/tmp WRANGLER_SEND_METRICS=false ./scripts/smoke.sh --build
```

Linux 将 `TMPDIR` 改成 `/tmp`。冒烟测试使用临时本地 D1 和假节点，并在临时环境中启用开发模式，不修改生产配置。

准备就绪后，由所有者在项目根目录执行：

```sh
./scripts/deploy-console.sh --deploy
```

脚本先检查本地配置，再执行 `wrangler secret list --format json`。缺少 `DATA_KEY` 或 `NODE_SECRET` 时拒绝发布，使用 Resend 时还检查 `RESEND_API_KEY`。它只能确认名称存在，不能读取或校验远程 secret 的值，所有者仍需确认密钥长度和节点两端内容一致。

检查通过后，脚本运行 `wrangler deploy`。Wrangler 的 `[build]` 依次执行 `npm --prefix ../web run build` 和 `CC=clang worker-build --release`，前端或 Rust 构建失败都会停止发布。该流程使用已经安装的 `worker-build`，不会每次重新安装。

静态资源来自 `web/dist`，绑定名称为 `ASSETS`。`/api` 和 `/api/*` 优先进入 Rust，其余地址先使用静态资源，没有对应文件时回退到 `index.html`，所以直接访问或刷新 `/login`、`/app/*` 都能进入 SPA。Rust 中的非 API 请求也转交 `ASSETS`。路由方式见 [Workers 静态资源配置](https://developers.cloudflare.com/workers/wrangler/configuration/#assets)。

## 创建第一个管理员和邀请码

以下脚本都在 `console/` 执行，并明确使用 `--remote`。省略它会操作本地开发数据库。

先创建一个单次使用、提供 30 天服务期限的邀请码：

```sh
cd /Users/niox/project/fish/console
./scripts/make-invite.sh 30 --remote --max-uses 1 --note bootstrap
```

最后一行是邀请码。打开 `https://fish.niox.lol/login` 的注册入口，用所有者自己的邮箱、邮件验证码和邀请码完成注册。邀请码不是密码，不要放到公开文档里。

然后提升这个已存在的账号，邮箱只在终端提示中输入：

```sh
printf '管理员邮箱: '
read -r ADMIN_EMAIL
./scripts/make-admin.sh "$ADMIN_EMAIL" --remote
unset ADMIN_EMAIL
```

脚本返回的查询结果必须包含该账号，且 `role` 为 `admin`。结果为空表示账号尚不存在，不算创建成功。重新加载页面，确认能够访问管理功能。

后端允许 `ADMIN_EMAILS` 名单内的邮箱免邀请码注册，并自动获得管理员角色。不过当前 `web/src/pages/auth/AuthPage.tsx` 在请求验证码前会校验邀请码格式，页面操作仍需填写邀请码。首次部署按上面的流程先生成邀请码，再注册和提升管理员。名单外用户始终需要邀请码。

## 上线检查

- [ ] `https://fish.niox.lol/`、`/login` 和 `/app/` 可访问，刷新深层页面不会出现静态 404。
- [ ] 未登录请求 `/api/me` 返回 401 JSON，`/api/captcha` 返回 `id` 和 `svg`，浏览器能显示图形验证码。
- [ ] 无效 API 地址返回 JSON 错误，不会返回 SPA HTML。浏览器直接打开 API 地址也保持此行为。
- [ ] 普通邮箱填写有效邀请码和图形验证码后能收到注册邮件，完成注册后服务期限正确。已使用的单次邀请码不能再次兑换。
- [ ] 第一个管理员的 `role` 是 `admin`，能进入管理页。
- [ ] 按 [节点部署说明](README.md) 启动节点和 cloudflared，`FISH_NODE_ID`、`FISH_CONSOLE_URL` 及签名密钥与控制台一致。
- [ ] 管理概览中 `node-1` 显示在线。`GET /api/admin/overview` 返回的 `nodes` 中，该节点的 `online` 为 `true`。这来自控制台对节点的签名健康检查，仅 Tunnel 进程存活不能证明节点在线。
- [ ] Worker 的定时触发器包含每 5 分钟任务，运行日志无持续迁移、解密或签名错误。
- [ ] Worker 变量和 secret 列表中没有 `FISH_DEV`，生产日志中没有 `[dev]` 验证码记录。

## 本地开发

安装前端依赖后，可以在 `console/.dev.vars` 中配置仅限本地使用的 `DATA_KEY`、`NODE_SECRET`、`FISH_DEV=1` 和 `EMAIL_PROVIDER=log`。生成密钥的方法与上面相同。不要把该文件用于生产。

在 `console/` 先执行 `wrangler d1 migrations apply DB --local`，再用 `wrangler dev --local` 启动。配置中的构建命令会生成 SPA 和 Wasm。需要前端热更新时，另开终端在 `web/` 执行 `npm run dev`，Vite 将 `/api` 转发到 `http://127.0.0.1:8787`。本地联调真实节点时，`DEFAULT_NODE_URL` 也要改为本地节点地址，仅放在本地配置中。

## QQ Webhook 回调放行

QQ 开放平台的回调请求打到 `https://fish.niox.lol/api/qq/webhook/<平台 ID>`。在 Cloudflare 后台创建一个 API token，权限为 Zone Read、Zone Settings Read、Bot Management Read 和 Zone WAF Edit，范围限定 `niox.lol`，写进项目根目录 `.env` 的 `CLOUDFLARE_API_TOKEN`，然后执行：

```sh
scripts/cf-qq-webhook.sh --check
scripts/cf-qq-webhook.sh --apply
```

`--check` 只读，列出安全级别、浏览器完整性检查、Bot Fight Mode 和现有自定义规则。`--apply` 在自定义规则最前面加一条跳过规则，只匹配 `POST /api/qq/webhook/*`，重复执行不会重复添加。免费版的 Bot Fight Mode 无法用规则跳过，脚本发现它开着时会提示，需要在 Security > Bots 手动关闭。
