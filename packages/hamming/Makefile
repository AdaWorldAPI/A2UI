# A2UI Hamming Protocol — Build system
#
# Targets:
#   make all        — Build everything (Rust + WASM + TypeScript + Extension)
#   make rust       — Build Rust server binary
#   make wasm       — Build WASM client
#   make typescript — Build TypeScript wrapper
#   make extension  — Build Chrome extension
#   make pwa        — Build PWA app
#   make docker     — Build Docker image
#   make docker-up  — Start with docker compose (WSL2 / local dev)
#   make test       — Run all tests
#   make clean      — Clean all build artifacts

.PHONY: all rust wasm typescript extension pwa docker docker-up test clean

RUST_DIR  := rust
TS_DIR    := typescript
EXT_DIR   := extension
PWA_DIR   := pwa
PROTO_DIR := ../../proto/a2ui

all: rust wasm typescript extension pwa

# ── Rust server ───────────────────────────────────────────────

rust:
	cd $(RUST_DIR) && cargo build --release --features grpc,simd

rust-debug:
	cd $(RUST_DIR) && cargo build --features grpc,simd

# ── WASM client ───────────────────────────────────────────────

wasm:
	cd $(RUST_DIR) && wasm-pack build \
		--target web \
		--features wasm \
		--no-default-features \
		--out-dir ../wasm-out
	# Copy WASM to extension and PWA
	mkdir -p $(EXT_DIR)/wasm $(PWA_DIR)/wasm
	cp wasm-out/a2ui_hamming_bg.wasm $(EXT_DIR)/wasm/
	cp wasm-out/a2ui_hamming.js $(EXT_DIR)/wasm/
	cp wasm-out/a2ui_hamming_bg.wasm $(PWA_DIR)/wasm/
	cp wasm-out/a2ui_hamming.js $(PWA_DIR)/wasm/

# ── TypeScript ────────────────────────────────────────────────

typescript:
	cd $(TS_DIR) && npm install && npx tsc

# ── Chrome Extension ──────────────────────────────────────────

extension: typescript
	cd $(EXT_DIR) && npm install && npx tsc

# ── PWA ───────────────────────────────────────────────────────

pwa: typescript
	cd $(PWA_DIR) && npx -y typescript@5.8 tsc

# ── Docker ────────────────────────────────────────────────────

docker:
	docker build -t a2ui-hamming .

docker-up:
	docker compose up --build

docker-down:
	docker compose down

# ── Tests ─────────────────────────────────────────────────────

test: test-rust test-typescript

test-rust:
	cd $(RUST_DIR) && cargo test --all-features

test-typescript:
	cd $(TS_DIR) && npm test 2>/dev/null || true

# ── Benchmarks ────────────────────────────────────────────────

bench:
	cd $(RUST_DIR) && cargo bench --features simd

# ── Clean ─────────────────────────────────────────────────────

clean:
	cd $(RUST_DIR) && cargo clean
	rm -rf $(TS_DIR)/dist $(TS_DIR)/node_modules
	rm -rf $(EXT_DIR)/dist $(EXT_DIR)/node_modules
	rm -rf $(PWA_DIR)/dist
	rm -rf wasm-out
	rm -rf $(EXT_DIR)/wasm $(PWA_DIR)/wasm
