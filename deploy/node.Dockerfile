# syntax=docker/dockerfile:1.7
# Build context: repository root.
#
# Optional build args for hosts with slow access to the default registries:
#   UV_IMAGE        image that provides /uv
#   RUST_IMAGE      Rust toolchain image for the compile stage, for example a Docker Hub mirror
#   CARGO_REGISTRY  sparse crates.io mirror, for example sparse+https://rsproxy.cn/index/
#   APT_MIRROR      replaces http://deb.debian.org, for example http://mirrors.tencent.com
#   PIP_INDEX       Python package index used while building
#
# NODE_BIN selects where the fish-node binary comes from:
#   build     compiles node/ in this build
#   prebuilt  copies deploy/dist/fish-node, produced by scripts/deploy-node.sh
ARG UV_IMAGE=ghcr.io/astral-sh/uv:0.8.0
ARG RUST_IMAGE=rust:1.94-bookworm
ARG PYTHON_IMAGE=python:3.13-slim-trixie
ARG NODE_BIN=build

# Compiles on the build machine for the target platform, so an arm64 machine can produce the amd64 binary.
FROM --platform=$BUILDPLATFORM ${RUST_IMAGE} AS bin-build
ARG TARGETARCH
ARG BUILDARCH
ARG CARGO_REGISTRY=
ARG APT_MIRROR=
WORKDIR /src/node
RUN case "$TARGETARCH" in \
        amd64) echo x86_64 > /arch ;; \
        arm64) echo aarch64 > /arch ;; \
        *) echo "unsupported TARGETARCH $TARGETARCH" >&2; exit 1 ;; \
    esac \
    && if [ "$TARGETARCH" != "$BUILDARCH" ]; then \
        arch=$(cat /arch); \
        if [ -n "$APT_MIRROR" ]; then \
            sed -i "s#http://deb.debian.org#$APT_MIRROR#g" /etc/apt/sources.list.d/debian.sources; \
        fi; \
        apt-get update \
        && apt-get install -y --no-install-recommends "gcc-$(echo "$arch" | tr _ -)-linux-gnu" "libc6-dev-$TARGETARCH-cross" \
        && rm -rf /var/lib/apt/lists/* \
        && rustup target add "$arch-unknown-linux-gnu"; \
    fi
COPY node/Cargo.toml node/Cargo.lock node/rustfmt.toml ./
COPY node/src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/node/target,id=fish-node-target-$TARGETARCH \
    arch=$(cat /arch) \
    && target="$arch-unknown-linux-gnu" \
    && if [ -n "$CARGO_REGISTRY" ]; then \
        printf '[source.crates-io]\nreplace-with = "mirror"\n[source.mirror]\nregistry = "%s"\n' "$CARGO_REGISTRY" > "$CARGO_HOME/config.toml"; \
    fi \
    && if [ "$TARGETARCH" != "$BUILDARCH" ]; then \
        upper=$(echo "$target" | tr a-z- A-Z_); \
        export "CARGO_TARGET_${upper}_LINKER=$arch-linux-gnu-gcc" "CC_$(echo "$target" | tr - _)=$arch-linux-gnu-gcc"; \
    fi \
    && cargo build --release --locked --target "$target" \
    && install -m 0755 "target/$target/release/fish-node" /usr/local/bin/fish-node

FROM scratch AS bin-prebuilt
COPY deploy/dist/fish-node /usr/local/bin/fish-node

FROM bin-${NODE_BIN} AS bin

FROM scratch AS bin-export
COPY --from=bin /usr/local/bin/fish-node /fish-node

FROM ${UV_IMAGE} AS uv

FROM ${PYTHON_IMAGE}
ARG APT_MIRROR=
ARG PIP_INDEX=
RUN if [ -n "$APT_MIRROR" ]; then \
        sed -i "s#http://deb.debian.org#$APT_MIRROR#g" /etc/apt/sources.list.d/debian.sources; \
    fi \
    && apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates bubblewrap tini \
    && rm -rf /var/lib/apt/lists/*
COPY --from=uv /uv /usr/local/bin/uv
RUN useradd --system --uid 10001 --user-group --no-create-home --shell /usr/sbin/nologin fishplug

COPY sidecar/pyproject.toml /app/sidecar/pyproject.toml
RUN if [ -n "$PIP_INDEX" ]; then export UV_DEFAULT_INDEX="$PIP_INDEX"; fi \
    && UV_PYTHON_DOWNLOADS=never uv pip install --system --break-system-packages --no-cache \
    --compile-bytecode --python /usr/local/bin/python3 -r /app/sidecar/pyproject.toml
# The sandbox root is read only, so bytecode caches must exist in the image.
# The base image ships the standard library without them.
RUN python3 -m compileall -q -j 0 "$(python3 -c 'import sysconfig; print(sysconfig.get_path("stdlib"))')" > /dev/null || true
COPY sidecar/fish_sidecar /app/sidecar/fish_sidecar
COPY sidecar/astrbot /app/sidecar/astrbot
RUN python3 -m compileall -q -j 0 /app/sidecar

COPY --from=bin /usr/local/bin/fish-node /usr/local/bin/fish-node
COPY deploy/node-entrypoint.sh /usr/local/bin/fish-entrypoint
RUN chmod 0755 /usr/local/bin/fish-entrypoint

ENV FISH_LISTEN=0.0.0.0:8700 \
    FISH_DATA_DIR=/data \
    FISH_SIDECAR_DIR=/app/sidecar \
    FISH_SIDECAR_PYTHON=/usr/local/bin/python3 \
    FISH_UV=/usr/local/bin/uv \
    FISH_PLUGIN_UID=10001 \
    FISH_PLUGIN_GID=10001 \
    RUST_LOG=info

VOLUME /data
EXPOSE 8700
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/fish-entrypoint"]
