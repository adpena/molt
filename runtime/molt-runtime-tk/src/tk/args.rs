use super::*;

pub(super) fn clear_last_error(handle: i64) {
    let mut registry = tk_registry().lock().unwrap();
    if let Some(app) = registry.apps.get_mut(&handle) {
        app.last_error = None;
    }
}

pub(super) fn set_last_error(handle: i64, message: impl Into<String>) {
    let mut registry = tk_registry().lock().unwrap();
    if let Some(app) = registry.apps.get_mut(&handle) {
        app.last_error = Some(message.into());
    }
}

pub(super) fn raise_tcl_for_handle(py: &PyToken, handle: i64, message: impl Into<String>) -> u64 {
    let message = message.into();
    set_last_error(handle, message.clone());
    raise_tcl_error(py, &message)
}

pub(super) fn get_string_arg(
    py: &PyToken,
    handle: i64,
    bits: u64,
    label: &str,
) -> Result<String, u64> {
    string_obj_to_owned(obj_from_bits(bits)).ok_or_else(|| {
        raise_tcl_for_handle(
            py,
            handle,
            format!("{label} must be str in tkinter command"),
        )
    })
}

pub(super) fn get_string_arg_allow_none(
    py: &PyToken,
    handle: i64,
    bits: u64,
    label: &str,
) -> Result<String, u64> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return Ok(String::new());
    }
    get_string_arg(py, handle, bits, label)
}

pub(super) fn get_text_arg(
    py: &PyToken,
    handle: i64,
    bits: u64,
    label: &str,
) -> Result<String, u64> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return Ok(String::new());
    }
    if let Some(text) = string_obj_to_owned(obj) {
        return Ok(text);
    }
    if let Some(value) = to_i64(obj) {
        return Ok(value.to_string());
    }
    if let Some(value) = to_f64(obj) {
        return Ok(value.to_string());
    }
    Err(raise_tcl_for_handle(
        py,
        handle,
        format!("{label} must be str/int/float in tkinter command"),
    ))
}

pub(super) fn parse_optional_i64_arg(
    py: &PyToken,
    handle: i64,
    bits: u64,
    label: &str,
) -> Result<Option<i64>, u64> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return Ok(None);
    }
    let Some(value) = to_i64(obj) else {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            format!("{label} must be an integer"),
        ));
    };
    Ok(Some(value))
}

pub(super) fn parse_optional_f64_arg(
    py: &PyToken,
    handle: i64,
    bits: u64,
    label: &str,
) -> Result<Option<f64>, u64> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return Ok(None);
    }
    let Some(value) = to_f64(obj) else {
        return Err(raise_tcl_for_handle(
            py,
            handle,
            format!("{label} must be a real number"),
        ));
    };
    Ok(Some(value))
}
