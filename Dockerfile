# syntax=docker/dockerfile:1.29

ARG RUST_VERSION=1.100.0

FROM --platform=$BUILDPLATFORM rust:1.100.0-bookworm@sha256:1b9dc70890ec494e466c87f5ad41ad544bfb30ea6acc99a3e068c47a4ae3f23e AS build

ARG RUST_VERSION
ARG TARGETARCH

ENV RUSTUP_TOOLCHAIN=${RUST_VERSION}

RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt,sharing=locked \
    rm -f /etc/apt/apt.conf.d/docker-clean && \
    apt-get update && \
    apt-get install -y --no-install-recommends gcc-aarch64-linux-gnu libc6-dev-arm64-cross

RUN case "$TARGETARCH" in \
      amd64) echo x86_64-unknown-linux-gnu > /rust-target ;; \
      arm64) echo aarch64-unknown-linux-gnu > /rust-target ;; \
      *) echo "unsupported TARGETARCH: $TARGETARCH" >&2; exit 1 ;; \
    esac && \
    rustup target add "$(cat /rust-target)"

ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc

WORKDIR /src
COPY . .

RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/src/target,id=scryarr-target-${TARGETARCH} \
    cargo build --release --locked --target "$(cat /rust-target)" && \
    install -D "target/$(cat /rust-target)/release/scryarr" /out/scryarr && \
    mkdir -p /out/data

FROM gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f AS runtime

COPY --from=build /out/scryarr /scryarr
COPY --from=build --chown=1000:1000 /out/data /data

ENV SCRYARR_DATA_DIR=/data
VOLUME ["/data"]
USER 1000:1000
EXPOSE 8080
ENTRYPOINT ["/scryarr"]
