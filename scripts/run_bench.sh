#!/usr/bin/env bash
set -euo pipefail

echo "==> Running criterion benchmarks..."
cargo bench

echo ""
echo "Compare 'serde_bincode_deserialize' vs 'bytemuck_pointer_cast' in the Criterion report."
