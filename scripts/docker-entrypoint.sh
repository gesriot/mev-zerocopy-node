#!/usr/bin/env bash
# docker-entrypoint.sh — set up tap0 and start the MEV node inside the container.
set -euo pipefail

TAP_NAME="tap0"
HOST_IP="192.168.69.1"

echo "[entrypoint] Creating TAP interface ${TAP_NAME}"
ip tuntap add name "${TAP_NAME}" mode tap || true
ip link set "${TAP_NAME}" up
ip addr add "${HOST_IP}/24" dev "${TAP_NAME}" || true
echo "[entrypoint] Interface ${TAP_NAME} ready (host=${HOST_IP}, node=192.168.69.2)"

echo "[entrypoint] Starting mev-zerocopy-node (backend=${MEV_BACKEND:-tap})"
exec env RUST_LOG="${RUST_LOG:-info}" \
         MEV_BACKEND="${MEV_BACKEND:-tap}" \
    /app/mev-zerocopy-node
