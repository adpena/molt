use crate::{FunctionIR, OpIR};

#[derive(Clone, Copy, Hash, Eq, PartialEq, Ord, PartialOrd, Debug)]
pub enum TrampolineTaskKind {
    Generator,
    Coroutine,
    AsyncGen,
}

impl TrampolineTaskKind {
    pub const fn from_marker_attr(attr: &str) -> Option<Self> {
        match attr.as_bytes() {
            b"__molt_is_generator__" => Some(Self::Generator),
            b"__molt_is_coroutine__" => Some(Self::Coroutine),
            b"__molt_is_async_generator__" => Some(Self::AsyncGen),
            _ => None,
        }
    }

    pub const fn trampoline_kind(self) -> TrampolineKind {
        match self {
            Self::Generator => TrampolineKind::Generator,
            Self::Coroutine => TrampolineKind::Coroutine,
            Self::AsyncGen => TrampolineKind::AsyncGen,
        }
    }
}

#[derive(Clone, Copy, Hash, Eq, PartialEq, Ord, PartialOrd, Debug)]
pub enum TrampolineBehavior {
    UnpackArgs,
    ForwardCallFrame,
    Task(TrampolineTaskKind),
}

macro_rules! trampoline_kinds {
    ($( $kind:ident => ($suffix:literal, $behavior:expr) ),+ $(,)?) => {
        #[derive(
            Clone,
            Copy,
            Hash,
            Eq,
            PartialEq,
            Ord,
            PartialOrd,
            Debug,
            serde::Deserialize,
            serde::Serialize,
        )]
        pub enum TrampolineKind {
            $( $kind, )+
        }

        impl TrampolineKind {
            pub const ALL: [Self; trampoline_kinds!(@count $( $kind )+)] = [
                $( Self::$kind, )+
            ];

            pub const fn symbol_suffix(self) -> &'static str {
                match self {
                    $( Self::$kind => $suffix, )+
                }
            }

            pub const fn behavior(self) -> TrampolineBehavior {
                match self {
                    $( Self::$kind => $behavior, )+
                }
            }
        }
    };
    (@count $( $kind:ident )+) => {
        <[()]>::len(&[$(trampoline_kinds!(@unit $kind)),+])
    };
    (@unit $kind:ident) => { () };
}

trampoline_kinds! {
    Plain => ("", TrampolineBehavior::UnpackArgs),
    CallFrame => ("_call_frame", TrampolineBehavior::ForwardCallFrame),
    Generator => ("_gen", TrampolineBehavior::Task(TrampolineTaskKind::Generator)),
    Coroutine => ("_coro", TrampolineBehavior::Task(TrampolineTaskKind::Coroutine)),
    AsyncGen => ("_asyncgen", TrampolineBehavior::Task(TrampolineTaskKind::AsyncGen)),
}

#[derive(Clone, Copy)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trampoline_kind_authority_is_complete_and_collision_free() {
        assert_eq!(TrampolineKind::ALL.len(), 5);
        let mut suffixes = std::collections::BTreeSet::new();
        for kind in TrampolineKind::ALL {
            assert!(
                suffixes.insert(kind.symbol_suffix()),
                "duplicate trampoline suffix for {kind:?}"
            );
        }
        assert_eq!(
            TrampolineKind::CallFrame.behavior(),
            TrampolineBehavior::ForwardCallFrame
        );
        assert_eq!(
            TrampolineTaskKind::from_marker_attr("__molt_is_async_generator__"),
            Some(TrampolineTaskKind::AsyncGen)
        );
    }
}
