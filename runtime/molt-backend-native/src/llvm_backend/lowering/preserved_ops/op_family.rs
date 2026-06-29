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
}
