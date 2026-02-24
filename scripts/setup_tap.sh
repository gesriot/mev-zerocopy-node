#!/usr/bin/env bash
# Creates a Linux TAP interface for the Userspace Network Stack
set -euo pipefail

TAP_NAME="tap0"
HOST_IP="192.168.69.1" # The OS IP
# The bot will take 192.168.69.2 inside smoltcp

echo "==> Setting up TUN/TAP interface: $TAP_NAME"
sudo ip tuntap add name $TAP_NAME mode tap user $USER || true
sudo ip link set $TAP_NAME up
sudo ip addr add $HOST_IP/24 dev $TAP_NAME || true

echo "==> Interface $TAP_NAME is ready."
echo "You can ping the bot at 192.168.69.2 once it starts."
