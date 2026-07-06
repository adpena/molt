use crate::dtype::DType;

use super::MilRenderer;

#[derive(Debug, Clone)]
pub(in crate::render::mil) struct MilValue {
    pub(in crate::render::mil) name: String,
    pub(in crate::render::mil) shape: Vec<usize>,
    pub(in crate::render::mil) dtype: DType,
}

impl MilValue {
    pub(in crate::render::mil) fn new(name: String, shape: &[usize], dtype: DType) -> Self {
        Self {
            name,
            shape: canonical_mil_shape(shape),
            dtype,
        }
    }

    pub(in crate::render::mil) fn scalar(name: String, dtype: DType) -> Self {
        Self {
            name,
            shape: Vec::new(),
            dtype,
        }
    }

    pub(in crate::render::mil) fn is_scalar(&self) -> bool {
        self.shape.is_empty()
    }
}

impl MilRenderer {
    /// Map a DType to MIL type string.
    pub(in crate::render::mil) fn mil_type(dtype: DType) -> &'static str {
        match dtype {
            DType::Bool => "bool",
            DType::Int8 => "int8",
            DType::Int16 => "int16",
            DType::Int32 => "int32",
            DType::Int64 => "int64",
            DType::UInt8 => "uint8",
            DType::UInt16 => "uint16",
            DType::UInt32 => "uint32",
            DType::UInt64 => "uint64",
            DType::Float16 | DType::BFloat16 => "fp16",
            DType::Float32 => "fp32",
            DType::Float64 => "fp64",
            DType::MxFP8 => "fp8",
            DType::MxFP4 => "fp4",
        }
    }

    pub(in crate::render::mil) fn mil_materialize_type(dtype: DType) -> &'static str {
        match dtype {
            DType::Bool => "bool",
            DType::Int8 => "int8",
            DType::Int16 => "int16",
            DType::Int32 => "int32",
            DType::UInt8 => "uint8",
            DType::UInt16 => "uint16",
            DType::UInt32 => "uint32",
            DType::Float16 => "fp16",
            DType::Float32 => "fp32",
            DType::BFloat16 => panic!(
                "molt-gpu MIL renderer: MaterializeCopy for BFloat16 requires a distinct bf16 storage proof"
            ),
            DType::Int64 | DType::UInt64 | DType::Float64 => panic!(
                "molt-gpu MIL renderer: MaterializeCopy for 64-bit dtypes requires MIL compile and byte-roundtrip proof"
            ),
            DType::MxFP8 | DType::MxFP4 => panic!(
                "molt-gpu MIL renderer: MaterializeCopy for MXFP requires explicit block/exponent storage lowering"
            ),
        }
    }

    /// Format a constant value as a MIL literal.
    pub(in crate::render::mil) fn format_const(val: f64, dtype: DType) -> String {
        match dtype {
            DType::Bool => {
                if val != 0.0 {
                    "true".to_string()
                } else {
                    "false".to_string()
                }
            }
            DType::Int8 | DType::Int16 | DType::Int32 | DType::Int64 => {
                format!("{}", val as i64)
            }
            DType::UInt8 | DType::UInt16 | DType::UInt32 | DType::UInt64 => {
                format!("{}", val as u64)
            }
            _ => {
                if val == f64::INFINITY {
                    "inf".to_string()
                } else if val == f64::NEG_INFINITY {
                    "-inf".to_string()
                } else if val.is_nan() {
                    "nan".to_string()
                } else {
                    format!("{}", val)
                }
            }
        }
    }
}

pub(in crate::render::mil) fn format_axes(axes: &[usize]) -> String {
    let joined = axes
        .iter()
        .map(|axis| axis.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{}]", joined)
}

pub(in crate::render::mil) fn canonical_mil_shape(shape: &[usize]) -> Vec<usize> {
    if shape.is_empty() {
        vec![1]
    } else {
        shape.to_vec()
    }
}

pub(in crate::render::mil) fn format_mil_shape(shape: &[usize]) -> String {
    let joined = canonical_mil_shape(shape)
        .iter()
        .map(|dim| dim.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{}]", joined)
}
