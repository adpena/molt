use crate::{FunctionIR, OpIR};

#[derive(
    Clone, Copy, Hash, Eq, PartialEq, Ord, PartialOrd, Debug, serde::Deserialize, serde::Serialize,
)]
#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub enum TrampolineKind {
    Plain,
    Generator,
    Coroutine,
    AsyncGen,
}

#[derive(Clone, Copy)]
#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub struct TrampolineSpec {
    pub arity: usize,
    pub has_closure: bool,
    pub kind: TrampolineKind,
    pub closure_size: i64,
    /// Whether the target function returns a value. Trampolines use this
    /// to set the correct import signature: functions with ret_void only
    /// don't have a return in their signature.
    pub target_has_ret: bool,
}

const EXTERN_SIGNATURE_RETURN_VALUE: &str = "__molt_extern_signature_return";

fn function_body_requires_value_return(func: &FunctionIR) -> bool {
    func.ops.iter().any(|op| {
        matches!(
            op.kind.as_str(),
            "ret"
                | "state_switch"
                | "state_transition"
                | "state_yield"
                | "chan_send_yield"
                | "chan_recv_yield"
        )
    })
}

pub fn externalize_function_with_signature(func: &mut FunctionIR) {
    let returns_value = function_body_requires_value_return(func);
    func.is_extern = true;
    func.ops = if returns_value {
        vec![
            OpIR {
                kind: "missing".to_string(),
                out: Some(EXTERN_SIGNATURE_RETURN_VALUE.to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".to_string(),
                args: Some(vec![EXTERN_SIGNATURE_RETURN_VALUE.to_string()]),
                ..OpIR::default()
            },
        ]
    } else {
        vec![OpIR {
            kind: "ret_void".to_string(),
            ..OpIR::default()
        }]
    };
}

pub fn function_requires_value_return(func: &FunctionIR) -> bool {
    if func.is_extern {
        assert!(
            !func.ops.is_empty(),
            "extern function `{}` is missing return-signature metadata",
            func.name
        );
    }
    function_body_requires_value_return(func)
}
