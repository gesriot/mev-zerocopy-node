#!/usr/bin/env bash
set -euo pipefail

HOST_IF="veth-host0"
BOT_IF="veth-bot0"

echo "==> Creating veth pair: ${HOST_IF} <-> ${BOT_IF}"
sudo ip link add "${HOST_IF}" type veth peer name "${BOT_IF}" 2>/dev/null || true

echo "==> Bringing interfaces up"
sudo ip link set "${HOST_IF}" up
sudo ip link set "${BOT_IF}" up

echo "==> Assigning IPv4 addresses"
sudo ip addr add 10.0.69.1/24 dev "${HOST_IF}" 2>/dev/null || true
sudo ip addr add 10.0.69.2/24 dev "${BOT_IF}" 2>/dev/null || true

echo "==> Resetting XDP program state on ${HOST_IF} (clean baseline)"
sudo ip link set dev "${HOST_IF}" xdp off 2>/dev/null || true
sudo ip link set dev "${HOST_IF}" xdpgeneric off 2>/dev/null || true

echo "veth setup complete."
echo "Attach your XDP/eBPF program separately, then run AF_XDP userspace."
