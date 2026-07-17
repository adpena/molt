use molt_codegen_abi::{HEADER_FLAG_GC_UNPUBLISHED, MOLT_FLAGS_ATOMIC, MoltFlags};
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
        witness ^= black_box(flags.load(Ordering::Relaxed));
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
    witness ^ flags.load(Ordering::Relaxed)
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
        witness ^= black_box(flags.update_relaxed(1, 0));
    }
    witness ^ flags.load(Ordering::Relaxed)
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

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn molt_flags_relaxed_changed_probe(initial: u32, iterations: u32) -> u32 {
    let flags = MoltFlags::new(initial);
    let mut witness = 0_u32;
    for _ in 0..iterations {
        witness ^= flags.update_relaxed(1, 0);
        witness ^= flags.update_relaxed(0, 1);
    }
    witness ^ flags.load(Ordering::Relaxed)
}

#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn molt_flags_synchronized_changed_probe(initial: u32, iterations: u32) -> u32 {
    let flags = MoltFlags::new(initial);
    let mut witness = 0_u32;
    for _ in 0..iterations {
        witness ^= flags.update_synchronized(1, 0);
        witness ^= flags.update_synchronized(0, 1);
    }
    witness ^ flags.load(Ordering::Acquire)
}

// This cfg is the compile-time form of MOLT_FLAGS_ATOMIC. It is needed because
// only the native representation is Send + Sync; availability must not follow
// the independent refcount/free-threaded mode.
#[cfg(not(target_arch = "wasm32"))]
fn measure_contended(iterations: u32, synchronized: bool) -> Option<f64> {
    use std::sync::{Arc, Barrier};

    let thread_count = 2_u32;
    let flags = Arc::new(MoltFlags::new(0));
    let barrier = Arc::new(Barrier::new(thread_count as usize + 1));
    let mut workers = Vec::with_capacity(thread_count as usize);
    for worker in 0..thread_count {
        let flags = Arc::clone(&flags);
        let barrier = Arc::clone(&barrier);
        workers.push(std::thread::spawn(move || {
            let bit = 1_u32 << worker;
            barrier.wait();
            let mut witness = 0_u32;
            for _ in 0..iterations {
                witness ^= if synchronized {
                    flags.update_synchronized(bit, 0)
                } else {
                    flags.update_relaxed(bit, 0)
                };
                witness ^= if synchronized {
                    flags.update_synchronized(0, bit)
                } else {
                    flags.update_relaxed(0, bit)
                };
            }
            witness
        }));
    }
    let started = Instant::now();
    barrier.wait();
    let witness = workers.into_iter().fold(0_u32, |value, worker| {
        value ^ worker.join().expect("worker")
    });
    black_box(witness);
    Some(started.elapsed().as_nanos() as f64 / f64::from(thread_count * iterations * 2))
}

#[cfg(target_arch = "wasm32")]
fn measure_contended(_iterations: u32, _synchronized: bool) -> Option<f64> {
    None
}

fn main() {
    let flags = MoltFlags::new(0);
    black_box(flags.load(Ordering::Relaxed));
    let load_iterations = 20_000_000;
    let rmw_iterations = 1_000_000;
    let mode_load = measure(load_iterations, molt_flags_load_probe);
    let cell_load = measure(load_iterations, cell_flags_load_probe);
    let mode_rmw = measure(rmw_iterations, molt_flags_roundtrip_probe);
    let cell_rmw = measure(rmw_iterations, cell_flags_roundtrip_probe);
    let mode_sticky = measure(load_iterations, molt_flags_sticky_probe);
    let cell_sticky = measure(load_iterations, cell_flags_sticky_probe);
    let relaxed_changed = measure(rmw_iterations, molt_flags_relaxed_changed_probe);
    let synchronized_changed = measure(rmw_iterations, molt_flags_synchronized_changed_probe);
    let contended_relaxed = measure_contended(250_000, false);
    let contended_synchronized = measure_contended(250_000, true);
    assert_eq!(contended_relaxed.is_some(), MOLT_FLAGS_ATOMIC);
    assert_eq!(contended_synchronized.is_some(), MOLT_FLAGS_ATOMIC);
    let contended_ratio = contended_relaxed
        .zip(contended_synchronized)
        .map(|(relaxed, synchronized)| synchronized / relaxed);
    let json_number = |value: Option<f64>| {
        value
            .map(|number| format!("{number:.6}"))
            .unwrap_or_else(|| "null".to_string())
    };
    println!(
        "{{\"atomic_mode\":{},\"size_bytes\":{},\"alignment_bytes\":{},\"heap_allocations_per_instance\":0,\"relaxed_load_ns_per_op\":{:.6},\"cell_load_ns_per_op\":{:.6},\"load_ratio\":{:.6},\"sticky_ns_per_op\":{:.6},\"cell_sticky_ns_per_op\":{:.6},\"sticky_ratio\":{:.6},\"transition_roundtrip_ns_per_op\":{:.6},\"cell_transition_roundtrip_ns_per_op\":{:.6},\"transition_roundtrip_ratio\":{:.6},\"relaxed_changed_roundtrip_ns_per_op\":{:.6},\"synchronized_changed_roundtrip_ns_per_op\":{:.6},\"synchronization_changed_ratio\":{:.6},\"contended_available\":{},\"contended_relaxed_ns_per_op\":{},\"contended_synchronized_ns_per_op\":{},\"contended_synchronization_ratio\":{}}}",
        MOLT_FLAGS_ATOMIC,
        std::mem::size_of::<MoltFlags>(),
        std::mem::align_of::<MoltFlags>(),
        mode_load,
        cell_load,
        mode_load / cell_load,
        mode_sticky,
        cell_sticky,
        mode_sticky / cell_sticky,
        mode_rmw,
        cell_rmw,
        mode_rmw / cell_rmw,
        relaxed_changed,
        synchronized_changed,
        synchronized_changed / relaxed_changed,
        contended_relaxed.is_some(),
        json_number(contended_relaxed),
        json_number(contended_synchronized),
        json_number(contended_ratio),
    );
}
