FROM rust:1.94-bookworm AS builder

RUN apt-get update \
    && apt-get install -y --no-install-recommends protobuf-compiler pkg-config clang cmake \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY . .

RUN CARGO_BUILD_JOBS=2 cargo build --release \
    -p xai-grok-superagent \
    --features e2b \
    --bin xai-grok-superagent-server

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --uid 10001 superagent

COPY --from=builder /src/target/release/xai-grok-superagent-server /usr/local/bin/xai-grok-superagent-server

USER superagent
WORKDIR /home/superagent
ENV SUPERAGENT_BIND=0.0.0.0:8787
EXPOSE 8787

ENTRYPOINT ["/usr/local/bin/xai-grok-superagent-server"]
