FROM rust:1.94-bookworm AS builder

RUN apt-get update \
    && apt-get install -y --no-install-recommends protobuf-compiler pkg-config clang cmake \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY . .

RUN CARGO_BUILD_JOBS=2 cargo build --release \
    -p supercharge-superagent \
    --features e2b \
    --bin supercharge-superagent-server \
    && CARGO_BUILD_JOBS=2 cargo build --release -p supercharge-pager-bin

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --uid 10001 superagent

COPY --from=builder /src/target/release/supercharge-superagent-server /usr/local/bin/supercharge-superagent-server
COPY --from=builder /src/target/release/supercharge-pager-bin /usr/local/bin/supercharge-pager-bin

USER superagent
WORKDIR /home/superagent
ENV SUPERAGENT_BIND=0.0.0.0:8787
ENV GROK_AGENT_BINARY=/usr/local/bin/supercharge-pager-bin
EXPOSE 8787

ENTRYPOINT ["/usr/local/bin/supercharge-superagent-server"]
