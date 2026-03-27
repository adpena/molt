//! TIR → AMD HIP C++ code generation.
//!
//! Converts a [`GpuKernel`] into HIP C++ source code targeting AMD GPUs via ROCm.
//! HIP is source-compatible with CUDA but uses `hip`-prefixed intrinsics and
//! is compiled by `hipcc`. The output is suitable for `hiprtc` JIT compilation
//! or ahead-of-time compilation with `hipcc`.
//!
//! # Differences from CUDA
//!
//! | CUDA intrinsic   | HIP intrinsic     |
//! |------------------|-------------------|
//! | `threadIdx.x`    | `hipThreadIdx_x`  |
//! | `blockIdx.x`     | `hipBlockIdx_x`   |
//! | `blockDim.x`     | `hipBlockDim_x`   |
//!
//! Everything else (qualifiers, type names, `__global__`, `__restrict__`) is
//! identical to CUDA C.
//!
//! # Type Mapping
//!
//! | TIR type  | HIP C++ type |
//! |-----------|--------------|
//! | `I64`     | `long long`  |
//! | `F64`     | `double`     |
//! | `Bool`    | `bool`       |
//! | other     | `uint64_t`   |
//!
//! # Output layout
//!
//! 1. `#include <hip/hip_runtime.h>` preamble.
//! 2. `extern "C" __global__ void <name>(…)` signature with buffer and scalar
//!    parameters using `__restrict__` qualifiers.
//! 3. Thread-ID computation using HIP intrinsics:
//!    `int tid = hipBlockIdx_x * hipBlockDim_x + hipThreadIdx_x`.
//! 4. Body statements lowered from TIR ops.

use super::gpu::{GpuBufferAccess, GpuKernel};
use super::ops::{AttrValue, OpCode, TirOp};
use super::types::TirType;
use super::values::ValueId;

/// Generate HIP C++ source code for a GPU kernel.
///
/// The output is suitable for compilation with `hipcc` or HIPRTC.
pub fn generate_hip(kernel: &GpuKernel) -> String {
    let mut out = String::with_capacity(1024);

    // Preamble
    out.push_str("#include <hip/hip_runtime.h>\n");
    out.push_str("#include <stdint.h>\n\n");

    // Function signature: extern "C" __global__ void <name>(
    out.push_str(&format!("extern \"C\" __global__ void {}(\n", kernel.name));

    let total_params = kernel.buffers.len() + kernel.scalar_params.len();
    let mut param_idx = 0;

    // Buffer parameters
    for buf in &kernel.buffers {
        let type_str = tir_type_to_hip(&buf.element_type);
        let const_qual = match buf.access {
            GpuBufferAccess::ReadOnly => "const ",
            GpuBufferAccess::WriteOnly | GpuBufferAccess::ReadWrite => "",
        };
        param_idx += 1;
        let comma = if param_idx < total_params { "," } else { "" };
        out.push_str(&format!(
            "    {}{}* __restrict__ {}{}",
            const_qual, type_str, buf.name, comma
        ));
        out.push('\n');
    }

    // Scalar parameters
    for (i, (name, ty)) in kernel.scalar_params.iter().enumerate() {
        let type_str = tir_type_to_hip(ty);
        param_idx += 1;
        let comma = if param_idx < total_params { "," } else { "" };
        let _ = i;
        out.push_str(&format!("    const {} {}{}", type_str, name, comma));
        out.push('\n');
    }

    out.push_str(") {\n");

    // Thread ID using HIP intrinsics
    out.push_str("    int tid = hipBlockIdx_x * hipBlockDim_x + hipThreadIdx_x;\n");

    // Body: convert TIR ops to HIP C++ statements
    let mut ctx = HipGenContext::new(kernel);
    for op in &kernel.body_ops {
        if let Some(stmt) = ctx.lower_op(op) {
            out.push_str("    ");
            out.push_str(&stmt);
            out.push('\n');
        }
    }

    out.push_str("}\n");
    out
}

/// Map a TIR type to its HIP C++ equivalent.
fn tir_type_to_hip(ty: &TirType) -> &'static str {
    match ty {
        TirType::I64 => "long long",
        TirType::F64 => "double",
        TirType::Bool => "bool",
        _ => "uint64_t",
    }
}

/// Context for HIP C++ code generation — tracks SSA value → variable mappings.
struct HipGenContext<'a> {
    kernel: &'a GpuKernel,
}

impl<'a> HipGenContext<'a> {
    fn new(kernel: &'a GpuKernel) -> Self {
        Self { kernel }
    }

    /// Get the C++ variable name for a ValueId.
    fn value_name(&self, id: &ValueId) -> String {
        format!("_v{}", id.0)
    }

    /// Resolve a buffer name from op attrs.
    fn buffer_name_from_op(&self, op: &TirOp) -> Option<String> {
        if let Some(AttrValue::Str(name)) = op.attrs.get("buffer") {
            return Some(name.clone());
        }
        None
    }

    /// Look up the HIP C++ type string for a buffer's element type.
    fn buffer_element_type(&self, name: &str) -> Option<&'static str> {
        self.kernel
            .buffers
            .iter()
            .find(|b| b.name == name)
            .map(|b| tir_type_to_hip(&b.element_type))
    }

    /// Lower a single TIR op to a HIP C++ statement. Returns None for no-ops.
    fn lower_op(&mut self, op: &TirOp) -> Option<String> {
        match op.opcode {
            OpCode::Index => {
                let result = &op.results[0];
                let buf_name = self.buffer_name_from_op(op)?;
                let index_name = if op.operands.len() > 1 {
                    self.value_name(&op.operands[1])
                } else {
                    "tid".into()
                };
                let ty = self.buffer_element_type(&buf_name).unwrap_or("auto");
                Some(format!(
                    "{} {} = {}[{}];",
                    ty,
                    self.value_name(result),
                    buf_name,
                    index_name
                ))
            }
            OpCode::StoreIndex => {
                let buf_name = self.buffer_name_from_op(op)?;
                let index_name = if op.operands.len() > 1 {
                    self.value_name(&op.operands[1])
                } else {
                    "tid".into()
                };
                let val_name = if op.operands.len() > 2 {
                    self.value_name(&op.operands[2])
                } else {
                    self.value_name(&op.operands[1])
                };
                Some(format!("{}[{}] = {};", buf_name, index_name, val_name))
            }
            OpCode::Add => self.lower_binary_op(op, "+"),
            OpCode::Sub => self.lower_binary_op(op, "-"),
            OpCode::Mul => self.lower_binary_op(op, "*"),
            OpCode::Div => self.lower_binary_op(op, "/"),
            OpCode::Mod => self.lower_binary_op(op, "%"),
            OpCode::BitAnd => self.lower_binary_op(op, "&"),
            OpCode::BitOr => self.lower_binary_op(op, "|"),
            OpCode::BitXor => self.lower_binary_op(op, "^"),
            OpCode::Shl => self.lower_binary_op(op, "<<"),
            OpCode::Shr => self.lower_binary_op(op, ">>"),
            OpCode::Eq => self.lower_binary_op(op, "=="),
            OpCode::Ne => self.lower_binary_op(op, "!="),
            OpCode::Lt => self.lower_binary_op(op, "<"),
            OpCode::Le => self.lower_binary_op(op, "<="),
            OpCode::Gt => self.lower_binary_op(op, ">"),
            OpCode::Ge => self.lower_binary_op(op, ">="),
            OpCode::And => self.lower_binary_op(op, "&&"),
            OpCode::Or => self.lower_binary_op(op, "||"),
            OpCode::Not => {
                if op.operands.is_empty() || op.results.is_empty() {
                    return None;
                }
                let result = self.value_name(&op.results[0]);
                let operand = self.value_name(&op.operands[0]);
                Some(format!("auto {} = !{};", result, operand))
            }
            OpCode::Neg => {
                if op.operands.is_empty() || op.results.is_empty() {
                    return None;
                }
                let result = self.value_name(&op.results[0]);
                let operand = self.value_name(&op.operands[0]);
                Some(format!("auto {} = -{};", result, operand))
            }
            OpCode::ConstInt => {
                if op.results.is_empty() {
                    return None;
                }
                let result = self.value_name(&op.results[0]);
                let val = match op.attrs.get("value") {
                    Some(AttrValue::Int(v)) => *v,
                    _ => 0,
                };
                Some(format!("long long {} = {};", result, val))
            }
            OpCode::ConstFloat => {
                if op.results.is_empty() {
                    return None;
                }
                let result = self.value_name(&op.results[0]);
                let val = match op.attrs.get("f_value").or_else(|| op.attrs.get("value")) {
                    Some(AttrValue::Float(v)) => *v,
                    _ => 0.0,
                };
                Some(format!("double {} = {};", result, val))
            }
            OpCode::ConstBool => {
                if op.results.is_empty() {
                    return None;
                }
                let result = self.value_name(&op.results[0]);
                let val = match op.attrs.get("value") {
                    Some(AttrValue::Bool(v)) => *v,
                    _ => false,
                };
                Some(format!("bool {} = {};", result, val))
            }
            _ => Some(format!("/* unsupported: {:?} */", op.opcode)),
        }
    }

    /// Lower a binary arithmetic/comparison op.
    fn lower_binary_op(&mut self, op: &TirOp, c_op: &str) -> Option<String> {
        if op.operands.len() < 2 || op.results.is_empty() {
            return None;
        }
        let result = self.value_name(&op.results[0]);
        let lhs = self.value_name(&op.operands[0]);
        let rhs = self.value_name(&op.operands[1]);
        Some(format!("auto {} = {} {} {};", result, lhs, c_op, rhs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::gpu::{GpuBuffer, GpuBufferAccess, GpuKernel, GpuLaunchConfig};
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;

    fn make_vector_add_kernel() -> GpuKernel {
        let ops = vec![
            TirOp {
                dialect: Dialect::Gpu,
                opcode: OpCode::Index,
                operands: vec![ValueId(0), ValueId(3)],
                results: vec![ValueId(4)],
                attrs: {
                    let mut m = AttrDict::new();
                    m.insert("buffer".into(), AttrValue::Str("a".into()));
                    m
                },
                source_span: None,
            },
            TirOp {
                dialect: Dialect::Gpu,
                opcode: OpCode::Index,
                operands: vec![ValueId(1), ValueId(3)],
                results: vec![ValueId(5)],
                attrs: {
                    let mut m = AttrDict::new();
                    m.insert("buffer".into(), AttrValue::Str("b".into()));
                    m
                },
                source_span: None,
            },
            TirOp {
                dialect: Dialect::Gpu,
                opcode: OpCode::Add,
                operands: vec![ValueId(4), ValueId(5)],
                results: vec![ValueId(6)],
                attrs: AttrDict::new(),
                source_span: None,
            },
            TirOp {
                dialect: Dialect::Gpu,
                opcode: OpCode::StoreIndex,
                operands: vec![ValueId(2), ValueId(3), ValueId(6)],
                results: vec![],
                attrs: {
                    let mut m = AttrDict::new();
                    m.insert("buffer".into(), AttrValue::Str("out".into()));
                    m
                },
                source_span: None,
            },
        ];

        GpuKernel {
            name: "vector_add".into(),
            buffers: vec![
                GpuBuffer {
                    name: "a".into(),
                    element_type: TirType::F64,
                    access: GpuBufferAccess::ReadOnly,
                },
                GpuBuffer {
                    name: "b".into(),
                    element_type: TirType::F64,
                    access: GpuBufferAccess::ReadOnly,
                },
                GpuBuffer {
                    name: "out".into(),
                    element_type: TirType::F64,
                    access: GpuBufferAccess::WriteOnly,
                },
            ],
            scalar_params: vec![("n".into(), TirType::I64)],
            body_ops: ops,
            launch_config: Some(GpuLaunchConfig {
                grid_size: [256, 1, 1],
                threadgroup_size: [256, 1, 1],
            }),
        }
    }

    /// Test 1: vector_add contains __global__, buffer params, hipThreadIdx_x
    #[test]
    fn vector_add_contains_global_and_hip_thread_builtins() {
        let hip = generate_hip(&make_vector_add_kernel());

        assert!(hip.contains("__global__"), "missing __global__ in:\n{hip}");
        assert!(
            hip.contains("hipThreadIdx_x"),
            "missing hipThreadIdx_x in:\n{hip}"
        );
        assert!(
            hip.contains("hipBlockIdx_x"),
            "missing hipBlockIdx_x in:\n{hip}"
        );
        assert!(
            hip.contains("hipBlockDim_x"),
            "missing hipBlockDim_x in:\n{hip}"
        );
        // Buffer params
        assert!(
            hip.contains("* __restrict__ a"),
            "missing buffer a in:\n{hip}"
        );
        assert!(
            hip.contains("* __restrict__ b"),
            "missing buffer b in:\n{hip}"
        );
        assert!(
            hip.contains("* __restrict__ out"),
            "missing buffer out in:\n{hip}"
        );
    }

    /// Test 2: type mapping — I64→long long, F64→double, Bool→bool
    #[test]
    fn type_mapping_correctness() {
        assert_eq!(tir_type_to_hip(&TirType::I64), "long long");
        assert_eq!(tir_type_to_hip(&TirType::F64), "double");
        assert_eq!(tir_type_to_hip(&TirType::Bool), "bool");
        // Fallback
        assert_eq!(tir_type_to_hip(&TirType::Str), "uint64_t");
    }

    /// Test 3: ReadOnly buffers get `const` qualifier
    #[test]
    fn readonly_buffer_gets_const_qualifier() {
        let hip = generate_hip(&make_vector_add_kernel());

        // ReadOnly buffers a and b should have `const double*`
        assert!(
            hip.contains("const double* __restrict__ a"),
            "expected const qualifier on a in:\n{hip}"
        );
        assert!(
            hip.contains("const double* __restrict__ b"),
            "expected const qualifier on b in:\n{hip}"
        );
        // WriteOnly buffer out should NOT have const
        assert!(
            !hip.contains("const double* __restrict__ out"),
            "unexpected const on out in:\n{hip}"
        );
        assert!(
            hip.contains("double* __restrict__ out"),
            "expected non-const out in:\n{hip}"
        );
    }

    /// Test 4: scalar params appear as kernel arguments
    #[test]
    fn scalar_params_as_kernel_args() {
        let hip = generate_hip(&make_vector_add_kernel());

        // Scalar `n` of type I64 → `const long long n`
        assert!(
            hip.contains("const long long n"),
            "expected scalar param 'n' as 'const long long n' in:\n{hip}"
        );
    }
}
