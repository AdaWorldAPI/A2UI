# A2UI Hamming Protocol — Multi-stage build
#
# Builds:
#   1. Rust gRPC server (native binary)
#   2. Rust WASM client (for browser)
#   3. TypeScript extension + PWA (bundled)
#
# Targets:
#   - Railway (PORT env var, single container)
#   - WSL2 (local development, docker compose)
#
# Usage:
#   docker build -t a2ui-hamming .
#   docker run -p 8090:8090 -p 8091:8091 a2ui-hamming
#
#   Railway: auto-detects via PORT env var

# ── Stage 1: Rust builder ─────────────────────────────────────
FROM rust:1.82-bookworm AS rust-builder

# Install wasm-pack and protobuf compiler
RUN cargo install wasm-pack && \
    apt-get update && \
    apt-get install -y --no-install-recommends protobuf-compiler && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Copy Rust sources
COPY rust/Cargo.toml rust/build.rs ./
COPY rust/src/ ./src/

# Copy proto files
COPY ../proto/ /proto/

# Build native server binary
RUN cargo build --release --features grpc,simd

# Build WASM client
RUN wasm-pack build --target web --features wasm --no-default-features \
    --out-dir /build/wasm-out \
    -- --lib

# ── Stage 2: TypeScript builder ───────────────────────────────
FROM node:22-bookworm-slim AS ts-builder

WORKDIR /build

# Copy TypeScript sources
COPY typescript/package.json typescript/tsconfig.json ./typescript/
COPY typescript/src/ ./typescript/src/

# Copy extension sources
COPY extension/ ./extension/

# Copy PWA sources
COPY pwa/ ./pwa/

# Install and build TypeScript wrapper
WORKDIR /build/typescript
RUN npm install && npx tsc

# Build extension TypeScript
WORKDIR /build/extension
RUN npm install 2>/dev/null || true
# Compile extension TS files (using same tsconfig pattern)
RUN npx -y typescript@5.8 tsc \
    --target ES2022 \
    --module ESNext \
    --moduleResolution bundler \
    --outDir dist \
    --rootDir src \
    --strict \
    --esModuleInterop \
    --skipLibCheck \
    --lib ES2022,DOM \
    src/**/*.ts 2>/dev/null || true

# ── Stage 3: Runtime ──────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        ca-certificates \
        tini \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN groupadd -r a2ui && useradd -r -g a2ui -d /app a2ui

WORKDIR /app

# Copy Rust server binary
COPY --from=rust-builder /build/target/release/a2ui-hamming ./server

# Copy WASM build artifacts
COPY --from=rust-builder /build/wasm-out/ ./static/wasm/

# Copy TypeScript build artifacts
COPY --from=ts-builder /build/typescript/dist/ ./static/typescript/

# Copy extension static files
COPY --from=ts-builder /build/extension/manifest.json ./static/extension/
COPY --from=ts-builder /build/extension/side-panel/ ./static/extension/side-panel/
COPY --from=ts-builder /build/extension/popup/ ./static/extension/popup/
COPY --from=ts-builder /build/extension/offscreen/ ./static/extension/offscreen/
COPY --from=ts-builder /build/extension/content/ ./static/extension/content/
COPY --from=ts-builder /build/extension/dist/ ./static/extension/dist/

# Copy PWA static files
COPY --from=ts-builder /build/pwa/index.html ./static/pwa/
COPY --from=ts-builder /build/pwa/manifest.json ./static/pwa/

# Copy entrypoint
COPY docker-entrypoint.sh ./

RUN chmod +x ./server ./docker-entrypoint.sh && \
    chown -R a2ui:a2ui /app

USER a2ui

# gRPC server port
EXPOSE 8090
# HTTP server port (serves WASM, extension files, PWA)
EXPOSE 8091
# UDP Hamming frames port
EXPOSE 8092/udp

# Railway uses PORT env var
ENV PORT=8090
ENV HTTP_PORT=8091
ENV UDP_PORT=8092

# Health check (gRPC reflection or HTTP /health)
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -sf http://localhost:${HTTP_PORT}/health || exit 1

ENTRYPOINT ["tini", "--"]
CMD ["./docker-entrypoint.sh"]
