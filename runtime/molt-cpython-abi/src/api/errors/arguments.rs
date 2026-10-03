//! One format grammar and physical-object ingress for every PyArg parser.
//! The C translation unit only collects the output addresses from va_list.

use super::*;
use crate::api::refcount::OwnedPyObject;
use crate::bridge::RuntimeValue;
use std::ops::Range;

struct FormatUnit {
    source: Range<usize>,
    outputs: Range<usize>,
    children: Vec<FormatUnit>,
}

struct ArgumentFormat {
    units: Vec<FormatUnit>,
    required: usize,
    positional: usize,
    outputs: usize,
}

enum ParseResource {
    Buffer(*mut Py_buffer),
    Encoded(*mut *mut c_char),
    Converted(ConverterFn, *mut c_void),
}

impl ParseResource {
    unsafe fn release(&self) {
        match *self {
            Self::Buffer(view) => unsafe { crate::api::buffer::PyBuffer_Release(view) },
            Self::Encoded(address) => unsafe {
                crate::api::memory::PyMem_Free((*address).cast());
                *address = ptr::null_mut();
            },
            Self::Converted(convert, address) => {
                unsafe { convert(ptr::null_mut(), address) };
            }
        }
    }
}

#[derive(Default)]
struct ParseCleanup {
    resources: Vec<ParseResource>,
    committed: bool,
}

impl ParseCleanup {
    fn retain(&mut self, resource: ParseResource) -> bool {
        if self.resources.try_reserve(1).is_err() {
            unsafe { PyErr_NoMemory() };
            with_preserved_error(|| unsafe { resource.release() });
            return false;
        }
        self.resources.push(resource);
        true
    }
}

impl Drop for ParseCleanup {
    fn drop(&mut self) {
        if !self.committed {
            for resource in &self.resources {
                with_preserved_error(|| unsafe { resource.release() });
            }
        }
    }
}

fn format_error<T>() -> Option<T> {
    unsafe { set_parse_format_error() };
    None
}

fn skip_format_space(format: &[u8], cursor: &mut usize) {
    while format.get(*cursor).is_some_and(u8::is_ascii_whitespace) {
        *cursor += 1;
    }
}

fn read_format_unit(
    format: &[u8],
    cursor: &mut usize,
    outputs: &mut usize,
    depth: usize,
) -> Option<FormatUnit> {
    if depth >= 32 {
        return format_error();
    }
    skip_format_space(format, cursor);
    let start = *cursor;
    let first_output = *outputs;
    let Some(&code) = format.get(*cursor) else {
        return format_error();
    };
    *cursor += 1;
    let mut children = Vec::new();
    let count = match code {
        b'(' => {
            loop {
                skip_format_space(format, cursor);
                if format.get(*cursor) == Some(&b')') {
                    *cursor += 1;
                    break;
                }
                children.push(read_format_unit(format, cursor, outputs, depth + 1)?);
            }
            0
        }
        b'O' => {
            if matches!(format.get(*cursor), Some(b'!' | b'&')) {
                *cursor += 1;
                2
            } else {
                1
            }
        }
        b's' | b'z' | b'y' => match format.get(*cursor) {
            Some(b'#') => {
                *cursor += 1;
                2
            }
            Some(b'*') => {
                *cursor += 1;
                1
            }
            _ => 1,
        },
        b'e' => {
            if !matches!(format.get(*cursor), Some(b's' | b't')) {
                return format_error();
            }
            *cursor += 1;
            if format.get(*cursor) == Some(&b'#') {
                *cursor += 1;
                3
            } else {
                2
            }
        }
        b'w' => {
            if format.get(*cursor) != Some(&b'*') {
                return format_error();
            }
            *cursor += 1;
            1
        }
        b'b' | b'B' | b'h' | b'H' | b'i' | b'I' | b'l' | b'k' | b'L' | b'K' | b'n' | b'd'
        | b'f' | b'D' | b'p' | b'c' | b'C' | b'S' | b'Y' | b'U' => 1,
        _ => return format_error(),
    };
    let Some(total) = outputs
        .checked_add(count)
        .filter(|count| *count <= c_int::MAX as usize)
    else {
        unsafe { PyErr_NoMemory() };
        return None;
    };
    *outputs = total;
    Some(FormatUnit {
        source: start..*cursor,
        outputs: first_output..*outputs,
        children,
    })
}

impl ArgumentFormat {
    fn read(format: &[u8], keywords: bool) -> Option<Self> {
        let mut units = Vec::new();
        let mut required = None;
        let mut positional = None;
        let mut outputs = 0;
        let mut cursor = 0;
        loop {
            skip_format_space(format, &mut cursor);
            match format.get(cursor) {
                None | Some(b':' | b';') => break,
                Some(b'|') => {
                    if required.is_some() || positional.is_some() {
                        return format_error();
                    }
                    required = Some(units.len());
                    cursor += 1;
                }
                Some(b'$') => {
                    if !keywords || required.is_none() || positional.is_some() {
                        return format_error();
                    }
                    positional = Some(units.len());
                    cursor += 1;
                }
                _ => units.push(read_format_unit(format, &mut cursor, &mut outputs, 0)?),
            }
        }
        Some(Self {
            required: required.unwrap_or(units.len()),
            positional: positional.unwrap_or(units.len()),
            units,
            outputs,
        })
    }
}

/// The same grammar that binds arguments tells C how many addresses to read.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_pyarg_format_out_count(format: *const c_char) -> c_int {
    if format.is_null() {
        unsafe { PyErr_BadInternalCall() };
        return -1;
    }
    let format = unsafe { CStr::from_ptr(format).to_bytes() };
    ArgumentFormat::read(format, true).map_or(-1, |plan| plan.outputs as c_int)
}

/// Snapshot exact borrowed dictionary pointers before any converter can reenter.
unsafe fn keyword_items(kwargs: *mut PyObject) -> Option<Vec<(OwnedPyObject, OwnedPyObject)>> {
    if unsafe { crate::api::mapping::PyDict_Check(kwargs) } == 0 {
        unsafe { PyErr_BadInternalCall() };
        return None;
    }
    let mut items = Vec::new();
    let mut position = 0;
    let mut key = ptr::null_mut();
    let mut value = ptr::null_mut();
    while unsafe {
        crate::api::mapping::PyDict_Next(kwargs, &raw mut position, &raw mut key, &raw mut value)
    } != 0
    {
        if unsafe { crate::api::strings::PyUnicode_Check(key) } == 0 {
            unsafe { set_parse_type_error("keywords must be strings") };
            return None;
        }
        if items.try_reserve(1).is_err() {
            unsafe { PyErr_NoMemory() };
            return None;
        }
        items.push((unsafe { OwnedPyObject::from_borrowed(key) }, unsafe {
            OwnedPyObject::from_borrowed(value)
        }));
    }
    if !unsafe { PyErr_Occurred() }.is_null() {
        return None;
    }
    Some(items)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyArg_ValidateKeywordArguments(kwargs: *mut PyObject) -> c_int {
    let kwargs = unsafe { OwnedPyObject::from_borrowed(kwargs) };
    i32::from(unsafe { keyword_items(kwargs.as_ptr()) }.is_some())
}

/// Pin the input containers, then read and pin each argument at its conversion
/// point. A converter may mutate later keyword values. Missing/duplicate/extra
/// arguments are diagnosed in the same order as the conversion transaction.
unsafe fn parse_arguments(
    args: *mut PyObject,
    kwargs: *mut PyObject,
    format: *const c_char,
    kwlist: *mut *mut c_char,
    outs: *mut *mut c_void,
    n_outs: c_int,
) -> c_int {
    if args.is_null() || format.is_null() {
        unsafe { PyErr_BadInternalCall() };
        return 0;
    }
    let args = unsafe { OwnedPyObject::from_borrowed(args) };
    let kwargs = unsafe { OwnedPyObject::from_borrowed(kwargs) };
    let format = unsafe { CStr::from_ptr(format).to_bytes() };
    let Some(plan) = ArgumentFormat::read(format, !kwlist.is_null()) else {
        return 0;
    };
    if n_outs < 0 || n_outs as usize != plan.outputs || (plan.outputs != 0 && outs.is_null()) {
        unsafe { PyErr_BadInternalCall() };
        return 0;
    }
    let outs = if plan.outputs == 0 {
        &mut []
    } else {
        unsafe { std::slice::from_raw_parts_mut(outs, plan.outputs) }
    };
    let nargs = unsafe { crate::api::sequences::PyTuple_Size(args.as_ptr()) };
    if nargs < 0 {
        return 0;
    }
    let nargs = nargs as usize;
    if kwlist.is_null() && nargs < plan.required {
        unsafe { set_parse_type_error("required argument is missing") };
        return 0;
    }
    if kwlist.is_null() && nargs > plan.positional {
        let qualifier = if plan.required == plan.positional {
            "exactly"
        } else {
            "at most"
        };
        let plural = if plan.positional == 1 { "" } else { "s" };
        unsafe {
            set_parse_type_error(&format!(
                "function takes {qualifier} {} argument{plural} ({nargs} given)",
                plan.positional
            ))
        };
        return 0;
    }
    let mut names = Vec::new();
    let mut remaining_keywords = 0usize;
    if !kwlist.is_null() {
        let mut named = false;
        for index in 0..plan.units.len() {
            let name = unsafe { *kwlist.add(index) };
            if name.is_null() {
                unsafe { set_parse_format_error() };
                return 0;
            }
            let name = unsafe { CStr::from_ptr(name).to_bytes() };
            if name.is_empty() {
                if named || index >= plan.positional {
                    unsafe { set_parse_format_error() };
                    return 0;
                }
            } else {
                named = true;
                if names.contains(&name) {
                    unsafe { set_parse_format_error() };
                    return 0;
                }
            }
            names.push(name);
        }
        if !unsafe { *kwlist.add(plan.units.len()) }.is_null() {
            unsafe { set_parse_format_error() };
            return 0;
        }
        if !kwargs.as_ptr().is_null() {
            if unsafe { crate::api::mapping::PyDict_Check(kwargs.as_ptr()) } == 0 {
                unsafe { PyErr_BadInternalCall() };
                return 0;
            }
            let size = unsafe { crate::api::mapping::PyDict_Size(kwargs.as_ptr()) };
            if size < 0 {
                return 0;
            }
            remaining_keywords = size as usize;
        }
        if nargs.saturating_add(remaining_keywords) > plan.units.len() {
            unsafe { set_parse_type_error("too many arguments") };
            return 0;
        }
    }
    let mut cleanup = ParseCleanup::default();
    for (index, unit) in plan.units.iter().enumerate() {
        if index == plan.positional && nargs > plan.positional {
            unsafe { set_parse_type_error("too many positional arguments") };
            return 0;
        }
        let value = if index < nargs {
            let item = unsafe {
                crate::api::sequences::PyTuple_GetItem(args.as_ptr(), index as Py_ssize_t)
            };
            if item.is_null() {
                return 0;
            }
            unsafe { OwnedPyObject::from_borrowed(item) }
        } else if remaining_keywords != 0 && !names[index].is_empty() {
            let mut item = ptr::null_mut();
            let found = unsafe {
                crate::api::mapping::PyDict_GetItemStringRef(
                    kwargs.as_ptr(),
                    *kwlist.add(index),
                    &raw mut item,
                )
            };
            if found < 0 {
                return 0;
            }
            if found != 0 {
                remaining_keywords -= 1;
            }
            unsafe { OwnedPyObject::from_owned(item) }
        } else {
            unsafe { OwnedPyObject::from_owned(ptr::null_mut()) }
        };
        if value.as_ptr().is_null() {
            if index < plan.required {
                unsafe { set_parse_type_error("required argument is missing") };
                return 0;
            }
            continue;
        }
        if unsafe { convert_unit(unit, &value, format, outs, &mut cleanup) } == 0 {
            return 0;
        }
    }
    if remaining_keywords != 0 {
        for (index, name) in names.iter().take(nargs).enumerate() {
            if name.is_empty() {
                continue;
            }
            let mut item = ptr::null_mut();
            let found = unsafe {
                crate::api::mapping::PyDict_GetItemStringRef(
                    kwargs.as_ptr(),
                    *kwlist.add(index),
                    &raw mut item,
                )
            };
            let _item = unsafe { OwnedPyObject::from_owned(item) };
            if found < 0 {
                return 0;
            }
            if found != 0 {
                unsafe {
                    set_parse_type_error(&format!(
                        "argument '{}' given by name and position",
                        String::from_utf8_lossy(name)
                    ))
                };
                return 0;
            }
        }
        let Some(items) = (unsafe { keyword_items(kwargs.as_ptr()) }) else {
            return 0;
        };
        for (key, _value) in items {
            let mut size = 0;
            let text = unsafe {
                crate::api::strings::PyUnicode_AsUTF8AndSize(key.as_ptr(), &raw mut size)
            };
            if text.is_null() {
                return 0;
            }
            let text = unsafe { std::slice::from_raw_parts(text.cast::<u8>(), size as usize) };
            if !names.iter().any(|name| !name.is_empty() && *name == text) {
                unsafe {
                    set_parse_type_error(&format!(
                        "unexpected keyword argument '{}'",
                        String::from_utf8_lossy(text)
                    ))
                };
                return 0;
            }
        }
        // Callbacks may have removed the original unmatched keys. The initial
        // residual count still makes this an invalid call and requires cleanup.
        unsafe { set_parse_type_error("invalid keyword arguments") };
        return 0;
    }
    cleanup.committed = true;
    1
}

unsafe fn convert_unit(
    unit: &FormatUnit,
    item: &OwnedPyObject,
    format: &[u8],
    outs: &mut [*mut c_void],
    cleanup: &mut ParseCleanup,
) -> c_int {
    if format[unit.source.start] != b'(' {
        return unsafe {
            convert_argument(
                item,
                &format[unit.source.clone()],
                &mut outs[unit.outputs.clone()],
                cleanup,
            )
        };
    }
    if unsafe { crate::api::strings::PyUnicode_Check(item.as_ptr()) } != 0 {
        unsafe { set_parse_type_error("argument must be a sequence, not str") };
        return 0;
    }
    let len = unsafe { crate::api::abstract_sequence::PySequence_Size(item.as_ptr()) };
    if len < 0 {
        return 0;
    }
    if len as usize != unit.children.len() {
        unsafe { set_parse_type_error("argument sequence has incorrect length") };
        return 0;
    }
    for (index, unit) in unit.children.iter().enumerate() {
        let child = unsafe {
            OwnedPyObject::from_owned(crate::api::abstract_sequence::PySequence_GetItem(
                item.as_ptr(),
                index as Py_ssize_t,
            ))
        };
        if child.as_ptr().is_null() {
            return 0;
        }
        if unsafe { convert_unit(unit, &child, format, outs, cleanup) } == 0 {
            return 0;
        }
    }
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_pyarg_parse_tuple_inner(
    args: *mut PyObject,
    format: *const c_char,
    outs: *mut *mut c_void,
    n_outs: c_int,
) -> c_int {
    unsafe { parse_arguments(args, ptr::null_mut(), format, ptr::null_mut(), outs, n_outs) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_pyarg_parse_tuple_keywords_inner(
    args: *mut PyObject,
    kwargs: *mut PyObject,
    format: *const c_char,
    kwlist: *mut *mut c_char,
    outs: *mut *mut c_void,
    n_outs: c_int,
) -> c_int {
    if kwlist.is_null() {
        unsafe { PyErr_BadInternalCall() };
        return 0;
    }
    unsafe { parse_arguments(args, kwargs, format, kwlist, outs, n_outs) }
}

unsafe fn convert_argument(
    item: &OwnedPyObject,
    fmt: &[u8],
    outs_slice: &mut [*mut c_void],
    cleanup: &mut ParseCleanup,
) -> c_int {
    let ch = fmt[0] as char;
    // The format plan already sliced one unit and its output addresses.
    // Output positions are local to this unit; no streaming cursor survives.
    macro_rules! write_out {
        ($index:expr, $ty:ty, $val:expr) => {{
            // Evaluate the value (which may itself raise + early-return)
            // OUTSIDE the unsafe store so it never nests in this block.
            let stored: $ty = $val;
            if $index < outs_slice.len() && !outs_slice[$index].is_null() {
                unsafe {
                    *(outs_slice[$index] as *mut $ty) = stored;
                }
            }
        }};
    }

    if matches!(ch, 'D' | 'Y' | 'w' | 'O' | 'S' | 'U' | 'p') {
        match ch {
            'D' => {
                let py_ptr = item.as_ptr();
                let value = unsafe { crate::api::numbers::PyComplex_AsCComplex(py_ptr) };
                if !unsafe { PyErr_Occurred() }.is_null() {
                    return 0;
                }
                write_out!(0, Py_complex, value);
            }
            'Y' => {
                let py_ptr = item.as_ptr();
                if unsafe { crate::api::strings::PyByteArray_Check(py_ptr) } != 0 {
                    write_out!(0, *mut PyObject, py_ptr);
                } else {
                    unsafe { set_parse_type_error("argument must be bytearray") };
                    return 0;
                }
            }
            'w' => {
                if fmt.get(1) != Some(&b'*') {
                    unsafe { set_parse_format_error() };
                    return 0;
                }
                let view = outs_slice
                    .first()
                    .copied()
                    .unwrap_or(ptr::null_mut())
                    .cast::<Py_buffer>();
                let py_ptr = item.as_ptr();
                if unsafe { crate::api::buffer::PyObject_GetBuffer(py_ptr, view, PyBUF_WRITABLE) }
                    != 0
                {
                    return 0;
                }
                if !cleanup.retain(ParseResource::Buffer(view)) {
                    return 0;
                }
            }
            'O' => {
                let py_ptr = item.as_ptr();
                // Peek for the 'O!' (type-checked) / 'O&' (converter) modifiers.
                let modifier = if matches!(fmt.get(1), Some(b'!' | b'&')) {
                    let m = fmt[1];
                    Some(m)
                } else {
                    None
                };
                match modifier {
                    None => write_out!(0, *mut PyObject, py_ptr),
                    Some(b'!') => {
                        // getargs.c 'O!' consumes TWO varargs: the caller's
                        // `PyTypeObject*` (a VALUE, never written through) then the
                        // `PyObject**` destination. The prior grammar skipped '!'
                        // and let the plain-'O' store write into the type slot,
                        // clobbering the type-object header (UB). Here the type is
                        // only READ, the object is stored into the destination.
                        let type_ptr = outs_slice
                            .first()
                            .copied()
                            .unwrap_or(std::ptr::null_mut())
                            .cast::<PyTypeObject>();
                        let dest = outs_slice
                            .get(1)
                            .copied()
                            .unwrap_or(std::ptr::null_mut())
                            .cast::<*mut PyObject>();
                        let arg_type = unsafe { crate::api::object::Py_TYPE(py_ptr) };
                        if type_ptr.is_null()
                            || unsafe { crate::api::typeobj::PyType_IsSubtype(arg_type, type_ptr) }
                                == 0
                        {
                            unsafe { set_parse_o_bang_type_error(type_ptr, arg_type) };
                            return 0;
                        }
                        if !dest.is_null() {
                            unsafe { *dest = py_ptr };
                        }
                    }
                    Some(b'&') => {
                        // 'O&' consumes a converter fn + a destination address and
                        // calls `convert(arg, addr)`; a 0 return fails the parse.
                        let raw = outs_slice.first().copied().unwrap_or(std::ptr::null_mut());
                        let addr = outs_slice.get(1).copied().unwrap_or(std::ptr::null_mut());
                        if raw.is_null() {
                            unsafe { set_parse_format_error() };
                            return 0;
                        }
                        let convert: ConverterFn =
                            unsafe { std::mem::transmute::<*mut c_void, ConverterFn>(raw) };
                        let status = unsafe { convert(py_ptr, addr) };
                        if status == 0 {
                            // The converter should have set an exception; guarantee
                            // a NULL-return never escapes without one.
                            if unsafe { PyErr_Occurred() }.is_null() {
                                unsafe { set_parse_type_error("argument conversion failed") };
                            }
                            return 0;
                        }
                        if status == 0x20000
                            && !cleanup.retain(ParseResource::Converted(convert, addr))
                        {
                            return 0;
                        }
                    }
                    Some(_) => unreachable!("modifier peek only accepts b'!'/b'&'"),
                }
            }
            'S' | 'U' => {
                let valid = if ch == 'S' {
                    unsafe { crate::api::strings::PyBytes_Check(item.as_ptr()) }
                } else {
                    unsafe { crate::api::strings::PyUnicode_Check(item.as_ptr()) }
                };
                if valid == 0 {
                    unsafe {
                        set_parse_type_error(if ch == 'S' {
                            "argument must be bytes"
                        } else {
                            "argument must be str"
                        })
                    };
                    return 0;
                }
                write_out!(0, *mut PyObject, item.as_ptr());
            }
            'p' => {
                let truth = unsafe { crate::api::object::PyObject_IsTrue(item.as_ptr()) };
                if truth < 0 {
                    return 0;
                }
                write_out!(0, c_int, truth);
            }
            _ => unreachable!(),
        }
        return 1;
    }
    let Some(value) = (unsafe { RuntimeValue::acquire(item.as_ptr()) }) else {
        return 0;
    };
    let bits = value.bits();
    let obj = MoltObject::from_bits(bits);
    // CPython (Python/getargs.c `convertsimple`): a converter that receives
    // the wrong type sets TypeError and the whole parse returns 0. Resolve
    // the current argument to an int-like `i64` (int, bool-as-int-subtype, or
    // a heap BigInt / `__index__` object via the runtime authority); a float
    // is NOT int-like for the integer units. `None` => raise TypeError.
    macro_rules! int_arg {
        () => {{
            match arg_int_like(&obj, bits) {
                Some(v) => v,
                None => {
                    unsafe { set_parse_type_error("argument must be an integer") };
                    return 0;
                }
            }
        }};
    }

    // Signed, range-checked store (CPython 'b'/'h'/'i' raise OverflowError
    // with the exact getargs.c message). The store width is the EXACT C width
    // the caller declared — u8 for b, i16 for h, i32 for i — so the previous
    // 4-byte `c_int` store into a 1/2-byte target (adjacent-memory clobber)
    // is gone.
    macro_rules! int_ranged {
        ($v:expr, $ty:ty, $lo:expr, $hi:expr, $lomsg:literal, $himsg:literal) => {{
            let value = $v;
            if value < $lo {
                unsafe { set_parse_overflow($lomsg) };
                return 0;
            }
            if value > $hi {
                unsafe { set_parse_overflow($himsg) };
                return 0;
            }
            write_out!(0, $ty, value as $ty);
        }};
    }

    match ch {
        // ── Signed, range-checked (OverflowError on out-of-range) ──────────
        // Each stores its EXACT declared C width; the previous `as c_int`
        // 4-byte store into a 1/2-byte b/B/H target (OOB write) is gone.
        'b' => int_ranged!(
            int_arg!(),
            u8,
            0,
            u8::MAX as i64,
            "unsigned byte integer is less than minimum",
            "unsigned byte integer is greater than maximum"
        ),
        'h' => int_ranged!(
            int_arg!(),
            i16,
            i16::MIN as i64,
            i16::MAX as i64,
            "signed short integer is less than minimum",
            "signed short integer is greater than maximum"
        ),
        'i' => int_ranged!(
            int_arg!(),
            i32,
            i32::MIN as i64,
            i32::MAX as i64,
            "signed integer is less than minimum",
            "signed integer is greater than maximum"
        ),
        // 'l': PyLong_AsLong range (OverflowError). `try_from` is width- and
        // platform-correct (c_long is 32-bit on Windows/wasm32, 64-bit on
        // LP64) and clippy-clean (no absurd fixed-width comparison).
        'l' => match c_long::try_from(int_arg!()) {
            Ok(v) => write_out!(0, c_long, v),
            Err(_) => {
                unsafe { set_parse_overflow("Python int too large to convert to C long") };
                return 0;
            }
        },
        // ── Unsigned bitfield (mask low N bits, no range check) ────────────
        'B' => write_out!(0, u8, int_arg!() as u8),
        'H' => write_out!(0, u16, int_arg!() as u16),
        'I' => write_out!(0, u32, int_arg!() as u32),
        'k' => write_out!(0, c_ulong, int_arg!() as c_ulong),
        'L' => write_out!(0, i64, int_arg!()),
        'K' => write_out!(0, u64, int_arg!() as u64),
        'n' => write_out!(0, Py_ssize_t, int_arg!() as Py_ssize_t),
        'd' => {
            let v = match float_like(&obj, bits) {
                Some(v) => v,
                None => {
                    unsafe { set_parse_type_error("argument must be a float") };
                    return 0;
                }
            };
            write_out!(0, f64, v);
        }
        'f' => {
            let v = match float_like(&obj, bits) {
                Some(v) => v as f32,
                None => {
                    unsafe { set_parse_type_error("argument must be a float") };
                    return 0;
                }
            };
            write_out!(0, f32, v);
        }
        'e' => {
            if !matches!(fmt.get(1), Some(b's' | b't')) {
                unsafe { set_parse_format_error() };
                return 0;
            }
            let accepts_bytes = fmt[1] == b't';
            let has_len = fmt.get(2) == Some(&b'#');
            let encoding = outs_slice
                .first()
                .copied()
                .unwrap_or(ptr::null_mut())
                .cast::<c_char>();
            let dest = outs_slice
                .get(1)
                .copied()
                .unwrap_or(ptr::null_mut())
                .cast::<*mut c_char>();
            let len_dest = if has_len {
                outs_slice
                    .get(2)
                    .copied()
                    .unwrap_or(ptr::null_mut())
                    .cast::<Py_ssize_t>()
            } else {
                ptr::null_mut()
            };
            if dest.is_null() {
                unsafe { set_parse_format_error() };
                return 0;
            }
            let py_ptr = item.as_ptr();
            let mut owned_bytes = ptr::null_mut();
            let source = if accepts_bytes && arg_is_bytes(&obj, bits) {
                py_ptr
            } else if arg_is_str(&obj, bits) {
                owned_bytes = unsafe {
                    crate::api::strings::PyUnicode_AsEncodedString(
                        py_ptr,
                        if encoding.is_null() {
                            c"utf-8".as_ptr()
                        } else {
                            encoding
                        },
                        c"strict".as_ptr(),
                    )
                };
                if owned_bytes.is_null() {
                    return 0;
                }
                owned_bytes
            } else {
                unsafe { set_parse_type_error("argument must be str") };
                return 0;
            };
            let mut source_ptr = ptr::null_mut();
            let mut source_len = 0;
            if unsafe {
                crate::api::strings::PyBytes_AsStringAndSize(
                    source,
                    &raw mut source_ptr,
                    &raw mut source_len,
                )
            } != 0
            {
                unsafe { crate::api::refcount::Py_XDECREF(owned_bytes) };
                return 0;
            }
            if !has_len
                && unsafe {
                    std::slice::from_raw_parts(source_ptr.cast::<u8>(), source_len as usize)
                }
                .contains(&0)
            {
                unsafe { crate::api::refcount::Py_XDECREF(owned_bytes) };
                unsafe { set_parse_type_error("encoded string without null bytes") };
                return 0;
            }
            let required = source_len as usize + 1;
            let buffer = unsafe { *dest };
            let output = if buffer.is_null() {
                unsafe { crate::api::memory::PyMem_Malloc(required) }.cast::<c_char>()
            } else {
                if !has_len || len_dest.is_null() || unsafe { *len_dest } < required as Py_ssize_t {
                    unsafe { crate::api::refcount::Py_XDECREF(owned_bytes) };
                    unsafe { set_parse_value_error("encoded string too long") };
                    return 0;
                }
                buffer
            };
            if output.is_null() {
                unsafe { crate::api::refcount::Py_XDECREF(owned_bytes) };
                return 0;
            }
            unsafe {
                ptr::copy_nonoverlapping(source_ptr, output, source_len as usize);
                *output.add(source_len as usize) = 0;
                *dest = output;
                if has_len && !len_dest.is_null() {
                    *len_dest = source_len;
                }
                crate::api::refcount::Py_XDECREF(owned_bytes);
            }
            if buffer.is_null() && !cleanup.retain(ParseResource::Encoded(dest)) {
                return 0;
            }
        }
        's' | 'z' => {
            // CPython 's' requires str; 'z' also accepts None (→ NULL).
            // A non-str non-None object is a TypeError — NOT a fabricated
            // empty string (the prior `molt_str_ptr` theater on an int/list).
            let has_len = fmt.get(1) == Some(&b'#');
            let has_buffer = fmt.get(1) == Some(&b'*');
            if obj.is_none() {
                if ch != 'z' {
                    unsafe { set_parse_type_error("argument must be str, not None") };
                    return 0;
                }
                if has_buffer {
                    let view = outs_slice
                        .first()
                        .copied()
                        .unwrap_or(ptr::null_mut())
                        .cast::<Py_buffer>();
                    if !view.is_null() {
                        unsafe { ptr::write_bytes(view, 0, 1) };
                    }
                    return 1;
                }
                write_out!(0, *const c_char, std::ptr::null());
                if has_len {
                    write_out!(1, Py_ssize_t, 0 as Py_ssize_t);
                }
            } else if arg_is_str(&obj, bits) {
                if has_buffer {
                    let view = outs_slice
                        .first()
                        .copied()
                        .unwrap_or(ptr::null_mut())
                        .cast::<Py_buffer>();
                    let py_ptr = item.as_ptr();
                    if unsafe {
                        crate::api::buffer::PyBuffer_FillInfo(
                            view,
                            py_ptr,
                            molt_str_ptr(bits).cast_mut().cast(),
                            molt_str_len(bits) as Py_ssize_t,
                            1,
                            PyBUF_SIMPLE,
                        )
                    } != 0
                    {
                        return 0;
                    }
                    if !cleanup.retain(ParseResource::Buffer(view)) {
                        return 0;
                    }
                    return 1;
                }
                if !has_len && str_has_interior_nul(bits) {
                    unsafe { set_parse_value_error("embedded null character") };
                    return 0;
                }
                write_out!(0, *const c_char, molt_str_ptr(bits));
                if has_len {
                    write_out!(1, Py_ssize_t, molt_str_len(bits) as Py_ssize_t);
                }
            } else {
                unsafe { set_parse_type_error("argument must be str") };
                return 0;
            }
        }
        'y' => {
            // CPython 'y' requires a bytes-like object (buffer protocol), NOT
            // the str authority. Non-'#' form rejects an interior NUL.
            let has_len = fmt.get(1) == Some(&b'#');
            let has_buffer = fmt.get(1) == Some(&b'*');
            if has_buffer {
                let view = outs_slice
                    .first()
                    .copied()
                    .unwrap_or(ptr::null_mut())
                    .cast::<Py_buffer>();
                let py_ptr = item.as_ptr();
                if unsafe { crate::api::buffer::PyObject_GetBuffer(py_ptr, view, PyBUF_SIMPLE) }
                    != 0
                {
                    return 0;
                }
                if !cleanup.retain(ParseResource::Buffer(view)) {
                    return 0;
                }
                return 1;
            }
            if arg_is_bytes(&obj, bits) {
                if !has_len && bytes_has_interior_nul(bits) {
                    unsafe { set_parse_value_error("embedded null byte") };
                    return 0;
                }
                write_out!(0, *const c_char, molt_bytes_ptr(bits));
                if has_len {
                    write_out!(1, Py_ssize_t, molt_bytes_len(bits) as Py_ssize_t);
                }
            } else {
                unsafe { set_parse_type_error("a bytes-like object is required") };
                return 0;
            }
        }
        'c' => {
            // A bytes/bytearray of length 1 → one C `char`.
            if arg_is_bytes(&obj, bits) && molt_bytes_len(bits) == 1 {
                let p = molt_bytes_ptr(bits);
                let byte = if p.is_null() {
                    0u8
                } else {
                    unsafe { *p.cast::<u8>() }
                };
                write_out!(0, c_char, byte as c_char);
            } else {
                unsafe { set_parse_type_error("argument must be a byte string of length 1") };
                return 0;
            }
        }
        'C' => {
            // A str of length 1 → the code point as a C `int`.
            match str_single_codepoint_if_str(&obj, bits) {
                Some(cp) => write_out!(0, c_int, cp as c_int),
                None => {
                    unsafe {
                        set_parse_type_error("argument must be a unicode character, not a string")
                    };
                    return 0;
                }
            }
        }
        _ => {
            // CPython raises SystemError("bad format string") for an
            // unrecognized format unit. Fail closed — never report success
            // for a format string we cannot honor.
            unsafe { set_parse_format_error() };
            return 0;
        }
    }
    1
}

/// Resolve an int-compatible object (heap BigInt or other) to i64 via the
/// runtime int-conversion authority. Returns `None` for non-integer objects so
/// the caller can raise TypeError. Inline int and bool are handled by the caller
/// before this is reached.
fn int_like_to_i64(bits: u64) -> Option<i64> {
    let h = crate::hooks::hooks_or_stubs();
    let mut out: i64 = 0;
    let rc = unsafe { (h.int_as_i64_checked)(bits, std::ptr::addr_of_mut!(out)) };
    (rc == 0).then_some(out)
}

/// Converter-function pointer for the PyArg `O&` unit
/// (CPython `int (*)(PyObject *, void *)`).
type ConverterFn = unsafe extern "C" fn(*mut PyObject, *mut c_void) -> c_int;

/// Resolve the current argument to an int-like `i64` for the integer format
/// units. Accepts int, bool (an int subtype), and a heap BigInt / `__index__`
/// object via the runtime authority; a float is deliberately NOT int-like
/// (CPython's integer units convert through `PyLong_AsLong`, which rejects a
/// float). `None` => the caller raises TypeError.
fn arg_int_like(obj: &MoltObject, bits: u64) -> Option<i64> {
    if let Some(v) = obj.as_int() {
        Some(v)
    } else if obj.is_bool() {
        Some(obj.as_bool().unwrap_or(false) as i64)
    } else if obj.is_float() {
        None
    } else {
        int_like_to_i64(bits)
    }
}

/// Classify a heap argument handle via the runtime tag hook (`None` for a
/// non-heap immediate). Backs the `s`/`z`/`y`/`S`/`U`/`c`/`C` type checks so a
/// wrong-typed arg raises TypeError instead of fabricating an empty string.
fn arg_heap_tag(obj: &MoltObject, bits: u64) -> Option<u8> {
    obj.is_ptr()
        .then(|| unsafe { (crate::hooks::hooks_or_stubs().classify_heap)(bits) })
}

fn arg_is_str(obj: &MoltObject, bits: u64) -> bool {
    arg_heap_tag(obj, bits) == Some(MoltTypeTag::Str as u8)
}

fn arg_is_bytes(obj: &MoltObject, bits: u64) -> bool {
    arg_heap_tag(obj, bits) == Some(MoltTypeTag::Bytes as u8)
}

/// Null-terminated pointer into a bytes handle's storage (runtime `bytes_data`
/// authority). Returns a null pointer when unavailable.
fn molt_bytes_ptr(bits: u64) -> *const c_char {
    let h = crate::hooks::hooks_or_stubs();
    let mut len = 0usize;
    let ptr = unsafe { (h.bytes_data)(bits, std::ptr::addr_of_mut!(len)) };
    if ptr.is_null() {
        std::ptr::null()
    } else {
        ptr.cast()
    }
}

fn molt_bytes_len(bits: u64) -> usize {
    let h = crate::hooks::hooks_or_stubs();
    let mut len = 0usize;
    unsafe { (h.bytes_data)(bits, std::ptr::addr_of_mut!(len)) };
    len
}

/// True when a str handle's UTF-8 storage contains an interior NUL byte, which
/// CPython rejects for the non-`#` `s`/`z` units (ValueError).
fn str_has_interior_nul(bits: u64) -> bool {
    let h = crate::hooks::hooks_or_stubs();
    let mut len = 0usize;
    let ptr = unsafe { (h.str_data)(bits, std::ptr::addr_of_mut!(len)) };
    !ptr.is_null() && unsafe { std::slice::from_raw_parts(ptr, len) }.contains(&0)
}

fn bytes_has_interior_nul(bits: u64) -> bool {
    let h = crate::hooks::hooks_or_stubs();
    let mut len = 0usize;
    let ptr = unsafe { (h.bytes_data)(bits, std::ptr::addr_of_mut!(len)) };
    !ptr.is_null() && unsafe { std::slice::from_raw_parts(ptr, len) }.contains(&0)
}

/// The single code point of a length-1 str argument (for the `C` unit); `None`
/// unless the arg is a str whose content is exactly one code point.
fn str_single_codepoint_if_str(obj: &MoltObject, bits: u64) -> Option<u32> {
    if !arg_is_str(obj, bits) {
        return None;
    }
    let h = crate::hooks::hooks_or_stubs();
    let mut len = 0usize;
    let ptr = unsafe { (h.str_data)(bits, std::ptr::addr_of_mut!(len)) };
    if ptr.is_null() {
        return None;
    }
    let text = std::str::from_utf8(unsafe { std::slice::from_raw_parts(ptr, len) }).ok()?;
    let mut chars = text.chars();
    let first = chars.next()?;
    chars.next().is_none().then_some(first as u32)
}

/// Set OverflowError for an out-of-range integer format unit (getargs.c range
/// checks).
unsafe fn set_parse_overflow(message: &str) {
    let cmsg = std::ffi::CString::new(message).unwrap_or_default();
    unsafe {
        PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_OverflowError).cast::<crate::abi_types::PyObject>(),
            cmsg.as_ptr(),
        );
    }
}

/// Set ValueError for an embedded-NUL `s`/`z`/`y` argument.
unsafe fn set_parse_value_error(message: &str) {
    let cmsg = std::ffi::CString::new(message).unwrap_or_default();
    unsafe {
        PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_ValueError).cast::<crate::abi_types::PyObject>(),
            cmsg.as_ptr(),
        );
    }
}

/// TypeError for an `O!` type mismatch, shaped like CPython's
/// `converterr(type->tp_name, ...)`.
unsafe fn set_parse_o_bang_type_error(want: *mut PyTypeObject, got: *mut PyTypeObject) {
    fn tp_name(tp: *mut PyTypeObject) -> String {
        if tp.is_null() {
            return "<unknown>".to_string();
        }
        let name = unsafe { (*tp).tp_name };
        if name.is_null() {
            "<unknown>".to_string()
        } else {
            unsafe { CStr::from_ptr(name) }
                .to_string_lossy()
                .into_owned()
        }
    }
    let message = format!("argument must be {}, not {}", tp_name(want), tp_name(got));
    unsafe { set_parse_type_error(&message) };
}

/// Resolve a float-compatible argument to f64 for the `d`/`f` format units.
/// CPython accepts float, int (incl. bool as int subtype), and any object with
/// `__float__`/`__index__`. Returns `None` for genuinely non-numeric objects so
/// the caller can raise TypeError.
fn float_like(obj: &MoltObject, bits: u64) -> Option<f64> {
    if obj.is_float() {
        obj.as_float()
    } else if let Some(x) = obj.as_int() {
        Some(x as f64)
    } else if obj.is_bool() {
        Some(obj.as_bool().unwrap_or(false) as i64 as f64)
    } else {
        int_like_to_i64(bits).map(|x| x as f64)
    }
}

/// Set a TypeError for a PyArg_ParseTuple converter type mismatch, matching
/// CPython's `convertsimple` behavior (Python/getargs.c). A NULL-returning /
/// zero-returning parse MUST leave a set exception.
unsafe fn set_parse_type_error(message: &str) {
    let cmsg = std::ffi::CString::new(message).unwrap_or_default();
    unsafe {
        PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
            cmsg.as_ptr(),
        );
    }
}

/// Set a SystemError for an unrecognized/malformed format unit, matching
/// CPython's handling of a bad format string in PyArg_ParseTuple.
unsafe fn set_parse_format_error() {
    unsafe {
        PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
            c"bad format string passed to PyArg_ParseTuple".as_ptr(),
        );
    }
}

// ─── Helpers — read Molt object internals ────────────────────────────────

/// Get a null-terminated UTF-8 pointer into a Molt str object's storage.
fn molt_str_ptr(bits: u64) -> *const c_char {
    let h = crate::hooks::hooks_or_stubs();
    let mut len: usize = 0;
    let ptr = unsafe { (h.str_data)(bits, std::ptr::addr_of_mut!(len)) };
    if ptr.is_null() {
        c"".as_ptr()
    } else {
        ptr.cast()
    }
}

fn molt_str_len(bits: u64) -> usize {
    let h = crate::hooks::hooks_or_stubs();
    let mut len: usize = 0;
    unsafe { (h.str_data)(bits, std::ptr::addr_of_mut!(len)) };
    len
}
