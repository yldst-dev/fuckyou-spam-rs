FROM rust:1.85-slim AS builder

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src \
    && echo 'fn main() {}' > src/main.rs \
    && cargo build --release \
    && rm -rf src target/release/deps/fuckyou_spam_rust* target/release/fuckyou-spam-rust

COPY src ./src
RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN groupadd -r -g 10001 botuser \
    && useradd -r -u 10001 -g botuser -s /usr/sbin/nologin botuser

WORKDIR /app

COPY --from=builder /app/target/release/fuckyou-spam-rust ./

RUN mkdir -p data logs \
    && chown -R botuser:botuser /app \
    && chmod 700 data logs

USER botuser

CMD ["./fuckyou-spam-rust"]
