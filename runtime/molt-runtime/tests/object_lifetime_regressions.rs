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
    fn molt_itertools_alloc_class(name_ptr: *const u8, name_len: usize, layout_size: i64) -> u64;
    fn molt_itertools_object_class_bits(ptr: *mut u8) -> u64;
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

#[test]
fn alloc_class_balances_heap_class_refcount() {
    init();

    let class_bits = unsafe { molt_itertools_alloc_class(b"HeapClassRef".as_ptr(), 12, 0) };
    assert_ne!(class_bits, none());
    let class_before = refcount(class_bits);

    let obj_bits = molt_runtime::molt_alloc_class(0, class_bits);
    assert_ne!(obj_bits, none());
    assert_eq!(
        unsafe {
            molt_itertools_object_class_bits(
            MoltObject::from_bits(obj_bits)
                .as_ptr()
                .expect("expected instance pointer")
            )
        },
        class_bits
    );
    assert_eq!(header_ref(obj_bits).flags & HEADER_FLAG_SKIP_CLASS_DECREF, 0);
    assert_eq!(refcount(class_bits), class_before + 1);

    molt_runtime::molt_dec_ref_obj(obj_bits);
    assert_eq!(refcount(class_bits), class_before);

    molt_runtime::molt_dec_ref_obj(class_bits);
}

#[test]
fn alloc_class_static_marks_skip_class_decref_and_preserves_class_refcount() {
    init();

    let class_bits = unsafe { molt_itertools_alloc_class(b"HeapClassStatic".as_ptr(), 15, 0) };
    assert_ne!(class_bits, none());
    let class_before = refcount(class_bits);

    let obj_bits = molt_runtime::molt_alloc_class_static(0, class_bits);
    assert_ne!(obj_bits, none());
    assert_eq!(
        unsafe {
            molt_itertools_object_class_bits(
            MoltObject::from_bits(obj_bits)
                .as_ptr()
                .expect("expected instance pointer")
            )
        },
        class_bits
    );
    assert_ne!(header_ref(obj_bits).flags & HEADER_FLAG_SKIP_CLASS_DECREF, 0);
    assert_eq!(refcount(class_bits), class_before);

    molt_runtime::molt_dec_ref_obj(obj_bits);
    assert_eq!(refcount(class_bits), class_before);

    molt_runtime::molt_dec_ref_obj(class_bits);
}
