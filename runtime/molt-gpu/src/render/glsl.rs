//! GlslRenderer — GLSL ES 3.0 fragment shader codegen for all 26 primitive ops.
//!
//! Generates GLSL ES 3.0 fragment shader source from FusedKernel IR.
//! WebGL2 has NO compute shaders, so computation is performed via
//! render-to-texture with fragment shaders:
//!
//! 1. Input data encoded as WebGL2 textures (RGBA32F / RGBA32I)
//! 2. Fragment shader reads input textures via `sampler2D` uniforms
//! 3. Output written to framebuffer attachment (another texture)
//! 4. `gl_FragCoord.xy` replaces `global_invocation_id` as the work index
//!
//! All dtypes are narrowed via `DType::narrow_webgl2()` before rendering.
//! WebGL2 shader math supports only: float (32-bit highp), int (32-bit),
//! uint (32-bit), bool. No f64, i64, u64, i8, u8, i16, u16 in shaders.
//!
//! Key GLSL ES 3.0 differences from WGSL/MSL:
//! - `#version 300 es` header required
//! - `precision highp float;` required for ML inference accuracy
//! - No storage buffers — use `uniform sampler2D` for input, framebuffer for output
//! - No `global_invocation_id` — use `gl_FragCoord.xy` as 2D index
//! - Ternary operator `?:` is available (unlike WGSL's `select()`)
//! - Bitcast via `intBitsToFloat()` / `floatBitsToInt()` / `floatBitsToUint()`
//! - `mod()` uses GLSL `%` for ints but needs manual C-semantics correction for
//!   negative operands (GLSL `%` on ints is implementation-defined pre-ES 3.1;
//!   we use the C-semantics formula `a - b * (a / b)`)
//!
//! Reduce ops generate a loop-based reduction inside a single fragment shader,
//! reading successive texels from the input texture. For large reductions,
//! the caller (WebGl2Device) orchestrates multi-pass ping-pong texture
//! dispatches, each halving the reduction dimension. This renderer generates
//! the per-pass shader; the device handles pass orchestration.

use std::fmt::Write;

use crate::dtype::DType;
use crate::ops::PrimitiveOp;
use crate::render::{BufferAccess, FusedKernel, FusedOp, FusedSrc, Renderer};

/// GLSL ES 3.0 fragment shader renderer for all 26 primitive ops.
///
/// Targets WebGL2 environments where WebGPU is unavailable (~25% of browser
/// users, especially iOS 15-25). Computation is performed via full-screen
/// quad draws with fragment shaders that read input textures and write to
/// framebuffer-attached output textures.
pub struct GlslRenderer;

impl GlslRenderer {
    /// Format a constant value as a GLSL ES 3.0 literal.
    fn format_const(val: f64, dtype: DType) -> String {
        let dtype = dtype.narrow_webgl2();
        match dtype {
            DType::Bool => {
                if val != 0.0 {
                    "true".to_string()
                } else {
                    "false".to_string()
                }
            }
            DType::Float32 => {
                if val == f64::INFINITY {
                    "intBitsToFloat(0x7f800000)".to_string()
                } else if val == f64::NEG_INFINITY {
                    "intBitsToFloat(int(0xff800000u))".to_string()
                } else if val.is_nan() {
                    "intBitsToFloat(0x7fc00000)".to_string()
                } else {
                    let s = format!("{}", val);
                    if s.contains('.') || s.contains('e') || s.contains('E') {
                        s
                    } else {
                        format!("{}.0", s)
                    }
                }
            }
            DType::Int32 => {
                format!("int({})", val as i64)
            }
            DType::UInt32 => {
                format!("uint({})", val as u64)
            }
            _ => unreachable!("narrow_webgl2 should have handled {:?}", dtype),
        }
    }

    /// Convert a linear element index to a 2D texture coordinate.
    ///
    /// WebGL2 textures are 2D. We pack 1D buffer data into 2D textures
    /// with a fixed width (power of 2, typically 4096 for RGBA32F max).
    /// The texture width is passed as a uniform `u_tex_width`.
    ///
    /// Linear index `idx` maps to:
    ///   - texel index: `idx / 4` (4 components per RGBA texel)
    ///   - component: `idx % 4` (R=0, G=1, B=2, A=3)
    ///   - column: `texel_index % u_tex_width`
    ///   - row: `texel_index / u_tex_width`
    ///
    /// Texture coordinate: `vec2(float(col) + 0.5, float(row) + 0.5) / tex_size`
    /// The +0.5 centers on the texel to avoid interpolation artifacts.
    fn render_tex_read(buf_id: usize, idx_expr: &str) -> String {
        // Each RGBA texel holds 4 float values. We compute:
        //   texel_idx = idx / 4
        //   component = idx % 4 (select r/g/b/a)
        //   col = texel_idx % u_tex_width
        //   row = texel_idx / u_tex_width
        //   uv = ivec2(col, row)
        // Then use texelFetch(sampler, uv, 0)[component]
        format!(
            "texelFetch(u_tex{buf}, ivec2(({idx} / 4) % u_tex_width, ({idx} / 4) / u_tex_width), 0)[({idx}) % 4]",
            buf = buf_id,
            idx = idx_expr,
        )
    }

    /// Render a buffer read expression at the given index variable,
    /// accounting for ShapeTracker view transformations.
    fn render_buf_read(binding: &crate::render::BufferBinding, idx_var: &str) -> String {
        let st = &binding.st;
        let view = st.view();

        let ndim = view.shape.len();
        if ndim == 0 {
            return Self::render_tex_read(binding.buf_id, "0");
        }

        if view.is_contiguous() && view.mask.is_none() {
            return Self::render_tex_read(binding.buf_id, idx_var);
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
                format!("({} % {})", idx_var, size)
            } else {
                let divisor: usize = view.shape[dim + 1..].iter().product();
                format!("({} / {} % {})", idx_var, divisor, size)
            };
            if stride == 1 {
                parts.push(idx_expr);
            } else if stride == -1 {
                parts.push(format!("({} - {})", size - 1, idx_expr));
            } else if stride > 0 {
                parts.push(format!("{} * {}", idx_expr, stride));
            } else {
                parts.push(format!("({} - {}) * {}", size - 1, idx_expr, -stride));
            }
        }

        let offset = if view.offset != 0 {
            format!("{} + ", view.offset)
        } else {
            String::new()
        };

        let idx_sum = if parts.is_empty() {
            "0".to_string()
        } else {
            parts.join(" + ")
        };

        let final_idx = format!("{}{}", offset, idx_sum);
        Self::render_tex_read(binding.buf_id, &final_idx)
    }

    /// Render a single op expression as GLSL ES 3.0.
    fn render_op(op: &FusedOp, _op_idx: usize, kernel: &FusedKernel, idx_var: &str) -> String {
        let src = |i: usize| -> String {
            match &op.srcs[i] {
                FusedSrc::Buf(buf_idx) => Self::render_buf_read(&kernel.bufs[*buf_idx], idx_var),
                FusedSrc::Op(prior_idx) => format!("v{}", prior_idx),
                FusedSrc::Const { val, dtype } => Self::format_const(*val, *dtype),
            }
        };

        let dst_type = op.dst_dtype.narrow_webgl2().glsl_type();

        match op.op {
            // Arithmetic
            PrimitiveOp::Add => format!("({} + {})", src(0), src(1)),
            PrimitiveOp::Sub => format!("({} - {})", src(0), src(1)),
            PrimitiveOp::Mul => format!("({} * {})", src(0), src(1)),
            // Integer division: C semantics (truncate toward zero).
            // GLSL ES 3.0 int division already truncates toward zero.
            PrimitiveOp::Idiv => format!("({} / {})", src(0), src(1)),
            // Mod with C semantics: a - b * (a / b).
            // GLSL `%` on ints has C semantics in ES 3.0 (sign of dividend).
            PrimitiveOp::Mod => format!("({} - {} * ({} / {}))", src(0), src(1), src(0), src(1)),
            PrimitiveOp::Neg => format!("(-{})", src(0)),

            // Comparison — output is always bool, rendered as float 1.0/0.0
            // for fragment shader output compatibility
            PrimitiveOp::Cmplt => format!("(({} < {}) ? 1.0 : 0.0)", src(0), src(1)),
            PrimitiveOp::Cmpeq => format!("(({} == {}) ? 1.0 : 0.0)", src(0), src(1)),
            PrimitiveOp::Cmpne => format!("(({} != {}) ? 1.0 : 0.0)", src(0), src(1)),

            // Bitwise — require int/uint operands
            PrimitiveOp::And => format!("(int({}) & int({}))", src(0), src(1)),
            PrimitiveOp::Or => format!("(int({}) | int({}))", src(0), src(1)),
            PrimitiveOp::Xor => format!("(int({}) ^ int({}))", src(0), src(1)),
            PrimitiveOp::Shl => format!("(int({}) << int({}))", src(0), src(1)),
            PrimitiveOp::Shr => format!("(int({}) >> int({}))", src(0), src(1)),

            // Math — all native GLSL ES 3.0 builtins
            PrimitiveOp::Exp2 => format!("exp2({})", src(0)),
            PrimitiveOp::Log2 => format!("log2({})", src(0)),
            PrimitiveOp::Sin => format!("sin({})", src(0)),
            PrimitiveOp::Sqrt => format!("sqrt({})", src(0)),
            PrimitiveOp::Reciprocal => format!("(1.0 / {})", src(0)),

            // Other
            PrimitiveOp::Trunc => format!("trunc({})", src(0)),
            // GLSL ES 3.0 max() behavior with NaN is implementation-defined.
            // The spec requires NaN-propagating max, but WebGL2 is a best-effort
            // fallback for ~25% of users without WebGPU. No portable NaN guard
            // exists in GLSL ES 3.0 (isnan() was added in ES 3.1).
            PrimitiveOp::Max => format!("max({}, {})", src(0), src(1)),
            // GLSL ES 3.0 supports ternary operator (unlike WGSL)
            PrimitiveOp::Where => format!("(({} != 0.0) ? {} : {})", src(0), src(1), src(2)),
            PrimitiveOp::Cast => format!("{}({})", dst_type, src(0)),
            // Bitcast in GLSL ES 3.0 uses intBitsToFloat / floatBitsToInt / floatBitsToUint
            PrimitiveOp::Bitcast => {
                let narrowed = op.dst_dtype.narrow_webgl2();
                match narrowed {
                    DType::Float32 => format!("intBitsToFloat(int({}))", src(0)),
                    DType::Int32 => format!("floatBitsToInt({})", src(0)),
                    DType::UInt32 => format!("floatBitsToUint({})", src(0)),
                    // Bool bitcast: treat as int -> bool
                    DType::Bool => format!("(int({}) != 0)", src(0)),
                    _ => unreachable!("narrow_webgl2 should have handled {:?}", narrowed),
                }
            }

            // Reduce ops are handled by the kernel loop generator
            PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax => {
                unreachable!("Reduce ops are handled by the kernel loop generator")
            }
        }
    }

    /// Render the GLSL type for a comparison result.
    /// Comparisons produce float 1.0/0.0 in fragment shaders since
    /// the output goes to a float texture.
    fn glsl_var_type(op: &FusedOp) -> &'static str {
        let narrowed = op.dst_dtype.narrow_webgl2();
        if matches!(
            op.op,
            PrimitiveOp::Cmplt | PrimitiveOp::Cmpeq | PrimitiveOp::Cmpne
        ) {
            // Comparison ops produce float for texture output
            "float"
        } else {
            narrowed.glsl_type()
        }
    }
}

impl Renderer for GlslRenderer {
    fn render(&self, kernel: &FusedKernel) -> String {
        let mut out = String::with_capacity(4096);

        // GLSL ES 3.0 header
        writeln!(out, "#version 300 es").unwrap();
        writeln!(out, "precision highp float;").unwrap();
        writeln!(out, "precision highp int;").unwrap();
        writeln!(out, "precision highp sampler2D;").unwrap();
        writeln!(out).unwrap();

        // Texture width uniform for linear-to-2D mapping
        writeln!(
            out,
            "// Texture packing: linear index -> 2D texel coordinate"
        )
        .unwrap();
        writeln!(
            out,
            "// Each RGBA texel holds 4 elements. u_tex_width is the"
        )
        .unwrap();
        writeln!(out, "// number of texels per row in the packing layout.").unwrap();
        writeln!(out, "uniform int u_tex_width;").unwrap();
        writeln!(
            out,
            "// Total number of output elements for bounds checking."
        )
        .unwrap();
        writeln!(out, "uniform int u_num_elements;").unwrap();
        writeln!(out).unwrap();

        // Input texture uniforms (bufs[1..] are inputs)
        for binding in kernel.bufs.iter() {
            if binding.access == BufferAccess::Read {
                writeln!(out, "uniform sampler2D u_tex{};", binding.buf_id).unwrap();
            }
        }
        writeln!(out).unwrap();

        // Fragment shader output — RGBA to pack 4 values per texel
        writeln!(out, "out vec4 frag_color;").unwrap();
        writeln!(out).unwrap();

        writeln!(out, "void main() {{").unwrap();

        // Convert gl_FragCoord.xy to a linear output texel index.
        // gl_FragCoord gives the center of the pixel (x+0.5, y+0.5),
        // so we floor to get integer coords.
        writeln!(out, "    int out_texel = int(gl_FragCoord.x - 0.5) + int(gl_FragCoord.y - 0.5) * u_tex_width;").unwrap();
        writeln!(out, "    int base_idx = out_texel * 4;").unwrap();
        writeln!(out).unwrap();

        // We compute 4 values per fragment (one full RGBA texel).
        // Each fragment writes all 4 components.
        writeln!(out, "    vec4 result = vec4(0.0);").unwrap();
        writeln!(out, "    for (int comp = 0; comp < 4; comp++) {{").unwrap();
        writeln!(out, "        int gid = base_idx + comp;").unwrap();
        writeln!(out, "        if (gid >= u_num_elements) break;").unwrap();

        // Check for reduce ops
        let has_reduce = kernel
            .ops
            .iter()
            .any(|op| matches!(op.op, PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax));
        let output_numel = kernel.bufs[0].st.numel();

        if !has_reduce {
            // Pure elementwise kernel
            for (i, op) in kernel.ops.iter().enumerate() {
                let type_str = Self::glsl_var_type(op);
                let expr = Self::render_op(op, i, kernel, "gid");
                writeln!(out, "        {} v{} = {};", type_str, i, expr).unwrap();
            }
            let last_op = kernel.ops.len() - 1;
            writeln!(out, "        result[comp] = float(v{});", last_op).unwrap();
        } else {
            // Fused kernel with reduce: elementwise prefix -> reduce -> elementwise suffix
            let reduce_idx = kernel
                .ops
                .iter()
                .position(|op| matches!(op.op, PrimitiveOp::ReduceSum | PrimitiveOp::ReduceMax))
                .expect("has_reduce but no reduce op found");

            let reduce_op = &kernel.ops[reduce_idx];
            let reduce_src = &reduce_op.srcs[0];
            let reduce_dtype = reduce_op.dst_dtype.narrow_webgl2();

            let input_buf = match reduce_src {
                FusedSrc::Buf(idx) => &kernel.bufs[*idx],
                FusedSrc::Op(_) => &kernel.bufs[1],
                FusedSrc::Const { .. } => unreachable!("reduce on constant"),
            };
            let reduce_size = input_buf.st.numel() / output_numel;

            let init_val = match reduce_op.op {
                PrimitiveOp::ReduceSum => format!("{}(0)", reduce_dtype.glsl_type()),
                PrimitiveOp::ReduceMax => "intBitsToFloat(int(0xff800000u))".to_string(),
                _ => unreachable!(),
            };

            writeln!(
                out,
                "        {} acc = {};",
                reduce_dtype.glsl_type(),
                init_val
            )
            .unwrap();

            if reduce_idx > 0 {
                // Pre-reduce elementwise ops inside reduction loop
                writeln!(
                    out,
                    "        for (int rid = 0; rid < {}; rid++) {{",
                    reduce_size
                )
                .unwrap();
                writeln!(out, "            int eidx = gid * {} + rid;", reduce_size).unwrap();

                for i in 0..reduce_idx {
                    let op = &kernel.ops[i];
                    let type_str = Self::glsl_var_type(op);
                    let expr = Self::render_op(op, i, kernel, "eidx");
                    writeln!(out, "            {} v{} = {};", type_str, i, expr).unwrap();
                }

                let src_var = format!("v{}", reduce_idx - 1);
                match reduce_op.op {
                    PrimitiveOp::ReduceSum => {
                        writeln!(out, "            acc = acc + {};", src_var).unwrap();
                    }
                    PrimitiveOp::ReduceMax => {
                        writeln!(out, "            acc = max(acc, {});", src_var).unwrap();
                    }
                    _ => unreachable!(),
                }
                writeln!(out, "        }}").unwrap();
            } else {
                // Reduce directly from texture
                writeln!(
                    out,
                    "        for (int rid = 0; rid < {}; rid++) {{",
                    reduce_size
                )
                .unwrap();
                writeln!(out, "            int eidx = gid * {} + rid;", reduce_size).unwrap();
                let src_expr = match reduce_src {
                    FusedSrc::Buf(idx) => Self::render_buf_read(&kernel.bufs[*idx], "eidx"),
                    _ => unreachable!(),
                };
                match reduce_op.op {
                    PrimitiveOp::ReduceSum => {
                        writeln!(out, "            acc = acc + {};", src_expr).unwrap();
                    }
                    PrimitiveOp::ReduceMax => {
                        writeln!(out, "            acc = max(acc, {});", src_expr).unwrap();
                    }
                    _ => unreachable!(),
                }
                writeln!(out, "        }}").unwrap();
            }

            // Store reduce result
            writeln!(
                out,
                "        {} v{} = acc;",
                reduce_dtype.glsl_type(),
                reduce_idx
            )
            .unwrap();

            // Post-reduce elementwise ops
            for i in (reduce_idx + 1)..kernel.ops.len() {
                let op = &kernel.ops[i];
                let type_str = Self::glsl_var_type(op);
                let expr = Self::render_op(op, i, kernel, "gid");
                writeln!(out, "        {} v{} = {};", type_str, i, expr).unwrap();
            }

            let last_op = kernel.ops.len() - 1;
            writeln!(out, "        result[comp] = float(v{});", last_op).unwrap();
        }

        writeln!(out, "    }}").unwrap();
        writeln!(out, "    frag_color = result;").unwrap();
        writeln!(out, "}}").unwrap();
        out
    }
}
