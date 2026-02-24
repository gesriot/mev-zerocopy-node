#!/usr/bin/env bash
# traffic-entrypoint.sh — generate pcap and replay it against the node.
set -euo pipefail

PCAP_FILE="${PCAP_FILE:-/app/traffic/mock_tx.pcap}"
PCAP_COUNT="${PCAP_COUNT:-200}"
PCAP_MODE="${PCAP_MODE:-mixed}"
PCAP_SEED="${PCAP_SEED:-42}"
REPLAY_MBPS="${REPLAY_MBPS:-10}"
REPLAY_IFACE="${REPLAY_IFACE:-tap0}"

echo "[traffic] Generating ${PCAP_COUNT} '${PCAP_MODE}' packets -> ${PCAP_FILE}"
python3 /app/scripts/gen_traffic.py \
    --count "${PCAP_COUNT}" \
    --mode  "${PCAP_MODE}" \
    --out   "${PCAP_FILE}" \
    --seed  "${PCAP_SEED}"

echo "[traffic] Waiting for interface ${REPLAY_IFACE} to appear..."
for i in $(seq 1 20); do
    if ip link show "${REPLAY_IFACE}" &>/dev/null; then
        echo "[traffic] Interface ${REPLAY_IFACE} is up."
        break
    fi
    sleep 0.5
done

echo "[traffic] Replaying ${PCAP_FILE} on ${REPLAY_IFACE} at ${REPLAY_MBPS} Mbps"
tcpreplay \
    --intf1="${REPLAY_IFACE}" \
    --mbps="${REPLAY_MBPS}" \
    "${PCAP_FILE}"

echo "[traffic] Done. Check node logs for latency metrics."
