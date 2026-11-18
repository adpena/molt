mod callable_metadata;
pub use callable_metadata::{CallableMetadata, merge_callable_facts};

use crate::FunctionIR;

#[derive(Clone, Copy, Hash, Eq, PartialEq, Ord, PartialOrd, Debug)]
pub enum TrampolineTaskKind {
    Generator,
    Coroutine,
    AsyncGen,
}

// Task construction's layout and protocol live in molt-codegen-abi's task_site
// generator, which every backend runs; this module names the trampoline kinds.
pub use molt_codegen_abi::task_site::{TaskCompletion, TaskConstructorLayout, TaskRuntimeKind};

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

    pub const fn constructor_layout(self) -> TaskConstructorLayout {
        match self {
            Self::Generator => TaskConstructorLayout::generator(),
            Self::Coroutine => TaskConstructorLayout::coroutine(),
            Self::AsyncGen => TaskConstructorLayout::async_generator(),
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
    fn task_payload_extents_share_checked_sizing_for_every_constructor() {
        let layouts = [
            TaskConstructorLayout::for_alloc_kind(None),
            TaskConstructorLayout::for_alloc_kind(Some("generator")),
            TaskConstructorLayout::for_alloc_kind(Some("coroutine")),
            TaskConstructorLayout::for_call_async(),
            TrampolineTaskKind::Generator.constructor_layout(),
            TrampolineTaskKind::Coroutine.constructor_layout(),
            TrampolineTaskKind::AsyncGen.constructor_layout(),
        ];
        for layout in layouts {
            for arity in [0, 1, 7] {
                for has_closure in [false, true] {
                    let expected = i64::from(layout.payload_base_offset(32))
                        + (arity + usize::from(has_closure)) as i64 * 8;
                    assert_eq!(
                        layout.required_closure_size(arity, has_closure, 32),
                        expected
                    );
                    layout.validate_closure_size(expected, arity, has_closure, 32);
                    assert!(
                        std::panic::catch_unwind(|| {
                            layout.validate_closure_size(expected - 1, arity, has_closure, 32);
                        })
                        .is_err()
                    );
                }
            }
            assert!(
                std::panic::catch_unwind(|| {
                    layout.required_closure_size(usize::MAX, true, 32);
                })
                .is_err()
            );
            assert!(
                std::panic::catch_unwind(|| {
                    layout.required_closure_size(0, false, -1);
                })
                .is_err()
            );
            // A byte extent can overflow even when the slot count fits usize.
            if let Ok(slots) = usize::try_from(i64::MAX / 8 + 1) {
                assert!(
                    std::panic::catch_unwind(|| {
                        layout.required_closure_size(slots, false, 32);
                    })
                    .is_err()
                );
            }
        }
    }

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
        let asyncgen = TrampolineTaskKind::AsyncGen.constructor_layout();
        assert_eq!(asyncgen.runtime_kind(), TaskRuntimeKind::Generator);
        assert_eq!(asyncgen.payload_base_offset(32), 32);
        assert_eq!(asyncgen.completion(), TaskCompletion::WrapAsyncGen);
        assert_eq!(
            TaskConstructorLayout::for_alloc_kind(Some("coroutine")).completion(),
            TaskCompletion::RegisterCancelToken
        );
        assert_eq!(
            TaskConstructorLayout::for_call_async().completion(),
            TaskCompletion::ReturnTask
        );
    }
}
