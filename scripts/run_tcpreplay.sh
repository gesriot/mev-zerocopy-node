#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "Usage: $0 <path-to-pcap>"
  exit 1
fi

PCAP_FILE="$1"
if [[ ! -f "$PCAP_FILE" ]]; then
  echo "PCAP file not found: $PCAP_FILE"
  exit 1
fi

echo "==> Preparing TAP interface"
"$(dirname "$0")/setup_tap.sh"

echo "==> Starting node"
RUST_LOG=info cargo run --release &
NODE_PID=$!
trap 'kill $NODE_PID 2>/dev/null || true' EXIT

sleep 2
echo "==> Replaying traffic with tcpreplay on tap0"
sudo tcpreplay --intf1=tap0 "$PCAP_FILE"

echo "==> Replay complete"
