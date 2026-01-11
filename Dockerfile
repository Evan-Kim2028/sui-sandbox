FROM rust:bookworm AS rust_builder

WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    ca-certificates \
    git \
  && rm -rf /var/lib/apt/lists/*

# 1. Layer Optimization: Pre-build Rust dependencies
# This caches the slow compilation of heavy Sui dependencies.
COPY Cargo.toml Cargo.lock ./
# Copy bundled framework bytecode (required for include_bytes! at compile time)
# Version must match SUI_VERSION in runtime stage below
COPY framework_bytecode ./framework_bytecode
RUN mkdir -p src/bin && \
    touch src/lib.rs && \
    echo "fn main() {}" > src/main.rs && \
    echo "fn main() {}" > src/bin/smi_tx_sim.rs && \
    cargo build --release --locked

# 2. Build actual source
COPY src ./src
# We touch the files to ensure cargo recognizes they've changed from the dummy versions
RUN touch src/lib.rs src/main.rs src/bin/smi_tx_sim.rs && \
    cargo build --release --locked

FROM python:3.11-slim-trixie AS runtime

WORKDIR /app

# Install runtime dependencies including tini for proper PID 1 signal handling
# and curl for health checks
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    git \
    unzip \
    tini \
    libc6 \
  && rm -rf /var/lib/apt/lists/*

# Install Sui CLI for deterministic Move builds in Docker.
# IMPORTANT: This version MUST match the tag in Cargo.toml's sui-* dependencies!
# When updating: change BOTH this ARG AND Cargo.toml's `tag = "mainnet-vX.XX.X"` values.
# Use mainnet releases for production evaluation (not testnet/devnet).
ARG SUI_VERSION=mainnet-v1.62.1
RUN set -eux; \
    ARCH="$(uname -m)"; \
    if [ "$ARCH" = "aarch64" ] || [ "$ARCH" = "arm64" ]; then PLATFORM="ubuntu-aarch64"; else PLATFORM="ubuntu-x86_64"; fi; \
    URL="https://github.com/MystenLabs/sui/releases/download/${SUI_VERSION}/sui-${SUI_VERSION}-${PLATFORM}.tgz"; \
    curl -L "$URL" -o /tmp/sui.tgz; \
    test "$(stat -c%s /tmp/sui.tgz 2>/dev/null || stat -f%z /tmp/sui.tgz)" -gt 1000000; \
    tar -xzf /tmp/sui.tgz -C /usr/local/bin; \
    # Find the extracted `sui` binary and place it on PATH.
    SUI_PATH="$(find /usr/local/bin -maxdepth 3 -type f -name sui | head -n 1)"; \
    test -n "$SUI_PATH"; \
    chmod +x "$SUI_PATH"; \
    if [ "$SUI_PATH" != "/usr/local/bin/sui" ]; then ln -sf "$SUI_PATH" /usr/local/bin/sui; fi; \
    rm -f /tmp/sui.tgz; \
    /usr/local/bin/sui --version

RUN python -m pip install --no-cache-dir uv==0.8.15

# 3. Copy binaries from builder (Highly Cached)
COPY --from=rust_builder /app/target/release/sui_move_interface_extractor /usr/local/bin/sui_move_interface_extractor
COPY --from=rust_builder /app/target/release/smi_tx_sim /usr/local/bin/smi_tx_sim

# 4. Layer Optimization: Install Python dependencies first
COPY benchmark/pyproject.toml benchmark/uv.lock ./benchmark/
RUN cd /app/benchmark && uv sync --frozen --no-dev --no-install-project

# 5. Copy static assets and docs
COPY scripts ./scripts
COPY docs ./docs
COPY README.md ./README.md
COPY benchmark/README.md ./benchmark/README.md
COPY benchmark/scripts ./benchmark/scripts

# 6. Copy Python source (Frequent Changes)
# Changing a file here will only trigger the steps below.
COPY benchmark/src ./benchmark/src

# Ensure the project is synced
RUN cd /app/benchmark && uv sync --frozen --no-dev

# Create non-root user for security (UID 1000 matches common host user)
RUN useradd -m -u 1000 -s /bin/bash smi && \
    mkdir -p /app/results /app/logs /tmp/smi_bench && \
    chown -R smi:smi /app /tmp/smi_bench

ENV PATH="/app/benchmark/.venv/bin:${PATH}" \
    SMI_RUST_BIN="/usr/local/bin/sui_move_interface_extractor" \
    SMI_TX_SIM_BIN="/usr/local/bin/smi_tx_sim" \
    SMI_TEMP_DIR="/tmp/smi_bench"

# Switch to non-root user
USER smi

HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:9999/health || exit 1

# Use SIGINT for graceful uvicorn shutdown
STOPSIGNAL SIGINT

# Use tini as PID 1 to properly handle signals and reap zombies
ENTRYPOINT ["/usr/bin/tini", "--", "smi-a2a-green"]