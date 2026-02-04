#!/bin/sh
set -e

# A2UI Hamming — Docker entrypoint
#
# Starts the gRPC Hamming server and a lightweight HTTP server
# for serving WASM/extension/PWA static files.
#
# Environment variables:
#   PORT      - gRPC server port (default: 8090, Railway sets this)
#   HTTP_PORT - HTTP static file server port (default: 8091)
#   UDP_PORT  - UDP Hamming frame port (default: 8092)
#   LOG_LEVEL - Log level: debug, info, warn, error (default: info)
#   CODEBOOK  - Path to initial codebook file (default: standard)

GRPC_PORT="${PORT:-8090}"
HTTP_PORT="${HTTP_PORT:-8091}"
UDP_PORT="${UDP_PORT:-8092}"
LOG_LEVEL="${LOG_LEVEL:-info}"

echo "A2UI Hamming Server"
echo "  gRPC:  0.0.0.0:${GRPC_PORT}"
echo "  HTTP:  0.0.0.0:${HTTP_PORT}"
echo "  UDP:   0.0.0.0:${UDP_PORT}"
echo "  Log:   ${LOG_LEVEL}"
echo ""

# Start the Rust server (handles gRPC + UDP)
# The server binary reads these env vars directly
export A2UI_GRPC_PORT="${GRPC_PORT}"
export A2UI_UDP_PORT="${UDP_PORT}"
export A2UI_HTTP_PORT="${HTTP_PORT}"
export A2UI_STATIC_DIR="/app/static"
export A2UI_LOG_LEVEL="${LOG_LEVEL}"

if [ -n "${CODEBOOK}" ]; then
    export A2UI_CODEBOOK_PATH="${CODEBOOK}"
fi

exec ./server
