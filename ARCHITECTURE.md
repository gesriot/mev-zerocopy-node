# Extended Zero-Copy MEV Architecture

## Pipeline

```text
[ NIC / veth ] -> [ AF_XDP socket probe ] -> [ userspace preallocated buffers ]
                 -> [ smoltcp TCP/UDP stack ] -> [ bytemuck POD cast ]
                 -> [ arbitrage logic ] -> [ cache-aligned response ring ]
                 -> [ packet send ]
```

## Hardware Sympathy

- CPU pinning: current processing thread is pinned to core `0` (`core_affinity`).
- False-sharing mitigation: hot counters and ring wrappers are aligned to 64-byte cache line.
- Hot-path allocations: avoided through fixed buffers and fixed-size socket/ring structures.

## Observability

- `LatencyClock`: cycle timing (`rdtsc`) + wall timing (`minstant`) per packet handling iteration.
- `perf` helper script collects call graph and grep-checks allocator symbols.

## Extended Task Mapping

- Kernel bypass direction: AF_XDP-capability probe and XDP-ready veth harness scripts.
- Zero-copy: `DexSwapTx` is POD and parsed with `bytemuck`.
- No-heap hot path: packet parse/decision/reply done without `Vec/String/Box/HashMap`.
- Reproducibility: scripts for TAP, veth/tcpreplay, and perf.
