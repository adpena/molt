/// WebGPU Shading Language (WGSL) code generation for GPU kernels.
///
/// WGSL is the shading language for WebGPU. It runs in browsers via the WebGPU
/// API and natively via the `wgpu` crate on desktop/server platforms.
///
/// # Type Mapping Limitations
///
/// WGSL has more restricted type support than Metal Shading Language (MSL):
///
/// - **i64 → i32**: WGSL has no 64-bit integer type in compute shaders on most
///   WebGPU backends. Values are silently narrowed to 32-bit. Use with care when
///   values exceed `i32::MAX` / `i32::MIN`.
/// - **f64 → f32**: WGSL `f64` (`double`) support is gated behind the
///   `shader-f64` feature flag, which is not universally available (especially in
///   browsers). We always emit `f32` for broad compatibility.
/// - **BigInt**: Not representable; callers must pre-check that kernel inputs fit
///   in i32 before dispatch.
/// - **DynBox / Str / Bytes / List / Dict**: Not representable in a compute
///   shader. These types must be serialised to plain buffer data before launch.

use std::fmt::Write as FmtWrite;

use super::types::TirType;

// ---------------------------------------------------------------------------
// WGSL-specific types
//
// `GpuBuffer` / `GpuBufferAccess` from `gpu.rs` use Metal-oriented access
// modes (ReadOnly / WriteOnly / ReadWrite). WGSL only has `read` and
// `read_write` — `WriteOnly` buffers must be declared `read_write` in WGSL.
// Rather than silently mapping WriteOnly → read_write (which changes semantics
// for callers), we define a WGSL-specific access enum and a thin WGSL kernel
// struct so the API surface is unambiguous.
// ---------------------------------------------------------------------------

/// Access mode for a WGSL storage buffer binding.
///
/// WGSL only supports `read` and `read_write` for storage buffers in compute
/// shaders. Unlike MSL, there is no write-only mode — use `read_write` instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WgslBufferAccess {
    /// Read-only input buffer (`var<storage, read>`).
    ReadOnly,
    /// Read-write output buffer (`var<storage, read_write>`).
    ReadWrite,
}

/// A single buffer binding passed to a WGSL GPU kernel.
#[derive(Debug, Clone)]
pub struct WgslBuffer {
    /// WGSL variable name (e.g. `"a"`, `"b"`, `"c"`).
    pub name: String,
    /// Element type of the array.
    pub element_type: TirType,
    /// Access mode determines the `var<storage, …>` qualifier.
    pub access: WgslBufferAccess,
}

/// A scalar uniform parameter (packed into a `Params` struct).
#[derive(Debug, Clone)]
pub struct WgslParam {
    /// Field name inside the `Params` struct (e.g. `"n"`).
    pub name: String,
    /// Type of the parameter.
    pub ty: TirType,
}

/// A single statement in a GPU kernel body.
///
/// Statements are emitted in declaration order. The `BoundsGuard` variant
/// covers the common `if (tid < params.n) { … }` pattern.
#[derive(Debug, Clone)]
pub enum GpuStatement {
    /// `let <dst> = <lhs> <op> <rhs>;`
    BinOp {
        dst: String,
        lhs: String,
        op: BinOp,
        rhs: String,
    },
    /// `<dst>[<idx>] = <src>;` — indexed store.
    StoreIndex {
        dst: String,
        idx: String,
        src: String,
    },
    /// `let <dst> = <src>[<idx>];` — indexed load.
    LoadIndex {
        dst: String,
        src: String,
        idx: String,
    },
    /// Guard: `if (<cond>) { … }` wrapping the inner body.
    BoundsGuard {
        /// The condition expression string, e.g. `"tid < params.n"`.
        cond: String,
        body: Vec<GpuStatement>,
    },
}

/// Arithmetic binary operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

impl BinOp {
    fn as_str(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Mod => "%",
        }
    }
}

/// A complete GPU compute kernel ready for WGSL emission.
#[derive(Debug, Clone)]
pub struct GpuKernel {
    /// Entry-point function name (e.g. `"vector_add"`).
    pub name: String,
    /// Number of threads per workgroup.  256 is a safe default for most GPUs.
    pub workgroup_size: u32,
    /// Storage buffers bound to `@group(0)`.
    pub buffers: Vec<WgslBuffer>,
    /// Scalar parameters packed into the `Params` uniform struct.
    /// Bound at the next binding slot after the last buffer.
    pub params: Vec<WgslParam>,
    /// Statements that form the kernel body (after the `let tid = gid.x;` preamble).
    pub body: Vec<GpuStatement>,
}

// ---------------------------------------------------------------------------
// WGSL type helpers
// ---------------------------------------------------------------------------

/// Map a `TirType` to its WGSL scalar type name.
///
/// Returns `None` for types that have no valid WGSL representation (e.g.
/// `Str`, `DynBox`, `List`).
///
/// # Limitations
///
/// - `I64` → `"i32"`: WGSL lacks a 64-bit integer in compute shaders on most
///   backends. The value is narrowed to 32 bits.
/// - `F64` → `"f32"`: 64-bit float support is not universally available;
///   `f32` is always emitted for compatibility.
pub fn tir_type_to_wgsl(ty: &TirType) -> Option<&'static str> {
    match ty {
        TirType::I64 => Some("i32"), // WGSL has no i64; narrow to i32
        TirType::F64 => Some("f32"), // f64 not universally available; use f32
        TirType::Bool => Some("bool"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// WGSL code generation
// ---------------------------------------------------------------------------

/// Generate WGSL source code for a GPU kernel.
///
/// WGSL is the shading language for WebGPU — runs in browsers (via the
/// `GPUDevice.createShaderModule` API) and natively via the `wgpu` crate.
///
/// # Output layout
///
/// 1. **Storage buffer bindings** — one `@group(0) @binding(N) var<storage, …>`
///    per buffer in `kernel.buffers`.
/// 2. **Params uniform struct** — a `Params` struct followed by a
///    `var<uniform>` binding if `kernel.params` is non-empty.
/// 3. **`@compute` entry point** — decorated with `@workgroup_size`.
/// 4. **Kernel body** — thread-id extraction followed by the statements in
///    `kernel.body`.
///
/// # Panics
///
/// Panics if a buffer's element type or a param type has no WGSL representation
/// (see [`tir_type_to_wgsl`]).
pub fn generate_wgsl(kernel: &GpuKernel) -> String {
    let mut out = String::new();

    // ------------------------------------------------------------------
    // 1. Storage buffer bindings
    // ------------------------------------------------------------------
    for (idx, buf) in kernel.buffers.iter().enumerate() {
        let wgsl_ty = tir_type_to_wgsl(&buf.element_type).unwrap_or_else(|| {
            panic!(
                "generate_wgsl: buffer '{}' has unsupported element type {:?}",
                buf.name, buf.element_type
            )
        });
        let access = match buf.access {
            WgslBufferAccess::ReadOnly => "read",
            WgslBufferAccess::ReadWrite => "read_write",
        };
        writeln!(
            out,
            "@group(0) @binding({idx}) var<storage, {access}> {name}: array<{ty}>;",
            idx = idx,
            access = access,
            name = buf.name,
            ty = wgsl_ty,
        )
        .unwrap();
    }

    // ------------------------------------------------------------------
    // 2. Params uniform struct (only emitted when scalar params exist)
    // ------------------------------------------------------------------
    let params_binding = kernel.buffers.len(); // next binding slot

    if !kernel.params.is_empty() {
        out.push('\n');
        out.push_str("struct Params {\n");
        for param in &kernel.params {
            let wgsl_ty = tir_type_to_wgsl(&param.ty).unwrap_or_else(|| {
                panic!(
                    "generate_wgsl: param '{}' has unsupported type {:?}",
                    param.name, param.ty
                )
            });
            writeln!(out, "    {}: {},", param.name, wgsl_ty).unwrap();
        }
        out.push_str("}\n");
        writeln!(
            out,
            "@group(0) @binding({params_binding}) var<uniform> params: Params;"
        )
        .unwrap();
    }

    // ------------------------------------------------------------------
    // 3. @compute entry point
    // ------------------------------------------------------------------
    out.push('\n');
    writeln!(out, "@compute @workgroup_size({})", kernel.workgroup_size).unwrap();
    writeln!(
        out,
        "fn {}(@builtin(global_invocation_id) gid: vec3<u32>) {{",
        kernel.name
    )
    .unwrap();

    // Standard thread-id extraction (x-axis only for 1-D kernels)
    out.push_str("    let tid = gid.x;\n");

    // ------------------------------------------------------------------
    // 4. Kernel body statements
    // ------------------------------------------------------------------
    emit_statements(&mut out, &kernel.body, 1);

    out.push_str("}\n");
    out
}

// ---------------------------------------------------------------------------
// Statement emission helpers
// ---------------------------------------------------------------------------

fn emit_statements(out: &mut String, stmts: &[GpuStatement], depth: usize) {
    let indent = "    ".repeat(depth);
    for stmt in stmts {
        emit_statement(out, stmt, &indent, depth);
    }
}

fn emit_statement(out: &mut String, stmt: &GpuStatement, indent: &str, depth: usize) {
    match stmt {
        GpuStatement::BinOp { dst, lhs, op, rhs } => {
            writeln!(
                out,
                "{indent}let {dst} = {lhs} {op} {rhs};",
                indent = indent,
                dst = dst,
                lhs = lhs,
                op = op.as_str(),
                rhs = rhs,
            )
            .unwrap();
        }
        GpuStatement::StoreIndex { dst, idx, src } => {
            writeln!(
                out,
                "{indent}{dst}[{idx}] = {src};",
                indent = indent,
                dst = dst,
                idx = idx,
                src = src,
            )
            .unwrap();
        }
        GpuStatement::LoadIndex { dst, src, idx } => {
            writeln!(
                out,
                "{indent}let {dst} = {src}[{idx}];",
                indent = indent,
                dst = dst,
                src = src,
                idx = idx,
            )
            .unwrap();
        }
        GpuStatement::BoundsGuard { cond, body } => {
            writeln!(out, "{indent}if ({cond}) {{", indent = indent, cond = cond).unwrap();
            emit_statements(out, body, depth + 1);
            writeln!(out, "{indent}}}", indent = indent).unwrap();
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a canonical `vector_add` kernel: c[tid] = a[tid] + b[tid]
    /// guarded by `tid < params.n`.
    fn make_vector_add_kernel() -> GpuKernel {
        GpuKernel {
            name: "vector_add".to_string(),
            workgroup_size: 256,
            buffers: vec![
                WgslBuffer {
                    name: "a".to_string(),
                    element_type: TirType::F64,
                    access: WgslBufferAccess::ReadOnly,
                },
                WgslBuffer {
                    name: "b".to_string(),
                    element_type: TirType::F64,
                    access: WgslBufferAccess::ReadOnly,
                },
                WgslBuffer {
                    name: "c".to_string(),
                    element_type: TirType::F64,
                    access: WgslBufferAccess::ReadWrite,
                },
            ],
            params: vec![WgslParam {
                name: "n".to_string(),
                ty: TirType::I64,
            }],
            body: vec![GpuStatement::BoundsGuard {
                cond: "tid < params.n".to_string(),
                body: vec![
                    GpuStatement::LoadIndex {
                        dst: "va".to_string(),
                        src: "a".to_string(),
                        idx: "tid".to_string(),
                    },
                    GpuStatement::LoadIndex {
                        dst: "vb".to_string(),
                        src: "b".to_string(),
                        idx: "tid".to_string(),
                    },
                    GpuStatement::BinOp {
                        dst: "sum".to_string(),
                        lhs: "va".to_string(),
                        op: BinOp::Add,
                        rhs: "vb".to_string(),
                    },
                    GpuStatement::StoreIndex {
                        dst: "c".to_string(),
                        idx: "tid".to_string(),
                        src: "sum".to_string(),
                    },
                ],
            }],
        }
    }

    #[test]
    fn vector_add_contains_compute_decorator() {
        let wgsl = generate_wgsl(&make_vector_add_kernel());
        assert!(wgsl.contains("@compute"), "expected @compute in:\n{wgsl}");
    }

    #[test]
    fn vector_add_contains_workgroup_size() {
        let wgsl = generate_wgsl(&make_vector_add_kernel());
        assert!(
            wgsl.contains("@workgroup_size(256)"),
            "expected @workgroup_size(256) in:\n{wgsl}"
        );
    }

    #[test]
    fn vector_add_contains_storage_var() {
        let wgsl = generate_wgsl(&make_vector_add_kernel());
        assert!(
            wgsl.contains("var<storage,"),
            "expected var<storage, …> in:\n{wgsl}"
        );
    }

    #[test]
    fn buffer_bindings_increment() {
        let wgsl = generate_wgsl(&make_vector_add_kernel());
        // Buffers a, b, c at bindings 0, 1, 2; params at 3
        assert!(wgsl.contains("@binding(0)"), "missing @binding(0) in:\n{wgsl}");
        assert!(wgsl.contains("@binding(1)"), "missing @binding(1) in:\n{wgsl}");
        assert!(wgsl.contains("@binding(2)"), "missing @binding(2) in:\n{wgsl}");
        assert!(wgsl.contains("@binding(3)"), "missing @binding(3) in:\n{wgsl}");
    }

    #[test]
    fn type_mapping_f64_to_f32() {
        let wgsl = generate_wgsl(&make_vector_add_kernel());
        assert!(
            wgsl.contains("array<f32>"),
            "expected array<f32> for F64 buffers in:\n{wgsl}"
        );
        assert!(!wgsl.contains("f64"), "unexpected f64 in WGSL output:\n{wgsl}");
    }

    #[test]
    fn type_mapping_i64_to_i32() {
        let kernel = GpuKernel {
            name: "int_add".to_string(),
            workgroup_size: 64,
            buffers: vec![
                WgslBuffer {
                    name: "x".to_string(),
                    element_type: TirType::I64,
                    access: WgslBufferAccess::ReadOnly,
                },
                WgslBuffer {
                    name: "y".to_string(),
                    element_type: TirType::I64,
                    access: WgslBufferAccess::ReadWrite,
                },
            ],
            params: vec![],
            body: vec![GpuStatement::StoreIndex {
                dst: "y".to_string(),
                idx: "tid".to_string(),
                src: "x[tid]".to_string(),
            }],
        };
        let wgsl = generate_wgsl(&kernel);
        assert!(
            wgsl.contains("array<i32>"),
            "expected array<i32> for I64 buffers in:\n{wgsl}"
        );
        assert!(!wgsl.contains("i64"), "unexpected i64 in WGSL output:\n{wgsl}");
    }

    #[test]
    fn bool_type_mapping() {
        assert_eq!(tir_type_to_wgsl(&TirType::Bool), Some("bool"));
    }

    #[test]
    fn unsupported_type_returns_none() {
        assert_eq!(tir_type_to_wgsl(&TirType::Str), None);
        assert_eq!(tir_type_to_wgsl(&TirType::DynBox), None);
    }

    #[test]
    fn params_struct_emitted_when_present() {
        let wgsl = generate_wgsl(&make_vector_add_kernel());
        assert!(
            wgsl.contains("struct Params {"),
            "expected Params struct in:\n{wgsl}"
        );
        assert!(
            wgsl.contains("var<uniform> params: Params;"),
            "expected uniform params binding in:\n{wgsl}"
        );
    }

    #[test]
    fn no_params_struct_when_empty() {
        let kernel = GpuKernel {
            name: "noop".to_string(),
            workgroup_size: 1,
            buffers: vec![],
            params: vec![],
            body: vec![],
        };
        let wgsl = generate_wgsl(&kernel);
        assert!(
            !wgsl.contains("struct Params"),
            "unexpected Params struct in:\n{wgsl}"
        );
    }

    #[test]
    fn read_only_access_mode() {
        let wgsl = generate_wgsl(&make_vector_add_kernel());
        assert!(
            wgsl.contains("var<storage, read>"),
            "expected read-only storage binding in:\n{wgsl}"
        );
    }

    #[test]
    fn read_write_access_mode() {
        let wgsl = generate_wgsl(&make_vector_add_kernel());
        assert!(
            wgsl.contains("var<storage, read_write>"),
            "expected read_write storage binding in:\n{wgsl}"
        );
    }

    #[test]
    fn global_invocation_id_builtin_present() {
        let wgsl = generate_wgsl(&make_vector_add_kernel());
        assert!(
            wgsl.contains("@builtin(global_invocation_id)"),
            "expected global_invocation_id builtin in:\n{wgsl}"
        );
    }

    #[test]
    fn full_vector_add_snapshot() {
        let wgsl = generate_wgsl(&make_vector_add_kernel());
        let checks = [
            "@group(0) @binding(0) var<storage, read> a: array<f32>;",
            "@group(0) @binding(1) var<storage, read> b: array<f32>;",
            "@group(0) @binding(2) var<storage, read_write> c: array<f32>;",
            "struct Params {",
            "n: i32,",
            "@group(0) @binding(3) var<uniform> params: Params;",
            "@compute @workgroup_size(256)",
            "fn vector_add(@builtin(global_invocation_id) gid: vec3<u32>)",
            "let tid = gid.x;",
        ];
        for fragment in &checks {
            assert!(
                wgsl.contains(fragment),
                "missing fragment {:?} in:\n{wgsl}",
                fragment
            );
        }
    }
}
