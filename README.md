# 大肥鱼托管班

大肥鱼托管班是运行在 `fish.niox.lol` 的聊天机器人托管项目。用户凭邀请码注册，自带模型 API 密钥，接入 QQ 或微信，再为自己的小鱼安装插件。控制台部署在 Cloudflare Workers，机器人由独立节点运行。

## 目录

- `console/`：Rust Worker、D1 数据库迁移和控制台 API。
- `node/`：Rust 节点程序，运行机器人、平台连接和资源统计。
- `sidecar/`：Python 插件宿主和 `astrbot` 插件兼容接口。
- `web/`：React + Vite 单页前端，由控制台 Worker 提供静态资源。
- `deploy/`：控制台部署说明、节点 Docker 镜像、compose 和 Tunnel 配置。
- `scripts/`：带生产配置检查的控制台部署入口。
- `docs/ARCHITECTURE.md`：组件边界、数据结构和接口约定。

控制台部署见 [deploy/CONSOLE.md](deploy/CONSOLE.md)，节点部署见 [deploy/README.md](deploy/README.md)。

## 本地检查

前端安装依赖并构建：

```sh
npm --prefix web ci
npm --prefix web run build
```

控制台单元测试和端到端冒烟测试：

```sh
cd console
CC=clang cargo test
TMPDIR=/private/tmp WRANGLER_SEND_METRICS=false ./scripts/smoke.sh --build
```

冒烟测试需要 `worker-build`、Wrangler、Python 3、jq、curl 和 OpenSSL。它使用临时本地 D1 和假节点，不部署 Worker。Linux 可将 `TMPDIR` 改成 `/tmp`。

节点与插件宿主测试：

```sh
cd node
CC=clang cargo test
```

```sh
cd sidecar
uv sync --group dev
uv run pytest
```

## 致谢与说明

本站的小鱼动画素材取自 YunYueSama 发布的 [codex-deepseek-pet](https://github.com/YunYueSama/codex-deepseek-pet) 版本，经缩放和格式转换。角色参考的来源另见[上游素材说明](https://github.com/YunYueSama/codex-deepseek-pet/blob/main/ASSET_LICENSE.md)。[随附的素材许可](assets/whale-LICENSE.txt)

插件接口是独立实现，兼容部分 AstrBot 插件，与 AstrBot 项目没有隶属或合作关系。AstrBot 源码仅用于参考，兼容范围见 [sidecar/COMPAT.md](sidecar/COMPAT.md)。

展示字体使用 Resource Han Rounded CN，遵循 [SIL Open Font License（SIL OFL）](assets/fonts/OFL-License.txt)。
