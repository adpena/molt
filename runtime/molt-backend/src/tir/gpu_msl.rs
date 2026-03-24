//! TIR → Metal Shading Language (MSL) code generation.
//!
//! Converts a [`GpuKernel`] into compilable MSL source code targeting
//! Apple Metal GPU compute pipelines.

use super::gpu::{GpuBufferAccess, GpuKernel};
use super::ops::{AttrValue, OpCode, TirOp};
use super::types::TirType;
use super::values::ValueId;

/// Generate MSL source code for a GPU kernel.
///
/// The output is a complete MSL compute kernel function with:
/// - `#include <metal_stdlib>` preamble
/// - Buffer parameters with correct address space qualifiers
/// - Scalar parameters as `constant` references
/// - Thread position in grid via `[[thread_position_in_grid]]`
/// - Body statements lowered from TIR ops
pub fn generate_msl(kernel: &GpuKernel) -> String {
    let mut msl = String::with_capacity(1024);
    msl.push_str("#include <metal_stdlib>\nusing namespace metal;\n\n");

    // Function signature
    msl.push_str(&format!("kernel void {}(\n", kernel.name));

    let total_params = kernel.buffers.len() + kernel.scalar_params.len() + 1; // +1 for tid
    let mut param_idx = 0;

    // Buffer parameters
    for (i, buf) in kernel.buffers.iter().enumerate() {
        let access = match buf.access {
            GpuBufferAccess::ReadOnly => "device const",
            GpuBufferAccess::WriteOnly | GpuBufferAccess::ReadWrite => "device",
        };
        let type_str = tir_type_to_msl(&buf.element_type);
        param_idx += 1;
        let comma = if param_idx < total_params { "," } else { "" };
        msl.push_str(&format!(
            "    {} {}* {} [[buffer({})]]{}",
            access, type_str, buf.name, i, comma
        ));
        msl.push('\n');
    }

    // Scalar parameters
    for (i, (name, ty)) in kernel.scalar_params.iter().enumerate() {
        let type_str = tir_type_to_msl(ty);
        let buf_idx = kernel.buffers.len() + i;
        param_idx += 1;
        let comma = if param_idx < total_params { "," } else { "" };
        msl.push_str(&format!(
            "    constant {}& {} [[buffer({})]]{}",
            type_str, name, buf_idx, comma
        ));
        msl.push('\n');
    }

    // Thread ID — always last parameter
    msl.push_str("    uint tid [[thread_position_in_grid]]\n");
    msl.push_str(") {\n");

    // Body: convert TIR ops to MSL statements
    let mut ctx = MslGenContext::new(kernel);
    for op in &kernel.body_ops {
        if let Some(stmt) = ctx.lower_op(op) {
            msl.push_str("    ");
            msl.push_str(&stmt);
            msl.push('\n');
        }
    }

    msl.push_str("}\n");
    msl
}

/// Map a TIR type to its MSL equivalent.
fn tir_type_to_msl(ty: &TirType) -> &'static str {
    match ty {
        TirType::I64 => "int64_t",
        TirType::F64 => "double",
        TirType::Bool => "bool",
        _ => "uint64_t", // fallback for unsupported types
    }
}

/// Context for MSL code generation — tracks SSA value → MSL variable mappings.
struct MslGenContext<'a> {
    kernel: &'a GpuKernel,
}

impl<'a> MslGenContext<'a> {
    fn new(kernel: &'a GpuKernel) -> Self {
        Self { kernel }
    }

    /// Get the MSL variable name for a ValueId.
    ///
    /// Buffer operands are mapped to their buffer name; the thread ID
    /// operand maps to `tid`; everything else is a temporary `_vN`.
    fn value_name(&self, id: &ValueId) -> String {
        format!("_v{}", id.0)
    }

    /// Resolve a buffer name from the op's attrs or from operand position.
    fn buffer_name_from_op(&self, op: &TirOp) -> Option<String> {
        if let Some(AttrValue::Str(name)) = op.attrs.get("buffer") {
            return Some(name.clone());
        }
        // Fallback: first operand might be a buffer index
        None
    }

    /// Lower a single TIR op to an MSL statement. Returns None for no-op ops.
    fn lower_op(&mut self, op: &TirOp) -> Option<String> {
        match op.opcode {
            OpCode::Index => {
                // array_read: result = buffer[index]
                let result = &op.results[0];
                let buf_name = self.buffer_name_from_op(op)?;
                let index_name = if op.operands.len() > 1 {
                    self.value_name(&op.operands[1])
                } else {
                    "tid".into()
                };
                // Determine type from the buffer
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
                // array_write: buffer[index] = value
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
                Some(format!("int64_t {} = {};", result, val))
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
            _ => {
                // Unsupported op — emit a comment for debugging
                Some(format!("/* unsupported: {:?} */", op.opcode))
            }
        }
    }

    /// Lower a binary arithmetic/comparison op.
    fn lower_binary_op(&mut self, op: &TirOp, msl_op: &str) -> Option<String> {
        if op.operands.len() < 2 || op.results.is_empty() {
            return None;
        }
        let result = self.value_name(&op.results[0]);
        let lhs = self.value_name(&op.operands[0]);
        let rhs = self.value_name(&op.operands[1]);
        Some(format!("auto {} = {} {} {};", result, lhs, msl_op, rhs))
    }

    /// Look up the MSL type string for a buffer's element type.
    fn buffer_element_type(&self, name: &str) -> Option<&'static str> {
        self.kernel
            .buffers
            .iter()
            .find(|b| b.name == name)
            .map(|b| tir_type_to_msl(&b.element_type))
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

    #[test]
    fn vector_add_kernel_generates_valid_msl() {
        let kernel = make_vector_add_kernel();
        let msl = generate_msl(&kernel);

        // Must contain Metal preamble
        assert!(msl.contains("#include <metal_stdlib>"));
        assert!(msl.contains("using namespace metal;"));

        // Must have kernel function signature
        assert!(msl.contains("kernel void vector_add("));

        // Must have thread ID parameter
        assert!(msl.contains("uint tid [[thread_position_in_grid]]"));

        // Must have buffer parameters
        assert!(msl.contains("device const double* a [[buffer(0)]]"));
        assert!(msl.contains("device const double* b [[buffer(1)]]"));
        assert!(msl.contains("device double* out [[buffer(2)]]"));
    }

    #[test]
    fn scalar_params_generate_constant_refs() {
        let kernel = make_vector_add_kernel();
        let msl = generate_msl(&kernel);

        // Scalar param n at buffer index 3 (after 3 buffers)
        assert!(msl.contains("constant int64_t& n [[buffer(3)]]"));
    }

    #[test]
    fn buffer_access_types_correct() {
        let kernel = GpuKernel {
            name: "rw_test".into(),
            buffers: vec![
                GpuBuffer {
                    name: "input".into(),
                    element_type: TirType::I64,
                    access: GpuBufferAccess::ReadOnly,
                },
                GpuBuffer {
                    name: "output".into(),
                    element_type: TirType::I64,
                    access: GpuBufferAccess::ReadWrite,
                },
            ],
            scalar_params: vec![],
            body_ops: vec![],
            launch_config: None,
        };
        let msl = generate_msl(&kernel);

        assert!(msl.contains("device const int64_t* input"));
        assert!(msl.contains("device int64_t* output"));
        // ReadWrite should NOT have "const"
        assert!(!msl.contains("device const int64_t* output"));
    }

    #[test]
    fn type_mapping_i64_f64_bool() {
        assert_eq!(tir_type_to_msl(&TirType::I64), "int64_t");
        assert_eq!(tir_type_to_msl(&TirType::F64), "double");
        assert_eq!(tir_type_to_msl(&TirType::Bool), "bool");
        // Fallback
        assert_eq!(tir_type_to_msl(&TirType::Str), "uint64_t");
    }

    #[test]
    fn body_contains_array_access_and_arithmetic() {
        let kernel = make_vector_add_kernel();
        let msl = generate_msl(&kernel);

        // Should contain array reads, add, and store
        assert!(msl.contains("a["));
        assert!(msl.contains("b["));
        assert!(msl.contains("out["));
        assert!(msl.contains("+"));
    }

    #[test]
    fn empty_kernel_generates_valid_msl() {
        let kernel = GpuKernel {
            name: "noop".into(),
            buffers: vec![],
            scalar_params: vec![],
            body_ops: vec![],
            launch_config: None,
        };
        let msl = generate_msl(&kernel);

        assert!(msl.contains("kernel void noop("));
        assert!(msl.contains("uint tid [[thread_position_in_grid]]"));
        assert!(msl.contains('}'));
    }
}
