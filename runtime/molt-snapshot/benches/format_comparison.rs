use criterion::{Criterion, criterion_group, criterion_main};
use molt_snapshot::{ExecutionSnapshot, PendingExternalCall, ProgramCounter, ResourceSnapshot};
use std::hint::black_box;

fn sample_snapshot(memory_size: usize) -> ExecutionSnapshot {
    ExecutionSnapshot {
        version: 1,
        memory: vec![0xABu8; memory_size],
        globals: (0..50).map(|i| 0x7ff8_0001_0000_0000u64 + i).collect(),
        table: (0..100).collect(),
        pc: ProgramCounter {
            func_index: 42,
            instruction_offset: 1024,
            call_depth: 8,
        },
        pending_call: PendingExternalCall {
            function_name: "fetch_user_data".into(),
            args: vec![0x7ff8_0001_0000_002a; 5],
            call_id: 99999,
        },
        resource_state: ResourceSnapshot {
            allocation_count: 50000,
            memory_used: memory_size,
            elapsed_ms: 1500,
        },
    }
}

fn bench_hand_rolled(c: &mut Criterion) {
    let snap = sample_snapshot(65536); // 64KB memory
    let serialized = snap.serialize();

    println!(
        "\n[format_comparison] hand-rolled size for 64KB snapshot: {} bytes",
        serialized.len()
    );

    c.bench_function("hand_rolled_serialize_64kb", |b| {
        b.iter(|| {
            let bytes = snap.serialize();
            black_box(bytes);
        })
    });

    c.bench_function("hand_rolled_deserialize_64kb", |b| {
        b.iter(|| {
            let restored = ExecutionSnapshot::deserialize(&serialized).unwrap();
            black_box(restored);
        })
    });

    c.bench_function("hand_rolled_roundtrip_64kb", |b| {
        b.iter(|| {
            let bytes = snap.serialize();
            let restored = ExecutionSnapshot::deserialize(&bytes).unwrap();
            black_box(restored);
        })
    });
}

criterion_group!(benches, bench_hand_rolled);
criterion_main!(benches);
