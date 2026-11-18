# syntax=docker/dockerfile:1.27

ARG RUST_VERSION=1.99.1

FROM --platform=$BUILDPLATFORM rust:${RUST_VERSION}-bookworm@sha256:e6f2cff35d0f309acf8433b4f05878ed77092602c5065cc81801177da0bb3a9c AS build

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

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked --target "$(cat /rust-target)" && \
    install -D "target/$(cat /rust-target)/release/scryarr" /out/scryarr

FROM gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f AS runtime

COPY --from=build /out/scryarr /scryarr

ENV SCRYARR_DATA_DIR=/data
VOLUME ["/data"]
USER 1000:1000
EXPOSE 8080
ENTRYPOINT ["/scryarr"]
