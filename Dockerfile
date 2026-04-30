# syntax=docker/dockerfile:1.7

# ────────────────────────────────────────────────────────────────
# Stage 1 — builder
# ────────────────────────────────────────────────────────────────
FROM rust:1-bookworm AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
      pkg-config cmake ca-certificates curl unzip \
    && rm -rf /var/lib/apt/lists/*

RUN curl -fsSL https://bun.sh/install | bash && ln -s /root/.bun/bin/bun /usr/local/bin/bun

WORKDIR /build
COPY . .
RUN cd web && bun install --frozen-lockfile && bun run build
RUN cargo build --release --bin wshm && strip target/release/wshm

# ────────────────────────────────────────────────────────────────
# Stage 2 — runtime
# ────────────────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates git \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --home-dir /home/wshm --shell /usr/sbin/nologin wshm

COPY --from=builder /build/target/release/wshm /usr/local/bin/wshm

USER wshm
WORKDIR /home/wshm

ENV WSHM_HOME=/home/wshm/.wshm
EXPOSE 3000

ENTRYPOINT ["/usr/local/bin/wshm"]
CMD ["daemon"]
