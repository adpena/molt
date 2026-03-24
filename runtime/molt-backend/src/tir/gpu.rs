//! GPU dialect operations and Metal Shading Language code generation.
//!
//! Defines the `molt.gpu` dialect data structures: kernel launch configuration,
//! buffer descriptors, and extracted GPU kernels that can be lowered to MSL.

use super::ops::TirOp;
use super::types::TirType;

/// GPU kernel launch configuration (Metal threadgroup grid).
#[derive(Debug, Clone)]
pub struct GpuLaunchConfig {
    /// Number of threadgroups in each dimension.
    pub grid_size: [u32; 3],
    /// Threads per threadgroup in each dimension.
    pub threadgroup_size: [u32; 3],
}

/// GPU buffer descriptor (kernel argument bound to a Metal buffer slot).
#[derive(Debug, Clone)]
pub struct GpuBuffer {
    pub name: String,
    pub element_type: TirType,
    pub access: GpuBufferAccess,
}

/// Access mode for a GPU buffer — determines Metal address space qualifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuBufferAccess {
    ReadOnly,
    WriteOnly,
    ReadWrite,
}

/// A GPU kernel extracted from TIR, ready for MSL code generation.
#[derive(Debug)]
pub struct GpuKernel {
    pub name: String,
    pub buffers: Vec<GpuBuffer>,
    pub scalar_params: Vec<(String, TirType)>,
    pub body_ops: Vec<TirOp>,
    pub launch_config: Option<GpuLaunchConfig>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;

    fn make_vector_add_kernel() -> GpuKernel {
        // body: out[tid] = a[tid] + b[tid]
        // Represented as: Index a[tid], Index b[tid], Add, StoreIndex out[tid]
        let ops = vec![
            TirOp {
                dialect: Dialect::Gpu,
                opcode: OpCode::Index,
                operands: vec![ValueId(0), ValueId(3)], // a, tid
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
                operands: vec![ValueId(1), ValueId(3)], // b, tid
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
                operands: vec![ValueId(2), ValueId(3), ValueId(6)], // out, tid, val
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
    fn kernel_construction() {
        let k = make_vector_add_kernel();
        assert_eq!(k.name, "vector_add");
        assert_eq!(k.buffers.len(), 3);
        assert_eq!(k.scalar_params.len(), 1);
        assert_eq!(k.body_ops.len(), 4);
        assert!(k.launch_config.is_some());
    }

    #[test]
    fn buffer_access_modes() {
        let k = make_vector_add_kernel();
        assert_eq!(k.buffers[0].access, GpuBufferAccess::ReadOnly);
        assert_eq!(k.buffers[1].access, GpuBufferAccess::ReadOnly);
        assert_eq!(k.buffers[2].access, GpuBufferAccess::WriteOnly);
    }

    #[test]
    fn launch_config_dimensions() {
        let cfg = GpuLaunchConfig {
            grid_size: [64, 64, 1],
            threadgroup_size: [16, 16, 1],
        };
        assert_eq!(cfg.grid_size[0] * cfg.threadgroup_size[0], 64 * 16);
    }
}
