# syntax=docker/dockerfile:1
# Cached Ubuntu 24.04 toolchain for Supercharge E2B binaries.
# Same glibc as sandbox/e2b-agent-template. Rebuild this image only when
# rustc or apt packages change; cargo compile then skips apt/rustup.
FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive
ENV RUSTUP_HOME=/opt/rustup
ENV CARGO_HOME=/opt/cargo
ENV PATH="/opt/cargo/bin:${PATH}"

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl git \
    build-essential pkg-config libssl-dev \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

RUN curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y -q --default-toolchain stable --profile minimal \
    && rustc --version && cargo --version && protoc --version

WORKDIR /work
