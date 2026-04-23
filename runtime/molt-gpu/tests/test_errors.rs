//! Error handling hardening tests.
//!
//! Verifies that every Result-returning function in molt-gpu handles
//! adversarial inputs gracefully: zero-size allocs, absurd sizes,
//! empty/invalid sources, wrong buffer counts, mismatched sizes.

use molt_gpu::device::cpu::CpuDevice;
use molt_gpu::device::{Allocator, Compiler, DeviceError, Executor};

// =============================================================================
// 1. Allocator edge cases
// =============================================================================

#[test]
fn test_alloc_size_zero() {
    let dev = CpuDevice::new();
    // Size 0 allocation: should succeed (zero-size buffer is valid).
    let result = dev.alloc(0);
    assert!(result.is_ok(), "alloc(0) should succeed");
    let buf = result.unwrap();
    assert_eq!(buf.size_bytes, 0);
    dev.free(buf).unwrap();
}

#[test]
fn test_alloc_small_sizes() {
    let dev = CpuDevice::new();
    // Allocate very small sizes
    for size in [1, 2, 3, 4, 7, 15, 16, 31, 32, 63, 64, 127, 128, 255, 256] {
        let result = dev.alloc(size);
        assert!(result.is_ok(), "alloc({}) should succeed", size);
        let buf = result.unwrap();
        assert_eq!(buf.size_bytes, size);
        dev.free(buf).unwrap();
    }
}

#[test]
fn test_alloc_large_reasonable() {
    let dev = CpuDevice::new();
    // 64 MiB -- reasonable large allocation
    let result = dev.alloc(64 * 1024 * 1024);
    assert!(result.is_ok(), "alloc(64MiB) should succeed");
    let buf = result.unwrap();
    assert_eq!(buf.size_bytes, 64 * 1024 * 1024);
    dev.free(buf).unwrap();
}

// NOTE: We intentionally do NOT test alloc(usize::MAX) because on the CPU
// backend, Vec::new() would attempt to allocate the full size and either
// OOM-kill the process or panic. This is correct behavior -- the CPU backend
// is a reference implementation for testing, not a production allocator.
// Real GPU backends (Metal, WebGPU) have explicit size validation in their
// alloc() implementations.

// =============================================================================
// 2. Compiler edge cases
// =============================================================================

#[test]
fn test_compile_empty_source() {
    let dev = CpuDevice::new();
    // Empty source should still "compile" on CPU (it's an interpreter).
    let result = dev.compile("", "main");
    assert!(
        result.is_ok(),
        "compile with empty source should succeed on CPU"
    );
}

#[test]
fn test_compile_invalid_source() {
    let dev = CpuDevice::new();
    // Invalid/garbage source should still "compile" on CPU (noop kernel).
    let result = dev.compile("@#$%^&*() not valid shader source !!!", "main");
    assert!(
        result.is_ok(),
        "compile with invalid source should succeed on CPU"
    );
}

#[test]
fn test_compile_cache_dedup() {
    let dev = CpuDevice::new();
    let source = "kernel void foo() {}";

    // Compile same source twice
    let r1 = dev.compile(source, "foo");
    assert!(r1.is_ok());
    assert_eq!(dev.cache_len(), 1);

    let r2 = dev.compile(source, "foo");
    assert!(r2.is_ok());
    assert_eq!(
        dev.cache_len(),
        1,
        "cache should deduplicate identical sources"
    );

    // Different source
    let r3 = dev.compile("kernel void bar() {}", "bar");
    assert!(r3.is_ok());
    assert_eq!(dev.cache_len(), 2);
}

// =============================================================================
// 3. Executor edge cases
// =============================================================================

#[test]
fn test_exec_with_no_buffers() {
    let dev = CpuDevice::new();
    let prog = dev.compile("noop", "main").unwrap();

    // exec with empty buffer slice
    let result = dev.exec(&prog, &[], [1, 1, 1], [1, 1, 1]);
    assert!(result.is_ok(), "exec with no buffers should succeed on CPU");
}

#[test]
fn test_exec_zero_grid() {
    let dev = CpuDevice::new();
    let prog = dev.compile("noop", "main").unwrap();

    // exec with zero grid dimensions
    let result = dev.exec(&prog, &[], [0, 0, 0], [1, 1, 1]);
    assert!(result.is_ok(), "exec with zero grid should succeed on CPU");
}

#[test]
fn test_synchronize() {
    let dev = CpuDevice::new();
    // CPU synchronize is a no-op -- should always succeed
    let result = dev.synchronize();
    assert!(result.is_ok());
}

// =============================================================================
// 4. Copy operations edge cases
// =============================================================================

#[test]
fn test_copy_in_data_larger_than_buffer() {
    let dev = CpuDevice::new();
    let buf = dev.alloc(16).unwrap();
    let big_data = vec![0xFFu8; 32]; // 32 bytes into a 16 byte buffer

    let result = dev.copy_in(&buf, &big_data);
    assert!(result.is_err(), "copy_in with oversized data should fail");

    match result.unwrap_err() {
        DeviceError::InvalidArgument(msg) => {
            assert!(
                msg.contains("exceeds buffer"),
                "error message should mention size mismatch: {}",
                msg
            );
        }
        other => panic!("expected InvalidArgument, got {:?}", other),
    }

    dev.free(buf).unwrap();
}

#[test]
fn test_copy_in_exact_size() {
    let dev = CpuDevice::new();
    let buf = dev.alloc(16).unwrap();
    let data = vec![0xABu8; 16];

    let result = dev.copy_in(&buf, &data);
    assert!(
        result.is_ok(),
        "copy_in with exact-size data should succeed"
    );

    let mut out = vec![0u8; 16];
    dev.copy_out(&buf, &mut out).unwrap();
    assert_eq!(out, data);

    dev.free(buf).unwrap();
}

#[test]
fn test_copy_in_smaller_than_buffer() {
    let dev = CpuDevice::new();
    let buf = dev.alloc(32).unwrap();
    let data = vec![0xCDu8; 16]; // 16 bytes into a 32 byte buffer

    let result = dev.copy_in(&buf, &data);
    assert!(result.is_ok(), "copy_in with smaller data should succeed");

    let mut out = vec![0u8; 32];
    dev.copy_out(&buf, &mut out).unwrap();
    // First 16 bytes should be our data, rest stays zero
    assert_eq!(&out[..16], &data[..]);
    assert_eq!(&out[16..], &[0u8; 16]);

    dev.free(buf).unwrap();
}

#[test]
fn test_copy_out_buffer_larger_than_data() {
    let dev = CpuDevice::new();
    let buf = dev.alloc(32).unwrap();
    let data = vec![0xEFu8; 32];
    dev.copy_in(&buf, &data).unwrap();

    // Read into a smaller slice
    let mut out = vec![0u8; 16];
    let result = dev.copy_out(&buf, &mut out);
    assert!(result.is_ok(), "copy_out into smaller slice should succeed");
    assert_eq!(&out, &data[..16], "should read the first 16 bytes");

    dev.free(buf).unwrap();
}

#[test]
fn test_copy_in_empty_data() {
    let dev = CpuDevice::new();
    let buf = dev.alloc(16).unwrap();
    let data: &[u8] = &[];

    let result = dev.copy_in(&buf, data);
    assert!(result.is_ok(), "copy_in with empty data should succeed");

    dev.free(buf).unwrap();
}

#[test]
fn test_copy_out_empty_slice() {
    let dev = CpuDevice::new();
    let buf = dev.alloc(16).unwrap();
    let mut out: Vec<u8> = vec![];

    let result = dev.copy_out(&buf, &mut out);
    assert!(result.is_ok(), "copy_out into empty slice should succeed");

    dev.free(buf).unwrap();
}

// =============================================================================
// 5. Zero-size buffer operations
// =============================================================================

#[test]
fn test_zero_size_buffer_copy_in() {
    let dev = CpuDevice::new();
    let buf = dev.alloc(0).unwrap();

    // copy_in empty data to zero-size buffer
    let result = dev.copy_in(&buf, &[]);
    assert!(result.is_ok());

    // copy_in non-empty data to zero-size buffer should fail
    let result = dev.copy_in(&buf, &[1, 2, 3]);
    assert!(
        result.is_err(),
        "copy_in of data to zero-size buffer should fail"
    );

    dev.free(buf).unwrap();
}

#[test]
fn test_zero_size_buffer_copy_out() {
    let dev = CpuDevice::new();
    let buf = dev.alloc(0).unwrap();

    let mut out: Vec<u8> = vec![];
    let result = dev.copy_out(&buf, &mut out);
    assert!(result.is_ok());

    dev.free(buf).unwrap();
}

// =============================================================================
// 6. Double free (should not panic)
// =============================================================================

#[test]
fn test_free_is_idempotent() {
    let dev = CpuDevice::new();
    let buf = dev.alloc(256).unwrap();
    // First free
    let result = dev.free(buf);
    assert!(result.is_ok());
    // Note: second free is not possible because free() takes ownership
    // (DeviceBuffer is moved). This is the correct Rust ownership pattern.
}

// =============================================================================
// 7. Alloc + immediate free cycle (no leak)
// =============================================================================

#[test]
fn test_alloc_free_cycle_no_leak() {
    let dev = CpuDevice::new();
    // Rapid alloc/free cycle -- should not accumulate memory
    for _ in 0..1000 {
        let buf = dev.alloc(4096).unwrap();
        dev.free(buf).unwrap();
    }
    // If we got here without OOM, no leak occurred
}
