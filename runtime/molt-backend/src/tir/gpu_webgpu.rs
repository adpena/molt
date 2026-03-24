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
        let module = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(wgsl_source.into()),
        });
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
        // Validate WGSL by creating the shader module; store source bytes as
        // opaque handle so `launch_kernel` can recreate the module.
        let _module = self.compile_wgsl(name, source)?;
        Ok(CompiledKernel::new(
            name.to_string(),
            GpuPlatform::WebGpu,
            source.as_bytes().to_vec(),
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
        let id = Self::buffer_id_from_handle(buffer)?;
        let registry = self.buffers.lock().unwrap();
        let wgpu_buf = registry
            .get(&id)
            .ok_or_else(|| GpuError::TransferFailed(format!("Unknown buffer id {id}")))?;
        self.queue.write_buffer(wgpu_buf, 0, data);
        Ok(())
    }

    fn copy_from_device(
        &self,
        buffer: &GpuBufferHandle,
        data: &mut [u8],
    ) -> Result<(), GpuError> {
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
        let copy_len = data.len().min(mapped.len());
        data[..copy_len].copy_from_slice(&mapped[..copy_len]);
        Ok(())
    }

    fn launch_kernel(
        &self,
        kernel: &CompiledKernel,
        grid: [u32; 3],
        _block: [u32; 3],
        buffers: &[&GpuBufferHandle],
    ) -> Result<(), GpuError> {
        // Retrieve the WGSL source stored in the opaque kernel handle by compile_kernel.
        let wgsl_source = kernel
            .wgsl_source()
            .ok_or_else(|| GpuError::LaunchFailed("No WGSL source in kernel handle".into()))?;

        let shader = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(wgsl_source.into()),
        });

        let pipeline = self
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: None,
                layout: None,
                module: &shader,
                entry_point: Some(&kernel.name),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });

        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: None,
                timestamp_writes: None,
            });
            pass.set_pipeline(&pipeline);

            // Only create a bind group when there are buffers to bind.
            if !buffers.is_empty() {
                let bind_group_layout = pipeline.get_bind_group_layout(0);
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
        self.buffers.lock().unwrap().remove(&id);
        Ok(())
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
impl WebGpuDevice {
    pub fn new() -> Result<Self, GpuError> {
        Ok(Self {
            _phantom: std::marker::PhantomData,
        })
    }

    pub fn compile_wgsl(&self, name: &str, wgsl_source: &str) -> Result<CompiledKernel, GpuError> {
        Ok(CompiledKernel::new(
            name.to_string(),
            GpuPlatform::WebGpu,
            wgsl_source.as_bytes().to_vec(),
        ))
    }
}

#[cfg(not(feature = "gpu-webgpu"))]
impl GpuDevice for WebGpuDevice {
    fn compile_kernel(&self, name: &str, source: &str) -> Result<CompiledKernel, GpuError> {
        self.compile_wgsl(name, source)
    }
    fn alloc_buffer(&self, size_bytes: usize) -> Result<GpuBufferHandle, GpuError> {
        Ok(GpuBufferHandle::new(size_bytes, GpuPlatform::WebGpu, vec![0; 8]))
    }
    fn copy_to_device(&self, _buffer: &GpuBufferHandle, _data: &[u8]) -> Result<(), GpuError> {
        Ok(())
    }
    fn copy_from_device(
        &self,
        _buffer: &GpuBufferHandle,
        _data: &mut [u8],
    ) -> Result<(), GpuError> {
        Ok(())
    }
    fn launch_kernel(
        &self,
        _kernel: &CompiledKernel,
        _grid: [u32; 3],
        _block: [u32; 3],
        _buffers: &[&GpuBufferHandle],
    ) -> Result<(), GpuError> {
        Ok(())
    }
    fn synchronize(&self) -> Result<(), GpuError> {
        Ok(())
    }
    fn free_buffer(&self, _buffer: GpuBufferHandle) -> Result<(), GpuError> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::gpu_runtime::GpuDevice;

    #[test]
    fn webgpu_device_implements_gpu_device_trait() {
        let device = WebGpuDevice::new().expect("WebGpuDevice::new should succeed");
        let buf = device.alloc_buffer(512).expect("alloc_buffer should succeed");
        assert_eq!(buf.size_bytes, 512);
        assert_eq!(buf.platform, GpuPlatform::WebGpu);
    }

    #[test]
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
}
