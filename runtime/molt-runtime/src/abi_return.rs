//! Typed failure contracts for non-object FFI return domains.
//!
//! Molt object bits and raw ABI values deliberately share integer carriers,
//! but they do not share failure sentinels. Keeping the domain in the type
//! parameter prevents an address-returning export from accidentally returning
//! the NaN-boxed object exception sentinel.

use crate::PyToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AbiReturnKind {
    RawAddress,
    RawHandle,
    RawIndex,
    Status,
}

pub(crate) trait AbiReturnDomain {
    type Repr: Copy;
    const KIND: AbiReturnKind;
    const FAILURE: Self::Repr;
}

pub(crate) enum NullAddress {}

impl AbiReturnDomain for NullAddress {
    type Repr = u64;
    const KIND: AbiReturnKind = AbiReturnKind::RawAddress;
    const FAILURE: Self::Repr = 0;
}

pub(crate) enum NullHandle {}

impl AbiReturnDomain for NullHandle {
    type Repr = u64;
    const KIND: AbiReturnKind = AbiReturnKind::RawHandle;
    const FAILURE: Self::Repr = 0;
}

pub(crate) enum ZeroIndex {}

impl AbiReturnDomain for ZeroIndex {
    type Repr = i64;
    const KIND: AbiReturnKind = AbiReturnKind::RawIndex;
    const FAILURE: Self::Repr = 0;
}

pub(crate) enum FailureStatus {}

impl AbiReturnDomain for FailureStatus {
    type Repr = i32;
    const KIND: AbiReturnKind = AbiReturnKind::Status;
    const FAILURE: Self::Repr = 0;
}

const _: [AbiReturnKind; 4] = [
    NullAddress::KIND,
    NullHandle::KIND,
    ZeroIndex::KIND,
    FailureStatus::KIND,
];

#[inline]
pub(crate) fn fail<D: AbiReturnDomain>(
    py: &PyToken<'_>,
    exception: &'static str,
    message: &'static str,
) -> D::Repr {
    let _ = D::KIND;
    crate::raise_exception::<()>(py, exception, message);
    D::FAILURE
}

#[inline]
pub(crate) fn fail_memory<D: AbiReturnDomain>(py: &PyToken<'_>) -> D::Repr {
    let _ = D::KIND;
    crate::record_memory_error_without_allocation(py);
    D::FAILURE
}

#[cfg(test)]
mod tests {
    use super::{
        AbiReturnDomain, AbiReturnKind, FailureStatus, NullAddress, NullHandle, ZeroIndex,
    };
    use crate::MoltObject;
    use crate::resource::{
        LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker, with_tracker,
    };
    use std::collections::BTreeSet;

    struct TrackerReset;

    impl Drop for TrackerReset {
        fn drop(&mut self) {
            set_tracker(Box::new(UnlimitedTracker));
        }
    }

    const RAW_INTEGER_ABI_EXPORTS: &[(&str, AbiReturnKind, &str)] = &[
        (
            "molt_scratch_alloc",
            AbiReturnKind::RawAddress,
            include_str!("wasm_abi_exports.rs"),
        ),
        (
            "molt_exception_pending_flag_ptr",
            AbiReturnKind::RawAddress,
            include_str!("builtins/exceptions/exception_state_abi.rs"),
        ),
        (
            "molt_handle_resolve",
            AbiReturnKind::RawAddress,
            include_str!("provenance/handles.rs"),
        ),
        (
            "molt_module_capi_get_def",
            AbiReturnKind::RawAddress,
            include_str!("c_api/molt_api.rs"),
        ),
        (
            "molt_module_state_find",
            AbiReturnKind::RawHandle,
            include_str!("c_api/molt_api.rs"),
        ),
        (
            "PyErr_NoMemory",
            AbiReturnKind::RawHandle,
            include_str!("c_api/cpython_compat.rs"),
        ),
        (
            "molt_list_int_data",
            AbiReturnKind::RawAddress,
            include_str!("object/ops/specialized_list.rs"),
        ),
        (
            "molt_list_int_getitem_raw_checked",
            AbiReturnKind::RawIndex,
            include_str!("object/ops/specialized_list.rs"),
        ),
        (
            "molt_seq_snapshot",
            AbiReturnKind::Status,
            include_str!("seq_snapshot_bridge.rs"),
        ),
    ];

    fn function_body<'a>(source: &'a str, symbol: &str) -> &'a str {
        let start = source
            .find(&format!("fn {symbol}"))
            .unwrap_or_else(|| panic!("missing ABI export {symbol}"));
        let open = source[start..]
            .find('{')
            .map(|offset| start + offset)
            .expect("function body open brace");
        let mut depth = 0_u32;
        for (offset, byte) in source.as_bytes()[open..].iter().copied().enumerate() {
            match byte {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &source[open + 1..open + offset];
                    }
                }
                _ => {}
            }
        }
        panic!("unterminated ABI export {symbol}");
    }

    #[test]
    fn raw_failure_domains_are_disjoint_from_molt_object_bits() {
        assert_eq!(NullAddress::FAILURE, 0);
        assert_eq!(NullHandle::FAILURE, 0);
        assert_eq!(ZeroIndex::FAILURE, 0);
        assert_eq!(FailureStatus::FAILURE, 0);
        assert_ne!(NullAddress::FAILURE, MoltObject::none().bits());
    }

    #[test]
    fn memory_failure_records_one_allocation_free_exception_domain() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            crate::clear_exception(py);
            assert_eq!(super::fail_memory::<NullHandle>(py), 0);
            assert!(crate::exception_pending(py));
            crate::clear_exception(py);
            assert!(!crate::exception_pending(py));
        });
    }

    #[test]
    fn memory_failure_does_not_charge_the_resource_tracker() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
            max_memory: Some(1),
            max_allocations: Some(1),
            ..Default::default()
        })));
        let _reset = TrackerReset;
        crate::with_gil_entry_nopanic!(py, {
            assert_eq!(super::fail_memory::<NullAddress>(py), 0);
            assert!(crate::exception_pending(py));
            crate::clear_exception(py);
        });
        assert!(
            with_tracker(|tracker| tracker.on_allocate(1)).is_ok(),
            "allocation-free error recording must leave the one-allocation budget intact"
        );
        with_tracker(|tracker| tracker.on_free(1));
    }

    #[test]
    fn warm_runtime_raw_failures_never_return_boxed_object_sentinels() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let _ = crate::raise_exception::<u64>(py, "ValueError", "warm runtime");
            crate::clear_exception(py);
        });

        assert_eq!(crate::molt_scratch_alloc(u64::MAX), 0);
        let _ = crate::molt_exception_clear();
        assert_eq!(crate::c_api::molt_module_state_find(0), 0);
        let _ = crate::molt_exception_clear();
        assert_eq!(crate::c_api::PyErr_NoMemory(), 0);
        let _ = crate::molt_exception_clear();
        assert_eq!(
            crate::molt_list_int_getitem_raw_checked(MoltObject::none().bits(), 0),
            0
        );
        assert_eq!(
            crate::seq_snapshot_bridge::molt_seq_snapshot(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            ),
            0
        );
    }

    #[test]
    fn raw_integer_abi_export_census_has_explicit_non_object_domains() {
        let mut symbols = BTreeSet::new();
        for &(symbol, kind, source) in RAW_INTEGER_ABI_EXPORTS {
            assert!(
                symbols.insert(symbol),
                "duplicate ABI contract for {symbol}"
            );
            assert!(
                source.contains(&format!("fn {symbol}")),
                "stale ABI contract for {symbol}"
            );
            assert!(matches!(
                kind,
                AbiReturnKind::RawAddress
                    | AbiReturnKind::RawHandle
                    | AbiReturnKind::RawIndex
                    | AbiReturnKind::Status
            ));
        }
        assert_eq!(symbols.len(), RAW_INTEGER_ABI_EXPORTS.len());

        let scratch = function_body(include_str!("wasm_abi_exports.rs"), "molt_scratch_alloc");
        assert!(!scratch.contains("raise_exception::<u64>"));
        assert_eq!(
            scratch
                .matches("fail_memory::<crate::abi_return::NullAddress>")
                .count(),
            4,
            "every scratch allocation failure must return the typed null-address sentinel"
        );
        let module_find =
            function_body(include_str!("c_api/molt_api.rs"), "molt_module_state_find");
        assert!(module_find.contains("fail::<crate::abi_return::NullHandle>"));
        let no_memory = function_body(include_str!("c_api/cpython_compat.rs"), "PyErr_NoMemory");
        assert!(no_memory.contains("fail_memory::<crate::abi_return::NullHandle>"));
        let snapshot = function_body(include_str!("seq_snapshot_bridge.rs"), "molt_seq_snapshot");
        assert!(snapshot.contains("export(py, ptr, out_ptr, out_len)"));
        let snapshot_export = function_body(include_str!("seq_snapshot_bridge.rs"), "export");
        assert!(snapshot_export.contains("fail_memory::<crate::abi_return::FailureStatus>"));
    }
}
