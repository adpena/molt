use molt_obj_model::MoltObject;
use molt_runtime::MoltHeader;
use std::sync::Once;
use std::sync::atomic::Ordering;

const HEADER_FLAG_SKIP_CLASS_DECREF: u32 = 1 << 1;

#[unsafe(no_mangle)]
pub extern "C" fn molt_isolate_bootstrap() -> u64 {
    MoltObject::none().bits()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_isolate_import(_: u64) -> u64 {
    MoltObject::none().bits()
}

unsafe extern "C" {
    fn molt_runtime_init() -> u64;
    fn molt_exception_clear() -> u64;
    fn molt_string_from(data: *const u8, len: u64) -> u64;
    fn molt_module_new(name_bits: u64) -> u64;
    fn molt_module_set_attr(module_bits: u64, attr_bits: u64, val_bits: u64) -> u64;
    fn molt_module_del_global(module_bits: u64, name_bits: u64) -> u64;
    fn molt_list_builder_new(capacity_bits: u64) -> u64;
    fn molt_list_builder_append(builder_bits: u64, val: u64);
    fn molt_list_builder_finish_owned(builder_bits: u64) -> u64;
    fn molt_list_pop(list_bits: u64, index_bits: u64) -> u64;
}

static INIT: Once = Once::new();

fn init() {
    INIT.call_once(|| unsafe {
        molt_runtime_init();
    });
    let _ = unsafe { molt_exception_clear() };
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

fn refcount(bits: u64) -> u32 {
    header_ref(bits).ref_count.load(Ordering::Acquire)
}

fn class_from_name(name: &[u8]) -> u64 {
    let name_bits = unsafe { molt_string_from(name.as_ptr(), name.len() as u64) };
    assert_ne!(name_bits, none());
    let class_bits = molt_runtime::molt_class_new(name_bits);
    assert_ne!(class_bits, none());
    molt_runtime::molt_dec_ref_obj(name_bits);
    class_bits
}

#[test]
fn alloc_class_balances_heap_class_refcount() {
    init();

    let class_bits = class_from_name(b"HeapClassRef");
    let class_before = refcount(class_bits);

    let obj_bits = molt_runtime::molt_alloc_class(0, class_bits);
    assert_ne!(obj_bits, none());
    assert_eq!(molt_runtime::molt_type_of_borrowed(obj_bits), class_bits);
    assert_eq!(
        header_ref(obj_bits).flags & HEADER_FLAG_SKIP_CLASS_DECREF,
        0
    );
    assert_eq!(refcount(class_bits), class_before + 1);

    molt_runtime::molt_dec_ref_obj(obj_bits);
    assert_eq!(refcount(class_bits), class_before);

    molt_runtime::molt_dec_ref_obj(class_bits);
}

#[test]
fn alloc_class_static_marks_skip_class_decref_and_preserves_class_refcount() {
    init();

    let class_bits = class_from_name(b"HeapClassStatic");
    let class_before = refcount(class_bits);

    let obj_bits = molt_runtime::molt_alloc_class_static(0, class_bits);
    assert_ne!(obj_bits, none());
    assert_eq!(molt_runtime::molt_type_of_borrowed(obj_bits), class_bits);
    assert_ne!(
        header_ref(obj_bits).flags & HEADER_FLAG_SKIP_CLASS_DECREF,
        0
    );
    assert_eq!(refcount(class_bits), class_before);

    molt_runtime::molt_dec_ref_obj(obj_bits);
    assert_eq!(refcount(class_bits), class_before);

    molt_runtime::molt_dec_ref_obj(class_bits);
}

#[test]
fn list_clear_detaches_owned_heap_refs_before_cascade_decref() {
    init();

    let elem_bits = unsafe { molt_string_from(b"owned-element".as_ptr(), 13) };
    assert_ne!(elem_bits, none());
    let elem_before = refcount(elem_bits);

    let list_bits = molt_runtime::molt_list_fill_new(MoltObject::from_int(1).bits(), elem_bits);
    assert_ne!(list_bits, none());
    assert_eq!(refcount(elem_bits), elem_before + 1);

    molt_runtime::molt_dec_ref_obj(elem_bits);
    assert_eq!(refcount(elem_bits), elem_before);

    let clear_result = molt_runtime::molt_list_clear(list_bits);
    assert_eq!(clear_result, none());

    // The list no longer owns any element refs. Dropping the list must only
    // free the now-empty vector, not cascade a second dec-ref into freed memory.
    molt_runtime::molt_dec_ref_obj(list_bits);
}

#[test]
fn module_del_global_then_local_drop_releases_list_element_owner() {
    init();

    let module_name_bits = unsafe { molt_string_from(b"test_module".as_ptr(), 11) };
    assert_ne!(module_name_bits, none());
    let module_bits = unsafe { molt_module_new(module_name_bits) };
    assert_ne!(module_bits, none());
    molt_runtime::molt_dec_ref_obj(module_name_bits);

    let attr_bits = unsafe { molt_string_from(b"bag2".as_ptr(), 4) };
    assert_ne!(attr_bits, none());
    let elem_bits = unsafe { molt_string_from(b"owned-child".as_ptr(), 11) };
    assert_ne!(elem_bits, none());
    let elem_before = refcount(elem_bits);

    let list_bits = molt_runtime::molt_list_fill_new(MoltObject::from_int(1).bits(), elem_bits);
    assert_ne!(list_bits, none());
    assert_eq!(refcount(elem_bits), elem_before + 1);

    let set_result = unsafe { molt_module_set_attr(module_bits, attr_bits, list_bits) };
    assert_eq!(set_result, none());
    assert_eq!(refcount(list_bits), 2);

    let del_result = unsafe { molt_module_del_global(module_bits, attr_bits) };
    assert_eq!(del_result, none());
    assert_eq!(refcount(list_bits), 1);
    assert_eq!(refcount(elem_bits), elem_before + 1);

    molt_runtime::molt_dec_ref_obj(list_bits);
    assert_eq!(refcount(elem_bits), elem_before);

    molt_runtime::molt_dec_ref_obj(elem_bits);
    molt_runtime::molt_dec_ref_obj(attr_bits);
    molt_runtime::molt_dec_ref_obj(module_bits);
}

#[test]
fn module_del_global_releases_owned_list_builder_literal_element() {
    init();

    let module_name_bits = unsafe { molt_string_from(b"builder_module".as_ptr(), 14) };
    assert_ne!(module_name_bits, none());
    let module_bits = unsafe { molt_module_new(module_name_bits) };
    assert_ne!(module_bits, none());
    molt_runtime::molt_dec_ref_obj(module_name_bits);

    let attr_bits = unsafe { molt_string_from(b"bag2".as_ptr(), 4) };
    assert_ne!(attr_bits, none());
    let elem0_bits = unsafe { molt_string_from(b"owned-zero".as_ptr(), 10) };
    let elem1_bits = unsafe { molt_string_from(b"owned-one".as_ptr(), 9) };
    assert_ne!(elem0_bits, none());
    assert_ne!(elem1_bits, none());
    let elem0_before = refcount(elem0_bits);

    molt_runtime::molt_inc_ref_obj(elem0_bits);
    let builder_bits = unsafe { molt_list_builder_new(MoltObject::from_int(2).bits()) };
    assert_ne!(builder_bits, none());
    unsafe {
        molt_list_builder_append(builder_bits, elem0_bits);
        molt_list_builder_append(builder_bits, elem1_bits);
    }
    let list_bits = unsafe { molt_list_builder_finish_owned(builder_bits) };
    assert_ne!(list_bits, none());
    assert_eq!(refcount(elem0_bits), elem0_before + 1);

    let set_result = unsafe { molt_module_set_attr(module_bits, attr_bits, list_bits) };
    assert_eq!(set_result, none());
    assert_eq!(refcount(list_bits), 2);

    let popped_bits = unsafe { molt_list_pop(list_bits, none()) };
    assert_eq!(popped_bits, elem1_bits);
    molt_runtime::molt_dec_ref_obj(popped_bits);

    let del_result = unsafe { molt_module_del_global(module_bits, attr_bits) };
    assert_eq!(del_result, none());
    assert_eq!(refcount(list_bits), 1);
    assert_eq!(refcount(elem0_bits), elem0_before + 1);

    molt_runtime::molt_dec_ref_obj(list_bits);
    assert_eq!(refcount(elem0_bits), elem0_before);

    molt_runtime::molt_dec_ref_obj(elem0_bits);
    molt_runtime::molt_dec_ref_obj(attr_bits);
    molt_runtime::molt_dec_ref_obj(module_bits);
}
