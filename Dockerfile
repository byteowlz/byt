# ghcr.io/byteowlz/byt — release builder image (ADR-0022).
#
# Bundles byt + the cross-compile toolchain (cargo-zigbuild/zig for Rust,
# go for Go, gh for publishing) so a CI runner needs nothing else: the reusable
# release workflow runs its job `container:`'d in this image and calls
# `byt release`. Built from source once (the bootstrap); thereafter byt's own
# releases self-host on the previous image.
#
# rust must be >= 1.88 (let-chains used in src/release.rs) and edition 2024 (>=1.85).

# ---- stage 1: build byt from source ----
FROM rust:1.90-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --bin byt \
    && install -Dm755 target/release/byt /out/byt

# ---- stage 2: byt + toolchains ----
FROM rust:1.90-bookworm
ARG ZIG_VERSION=0.13.0
ARG GO_VERSION=1.23.4

# OS deps + GitHub CLI (for `byt release publish` -> `gh release create`)
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl git tar xz-utils \
    && curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
        -o /usr/share/keyrings/githubcli-archive-keyring.gpg \
    && echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" \
        > /etc/apt/sources.list.d/github-cli.list \
    && apt-get update \
    && apt-get install -y --no-install-recommends gh \
    && rm -rf /var/lib/apt/lists/*

# zig (backend for cargo-zigbuild cross-compilation)
RUN curl -fsSL "https://ziglang.org/download/${ZIG_VERSION}/zig-linux-x86_64-${ZIG_VERSION}.tar.xz" \
        | tar -xJ -C /opt \
    && ln -s "/opt/zig-linux-x86_64-${ZIG_VERSION}/zig" /usr/local/bin/zig

# cargo-zigbuild + the required cross std libs (ADR-0021 required targets)
RUN cargo install cargo-zigbuild --locked \
    && rustup target add x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu

# Go toolchain (for Go apps: sx, scrpr)
RUN curl -fsSL "https://go.dev/dl/go${GO_VERSION}.linux-amd64.tar.gz" \
        | tar -xz -C /usr/local \
    && ln -s /usr/local/go/bin/go /usr/local/bin/go

# byt itself
COPY --from=build /out/byt /usr/local/bin/byt

CMD ["byt", "--help"]
