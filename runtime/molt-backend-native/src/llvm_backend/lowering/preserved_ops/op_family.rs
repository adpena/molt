//! Handler-owned routing for LLVM preserved SimpleIR ops.
//!
//! `lower_preserved_simpleir_op` used to hand-mirror the same kind families it
//! delegated to child handlers. This module builds the dispatcher from those
//! handlers' local authorities so adding or removing a kind happens beside the
//! lowering arm that owns it.

use std::collections::HashMap;
use std::sync::OnceLock;

use super::{callable_ops, container_ops, direct_ops, vector_reductions};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub(super) enum LlvmPreservedOpFamily {
    Direct,
    VectorReduction,
    Container,
    Callable,
}

const FAMILY_DISPATCH_TABLE: &[(LlvmPreservedOpFamily, &[&str])] = &[
    (LlvmPreservedOpFamily::Direct, direct_ops::HANDLED_KINDS),
    (
        LlvmPreservedOpFamily::Container,
        container_ops::HANDLED_KINDS,
    ),
    (LlvmPreservedOpFamily::Callable, callable_ops::HANDLED_KINDS),
];

fn family_map() -> &'static HashMap<&'static str, LlvmPreservedOpFamily> {
    static MAP: OnceLock<HashMap<&'static str, LlvmPreservedOpFamily>> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut map: HashMap<&'static str, LlvmPreservedOpFamily> = HashMap::new();
        for (family, kinds) in FAMILY_DISPATCH_TABLE {
            for &kind in *kinds {
                if let Some(existing) = map.insert(kind, *family) {
                    panic!(
                        "LLVM preserved-op family table is not disjoint: kind `{kind}` \
                         is claimed by both {existing:?} and {family:?}",
                    );
                }
            }
        }
        for &(kind, _) in vector_reductions::VEC_REDUCTION_OPS {
            if let Some(existing) = map.insert(kind, LlvmPreservedOpFamily::VectorReduction) {
                panic!(
                    "LLVM preserved-op family table is not disjoint: vector reduction \
                     kind `{kind}` is also claimed by {existing:?}",
                );
            }
        }
        map
    })
}

pub(super) fn llvm_preserved_op_family(kind: &str) -> Option<LlvmPreservedOpFamily> {
    family_map().get(kind).copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn family_dispatch_table_is_disjoint() {
        let mut seen: HashSet<&str> = HashSet::new();
        for (family, kinds) in FAMILY_DISPATCH_TABLE {
            for &kind in *kinds {
                assert!(
                    seen.insert(kind),
                    "kind `{kind}` claimed by more than one LLVM preserved-op family \
                     (last seen at {family:?})",
                );
            }
        }
        for &(kind, _) in vector_reductions::VEC_REDUCTION_OPS {
            assert!(
                seen.insert(kind),
                "vector reduction kind `{kind}` also appears in a preserved-op family",
            );
        }
        let _ = family_map();
    }

    #[test]
    fn each_family_row_is_non_empty() {
        for (family, kinds) in FAMILY_DISPATCH_TABLE {
            assert!(
                !kinds.is_empty(),
                "LLVM preserved-op family {family:?} has no routed kinds",
            );
        }
        assert!(
            !vector_reductions::VEC_REDUCTION_OPS.is_empty(),
            "LLVM vector reduction family has no routed kinds",
        );
    }

    #[test]
    fn representative_kinds_route_to_their_owning_family() {
        assert_eq!(
            llvm_preserved_op_family("floordiv"),
            Some(LlvmPreservedOpFamily::Direct),
        );
        assert_eq!(
            llvm_preserved_op_family("vec_sum_int_range_iter_trusted"),
            Some(LlvmPreservedOpFamily::VectorReduction),
        );
        assert_eq!(
            llvm_preserved_op_family("dict_new"),
            Some(LlvmPreservedOpFamily::Container),
        );
        assert_eq!(
            llvm_preserved_op_family("func_new"),
            Some(LlvmPreservedOpFamily::Callable),
        );
    }

    #[test]
    fn fixed_boxed_container_calls_bypass_dedicated_family_routing() {
        for (kind, arity) in [
            ("list_from_range", 3),
            ("list_fill_new", 2),
            ("list_append", 2),
            ("list_extend", 2),
            ("tuple_from_list", 1),
            ("set_add", 2),
            ("set_add_probe", 2),
            ("frozenset_add", 2),
            ("dict_set", 3),
            ("dict_setdefault", 3),
            ("dict_setdefault_empty_list", 2),
            ("dict_get", 3),
            ("dict_update", 2),
            ("dict_update_missing", 3),
            ("dict_update_kwstar", 2),
            ("dict_clear", 1),
            ("dict_copy", 1),
            ("dict_popitem", 1),
            ("slice", 3),
            ("slice_new", 3),
            ("dict_keys", 1),
            ("dict_values", 1),
            ("dict_items", 1),
            ("enumerate", 3),
            ("dict_from_obj", 1),
        ] {
            assert_eq!(
                llvm_preserved_op_family(kind),
                None,
                "fixed boxed call `{kind}` must reach generated runtime ABI dispatch",
            );
            assert!(
                molt_ir::runtime_boxed_abi_generated::runtime_boxed_abi(
                    &format!("molt_{kind}"),
                    arity,
                )
                .is_some(),
                "deleted handler `{kind}` must have an exact generated boxed ABI",
            );
        }

        for kind in [
            "iter_next_unboxed",
            "len",
            "list_new",
            "dict_new",
            "tuple_new",
            "set_new",
            "frozenset_new",
            "iter",
            "unpack_sequence",
        ] {
            assert_eq!(
                llvm_preserved_op_family(kind),
                Some(LlvmPreservedOpFamily::Container),
                "custom container protocol `{kind}` must keep dedicated lowering",
            );
        }
    }
}
