# wshm daemon — multi-stage build for minimal image
# Build: docker build -t wshm .
# Run:   docker run -e GITHUB_TOKEN -e ANTHROPIC_API_KEY wshm daemon --poll --no-server --apply

# ── Build stage ───────────────────────────────────────────────
FROM rust:1.86-bookworm AS builder

WORKDIR /build

# Cache dependencies: copy manifests first
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs && \
    cargo build --release 2>/dev/null || true && \
    rm -rf src

# Build the actual binary
COPY src/ src/
RUN cargo build --release --bin wshm && \
    strip target/release/wshm

# ── Runtime stage ─────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    git \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd -m -s /bin/bash wshm

COPY --from=builder /build/target/release/wshm /usr/local/bin/wshm

# Git config for automated commits
RUN git config --system user.name "wshm[bot]" && \
    git config --system user.email "wshm[bot]@users.noreply.github.com"

USER wshm
WORKDIR /home/wshm

# Data directory for repos and state
RUN mkdir -p /home/wshm/.wshm

EXPOSE 3000

ENTRYPOINT ["wshm"]
CMD ["daemon", "--poll", "--no-server", "--apply"]
