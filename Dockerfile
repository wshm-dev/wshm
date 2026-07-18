# syntax=docker/dockerfile:1

# ────────────────────────────────────────────────────────────────
# Stage 1 — builder
# ────────────────────────────────────────────────────────────────
ARG ALPINE_VERSION=3.23
FROM rust:1-alpine$ALPINE_VERSION AS builder

COPY --from=oven/bun:1-alpine --chmod=a=rX /usr/local/bin/bun /usr/local/bin/

WORKDIR /build
COPY . .
RUN cd web && bun install --frozen-lockfile && bun run build
RUN apk add --no-cache musl-dev perl make ca-certificates
RUN cargo build --release --bin wshm && strip target/release/wshm

# ────────────────────────────────────────────────────────────────
# Stage 2 — runtime
# ────────────────────────────────────────────────────────────────
FROM alpine:$ALPINE_VERSION

# libgit2 is statically linked into the binary by the git2 crate, so the
# runtime image only needs CA certificates.
RUN apk add --no-cache ca-certificates \
    && adduser -S wshm -h /home/wshm -s /bin/false

COPY --from=builder /build/target/release/wshm /usr/local/bin/wshm

USER wshm
WORKDIR /home/wshm

ENV WSHM_HOME=/home/wshm/.wshm
EXPOSE 3000

ENTRYPOINT ["/usr/local/bin/wshm"]
CMD ["daemon"]
