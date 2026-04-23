//! WasmCpuDevice — WASM-specific CPU reference backend.
//!
//! A variant of CpuDevice designed for `wasm32-unknown-unknown`:
//! - No `std::sync::Mutex` (WASM is single-threaded by default) — uses `RefCell`
//! - No `std::time` — timer operations are no-ops
//! - No file I/O or threading APIs
//! - Same interpreter as `cpu.rs` for correctness parity

use core::cell::RefCell;
use std::collections::HashMap;

use crate::device::{
    Allocator, BufferHandle, CompiledProgram, Compiler, CpuKernelFn, DeviceBuffer, DeviceError,
    Executor, ProgramHandle,
};

/// WASM-compatible CPU reference device backend.
///
/// Functionally identical to `CpuDevice` but avoids `Mutex`, threads,
/// file I/O, and `std::time` — all unavailable on `wasm32-unknown-unknown`.
/// Uses `RefCell` for interior mutability (safe because WASM is single-threaded).
pub struct WasmCpuDevice {
    /// Buffer allocation counter for unique IDs.
    _next_id: RefCell<usize>,
    /// Compiled program cache: source hash -> entry name.
    compile_cache: RefCell<HashMap<u64, String>>,
}

// SAFETY: WASM is single-threaded. These impls are required by the Allocator/
// Compiler/Executor traits (Send + Sync bounds) and are safe because no
// concurrent access is possible on wasm32.
unsafe impl Send for WasmCpuDevice {}
unsafe impl Sync for WasmCpuDevice {}

impl WasmCpuDevice {
    /// Create a new WASM CPU device.
    pub fn new() -> Self {
        Self {
            _next_id: RefCell::new(0),
            compile_cache: RefCell::new(HashMap::new()),
        }
    }

    /// Hash shader source for cache lookup (same algorithm as CpuDevice/MetalDevice).
    fn hash_source(source: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        source.hash(&mut hasher);
        hasher.finish()
    }

    /// Returns the number of cached compiled programs.
    pub fn cache_len(&self) -> usize {
        self.compile_cache.borrow().len()
    }
}

impl Default for WasmCpuDevice {
    fn default() -> Self {
        Self::new()
    }
}

impl Allocator for WasmCpuDevice {
    fn alloc(&self, size_bytes: usize) -> Result<DeviceBuffer, DeviceError> {
        let buf = vec![0u8; size_bytes];
        Ok(DeviceBuffer {
            handle: BufferHandle::Cpu(buf),
            size_bytes,
        })
    }

    fn free(&self, _buf: DeviceBuffer) -> Result<(), DeviceError> {
        // Drop handles deallocation for CPU buffers.
        Ok(())
    }

    fn copy_in(&self, buf: &DeviceBuffer, data: &[u8]) -> Result<(), DeviceError> {
        match &buf.handle {
            BufferHandle::Cpu(inner) => {
                if data.len() > inner.len() {
                    return Err(DeviceError::InvalidArgument(format!(
                        "copy_in: data ({} bytes) exceeds buffer ({} bytes)",
                        data.len(),
                        inner.len()
                    )));
                }
                // SAFETY: Interior mutability for the WASM single-threaded backend.
                // The CPU backend uses this for testing only.
                let inner_ptr = inner.as_ptr() as *mut u8;
                unsafe {
                    core::ptr::copy_nonoverlapping(data.as_ptr(), inner_ptr, data.len());
                }
                Ok(())
            }
            #[cfg(target_os = "macos")]
            BufferHandle::Metal(_) => Err(DeviceError::InvalidArgument(
                "cannot copy_in to Metal buffer via WasmCpuDevice".into(),
            )),
        }
    }

    fn copy_out(&self, buf: &DeviceBuffer, data: &mut [u8]) -> Result<(), DeviceError> {
        match &buf.handle {
            BufferHandle::Cpu(inner) => {
                let len = data.len().min(inner.len());
                data[..len].copy_from_slice(&inner[..len]);
                Ok(())
            }
            #[cfg(target_os = "macos")]
            BufferHandle::Metal(_) => Err(DeviceError::InvalidArgument(
                "cannot copy_out from Metal buffer via WasmCpuDevice".into(),
            )),
        }
    }
}

impl Compiler for WasmCpuDevice {
    fn compile(&self, source: &str, entry: &str) -> Result<CompiledProgram, DeviceError> {
        let hash = Self::hash_source(source);

        // Check cache — return early if already compiled.
        {
            let cache = self.compile_cache.borrow();
            if let Some(cached_entry) = cache.get(&hash) {
                fn noop_kernel(_bufs: &[&[u8]], _out: &mut [u8], _num_elements: usize) {}
                return Ok(CompiledProgram {
                    handle: ProgramHandle::Cpu(noop_kernel as CpuKernelFn),
                    entry: cached_entry.clone(),
                });
            }
        }

        // WASM CPU device doesn't compile shader source — it interprets FusedKernel directly.
        fn noop_kernel(_bufs: &[&[u8]], _out: &mut [u8], _num_elements: usize) {}

        // Store in cache.
        self.compile_cache
            .borrow_mut()
            .insert(hash, entry.to_string());

        Ok(CompiledProgram {
            handle: ProgramHandle::Cpu(noop_kernel as CpuKernelFn),
            entry: entry.to_string(),
        })
    }

    fn max_local_size(&self) -> [u32; 3] {
        // WASM has no hardware threads — local size is effectively 1.
        [1, 1, 1]
    }

    fn max_grid_size(&self) -> [u32; 3] {
        // WASM linear memory limits apply, but grid dispatch is logical.
        [u32::MAX, 1, 1]
    }
}

impl Executor for WasmCpuDevice {
    fn exec(
        &self,
        _prog: &CompiledProgram,
        _bufs: &[&DeviceBuffer],
        _grid: [u32; 3],
        _local: [u32; 3],
    ) -> Result<(), DeviceError> {
        // WASM CPU execution is done through the interpret_kernel method, not exec.
        Ok(())
    }

    fn synchronize(&self) -> Result<(), DeviceError> {
        // WASM is single-threaded and synchronous — nothing to wait for.
        Ok(())
    }
}

/// WASM-compatible kernel interpreter — same correctness as `cpu::interpret`
/// but without any std features unavailable on wasm32.
///
/// Re-exports from `cpu::interpret` since the interpreter itself uses no
/// WASM-incompatible APIs (no Mutex, no threads, no file I/O, no std::time).
/// The interpreter operates on plain `Vec<u8>` buffers with pure arithmetic.
pub mod interpret {
    pub use crate::device::cpu::interpret::execute_kernel;
}
