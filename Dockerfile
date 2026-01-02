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

FROM python:3.11-slim-bookworm AS runtime

WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    git \
  && rm -rf /var/lib/apt/lists/*

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

# 6. Copy Python source (Frequent Changes)
# Changing a file here will only trigger the steps below.
COPY benchmark/src ./benchmark/src

# Ensure the project is synced
RUN cd /app/benchmark && uv sync --frozen --no-dev

ENV PATH="/app/benchmark/.venv/bin:${PATH}" \
    SMI_RUST_BIN="/usr/local/bin/sui_move_interface_extractor" \
    SMI_TX_SIM_BIN="/usr/local/bin/smi_tx_sim" \
    SMI_TEMP_DIR="/tmp/smi_bench"

ENTRYPOINT ["smi-a2a-green"]