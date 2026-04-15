//! WebGpuDevice — WebGPU backend via the `wgpu` crate.
//!
//! Implements Allocator, Compiler, and Executor for WebGPU (native + browser).
//! Uses WGSL shaders rendered by `WgslRenderer`.

#![cfg(feature = "webgpu-backend")]

use std::collections::HashMap;
use std::sync::Mutex;

use wgpu::{
    BindGroupDescriptor, BindGroupEntry, BindGroupLayoutDescriptor,
    BindGroupLayoutEntry, BindingType, BufferBindingType, BufferDescriptor,
    BufferUsages, ComputePassDescriptor, ComputePipelineDescriptor,
    DeviceDescriptor, Instance, Limits, PipelineLayoutDescriptor,
    RequestAdapterOptions, ShaderModuleDescriptor, ShaderStages,
};

use crate::device::{
    Allocator, BufferHandle, Compiler, CompiledProgram, DeviceBuffer,
    DeviceError, Executor, ProgramHandle,
};

/// Compiled WebGPU pipeline with its bind group layout.
struct WgpuPipeline {
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
}

/// WebGPU device backend via the `wgpu` crate.
///
/// Supports both native and browser targets through WGSL shaders.
pub struct WebGpuDevice {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// Buffer storage: maps buffer pointer address to wgpu::Buffer.
    live_buffers: Mutex<HashMap<usize, wgpu::Buffer>>,
    /// Compiled pipeline cache: source hash -> pipeline + layout.
    pipelines: Mutex<HashMap<u64, WgpuPipeline>>,
    /// Counter for unique buffer IDs.
    next_buf_id: Mutex<usize>,
}

impl WebGpuDevice {
    /// Create a new WebGPU device. Blocks until adapter + device are acquired.
    pub fn new() -> Result<Self, DeviceError> {
        let instance = Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .ok_or_else(|| DeviceError::AllocationFailed("no WebGPU adapter found".into()))?;

        let (device, queue) = pollster::block_on(adapter.request_device(
            &DeviceDescriptor {
                label: Some("molt-gpu"),
                required_features: wgpu::Features::empty(),
                required_limits: Limits::default(),
                memory_hints: wgpu::MemoryHints::Performance,
            },
            None,
        ))
        .map_err(|e| DeviceError::AllocationFailed(format!("WebGPU device request failed: {}", e)))?;

        Ok(Self {
            device,
            queue,
            live_buffers: Mutex::new(HashMap::new()),
            pipelines: Mutex::new(HashMap::new()),
            next_buf_id: Mutex::new(1),
        })
    }

    fn hash_source(source: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        source.hash(&mut hasher);
        hasher.finish()
    }

    /// Get next unique buffer ID.
    fn next_id(&self) -> usize {
        let mut id = self.next_buf_id.lock().unwrap();
        let val = *id;
        *id += 1;
        val
    }
}

impl Allocator for WebGpuDevice {
    fn alloc(&self, size_bytes: usize) -> Result<DeviceBuffer, DeviceError> {
        let buffer = self.device.create_buffer(&BufferDescriptor {
            label: Some("molt-gpu-buffer"),
            size: size_bytes as u64,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let id = self.next_id();
        self.live_buffers.lock().unwrap().insert(id, buffer);

        Ok(DeviceBuffer {
            handle: BufferHandle::Cpu(id.to_le_bytes().to_vec()),
            size_bytes,
        })
    }

    fn free(&self, buf: DeviceBuffer) -> Result<(), DeviceError> {
        let id = self.extract_id(&buf)?;
        let mut live = self.live_buffers.lock().unwrap();
        if let Some(wgpu_buf) = live.remove(&id) {
            wgpu_buf.destroy();
        }
        Ok(())
    }

    fn copy_in(&self, buf: &DeviceBuffer, data: &[u8]) -> Result<(), DeviceError> {
        let id = self.extract_id(buf)?;
        let live = self.live_buffers.lock().unwrap();
        let wgpu_buf = live.get(&id)
            .ok_or_else(|| DeviceError::InvalidArgument("buffer not found".into()))?;
        self.queue.write_buffer(wgpu_buf, 0, &data[..data.len().min(buf.size_bytes)]);
        Ok(())
    }

    fn copy_out(&self, buf: &DeviceBuffer, data: &mut [u8]) -> Result<(), DeviceError> {
        let id = self.extract_id(buf)?;
        let live = self.live_buffers.lock().unwrap();
        let wgpu_buf = live.get(&id)
            .ok_or_else(|| DeviceError::InvalidArgument("buffer not found".into()))?;

        let len = data.len().min(buf.size_bytes) as u64;

        // Create a staging buffer for readback
        let staging = self.device.create_buffer(&BufferDescriptor {
            label: Some("molt-gpu-staging"),
            size: len,
            usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("molt-gpu-copy-out"),
        });
        encoder.copy_buffer_to_buffer(wgpu_buf, 0, &staging, 0, len);
        self.queue.submit(std::iter::once(encoder.finish()));

        // Map the staging buffer and read back
        let slice = staging.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        self.device.poll(wgpu::Maintain::Wait);
        receiver.recv()
            .map_err(|_| DeviceError::ExecutionFailed("buffer map channel closed".into()))?
            .map_err(|e| DeviceError::ExecutionFailed(format!("buffer map failed: {}", e)))?;

        let mapped = slice.get_mapped_range();
        data[..len as usize].copy_from_slice(&mapped[..len as usize]);
        drop(mapped);
        staging.unmap();
        staging.destroy();

        Ok(())
    }
}

impl WebGpuDevice {
    fn extract_id(&self, buf: &DeviceBuffer) -> Result<usize, DeviceError> {
        match &buf.handle {
            BufferHandle::Cpu(bytes) => {
                if bytes.len() >= std::mem::size_of::<usize>() {
                    Ok(usize::from_le_bytes(bytes[..std::mem::size_of::<usize>()].try_into().unwrap()))
                } else {
                    Err(DeviceError::InvalidArgument("invalid WebGPU buffer handle".into()))
                }
            }
            #[cfg(target_os = "macos")]
            BufferHandle::Metal(_) => {
                Err(DeviceError::InvalidArgument("not a WebGPU buffer".into()))
            }
        }
    }
}

impl Compiler for WebGpuDevice {
    fn compile(&self, source: &str, _entry: &str) -> Result<CompiledProgram, DeviceError> {
        let hash = Self::hash_source(source);

        // Check cache
        {
            let cache = self.pipelines.lock().unwrap();
            if cache.contains_key(&hash) {
                return Ok(CompiledProgram {
                    handle: ProgramHandle::Cpu(|_bufs: &[&[u8]], _out: &mut [u8], _n: usize| {}),
                    entry: format!("{}", hash),
                });
            }
        }

        let shader_module = self.device.create_shader_module(ShaderModuleDescriptor {
            label: Some("molt-gpu-shader"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });

        // Create bind group layout dynamically based on source
        // Count @binding annotations to determine buffer count
        let binding_count = source.matches("@binding(").count();
        let entries: Vec<BindGroupLayoutEntry> = (0..binding_count)
            .map(|i| BindGroupLayoutEntry {
                binding: i as u32,
                visibility: ShaderStages::COMPUTE,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            })
            .collect();

        let bind_group_layout = self.device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("molt-gpu-bind-group-layout"),
            entries: &entries,
        });

        let pipeline_layout = self.device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("molt-gpu-pipeline-layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = self.device.create_compute_pipeline(&ComputePipelineDescriptor {
            label: Some("molt-gpu-pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader_module,
            entry_point: Some("molt_kernel"),
            compilation_options: Default::default(),
            cache: None,
        });

        self.pipelines.lock().unwrap().insert(hash, WgpuPipeline {
            pipeline,
            bind_group_layout,
        });

        Ok(CompiledProgram {
            handle: ProgramHandle::Cpu(|_bufs: &[&[u8]], _out: &mut [u8], _n: usize| {}),
            entry: format!("{}", hash),
        })
    }

    fn max_local_size(&self) -> [u32; 3] {
        [256, 256, 64]
    }

    fn max_grid_size(&self) -> [u32; 3] {
        [65535, 65535, 65535]
    }
}

impl Executor for WebGpuDevice {
    fn exec(
        &self,
        prog: &CompiledProgram,
        bufs: &[&DeviceBuffer],
        grid: [u32; 3],
        _local: [u32; 3],
    ) -> Result<(), DeviceError> {
        let hash: u64 = prog.entry.parse()
            .map_err(|_| DeviceError::InvalidArgument("invalid program hash".into()))?;

        let pipelines = self.pipelines.lock().unwrap();
        let wgpu_pipeline = pipelines.get(&hash)
            .ok_or_else(|| DeviceError::InvalidArgument("pipeline not found".into()))?;

        let live = self.live_buffers.lock().unwrap();

        // Build bind group entries
        let mut bind_entries = Vec::with_capacity(bufs.len());
        let mut wgpu_bufs = Vec::with_capacity(bufs.len());
        for buf in bufs {
            let id = match &buf.handle {
                BufferHandle::Cpu(bytes) => {
                    usize::from_le_bytes(bytes[..std::mem::size_of::<usize>()].try_into().unwrap())
                }
                #[cfg(target_os = "macos")]
                _ => return Err(DeviceError::InvalidArgument("not a WebGPU buffer".into())),
            };
            let wgpu_buf = live.get(&id)
                .ok_or_else(|| DeviceError::InvalidArgument("buffer not found in exec".into()))?;
            wgpu_bufs.push(wgpu_buf);
        }

        for (i, wgpu_buf) in wgpu_bufs.iter().enumerate() {
            bind_entries.push(BindGroupEntry {
                binding: i as u32,
                resource: wgpu_buf.as_entire_binding(),
            });
        }

        let bind_group = self.device.create_bind_group(&BindGroupDescriptor {
            label: Some("molt-gpu-bind-group"),
            layout: &wgpu_pipeline.bind_group_layout,
            entries: &bind_entries,
        });

        drop(live);

        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("molt-gpu-compute"),
        });

        {
            let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
                label: Some("molt-gpu-pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&wgpu_pipeline.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(grid[0], grid[1], grid[2]);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        Ok(())
    }

    fn synchronize(&self) -> Result<(), DeviceError> {
        self.device.poll(wgpu::Maintain::Wait);
        Ok(())
    }
}
