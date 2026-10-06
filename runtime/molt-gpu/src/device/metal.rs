//! MetalDevice — Apple GPU backend.
//!
//! Implements Allocator, Compiler, and Executor for Metal on macOS through
//! the maintained `objc2-metal` bindings. Device pool and kernel cache are
//! internal to this struct.

#![cfg(target_os = "macos")]

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Mutex;

use objc2::Message;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::{NSError, NSString};
use objc2_metal::{
    MTLBuffer, MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue, MTLComputeCommandEncoder,
    MTLComputePipelineState, MTLCreateSystemDefaultDevice, MTLDevice, MTLLibrary,
    MTLResourceOptions, MTLSize,
};

use crate::device::{
    Allocator, BufferHandle, CompiledProgram, Compiler, DeviceBuffer, DeviceError, Executor,
    ProgramHandle,
};

type Device = Retained<ProtocolObject<dyn MTLDevice>>;
type CommandQueue = Retained<ProtocolObject<dyn MTLCommandQueue>>;
type Buffer = Retained<ProtocolObject<dyn MTLBuffer>>;
type PipelineState = Retained<ProtocolObject<dyn MTLComputePipelineState>>;

/// Apple Metal GPU device backend.
///
/// Manages Metal buffer allocation, MSL shader compilation with caching,
/// and kernel dispatch via command buffers.
pub struct MetalDevice {
    device: Device,
    queue: CommandQueue,
    /// Compiled pipeline state cache: source hash -> pipeline state.
    cache: Mutex<HashMap<u64, PipelineState>>,
    /// Live Metal buffers: object address -> retained buffer (prevents premature drop).
    live_buffers: Mutex<HashMap<usize, Buffer>>,
}

// SAFETY: Metal devices, command queues, buffers and pipeline states are
// thread-safe objects (Metal Programming Guide, "Thread Safety"); only command
// buffers and encoders are not, and this device creates those inside one call
// and never stores them. The retained maps are behind mutexes.
unsafe impl Send for MetalDevice {}
unsafe impl Sync for MetalDevice {}

/// The stable identity of a retained Metal object: its Objective-C address.
fn object_address<T: ?Sized + Message>(object: &Retained<T>) -> *mut c_void {
    Retained::as_ptr(object).cast_mut().cast()
}

fn error_text(error: &NSError) -> String {
    error.localizedDescription().to_string()
}

impl MetalDevice {
    /// Create a new Metal device from the system default GPU.
    pub fn new() -> Result<Self, DeviceError> {
        let device = MTLCreateSystemDefaultDevice()
            .ok_or_else(|| DeviceError::AllocationFailed("no Metal device found".into()))?;
        let queue = device.newCommandQueue().ok_or_else(|| {
            DeviceError::AllocationFailed("Metal command queue creation failed".into())
        })?;
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
        // Metal rejects a zero-length buffer; a zero-sized tensor still owns a
        // handle, so it is backed by one byte while `size_bytes` stays exact.
        let buffer = self
            .device
            .newBufferWithLength_options(size_bytes.max(1), MTLResourceOptions::StorageModeShared)
            .ok_or_else(|| {
                DeviceError::AllocationFailed(format!(
                    "Metal buffer allocation of {size_bytes} bytes failed"
                ))
            })?;
        let ptr = object_address(&buffer);
        let key = ptr as usize;

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
                let mtl_buf = live
                    .get(&key)
                    .ok_or_else(|| DeviceError::InvalidArgument("buffer not found".into()))?;
                let contents = mtl_buf.contents().as_ptr().cast::<u8>();
                // SAFETY: `contents()` of a shared-mode buffer is CPU-visible for
                // the buffer's whole lifetime, which `live_buffers` holds. The
                // copy length is clamped to the buffer size, so the write stays
                // in bounds, and shared-mode memory needs no synchronization.
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        data.as_ptr(),
                        contents,
                        data.len().min(buf.size_bytes),
                    );
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
                let mtl_buf = live
                    .get(&key)
                    .ok_or_else(|| DeviceError::InvalidArgument("buffer not found".into()))?;
                let contents = mtl_buf.contents().as_ptr().cast::<u8>().cast_const();
                let len = data.len().min(buf.size_bytes);
                // SAFETY: `contents()` of a shared-mode buffer is CPU-visible for
                // the buffer's whole lifetime, which `live_buffers` holds.
                // `synchronize()` above completed every queued GPU write, and the
                // copy length is the minimum of the buffer and output sizes.
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
                return Ok(CompiledProgram {
                    handle: ProgramHandle::Metal(object_address(pso)),
                    entry: entry.to_string(),
                });
            }
        }

        // Compile MSL source with the default compile options.
        let library = self
            .device
            .newLibraryWithSource_options_error(&NSString::from_str(source), None)
            .map_err(|error| DeviceError::CompilationFailed(error_text(&error)))?;

        let function = library
            .newFunctionWithName(&NSString::from_str(entry))
            .ok_or_else(|| {
                DeviceError::CompilationFailed(format!(
                    "function '{entry}': not found in the compiled library"
                ))
            })?;

        let pso = self
            .device
            .newComputePipelineStateWithFunction_error(&function)
            .map_err(|error| DeviceError::CompilationFailed(error_text(&error)))?;

        let ptr = object_address(&pso);

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
        let command_buffer = self.queue.commandBuffer().ok_or_else(|| {
            DeviceError::ExecutionFailed("Metal command buffer creation failed".into())
        })?;
        let encoder = command_buffer.computeCommandEncoder().ok_or_else(|| {
            DeviceError::ExecutionFailed("Metal compute command encoder creation failed".into())
        })?;

        // Set pipeline state from cached PSO
        match &prog.handle {
            ProgramHandle::Metal(ptr) => {
                // SAFETY: `compile` produced this address from a pipeline state
                // that `self.cache` retains for the device's lifetime, so the
                // object is alive for this borrow; the encoder retains it on its
                // own for the command buffer's lifetime.
                let pso = unsafe { &*ptr.cast::<ProtocolObject<dyn MTLComputePipelineState>>() };
                encoder.setComputePipelineState(pso);
            }
            _ => return Err(DeviceError::InvalidArgument("not a Metal program".into())),
        }

        // Bind buffers
        let live = self.live_buffers.lock().unwrap();
        for (i, buf) in bufs.iter().enumerate() {
            match &buf.handle {
                BufferHandle::Metal(ptr) => {
                    let key = *ptr as usize;
                    let mtl_buf = live
                        .get(&key)
                        .ok_or_else(|| DeviceError::InvalidArgument("buffer not found".into()))?;
                    // SAFETY: the buffer is a live object of this device, bound
                    // at offset 0 inside its own length; the encoder retains it.
                    unsafe { encoder.setBuffer_offset_atIndex(Some(&**mtl_buf), 0, i) };
                }
                _ => return Err(DeviceError::InvalidArgument("not a Metal buffer".into())),
            }
        }
        drop(live);

        // Dispatch. `grid` is the number of THREADGROUPS (tinygrad's dispatch
        // model, and exactly what `schedule::specialize_shapes` computes —
        // `grid_x = ceil(total / local)`). `dispatchThreadgroups` launches
        // `grid * local` total threads, the kernel guarding the `gid >= total`
        // tail. The previous `dispatch_threads` treated `grid` as a raw thread
        // count, so a specialized kernel whose grid is the threadgroup count
        // launched only `ceil(total/local)` threads (e.g. 16 of 1024 elements)
        // and silently left the rest unwritten.
        let threadgroups = MTLSize {
            width: grid[0] as usize,
            height: grid[1] as usize,
            depth: grid[2] as usize,
        };
        let threads_per_group = MTLSize {
            width: local[0] as usize,
            height: local[1] as usize,
            depth: local[2] as usize,
        };
        encoder.dispatchThreadgroups_threadsPerThreadgroup(threadgroups, threads_per_group);
        encoder.endEncoding();
        command_buffer.commit();

        Ok(())
    }

    fn synchronize(&self) -> Result<(), DeviceError> {
        let command_buffer = self.queue.commandBuffer().ok_or_else(|| {
            DeviceError::ExecutionFailed("Metal command buffer creation failed".into())
        })?;
        command_buffer.commit();
        command_buffer.waitUntilCompleted();
        Ok(())
    }
}
