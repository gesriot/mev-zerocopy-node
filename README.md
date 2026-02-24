# MEV Zero-Copy Node

High-frequency MEV network node demonstrating kernel-bypass architecture,
hardware-friendly hot path, and deterministic microsecond-level latency in Rust.

## Architecture

```
NIC / tcpreplay
    │
    ▼
[AF_XDP socket]  ← kernel-bypass: DMA directly into UMEM, no sk_buff allocation
    │  (or TAP fallback for dev/testing)
    ▼
[smoltcp TCP/UDP] ← userspace network stack, no kernel syscalls in hot loop
    │
    ▼
[bytemuck::try_from_bytes::<DexSwapTx>()]  ← zero-copy pointer cast (48 bytes)
    │
    ▼
[AMM sandwich arbitrage]  ← constant-product x*y=k, no heap, no floats
    │
    ▼
[heapless ResponseRing<1024>]  ← cache-aligned SPSC queue, stack-allocated
    │
    ▼
[smoltcp send_slice()]  ← reply to sender
```

**Why not `std::net::TcpStream`?**
Standard sockets go through the full Linux kernel path: `sk_buff` allocation,
interrupt handling, context switch to userspace, `copy_to_user()` buffer copy.
Each hop adds unpredictable jitter (10–100µs). AF_XDP eliminates all of this —
packets go from NIC DMA → UMEM → our code with no kernel involvement.

## What Is Implemented

| Component | Crate | Role |
|---|---|---|
| Kernel-bypass probe | `libc` / AF_XDP socket | Verify kernel supports AF_XDP (`probe_af_xdp_socket`) |
| AF_XDP UMEM | `libc` mmap + setsockopt | Full UMEM allocation, mlock, ring sizing, bind (`XdpUmem`, `XdpSocket`) |
| Userspace TCP/UDP | `smoltcp` | Ethernet+IPv4+TCP+UDP stack over TAP, no kernel read()/write() |
| Zero-copy hot path | `bytemuck` | `DexSwapTx` POD cast — pointer reinterpretation, zero allocation |
| Validated cast | `zerocopy` | `PoolStateUpdate` with `ref_from` + sequence/reserve checks |
| AMM arbitrage | inline math | Constant-product sandwich profit calculation (`AmmPoolState`) |
| Ring buffer | `heapless::spsc` | Cache-line-aligned SPSC queue, 1024 slots, stack-allocated |
| CPU pinning | `core_affinity` | Thread pinned to core 0, prevents cache thrashing |
| Latency telemetry | `minstant` + `rdtsc` | Cycle-accurate and wall-clock timing per packet |
| Benchmarks | `criterion` | serde_bincode vs bytemuck vs zerocopy vs full hot path |
| Traffic generator | Python | Generates `.pcap` with `DexSwapTx` UDP packets for tcpreplay |
| Test environment | Docker Compose | `node` + `traffic` containers with shared pcap volume |

## Quick Start

### Option A — Docker Compose (recommended)

```bash
# Build and run: node starts, traffic generator fires 200 mixed packets
docker compose up --build

# Customize traffic:
PCAP_COUNT=1000 PCAP_MODE=large REPLAY_MBPS=100 docker compose up --build

# Available PCAP_MODE values:
#   mixed  — profitable + small + tight-slippage (default)
#   large  — large swaps, all profitable sandwich targets
#   small  — below MIN_AMOUNT_IN threshold, all rejected
#   tight  — tight slippage, may revert after front-run
```

### Option B — Native Linux (TAP mode)

```bash
# 1. Create TAP interface
./scripts/setup_tap.sh

# 2. Generate test traffic
python3 scripts/gen_traffic.py --count 200 --mode mixed --out traffic/mock_tx.pcap

# 3. Start node + replay
./scripts/run_tcpreplay.sh traffic/mock_tx.pcap
```

### Option C — AF_XDP mode (requires CAP_NET_ADMIN + XDP-capable NIC)

```bash
./scripts/run_tcpreplay_veth.sh traffic/mock_tx.pcap
# Sets up veth pair, starts node with MEV_BACKEND=af_xdp
```

## Benchmarks

```bash
./scripts/run_bench.sh
# or directly:
cargo bench
```

Three benchmark groups:

| Group | What it measures |
|---|---|
| `mev_payload_parsing` | `serde_bincode` vs `bytemuck` DexSwapTx deserialization |
| `pool_state_update_parsing` | `zerocopy::ref_from` vs `serde_json` pool update parsing |
| `full_hot_path` | Complete pipeline: bytemuck cast + AMM sandwich calculation |

Expected speedups (bytemuck/zerocopy vs serde):
- DexSwapTx cast: **20–50x faster** than bincode
- PoolStateUpdate: **30–100x faster** than serde_json

## Flamegraph / Perf

```bash
./scripts/run_perf_flamegraph.sh
```

Produces `perf-report.txt`. The script greps for `malloc`/`free` symbols —
in a correct no-heap build, the hot loop will show zero allocator calls.

## Traffic Generator

```bash
python3 scripts/gen_traffic.py --help

# Examples:
python3 scripts/gen_traffic.py --count 500 --mode large --out traffic/large.pcap
python3 scripts/gen_traffic.py --count 100 --mode tight --out traffic/tight.pcap
```

Generated packets are valid Ethernet/IPv4/UDP frames with `DexSwapTx` payloads
in little-endian binary layout, ready for `tcpreplay`.

## Zero-Copy Modules

### `bytemuck` — hot path (`src/processor.rs`, `src/payload.rs`)

```rust
// One pointer cast, no copy, no allocation:
let tx = bytemuck::try_from_bytes::<DexSwapTx>(wire).ok()?;
let profit = MOCK_POOL.sandwich_profit(tx.amount_in(), OUR_AMOUNT, zero_for_one);
```

### `zerocopy` — validated outer layer (`src/validator.rs`)

```rust
// Safe cast with layout check + domain validation:
let update = PoolStateUpdate::ref_from(&data[..64]).ok_or(LayoutMismatch)?;
// update points into the original buffer — no copy
```

## AF_XDP (`src/xdp.rs`)

Full UMEM setup on Linux:
1. `socket(AF_XDP, SOCK_RAW, 0)` — open socket
2. `mmap(MAP_ANONYMOUS | MAP_POPULATE)` — allocate UMEM region
3. `mlock()` — pin memory, prevent paging
4. `setsockopt(XDP_UMEM_REG)` — register with kernel
5. `setsockopt(XDP_UMEM_FILL_RING / XDP_UMEM_COMPLETION_RING)` — size rings
6. `setsockopt(XDP_RX_RING / XDP_TX_RING)` — size RX/TX rings
7. `bind(sockaddr_xdp)` — attach to NIC queue

In production, add `getsockopt(XDP_MMAP_OFFSETS)` + `mmap` to obtain
ring buffer pointers, then load an eBPF XDP program via `aya`.

## CI

`.github/workflows/ci.yml` runs on every push:
- `cargo fmt --check`
- `cargo clippy -- -D warnings`
- `cargo test --lib --bins`
- `cargo check --features af_xdp`
- `cargo bench --no-run`
- `cargo audit`
