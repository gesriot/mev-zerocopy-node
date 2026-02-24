#!/usr/bin/env python3
"""
gen_traffic.py — Generate test .pcap files with DexSwapTx wire payloads.

Creates UDP packets containing DexSwapTx structs in little-endian binary
layout (matching the Rust #[repr(C)] definition) for use with tcpreplay.

DexSwapTx wire layout (48 bytes, little-endian):
  [0..8]   nonce_le        u64
  [8..28]  pool_address    [u8; 20]
  [28..36] amount_in_le    u64
  [36..44] min_amount_out_le u64
  [44]     token_direction u8
  [45..48] _reserved       [u8; 3]

Usage:
  # Install dependencies (scapy):
  pip install scapy

  # Generate traffic with defaults (100 packets, mixed amounts):
  python3 scripts/gen_traffic.py

  # Generate 500 large-swap packets for sandwich testing:
  python3 scripts/gen_traffic.py --count 500 --mode large --out traffic/large_swaps.pcap

  # Generate a mix of profitable and non-profitable swaps:
  python3 scripts/gen_traffic.py --count 200 --mode mixed --out traffic/mixed.pcap
"""

import argparse
import struct
import random
import sys

# ── DexSwapTx encoding ──────────────────────────────────────────────────────

WIRE_SIZE = 48  # must match Rust: core::mem::size_of::<DexSwapTx>()


def encode_dex_swap_tx(
    nonce: int,
    pool_address: bytes,
    amount_in: int,
    min_amount_out: int,
    token_direction: int,
) -> bytes:
    """Encode a DexSwapTx as 48 little-endian bytes (matches Rust #[repr(C)])."""
    assert len(pool_address) == 20, "pool_address must be 20 bytes"
    assert token_direction in (0, 1), "token_direction must be 0 or 1"
    return struct.pack(
        "<Q20sQQBxxx",  # little-endian: u64, 20s, u64, u64, u8, 3-pad
        nonce & 0xFFFF_FFFF_FFFF_FFFF,
        pool_address,
        amount_in & 0xFFFF_FFFF_FFFF_FFFF,
        min_amount_out & 0xFFFF_FFFF_FFFF_FFFF,
        token_direction,
    )


# ── Packet generation scenarios ─────────────────────────────────────────────

POOL_ADDR_ETH_USDC = bytes.fromhex("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"[:40].ljust(40, "0"))
POOL_ADDR_WBTC_ETH = bytes.fromhex("cBCdF9626bC03E24f779434178A73a0B4bad62eD"[:40].ljust(40, "0"))


def make_small_swap(nonce: int) -> bytes:
    """Below MIN_AMOUNT_IN threshold — should be rejected by process_packet."""
    return encode_dex_swap_tx(
        nonce=nonce,
        pool_address=POOL_ADDR_ETH_USDC,
        amount_in=random.randint(1, 999_999),
        min_amount_out=1,
        token_direction=random.randint(0, 1),
    )


def make_large_swap(nonce: int) -> bytes:
    """Large swap — profitable sandwich target."""
    amount_in = random.randint(10_000_000, 200_000_000)
    # min_amount_out set to 1% slippage (generous — will not revert)
    min_out = int(amount_in * 0.90)
    return encode_dex_swap_tx(
        nonce=nonce,
        pool_address=random.choice([POOL_ADDR_ETH_USDC, POOL_ADDR_WBTC_ETH]),
        amount_in=amount_in,
        min_amount_out=min_out,
        token_direction=random.randint(0, 1),
    )


def make_tight_slippage_swap(nonce: int) -> bytes:
    """Swap with very tight slippage — may be rejected after front-run moves price."""
    amount_in = random.randint(5_000_000, 50_000_000)
    # 99.9% min_out — will likely revert after our front-run
    min_out = int(amount_in * 0.999)
    return encode_dex_swap_tx(
        nonce=nonce,
        pool_address=POOL_ADDR_ETH_USDC,
        amount_in=amount_in,
        min_amount_out=min_out,
        token_direction=0,
    )


# ── pcap writing (manual, no scapy dependency) ───────────────────────────────

PCAP_GLOBAL_HEADER = struct.pack(
    "<IHHiIII",
    0xA1B2C3D4,  # magic number
    2,           # version major
    4,           # version minor
    0,           # thiszone
    0,           # sigfigs
    65535,       # snaplen
    1,           # network: LINKTYPE_ETHERNET
)

PCAP_RECORD_HEADER_FMT = "<IIII"  # ts_sec, ts_usec, incl_len, orig_len


def ethernet_ip_udp_frame(
    payload: bytes,
    src_ip: str = "192.168.69.1",
    dst_ip: str = "192.168.69.2",
    src_port: int = 54321,
    dst_port: int = 8080,
    src_mac: bytes = b"\x00\x11\x22\x33\x44\x55",
    dst_mac: bytes = b"\x02\x00\x00\x00\x00\x01",
) -> bytes:
    """Build a raw Ethernet frame containing an IPv4 UDP packet with payload."""
    import socket as _socket

    udp_len = 8 + len(payload)
    # UDP header (no checksum — set to 0, valid for IPv4 UDP)
    udp_hdr = struct.pack(">HHHH", src_port, dst_port, udp_len, 0)

    ip_src = _socket.inet_aton(src_ip)
    ip_dst = _socket.inet_aton(dst_ip)
    ip_len = 20 + udp_len
    # IPv4 header (no options, TTL=64, proto=17 UDP, checksum=0 for simplicity)
    ip_hdr = struct.pack(
        ">BBHHHBBH4s4s",
        0x45,    # version=4, IHL=5
        0,       # DSCP/ECN
        ip_len,  # total length
        random.randint(1, 65535),  # identification
        0x4000,  # flags: DF, fragment offset=0
        64,      # TTL
        17,      # protocol: UDP
        0,       # checksum (0 = let tcpreplay recalculate, or skip)
        ip_src,
        ip_dst,
    )

    # Ethernet frame: dst_mac(6) + src_mac(6) + ethertype(2) + ip_hdr + udp_hdr + payload
    eth_frame = dst_mac + src_mac + b"\x08\x00" + ip_hdr + udp_hdr + payload
    return eth_frame


def write_pcap(filename: str, frames: list, base_ts_sec: int = 1_700_000_000) -> None:
    """Write a list of raw Ethernet frames to a .pcap file."""
    with open(filename, "wb") as f:
        f.write(PCAP_GLOBAL_HEADER)
        for i, frame in enumerate(frames):
            ts_sec = base_ts_sec + i // 1000
            ts_usec = (i % 1000) * 1000  # 1ms between packets
            incl_len = len(frame)
            orig_len = incl_len
            f.write(struct.pack(PCAP_RECORD_HEADER_FMT, ts_sec, ts_usec, incl_len, orig_len))
            f.write(frame)


# ── Main ─────────────────────────────────────────────────────────────────────

def main() -> None:
    parser = argparse.ArgumentParser(description="Generate DexSwapTx pcap traffic for tcpreplay")
    parser.add_argument("--count", type=int, default=100, help="Number of packets to generate (default: 100)")
    parser.add_argument(
        "--mode",
        choices=["large", "small", "tight", "mixed"],
        default="mixed",
        help=(
            "Packet type: "
            "large=profitable sandwich targets, "
            "small=below threshold (rejected), "
            "tight=tight slippage (may revert), "
            "mixed=all types (default)"
        ),
    )
    parser.add_argument("--out", default="traffic/mock_tx.pcap", help="Output .pcap file path")
    parser.add_argument("--seed", type=int, default=42, help="Random seed for reproducibility")
    args = parser.parse_args()

    random.seed(args.seed)

    frames = []
    for i in range(args.count):
        if args.mode == "large":
            payload = make_large_swap(nonce=i)
        elif args.mode == "small":
            payload = make_small_swap(nonce=i)
        elif args.mode == "tight":
            payload = make_tight_slippage_swap(nonce=i)
        else:  # mixed
            r = i % 4
            if r == 0:
                payload = make_small_swap(nonce=i)
            elif r == 1:
                payload = make_tight_slippage_swap(nonce=i)
            else:
                payload = make_large_swap(nonce=i)

        assert len(payload) == WIRE_SIZE, f"payload size {len(payload)} != {WIRE_SIZE}"
        frames.append(ethernet_ip_udp_frame(payload))

    write_pcap(args.out, frames)
    print(f"[gen_traffic] Written {len(frames)} packets to {args.out}")
    print(f"  DexSwapTx wire size: {WIRE_SIZE} bytes per packet")
    print(f"  Mode: {args.mode}, seed: {args.seed}")
    print(f"  Replay with: tcpreplay --intf1=tap0 --mbps=100 {args.out}")


if __name__ == "__main__":
    main()
