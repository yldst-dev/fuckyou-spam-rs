# Multi-stage build for Rust application
FROM rust:1.85-slim as builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    libsqlite3-dev \
    && rm -rf /var/lib/apt/lists/*

# Set working directory
WORKDIR /app

# Copy manifest and sources
COPY Cargo.toml Cargo.lock ./
COPY src ./src

# Build the application
RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libsqlite3-0 \
    && rm -rf /var/lib/apt/lists/*

# Create app user
RUN groupadd -r botuser && useradd -r -g botuser botuser

# Set working directory
WORKDIR /app

# Copy binary from builder stage
COPY --from=builder /app/target/release/fuckyou-spam-rust ./

# Create necessary directories
RUN mkdir -p data logs && \
    chown -R botuser:botuser /app

# Switch to non-root user
USER botuser

# Run the application
CMD ["./fuckyou-spam-rust"]
