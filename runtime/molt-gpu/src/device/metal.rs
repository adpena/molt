//! MetalDevice — Apple GPU backend.
//!
//! Implements Allocator, Compiler, and Executor for Metal on macOS.
//! Device pool and kernel cache are internal to this struct.

#![cfg(target_os = "macos")]

use std::collections::HashMap;
use std::sync::Mutex;

use metal::foreign_types::ForeignType;
use metal::{Device, MTLResourceOptions, MTLSize};

use crate::device::{
    Allocator, BufferHandle, Compiler, CompiledProgram, DeviceBuffer,
    DeviceError, Executor, ProgramHandle,
};

/// Apple Metal GPU device backend.
///
/// Manages Metal buffer allocation, MSL shader compilation with caching,
/// and kernel dispatch via command buffers.
pub struct MetalDevice {
    device: Device,
    queue: metal::CommandQueue,
    /// Compiled pipeline state cache: source hash -> pipeline state.
    cache: Mutex<HashMap<u64, metal::ComputePipelineState>>,
    /// Live Metal buffers: ptr -> retained Buffer (prevents premature drop).
    live_buffers: Mutex<HashMap<usize, metal::Buffer>>,
}

impl MetalDevice {
    /// Create a new Metal device from the system default GPU.
    pub fn new() -> Result<Self, DeviceError> {
        let device = Device::system_default()
            .ok_or_else(|| DeviceError::AllocationFailed("no Metal device found".into()))?;
        let queue = device.new_command_queue();
        Ok(Self {
            device,
            queue,
            cache: Mutex::new(HashMap::new()),
            live_buffers: Mutex::new(HashMap::new()),
        })
    }

    /// Hash shader source for cache lookup.
    fn hash_source(source: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        source.hash(&mut hasher);
        hasher.finish()
    }
}

impl Allocator for MetalDevice {
    fn alloc(&self, size_bytes: usize) -> Result<DeviceBuffer, DeviceError> {
        let buffer = self.device.new_buffer(
            size_bytes as u64,
            MTLResourceOptions::StorageModeShared,
        );
        let key = buffer.as_ptr() as usize;
        let ptr = buffer.as_ptr() as *mut std::ffi::c_void;

        // Keep buffer alive in our map
        self.live_buffers.lock().unwrap().insert(key, buffer);

        Ok(DeviceBuffer {
            handle: BufferHandle::Metal(ptr),
            size_bytes,
        })
    }

    fn free(&self, buf: DeviceBuffer) -> Result<(), DeviceError> {
        self.synchronize()?;
        match buf.handle {
            BufferHandle::Metal(ptr) => {
                let key = ptr as usize;
                self.live_buffers.lock().unwrap().remove(&key);
                Ok(())
            }
            _ => Err(DeviceError::InvalidArgument("not a Metal buffer".into())),
        }
    }

    fn copy_in(&self, buf: &DeviceBuffer, data: &[u8]) -> Result<(), DeviceError> {
        match &buf.handle {
            BufferHandle::Metal(ptr) => {
                let key = *ptr as usize;
                let live = self.live_buffers.lock().unwrap();
                let mtl_buf = live.get(&key)
                    .ok_or_else(|| DeviceError::InvalidArgument("buffer not found".into()))?;
                let contents = mtl_buf.contents() as *mut u8;
                // SAFETY: MTLBuffer::contents() returns a valid shared-mode pointer.
                // The copy length is clamped to the buffer size, preventing out-of-bounds writes.
                // Metal shared-mode buffers are CPU-accessible without synchronization.
                unsafe {
                    std::ptr::copy_nonoverlapping(data.as_ptr(), contents, data.len().min(buf.size_bytes));
                }
                Ok(())
            }
            _ => Err(DeviceError::InvalidArgument("not a Metal buffer".into())),
        }
    }

    fn copy_out(&self, buf: &DeviceBuffer, data: &mut [u8]) -> Result<(), DeviceError> {
        self.synchronize()?;
        match &buf.handle {
            BufferHandle::Metal(ptr) => {
                let key = *ptr as usize;
                let live = self.live_buffers.lock().unwrap();
                let mtl_buf = live.get(&key)
                    .ok_or_else(|| DeviceError::InvalidArgument("buffer not found".into()))?;
                let contents = mtl_buf.contents() as *const u8;
                let len = data.len().min(buf.size_bytes);
                // SAFETY: MTLBuffer::contents() returns a valid shared-mode pointer.
                // synchronize() was called above, guaranteeing all GPU writes are visible.
                // The copy length is clamped to the minimum of buffer and output sizes.
                unsafe {
                    std::ptr::copy_nonoverlapping(contents, data.as_mut_ptr(), len);
                }
                Ok(())
            }
            _ => Err(DeviceError::InvalidArgument("not a Metal buffer".into())),
        }
    }
}

impl Compiler for MetalDevice {
    fn compile(&self, source: &str, entry: &str) -> Result<CompiledProgram, DeviceError> {
        let hash = Self::hash_source(source);

        // Check cache
        {
            let cache = self.cache.lock().unwrap();
            if let Some(pso) = cache.get(&hash) {
                let ptr = pso.as_ptr() as *mut std::ffi::c_void;
                return Ok(CompiledProgram {
                    handle: ProgramHandle::Metal(ptr),
                    entry: entry.to_string(),
                });
            }
        }

        // Compile MSL source
        let options = metal::CompileOptions::new();
        let library = self
            .device
            .new_library_with_source(source, &options)
            .map_err(|e| DeviceError::CompilationFailed(e.to_string()))?;

        let function = library
            .get_function(entry, None)
            .map_err(|e| DeviceError::CompilationFailed(format!("function '{}': {}", entry, e)))?;

        let pso = self
            .device
            .new_compute_pipeline_state_with_function(&function)
            .map_err(|e| DeviceError::CompilationFailed(e.to_string()))?;

        let ptr = pso.as_ptr() as *mut std::ffi::c_void;

        // Cache (keeps the pso alive)
        self.cache.lock().unwrap().insert(hash, pso);

        Ok(CompiledProgram {
            handle: ProgramHandle::Metal(ptr),
            entry: entry.to_string(),
        })
    }

    fn max_local_size(&self) -> [u32; 3] {
        [1024, 1024, 1024]
    }

    fn max_grid_size(&self) -> [u32; 3] {
        [u32::MAX, u32::MAX, u32::MAX]
    }
}

impl Executor for MetalDevice {
    fn exec(
        &self,
        prog: &CompiledProgram,
        bufs: &[&DeviceBuffer],
        grid: [u32; 3],
        local: [u32; 3],
    ) -> Result<(), DeviceError> {
        let command_buffer = self.queue.new_command_buffer();
        let encoder = command_buffer.new_compute_command_encoder();

        // Set pipeline state from cached PSO
        match &prog.handle {
            ProgramHandle::Metal(ptr) => {
                // SAFETY: The pointer was obtained from a cached ComputePipelineState
                // that remains alive in self.cache for the device's lifetime.
                // We reconstruct a temporary handle, use it, then forget it to
                // prevent double-free since the cache owns the underlying object.
                unsafe {
                    let pso = metal::ComputePipelineState::from_ptr(*ptr as *mut _);
                    encoder.set_compute_pipeline_state(&pso);
                    std::mem::forget(pso);
                }
            }
            _ => return Err(DeviceError::InvalidArgument("not a Metal program".into())),
        }

        // Bind buffers
        let live = self.live_buffers.lock().unwrap();
        for (i, buf) in bufs.iter().enumerate() {
            match &buf.handle {
                BufferHandle::Metal(ptr) => {
                    let key = *ptr as usize;
                    let mtl_buf = live.get(&key)
                        .ok_or_else(|| DeviceError::InvalidArgument("buffer not found".into()))?;
                    encoder.set_buffer(i as u64, Some(mtl_buf), 0);
                }
                _ => return Err(DeviceError::InvalidArgument("not a Metal buffer".into())),
            }
        }
        drop(live);

        // Dispatch
        let grid_size = MTLSize::new(grid[0] as u64, grid[1] as u64, grid[2] as u64);
        let local_size = MTLSize::new(local[0] as u64, local[1] as u64, local[2] as u64);
        encoder.dispatch_threads(grid_size, local_size);
        encoder.end_encoding();
        command_buffer.commit();

        Ok(())
    }

    fn synchronize(&self) -> Result<(), DeviceError> {
        let command_buffer = self.queue.new_command_buffer();
        command_buffer.commit();
        command_buffer.wait_until_completed();
        Ok(())
    }
}
