//! Handler-owned routing for LLVM preserved SimpleIR ops.
//!
//! `lower_preserved_simpleir_op` used to hand-mirror the same kind families it
//! delegated to child handlers. This module builds the dispatcher from those
//! handlers' local authorities so adding or removing a kind happens beside the
//! lowering arm that owns it.

use std::collections::HashMap;
use std::sync::OnceLock;

use super::{callable_ops, container_ops, direct_ops};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub(super) enum LlvmPreservedOpFamily {
    Direct,
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
    }

    #[test]
    fn representative_kinds_route_to_their_owning_family() {
        assert_eq!(
            llvm_preserved_op_family("floordiv"),
            Some(LlvmPreservedOpFamily::Direct),
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

    #[test]
    fn vector_reductions_use_the_shared_owned_boxed_abi() {
        use molt_ir::runtime_boxed_abi_generated::{RuntimeBoxedReturn, runtime_boxed_abi};

        for kind in ["vec_sum", "vec_prod", "vec_min", "vec_max"] {
            assert_eq!(llvm_preserved_op_family(kind), None);
            let symbol = format!("molt_{kind}");
            let abi = runtime_boxed_abi(&symbol, 3)
                .unwrap_or_else(|| panic!("{kind} must have a three-object boxed ABI"));
            assert_eq!(abi.result, RuntimeBoxedReturn::OwnedValue);
            for arity in [0, 2, 4] {
                assert!(
                    runtime_boxed_abi(&symbol, arity).is_none(),
                    "{kind} must reject arity {arity}",
                );
            }
        }
    }

    #[test]
    fn exact_boxed_runtime_kinds_bypass_direct_and_callable_family_routing() {
        for (kind, arity) in [
            ("aiter", 1),
            ("context_exit", 2),
            ("module_new", 1),
            ("module_cache_get", 1),
            ("module_cache_set", 2),
            ("module_cache_del", 1),
            ("module_get_attr", 2),
            ("module_import_from", 2),
            ("module_get_global", 2),
            ("module_set_attr", 3),
            ("module_del_global", 2),
            ("module_del_global_if_present", 2),
            ("exception_class", 1),
            ("exception_new", 2),
            ("exception_stack_set_depth", 1),
            ("exception_stack_exit", 1),
            ("exception_enter_handler", 1),
            ("exception_resolve_captured", 1),
            ("exception_set_last", 1),
            ("exception_context_set", 1),
            ("exception_set_cause", 2),
            ("class_apply_set_name", 1),
            ("class_merge_layout", 3),
            ("str_from_obj", 1),
            ("repr_from_obj", 1),
            ("int_from_obj", 3),
            ("float_from_obj", 1),
            ("ascii_from_obj", 1),
            ("complex_from_obj", 3),
            ("int_from_str_of_obj", 3),
            ("ord", 1),
            ("ord_at", 2),
            ("string_join", 2),
            ("isinstance", 2),
            ("issubclass", 2),
            ("has_attr_name", 2),
            ("is_callable", 1),
            ("context_unwind_to", 2),
            ("code_new", 9),
            ("callargs_push_pos", 2),
            ("callargs_push_kw", 3),
            ("callargs_expand_star", 2),
            ("callargs_expand_kwstar", 2),
        ] {
            assert_eq!(
                llvm_preserved_op_family(kind),
                None,
                "exact runtime kind `{kind}` must reach the admitted boxed-call route",
            );
            assert!(
                molt_ir::runtime_boxed_abi_generated::runtime_boxed_abi(
                    &format!("molt_{kind}"),
                    arity,
                )
                .is_some(),
                "retired handler `{kind}` must have an exact generated boxed ABI",
            );
        }
    }
}
