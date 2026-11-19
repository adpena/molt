use crate::ir::{FunctionIR, OpIR};

pub(crate) fn op(
    kind: molt_ir::OpKind,
    out: Option<&str>,
    var: Option<&str>,
    args: &[&str],
) -> OpIR {
    OpIR {
        kind,
        out: out.map(str::to_string),
        var: var.map(str::to_string),
        args: (!args.is_empty()).then(|| args.iter().map(|arg| arg.to_string()).collect()),
        ..OpIR::new(kind)
    }
}

pub(crate) fn function(
    name: &str,
    params: &[&str],
    param_types: Option<Vec<&str>>,
    ops: Vec<OpIR>,
) -> FunctionIR {
    FunctionIR {
        module: None,
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: name.to_string(),
        params: params.iter().map(|param| param.to_string()).collect(),
        ops,
        param_types: param_types.map(|types| types.into_iter().map(str::to_string).collect()),
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        frame_locals_exposed: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    }
}
