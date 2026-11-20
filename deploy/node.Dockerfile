# syntax=docker/dockerfile:1.7
# Build context: repository root.
#
# Optional build args for hosts with slow access to the default registries:
#   UV_IMAGE        image that provides /uv
#   CARGO_REGISTRY  sparse crates.io mirror, for example sparse+https://rsproxy.cn/index/
#   APT_MIRROR      replaces http://deb.debian.org, for example http://mirrors.tencent.com
#   PIP_INDEX       Python package index used while building
ARG UV_IMAGE=ghcr.io/astral-sh/uv:0.8.0

FROM rust:1.94-bookworm AS build
ARG CARGO_REGISTRY=
WORKDIR /src/node
COPY node/Cargo.toml node/Cargo.lock node/rustfmt.toml ./
COPY node/src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/node/target \
    if [ -n "$CARGO_REGISTRY" ]; then \
        printf '[source.crates-io]\nreplace-with = "mirror"\n[source.mirror]\nregistry = "%s"\n' "$CARGO_REGISTRY" > "$CARGO_HOME/config.toml"; \
    fi \
    && cargo build --release --locked \
    && install -m 0755 target/release/fish-node /usr/local/bin/fish-node

FROM ${UV_IMAGE} AS uv

FROM debian:bookworm-slim
ARG APT_MIRROR=
ARG PIP_INDEX=
RUN if [ -n "$APT_MIRROR" ]; then \
        sed -i "s#http://deb.debian.org#$APT_MIRROR#g" /etc/apt/sources.list.d/debian.sources; \
    fi \
    && apt-get update \
    && apt-get install -y --no-install-recommends python3 ca-certificates bubblewrap tini \
    && rm -rf /var/lib/apt/lists/*
COPY --from=uv /uv /usr/local/bin/uv
RUN useradd --system --uid 10001 --user-group --no-create-home --shell /usr/sbin/nologin fishplug

COPY sidecar/pyproject.toml /app/sidecar/pyproject.toml
RUN if [ -n "$PIP_INDEX" ]; then export UV_DEFAULT_INDEX="$PIP_INDEX"; fi \
    && UV_PYTHON_DOWNLOADS=never uv pip install --system --break-system-packages --no-cache \
    -r /app/sidecar/pyproject.toml
COPY sidecar/fish_sidecar /app/sidecar/fish_sidecar
COPY sidecar/astrbot /app/sidecar/astrbot

COPY --from=build /usr/local/bin/fish-node /usr/local/bin/fish-node
COPY deploy/node-entrypoint.sh /usr/local/bin/fish-entrypoint
RUN chmod 0755 /usr/local/bin/fish-entrypoint

ENV FISH_LISTEN=0.0.0.0:8700 \
    FISH_DATA_DIR=/data \
    FISH_SIDECAR_DIR=/app/sidecar \
    FISH_SIDECAR_PYTHON=/usr/bin/python3 \
    FISH_UV=/usr/local/bin/uv \
    FISH_PLUGIN_UID=10001 \
    FISH_PLUGIN_GID=10001 \
    RUST_LOG=info

VOLUME /data
EXPOSE 8700
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/fish-entrypoint"]
