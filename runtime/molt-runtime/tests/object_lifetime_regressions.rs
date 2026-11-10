use molt_obj_model::MoltObject;
use molt_runtime::MoltHeader;
use std::sync::{Mutex, MutexGuard, Once};

molt_runtime::declare_app_bootstrap!(molt_runtime::AppBootstrapProvider::Unavailable(
    "molt-runtime/object_lifetime_regressions"
));

unsafe extern "C" {
    fn molt_runtime_init() -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_exception_clear") ]
    fn molt_exception_clear(context: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_string_from") ]
    fn molt_string_from(context: u64, data: *const u8, len: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_module_new") ]
    fn molt_module_new(context: u64, name_bits: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_module_set_attr") ]
    fn molt_module_set_attr(context: u64, module_bits: u64, attr_bits: u64, val_bits: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_module_del_global") ]
    fn molt_module_del_global(context: u64, module_bits: u64, name_bits: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_list_builder_new") ]
    fn molt_list_builder_new(context: u64, capacity_bits: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_list_builder_append") ]
    fn molt_list_builder_append(context: u64, builder_bits: u64, val: u64) -> i32;
    #[link_name = molt_runtime_core::attached_symbol!("molt_list_builder_finish") ]
    fn molt_list_builder_finish(context: u64, builder_bits: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_list_pop") ]
    fn molt_list_pop(context: u64, list_bits: u64, index_bits: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_iter") ]
    fn molt_iter(context: u64, iter_bits: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_iter_next_unboxed") ]
    fn molt_iter_next_unboxed(context: u64, iter_bits: u64, value_out: *mut u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_iter_next_dict_items") ]
    fn molt_iter_next_dict_items(
        context: u64,
        iter_bits: u64,
        key_out: *mut u64,
        value_out: *mut u64,
    ) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_dict_new") ]
    fn molt_dict_new(context: u64, capacity_bits: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_dict_set") ]
    fn molt_dict_set(context: u64, dict_bits: u64, key_bits: u64, val_bits: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_dict_items") ]
    fn molt_dict_items(context: u64, dict_bits: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_string_join") ]
    fn molt_string_join(context: u64, sep_bits: u64, items_bits: u64) -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_string_eq") ]
    fn molt_string_eq(context: u64, a_bits: u64, b_bits: u64) -> u64;
}

static INIT: Once = Once::new();
static TEST_LOCK: Mutex<()> = Mutex::new(());

struct Fixture {
    execution: molt_runtime_core::CoreGilGuard,
    _exclusive: MutexGuard<'static, ()>,
}

fn init() -> Fixture {
    let exclusive = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    INIT.call_once(|| unsafe {
        molt_runtime_init();
    });
    let execution = molt_runtime_core::CoreGilGuard::new();
    let py = unsafe { execution.__token_unchecked() };
    unsafe {
        molt_exception_clear(py.attached_context());
    }
    Fixture {
        execution,
        _exclusive: exclusive,
    }
}

fn none() -> u64 {
    MoltObject::none().bits()
}

fn header_ref(bits: u64) -> &'static MoltHeader {
    let ptr = MoltObject::from_bits(bits)
        .as_ptr()
        .expect("expected heap object pointer");
    let header_ptr = unsafe { ptr.sub(std::mem::size_of::<MoltHeader>()) as *const MoltHeader };
    unsafe { &*header_ptr }
}

fn object_ptr(bits: u64) -> *mut u8 {
    MoltObject::from_bits(bits)
        .as_ptr()
        .expect("expected heap object pointer")
}

fn refcount(bits: u64) -> u32 {
    header_ref(bits).ref_count_snapshot()
}

fn assert_string_eq(py: &molt_runtime_core::PyToken, lhs: u64, rhs: u64) {
    let eq_bits = unsafe { molt_string_eq(py.attached_context(), lhs, rhs) };
    assert_eq!(MoltObject::from_bits(eq_bits).as_bool(), Some(true));
}

#[test]
fn iter_next_unboxed_overwrites_value_out_on_exhaustion() {
    let _guard = init();
    let py = unsafe { _guard.execution.__token_unchecked() };

    let elem_bits = unsafe { molt_string_from(py.attached_context(), b"iter-owned".as_ptr(), 10) };
    assert_ne!(elem_bits, none());
    let elem_before = refcount(elem_bits);

    let list_bits = unsafe {
        molt_runtime::molt_list_fill_new(
            py.attached_context(),
            MoltObject::from_int(1).bits(),
            elem_bits,
        )
    };
    assert_ne!(list_bits, none());
    assert_eq!(refcount(elem_bits), elem_before + 1);

    let iter_bits = unsafe { molt_iter(py.attached_context(), list_bits) };
    assert_ne!(iter_bits, none());

    let done_false = MoltObject::from_bool(false).bits();
    let done_true = MoltObject::from_bool(true).bits();

    let mut value_bits = none();
    let done_bits =
        unsafe { molt_iter_next_unboxed(py.attached_context(), iter_bits, &mut value_bits) };
    assert_eq!(done_bits, done_false);
    assert_eq!(value_bits, elem_bits);
    assert_eq!(refcount(elem_bits), elem_before + 2);
    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), value_bits) };

    value_bits = elem_bits;
    let done_bits =
        unsafe { molt_iter_next_unboxed(py.attached_context(), iter_bits, &mut value_bits) };
    assert_eq!(done_bits, done_true);
    assert_eq!(value_bits, none());

    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), iter_bits) };
    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), list_bits) };
    assert_eq!(refcount(elem_bits), elem_before);
    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), elem_bits) };
}

#[test]
fn iter_next_dict_items_overwrites_outputs_on_exhaustion() {
    let _guard = init();
    let py = unsafe { _guard.execution.__token_unchecked() };

    let dict_bits = unsafe { molt_dict_new(py.attached_context(), 1) };
    assert_ne!(dict_bits, none());
    let key_bits = unsafe { molt_string_from(py.attached_context(), b"k".as_ptr(), 1) };
    let val_bits = unsafe { molt_string_from(py.attached_context(), b"v".as_ptr(), 1) };
    assert_ne!(key_bits, none());
    assert_ne!(val_bits, none());

    let set_result = unsafe { molt_dict_set(py.attached_context(), dict_bits, key_bits, val_bits) };
    assert_eq!(set_result, dict_bits);

    let items_bits = unsafe { molt_dict_items(py.attached_context(), dict_bits) };
    assert_ne!(items_bits, none());
    let iter_bits = unsafe { molt_iter(py.attached_context(), items_bits) };
    assert_ne!(iter_bits, none());

    let done_false = MoltObject::from_bool(false).bits();
    let done_true = MoltObject::from_bool(true).bits();

    let mut out_key_bits = none();
    let mut out_val_bits = none();
    let done_bits = unsafe {
        molt_iter_next_dict_items(
            py.attached_context(),
            iter_bits,
            &mut out_key_bits,
            &mut out_val_bits,
        )
    };
    assert_eq!(done_bits, done_false);
    assert_eq!(out_key_bits, key_bits);
    assert_eq!(out_val_bits, val_bits);
    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), out_key_bits) };
    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), out_val_bits) };

    out_key_bits = key_bits;
    out_val_bits = val_bits;
    let done_bits = unsafe {
        molt_iter_next_dict_items(
            py.attached_context(),
            iter_bits,
            &mut out_key_bits,
            &mut out_val_bits,
        )
    };
    assert_eq!(done_bits, done_true);
    assert_eq!(out_key_bits, none());
    assert_eq!(out_val_bits, none());

    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), iter_bits) };
    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), items_bits) };
    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), dict_bits) };
    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), key_bits) };
    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), val_bits) };
}

#[test]
fn list_clear_detaches_owned_heap_refs_before_cascade_decref() {
    let _guard = init();
    let py = unsafe { _guard.execution.__token_unchecked() };

    let elem_bits =
        unsafe { molt_string_from(py.attached_context(), b"owned-element".as_ptr(), 13) };
    assert_ne!(elem_bits, none());
    let elem_before = refcount(elem_bits);

    let list_bits = unsafe {
        molt_runtime::molt_list_fill_new(
            py.attached_context(),
            MoltObject::from_int(1).bits(),
            elem_bits,
        )
    };
    assert_ne!(list_bits, none());
    assert_eq!(refcount(elem_bits), elem_before + 1);

    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), elem_bits) };
    assert_eq!(refcount(elem_bits), elem_before);

    let clear_result = unsafe { molt_runtime::molt_list_clear(py.attached_context(), list_bits) };
    assert_eq!(clear_result, none());

    // The list no longer owns any element refs. Dropping the list must only
    // free the now-empty vector, not cascade a second dec-ref into freed memory.
    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), list_bits) };
}

#[test]
fn module_del_global_then_local_drop_releases_list_element_owner() {
    let _guard = init();
    let py = unsafe { _guard.execution.__token_unchecked() };

    let module_name_bits =
        unsafe { molt_string_from(py.attached_context(), b"test_module".as_ptr(), 11) };
    assert_ne!(module_name_bits, none());
    let module_bits = unsafe { molt_module_new(py.attached_context(), module_name_bits) };
    assert_ne!(module_bits, none());
    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), module_name_bits) };

    let attr_bits = unsafe { molt_string_from(py.attached_context(), b"bag2".as_ptr(), 4) };
    assert_ne!(attr_bits, none());
    let elem_bits = unsafe { molt_string_from(py.attached_context(), b"owned-child".as_ptr(), 11) };
    assert_ne!(elem_bits, none());
    let elem_before = refcount(elem_bits);

    let list_bits = unsafe {
        molt_runtime::molt_list_fill_new(
            py.attached_context(),
            MoltObject::from_int(1).bits(),
            elem_bits,
        )
    };
    assert_ne!(list_bits, none());
    assert_eq!(refcount(elem_bits), elem_before + 1);

    let set_result =
        unsafe { molt_module_set_attr(py.attached_context(), module_bits, attr_bits, list_bits) };
    assert_eq!(set_result, none());
    assert_eq!(refcount(list_bits), 2);

    let del_result =
        unsafe { molt_module_del_global(py.attached_context(), module_bits, attr_bits) };
    assert_eq!(del_result, none());
    assert_eq!(refcount(list_bits), 1);
    assert_eq!(refcount(elem_bits), elem_before + 1);

    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), list_bits) };
    assert_eq!(refcount(elem_bits), elem_before);

    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), elem_bits) };
    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), attr_bits) };
    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), module_bits) };
}

#[test]
fn module_del_global_releases_owned_list_builder_literal_element() {
    let _guard = init();
    let py = unsafe { _guard.execution.__token_unchecked() };

    let module_name_bits =
        unsafe { molt_string_from(py.attached_context(), b"builder_module".as_ptr(), 14) };
    assert_ne!(module_name_bits, none());
    let module_bits = unsafe { molt_module_new(py.attached_context(), module_name_bits) };
    assert_ne!(module_bits, none());
    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), module_name_bits) };

    let attr_bits = unsafe { molt_string_from(py.attached_context(), b"bag2".as_ptr(), 4) };
    assert_ne!(attr_bits, none());
    let elem0_bits = unsafe { molt_string_from(py.attached_context(), b"owned-zero".as_ptr(), 10) };
    let elem1_bits = unsafe { molt_string_from(py.attached_context(), b"owned-one".as_ptr(), 9) };
    assert_ne!(elem0_bits, none());
    assert_ne!(elem1_bits, none());
    let elem0_before = refcount(elem0_bits);

    let builder_bits =
        unsafe { molt_list_builder_new(py.attached_context(), MoltObject::from_int(2).bits()) };
    assert_ne!(builder_bits, none());
    unsafe {
        assert_eq!(
            molt_list_builder_append(py.attached_context(), builder_bits, elem0_bits),
            0
        );
        assert_eq!(
            molt_list_builder_append(py.attached_context(), builder_bits, elem1_bits),
            0
        );
    }
    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), elem1_bits) };
    let list_bits = unsafe { molt_list_builder_finish(py.attached_context(), builder_bits) };
    assert_ne!(list_bits, none());
    assert_eq!(refcount(elem0_bits), elem0_before + 1);

    let set_result =
        unsafe { molt_module_set_attr(py.attached_context(), module_bits, attr_bits, list_bits) };
    assert_eq!(set_result, none());
    assert_eq!(refcount(list_bits), 2);

    let popped_bits = unsafe {
        molt_list_pop(
            py.attached_context(),
            list_bits,
            MoltObject::from_int(-1).bits(),
        )
    };
    assert_eq!(popped_bits, elem1_bits);
    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), popped_bits) };

    let del_result =
        unsafe { molt_module_del_global(py.attached_context(), module_bits, attr_bits) };
    assert_eq!(del_result, none());
    assert_eq!(refcount(list_bits), 1);
    assert_eq!(refcount(elem0_bits), elem0_before + 1);

    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), list_bits) };
    assert_eq!(refcount(elem0_bits), elem0_before);

    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), elem0_bits) };
    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), attr_bits) };
    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), module_bits) };
}

#[test]
fn string_join_singleton_list_returns_owned_exact_string() {
    let _guard = init();
    let py = unsafe { _guard.execution.__token_unchecked() };

    // Avoid immortal ASCII-character and identifier interns: this fixture must
    // observe real ownership changes, including the final surviving reference.
    let sep_bits = unsafe { molt_string_from(py.attached_context(), b"::".as_ptr(), 2) };
    let elem_bits = unsafe { molt_string_from(py.attached_context(), b"math value".as_ptr(), 10) };
    let expected_bits =
        unsafe { molt_string_from(py.attached_context(), b"math value".as_ptr(), 10) };
    assert_ne!(sep_bits, none());
    assert_ne!(elem_bits, none());
    assert_ne!(expected_bits, none());
    assert_eq!(refcount(sep_bits), 1);
    assert_eq!(refcount(elem_bits), 1);

    let builder_bits =
        unsafe { molt_list_builder_new(py.attached_context(), MoltObject::from_int(1).bits()) };
    assert_ne!(builder_bits, none());
    unsafe {
        assert_eq!(
            molt_list_builder_append(py.attached_context(), builder_bits, elem_bits),
            0
        );
    }
    let list_bits = unsafe { molt_list_builder_finish(py.attached_context(), builder_bits) };
    assert_ne!(list_bits, none());
    assert_eq!(refcount(list_bits), 1);
    assert_eq!(refcount(elem_bits), 2);

    // CPython's exact-str singleton result is the same object with a new owned
    // reference. Identity alone would also accept an unsafe borrowed return.
    let joined_bits = unsafe { molt_string_join(py.attached_context(), sep_bits, list_bits) };
    assert_ne!(joined_bits, none());
    assert_eq!(object_ptr(joined_bits), object_ptr(elem_bits));
    assert_eq!(refcount(elem_bits), 3);
    assert_eq!(refcount(list_bits), 1);
    assert_eq!(refcount(sep_bits), 1);
    assert_string_eq(py, joined_bits, expected_bits);

    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), joined_bits) };
    assert_eq!(refcount(elem_bits), 2);
    assert_string_eq(py, elem_bits, expected_bits);

    let joined_bits = unsafe { molt_string_join(py.attached_context(), sep_bits, list_bits) };
    assert_ne!(joined_bits, none());
    assert_eq!(object_ptr(joined_bits), object_ptr(elem_bits));
    assert_eq!(refcount(elem_bits), 3);
    assert_eq!(refcount(list_bits), 1);
    assert_eq!(refcount(sep_bits), 1);

    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), list_bits) };
    assert_eq!(refcount(joined_bits), 2);
    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), elem_bits) };
    assert_eq!(refcount(joined_bits), 1);
    assert_string_eq(py, joined_bits, expected_bits);

    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), expected_bits) };
    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), joined_bits) };
    unsafe { molt_runtime::molt_dec_ref_obj(py.attached_context(), sep_bits) };
}
