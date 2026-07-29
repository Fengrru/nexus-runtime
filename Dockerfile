# ── Builder Stage ────────────────────────────────────────────
FROM rust:1.80-slim-bookworm AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY Cargo.toml Cargo.lock ./

# Pre-build dependencies (layer caching)
COPY crates/nexus-core/Cargo.toml      crates/nexus-core/
COPY crates/nexus-event-store/Cargo.toml crates/nexus-event-store/
COPY crates/nexus-rpc/Cargo.toml       crates/nexus-rpc/
COPY crates/nexus-security/Cargo.toml  crates/nexus-security/
COPY crates/nexus-cli/Cargo.toml       crates/nexus-cli/
COPY crates/nexus-scheduler/Cargo.toml crates/nexus-scheduler/
COPY crates/nexus-coordinator/Cargo.toml crates/nexus-coordinator/
COPY crates/nexus-message-bus/Cargo.toml crates/nexus-message-bus/
COPY crates/nexus-temporal/Cargo.toml  crates/nexus-temporal/
COPY crates/nexus-metrics/Cargo.toml   crates/nexus-metrics/
COPY crates/phoenix-tests/Cargo.toml   crates/phoenix-tests/
COPY workers/rust-worker/Cargo.toml    workers/rust-worker/
COPY adapters/openclaw/Cargo.toml      adapters/openclaw/
COPY adapters/hermes/Cargo.toml        adapters/hermes/
COPY sdk/rust/Cargo.toml              sdk/rust/

RUN mkdir -p crates/nexus-core/src \
    crates/nexus-event-store/src \
    crates/nexus-rpc/src \
    crates/nexus-security/src \
    crates/nexus-cli/src \
    crates/nexus-scheduler/src \
    crates/nexus-coordinator/src \
    crates/nexus-message-bus/src \
    crates/nexus-temporal/src \
    crates/nexus-metrics/src \
    crates/phoenix-tests/src \
    workers/rust-worker/src \
    adapters/openclaw/src \
    adapters/hermes/src \
    sdk/rust/src \
    && echo 'fn main() {}' > crates/nexus-cli/src/main.rs \
    && echo '' > crates/nexus-core/src/lib.rs \
    && echo '' > crates/nexus-event-store/src/lib.rs \
    && echo '' > crates/nexus-rpc/src/lib.rs \
    && echo '' > crates/nexus-security/src/lib.rs \
    && echo '' > crates/nexus-scheduler/src/lib.rs \
    && echo '' > crates/nexus-coordinator/src/lib.rs \
    && echo '' > crates/nexus-message-bus/src/lib.rs \
    && echo '' > crates/nexus-temporal/src/lib.rs \
    && echo '' > crates/nexus-metrics/src/lib.rs \
    && echo '' > crates/phoenix-tests/src/lib.rs \
    && echo 'fn main() {}' > workers/rust-worker/src/main.rs \
    && echo '' > adapters/openclaw/src/lib.rs \
    && echo '' > adapters/hermes/src/lib.rs \
    && echo '' > sdk/rust/src/lib.rs

RUN cargo build --bin nexus --release 2>/dev/null || true

# Build the real thing
COPY . .
RUN touch crates/*/src/*.rs workers/*/src/*.rs adapters/*/src/*.rs sdk/*/src/*.rs
RUN cargo build --bin nexus --release

# ── Runtime Stage ────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    python3 python3-pip \
    nodejs npm \
    && rm -rf /var/lib/apt/lists/*

RUN useradd --create-home --shell /bin/bash nexus
USER nexus
WORKDIR /home/nexus

COPY --from=builder /app/target/release/nexus /usr/local/bin/nexus
COPY --from=builder /app/workers/ /home/nexus/workers/

ENV NEXUS_MODE=lite
ENV NEXUS_DB_PATH=/home/nexus/.nexus/events.db
ENV NEXUS_VAULT_PATH=/home/nexus/.nexus/vault

RUN mkdir -p /home/nexus/.nexus/vault

ENTRYPOINT ["nexus"]
CMD ["--help"]
