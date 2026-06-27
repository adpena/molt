use std::iter::ExactSizeIterator;
use wasm_encoder::{TypeSection, ValType};

pub(crate) use crate::wasm_abi_generated::{
    POLL_TABLE_FUNCS, RUNTIME_CALLABLE_IMPORTS, RuntimeCallableResult,
};
pub(crate) use molt_codegen_abi::{
    GENERATOR_CONTROL_BYTES as GEN_CONTROL_SIZE, TASK_KIND_COROUTINE, TASK_KIND_FUTURE,
    TASK_KIND_GENERATOR,
};
pub(crate) const RELOC_TABLE_BASE_DEFAULT: u32 = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ReservedRuntimeCallableSpec {
    pub(crate) index: u32,
    pub(crate) runtime_name: &'static str,
    pub(crate) import_name: &'static str,
    pub(crate) arity: usize,
}

pub(crate) const RESERVED_RUNTIME_CALLABLE_SPECS: &[ReservedRuntimeCallableSpec] = &{
    macro_rules! entry_list {
        ($(($idx:expr, $sym:ident, $import:literal, $arity:expr))+) => {
            [
                $(
                    ReservedRuntimeCallableSpec {
                        index: $idx,
                        runtime_name: stringify!($sym),
                        import_name: $import,
                        arity: $arity,
                    },
                )+
            ]
        };
    }
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../wasm_runtime_callables.inc"
    ))
};
pub(crate) const RESERVED_RUNTIME_CALLABLE_COUNT: u32 =
    RESERVED_RUNTIME_CALLABLE_SPECS.len() as u32;

// ---------------------------------------------------------------------------
// WASM Exception Handling (WASM_OPTIMIZATION_PLAN.md Section 3.6)
//
// Native WASM exception handling replaces the host-imported exception
// mechanism (exception_push/exception_pending/exception_pop) with the
// standardized WASM exception handling instructions (try_table/throw/catch).
//
// The exception tag carries a single i64 payload: the exception object
// handle.  This matches type index 1 in the static type section:
// (i64) -> ().
//
// Current host-call exception model:
//   try block entry:  call exception_push   (push handler frame)
//   after each call:  call exception_pending (poll for raised exception)
//                     br_if to handler      (branch if pending != 0)
//   try block exit:   call exception_pop    (pop handler frame)
//   raise:            call raise            (set pending + unwind)
//
// Native WASM EH model (target):
//   try block entry:  try_table with catch clause
//   after each call:  (eliminated -- WASM catches automatically)
//   try block exit:   end (implicit)
//   raise:            throw $molt_exception <handle>
//
// Estimated impact: 20-40% speedup for exception-heavy code; 5-10%
// binary size reduction from eliminating exception_pending checks.
//
// Enabled by default; set MOLT_WASM_NATIVE_EH=0 to disable.
// ---------------------------------------------------------------------------

/// Type index for the exception tag payload: (i64) -> ()
/// This is type 1 in the static type section.
pub(crate) const TAG_EXCEPTION_FUNC_TYPE: u32 = 1;

/// Tag index for the molt exception tag (first and only tag in the module).
pub(crate) const TAG_EXCEPTION_INDEX: u32 = 0;

// ---------------------------------------------------------------------------
// Multi-value return type indices (WASM 2.0 multi-value proposal)
//
// These type indices are reserved in the static type section for functions
// that return 2-3 i64 values instead of allocating a tuple on the heap.
// This enables the optimization described in WASM_OPTIMIZATION_PLAN.md §3.1:
// eliminate 1 alloc + N field_get calls per multi-return call site.
//
// Builtins that always return a known-size tuple (e.g. divmod -> 2 values,
// dict items iteration -> 2 values) can be migrated to use these signatures
// once both the host import and call-site lowering are updated.
// ---------------------------------------------------------------------------

pub(crate) trait TypeSectionExt {
    fn function<P, R>(&mut self, params: P, results: R)
    where
        P: IntoIterator<Item = ValType>,
        P::IntoIter: ExactSizeIterator,
        R: IntoIterator<Item = ValType>,
        R::IntoIter: ExactSizeIterator;
}

impl TypeSectionExt for TypeSection {
    fn function<P, R>(&mut self, params: P, results: R)
    where
        P: IntoIterator<Item = ValType>,
        P::IntoIter: ExactSizeIterator,
        R: IntoIterator<Item = ValType>,
        R::IntoIter: ExactSizeIterator,
    {
        self.ty().function(params, results);
    }
}

#[derive(Clone, Copy)]
struct StaticFuncType {
    params: &'static [ValType],
    results: &'static [ValType],
}

const fn static_func_type(
    params: &'static [ValType],
    results: &'static [ValType],
) -> StaticFuncType {
    StaticFuncType { params, results }
}

const I64: ValType = ValType::I64;
const I32: ValType = ValType::I32;

const STATIC_FUNC_TYPES: [StaticFuncType; 51] = [
    static_func_type(&[], &[I64]),
    static_func_type(&[I64], &[]),
    static_func_type(&[I64], &[I64]),
    static_func_type(&[I64, I64], &[I64]),
    static_func_type(&[I64, I64, I64], &[I32]),
    static_func_type(&[I64, I64, I64], &[I64]),
    static_func_type(&[I64, I64], &[]),
    static_func_type(&[I64, I64, I64, I64], &[I64]),
    static_func_type(&[], &[]),
    static_func_type(&[I64, I64, I64, I64, I64, I64], &[I64]),
    static_func_type(&[I64, I64, I64, I64, I64, I64, I64], &[I64]),
    static_func_type(&[I64, I64, I64, I64], &[I32]),
    static_func_type(&[I64, I64, I64, I64, I64], &[I64]),
    static_func_type(&[I64], &[I32]),
    static_func_type(&[I32], &[I64]),
    static_func_type(&[I32], &[]),
    static_func_type(&[I32, I64], &[I64]),
    static_func_type(&[I32, I64, I64], &[I64]),
    static_func_type(&[I64, I32, I64], &[I64]),
    static_func_type(&[I32, I64, I32], &[I32]),
    static_func_type(&[I64, I32, I32], &[I32]),
    static_func_type(&[I32, I64, I64, I64, I32, I64], &[I64]),
    static_func_type(&[I32, I64, I64, I64, I64, I32, I64], &[I64]),
    static_func_type(&[I32, I32, I64], &[I64]),
    static_func_type(&[I32, I32, I64, I64], &[I64]),
    static_func_type(&[I64, I32, I64, I64], &[I64]),
    static_func_type(&[I32, I64, I32, I64], &[I32]),
    static_func_type(&[I32, I32], &[I64]),
    static_func_type(&[I64, I64, I64, I64, I64, I64, I64, I64], &[I64]),
    static_func_type(&[I64, I64, I64, I64, I64, I64], &[I64]),
    static_func_type(&[I64, I64, I64], &[]),
    static_func_type(&[I64, I64], &[I64, I64]),
    static_func_type(&[I64, I64, I64], &[I64, I64, I64]),
    static_func_type(&[I64], &[I64, I64]),
    static_func_type(&[], &[I64, I64]),
    static_func_type(&[I64, I64, I64, I64, I64, I64, I64, I64, I64], &[I64]),
    static_func_type(&[I64, I64, I64, I64, I64, I64, I64, I64, I64, I64], &[I64]),
    static_func_type(
        &[I64, I64, I64, I64, I64, I64, I64, I64, I64, I64, I64],
        &[I64],
    ),
    static_func_type(
        &[I64, I64, I64, I64, I64, I64, I64, I64, I64, I64, I64, I64],
        &[I64],
    ),
    static_func_type(&[I32, I64], &[]),
    static_func_type(&[I64, I32, I64, I32, I64, I64, I64, I64], &[I64]),
    static_func_type(&[I64, I64, I32, I64], &[I64]),
    static_func_type(&[I64, I64, I32, I64, I64], &[I64]),
    static_func_type(&[I64, I64, I32, I64, I64, I64], &[I64]),
    static_func_type(&[I64, I64, I32, I64, I64, I64, I64], &[I64]),
    static_func_type(&[I64, I64, I32, I64, I64, I64, I64, I64], &[I64]),
    static_func_type(&[I64, I64, I64, I32, I64], &[I64]),
    static_func_type(&[I64, I64, I64, I32, I64, I64], &[I64]),
    static_func_type(&[I64, I64, I64, I32, I64, I64, I64], &[I64]),
    static_func_type(&[I64, I64, I64, I32, I64, I64, I64, I64], &[I64]),
    static_func_type(&[I64, I64, I64, I32, I64, I64, I64, I64, I64], &[I64]),
];

/// First dynamic type index; must equal the count of all statically-defined types.
///
/// Static signatures occupy indices `0..STATIC_TYPE_COUNT`. Dynamic user arity
/// signatures and wrapper signatures must start after that fixed set.
pub(crate) const STATIC_TYPE_COUNT: u32 = STATIC_FUNC_TYPES.len() as u32;

pub(crate) fn emit_static_type_section(types: &mut TypeSection) {
    for static_type in STATIC_FUNC_TYPES {
        types.function(
            static_type.params.iter().copied(),
            static_type.results.iter().copied(),
        );
    }
}

// Constant folding pass is now shared via crate::fold_constants in passes.rs.

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_encoder::Module;
    use wasmparser::{CompositeInnerType, Parser, Payload};

    fn static_type_section_signatures() -> Vec<(usize, usize)> {
        let mut types = TypeSection::new();
        emit_static_type_section(&mut types);
        let mut module = Module::new();
        module.section(&types);
        let wasm = module.finish();

        let mut sigs = Vec::new();
        for payload in Parser::new(0).parse_all(&wasm) {
            if let Payload::TypeSection(reader) = payload.expect("valid payload") {
                for rec_group in reader.into_iter() {
                    let rec_group = rec_group.expect("valid rec group");
                    for sub_type in rec_group.into_types() {
                        if let CompositeInnerType::Func(func_type) = &sub_type.composite_type.inner
                        {
                            sigs.push((func_type.params().len(), func_type.results().len()));
                        }
                    }
                }
            }
        }
        sigs
    }

    #[test]
    fn static_type_section_signatures_are_pinned_to_static_type_count() {
        let sigs = static_type_section_signatures();

        assert_eq!(
            sigs.len(),
            STATIC_TYPE_COUNT as usize,
            "static type table must emit exactly STATIC_TYPE_COUNT entries"
        );

        let pinned: &[(usize, (usize, usize))] = &[
            (0, (0, 1)),   // () -> i64
            (1, (1, 0)),   // (i64) -> ()
            (8, (0, 0)),   // () -> ()
            (31, (2, 2)),  // MULTI_RETURN_2
            (32, (3, 3)),  // MULTI_RETURN_3
            (33, (1, 2)),  // MULTI_RETURN_UNARY_TO_2
            (34, (0, 2)),  // MULTI_RETURN_NULLARY_TO_2
            (35, (9, 1)),  // high arity
            (38, (12, 1)), // high arity
            (41, (4, 1)),  // call_method_ic0
            (45, (8, 1)),  // call_method_ic4
            (46, (5, 1)),  // call_super_method_ic0
            (50, (9, 1)),  // call_super_method_ic4
        ];
        for &(idx, expected) in pinned {
            assert_eq!(
                sigs[idx], expected,
                "static WASM type {idx} drifted to {:?}, expected {expected:?}",
                sigs[idx]
            );
        }
    }
}
