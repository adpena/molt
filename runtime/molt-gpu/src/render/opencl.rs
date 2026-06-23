//! OpenClRenderer — OpenCL C codegen for all 26 primitive ops.
//!
//! Generates OpenCL C compute kernel source from FusedKernel IR.
//! Thread index: `get_global_id(0)`
//! Buffer args: `__global float* restrict`
//!
//! OpenCL supports i64 natively. f64 requires the `cl_khr_fp64` extension;
//! when f64 buffers are present, the renderer emits the required pragma.
//! BFloat16 is not supported in OpenCL — narrowed to Float32 via `narrow_opencl`.
//!
//! Reduce ops use workgroup-level reduction with `barrier(CLK_LOCAL_MEM_FENCE)`
//! and `__local` shared memory for efficient parallel reduction within a
//! workgroup, falling back to a sequential loop over the full reduction
//! dimension per work-item.

use std::fmt::Write;

use crate::dtype::DType;
use crate::ops::PrimitiveOp;
use crate::render::indexing::{
    render_reduction_input_index, render_shapetracker_index, zero_literal_for_dtype, IndexDialect,
};
use crate::render::{BufferAccess, FusedKernel, FusedOp, FusedSrc, KernelBody, Renderer};

/// OpenCL C renderer for all 26 primitive ops.
pub struct OpenClRenderer {
    /// Whether the target device supports the `cl_khr_fp64` extension.
    pub has_fp64: bool,
}

impl OpenClRenderer {
    /// Create a new OpenCL renderer.
    ///
    /// `has_fp64`: set to `true` if the target device advertises `cl_khr_fp64`.
    /// When `false`, Float64 dtypes are narrowed to Float32.
    pub fn new(has_fp64: bool) -> Self {
        Self { has_fp64 }
    }

    /// Format a constant value as an OpenCL C literal.
    fn format_const(&self, val: f64, dtype: DType) -> String {
        let dtype = dtype.narrow_opencl(self.has_fp64);
        match dtype {
            DType::Bool => {
                if val != 0.0 {
                    "1".to_string()
                } else {
                    "0".to_string()
                }
            }
            DType::Float16 => {
                let s = format!("{}", val);
                if s.contains('.') || s.contains('e') || s.contains('E') {
                    format!("(half)({}f)", s)
                } else {
                    format!("(half)({}.0f)", s)
                }
            }
            DType::Float32 => {
                if val == f64::INFINITY {
                    "INFINITY".to_string()
                } else if val == f64::NEG_INFINITY {
                    "(-INFINITY)".to_string()
                } else if val.is_nan() {
                    "NAN".to_string()
                } else {
                    let s = format!("{}", val);
                    if s.contains('.') || s.contains('e') || s.contains('E') {
                        format!("{}f", s)
                    } else {
                        format!("{}.0f", s)
                    }
                }
            }
            DType::Float64 => {
                if val == f64::INFINITY {
                    "INFINITY".to_string()
                } else if val == f64::NEG_INFINITY {
                    "(-INFINITY)".to_string()
                } else if val.is_nan() {
                    "NAN".to_string()
                } else {
                    let s = format!("{}", val);
                    if s.contains('.') || s.contains('e') || s.contains('E') {
                        s
                    } else {
                        format!("{}.0", s)
                    }
                }
            }
            DType::Int8 | DType::Int16 | DType::Int32 => {
                format!("(({}){})", dtype.opencl_type(), val as i64)
            }
            DType::Int64 => format!("{}L", val as i64),
            DType::UInt8 | DType::UInt16 | DType::UInt32 => {
                format!("(({}){}u)", dtype.opencl_type(), val as u64)
            }
            DType::UInt64 => format!("{}UL", val as u64),
            DType::BFloat16 => {
                // BFloat16 is narrowed to Float32 by narrow_opencl
                unreachable!("BFloat16 should have been narrowed to Float32")
            }
            // MXFP types: constants are stored as uchar (raw byte).
            DType::MxFP8 | DType::MxFP4 => format!("((uchar){})", val as u8),
        }
    }

    /// Render a buffer read expression at the given index.
    fn render_buf_read(
        binding_idx: usize,
        binding: &crate::render::BufferBinding,
        idx_var: &str,
    ) -> String {
        let idx = render_shapetracker_index(&binding.st, idx_var, IndexDialect::CLike);
        let read = format!("buf{}[{}]", binding_idx, idx.index);
        if let Some(valid) = idx.valid {
            let zero = zero_literal_for_dtype(binding.dtype.narrow_opencl(true), "0");
            format!("({} ? {} : {})", valid, read, zero)
        } else {
            read
        }
    }

    fn render_src(&self, src: &FusedSrc, kernel: &FusedKernel, idx_var: &str) -> String {
        match src {
            FusedSrc::Buf(buf_idx) => {
                Self::render_buf_read(*buf_idx, &kernel.bufs[*buf_idx], idx_var)
            }
            FusedSrc::Op(prior_idx) => format!("v{}", prior_idx),
            FusedSrc::Const { val, dtype } => self.format_const(*val, *dtype),
        }
    }

    /// Render a single op expression as OpenCL C.
    fn render_op(
        &self,
        op: &FusedOp,
        _op_idx: usize,
        kernel: &FusedKernel,
        idx_var: &str,
    ) -> String {
        let src = |i: usize| -> String { self.render_src(&op.srcs()[i], kernel, idx_var) };

        let dst_dtype = op.dst_dtype().narrow_opencl(self.has_fp64);
        let dst_type = dst_dtype.opencl_type();

        match op.op() {
            PrimitiveOp::Add => format!("({} + {})", src(0), src(1)),
            PrimitiveOp::Sub => format!("({} - {})", src(0), src(1)),
            PrimitiveOp::Mul => format!("({} * {})", src(0), src(1)),
            PrimitiveOp::Idiv => format!("({} / {})", src(0), src(1)),
            PrimitiveOp::Mod => format!("({} % {})", src(0), src(1)),
            PrimitiveOp::Neg => format!("(-{})", src(0)),

            PrimitiveOp::Cmplt => format!("({} < {})", src(0), src(1)),
            PrimitiveOp::Cmpeq => format!("({} == {})", src(0), src(1)),
            PrimitiveOp::Cmpne => format!("({} != {})", src(0), src(1)),

            PrimitiveOp::And => format!("({} & {})", src(0), src(1)),
            PrimitiveOp::Or => format!("({} | {})", src(0), src(1)),
            PrimitiveOp::Xor => format!("({} ^ {})", src(0), src(1)),
            PrimitiveOp::Shl => format!("({} << {})", src(0), src(1)),
            PrimitiveOp::Shr => format!("({} >> {})", src(0), src(1)),

            PrimitiveOp::Exp2 => format!("exp2({})", src(0)),
            PrimitiveOp::Log2 => format!("log2({})", src(0)),
            PrimitiveOp::Sin => format!("sin({})", src(0)),
            PrimitiveOp::Sqrt => format!("sqrt({})", src(0)),
            PrimitiveOp::Reciprocal => {
                if dst_dtype == DType::Float64 {
                    format!("(1.0 / {})", src(0))
                } else {
                    format!("(1.0f / {})", src(0))
                }
            }

            PrimitiveOp::Trunc => format!("trunc({})", src(0)),
            // NaN-propagating max: if either operand is NaN, result is NaN.
            // fmax is NaN-suppressing (IEEE 754 minNum), so we add an explicit NaN check.
            PrimitiveOp::Max => format!(
                "(isnan({a}) || isnan({b}) ? NAN : fmax({a}, {b}))",
                a = src(0),
                b = src(1)
            ),
            PrimitiveOp::Where => format!("({} ? {} : {})", src(0), src(1), src(2)),
            PrimitiveOp::Cast => format!("(({})({}))", dst_type, src(0)),
            PrimitiveOp::Bitcast => format!("as_{}({})", dst_type, src(0)),

            PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax => {
                unreachable!("Reduce ops are handled by the kernel loop generator")
            }
        }
    }

    /// Detect FMA pattern: ADD(MUL(a, b), c) or ADD(c, MUL(a, b)).
    fn detect_fma(
        &self,
        op: &FusedOp,
        op_idx: usize,
        kernel: &FusedKernel,
        idx_var: &str,
    ) -> Option<(String, String, String)> {
        if op.op() != PrimitiveOp::Add {
            return None;
        }
        if !op.dst_dtype().narrow_opencl(self.has_fp64).is_float() {
            return None;
        }

        for (mul_src_pos, add_src_pos) in [(0, 1), (1, 0)] {
            if let FusedSrc::Op(prior_idx) = &op.srcs()[mul_src_pos] {
                if *prior_idx < op_idx {
                    let prior_op = &kernel.ops[*prior_idx];
                    if prior_op.op() == PrimitiveOp::Mul {
                        let a = match &prior_op.srcs()[0] {
                            FusedSrc::Buf(buf_idx) => {
                                Self::render_buf_read(*buf_idx, &kernel.bufs[*buf_idx], idx_var)
                            }
                            FusedSrc::Op(p) => format!("v{}", p),
                            FusedSrc::Const { val, dtype } => self.format_const(*val, *dtype),
                        };
                        let b = match &prior_op.srcs()[1] {
                            FusedSrc::Buf(buf_idx) => {
                                Self::render_buf_read(*buf_idx, &kernel.bufs[*buf_idx], idx_var)
                            }
                            FusedSrc::Op(p) => format!("v{}", p),
                            FusedSrc::Const { val, dtype } => self.format_const(*val, *dtype),
                        };
                        let c = match &op.srcs()[add_src_pos] {
                            FusedSrc::Buf(buf_idx) => {
                                Self::render_buf_read(*buf_idx, &kernel.bufs[*buf_idx], idx_var)
                            }
                            FusedSrc::Op(p) => format!("v{}", p),
                            FusedSrc::Const { val, dtype } => self.format_const(*val, *dtype),
                        };
                        return Some((a, b, c));
                    }
                }
            }
        }
        None
    }

    /// Check whether any buffer in the kernel uses Float64 (pre-narrowing).
    fn needs_fp64(kernel: &FusedKernel) -> bool {
        kernel.bufs.iter().any(|b| b.dtype == DType::Float64)
            || kernel.ops.iter().any(|op| op.dst_dtype() == DType::Float64)
    }
}

impl Renderer for OpenClRenderer {
    fn render(&self, kernel: &FusedKernel) -> String {
        kernel.assert_no_mxfp_dtypes("OpenCL renderer");
        let mut out = String::with_capacity(4096);

        // Emit fp64 pragma if needed and supported
        if self.has_fp64 && Self::needs_fp64(kernel) {
            writeln!(out, "#pragma OPENCL EXTENSION cl_khr_fp64 : enable").unwrap();
            writeln!(out).unwrap();
        }

        // Kernel function signature
        write!(out, "__kernel void molt_kernel(").unwrap();

        for (i, binding) in kernel.bufs.iter().enumerate() {
            let narrowed_dtype = binding.dtype.narrow_opencl(self.has_fp64);
            let dtype_str = narrowed_dtype.opencl_type();
            let qualifier = match binding.access {
                BufferAccess::Read => "__global const ",
                BufferAccess::Write | BufferAccess::ReadWrite => "__global ",
            };
            if i > 0 {
                write!(out, ", ").unwrap();
            }
            write!(out, "{}{} * restrict buf{}", qualifier, dtype_str, i).unwrap();
        }
        writeln!(out, ") {{").unwrap();

        // Thread index
        writeln!(out, "    unsigned int gid = get_global_id(0);").unwrap();

        // Bounds check
        let output_numel = kernel.bufs[0].st.numel();
        writeln!(out, "    if (gid >= {}u) return;", output_numel).unwrap();

        if kernel.body == KernelBody::MaterializeCopy {
            let (_, src_binding, copy_numel) = kernel.materialize_copy_contract();
            assert_eq!(copy_numel, output_numel);
            assert_eq!(
                src_binding.dtype,
                src_binding.dtype.narrow_opencl(self.has_fp64),
                "OpenCL MaterializeCopy requires a non-narrowed dtype"
            );
            let src = Self::render_buf_read(1, src_binding, "gid");
            writeln!(out, "    buf0[gid] = {};", src).unwrap();
            writeln!(out, "}}").unwrap();
            return out;
        }
        kernel.compute_body_contract();

        let has_reduce = kernel
            .ops
            .iter()
            .any(|op| matches!(op.op(), PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax));

        if kernel.vectorize_width == 4 && !has_reduce {
            // Vectorized 4-wide: each thread processes 4 elements via vload4/vstore4
            let vec_numel = output_numel / 4;
            writeln!(out, "    if (gid >= {}u) return;", vec_numel).unwrap();
            writeln!(
                out,
                "    // Vectorized 4-wide: vload4/vstore4 coalesced access"
            )
            .unwrap();
            writeln!(out, "    unsigned int base = gid * 4;").unwrap();
            writeln!(out, "    #pragma unroll").unwrap();
            writeln!(out, "    for (unsigned int lane = 0; lane < 4; lane++) {{").unwrap();
            writeln!(out, "        unsigned int eidx = base + lane;").unwrap();

            for (i, op) in kernel.ops.iter().enumerate() {
                let dtype_str = op.dst_dtype().narrow_opencl(self.has_fp64).opencl_type();
                let expr = if let Some((a, b, c)) = self.detect_fma(op, i, kernel, "eidx") {
                    format!("fma({}, {}, {})", a, b, c)
                } else {
                    self.render_op(op, i, kernel, "eidx")
                };
                writeln!(out, "        {} v{} = {};", dtype_str, i, expr).unwrap();
            }
            let last_op = kernel.ops.len() - 1;
            writeln!(out, "        buf0[eidx] = v{};", last_op).unwrap();
            writeln!(out, "    }}").unwrap();
        } else if !has_reduce {
            for (i, op) in kernel.ops.iter().enumerate() {
                let dtype_str = op.dst_dtype().narrow_opencl(self.has_fp64).opencl_type();
                let expr = if let Some((a, b, c)) = self.detect_fma(op, i, kernel, "gid") {
                    format!("fma({}, {}, {})", a, b, c)
                } else {
                    self.render_op(op, i, kernel, "gid")
                };
                writeln!(out, "    {} v{} = {};", dtype_str, i, expr).unwrap();
            }
            let last_op = kernel.ops.len() - 1;
            writeln!(out, "    buf0[gid] = v{};", last_op).unwrap();
        } else {
            let reduce_idx = kernel
                .ops
                .iter()
                .position(|op| matches!(op.op(), PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax))
                .expect("has_reduce but no reduce op found");

            let reduce_op = &kernel.ops[reduce_idx];
            let reduce_src = &reduce_op.srcs()[0];
            let reduce_dtype = reduce_op.dst_dtype().narrow_opencl(self.has_fp64);
            let reduce_type = reduce_dtype.opencl_type();
            let domain = reduce_op.require_reduction_domain();
            assert_eq!(
                domain.output_numel(),
                output_numel,
                "OpenCL reduction domain output shape must match kernel output"
            );
            let reduce_size = domain.reduce_size;
            let reduce_index =
                render_reduction_input_index(domain, "gid", "rid", IndexDialect::CLike);

            let init_val = match reduce_op.op() {
                PrimitiveOp::ReduceSum => format!("({})0", reduce_type),
                PrimitiveOp::ReduceMax => "(-INFINITY)".to_string(),
                _ => unreachable!(),
            };

            let local_size = kernel.local[0];

            // Workgroup-level reduction with __local shared memory
            writeln!(out, "    __local {} sdata[{}];", reduce_type, local_size).unwrap();
            writeln!(out, "    {} acc = {};", reduce_type, init_val).unwrap();

            if reduce_idx > 0 {
                if reduce_size <= 16 {
                    writeln!(out, "    __attribute__((opencl_unroll_hint))").unwrap();
                }
                writeln!(
                    out,
                    "    for (unsigned int rid = 0; rid < {}u; rid++) {{",
                    reduce_size
                )
                .unwrap();
                writeln!(out, "        unsigned int eidx = {};", reduce_index).unwrap();

                for i in 0..reduce_idx {
                    let op = &kernel.ops[i];
                    let dtype_str = op.dst_dtype().narrow_opencl(self.has_fp64).opencl_type();
                    let expr = self.render_op(op, i, kernel, "eidx");
                    writeln!(out, "        {} v{} = {};", dtype_str, i, expr).unwrap();
                }

                let src_expr = self.render_src(reduce_src, kernel, "eidx");
                match reduce_op.op() {
                    PrimitiveOp::ReduceSum => {
                        writeln!(out, "        acc += {};", src_expr).unwrap()
                    }
                    PrimitiveOp::ReduceMax => writeln!(
                        out,
                        "        acc = (isnan({v}) || isnan(acc)) ? NAN : fmax(acc, {v});",
                        v = src_expr
                    )
                    .unwrap(),
                    _ => unreachable!(),
                }
                writeln!(out, "    }}").unwrap();
            } else {
                if reduce_size <= 16 {
                    writeln!(out, "    __attribute__((opencl_unroll_hint))").unwrap();
                }
                writeln!(
                    out,
                    "    for (unsigned int rid = 0; rid < {}u; rid++) {{",
                    reduce_size
                )
                .unwrap();
                writeln!(out, "        unsigned int eidx = {};", reduce_index).unwrap();
                let src_expr = self.render_src(reduce_src, kernel, "eidx");
                match reduce_op.op() {
                    PrimitiveOp::ReduceSum => {
                        writeln!(out, "        acc += {};", src_expr).unwrap()
                    }
                    PrimitiveOp::ReduceMax => {
                        writeln!(out, "        {{ float _rv = {}; acc = (isnan(_rv) || isnan(acc)) ? NAN : fmax(acc, _rv); }}", src_expr).unwrap();
                    }
                    _ => unreachable!(),
                }
                writeln!(out, "    }}").unwrap();
            }

            // Store per-work-item partial result to local memory
            writeln!(out, "    unsigned int lid = get_local_id(0);").unwrap();
            writeln!(out, "    sdata[lid] = acc;").unwrap();
            writeln!(out, "    barrier(CLK_LOCAL_MEM_FENCE);").unwrap();

            // Tree reduction within workgroup
            writeln!(
                out,
                "    for (unsigned int s = get_local_size(0) / 2; s > 0; s >>= 1) {{"
            )
            .unwrap();
            writeln!(out, "        if (lid < s) {{").unwrap();
            match reduce_op.op() {
                PrimitiveOp::ReduceSum => writeln!(out, "            sdata[lid] += sdata[lid + s];").unwrap(),
                PrimitiveOp::ReduceMax => writeln!(out, "            sdata[lid] = (isnan(sdata[lid]) || isnan(sdata[lid + s])) ? NAN : fmax(sdata[lid], sdata[lid + s]);").unwrap(),
                _ => unreachable!(),
            }
            writeln!(out, "        }}").unwrap();
            writeln!(out, "        barrier(CLK_LOCAL_MEM_FENCE);").unwrap();
            writeln!(out, "    }}").unwrap();

            writeln!(out, "    {} v{} = acc;", reduce_type, reduce_idx).unwrap();

            for i in (reduce_idx + 1)..kernel.ops.len() {
                let op = &kernel.ops[i];
                let dtype_str = op.dst_dtype().narrow_opencl(self.has_fp64).opencl_type();
                let expr = self.render_op(op, i, kernel, "gid");
                writeln!(out, "    {} v{} = {};", dtype_str, i, expr).unwrap();
            }

            let last_op = kernel.ops.len() - 1;
            writeln!(out, "    buf0[gid] = v{};", last_op).unwrap();
        }

        writeln!(out, "}}").unwrap();
        out
    }
}
