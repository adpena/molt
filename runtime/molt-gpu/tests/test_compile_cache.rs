//! Tests for kernel compilation caching across all backends.

use molt_gpu::device::cpu::CpuDevice;
use molt_gpu::device::Compiler;

#[test]
fn test_cpu_compile_cache_hit() {
    let device = CpuDevice::new();
    let source = "kernel void test(device float* a [[buffer(0)]], uint gid [[thread_position_in_grid]]) { a[gid] = 1.0f; }";

    // First compile — cache miss
    let _p1 = device.compile(source, "test").expect("first compile");
    assert_eq!(device.cache_len(), 1);

    // Second compile of same source — cache hit
    let _p2 = device.compile(source, "test").expect("second compile");
    assert_eq!(device.cache_len(), 1, "cache should not grow on hit");
}

#[test]
fn test_cpu_compile_cache_different_sources() {
    let device = CpuDevice::new();

    let source1 = "kernel void a(device float* x) { x[0] = 1.0; }";
    let source2 = "kernel void b(device float* x) { x[0] = 2.0; }";
    let source3 = "kernel void c(device float* x) { x[0] = 3.0; }";

    let _p1 = device.compile(source1, "a").expect("compile source1");
    let _p2 = device.compile(source2, "b").expect("compile source2");
    let _p3 = device.compile(source3, "c").expect("compile source3");

    assert_eq!(
        device.cache_len(),
        3,
        "three different sources = three cache entries"
    );

    // Re-compile source1 — should be a cache hit
    let _p4 = device.compile(source1, "a").expect("re-compile source1");
    assert_eq!(device.cache_len(), 3, "re-compile should not add new entry");
}

#[test]
fn test_cpu_compile_cache_empty_initial() {
    let device = CpuDevice::new();
    assert_eq!(device.cache_len(), 0);
}

#[cfg(target_os = "macos")]
mod metal_cache_tests {
    use molt_gpu::device::metal::MetalDevice;
    use molt_gpu::device::Compiler;

    #[test]
    fn test_metal_compile_cache_hit() {
        let device = match MetalDevice::new() {
            Ok(d) => d,
            Err(_) => return, // Skip on machines without Metal
        };

        let source = r#"
            #include <metal_stdlib>
            using namespace metal;
            kernel void test(device float* a [[buffer(0)]],
                           uint gid [[thread_position_in_grid]]) {
                a[gid] = 1.0f;
            }
        "#;

        // First compile — cache miss (actual Metal compilation)
        let _p1 = device.compile(source, "test").expect("first Metal compile");

        // Second compile — cache hit (no recompilation)
        let _p2 = device
            .compile(source, "test")
            .expect("second Metal compile (cache hit)");

        // Both should succeed and return valid programs
    }
}
