use molt_codegen_abi::{HEADER_FLAG_GC_UNPUBLISHED, MoltFlags};
use std::cell::Cell;
use std::hint::black_box;
use std::sync::atomic::Ordering;
use std::time::Instant;

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn molt_flags_load_probe(initial: u32, iterations: u32) -> u32 {
    let flags = MoltFlags::new(initial);
    let mut witness = 0_u32;
    for _ in 0..iterations {
        witness ^= black_box(flags.load(Ordering::Acquire));
    }
    witness
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn cell_flags_load_probe(initial: u32, iterations: u32) -> u32 {
    let flags = Cell::new(initial);
    let mut witness = 0_u32;
    for _ in 0..iterations {
        witness ^= black_box(flags.get());
    }
    witness
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn molt_flags_roundtrip_probe(initial: u32, iterations: u32) -> u32 {
    let flags = MoltFlags::new(initial);
    let mut witness = 0_u32;
    for index in 0..iterations {
        let bit = 1_u32 << (index & 31);
        witness ^= flags.fetch_or(bit, Ordering::AcqRel);
        witness ^= flags.fetch_and(!bit, Ordering::AcqRel);
    }
    witness ^ flags.load(Ordering::Acquire)
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn cell_flags_roundtrip_probe(initial: u32, iterations: u32) -> u32 {
    let flags = Cell::new(initial);
    let mut witness = 0_u32;
    for index in 0..iterations {
        let bit = 1_u32 << (index & 31);
        let previous = flags.get();
        flags.set(previous | bit);
        witness ^= previous;
        let previous = flags.get();
        flags.set(previous & !bit);
        witness ^= previous;
    }
    witness ^ flags.get()
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn molt_flags_sticky_probe(initial: u32, iterations: u32) -> u32 {
    let flags = MoltFlags::new(initial);
    let mut witness = 0_u32;
    for _ in 0..iterations {
        witness ^= black_box(flags.update(1, 0));
    }
    witness ^ flags.load(Ordering::Acquire)
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn cell_flags_sticky_probe(initial: u32, iterations: u32) -> u32 {
    let flags = Cell::new(initial);
    let mut witness = 0_u32;
    for _ in 0..iterations {
        let previous = flags.get();
        if previous & 1 == 0 {
            flags.set(previous | 1);
        }
        witness ^= black_box(previous);
    }
    witness ^ flags.get()
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn molt_flags_publication_probe(initial: u32) -> u32 {
    let flags = MoltFlags::new_unpublished(initial);
    let unpublished = u32::from(!flags.is_published());
    flags.publish_initialized();
    let published = u32::from(flags.is_published());
    initial ^ unpublished ^ published ^ flags.load(Ordering::Acquire)
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn cell_flags_publication_probe(initial: u32) -> u32 {
    let flags = Cell::new(initial | HEADER_FLAG_GC_UNPUBLISHED);
    let unpublished = u32::from(flags.get() & HEADER_FLAG_GC_UNPUBLISHED != 0);
    flags.set(flags.get() & !HEADER_FLAG_GC_UNPUBLISHED);
    let published = u32::from(flags.get() & HEADER_FLAG_GC_UNPUBLISHED == 0);
    initial ^ unpublished ^ published ^ flags.get()
}

fn median_ns_per_iteration(mut samples: Vec<f64>) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[samples.len() / 2]
}

fn measure(iterations: u32, operation: extern "C" fn(u32, u32) -> u32) -> f64 {
    let mut samples = Vec::with_capacity(9);
    let mut witness = 0_u32;
    for _ in 0..9 {
        let started = Instant::now();
        witness ^= black_box(operation(0x5a5a_5a5a, iterations));
        samples.push(started.elapsed().as_nanos() as f64 / f64::from(iterations));
    }
    black_box(witness);
    median_ns_per_iteration(samples)
}

fn main() {
    let constructed = Instant::now();
    let flags = MoltFlags::new(0);
    black_box(flags.load(Ordering::Acquire));
    let construct_ns = constructed.elapsed().as_nanos();
    let load_iterations = 20_000_000;
    let rmw_iterations = 1_000_000;
    let mode_load = measure(load_iterations, molt_flags_load_probe);
    let cell_load = measure(load_iterations, cell_flags_load_probe);
    let mode_rmw = measure(rmw_iterations, molt_flags_roundtrip_probe);
    let cell_rmw = measure(rmw_iterations, cell_flags_roundtrip_probe);
    let mode_sticky = measure(load_iterations, molt_flags_sticky_probe);
    let cell_sticky = measure(load_iterations, cell_flags_sticky_probe);
    println!(
        "{{\"atomic_mode\":{},\"size_bytes\":{},\"alignment_bytes\":{},\"heap_allocations_per_instance\":0,\"construct_ns\":{},\"load_ns_per_op\":{:.6},\"cell_load_ns_per_op\":{:.6},\"load_ratio\":{:.6},\"sticky_ns_per_op\":{:.6},\"cell_sticky_ns_per_op\":{:.6},\"sticky_ratio\":{:.6},\"transition_roundtrip_ns_per_op\":{:.6},\"cell_transition_roundtrip_ns_per_op\":{:.6},\"transition_roundtrip_ratio\":{:.6}}}",
        molt_codegen_abi::MOLT_FLAGS_ATOMIC,
        std::mem::size_of::<MoltFlags>(),
        std::mem::align_of::<MoltFlags>(),
        construct_ns,
        mode_load,
        cell_load,
        mode_load / cell_load,
        mode_sticky,
        cell_sticky,
        mode_sticky / cell_sticky,
        mode_rmw,
        cell_rmw,
        mode_rmw / cell_rmw,
    );
}
