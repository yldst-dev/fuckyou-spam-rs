FROM rust:1.94-slim-bookworm@sha256:cf9dd0ec73e75f827fe59123fff9dc65af1a1c8363c3c31ee8d7f8ad0b6a5fb2 AS builder

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
RUN --mount=type=cache,id=fuckyou-spam-rust-cargo-registry-v1,target=/usr/local/cargo/registry,sharing=locked \
    mkdir -p src \
    && echo 'fn main() {}' > src/main.rs \
    && cargo build --release --locked \
    && rm -rf src target/release/deps/fuckyou_spam_rust* target/release/fuckyou-spam-rust

COPY src ./src
COPY migrations ./migrations
RUN --mount=type=cache,id=fuckyou-spam-rust-cargo-registry-v1,target=/usr/local/cargo/registry,sharing=locked \
    cargo build --release --locked \
    && mkdir -p /out \
    && cp target/release/fuckyou-spam-rust /out/fuckyou-spam-rust \
    && strip /out/fuckyou-spam-rust

FROM debian:bookworm-slim@sha256:7b140f374b289a7c2befc338f42ebe6441b7ea838a042bbd5acbfca6ec875818

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN groupadd -r -g 10001 botuser \
    && useradd -r -u 10001 -g botuser -s /usr/sbin/nologin botuser

WORKDIR /app

COPY --from=builder /out/fuckyou-spam-rust ./

RUN mkdir -p data logs \
    && chown -R botuser:botuser /app \
    && chmod 700 data logs

USER botuser

STOPSIGNAL SIGTERM

CMD ["./fuckyou-spam-rust"]
