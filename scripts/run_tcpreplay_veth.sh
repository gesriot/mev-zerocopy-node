#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "Usage: $0 <path-to-pcap>"
  exit 1
fi

PCAP_FILE="$1"
if [[ ! -f "${PCAP_FILE}" ]]; then
  echo "PCAP file not found: ${PCAP_FILE}"
  exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
"${SCRIPT_DIR}/setup_veth_xdp.sh"

echo "==> Starting node in AF_XDP mode (with TAP fallback)"
MEV_BACKEND=af_xdp RUST_LOG=info cargo run --release &
NODE_PID=$!
trap 'kill ${NODE_PID} 2>/dev/null || true' EXIT

sleep 2
echo "==> Replaying PCAP on veth-host0"
sudo tcpreplay --intf1=veth-host0 "${PCAP_FILE}"
echo "==> Replay complete"
