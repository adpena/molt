use crate::object::ops::molt_print_builtin;
use crate::object::ops::type_name;
use crate::{
    MoltObject, PyToken, alloc_exception_from_class_bits, alloc_string, alloc_tuple, dec_ref_bits,
    exception_pending, exception_type_bits_from_name, obj_from_bits, record_exception,
};

const MOLT_CREDITS_TEXT: &str = concat!(
    "Molt is authored by Alejandro Peña and contributors.\n",
    "Molt is open source software.\n",
    "See the project repository for more information."
);

// Keep `license()` self-contained: compiled binaries should not depend on files
// being present at runtime.
const MOLT_LICENSE_TEXT: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../LICENSE"));

fn print_str(_py: &PyToken<'_>, text: &[u8]) {
    let ptr = alloc_string(_py, text);
    if ptr.is_null() {
        return;
    }
    let text_bits = MoltObject::from_ptr(ptr).bits();
    let args_ptr = alloc_tuple(_py, &[text_bits]);
    if args_ptr.is_null() {
        dec_ref_bits(_py, text_bits);
        return;
    }
    let args_bits = MoltObject::from_ptr(args_ptr).bits();
    let none_bits = MoltObject::none().bits();
    let flush_bits = MoltObject::from_bool(false).bits();
    let _res_bits = molt_print_builtin(args_bits, none_bits, none_bits, none_bits, flush_bits);
    dec_ref_bits(_py, args_bits);
    dec_ref_bits(_py, text_bits);
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_site_help0() -> u64 {
    crate::with_gil_entry!(_py, {
        print_str(_py, b"Molt help is not available in compiled binaries.");
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_site_help1(target_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        print_str(_py, b"Molt help is not available in compiled binaries.");
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let target_obj = obj_from_bits(target_bits);
        let ty = type_name(_py, target_obj);
        let mut buf = Vec::with_capacity(b"Target type: ".len() + ty.len());
        buf.extend_from_slice(b"Target type: ");
        buf.extend_from_slice(ty.as_bytes());
        print_str(_py, buf.as_slice());
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_site_credits() -> u64 {
    crate::with_gil_entry!(_py, {
        print_str(_py, MOLT_CREDITS_TEXT.as_bytes());
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_site_license() -> u64 {
    crate::with_gil_entry!(_py, {
        print_str(_py, MOLT_LICENSE_TEXT.as_bytes());
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_site_copyright() -> u64 {
    crate::with_gil_entry!(_py, {
        print_str(
            _py,
            "Copyright (c) 2026 Alejandro Peña.\nAll Rights Reserved.".as_bytes(),
        );
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_site_quitter_call(code_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let code_obj = obj_from_bits(code_bits);
        let args_ptr = if code_obj.is_none() {
            alloc_tuple(_py, &[])
        } else {
            alloc_tuple(_py, &[code_bits])
        };
        if args_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let args_bits = MoltObject::from_ptr(args_ptr).bits();
        let class_bits = exception_type_bits_from_name(_py, "SystemExit");
        if class_bits == 0 {
            dec_ref_bits(_py, args_bits);
            return MoltObject::none().bits();
        }
        let exc_ptr = alloc_exception_from_class_bits(_py, class_bits, args_bits);
        if exc_ptr.is_null() {
            return MoltObject::none().bits();
        }
        record_exception(_py, exc_ptr);
        // Raising is communicated via the exception state; `None` is the sentinel.
        MoltObject::none().bits()
    })
}
