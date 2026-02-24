#!/usr/bin/env bash
set -euo pipefail

echo "==> Recording perf profile (requires Linux perf permissions)"
perf record -F 999 -g -- cargo run --release

echo "==> Building textual report"
perf report --stdio > perf-report.txt
echo "Saved: perf-report.txt"
echo "Saved allocator probe:"
grep -E "malloc|free|__libc_malloc|__libc_free" perf-report.txt || true

echo "Tip: install cargo-flamegraph and run:"
echo "  cargo flamegraph --root --bin mev-zerocopy-node"
