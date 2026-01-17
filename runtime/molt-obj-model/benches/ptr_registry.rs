use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use molt_obj_model::{register_ptr, release_ptr, reset_ptr_registry, resolve_ptr};

fn bench_register_resolve_release(c: &mut Criterion) {
    let mut group = c.benchmark_group("ptr_registry");
    for size in [1024usize, 8192, 65536] {
        let mut objects: Vec<Box<u64>> = (0..size).map(|i| Box::new(i as u64)).collect();
        let ptrs: Vec<*mut u8> = objects
            .iter_mut()
            .map(|b| b.as_mut() as *mut u64 as *mut u8)
            .collect();
        group.bench_with_input(
            BenchmarkId::new("register_resolve_release", size),
            &ptrs,
            |b, ptrs| {
                b.iter(|| {
                    for &ptr in ptrs {
                        let addr = register_ptr(ptr);
                        black_box(resolve_ptr(addr));
                        black_box(release_ptr(ptr));
                    }
                });
            },
        );
        reset_ptr_registry();
    }
    group.finish();
}

fn bench_resolve_only(c: &mut Criterion) {
    let mut group = c.benchmark_group("ptr_registry");
    for size in [1024usize, 8192, 65536] {
        let mut objects: Vec<Box<u64>> = (0..size).map(|i| Box::new(i as u64)).collect();
        let ptrs: Vec<*mut u8> = objects
            .iter_mut()
            .map(|b| b.as_mut() as *mut u64 as *mut u8)
            .collect();
        let addrs: Vec<u64> = ptrs.iter().map(|&ptr| register_ptr(ptr)).collect();
        group.bench_with_input(BenchmarkId::new("resolve", size), &addrs, |b, addrs| {
            b.iter(|| {
                for &addr in addrs {
                    black_box(resolve_ptr(addr));
                }
            });
        });
        for ptr in ptrs {
            black_box(release_ptr(ptr));
        }
        reset_ptr_registry();
    }
    group.finish();
}

criterion_group!(
    ptr_registry_benches,
    bench_register_resolve_release,
    bench_resolve_only
);
criterion_main!(ptr_registry_benches);
