//! CpuDevice — CPU reference backend for testing.
//!
//! Executes kernels by interpreting the FusedKernel IR directly.
//! Not performant — used only for correctness reference.

use std::sync::Mutex;

use crate::device::{
    Allocator, BufferHandle, Compiler, CompiledProgram, CpuKernelFn,
    DeviceBuffer, DeviceError, Executor, ProgramHandle,
};

/// CPU reference device backend for correctness testing.
///
/// Allocates CPU buffers and interprets FusedKernel IR directly.
/// Not performant -- used as the ground-truth reference.
pub struct CpuDevice {
    /// Buffer allocation counter for unique IDs.
    _next_id: Mutex<usize>,
}

impl CpuDevice {
    /// Create a new CPU device.
    pub fn new() -> Self {
        Self {
            _next_id: Mutex::new(0),
        }
    }
}

impl Default for CpuDevice {
    fn default() -> Self {
        Self::new()
    }
}

impl Allocator for CpuDevice {
    fn alloc(&self, size_bytes: usize) -> Result<DeviceBuffer, DeviceError> {
        let buf = vec![0u8; size_bytes];
        Ok(DeviceBuffer {
            handle: BufferHandle::Cpu(buf),
            size_bytes,
        })
    }

    fn free(&self, _buf: DeviceBuffer) -> Result<(), DeviceError> {
        // Drop handles deallocation for CPU buffers
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
                // SAFETY: We need interior mutability. The CPU backend uses
                // this for testing only. In production, Metal/WebGPU handle
                // synchronization at the command buffer level.
                let inner_ptr = inner.as_ptr() as *mut u8;
                unsafe {
                    std::ptr::copy_nonoverlapping(data.as_ptr(), inner_ptr, data.len());
                }
                Ok(())
            }
            #[cfg(target_os = "macos")]
            BufferHandle::Metal(_) => Err(DeviceError::InvalidArgument(
                "cannot copy_in to Metal buffer via CpuDevice".into(),
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
                "cannot copy_out from Metal buffer via CpuDevice".into(),
            )),
        }
    }
}

impl Compiler for CpuDevice {
    fn compile(&self, _source: &str, entry: &str) -> Result<CompiledProgram, DeviceError> {
        // CPU device doesn't compile shader source — it interprets FusedKernel directly.
        fn noop_kernel(_bufs: &[&[u8]], _out: &mut [u8], _num_elements: usize) {}
        Ok(CompiledProgram {
            handle: ProgramHandle::Cpu(noop_kernel as CpuKernelFn),
            entry: entry.to_string(),
        })
    }

    fn max_local_size(&self) -> [u32; 3] {
        [1024, 1, 1]
    }

    fn max_grid_size(&self) -> [u32; 3] {
        [u32::MAX, 1, 1]
    }
}

impl Executor for CpuDevice {
    fn exec(
        &self,
        _prog: &CompiledProgram,
        _bufs: &[&DeviceBuffer],
        _grid: [u32; 3],
        _local: [u32; 3],
    ) -> Result<(), DeviceError> {
        // CPU execution is done through the interpret_kernel method, not exec.
        Ok(())
    }

    fn synchronize(&self) -> Result<(), DeviceError> {
        // CPU is synchronous — nothing to wait for.
        Ok(())
    }
}

/// CPU kernel interpreter — executes a FusedKernel op-by-op on CPU.
/// This is the reference implementation used for correctness testing.
pub mod interpret {
    use crate::dtype::DType;
    use crate::ops::PrimitiveOp;
    use crate::render::{FusedKernel, FusedSrc};

    /// Interpret and execute a FusedKernel on CPU buffers.
    /// `bufs` are raw byte slices matching kernel.bufs order.
    /// bufs[0] is the output buffer (written to).
    pub fn execute_kernel(kernel: &FusedKernel, bufs: &mut [Vec<u8>]) {
        let output_numel = kernel.bufs[0].st.numel();

        for gid in 0..output_numel {
            let mut values: Vec<f64> = Vec::with_capacity(kernel.ops.len());

            for (op_idx, op) in kernel.ops.iter().enumerate() {
                if matches!(op.op, PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax) {
                    // Handle reduce ops
                    let input_buf_idx = match &op.srcs[0] {
                        FusedSrc::Buf(idx) => *idx,
                        FusedSrc::Op(_) => 1,
                        FusedSrc::Const { .. } => unreachable!(),
                    };
                    let input_numel = kernel.bufs[input_buf_idx].st.numel();
                    let reduce_size = input_numel / output_numel;

                    let mut acc = match op.op {
                        PrimitiveOp::ReduceSum => 0.0f64,
                        PrimitiveOp::ReduceMax => f64::NEG_INFINITY,
                        _ => unreachable!(),
                    };

                    for rid in 0..reduce_size {
                        let eidx = gid * reduce_size + rid;

                        // If there are pre-reduce ops, compute them for this element
                        let val = if op_idx > 0 {
                            // Re-compute pre-reduce elementwise chain for this element index
                            let mut pre_values: Vec<f64> = Vec::with_capacity(op_idx);
                            for pre_op in &kernel.ops[..op_idx] {
                                let get_pre_src = |i: usize| -> f64 {
                                    match &pre_op.srcs[i] {
                                        FusedSrc::Buf(idx) => {
                                            read_f64(&bufs[*idx], eidx, kernel.bufs[*idx].dtype)
                                        }
                                        FusedSrc::Op(prior) => pre_values[*prior],
                                        FusedSrc::Const { val, .. } => *val,
                                    }
                                };
                                let result = compute_elementwise(pre_op.op, &get_pre_src, pre_op.srcs.len());
                                pre_values.push(result);
                            }
                            *pre_values.last().unwrap()
                        } else {
                            read_f64(&bufs[input_buf_idx], eidx, kernel.bufs[input_buf_idx].dtype)
                        };

                        acc = match op.op {
                            PrimitiveOp::ReduceSum => acc + val,
                            PrimitiveOp::ReduceMax => {
                                // NaN-propagating max for floats
                                if val.is_nan() || acc.is_nan() {
                                    f64::NAN
                                } else {
                                    acc.max(val)
                                }
                            }
                            _ => unreachable!(),
                        };
                    }
                    values.push(acc);
                    continue;
                }

                let get_src = |i: usize| -> f64 {
                    match &op.srcs[i] {
                        FusedSrc::Buf(idx) => {
                            read_f64(&bufs[*idx], gid, kernel.bufs[*idx].dtype)
                        }
                        FusedSrc::Op(prior) => values[*prior],
                        FusedSrc::Const { val, .. } => *val,
                    }
                };

                let result = compute_elementwise(op.op, &get_src, op.srcs.len());
                values.push(result);
            }

            // Write output
            let result = values.last().copied().unwrap_or(0.0);
            write_f64(&mut bufs[0], gid, result, kernel.bufs[0].dtype);
        }
    }

    fn compute_elementwise(op: PrimitiveOp, get_src: &dyn Fn(usize) -> f64, _arity: usize) -> f64 {
        match op {
            PrimitiveOp::Add => get_src(0) + get_src(1),
            PrimitiveOp::Sub => get_src(0) - get_src(1),
            PrimitiveOp::Mul => get_src(0) * get_src(1),
            PrimitiveOp::Idiv => {
                let a = get_src(0) as i64;
                let b = get_src(1) as i64;
                if b == 0 { 0.0 } else { (a / b) as f64 }
            }
            PrimitiveOp::Mod => {
                let a = get_src(0) as i64;
                let b = get_src(1) as i64;
                if b == 0 { 0.0 } else { (a % b) as f64 }
            }
            PrimitiveOp::Neg => -get_src(0),
            PrimitiveOp::Cmplt => if get_src(0) < get_src(1) { 1.0 } else { 0.0 },
            PrimitiveOp::Cmpeq => if get_src(0) == get_src(1) { 1.0 } else { 0.0 },
            PrimitiveOp::Cmpne => if get_src(0) != get_src(1) { 1.0 } else { 0.0 },
            PrimitiveOp::And => ((get_src(0) as i64) & (get_src(1) as i64)) as f64,
            PrimitiveOp::Or => ((get_src(0) as i64) | (get_src(1) as i64)) as f64,
            PrimitiveOp::Xor => ((get_src(0) as i64) ^ (get_src(1) as i64)) as f64,
            PrimitiveOp::Shl => ((get_src(0) as i64) << (get_src(1) as i64)) as f64,
            PrimitiveOp::Shr => ((get_src(0) as i64) >> (get_src(1) as i64)) as f64,
            PrimitiveOp::Exp2 => get_src(0).exp2(),
            PrimitiveOp::Log2 => get_src(0).log2(),
            PrimitiveOp::Sin => get_src(0).sin(),
            PrimitiveOp::Sqrt => get_src(0).sqrt(),
            PrimitiveOp::Reciprocal => 1.0 / get_src(0),
            PrimitiveOp::Trunc => get_src(0).trunc(),
            PrimitiveOp::Max => {
                let a = get_src(0);
                let b = get_src(1);
                // NaN-propagating max: if either operand is NaN, result is NaN.
                if a.is_nan() || b.is_nan() {
                    f64::NAN
                } else {
                    a.max(b)
                }
            }
            PrimitiveOp::Where => {
                if get_src(0) != 0.0 { get_src(1) } else { get_src(2) }
            }
            PrimitiveOp::Cast => get_src(0),
            PrimitiveOp::Bitcast => get_src(0),
            PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax => unreachable!(),
        }
    }

    fn read_f64(buf: &[u8], idx: usize, dtype: DType) -> f64 {
        match dtype {
            DType::Float32 => {
                let offset = idx * 4;
                if offset + 4 > buf.len() { return 0.0; }
                f32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap()) as f64
            }
            DType::Float64 => {
                let offset = idx * 8;
                if offset + 8 > buf.len() { return 0.0; }
                f64::from_le_bytes(buf[offset..offset + 8].try_into().unwrap())
            }
            DType::Int32 => {
                let offset = idx * 4;
                if offset + 4 > buf.len() { return 0.0; }
                i32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap()) as f64
            }
            DType::Bool | DType::UInt8 => {
                if idx >= buf.len() { return 0.0; }
                buf[idx] as f64
            }
            DType::Int8 => {
                if idx >= buf.len() { return 0.0; }
                (buf[idx] as i8) as f64
            }
            DType::Int16 => {
                let offset = idx * 2;
                if offset + 2 > buf.len() { return 0.0; }
                i16::from_le_bytes(buf[offset..offset + 2].try_into().unwrap()) as f64
            }
            DType::UInt16 => {
                let offset = idx * 2;
                if offset + 2 > buf.len() { return 0.0; }
                u16::from_le_bytes(buf[offset..offset + 2].try_into().unwrap()) as f64
            }
            DType::Int64 => {
                let offset = idx * 8;
                if offset + 8 > buf.len() { return 0.0; }
                i64::from_le_bytes(buf[offset..offset + 8].try_into().unwrap()) as f64
            }
            DType::UInt32 => {
                let offset = idx * 4;
                if offset + 4 > buf.len() { return 0.0; }
                u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap()) as f64
            }
            DType::UInt64 => {
                let offset = idx * 8;
                if offset + 8 > buf.len() { return 0.0; }
                u64::from_le_bytes(buf[offset..offset + 8].try_into().unwrap()) as f64
            }
            DType::Float16 => {
                let offset = idx * 2;
                if offset + 2 > buf.len() { return 0.0; }
                let bits = u16::from_le_bytes(buf[offset..offset + 2].try_into().unwrap());
                half::f16::from_bits(bits).to_f64()
            }
            DType::BFloat16 => {
                let offset = idx * 2;
                if offset + 2 > buf.len() { return 0.0; }
                let bits = u16::from_le_bytes(buf[offset..offset + 2].try_into().unwrap());
                half::bf16::from_bits(bits).to_f64()
            }
            // MXFP types: raw byte read. Dequantization happens at a higher level.
            DType::MxFP8 | DType::MxFP4 => {
                if idx >= buf.len() { return 0.0; }
                buf[idx] as f64
            }
        }
    }

    fn write_f64(buf: &mut [u8], idx: usize, val: f64, dtype: DType) {
        match dtype {
            DType::Float32 => {
                let offset = idx * 4;
                if offset + 4 <= buf.len() {
                    buf[offset..offset + 4].copy_from_slice(&(val as f32).to_le_bytes());
                }
            }
            DType::Float64 => {
                let offset = idx * 8;
                if offset + 8 <= buf.len() {
                    buf[offset..offset + 8].copy_from_slice(&val.to_le_bytes());
                }
            }
            DType::Int32 => {
                let offset = idx * 4;
                if offset + 4 <= buf.len() {
                    buf[offset..offset + 4].copy_from_slice(&(val as i32).to_le_bytes());
                }
            }
            DType::Bool | DType::UInt8 => {
                if idx < buf.len() {
                    buf[idx] = if val != 0.0 { 1 } else { 0 };
                }
            }
            DType::Int8 => {
                if idx < buf.len() {
                    buf[idx] = (val as i8) as u8;
                }
            }
            DType::Int16 => {
                let offset = idx * 2;
                if offset + 2 <= buf.len() {
                    buf[offset..offset + 2].copy_from_slice(&(val as i16).to_le_bytes());
                }
            }
            DType::UInt16 => {
                let offset = idx * 2;
                if offset + 2 <= buf.len() {
                    buf[offset..offset + 2].copy_from_slice(&(val as u16).to_le_bytes());
                }
            }
            DType::Int64 => {
                let offset = idx * 8;
                if offset + 8 <= buf.len() {
                    buf[offset..offset + 8].copy_from_slice(&(val as i64).to_le_bytes());
                }
            }
            DType::UInt32 => {
                let offset = idx * 4;
                if offset + 4 <= buf.len() {
                    buf[offset..offset + 4].copy_from_slice(&(val as u32).to_le_bytes());
                }
            }
            DType::UInt64 => {
                let offset = idx * 8;
                if offset + 8 <= buf.len() {
                    buf[offset..offset + 8].copy_from_slice(&(val as u64).to_le_bytes());
                }
            }
            DType::Float16 => {
                let offset = idx * 2;
                if offset + 2 <= buf.len() {
                    let h = half::f16::from_f64(val);
                    buf[offset..offset + 2].copy_from_slice(&h.to_bits().to_le_bytes());
                }
            }
            DType::BFloat16 => {
                let offset = idx * 2;
                if offset + 2 <= buf.len() {
                    let h = half::bf16::from_f64(val);
                    buf[offset..offset + 2].copy_from_slice(&h.to_bits().to_le_bytes());
                }
            }
            // MXFP types: raw byte write. Quantization happens at a higher level.
            DType::MxFP8 | DType::MxFP4 => {
                if idx < buf.len() {
                    buf[idx] = val as u8;
                }
            }
        }
    }
}
