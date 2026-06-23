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
use crate::render::indexing::{
    render_reduction_input_index, render_shapetracker_index, zero_literal_for_dtype, IndexDialect,
};
use crate::render::{BufferAccess, FusedKernel, FusedOp, FusedSrc, KernelBody, Renderer};

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
        Self {
            config: WgslRendererConfig::default(),
        }
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
                if val != 0.0 {
                    "true".to_string()
                } else {
                    "false".to_string()
                }
            }
            DType::Float16 => format!("f16({})", val),
            DType::Float32 | DType::BFloat16 => {
                if val == f64::INFINITY {
                    "bitcast<f32>(0x7f800000u)".to_string()
                } else if val == f64::NEG_INFINITY {
                    "bitcast<f32>(0xff800000u)".to_string()
                } else if val.is_nan() {
                    "bitcast<f32>(0x7fc00000u)".to_string()
                } else {
                    format!("f32({})", val)
                }
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
    fn render_buf_read(
        binding_idx: usize,
        binding: &crate::render::BufferBinding,
        idx_var: &str,
    ) -> String {
        let idx = render_shapetracker_index(&binding.st, idx_var, IndexDialect::Wgsl);
        if let Some(valid) = idx.valid {
            let safe_idx = format!("select(0i, {}, {})", idx.index, valid);
            let read = format!("buf{}[u32({})]", binding_idx, safe_idx);
            let zero = zero_literal_for_dtype(binding.dtype.narrow_webgpu(), "false");
            format!("select({}, {}, {})", zero, read, valid)
        } else if idx.index == idx_var {
            format!("buf{}[{}]", binding_idx, idx.index)
        } else {
            format!("buf{}[u32({})]", binding_idx, idx.index)
        }
    }

    fn render_src(src: &FusedSrc, kernel: &FusedKernel, idx_var: &str) -> String {
        match src {
            FusedSrc::Buf(buf_idx) => {
                Self::render_buf_read(*buf_idx, &kernel.bufs[*buf_idx], idx_var)
            }
            FusedSrc::Op(prior_idx) => format!("v{}", prior_idx),
            FusedSrc::Const { val, dtype } => Self::format_const(*val, *dtype),
        }
    }

    /// Render a single op expression.
    fn render_op(op: &FusedOp, _op_idx: usize, kernel: &FusedKernel, idx_var: &str) -> String {
        let src = |i: usize| -> String { Self::render_src(&op.srcs()[i], kernel, idx_var) };

        let dst_type = op.dst_dtype().narrow_webgpu().wgsl_type();

        match op.op() {
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
    fn detect_fma(
        op: &FusedOp,
        op_idx: usize,
        kernel: &FusedKernel,
        idx_var: &str,
    ) -> Option<(String, String, String)> {
        if op.op() != PrimitiveOp::Add {
            return None;
        }
        if !op.dst_dtype().narrow_webgpu().is_float() {
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
                            FusedSrc::Const { val, dtype } => Self::format_const(*val, *dtype),
                        };
                        let b = match &prior_op.srcs()[1] {
                            FusedSrc::Buf(buf_idx) => {
                                Self::render_buf_read(*buf_idx, &kernel.bufs[*buf_idx], idx_var)
                            }
                            FusedSrc::Op(p) => format!("v{}", p),
                            FusedSrc::Const { val, dtype } => Self::format_const(*val, *dtype),
                        };
                        let c = match &op.srcs()[add_src_pos] {
                            FusedSrc::Buf(buf_idx) => {
                                Self::render_buf_read(*buf_idx, &kernel.bufs[*buf_idx], idx_var)
                            }
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
        kernel.assert_no_mxfp_dtypes("WGSL renderer");
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
                i, access, i, dtype_str
            )
            .unwrap();
        }
        writeln!(out).unwrap();

        // Compute shader entry point
        writeln!(
            out,
            "@compute @workgroup_size({}, {}, {})",
            kernel.local[0], kernel.local[1], kernel.local[2]
        )
        .unwrap();
        writeln!(
            out,
            "fn molt_kernel(@builtin(global_invocation_id) gid_vec: vec3<u32>) {{"
        )
        .unwrap();
        writeln!(out, "    let gid = gid_vec.x;").unwrap();

        // Bounds check: elide when shape specialization proves total_elements
        // is exactly divisible by workgroup size (no out-of-bounds threads).
        // Per Maczan 2026, every instruction in the dispatch path matters when
        // per-operation overhead dominates at batch=1.
        let output_numel = kernel.bufs[0].st.numel();
        let bounds_check_elim = kernel.spec.as_ref().is_some_and(|s| s.bounds_check_elim);
        if !bounds_check_elim {
            writeln!(out, "    if (gid >= {}u) {{ return; }}", output_numel).unwrap();
        }

        if kernel.body == KernelBody::MaterializeCopy {
            let (_, src_binding, copy_numel) = kernel.materialize_copy_contract();
            assert_eq!(copy_numel, output_numel);
            assert_eq!(
                src_binding.dtype,
                src_binding.dtype.narrow_webgpu(),
                "WGSL MaterializeCopy requires a non-narrowed dtype"
            );
            let src = Self::render_buf_read(1, src_binding, "gid");
            writeln!(out, "    buf0[gid] = {};", src).unwrap();
            writeln!(out, "}}").unwrap();
            return out;
        }
        kernel.compute_body_contract();

        // Check for reduce ops
        let has_reduce = kernel
            .ops
            .iter()
            .any(|op| matches!(op.op(), PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax));

        if kernel.vectorize_width == 4 && !has_reduce {
            // Vectorized 4-wide: each thread processes 4 contiguous elements
            // using vec4<f32> access for coalesced memory bandwidth.
            let vec_numel = output_numel / 4;
            if !bounds_check_elim {
                writeln!(out, "    if (gid >= {}u) {{ return; }}", vec_numel).unwrap();
            }
            writeln!(out, "    // Vectorized 4-wide path").unwrap();
            writeln!(out, "    let base = gid * 4u;").unwrap();
            writeln!(
                out,
                "    for (var lane: u32 = 0u; lane < 4u; lane = lane + 1u) {{"
            )
            .unwrap();
            writeln!(out, "        let eidx = base + lane;").unwrap();

            for (i, op) in kernel.ops.iter().enumerate() {
                let dtype_str = op.dst_dtype().narrow_webgpu().wgsl_type();
                let expr = if let Some((a, b, c)) = Self::detect_fma(op, i, kernel, "eidx") {
                    format!("fma({}, {}, {})", a, b, c)
                } else {
                    Self::render_op(op, i, kernel, "eidx")
                };
                writeln!(out, "        var v{}: {} = {};", i, dtype_str, expr).unwrap();
            }
            let last_op = kernel.ops.len() - 1;
            writeln!(out, "        buf0[eidx] = v{};", last_op).unwrap();
            writeln!(out, "    }}").unwrap();
        } else if !has_reduce {
            for (i, op) in kernel.ops.iter().enumerate() {
                let dtype_str = op.dst_dtype().narrow_webgpu().wgsl_type();
                let expr = if let Some((a, b, c)) = Self::detect_fma(op, i, kernel, "gid") {
                    format!("fma({}, {}, {})", a, b, c)
                } else {
                    Self::render_op(op, i, kernel, "gid")
                };
                writeln!(out, "    var v{}: {} = {};", i, dtype_str, expr).unwrap();
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
            let reduce_dtype = reduce_op.dst_dtype().narrow_webgpu();
            let domain = reduce_op.require_reduction_domain();
            assert_eq!(
                domain.output_numel(),
                output_numel,
                "WGSL reduction domain output shape must match kernel output"
            );
            let reduce_size = domain.reduce_size;
            let reduce_index =
                render_reduction_input_index(domain, "gid", "rid", IndexDialect::Wgsl);

            let init_val = match reduce_op.op() {
                PrimitiveOp::ReduceSum => format!("{}(0)", reduce_dtype.wgsl_type()),
                PrimitiveOp::ReduceMax => "bitcast<f32>(0xff800000u)".to_string(),
                _ => unreachable!(),
            };

            writeln!(
                out,
                "    var acc: {} = {};",
                reduce_dtype.wgsl_type(),
                init_val
            )
            .unwrap();

            if reduce_idx > 0 {
                // Emit loop with optional unroll annotation for small reduces.
                // WGSL does not have @unroll, but small constant-count loops
                // are unrolled by the WGSL -> SPIR-V / MSL compiler when the
                // trip count is known at compile time (which it always is here).
                writeln!(
                    out,
                    "    for (var rid: u32 = 0u; rid < {}u; rid = rid + 1u) {{",
                    reduce_size
                )
                .unwrap();
                writeln!(out, "        let eidx = {};", reduce_index).unwrap();

                for i in 0..reduce_idx {
                    let op = &kernel.ops[i];
                    let dtype_str = op.dst_dtype().narrow_webgpu().wgsl_type();
                    let expr = Self::render_op(op, i, kernel, "eidx");
                    writeln!(out, "        var v{}: {} = {};", i, dtype_str, expr).unwrap();
                }

                let src_expr = Self::render_src(reduce_src, kernel, "eidx");
                match reduce_op.op() {
                    PrimitiveOp::ReduceSum => {
                        writeln!(out, "        acc = acc + {};", src_expr).unwrap()
                    }
                    PrimitiveOp::ReduceMax => {
                        writeln!(out, "        acc = max(acc, {});", src_expr).unwrap()
                    }
                    _ => unreachable!(),
                }
                writeln!(out, "    }}").unwrap();
            } else {
                writeln!(
                    out,
                    "    for (var rid: u32 = 0u; rid < {}u; rid = rid + 1u) {{",
                    reduce_size
                )
                .unwrap();
                writeln!(out, "        let eidx = {};", reduce_index).unwrap();
                let src_expr = Self::render_src(reduce_src, kernel, "eidx");
                match reduce_op.op() {
                    PrimitiveOp::ReduceSum => {
                        writeln!(out, "        acc = acc + {};", src_expr).unwrap()
                    }
                    PrimitiveOp::ReduceMax => {
                        writeln!(out, "        acc = max(acc, {});", src_expr).unwrap()
                    }
                    _ => unreachable!(),
                }
                writeln!(out, "    }}").unwrap();
            }

            // Apply subgroup reduction when enabled.
            if self.config.use_subgroups {
                match reduce_op.op() {
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

            writeln!(
                out,
                "    var v{}: {} = acc;",
                reduce_idx,
                reduce_dtype.wgsl_type()
            )
            .unwrap();

            for i in (reduce_idx + 1)..kernel.ops.len() {
                let op = &kernel.ops[i];
                let dtype_str = op.dst_dtype().narrow_webgpu().wgsl_type();
                let expr = Self::render_op(op, i, kernel, "gid");
                writeln!(out, "    var v{}: {} = {};", i, dtype_str, expr).unwrap();
            }

            let last_op = kernel.ops.len() - 1;
            writeln!(out, "    buf0[gid] = v{};", last_op).unwrap();
        }

        writeln!(out, "}}").unwrap();
        out
    }
}
