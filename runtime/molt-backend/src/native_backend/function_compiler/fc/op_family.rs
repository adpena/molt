//! `op_family` — the single source of truth for native-backend op-kind routing.
//!
//! ## Why this module exists
//!
//! `compile_func_inner`'s per-op `match op.kind.as_str()` dispatch routes each
//! IR op-kind to one of the extracted `fc::*` family handlers (`handle_arith_op`,
//! `handle_sequence_op`, …). Historically the dispatch arm hand-listed the kinds
//! it routed to a handler, and the handler *independently* matched the same set
//! internally — two hand-synced copies of one kind list.
//!
//! That duplication is exactly what regressed in commit `8b5773878` ("Extract
//! arithmetic codegen handler"): the dispatch arm listed only the scalar arith
//! kinds, dropping the 24 `vec_*` reduction kinds that `handle_arith_op`
//! delegates to `fc::vec_reductions`. The dropped kinds fell through the silent
//! `_ => {}` catch-all — no codegen emitted, the result SSA value left undefined
//! (resolved to the None sentinel), and every in-function accumulator loop
//! (`for i in range(n): total += i`) silently miscompiled until the dispatch arm
//! was restored in `0323ad28c`.
//!
//! ## The fix: derive routing, never mirror it
//!
//! Each `fc::*` handler now declares its handled kinds ONCE, in a
//! `pub(in …) const HANDLED_KINDS: &[&str]` co-located with its `match`. This
//! module aggregates those authorities into [`FAMILY_DISPATCH_TABLE`] and builds
//! a kind → [`NativeOpFamily`] map ([`native_op_family`]). The dispatch *consults*
//! that map instead of carrying its own copy of every kind list, so the dispatch
//! can no longer disagree with a handler about which kinds it owns — the
//! `8b5773878` drift class is now unexpressible.
//!
//! Residual drift between a handler's `HANDLED_KINDS` and its internal `match`
//! arms is caught loudly, never silently: a kind in `HANDLED_KINDS` but missing
//! a `match` arm hits the handler's own `_ => unreachable!`, and a kind a handler
//! `match`es but omits from `HANDLED_KINDS` reaches the dispatch's now-loud
//! catch-all (see [`NATIVE_NO_CODEGEN_RESULT_KINDS`]).
//!
//! The `vec_*` family — the kinds that actually drifted — is the cleanest
//! illustration: those 24 strings live in exactly ONE place,
//! [`super::vec_reductions::HANDLED_KINDS`], and the table maps that slice to
//! [`NativeOpFamily::Arith`] (whose handler delegates them to
//! `fc::vec_reductions`). There is no second copy anywhere to fall out of sync.

#[cfg(feature = "native-backend")]
use std::collections::HashMap;
#[cfg(feature = "native-backend")]
use std::sync::OnceLock;

/// One extracted `fc::*` family handler the per-op dispatch can route to.
///
/// Each variant corresponds to exactly one `handle_*` entry point in the
/// `compile_func_inner` dispatch. `Arith` is backed by two kind-slices (the
/// scalar arith kinds and the delegated `vec_*` reduction kinds) because
/// `handle_arith_op` forwards the reduction family to `fc::vec_reductions`.
#[cfg(feature = "native-backend")]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub(in crate::native_backend::function_compiler) enum NativeOpFamily {
    Arith,
    Sequence,
    Generators,
    ScalarBuiltins,
    Callargs,
    ListOps,
    DictOps,
    SetOps,
    Indexing,
    TextPredicates,
    TextTransform,
    RuntimeOps,
    Statistics,
    TypeConversions,
    MemoryviewBuffer,
    Dataclass,
    Compare,
    UnaryLogic,
    ParseOps,
    Coroutine,
    FuturePromise,
    Funcobj,
    ObjectConstruct,
    GpuIntrinsic,
    Calls,
    ValueTransfer,
    Modules,
    ClassOps,
    TypeChecks,
    Exceptions,
    ContextMgmt,
    ExceptionStack,
    ExceptionControl,
    FileIo,
    ControlFlow,
    Loops,
    Memory,
    Attrs,
    RetJump,
}

/// The routing authority: `(family, kinds-owned-by-that-family)` rows.
///
/// Every `&[&str]` here is a handler's own `HANDLED_KINDS` const — this table
/// references those authorities, it does not restate any kind string. A family
/// may appear in more than one row when its handler is backed by several
/// kind-slices (e.g. `Arith` owns both the scalar arith kinds and the delegated
/// `vec_*` reduction kinds).
///
/// INVARIANT (asserted at first use of [`native_op_family`] and by
/// `family_dispatch_table_is_disjoint`): every kind string appears in at most
/// one row across the whole table — no two families may claim the same kind.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const FAMILY_DISPATCH_TABLE: &[(
    NativeOpFamily,
    &[&str],
)] = &[
    (NativeOpFamily::Arith, super::arith::HANDLED_KINDS),
    // `vec_*` reductions: handled by `handle_arith_op` via delegation to
    // `fc::vec_reductions`. Their authority lives ONLY in
    // `vec_reductions::HANDLED_KINDS`; routing them to `Arith` here is what lets
    // that single copy drive both codegen and dispatch.
    (NativeOpFamily::Arith, super::vec_reductions::HANDLED_KINDS),
    (NativeOpFamily::Sequence, super::sequence_ops::HANDLED_KINDS),
    (NativeOpFamily::Generators, super::generators::HANDLED_KINDS),
    (
        NativeOpFamily::ScalarBuiltins,
        super::scalar_builtins::HANDLED_KINDS,
    ),
    (NativeOpFamily::Callargs, super::callargs::HANDLED_KINDS),
    (NativeOpFamily::ListOps, super::list_ops::HANDLED_KINDS),
    (NativeOpFamily::DictOps, super::dict_ops::HANDLED_KINDS),
    (NativeOpFamily::SetOps, super::set_ops::HANDLED_KINDS),
    (NativeOpFamily::Indexing, super::indexing::HANDLED_KINDS),
    (
        NativeOpFamily::TextPredicates,
        super::text_predicates::HANDLED_KINDS,
    ),
    (
        NativeOpFamily::TextTransform,
        super::text_transform::HANDLED_KINDS,
    ),
    (
        NativeOpFamily::RuntimeOps,
        super::runtime_ops::HANDLED_KINDS,
    ),
    (NativeOpFamily::Statistics, super::statistics::HANDLED_KINDS),
    (
        NativeOpFamily::TypeConversions,
        super::type_conversions::HANDLED_KINDS,
    ),
    (
        NativeOpFamily::MemoryviewBuffer,
        super::memoryview_buffer::HANDLED_KINDS,
    ),
    (NativeOpFamily::Dataclass, super::dataclass::HANDLED_KINDS),
    (NativeOpFamily::Compare, super::compare::HANDLED_KINDS),
    (
        NativeOpFamily::UnaryLogic,
        super::unary_logic::HANDLED_KINDS,
    ),
    (NativeOpFamily::ParseOps, super::parse_ops::HANDLED_KINDS),
    (NativeOpFamily::Coroutine, super::coroutine::HANDLED_KINDS),
    (
        NativeOpFamily::FuturePromise,
        super::future_promise::HANDLED_KINDS,
    ),
    (NativeOpFamily::Funcobj, super::funcobj::HANDLED_KINDS),
    (
        NativeOpFamily::ObjectConstruct,
        super::object_construct::HANDLED_KINDS,
    ),
    (
        NativeOpFamily::GpuIntrinsic,
        super::funcobj::GPU_INTRINSIC_HANDLED_KINDS,
    ),
    (NativeOpFamily::Calls, super::calls::HANDLED_KINDS),
    (
        NativeOpFamily::ValueTransfer,
        super::value_transfer::HANDLED_KINDS,
    ),
    (NativeOpFamily::Modules, super::modules::HANDLED_KINDS),
    (NativeOpFamily::ClassOps, super::class_ops::HANDLED_KINDS),
    (
        NativeOpFamily::TypeChecks,
        super::type_checks::HANDLED_KINDS,
    ),
    (NativeOpFamily::Exceptions, super::exceptions::HANDLED_KINDS),
    (
        NativeOpFamily::ContextMgmt,
        super::context_mgmt::HANDLED_KINDS,
    ),
    (
        NativeOpFamily::ExceptionStack,
        super::exception_stack::HANDLED_KINDS,
    ),
    (
        NativeOpFamily::ExceptionControl,
        super::exception_control::HANDLED_KINDS,
    ),
    (NativeOpFamily::FileIo, super::file_io::HANDLED_KINDS),
    (
        NativeOpFamily::ControlFlow,
        super::control_flow::HANDLED_KINDS,
    ),
    (NativeOpFamily::Loops, super::loops::HANDLED_KINDS),
    (NativeOpFamily::Memory, super::memory::HANDLED_KINDS),
    (NativeOpFamily::Attrs, super::attrs::HANDLED_KINDS),
    (NativeOpFamily::RetJump, super::ret_jump::HANDLED_KINDS),
];

/// Kinds the dispatch handles with INLINE arms (the literal `"const" => {…}`
/// arms that precede the family-routed arms), not via an extracted family.
///
/// These are listed here only so the enforcement tests can prove the inline set
/// is disjoint from every family's kinds (the dispatch matches the literal arms
/// first, so any overlap would silently shadow a family). Keep in sync with the
/// literal arms at the top of the `compile_func_inner` op dispatch.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const INLINE_DISPATCH_KINDS: &[&str] = &[
    "const",
    "const_bigint",
    "const_bool",
    "const_none",
    "const_not_implemented",
    "const_ellipsis",
    "const_float",
    "const_str",
    "const_bytes",
];

/// Result-producing op kinds that legitimately reach the dispatch's catch-all
/// with NO native codegen (their `out` value is materialized elsewhere).
///
/// The catch-all panics for any result-producing (`op.out.is_some()`) kind NOT
/// on this allowlist, because leaving such a kind unhandled is the silent
/// miscompile class from `8b5773878` (undefined result SSA value → None
/// sentinel). This list is intentionally empty: every result-producing kind is
/// currently owned by an inline arm or a family. Add an entry here ONLY with a
/// documented reason why the kind needs no native codegen, never to silence a
/// genuinely-missing handler.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const NATIVE_NO_CODEGEN_RESULT_KINDS: &[&str] =
    &[];

#[cfg(feature = "native-backend")]
fn family_map() -> &'static HashMap<&'static str, NativeOpFamily> {
    static MAP: OnceLock<HashMap<&'static str, NativeOpFamily>> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut map: HashMap<&'static str, NativeOpFamily> = HashMap::new();
        for (family, kinds) in FAMILY_DISPATCH_TABLE {
            for &kind in *kinds {
                if let Some(existing) = map.insert(kind, *family) {
                    // Two families claim the same kind — the dispatch would route
                    // it to whichever guard is checked first, silently shadowing
                    // the other. This is a build-time invariant violation in the
                    // FAMILY_DISPATCH_TABLE, not a user error.
                    panic!(
                        "native op-family dispatch table is not disjoint: kind `{kind}` \
                         is claimed by both {existing:?} and {family:?}",
                    );
                }
            }
        }
        for &kind in INLINE_DISPATCH_KINDS {
            if let Some(family) = map.get(kind) {
                panic!(
                    "native op-family dispatch table shadows inline kind `{kind}` \
                     with family {family:?}",
                );
            }
        }
        map
    })
}

/// Resolve an op-kind string to the extracted family that owns its codegen, or
/// `None` if no family claims it (an inline-arm kind, or a kind with no native
/// codegen). This is the dispatch's single routing decision — built once from
/// [`FAMILY_DISPATCH_TABLE`], O(1) per lookup.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn native_op_family(
    kind: &str,
) -> Option<NativeOpFamily> {
    family_map().get(kind).copied()
}

#[cfg(all(test, feature = "native-backend"))]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// No kind may be claimed by two families: the dispatch routes via the first
    /// matching guard, so an overlap would silently send a kind to the wrong
    /// handler. (Also enforced at first use via `family_map`'s panic; this test
    /// surfaces it as a clean CI failure with the colliding kind named.)
    #[test]
    fn family_dispatch_table_is_disjoint() {
        let mut seen: HashSet<&str> = HashSet::new();
        for (family, kinds) in FAMILY_DISPATCH_TABLE {
            for &kind in *kinds {
                assert!(
                    seen.insert(kind),
                    "kind `{kind}` claimed by more than one family (last seen at {family:?})",
                );
            }
        }
        // Building the map exercises the same invariant at runtime.
        let _ = family_map();
    }

    /// The inline (`const*`) dispatch arms must not overlap any family's kinds.
    /// The dispatch matches the literal inline arms before the family guards, so
    /// an overlap would shadow the family handler for that kind.
    #[test]
    fn inline_kinds_are_disjoint_from_families() {
        let family_kinds: HashSet<&str> = FAMILY_DISPATCH_TABLE
            .iter()
            .flat_map(|(_, kinds)| kinds.iter().copied())
            .collect();
        for &inline in INLINE_DISPATCH_KINDS {
            assert!(
                !family_kinds.contains(inline),
                "inline dispatch kind `{inline}` also appears in a family's HANDLED_KINDS",
            );
        }
    }

    /// The `vec_*` reduction kinds — the family that drifted in `8b5773878` —
    /// must live in exactly one authority and route to `Arith`. This pins the
    /// invariant that prevents the regression from recurring.
    #[test]
    fn vec_reduction_kinds_route_to_arith() {
        assert!(
            !super::super::vec_reductions::HANDLED_KINDS.is_empty(),
            "vec_reductions::HANDLED_KINDS must enumerate the reduction kinds",
        );
        for &kind in super::super::vec_reductions::HANDLED_KINDS {
            assert!(
                kind.starts_with("vec_"),
                "vec_reductions::HANDLED_KINDS contains non-vec kind `{kind}`",
            );
            assert_eq!(
                native_op_family(kind),
                Some(NativeOpFamily::Arith),
                "vec reduction kind `{kind}` must route to the Arith handler \
                 (which delegates to fc::vec_reductions)",
            );
        }
        // The scalar arith authority must NOT also carry the vec_* kinds — they
        // belong to the vec_reductions authority alone (no second copy).
        for &kind in super::super::arith::HANDLED_KINDS {
            assert!(
                !kind.starts_with("vec_"),
                "arith::HANDLED_KINDS should not list vec_* kind `{kind}`; the \
                 vec_reductions authority owns it",
            );
        }
    }

    /// Every family in the table must be reachable (non-empty kind set), so a
    /// stale/empty `HANDLED_KINDS` const is caught rather than silently routing
    /// nothing.
    #[test]
    fn every_family_row_is_non_empty() {
        for (family, kinds) in FAMILY_DISPATCH_TABLE {
            assert!(
                !kinds.is_empty(),
                "family {family:?} has an empty kind set in FAMILY_DISPATCH_TABLE",
            );
        }
    }
}
