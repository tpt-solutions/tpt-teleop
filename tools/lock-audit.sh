#!/usr/bin/env bash
# Phase 14, item 3: verify the hot path uses no locking primitives.
#
# Scans every library `src/` tree (the compiled hot path) for mutex / rwlock
# / third-party locking crates. A zero-lock data plane is a hard requirement
# for the deterministic real-time loop, so any hit in `src/` is a hard failure.
# Lock usage inside `tests/`/`benches/` is reported separately (informational
# only) because those are not on the runtime critical path.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
LOCK_PATTERNS='Mutex|RwLock|parking_lot|lazy_static|once_cell|std::sync::Lock|spin::Mutex|crossbeam::|std::sync::Condvar'

# --- Hot path: crates/*/src ------------------------------------------------
hot_hits="$(grep -rEn "$LOCK_PATTERNS" "$ROOT"/crates/*/src 2>/dev/null || true)"

# --- Off-path: tests/benches (informational only) -------------------------
off_hits="$(grep -rEn "$LOCK_PATTERNS" "$ROOT"/crates/*/tests "$ROOT"/crates/*/benches 2>/dev/null || true)"

if [ -n "$off_hits" ]; then
    echo "INFO: locking primitives in tests/benches (not on the hot path):"
    echo "$off_hits" | sed 's/^/  /'
fi

if [ -z "$hot_hits" ]; then
    echo "PASS: no locking primitives in the hot-path library source."
    exit 0
else
    echo "FAIL: locking primitives found in hot-path source:"
    echo "$hot_hits" | sed 's/^/  /'
    exit 1
fi
