mod callable_metadata;
pub use callable_metadata::{CallableMetadata, merge_callable_facts};

use crate::FunctionIR;

use molt_ir::task_kind::TaskKind;

macro_rules! trampoline_kinds {
    ($( $kind:ident => $suffix:literal ),+ $(,)?) => {
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

        }
    };
    (@count $( $kind:ident )+) => {
        <[()]>::len(&[$(trampoline_kinds!(@unit $kind)),+])
    };
    (@unit $kind:ident) => { () };
}

trampoline_kinds! {
    Plain => "",
    CallFrame => "_call_frame",
    Generator => "_gen",
    Coroutine => "_coro",
    AsyncGen => "_asyncgen",
}

impl TrampolineKind {
    pub const fn for_task(kind: TaskKind) -> Option<Self> {
        match kind {
            TaskKind::Future => None,
            TaskKind::Generator => Some(Self::Generator),
            TaskKind::Coroutine => Some(Self::Coroutine),
            TaskKind::AsyncGenerator => Some(Self::AsyncGen),
        }
    }
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

pub fn externalize_function_with_signature(func: &mut FunctionIR) {
    func.externalize_with_signature()
        .unwrap_or_else(|error| panic!("{error}"));
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
            TrampolineKind::for_task(TaskKind::AsyncGenerator),
            Some(TrampolineKind::AsyncGen)
        );
    }
}
