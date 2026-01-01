FROM rust:1.78-bookworm AS rust_builder

WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    ca-certificates \
    git \
  && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

FROM python:3.11-slim-bookworm AS runtime

WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    git \
  && rm -rf /var/lib/apt/lists/*

RUN python -m pip install --no-cache-dir uv==0.8.15

COPY --from=rust_builder /app/target/release/sui_move_interface_extractor /usr/local/bin/sui_move_interface_extractor

COPY scripts ./scripts
COPY docs ./docs
COPY README.md ./README.md
COPY benchmark/pyproject.toml benchmark/uv.lock ./benchmark/
COPY benchmark/src ./benchmark/src
COPY benchmark/README.md ./benchmark/README.md

RUN cd /app/benchmark && uv sync --frozen --no-dev

ENV PATH="/app/benchmark/.venv/bin:${PATH}"

CMD ["sui_move_interface_extractor", "--help"]
