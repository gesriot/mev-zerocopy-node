use bytemuck::bytes_of;
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use mev_zerocopy_node::payload::DexSwapTx;
use mev_zerocopy_node::processor;
use mev_zerocopy_node::validator::{PoolStateUpdate, validate_pool_update};
use serde::{Deserialize, Serialize};
use zerocopy::AsBytes;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DexSwapTxSerde {
    nonce: u64,
    pool_address: [u8; 20],
    amount_in: u64,
    min_amount_out: u64,
    token_direction: u8,
}

fn bench_deserialization(c: &mut Criterion) {
    let mut group = c.benchmark_group("mev_payload_parsing");

    let serde_value = DexSwapTxSerde {
        nonce: 1337,
        pool_address: [0xAA; 20],
        amount_in: 5_000_000,
        min_amount_out: 4_990_000,
        token_direction: 1,
    };
    let bincode_bytes = bincode::serialize(&serde_value).expect("bincode serialize");

    let zero_copy_value = DexSwapTx {
        nonce_le: 1337u64.to_le_bytes(),
        pool_address: [0xAA; 20],
        amount_in_le: 5_000_000u64.to_le_bytes(),
        min_amount_out_le: 4_990_000u64.to_le_bytes(),
        token_direction: 1,
        _reserved: [0; 3],
    };
    let zero_copy_bytes = bytes_of(&zero_copy_value);

    group.bench_function("serde_bincode_deserialize", |b| {
        b.iter(|| {
            let parsed: DexSwapTxSerde =
                bincode::deserialize(black_box(&bincode_bytes)).expect("bincode deserialize");
            black_box(parsed.amount_in);
        })
    });

    group.bench_function("bytemuck_pointer_cast", |b| {
        b.iter(|| {
            let parsed = bytemuck::try_from_bytes::<DexSwapTx>(black_box(zero_copy_bytes))
                .expect("bytemuck cast");
            black_box(parsed.amount_in());
        })
    });

    group.finish();
}

/// Benchmark 2: zerocopy::ref_from (PoolStateUpdate) vs serde_json for pool updates.
fn bench_pool_update_parsing(c: &mut Criterion) {
    let mut group = c.benchmark_group("pool_state_update_parsing");

    // zerocopy path: build wire bytes once
    let update = PoolStateUpdate {
        pool_address: [0xAB; 20],
        reserve0_le: 1_000_000_000u64.to_le_bytes(),
        reserve1_le: 500_000_000u64.to_le_bytes(),
        slot_le: 12_345_678u64.to_le_bytes(),
        seq_le: 1u32.to_le_bytes(),
        _pad: [0u8; 16],
    };
    let wire_bytes: Vec<u8> = update.as_bytes().to_vec();

    // serde_json path: build JSON bytes once
    let json_bytes = format!(
        r#"{{"pool":"0xabababababababababababababababababababababab","reserve0":1000000000,"reserve1":500000000,"slot":12345678,"seq":1}}"#
    );

    group.bench_function("zerocopy_ref_from", |b| {
        b.iter(|| {
            let u = validate_pool_update(black_box(&wire_bytes), 0).expect("valid");
            black_box(u.reserve0());
        })
    });

    group.bench_function("serde_json_deserialize", |b| {
        b.iter(|| {
            // Simulate what a naive implementation would do
            let s: serde_json::Value =
                serde_json::from_str(black_box(json_bytes.as_str())).expect("parse");
            black_box(s["reserve0"].as_u64().unwrap_or(0));
        })
    });

    group.finish();
}

/// Benchmark 3: full hot-path (bytemuck cast + AMM sandwich calculation).
fn bench_full_hot_path(c: &mut Criterion) {
    let mut group = c.benchmark_group("full_hot_path");

    let tx = DexSwapTx::from_parts(42, [0xAB; 20], 50_000_000, 1, 0);
    let wire = bytes_of(&tx);

    group.bench_function("process_packet_amm_sandwich", |b| {
        b.iter(|| {
            black_box(processor::process_packet(black_box(wire)));
        })
    });

    group.finish();
}

criterion_group!(benches, bench_deserialization, bench_pool_update_parsing, bench_full_hot_path);
criterion_main!(benches);
