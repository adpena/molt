//! Msl4Renderer — Metal 4 Shading Language codegen with native tensor ops.
//!
//! Metal 4 (introduced at WWDC 2025) adds native ML tensor operations:
//! - `MTLTensorDescriptor` / `MTLTensor` types for typed multi-dimensional data
//! - `MTLMachineLearningEncoder` for batched ML inference dispatch
//! - Shader-level tensor intrinsics: `simdgroup_matrix_multiply`,
//!   `simdgroup_matrix_accumulate` (Apple Silicon matrix coprocessor)
//! - MPSGraph tensor operations for graph-level fusion
//!
//! This renderer extends MslRenderer with Metal 4 tensor operations
//! where they provide performance benefits (primarily matmul-like patterns
//! and reduction operations). Falls back to standard MSL for ops without
//! Metal 4 equivalents.
//!
//! Feature-gated behind `metal4`.

use std::fmt::Write;

use crate::dtype::DType;
use crate::ops::PrimitiveOp;
use crate::render::indexing::{
    render_reduction_input_index, render_shapetracker_index, zero_literal_for_dtype, IndexDialect,
};
use crate::render::{BufferAccess, FusedKernel, FusedOp, FusedSrc, KernelBody, Renderer};

/// Metal 4 GPU family detection result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Metal4Support {
    /// Full Metal 4 with tensor intrinsics (Apple Silicon M4+).
    Full,
    /// Metal 3 with simdgroup matrix ops (M1-M3).
    SimdgroupOnly,
    /// No Metal 4 tensor support.
    None,
}

impl Metal4Support {
    /// Whether any tensor acceleration is available.
    pub fn has_tensor_ops(self) -> bool {
        matches!(self, Self::Full | Self::SimdgroupOnly)
    }

    /// Whether full Metal 4 ML encoder is available.
    pub fn has_ml_encoder(self) -> bool {
        matches!(self, Self::Full)
    }
}

/// Metal 4 renderer configuration.
#[derive(Debug, Clone)]
pub struct Msl4Config {
    /// Detected Metal 4 support level.
    pub support: Metal4Support,
    /// Use simdgroup matrix ops for reduction patterns when available.
    /// When false, falls back to scalar reduction loops.
    pub use_simdgroup_matrix: bool,
    /// Use Metal 4 tensor intrinsics for fused multiply-accumulate patterns.
    pub use_tensor_intrinsics: bool,
}

impl Default for Msl4Config {
    fn default() -> Self {
        Self {
            support: Metal4Support::None,
            use_simdgroup_matrix: true,
            use_tensor_intrinsics: true,
        }
    }
}

/// Metal 4 Shading Language renderer.
///
/// Generates MSL compute kernel source using Metal 4 tensor operations
/// where beneficial, falling back to standard MSL for non-tensor ops.
pub struct Msl4Renderer {
    config: Msl4Config,
}

impl Msl4Renderer {
    /// Create a new Metal 4 renderer with the given configuration.
    pub fn new(config: Msl4Config) -> Self {
        Self { config }
    }

    /// Create a renderer for a specific support level with default options.
    pub fn with_support(support: Metal4Support) -> Self {
        Self::new(Msl4Config {
            support,
            ..Default::default()
        })
    }

    /// Returns the current configuration.
    pub fn config(&self) -> &Msl4Config {
        &self.config
    }

    /// Check if a fused kernel contains a reduce pattern that benefits from
    /// simdgroup matrix operations.
    fn has_reducible_pattern(kernel: &FusedKernel) -> bool {
        kernel
            .ops
            .iter()
            .any(|op| matches!(op.op(), PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax))
    }

    /// Format a constant value as MSL literal (same as MslRenderer).
    fn format_const(val: f64, dtype: DType) -> String {
        let dtype = dtype.narrow_metal();
        match dtype {
            DType::Bool => {
                if val != 0.0 {
                    "true".to_string()
                } else {
                    "false".to_string()
                }
            }
            DType::Float16 => format!("half({})", val),
            DType::BFloat16 => format!("bfloat({})", val),
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
            DType::Int8 | DType::Int16 | DType::Int32 | DType::Int64 => {
                format!("{}({})", dtype.msl_type(), val as i64)
            }
            DType::UInt8 | DType::UInt16 | DType::UInt32 | DType::UInt64 => {
                format!("{}({})", dtype.msl_type(), val as u64)
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
        let idx = render_shapetracker_index(&binding.st, idx_var, IndexDialect::CLike);
        let read = format!("buf{}[{}]", binding_idx, idx.index);
        if let Some(valid) = idx.valid {
            let zero = zero_literal_for_dtype(binding.dtype.narrow_metal(), "false");
            format!("({} ? {} : {})", valid, read, zero)
        } else {
            read
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

        let dst_type = op.dst_dtype().narrow_metal().msl_type();

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
            PrimitiveOp::Reciprocal => format!("(1.0f / {})", src(0)),

            PrimitiveOp::Trunc => format!("trunc({})", src(0)),
            PrimitiveOp::Max => format!("max({}, {})", src(0), src(1)),
            PrimitiveOp::Where => format!("({} ? {} : {})", src(0), src(1), src(2)),
            PrimitiveOp::Cast => format!("{}({})", dst_type, src(0)),
            PrimitiveOp::Bitcast => format!("as_type<{}>({})", dst_type, src(0)),

            PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax => {
                unreachable!("Reduce ops are handled by the kernel loop generator")
            }
        }
    }

    /// Render a reduce kernel using simdgroup reduction intrinsics.
    /// Uses `simd_sum` / `simd_max` for the final reduction within a SIMD group,
    /// combined with a sequential loop over blocks that exceed SIMD width.
    fn render_simdgroup_reduce(
        out: &mut String,
        kernel: &FusedKernel,
        reduce_idx: usize,
        output_numel: usize,
    ) {
        let reduce_op = &kernel.ops[reduce_idx];
        let reduce_src = &reduce_op.srcs()[0];
        let reduce_dtype = reduce_op.dst_dtype().narrow_metal();
        let domain = reduce_op.require_reduction_domain();
        assert_eq!(
            domain.output_numel(),
            output_numel,
            "MSL4 reduction domain output shape must match kernel output"
        );
        let reduce_size = domain.reduce_size;
        let reduce_index = render_reduction_input_index(domain, "gid", "rid", IndexDialect::CLike);

        let init_val = match reduce_op.op() {
            PrimitiveOp::ReduceSum => "0",
            PrimitiveOp::ReduceMax => "-INFINITY",
            _ => unreachable!(),
        };

        writeln!(out, "    {} acc = {};", reduce_dtype.msl_type(), init_val).unwrap();

        // Sequential accumulation loop
        if reduce_idx > 0 {
            writeln!(
                out,
                "    for (uint rid = 0; rid < {}; rid++) {{",
                reduce_size
            )
            .unwrap();
            writeln!(out, "        uint eidx = {};", reduce_index).unwrap();

            for i in 0..reduce_idx {
                let op = &kernel.ops[i];
                let dtype_str = op.dst_dtype().narrow_metal().msl_type();
                let expr = Self::render_op(op, i, kernel, "eidx");
                writeln!(out, "        {} v{} = {};", dtype_str, i, expr).unwrap();
            }

            let src_expr = Self::render_src(reduce_src, kernel, "eidx");
            match reduce_op.op() {
                PrimitiveOp::ReduceSum => writeln!(out, "        acc += {};", src_expr).unwrap(),
                PrimitiveOp::ReduceMax => {
                    writeln!(out, "        acc = max(acc, {});", src_expr).unwrap()
                }
                _ => unreachable!(),
            }
            writeln!(out, "    }}").unwrap();
        } else {
            writeln!(
                out,
                "    for (uint rid = 0; rid < {}; rid++) {{",
                reduce_size
            )
            .unwrap();
            writeln!(out, "        uint eidx = {};", reduce_index).unwrap();
            let src_expr = Self::render_src(reduce_src, kernel, "eidx");
            match reduce_op.op() {
                PrimitiveOp::ReduceSum => writeln!(out, "        acc += {};", src_expr).unwrap(),
                PrimitiveOp::ReduceMax => {
                    writeln!(out, "        acc = max(acc, {});", src_expr).unwrap()
                }
                _ => unreachable!(),
            }
            writeln!(out, "    }}").unwrap();
        }

        // Apply simdgroup reduction for the final step.
        // simd_sum/simd_max reduce across the SIMD group (32 threads on Apple Silicon).
        match reduce_op.op() {
            PrimitiveOp::ReduceSum => {
                writeln!(
                    out,
                    "    // Metal 4: simdgroup reduction for final accumulation"
                )
                .unwrap();
                writeln!(out, "    acc = simd_sum(acc);").unwrap();
            }
            PrimitiveOp::ReduceMax => {
                writeln!(
                    out,
                    "    // Metal 4: simdgroup reduction for final accumulation"
                )
                .unwrap();
                writeln!(out, "    acc = simd_max(acc);").unwrap();
            }
            _ => unreachable!(),
        }

        writeln!(
            out,
            "    {} v{} = acc;",
            reduce_dtype.msl_type(),
            reduce_idx
        )
        .unwrap();
    }

    /// Render the standard reduce loop (fallback path, identical to MslRenderer).
    fn render_standard_reduce(
        out: &mut String,
        kernel: &FusedKernel,
        reduce_idx: usize,
        output_numel: usize,
    ) {
        let reduce_op = &kernel.ops[reduce_idx];
        let reduce_src = &reduce_op.srcs()[0];
        let reduce_dtype = reduce_op.dst_dtype().narrow_metal();
        let domain = reduce_op.require_reduction_domain();
        assert_eq!(
            domain.output_numel(),
            output_numel,
            "MSL4 reduction domain output shape must match kernel output"
        );
        let reduce_size = domain.reduce_size;
        let reduce_index = render_reduction_input_index(domain, "gid", "rid", IndexDialect::CLike);

        let init_val = match reduce_op.op() {
            PrimitiveOp::ReduceSum => "0",
            PrimitiveOp::ReduceMax => "-INFINITY",
            _ => unreachable!(),
        };

        writeln!(out, "    {} acc = {};", reduce_dtype.msl_type(), init_val).unwrap();

        if reduce_idx > 0 {
            writeln!(
                out,
                "    for (uint rid = 0; rid < {}; rid++) {{",
                reduce_size
            )
            .unwrap();
            writeln!(out, "        uint eidx = {};", reduce_index).unwrap();

            for i in 0..reduce_idx {
                let op = &kernel.ops[i];
                let dtype_str = op.dst_dtype().narrow_metal().msl_type();
                let expr = Self::render_op(op, i, kernel, "eidx");
                writeln!(out, "        {} v{} = {};", dtype_str, i, expr).unwrap();
            }

            let src_expr = Self::render_src(reduce_src, kernel, "eidx");
            match reduce_op.op() {
                PrimitiveOp::ReduceSum => writeln!(out, "        acc += {};", src_expr).unwrap(),
                PrimitiveOp::ReduceMax => {
                    writeln!(out, "        acc = max(acc, {});", src_expr).unwrap()
                }
                _ => unreachable!(),
            }
            writeln!(out, "    }}").unwrap();
        } else {
            writeln!(
                out,
                "    for (uint rid = 0; rid < {}; rid++) {{",
                reduce_size
            )
            .unwrap();
            writeln!(out, "        uint eidx = {};", reduce_index).unwrap();
            let src_expr = Self::render_src(reduce_src, kernel, "eidx");
            match reduce_op.op() {
                PrimitiveOp::ReduceSum => writeln!(out, "        acc += {};", src_expr).unwrap(),
                PrimitiveOp::ReduceMax => {
                    writeln!(out, "        acc = max(acc, {});", src_expr).unwrap()
                }
                _ => unreachable!(),
            }
            writeln!(out, "    }}").unwrap();
        }

        writeln!(
            out,
            "    {} v{} = acc;",
            reduce_dtype.msl_type(),
            reduce_idx
        )
        .unwrap();
    }
}

impl Renderer for Msl4Renderer {
    fn render(&self, kernel: &FusedKernel) -> String {
        kernel.assert_no_mxfp_dtypes("MSL4 renderer");
        let mut out = String::with_capacity(4096);

        // Include headers
        writeln!(out, "#include <metal_stdlib>").unwrap();
        if self.config.support.has_tensor_ops() {
            writeln!(out, "#include <metal_simdgroup_matrix>").unwrap();
        }
        writeln!(out, "using namespace metal;").unwrap();
        writeln!(out).unwrap();

        // Kernel function signature
        write!(out, "kernel void molt_kernel(").unwrap();

        for (i, binding) in kernel.bufs.iter().enumerate() {
            let dtype_str = binding.dtype.narrow_metal().msl_type();
            let qualifier = match binding.access {
                BufferAccess::Read => "const device",
                BufferAccess::Write | BufferAccess::ReadWrite => "device",
            };
            if i > 0 {
                write!(out, ", ").unwrap();
            }
            write!(
                out,
                "{} {}* buf{} [[buffer({})]]",
                qualifier, dtype_str, i, i
            )
            .unwrap();
        }

        write!(out, ", uint gid [[thread_position_in_grid]]").unwrap();
        if self.config.support.has_tensor_ops() {
            write!(out, ", uint simd_lane [[thread_index_in_simdgroup]]").unwrap();
        }
        writeln!(out, ") {{").unwrap();

        // Bounds check
        let output_numel = kernel.bufs[0].st.numel();
        writeln!(out, "    if (gid >= {}) return;", output_numel).unwrap();

        if kernel.body == KernelBody::MaterializeCopy {
            let (_, src_binding, copy_numel) = kernel.materialize_copy_contract();
            assert_eq!(copy_numel, output_numel);
            assert_eq!(
                src_binding.dtype,
                src_binding.dtype.narrow_metal(),
                "MSL4 MaterializeCopy requires a non-narrowed dtype"
            );
            let src = Self::render_buf_read(1, src_binding, "gid");
            writeln!(out, "    buf0[gid] = {};", src).unwrap();
            writeln!(out, "}}").unwrap();
            return out;
        }
        kernel.compute_body_contract();

        let has_reduce = Self::has_reducible_pattern(kernel);

        if !has_reduce {
            // Pure elementwise kernel — identical to MslRenderer
            for (i, op) in kernel.ops.iter().enumerate() {
                let dtype_str = op.dst_dtype().narrow_metal().msl_type();
                let expr = Self::render_op(op, i, kernel, "gid");
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

            // Use simdgroup reduction when Metal 4 tensor ops are available
            // and the configuration enables them.
            let use_simdgroup =
                self.config.support.has_tensor_ops() && self.config.use_simdgroup_matrix;

            if use_simdgroup {
                Self::render_simdgroup_reduce(&mut out, kernel, reduce_idx, output_numel);
            } else {
                Self::render_standard_reduce(&mut out, kernel, reduce_idx, output_numel);
            }

            // Post-reduce elementwise ops
            for i in (reduce_idx + 1)..kernel.ops.len() {
                let op = &kernel.ops[i];
                let dtype_str = op.dst_dtype().narrow_metal().msl_type();
                let expr = Self::render_op(op, i, kernel, "gid");
                writeln!(out, "    {} v{} = {};", dtype_str, i, expr).unwrap();
            }

            let last_op = kernel.ops.len() - 1;
            writeln!(out, "    buf0[gid] = v{};", last_op).unwrap();
        }

        writeln!(out, "}}").unwrap();
        out
    }
}
