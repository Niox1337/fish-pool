# 节点部署

本目录用于在 Linux 主机上运行大肥鱼托管班的节点 (`fish-node`)。当前按 4 GB 主机规划。节点容器内包含 Rust 节点程序、Python 插件宿主 (sidecar)、uv 和 bubblewrap。对外访问通过 Cloudflare Tunnel 完成，主机不需要开放任何入站端口。

## 文件说明

- `node.Dockerfile`: 多阶段构建。第一阶段编译 `node/`，最终镜像基于 Debian bookworm slim。构建上下文是仓库根目录。
- `node.Dockerfile.dockerignore`: 只把构建需要的文件发送给 Docker，避免上传 `node/target` 等大目录。
- `node-entrypoint.sh`: 容器入口，尝试把 cgroup 目录重新挂载为可写，然后启动节点。
- `compose.yml`: 节点和 cloudflared 两个服务。
- `.env.example`: 环境变量模板。

## 首次部署

1. 安装 Docker Engine 和 compose 插件，确认主机使用 cgroup v2 (`stat -fc %T /sys/fs/cgroup` 输出 `cgroup2fs`)。
2. 复制环境变量模板:

   ```sh
   cp deploy/.env.example deploy/.env
   ```

3. 生成节点密钥并写入 `deploy/.env` 的 `FISH_NODE_SECRET`:

   ```sh
   openssl rand -base64 32
   ```

   这个值必须和控制台 Worker 的 `NODE_SECRET` 完全一致。双方用它的 base64 解码结果作为 HMAC 密钥签名请求。
4. 在 Cloudflare Zero Trust 面板创建一个命名隧道，复制隧道 token 填入 `CLOUDFLARE_TUNNEL_TOKEN`。在隧道的 Public Hostname 中添加:
   - 主机名 `node-1.fish.niox.lol`
   - 服务 `http://node:8700`
5. `FISH_NODE_ID` 填写控制台里登记的节点 id (默认 `node-1`)。`FISH_CONSOLE_URL` 默认是 `https://fish.niox.lol`，节点会向它回报事件。
6. 在仓库根目录启动:

   ```sh
   docker compose -f deploy/compose.yml up -d --build
   ```

## 在内存较小的主机上构建

release 构建开启了 LTO，2G 内存可能不够用。可以选择以下任一方式:

- 先加 2G swap 再构建:

  ```sh
  sudo fallocate -l 2G /swapfile && sudo chmod 600 /swapfile
  sudo mkswap /swapfile && sudo swapon /swapfile
  ```

- 在另一台 x86_64 机器上构建镜像，再传到主机:

  ```sh
  docker build -f deploy/node.Dockerfile -t fish-node:latest .
  docker save fish-node:latest | ssh 主机 docker load
  ```

  之后在主机上执行 `docker compose -f deploy/compose.yml up -d` (不加 `--build`)。

## 日常运维

- 查看日志: `docker compose -f deploy/compose.yml logs -f node`。调整日志级别可以在 `.env` 里设置 `RUST_LOG=debug`。
- 每个机器人的运行日志也会进入节点内的环形缓冲区，控制台的日志页面读取的就是它。
- 数据都在命名卷 `fish-data` (容器内 `/data`):
  - `node.db` 保存期望状态、平台会话状态和用量采样
  - `conversations.db` 保存会话历史
  - `tenants/` 保存插件代码、虚拟环境和插件数据
- 备份: 先停止节点再打包数据卷，避免 SQLite 写入一半。

  ```sh
  docker compose -f deploy/compose.yml stop node
  docker run --rm -v fish_fish-data:/data -v "$PWD":/backup debian:bookworm-slim \
      tar czf /backup/fish-data.tgz -C /data .
  docker compose -f deploy/compose.yml start node
  ```

- 升级: 拉取新代码后重新执行 `docker compose -f deploy/compose.yml up -d --build`。节点启动时会从 `node.db` 恢复所有机器人，控制台也会重新下发期望状态，所以重建容器不会丢失配置。
- cloudflared 镜像版本由 `CLOUDFLARED_TAG` 控制，需要升级时在 `.env` 里修改。

## 隔离与降级

节点以 root 运行，插件以 uid 10001 (`fishplug`) 运行。

- 节点为每个租户创建 `t-<user>` cgroup 并写入 `memory.max`，在插件加载前将进程加入该组。Compose 设置了 `cgroup: private` 和 `SYS_ADMIN`，入口脚本会尝试将 cgroup 目录重新挂载为可写。节点无法启用 cgroup 时，每 5 分钟采样一次，连续 3 次超出配额后停止占用内存最大的插件。
- 文件系统隔离: 节点启动时会探测 bubblewrap。可用时插件宿主运行在只读根文件系统中，只能写自己的租户目录和运行目录，看不到 `node.db` 等节点文件。不可用时会记录警告并直接运行插件宿主，此时插件没有沙箱。
- seccomp 和 AppArmor 被设置为 unconfined，否则 bubblewrap 无法在容器内创建命名空间。

## 本地开发

在 macOS 或 Linux 上可以不用 Docker 直接运行:

```sh
cd node
cargo run -- --dev
```

`--dev` 会使用内置的开发密钥，关闭沙箱和 cgroup，数据写到 `node/data`。插件宿主默认从 `../sidecar` 加载，需要本机有 python3 和 uv。如果环境变量 `CC` 指向了不存在的编译器，可以用 `CC=cc CXX=c++ cargo run -- --dev` 运行。
