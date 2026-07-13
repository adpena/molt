use molt_cpython_abi::abi_types::PyObject;

#[allow(dead_code)]
pub mod fake_complex;
#[allow(dead_code)]
pub mod fake_foreign;
pub mod fake_strings;

/// Consume the exact pending exception and render its normalized instance.
/// Tests use the public ownership API rather than reviving the deleted
/// text-only error side channel.
#[allow(dead_code)]
pub fn take_current_error_text() -> Option<String> {
    let error = molt_cpython_abi::api::errors::take_current_error()?;
    if error.value.is_null() {
        return None;
    }
    unsafe {
        let rendered = molt_cpython_abi::api::typeobj::PyObject_Str(error.value);
        if rendered.is_null() {
            molt_cpython_abi::api::errors::PyErr_Clear();
            return None;
        }
        let mut len = 0;
        let data = molt_cpython_abi::api::strings::PyUnicode_AsUTF8AndSize(rendered, &raw mut len);
        let text = (!data.is_null() && len >= 0).then(|| {
            String::from_utf8_lossy(std::slice::from_raw_parts(data.cast::<u8>(), len as usize))
                .into_owned()
        });
        molt_cpython_abi::api::refcount::Py_DECREF(rendered.cast::<PyObject>());
        molt_cpython_abi::api::errors::PyErr_Clear();
        text
    }
}
