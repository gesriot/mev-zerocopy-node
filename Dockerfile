# ── Stage 1: builder ──────────────────────────────────────────────────────────
FROM rust:1.78-slim-bookworm AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libclang-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Cache dependencies separately from source
COPY Cargo.toml Cargo.lock ./
# Create a dummy main so cargo can build deps without the real source
RUN mkdir -p src benches && \
    echo 'fn main() {}' > src/main.rs && \
    echo 'pub mod payload; pub mod processor; pub mod ring; pub mod runtime; pub mod affinity; pub mod xdp; pub mod validator;' > src/lib.rs && \
    touch benches/zero_copy_bench.rs && \
    cargo build --release 2>/dev/null || true && \
    rm -rf src benches

# Now copy the real source and build
COPY src ./src
COPY benches ./benches
RUN cargo build --release

# ── Stage 2: runtime ──────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

# iproute2: ip tuntap / ip link commands
# tcpreplay: traffic replay
# python3: gen_traffic.py pcap generator
RUN apt-get update && apt-get install -y --no-install-recommends \
    iproute2 \
    tcpreplay \
    python3 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/mev-zerocopy-node ./mev-zerocopy-node
COPY scripts/ ./scripts/
COPY traffic/ ./traffic/

# The node needs NET_ADMIN to create tap0 (granted via docker-compose cap_add)
ENTRYPOINT ["/app/scripts/docker-entrypoint.sh"]
