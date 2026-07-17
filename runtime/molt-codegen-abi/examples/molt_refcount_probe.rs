//! Refcount storage cost probe for the generated object ABI.
//!
//! This measures the exact checked-retain and release primitives consumed by
//! `molt-runtime`, independent of allocator, GIL-entry, and destructor costs.
//! It is diagnostic telemetry, not a timing-sensitive test gate.

use std::cell::Cell;
use std::hint::black_box;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

use molt_codegen_abi::MoltRefCount;

const ITERATIONS: u32 = 10_000_000;
const SAMPLES: usize = 9;

#[inline(always)]
fn atomic_checked_retain(counter: &AtomicU32) -> u32 {
    let mut current = counter.load(Ordering::Relaxed);
    loop {
        let next = current.checked_add(1).expect("probe refcount overflow");
        match counter.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return current,
            Err(observed) => current = observed,
        }
    }
}

#[inline(always)]
fn cell_checked_retain(counter: &Cell<u32>) -> u32 {
    let current = counter.get();
    counter.set(current.checked_add(1).expect("probe refcount overflow"));
    current
}

#[inline(never)]
fn atomic_roundtrip(iterations: u32) -> u32 {
    let counter = AtomicU32::new(1);
    let mut witness = 0;
    for _ in 0..iterations {
        witness ^= black_box(atomic_checked_retain(&counter));
        witness ^= black_box(counter.fetch_sub(1, Ordering::Release));
    }
    witness ^ counter.load(Ordering::Acquire)
}

#[inline(never)]
fn cell_roundtrip(iterations: u32) -> u32 {
    let counter = Cell::new(1_u32);
    let mut witness = 0;
    for _ in 0..iterations {
        witness ^= black_box(cell_checked_retain(&counter));
        let previous = counter.get();
        counter.set(previous.checked_sub(1).expect("probe refcount underflow"));
        witness ^= black_box(previous);
    }
    witness ^ counter.get()
}

#[inline(never)]
fn mode_roundtrip(iterations: u32) -> u32 {
    let counter = MoltRefCount::new(1);
    let mut witness = 0;
    for _ in 0..iterations {
        witness ^= black_box(
            counter
                .retain_owned(1, || false)
                .expect("probe retain must remain live"),
        );
        witness ^= black_box(
            counter
                .release_owned()
                .expect("probe release must remain live")
                .previous(),
        );
    }
    witness ^ counter.snapshot_acquire()
}

fn median_ns_per_roundtrip(operation: fn(u32) -> u32) -> f64 {
    let mut samples = Vec::with_capacity(SAMPLES);
    let mut witness = 0;
    for _ in 0..SAMPLES {
        let started = Instant::now();
        witness ^= black_box(operation(ITERATIONS));
        samples.push(started.elapsed().as_nanos() as f64 / f64::from(ITERATIONS));
    }
    black_box(witness);
    samples.sort_by(f64::total_cmp);
    samples[SAMPLES / 2]
}

fn main() {
    let atomic = median_ns_per_roundtrip(atomic_roundtrip);
    let cell = median_ns_per_roundtrip(cell_roundtrip);
    let mode = median_ns_per_roundtrip(mode_roundtrip);
    println!(
        "{{\"iterations_per_sample\":{ITERATIONS},\"samples\":{SAMPLES},\"atomic_mode\":{},\"mode_checked_roundtrip_ns\":{mode:.6},\"atomic_checked_roundtrip_ns\":{atomic:.6},\"cell_checked_roundtrip_ns\":{cell:.6},\"atomic_to_mode_ratio\":{:.6},\"mode_to_cell_ratio\":{:.6},\"size_bytes\":{},\"alignment_bytes\":{}}}",
        molt_codegen_abi::MOLT_REFCOUNT_ATOMIC,
        atomic / mode,
        mode / cell,
        std::mem::size_of::<MoltRefCount>(),
        std::mem::align_of::<MoltRefCount>(),
    );
}
