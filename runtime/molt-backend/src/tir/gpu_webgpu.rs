//! WebGPU device implementation.
//! Works natively via wgpu crate or in browsers via WebGPU API.
//!
//! Feature `gpu-webgpu`: real wgpu dispatch using the wgpu + pollster crates.
//! Without the feature: stub implementation (all operations succeed as no-ops).

use super::gpu_runtime::*;

// ---------------------------------------------------------------------------
// Real wgpu implementation (feature = "gpu-webgpu")
// ---------------------------------------------------------------------------

#[cfg(feature = "gpu-webgpu")]
use std::collections::HashMap;
#[cfg(feature = "gpu-webgpu")]
use std::sync::{Arc, Mutex};

/// Internal buffer registry — maps a u64 ID stored in `GpuBufferHandle._handle`
/// to the live `wgpu::Buffer`.
#[cfg(feature = "gpu-webgpu")]
type BufferRegistry = Arc<Mutex<HashMap<u64, wgpu::Buffer>>>;

#[cfg(feature = "gpu-webgpu")]
struct WebGpuPipeline {
    #[allow(dead_code)]
    shader: wgpu::ShaderModule,
    pipeline: wgpu::ComputePipeline,
}

#[cfg(feature = "gpu-webgpu")]
pub struct WebGpuDevice {
    device: wgpu::Device,
    queue: wgpu::Queue,
    buffers: BufferRegistry,
    next_id: Mutex<u64>,
}

#[cfg(feature = "gpu-webgpu")]
impl WebGpuDevice {
    pub fn new() -> Result<Self, GpuError> {
        pollster::block_on(async {
            // wgpu 29: use `new_without_display_handle()` — `default()` is not implemented.
            let instance =
                wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .map_err(|e| GpuError::DeviceNotAvailable(e.to_string()))?;
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor::default())
                .await
                .map_err(|e| GpuError::DeviceNotAvailable(e.to_string()))?;
            Ok(Self {
                device,
                queue,
                buffers: Arc::new(Mutex::new(HashMap::new())),
                next_id: Mutex::new(1),
            })
        })
    }

    /// Compile WGSL source into a `wgpu::ShaderModule`.
    pub fn compile_wgsl(
        &self,
        _name: &str,
        wgsl_source: &str,
    ) -> Result<wgpu::ShaderModule, GpuError> {
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: None,
                source: wgpu::ShaderSource::Wgsl(wgsl_source.into()),
            });
        let validation = pollster::block_on(scope.pop());
        if let Some(err) = validation {
            return Err(GpuError::CompilationFailed(err.to_string()));
        }
        Ok(module)
    }

    /// Allocate the next buffer ID (monotonically increasing).
    fn next_buffer_id(&self) -> u64 {
        let mut id = self.next_id.lock().unwrap();
        let current = *id;
        *id += 1;
        current
    }
}

#[cfg(feature = "gpu-webgpu")]
impl GpuDevice for WebGpuDevice {
    fn compile_kernel(&self, name: &str, source: &str) -> Result<CompiledKernel, GpuError> {
        let shader = self.compile_wgsl(name, source)?;
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let pipeline = self
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: None,
                layout: None,
                module: &shader,
                entry_point: Some(name),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            });
        let validation = pollster::block_on(scope.pop());
        if let Some(err) = validation {
            return Err(GpuError::CompilationFailed(err.to_string()));
        }
        let handle = Arc::new(WebGpuPipeline { shader, pipeline });
        let raw = Arc::into_raw(handle);
        Ok(CompiledKernel::new(
            name.to_string(),
            GpuPlatform::WebGpu,
            (raw as usize).to_ne_bytes().to_vec(),
        ))
    }

    fn alloc_buffer(&self, size_bytes: usize) -> Result<GpuBufferHandle, GpuError> {
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: size_bytes as u64,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let id = self.next_buffer_id();
        self.buffers.lock().unwrap().insert(id, buf);
        // Encode the u64 ID as little-endian bytes in the opaque handle slot.
        Ok(GpuBufferHandle::new(
            size_bytes,
            GpuPlatform::WebGpu,
            id.to_le_bytes().to_vec(),
        ))
    }

    fn copy_to_device(&self, buffer: &GpuBufferHandle, data: &[u8]) -> Result<(), GpuError> {
        if data.len() > buffer.size_bytes {
            return Err(GpuError::TransferFailed(format!(
                "Host write ({}) exceeds device buffer size ({})",
                data.len(),
                buffer.size_bytes
            )));
        }
        let id = Self::buffer_id_from_handle(buffer)?;
        let registry = self.buffers.lock().unwrap();
        let wgpu_buf = registry
            .get(&id)
            .ok_or_else(|| GpuError::TransferFailed(format!("Unknown buffer id {id}")))?;
        self.queue.write_buffer(wgpu_buf, 0, data);
        Ok(())
    }

    fn copy_from_device(&self, buffer: &GpuBufferHandle, data: &mut [u8]) -> Result<(), GpuError> {
        let id = Self::buffer_id_from_handle(buffer)?;
        let size = buffer.size_bytes as u64;

        // Create a staging (MAP_READ) buffer.
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("staging_readback"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });

        {
            let registry = self.buffers.lock().unwrap();
            let src = registry
                .get(&id)
                .ok_or_else(|| GpuError::TransferFailed(format!("Unknown buffer id {id}")))?;
            encoder.copy_buffer_to_buffer(src, 0, &staging, 0, size);
        }

        self.queue.submit(Some(encoder.finish()));

        // Map the staging buffer synchronously.
        let slice = staging.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |res| {
            let _ = tx.send(res);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| GpuError::TransferFailed(format!("poll error: {e}")))?;
        rx.recv()
            .map_err(|_| GpuError::TransferFailed("map channel dropped".into()))?
            .map_err(|e| GpuError::TransferFailed(e.to_string()))?;

        let mapped = slice.get_mapped_range();
        if data.len() > mapped.len() {
            return Err(GpuError::TransferFailed(format!(
                "Host read buffer ({}) exceeds mapped device bytes ({})",
                data.len(),
                mapped.len()
            )));
        }
        data.copy_from_slice(&mapped[..data.len()]);
        drop(mapped);
        staging.unmap();
        Ok(())
    }

    fn launch_kernel(
        &self,
        kernel: &CompiledKernel,
        grid: [u32; 3],
        _block: [u32; 3],
        buffers: &[&GpuBufferHandle],
    ) -> Result<(), GpuError> {
        if kernel.platform != GpuPlatform::WebGpu {
            return Err(GpuError::LaunchFailed(
                "Kernel is not a WebGPU kernel".into(),
            ));
        }
        let ptr_val = usize::from_ne_bytes(
            kernel
                ._handle_bytes()
                .try_into()
                .map_err(|_| GpuError::LaunchFailed("Invalid kernel handle".into()))?,
        );
        let arc: Arc<WebGpuPipeline> = unsafe { Arc::from_raw(ptr_val as *const WebGpuPipeline) };
        let pipeline_arc = Arc::clone(&arc);
        std::mem::forget(arc);

        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });

        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: None,
                timestamp_writes: None,
            });
            pass.set_pipeline(&pipeline_arc.pipeline);

            // Only create a bind group when there are buffers to bind.
            if !buffers.is_empty() {
                let bind_group_layout = pipeline_arc.pipeline.get_bind_group_layout(0);
                let registry = self.buffers.lock().unwrap();
                let mut bg_entries: Vec<wgpu::BindGroupEntry<'_>> =
                    Vec::with_capacity(buffers.len());
                for (i, handle) in buffers.iter().enumerate() {
                    let id = Self::buffer_id_from_handle(handle)
                        .map_err(|e| GpuError::LaunchFailed(format!("Buffer {i}: {e}")))?;
                    let wgpu_buf = registry.get(&id).ok_or_else(|| {
                        GpuError::LaunchFailed(format!("Unknown buffer id {id} at slot {i}"))
                    })?;
                    bg_entries.push(wgpu::BindGroupEntry {
                        binding: i as u32,
                        resource: wgpu_buf.as_entire_binding(),
                    });
                }
                let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &bind_group_layout,
                    entries: &bg_entries,
                });
                pass.set_bind_group(0, &bind_group, &[]);
            }

            pass.dispatch_workgroups(grid[0], grid[1], grid[2]);
        }

        self.queue.submit(Some(encoder.finish()));
        let validation = pollster::block_on(scope.pop());
        if let Some(err) = validation {
            return Err(GpuError::LaunchFailed(err.to_string()));
        }
        Ok(())
    }

    fn synchronize(&self) -> Result<(), GpuError> {
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| GpuError::LaunchFailed(format!("synchronize poll error: {e}")))?;
        Ok(())
    }

    fn free_buffer(&self, buffer: GpuBufferHandle) -> Result<(), GpuError> {
        let id = Self::buffer_id_from_handle(&buffer)?;
        self.buffers
            .lock()
            .unwrap()
            .remove(&id)
            .map(|_| ())
            .ok_or_else(|| GpuError::TransferFailed(format!("Unknown buffer id {id}")))
    }
}

#[cfg(feature = "gpu-webgpu")]
impl WebGpuDevice {
    /// Decode the opaque buffer ID from a `GpuBufferHandle`'s handle bytes.
    fn buffer_id_from_handle(handle: &GpuBufferHandle) -> Result<u64, GpuError> {
        handle
            .buffer_id_u64()
            .ok_or_else(|| GpuError::TransferFailed("Invalid buffer handle".into()))
    }
}

// ---------------------------------------------------------------------------
// Stub implementation (no feature = "gpu-webgpu")
// ---------------------------------------------------------------------------

#[cfg(not(feature = "gpu-webgpu"))]
pub struct WebGpuDevice {
    _phantom: std::marker::PhantomData<()>,
}

#[cfg(not(feature = "gpu-webgpu"))]
const WEBGPU_STUB_MSG: &str = "WebGPU support requires the `gpu-webgpu` feature";

#[cfg(not(feature = "gpu-webgpu"))]
impl WebGpuDevice {
    pub fn new() -> Result<Self, GpuError> {
        Err(GpuError::DeviceNotAvailable(WEBGPU_STUB_MSG.into()))
    }

    pub fn compile_wgsl(
        &self,
        _name: &str,
        _wgsl_source: &str,
    ) -> Result<CompiledKernel, GpuError> {
        Err(GpuError::DeviceNotAvailable(WEBGPU_STUB_MSG.into()))
    }
}

#[cfg(not(feature = "gpu-webgpu"))]
impl GpuDevice for WebGpuDevice {
    fn compile_kernel(&self, _name: &str, _source: &str) -> Result<CompiledKernel, GpuError> {
        Err(GpuError::DeviceNotAvailable(WEBGPU_STUB_MSG.into()))
    }
    fn alloc_buffer(&self, _size_bytes: usize) -> Result<GpuBufferHandle, GpuError> {
        Err(GpuError::DeviceNotAvailable(WEBGPU_STUB_MSG.into()))
    }
    fn copy_to_device(&self, _buffer: &GpuBufferHandle, _data: &[u8]) -> Result<(), GpuError> {
        Err(GpuError::DeviceNotAvailable(WEBGPU_STUB_MSG.into()))
    }
    fn copy_from_device(
        &self,
        _buffer: &GpuBufferHandle,
        _data: &mut [u8],
    ) -> Result<(), GpuError> {
        Err(GpuError::DeviceNotAvailable(WEBGPU_STUB_MSG.into()))
    }
    fn launch_kernel(
        &self,
        _kernel: &CompiledKernel,
        _grid: [u32; 3],
        _block: [u32; 3],
        _buffers: &[&GpuBufferHandle],
    ) -> Result<(), GpuError> {
        Err(GpuError::DeviceNotAvailable(WEBGPU_STUB_MSG.into()))
    }
    fn synchronize(&self) -> Result<(), GpuError> {
        Err(GpuError::DeviceNotAvailable(WEBGPU_STUB_MSG.into()))
    }
    fn free_buffer(&self, _buffer: GpuBufferHandle) -> Result<(), GpuError> {
        Err(GpuError::DeviceNotAvailable(WEBGPU_STUB_MSG.into()))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "gpu-webgpu")]
    use crate::tir::gpu_runtime::GpuDevice;

    #[test]
    #[cfg(feature = "gpu-webgpu")]
    fn webgpu_device_implements_gpu_device_trait() {
        let device = WebGpuDevice::new().expect("WebGpuDevice::new should succeed");
        let buf = device
            .alloc_buffer(512)
            .expect("alloc_buffer should succeed");
        assert_eq!(buf.size_bytes, 512);
        assert_eq!(buf.platform, GpuPlatform::WebGpu);
    }

    #[test]
    #[cfg(not(feature = "gpu-webgpu"))]
    fn webgpu_device_stub_returns_error() {
        let result = WebGpuDevice::new();
        assert!(result.is_err(), "Stub WebGpuDevice::new should fail");
    }

    #[test]
    #[cfg(feature = "gpu-webgpu")]
    fn webgpu_device_compile_and_launch() {
        let device = WebGpuDevice::new().expect("WebGpuDevice::new should succeed");
        let wgsl = "@compute @workgroup_size(64) fn main() {}";
        let kernel = device
            .compile_kernel("main", wgsl)
            .expect("compile_kernel should succeed");
        assert_eq!(kernel.name, "main");
        assert_eq!(kernel.platform, GpuPlatform::WebGpu);

        device
            .launch_kernel(&kernel, [1, 1, 1], [64, 1, 1], &[])
            .expect("launch_kernel should not crash");
        device.synchronize().expect("synchronize should succeed");
    }

    #[test]
    #[cfg(feature = "gpu-webgpu")]
    fn webgpu_buffer_copy_roundtrip() {
        let device = WebGpuDevice::new().expect("WebGpuDevice::new should succeed");
        let buf = device.alloc_buffer(16).expect("alloc buffer");
        let input: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
        device.copy_to_device(&buf, &input).expect("copy to device");
        let mut output = [0u8; 16];
        device
            .copy_from_device(&buf, &mut output)
            .expect("copy from device");
        assert_eq!(output, input);
        device.free_buffer(buf).expect("free buffer");
    }

    #[test]
    #[cfg(feature = "gpu-webgpu")]
    fn webgpu_copy_to_device_rejects_oversized_write() {
        let device = WebGpuDevice::new().expect("WebGpuDevice::new should succeed");
        let buf = device.alloc_buffer(4).expect("alloc buffer");
        let too_large = [1u8; 8];
        let err = device
            .copy_to_device(&buf, &too_large)
            .expect_err("oversized write should fail");
        let msg = err.to_string();
        assert!(
            msg.contains("exceeds device buffer size"),
            "unexpected error: {msg}"
        );
        device.free_buffer(buf).expect("free buffer");
    }

    #[test]
    #[cfg(feature = "gpu-webgpu")]
    fn webgpu_free_buffer_rejects_unknown_handle() {
        let device = WebGpuDevice::new().expect("WebGpuDevice::new should succeed");
        let unknown =
            GpuBufferHandle::new(16, GpuPlatform::WebGpu, (999_999u64).to_le_bytes().to_vec());
        let err = device
            .free_buffer(unknown)
            .expect_err("free unknown buffer should fail");
        let msg = err.to_string();
        assert!(msg.contains("Unknown buffer id"), "unexpected error: {msg}");
    }
}
