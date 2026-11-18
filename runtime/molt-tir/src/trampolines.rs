mod callable_metadata;
pub use callable_metadata::{CallableMetadata, merge_callable_facts};

use crate::FunctionIR;

#[derive(Clone, Copy, Hash, Eq, PartialEq, Ord, PartialOrd, Debug)]
pub enum TrampolineTaskKind {
    Generator,
    Coroutine,
    AsyncGen,
}

impl TrampolineTaskKind {
    pub const fn from_constructor_kind(kind: &str) -> Option<Self> {
        match kind.as_bytes() {
            b"generator" => Some(Self::Generator),
            b"coroutine" => Some(Self::Coroutine),
            b"async_generator" => Some(Self::AsyncGen),
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

/// A runtime callable's manifest transport selects its trampoline kind once;
/// Cranelift, LLVM and WASM consume this one mapping.
impl From<molt_ir::runtime_callable_abi_generated::RuntimeCallableTrampolineAbi>
    for TrampolineKind
{
    fn from(abi: molt_ir::runtime_callable_abi_generated::RuntimeCallableTrampolineAbi) -> Self {
        use molt_ir::runtime_callable_abi_generated::RuntimeCallableTrampolineAbi;
        match abi {
            RuntimeCallableTrampolineAbi::UnpackArgs => Self::Plain,
            RuntimeCallableTrampolineAbi::CallFrame => Self::CallFrame,
        }
    }
}

#[derive(Clone, Copy)]
pub struct TrampolineSpec {
    pub arity: usize,
    pub has_closure: bool,
    pub kind: TrampolineKind,
    pub closure_size: i64,
    /// The target's authored function return ABI. Trampolines project this
    /// metadata into the import signature independently of surviving exits.
    pub target_has_ret: bool,
}

pub fn externalize_function_with_signature(func: &mut FunctionIR) {
    func.externalize_with_signature()
        .unwrap_or_else(|error| panic!("{error}"));
}

pub fn function_requires_value_return(func: &FunctionIR) -> bool {
    func.returns_value()
        .unwrap_or_else(|error| panic!("{error}"))
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
            TrampolineTaskKind::from_constructor_kind("async_generator"),
            Some(TrampolineTaskKind::AsyncGen)
        );
    }
}
