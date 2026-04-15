//! WgslRenderer — WebGPU Shading Language codegen for all 26 primitive ops.
//!
//! Generates WGSL compute shader source from FusedKernel IR.
//! All dtypes are narrowed via DType::narrow_webgpu() before rendering.
//! Key WGSL differences from MSL:
//! - No ternary operator: use `select(false_val, true_val, cond)`
//! - Bitcast syntax: `bitcast<T>(x)`
//! - Thread index: `@builtin(global_invocation_id) gid: vec3<u32>`
//! - Workgroup size annotation: `@workgroup_size(X, Y, Z)`
//!
//! WebGPU subgroups (Chrome 134+, Edge 144+): when enabled, reduction ops
//! use `subgroupAdd()` / `subgroupMax()` instead of sequential loops for
//! the final reduction within a subgroup.

use std::fmt::Write;

use crate::dtype::DType;
use crate::ops::PrimitiveOp;
use crate::render::{BufferAccess, FusedKernel, FusedOp, FusedSrc, Renderer};

/// Configuration for the WGSL renderer.
#[derive(Debug, Clone, Default)]
pub struct WgslRendererConfig {
    /// When true, emit `enable subgroups;` and use `subgroupAdd()` /
    /// `subgroupMax()` for reduction operations. Requires WebGPU subgroups
    /// support (Chrome 134+, Edge 144+).
    pub use_subgroups: bool,
}

/// WebGPU Shading Language renderer for all 26 primitive ops.
///
/// Optionally uses WebGPU subgroup operations for efficient reductions
/// when `config.use_subgroups` is true.
pub struct WgslRenderer {
    /// Renderer configuration. When `None`, uses default (no subgroups).
    config: WgslRendererConfig,
}

impl WgslRenderer {
    /// Create a new WGSL renderer with default configuration (no subgroups).
    pub fn new() -> Self {
        Self { config: WgslRendererConfig::default() }
    }

    /// Create a new WGSL renderer with the given configuration.
    pub fn with_config(config: WgslRendererConfig) -> Self {
        Self { config }
    }
}

impl WgslRenderer {
    /// Format a constant value as WGSL literal.
    fn format_const(val: f64, dtype: DType) -> String {
        let dtype = dtype.narrow_webgpu();
        match dtype {
            DType::Bool => {
                if val != 0.0 { "true".to_string() } else { "false".to_string() }
            }
            DType::Float16 => format!("f16({})", val),
            DType::Float32 | DType::BFloat16 => {
                if val == f64::INFINITY { "bitcast<f32>(0x7f800000u)".to_string() }
                else if val == f64::NEG_INFINITY { "bitcast<f32>(0xff800000u)".to_string() }
                else if val.is_nan() { "bitcast<f32>(0x7fc00000u)".to_string() }
                else { format!("f32({})", val) }
            }
            DType::Int8 | DType::Int16 | DType::Int32 | DType::Int64 => {
                format!("i32({})", val as i64)
            }
            DType::UInt8 | DType::UInt16 | DType::UInt32 | DType::UInt64 => {
                format!("u32({})", val as u64)
            }
            _ => format!("{}", val),
        }
    }

    /// Render a buffer read expression at the given index.
    fn render_buf_read(binding: &crate::render::BufferBinding, idx_var: &str) -> String {
        let st = &binding.st;
        let view = st.view();

        let ndim = view.shape.len();
        if ndim == 0 {
            return format!("buf{}[0]", binding.buf_id);
        }

        if view.is_contiguous() && view.mask.is_none() {
            return format!("buf{}[{}]", binding.buf_id, idx_var);
        }

        // General case: decompose linear index, apply strides
        let mut parts = Vec::new();
        for dim in 0..ndim {
            let stride = view.strides[dim];
            if stride == 0 {
                continue;
            }
            let size = view.shape[dim];
            let idx_expr = if dim == ndim - 1 {
                format!("({} % {}u)", idx_var, size)
            } else {
                let divisor: usize = view.shape[dim + 1..].iter().product();
                format!("({} / {}u % {}u)", idx_var, divisor, size)
            };
            if stride == 1 {
                parts.push(idx_expr);
            } else if stride == -1 {
                parts.push(format!("({}u - {})", size - 1, idx_expr));
            } else if stride > 0 {
                parts.push(format!("{} * {}u", idx_expr, stride));
            } else {
                parts.push(format!("({}u - {}) * {}u", size - 1, idx_expr, -stride));
            }
        }

        let offset = if view.offset != 0 {
            format!("{}u + ", view.offset)
        } else {
            String::new()
        };

        let idx_sum = if parts.is_empty() {
            "0u".to_string()
        } else {
            parts.join(" + ")
        };

        format!("buf{}[{}{}]", binding.buf_id, offset, idx_sum)
    }

    /// Render a single op expression.
    fn render_op(op: &FusedOp, _op_idx: usize, kernel: &FusedKernel, idx_var: &str) -> String {
        let src = |i: usize| -> String {
            match &op.srcs[i] {
                FusedSrc::Buf(buf_idx) => {
                    Self::render_buf_read(&kernel.bufs[*buf_idx], idx_var)
                }
                FusedSrc::Op(prior_idx) => format!("v{}", prior_idx),
                FusedSrc::Const { val, dtype } => Self::format_const(*val, *dtype),
            }
        };

        let dst_type = op.dst_dtype.narrow_webgpu().wgsl_type();

        match op.op {
            // Arithmetic
            PrimitiveOp::Add => format!("({} + {})", src(0), src(1)),
            PrimitiveOp::Sub => format!("({} - {})", src(0), src(1)),
            PrimitiveOp::Mul => format!("({} * {})", src(0), src(1)),
            PrimitiveOp::Idiv => format!("({} / {})", src(0), src(1)),
            PrimitiveOp::Mod => format!("({} % {})", src(0), src(1)),
            PrimitiveOp::Neg => format!("(-{})", src(0)),

            // Comparison
            PrimitiveOp::Cmplt => format!("({} < {})", src(0), src(1)),
            PrimitiveOp::Cmpeq => format!("({} == {})", src(0), src(1)),
            PrimitiveOp::Cmpne => format!("({} != {})", src(0), src(1)),

            // Bitwise
            PrimitiveOp::And => format!("({} & {})", src(0), src(1)),
            PrimitiveOp::Or => format!("({} | {})", src(0), src(1)),
            PrimitiveOp::Xor => format!("({} ^ {})", src(0), src(1)),
            PrimitiveOp::Shl => format!("({} << {})", src(0), src(1)),
            PrimitiveOp::Shr => format!("({} >> {})", src(0), src(1)),

            // Math
            PrimitiveOp::Exp2 => format!("exp2({})", src(0)),
            PrimitiveOp::Log2 => format!("log2({})", src(0)),
            PrimitiveOp::Sin => format!("sin({})", src(0)),
            PrimitiveOp::Sqrt => format!("sqrt({})", src(0)),
            PrimitiveOp::Reciprocal => format!("(f32(1.0) / {})", src(0)),

            // Other
            PrimitiveOp::Trunc => format!("trunc({})", src(0)),
            PrimitiveOp::Max => format!("max({}, {})", src(0), src(1)),
            // WGSL has no ternary operator; use select(false_val, true_val, cond)
            PrimitiveOp::Where => format!("select({}, {}, {})", src(2), src(1), src(0)),
            PrimitiveOp::Cast => format!("{}({})", dst_type, src(0)),
            PrimitiveOp::Bitcast => format!("bitcast<{}>({})", dst_type, src(0)),

            // Reduce
            PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax => {
                unreachable!("Reduce ops are handled by the kernel loop generator")
            }
        }
    }

    /// Detect FMA pattern: ADD(MUL(a, b), c) or ADD(c, MUL(a, b)).
    fn detect_fma(op: &FusedOp, op_idx: usize, kernel: &FusedKernel) -> Option<(String, String, String)> {
        if op.op != PrimitiveOp::Add {
            return None;
        }
        if !op.dst_dtype.narrow_webgpu().is_float() {
            return None;
        }

        for (mul_src_pos, add_src_pos) in [(0, 1), (1, 0)] {
            if let FusedSrc::Op(prior_idx) = &op.srcs[mul_src_pos] {
                if *prior_idx < op_idx {
                    let prior_op = &kernel.ops[*prior_idx];
                    if prior_op.op == PrimitiveOp::Mul {
                        let a = match &prior_op.srcs[0] {
                            FusedSrc::Buf(buf_idx) => Self::render_buf_read(&kernel.bufs[*buf_idx], "gid"),
                            FusedSrc::Op(p) => format!("v{}", p),
                            FusedSrc::Const { val, dtype } => Self::format_const(*val, *dtype),
                        };
                        let b = match &prior_op.srcs[1] {
                            FusedSrc::Buf(buf_idx) => Self::render_buf_read(&kernel.bufs[*buf_idx], "gid"),
                            FusedSrc::Op(p) => format!("v{}", p),
                            FusedSrc::Const { val, dtype } => Self::format_const(*val, *dtype),
                        };
                        let c = match &op.srcs[add_src_pos] {
                            FusedSrc::Buf(buf_idx) => Self::render_buf_read(&kernel.bufs[*buf_idx], "gid"),
                            FusedSrc::Op(p) => format!("v{}", p),
                            FusedSrc::Const { val, dtype } => Self::format_const(*val, *dtype),
                        };
                        return Some((a, b, c));
                    }
                }
            }
        }
        None
    }
}

impl Default for WgslRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderer for WgslRenderer {
    fn render(&self, kernel: &FusedKernel) -> String {
        let mut out = String::with_capacity(4096);

        // Emit subgroup enable directive when configured.
        if self.config.use_subgroups {
            writeln!(out, "enable subgroups;").unwrap();
            writeln!(out).unwrap();
        }

        // Buffer bindings as storage buffers
        for (i, binding) in kernel.bufs.iter().enumerate() {
            let dtype_str = binding.dtype.narrow_webgpu().wgsl_type();
            let access = match binding.access {
                BufferAccess::Read => "read",
                BufferAccess::Write | BufferAccess::ReadWrite => "read_write",
            };
            writeln!(
                out,
                "@group(0) @binding({}) var<storage, {}> buf{}: array<{}>;",
                i, access, binding.buf_id, dtype_str
            ).unwrap();
        }
        writeln!(out).unwrap();

        // Compute shader entry point
        writeln!(
            out,
            "@compute @workgroup_size({}, {}, {})",
            kernel.local[0], kernel.local[1], kernel.local[2]
        ).unwrap();
        writeln!(out, "fn molt_kernel(@builtin(global_invocation_id) gid_vec: vec3<u32>) {{").unwrap();
        writeln!(out, "    let gid = gid_vec.x;").unwrap();

        // Bounds check: elide when shape specialization proves total_elements
        // is exactly divisible by workgroup size (no out-of-bounds threads).
        // Per Maczan 2026, every instruction in the dispatch path matters when
        // per-operation overhead dominates at batch=1.
        let output_numel = kernel.bufs[0].st.numel();
        let bounds_check_elim = kernel.spec.as_ref()
            .is_some_and(|s| s.bounds_check_elim);
        if !bounds_check_elim {
            writeln!(out, "    if (gid >= {}u) {{ return; }}", output_numel).unwrap();
        }

        // Check for reduce ops
        let has_reduce = kernel.ops.iter().any(|op| matches!(op.op, PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax));

        if kernel.vectorize_width == 4 && !has_reduce {
            // Vectorized 4-wide: each thread processes 4 contiguous elements
            // using vec4<f32> access for coalesced memory bandwidth.
            let vec_numel = output_numel / 4;
            if !bounds_check_elim {
                writeln!(out, "    if (gid >= {}u) {{ return; }}", vec_numel).unwrap();
            }
            writeln!(out, "    // Vectorized 4-wide path").unwrap();
            writeln!(out, "    let base = gid * 4u;").unwrap();
            writeln!(out, "    for (var lane: u32 = 0u; lane < 4u; lane = lane + 1u) {{").unwrap();
            writeln!(out, "        let eidx = base + lane;").unwrap();

            for (i, op) in kernel.ops.iter().enumerate() {
                let dtype_str = op.dst_dtype.narrow_webgpu().wgsl_type();
                let expr = if let Some((a, b, c)) = Self::detect_fma(op, i, kernel) {
                    format!("fma({}, {}, {})", a, b, c)
                } else {
                    Self::render_op(op, i, kernel, "eidx")
                };
                writeln!(out, "        var v{}: {} = {};", i, dtype_str, expr).unwrap();
            }
            let last_op = kernel.ops.len() - 1;
            writeln!(out, "        buf{}[eidx] = v{};", kernel.bufs[0].buf_id, last_op).unwrap();
            writeln!(out, "    }}").unwrap();
        } else if !has_reduce {
            for (i, op) in kernel.ops.iter().enumerate() {
                let dtype_str = op.dst_dtype.narrow_webgpu().wgsl_type();
                let expr = if let Some((a, b, c)) = Self::detect_fma(op, i, kernel) {
                    format!("fma({}, {}, {})", a, b, c)
                } else {
                    Self::render_op(op, i, kernel, "gid")
                };
                writeln!(out, "    var v{}: {} = {};", i, dtype_str, expr).unwrap();
            }
            let last_op = kernel.ops.len() - 1;
            writeln!(out, "    buf{}[gid] = v{};", kernel.bufs[0].buf_id, last_op).unwrap();
        } else {
            let reduce_idx = kernel.ops.iter().position(|op| {
                matches!(op.op, PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax)
            }).expect("has_reduce but no reduce op found");

            let reduce_op = &kernel.ops[reduce_idx];
            let reduce_src = &reduce_op.srcs[0];
            let reduce_dtype = reduce_op.dst_dtype.narrow_webgpu();

            let input_buf = match reduce_src {
                FusedSrc::Buf(idx) => &kernel.bufs[*idx],
                FusedSrc::Op(_) => &kernel.bufs[1],
                FusedSrc::Const { .. } => unreachable!("reduce on constant"),
            };
            let reduce_size = input_buf.st.numel() / output_numel;

            let init_val = match reduce_op.op {
                PrimitiveOp::ReduceSum => format!("{}(0)", reduce_dtype.wgsl_type()),
                PrimitiveOp::ReduceMax => "bitcast<f32>(0xff800000u)".to_string(),
                _ => unreachable!(),
            };

            writeln!(out, "    var acc: {} = {};", reduce_dtype.wgsl_type(), init_val).unwrap();

            if reduce_idx > 0 {
                writeln!(out, "    for (var rid: u32 = 0u; rid < {}u; rid = rid + 1u) {{", reduce_size).unwrap();
                writeln!(out, "        let eidx = gid * {}u + rid;", reduce_size).unwrap();

                for i in 0..reduce_idx {
                    let op = &kernel.ops[i];
                    let dtype_str = op.dst_dtype.narrow_webgpu().wgsl_type();
                    let expr = Self::render_op(op, i, kernel, "eidx");
                    writeln!(out, "        var v{}: {} = {};", i, dtype_str, expr).unwrap();
                }

                let src_var = format!("v{}", reduce_idx - 1);
                match reduce_op.op {
                    PrimitiveOp::ReduceSum => writeln!(out, "        acc = acc + {};", src_var).unwrap(),
                    PrimitiveOp::ReduceMax => writeln!(out, "        acc = max(acc, {});", src_var).unwrap(),
                    _ => unreachable!(),
                }
                writeln!(out, "    }}").unwrap();
            } else {
                writeln!(out, "    for (var rid: u32 = 0u; rid < {}u; rid = rid + 1u) {{", reduce_size).unwrap();
                writeln!(out, "        let eidx = gid * {}u + rid;", reduce_size).unwrap();
                let src_expr = match reduce_src {
                    FusedSrc::Buf(idx) => Self::render_buf_read(&kernel.bufs[*idx], "eidx"),
                    _ => unreachable!(),
                };
                match reduce_op.op {
                    PrimitiveOp::ReduceSum => writeln!(out, "        acc = acc + {};", src_expr).unwrap(),
                    PrimitiveOp::ReduceMax => writeln!(out, "        acc = max(acc, {});", src_expr).unwrap(),
                    _ => unreachable!(),
                }
                writeln!(out, "    }}").unwrap();
            }

            // Apply subgroup reduction when enabled.
            if self.config.use_subgroups {
                match reduce_op.op {
                    PrimitiveOp::ReduceSum => {
                        writeln!(out, "    // WebGPU subgroup reduction").unwrap();
                        writeln!(out, "    acc = subgroupAdd(acc);").unwrap();
                    }
                    PrimitiveOp::ReduceMax => {
                        writeln!(out, "    // WebGPU subgroup reduction").unwrap();
                        writeln!(out, "    acc = subgroupMax(acc);").unwrap();
                    }
                    _ => unreachable!(),
                }
            }

            writeln!(out, "    var v{}: {} = acc;", reduce_idx, reduce_dtype.wgsl_type()).unwrap();

            for i in (reduce_idx + 1)..kernel.ops.len() {
                let op = &kernel.ops[i];
                let dtype_str = op.dst_dtype.narrow_webgpu().wgsl_type();
                let expr = Self::render_op(op, i, kernel, "gid");
                writeln!(out, "    var v{}: {} = {};", i, dtype_str, expr).unwrap();
            }

            let last_op = kernel.ops.len() - 1;
            writeln!(out, "    buf{}[gid] = v{};", kernel.bufs[0].buf_id, last_op).unwrap();
        }

        writeln!(out, "}}").unwrap();
        out
    }
}
