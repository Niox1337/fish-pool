# 节点部署

本目录用于在 Linux 主机上运行大肥鱼托管班的节点 (`fish-node`)。当前按 4 GB 主机规划。节点容器内包含 Rust 节点程序、Python 插件宿主 (sidecar)、uv 和 bubblewrap。对外访问通过 Cloudflare Tunnel 完成，主机不需要开放任何入站端口。

## 文件说明

- `node.Dockerfile`: 多阶段构建。第一阶段编译 `node/`，最终镜像基于 `python:3.13-slim-trixie`，插件运行在 CPython 3.13 上。构建上下文是仓库根目录。`NODE_BIN=prebuilt` 时跳过编译，直接使用 `deploy/dist/fish-node`。
- `node.Dockerfile.dockerignore`: 只把构建需要的文件发送给 Docker，避免上传 `node/target` 等大目录。
- `node-entrypoint.sh`: 容器入口，尝试把 cgroup 目录重新挂载为可写，然后启动节点。
- `compose.yml`: 节点和 cloudflared 两个服务。
- `.env.example`: 环境变量模板。
- `../scripts/deploy-node.sh`: 在本机交叉编译 linux/amd64 的节点程序，上传到主机后只构建运行镜像并重启节点。

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
   - 主机名 `fish-node-1.niox.lol`。Cloudflare 免费证书只覆盖 `*.niox.lol` 一层子域，`node-1.fish.niox.lol` 这样的二级子域无法建立 HTTPS 连接。
   - 服务 `http://node:8700`
5. `FISH_NODE_ID` 填写控制台里登记的节点 id (默认 `node-1`)。`FISH_CONSOLE_URL` 默认是 `https://fish.niox.lol`，节点会向它回报事件。
6. 在仓库根目录启动:

   ```sh
   docker compose -f deploy/compose.yml up -d --build
   ```

## 在本机编译后部署

release 构建开启了 LTO，在 2 核 4G 的主机上编译会占用大量内存，Rust 镜像和编译缓存还要占近 4 GB 磁盘。推荐在开发机上编译，主机只构建运行镜像:

```sh
scripts/deploy-node.sh --deploy
```

脚本用 `docker buildx` 编译 linux/amd64 的 `fish-node`（arm64 开发机也可以，构建阶段在本机架构上运行并交叉编译），把二进制、sidecar 和部署文件上传到主机的 `/opt/fish`，在 `deploy/.env` 里写入 `NODE_BIN=prebuilt`，然后构建运行镜像、重启节点并清理悬空镜像和多余的构建缓存。只编译不部署时用 `--build`，产物在 `deploy/dist/fish-node`。SSH 目标默认取仓库根目录 `.env` 的 `SSH`，可以用 `FISH_SSH`、`FISH_SSH_KEY` 和 `FISH_REMOTE` 覆盖。

也可以在主机上直接编译 (`NODE_BIN=build`，默认值)。内存不够时先加 swap:

```sh
sudo fallocate -l 2G /swapfile && sudo chmod 600 /swapfile
sudo mkswap /swapfile && sudo swapon /swapfile
```

## 国内主机

国内主机访问 Docker Hub、ghcr.io、crates.io 和 PyPI 都很慢，从境外上传镜像通常也很慢。做法是只上传源码，在主机上用国内镜像源构建。在 `deploy/.env` 里加上：

```sh
UV_IMAGE=astral/uv:0.8.0
CARGO_REGISTRY=sparse+https://rsproxy.cn/index/
APT_MIRROR=http://mirrors.tencent.com
PIP_INDEX=https://mirrors.cloud.tencent.com/pypi/simple
```

`PIP_INDEX` 同时用于构建镜像和运行时安装插件依赖。Docker 本身的镜像加速在 `/etc/docker/daemon.json` 的 `registry-mirrors` 配置，腾讯云镜像默认已经配置好。在主机上编译时 `CARGO_REGISTRY` 才会用到。

插件压缩包默认经 console 中转下载（`/api/internal/plugin-archive/...`，Cloudflare 到 GitHub 的链路），中转失败再直连 GitHub，所以国内主机不需要额外配置 GitHub 代理。

`fish-node-1` 当前部署在腾讯云上海的轻量应用服务器上，代码位于 `/opt/fish`，用 `scripts/deploy-node.sh --deploy` 更新。主机上不保留 Rust 镜像和编译缓存，节点容器内存上限是 2700 MB (`NODE_MEM_LIMIT`)，主机另有 4 GB swap。

## 日常运维

- 查看日志: `docker compose -f deploy/compose.yml logs -f node`。调整日志级别可以在 `.env` 里设置 `RUST_LOG=debug`。
- 每个机器人的运行日志也会进入节点内的环形缓冲区，控制台的日志页面读取的就是它。
- 数据都在命名卷 `fish-data` (容器内 `/data`):
  - `node.db` 保存期望状态、平台会话状态和用量采样
  - `conversations.db` 保存会话历史
  - `tenants/` 保存每个机器人的插件数据和安装记录
  - `shared/plugins/` 保存插件代码和虚拟环境。安装了同一个插件压缩包的机器人共用一份，插件进程只能读取。没有机器人使用的目录会在节点启动、安装和卸载插件后删除
- 备份: 先停止节点再打包数据卷，避免 SQLite 写入一半。

  ```sh
  docker compose -f deploy/compose.yml stop node
  docker run --rm -v fish_fish-data:/data -v "$PWD":/backup python:3.13-slim-trixie \
      tar czf /backup/fish-data.tgz -C /data .
  docker compose -f deploy/compose.yml start node
  ```

- 升级: 拉取新代码后重新执行 `docker compose -f deploy/compose.yml up -d --build`，或在开发机上执行 `scripts/deploy-node.sh --deploy`。节点启动时会从 `node.db` 恢复所有机器人，控制台也会重新下发期望状态，所以重建容器不会丢失配置。
- cloudflared 镜像版本由 `CLOUDFLARED_TAG` 控制，需要升级时在 `.env` 里修改。
- Python 版本由构建参数 `PYTHON_IMAGE` 决定。更换版本后，节点发现插件的安装记录对应旧版本解释器，会在启动时重新下载插件并重建虚拟环境，插件的 `data/` 目录保留。

## 隔离与降级

节点以 root 运行，插件以 uid 10001 (`fishplug`) 运行。

- 节点为每个租户创建 `t-<user>` cgroup 并写入 `memory.max`，在插件加载前将进程加入该组。Compose 设置了 `cgroup: private` 和 `SYS_ADMIN`，入口脚本会尝试将 cgroup 目录重新挂载为可写。`SYS_PTRACE` 让节点能读取沙箱内插件进程（uid 10001）的 `/proc/<pid>/smaps_rollup`，按私有内存（Private_Clean 加 Private_Dirty）统计插件内存，和 fork server 共享的页不计入插件。缺少它时节点退回 VmRSS，共享内存会被重复计入。插件进程的能力上限集为空，拿不到这项能力。节点每 10 秒按自身 cgroup 的空闲内存调整各租户的 `memory.max`，在配额和两倍配额之间，并保留 256 MiB 不分出去，所以节点有余量时租户可以短时间超出配额。持续超出配额 3 分钟后，停止该租户占用内存最大的插件。节点无法启用 cgroup 时，每 5 分钟采样一次，连续 3 次超出配额后停止占用内存最大的插件。
- 文件系统隔离: 节点启动时会探测 bubblewrap。可用时插件宿主运行在只读根文件系统中，只能写自己的租户目录和运行目录，看不到 `node.db` 等节点文件。不可用时会记录警告并直接运行插件宿主，此时插件没有沙箱。
- seccomp 和 AppArmor 被设置为 unconfined，否则 bubblewrap 无法在容器内创建命名空间。
- 共享导入: 同一份插件安装在插件宿主里只导入一次 (模板进程)，各机器人的插件进程从模板 fork 出来，共享已导入模块的内存，配置、数据目录和工作目录仍然各自独立。模板进程放在 `templates` cgroup，内存上限 512 MB。导入时依赖工作目录、随机数、线程或打开文件，或在模块里保存了导入时的时间或进程号的插件会自动改为单独加载，日志里会写原因。共享安装按插件提交号、当前解析出的依赖版本和 Python 版本区分，依赖有新版本时新安装不会复用旧的依赖。设置 `FISH_SIDECAR_TEMPLATES=0` 可以关闭。
- 插件宿主设置 `MALLOC_ARENA_MAX=2`，减少多线程下 glibc 的内存碎片。机器人 30 分钟没有收到消息时，空闲插件会休眠，收到消息时自动唤醒。符合条件且在 `metadata.yaml` 声明 `fish_pool: { command_only_idle: true }` 的插件按自己的指令计算空闲时间，由指令或别名唤醒。在 `deploy/.env` 里设置 `FISH_PLUGIN_IDLE_SECS` 可以修改等待时间，设为 0 关闭休眠。
- 知识库读取 `.txt` 时直接解码文本。使用可写 cgroup v2 的 Linux 节点可以在 `deploy/.env` 里设 `FISH_SIDECAR_WARM_JIEBA=1`，在插件宿主启动时加载并共享知识库的默认分词字典。默认值为 0。开启后插件宿主会常驻字典，适合有多个插件使用知识库的节点。
- Linux 节点可在 `deploy/.env` 里设 `FISH_SIDECAR_WARM_FAISS=1`，由插件宿主共享镜像内置的 FAISS 导入。默认值为 0。宿主预加载时将 OpenBLAS 初始化为单线程，子进程继承这份库。启用前可验证插件要求的 FAISS 版本和大索引性能。预加载失败或出现额外线程时，宿主会重新启动并改为按需导入。

## 本地开发

在 macOS 或 Linux 上可以不用 Docker 直接运行:

```sh
cd node
cargo run -- --dev
```

`--dev` 会使用内置的开发密钥，关闭沙箱和 cgroup，数据写到 `node/data`。插件宿主默认从 `../sidecar` 加载，需要本机有 python3 和 uv。如果环境变量 `CC` 指向了不存在的编译器，可以用 `CC=cc CXX=c++ cargo run -- --dev` 运行。
