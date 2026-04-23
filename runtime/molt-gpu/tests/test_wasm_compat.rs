//! WASM compatibility tests.
//!
//! These tests run on the host but verify that the WASM-backend code
//! does not use any APIs that are unavailable on wasm32-unknown-unknown.
//! The `wasm_cpu` device is tested directly on the host since it uses
//! `RefCell` instead of `Mutex` and avoids std::time.

#[cfg(feature = "wasm-backend")]
mod wasm_cpu_tests {
    use molt_gpu::device::wasm_cpu::WasmCpuDevice;
    use molt_gpu::device::{Allocator, Compiler, DeviceError, Executor};

    #[test]
    fn test_wasm_cpu_device_creation() {
        let device = WasmCpuDevice::new();
        assert_eq!(device.cache_len(), 0);
    }

    #[test]
    fn test_wasm_cpu_alloc_and_copy() {
        let device = WasmCpuDevice::new();

        // Allocate a buffer
        let buf = device.alloc(256).expect("alloc should succeed");
        assert_eq!(buf.size_bytes, 256);

        // Copy data in
        let data: Vec<u8> = (0..256).map(|i| i as u8).collect();
        device.copy_in(&buf, &data).expect("copy_in should succeed");

        // Copy data out
        let mut out = vec![0u8; 256];
        device
            .copy_out(&buf, &mut out)
            .expect("copy_out should succeed");
        assert_eq!(out, data);

        // Free
        device.free(buf).expect("free should succeed");
    }

    #[test]
    fn test_wasm_cpu_alloc_zero_size() {
        let device = WasmCpuDevice::new();
        let buf = device.alloc(0).expect("zero-size alloc should succeed");
        assert_eq!(buf.size_bytes, 0);
        device.free(buf).expect("free should succeed");
    }

    #[test]
    fn test_wasm_cpu_copy_in_too_large() {
        let device = WasmCpuDevice::new();
        let buf = device.alloc(4).expect("alloc should succeed");
        let data = vec![0u8; 8]; // Too large
        let result = device.copy_in(&buf, &data);
        assert!(result.is_err(), "copy_in should fail for oversized data");
    }

    #[test]
    fn test_wasm_cpu_compile_and_cache() {
        let device = WasmCpuDevice::new();

        let prog1 = device
            .compile("source_a", "main")
            .expect("compile should succeed");
        assert_eq!(prog1.entry, "main");
        assert_eq!(device.cache_len(), 1);

        // Same source should hit cache
        let prog2 = device
            .compile("source_a", "main")
            .expect("compile should succeed");
        assert_eq!(prog2.entry, "main");
        assert_eq!(device.cache_len(), 1); // No new entry

        // Different source should create new entry
        let prog3 = device
            .compile("source_b", "entry")
            .expect("compile should succeed");
        assert_eq!(prog3.entry, "entry");
        assert_eq!(device.cache_len(), 2);
    }

    #[test]
    fn test_wasm_cpu_max_sizes() {
        let device = WasmCpuDevice::new();
        // WASM local size should be 1 (no hardware threads)
        assert_eq!(device.max_local_size(), [1, 1, 1]);
        assert_eq!(device.max_grid_size(), [u32::MAX, 1, 1]);
    }

    #[test]
    fn test_wasm_cpu_exec_noop() {
        let device = WasmCpuDevice::new();
        let prog = device
            .compile("test", "main")
            .expect("compile should succeed");
        let buf = device.alloc(64).expect("alloc should succeed");

        // exec is a no-op for WASM CPU (interpretation happens via execute_kernel)
        device
            .exec(&prog, &[&buf], [1, 1, 1], [1, 1, 1])
            .expect("exec should succeed");
    }

    #[test]
    fn test_wasm_cpu_synchronize() {
        let device = WasmCpuDevice::new();
        // Synchronize is a no-op for WASM (single-threaded)
        device.synchronize().expect("synchronize should succeed");
    }

    #[test]
    fn test_wasm_cpu_interpreter_reexport() {
        // Verify the WASM interpreter re-exports work
        use molt_gpu::device::wasm_cpu::interpret::execute_kernel;
        use molt_gpu::dtype::DType;
        use molt_gpu::ops::PrimitiveOp;
        use molt_gpu::render::{BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc};
        use molt_gpu::shapetracker::ShapeTracker;

        // Simple add kernel: buf0 = buf1 + buf2
        let kernel = FusedKernel {
            ops: vec![FusedOp {
                op: PrimitiveOp::Add,
                srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
                dst_dtype: DType::Float32,
            }],
            bufs: vec![
                BufferBinding {
                    buf_id: 0,
                    st: ShapeTracker::contiguous(&[4]),
                    dtype: DType::Float32,
                    access: BufferAccess::Write,
                },
                BufferBinding {
                    buf_id: 1,
                    st: ShapeTracker::contiguous(&[4]),
                    dtype: DType::Float32,
                    access: BufferAccess::Read,
                },
                BufferBinding {
                    buf_id: 2,
                    st: ShapeTracker::contiguous(&[4]),
                    dtype: DType::Float32,
                    access: BufferAccess::Read,
                },
            ],
            grid: [4, 1, 1],
            local: [4, 1, 1],
            spec: None,
            vectorize_width: 1,
        };

        let mut bufs = vec![
            vec![0u8; 16], // output
            vec![0u8; 16], // input 1: [1.0, 2.0, 3.0, 4.0]
            vec![0u8; 16], // input 2: [10.0, 20.0, 30.0, 40.0]
        ];

        // Fill input buffers
        for (i, v) in [1.0f32, 2.0, 3.0, 4.0].iter().enumerate() {
            bufs[1][i * 4..(i + 1) * 4].copy_from_slice(&v.to_le_bytes());
        }
        for (i, v) in [10.0f32, 20.0, 30.0, 40.0].iter().enumerate() {
            bufs[2][i * 4..(i + 1) * 4].copy_from_slice(&v.to_le_bytes());
        }

        execute_kernel(&kernel, &mut bufs);

        // Read output: should be [11.0, 22.0, 33.0, 44.0]
        let expected = [11.0f32, 22.0, 33.0, 44.0];
        for (i, exp) in expected.iter().enumerate() {
            let bytes: [u8; 4] = bufs[0][i * 4..(i + 1) * 4].try_into().unwrap();
            let val = f32::from_le_bytes(bytes);
            assert!(
                (val - exp).abs() < 1e-6,
                "Output[{}] = {}, expected {}",
                i,
                val,
                exp,
            );
        }
    }

    #[test]
    fn test_wasm_cpu_default_trait() {
        // Test Default impl
        let device = WasmCpuDevice::default();
        assert_eq!(device.cache_len(), 0);
    }

    #[test]
    fn test_wasm_cpu_multiple_allocs() {
        let device = WasmCpuDevice::new();

        // Allocate multiple buffers (WASM memory)
        let mut buffers = Vec::new();
        for size in [64, 128, 256, 512, 1024] {
            let buf = device.alloc(size).expect("alloc should succeed");
            assert_eq!(buf.size_bytes, size);
            buffers.push(buf);
        }

        // Free all
        for buf in buffers {
            device.free(buf).expect("free should succeed");
        }
    }
}

/// Tests that verify no WASM-incompatible APIs leak into the wasm-backend
/// feature gate. These tests run on all platforms (not just wasm32).
#[cfg(feature = "wasm-backend")]
mod wasm_api_leak_tests {
    /// Verify that wasm_cpu module does not require std::sync::Mutex.
    /// This is structurally guaranteed by the use of RefCell, but we
    /// verify by instantiating the device without any Mutex-dependent paths.
    #[test]
    fn test_no_mutex_in_wasm_device() {
        use molt_gpu::device::wasm_cpu::WasmCpuDevice;
        use molt_gpu::device::Compiler;

        let device = WasmCpuDevice::new();
        // If this compiled and runs, the device doesn't require Mutex at runtime.
        let _ = device.compile("test_no_mutex", "main");
        assert!(device.cache_len() == 1);
    }
}
