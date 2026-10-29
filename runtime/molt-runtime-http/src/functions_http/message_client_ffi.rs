use super::*;
use molt_runtime_core::BuiltinExc;

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_parse_header_pairs(data_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let raw = match socketserver_extract_bytes(_py, data_bits, "header data") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let headers = http_parse_header_pairs(raw.as_slice());
        match urllib_http_headers_to_list(_py, &headers) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_message_new() -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = http_message_store_new() else {
            return MoltObject::none().bits();
        };
        MoltObject::from_int(handle).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_message_parse(data_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let raw = match socketserver_extract_bytes(_py, data_bits, "header data") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let headers = http_parse_header_pairs(raw.as_slice());
        let Some(handle) = http_message_store(headers) else {
            return MoltObject::none().bits();
        };
        MoltObject::from_int(handle).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_message_set_raw(
    handle_bits: u64,
    name_bits: u64,
    value_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let handle = match http_message_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(name) = crate::bridge::format_obj_str(_py, obj_from_bits(name_bits)) else {
            return MoltObject::none().bits();
        };
        let Some(value) = crate::bridge::format_obj_str(_py, obj_from_bits(value_bits)) else {
            return MoltObject::none().bits();
        };
        let Some(()) = http_message_with_mut(handle, |message| {
            http_message_push_header(_py, message, name, value);
        }) else {
            return raise_exception::<_>(
                _py,
                BuiltinExc::RuntimeError,
                "http message handle is invalid",
            );
        };
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_message_get(
    handle_bits: u64,
    name_bits: u64,
    default_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let handle = match http_message_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(name) = crate::bridge::format_obj_str(_py, obj_from_bits(name_bits)) else {
            return MoltObject::none().bits();
        };
        let needle = http_message_header_key(name.as_str());
        let Some(out) = http_message_with(handle, |message| {
            let Some(idx) = message
                .index
                .get(&needle)
                .and_then(|positions| positions.last())
            else {
                return Ok(None);
            };
            let ptr = alloc_string(_py, message.headers[*idx].1.as_bytes());
            if ptr.is_null() {
                Err(MoltObject::none().bits())
            } else {
                Ok(Some(MoltObject::from_ptr(ptr).bits()))
            }
        }) else {
            return raise_exception::<_>(
                _py,
                BuiltinExc::RuntimeError,
                "http message handle is invalid",
            );
        };
        match out {
            Ok(Some(bits)) => bits,
            Ok(None) => {
                inc_ref_bits(_py, default_bits);
                default_bits
            }
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_message_get_all(handle_bits: u64, name_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let handle = match http_message_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(name) = crate::bridge::format_obj_str(_py, obj_from_bits(name_bits)) else {
            return MoltObject::none().bits();
        };
        let needle = http_message_header_key(name.as_str());
        let Some(out) = http_message_with(handle, |message| {
            let indices = message
                .index
                .get(&needle)
                .map(Vec::as_slice)
                .unwrap_or_default();
            http_message_values_to_list_from_indices(_py, message, indices)
        }) else {
            return raise_exception::<_>(
                _py,
                BuiltinExc::RuntimeError,
                "http message handle is invalid",
            );
        };
        match out {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_message_items(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let handle = match http_message_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(out) = http_message_with_mut(handle, |message| {
            if let Some(cached_bits) = message.items_list_cache
                && !obj_from_bits(cached_bits).is_none()
            {
                inc_ref_bits(_py, cached_bits);
                return Ok(cached_bits);
            }
            let out_bits = urllib_http_headers_to_list(_py, &message.headers)?;
            inc_ref_bits(_py, out_bits);
            message.items_list_cache = Some(out_bits);
            Ok(out_bits)
        }) else {
            return raise_exception::<_>(
                _py,
                BuiltinExc::RuntimeError,
                "http message handle is invalid",
            );
        };
        match out {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_message_contains(handle_bits: u64, name_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let handle = match http_message_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(name) = crate::bridge::format_obj_str(_py, obj_from_bits(name_bits)) else {
            return MoltObject::none().bits();
        };
        let needle = http_message_header_key(name.as_str());
        let Some(found) = http_message_with(handle, |message| message.index.contains_key(&needle))
        else {
            return raise_exception::<_>(
                _py,
                BuiltinExc::RuntimeError,
                "http message handle is invalid",
            );
        };
        MoltObject::from_bool(found).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_message_len(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let handle = match http_message_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(len_value) = http_message_with(handle, |message| message.headers.len()) else {
            return raise_exception::<_>(
                _py,
                BuiltinExc::RuntimeError,
                "http message handle is invalid",
            );
        };
        let len_i64 = i64::try_from(len_value).unwrap_or(i64::MAX);
        MoltObject::from_int(len_i64).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_message_drop(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let handle = match http_message_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        http_message_drop(_py, handle);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_connection_new(
    host_bits: u64,
    port_bits: u64,
    timeout_bits: u64,
) -> u64 {
    http_client_connection_new_impl(host_bits, port_bits, timeout_bits, false)
}

/// Constructor for `http.client.HTTPSConnection` — same shape as
/// `molt_http_client_connection_new` but marks the connection as TLS so that
/// request execution dispatches over rustls.
#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_connection_new_https(
    host_bits: u64,
    port_bits: u64,
    timeout_bits: u64,
) -> u64 {
    http_client_connection_new_impl(host_bits, port_bits, timeout_bits, true)
}

pub(super) fn http_client_connection_new_impl(
    host_bits: u64,
    port_bits: u64,
    timeout_bits: u64,
    use_tls: bool,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(host) = string_obj_to_owned(obj_from_bits(host_bits)) else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "host must be str");
        };
        if let Err(bits) = http_client_validate_path(_py, &host) {
            return bits;
        }
        let Some(port_value) = to_i64(obj_from_bits(port_bits)) else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "port must be int");
        };
        if !(0..=u16::MAX as i64).contains(&port_value) {
            return raise_exception::<_>(_py, BuiltinExc::ValueError, "port out of range");
        }
        let timeout = if obj_from_bits(timeout_bits).is_none() {
            None
        } else {
            let Some(value) = to_f64(obj_from_bits(timeout_bits))
                .or_else(|| to_i64(obj_from_bits(timeout_bits)).map(|v| v as f64))
            else {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "timeout must be float or None",
                );
            };
            Some(value)
        };
        let Some(handle) = http_client_connection_store(host, port_value as u16, timeout, use_tls)
        else {
            return MoltObject::none().bits();
        };
        MoltObject::from_int(handle).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_connection_putrequest(
    handle_bits: u64,
    method_bits: u64,
    url_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let handle = match http_client_connection_handle_from_bits(_py, handle_bits) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let Some(method) = string_obj_to_owned(obj_from_bits(method_bits)) else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "method must be str");
        };
        let Some(url) = string_obj_to_owned(obj_from_bits(url_bits)) else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "url must be str");
        };
        let url = if url.is_empty() { "/".to_string() } else { url };
        let mut line = match http_client_request_line_bytes(_py, &method, &url) {
            Ok(v) => v,
            Err(e) => return e,
        };
        line.truncate(line.len() - 2);
        let ptr = alloc_bytes(_py, &line);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        let line = unsafe {
            ProtectedRuntimeValue::from_owned_bits(_py, MoltObject::from_ptr(ptr).bits())
        };
        let Some(()) = http_client_connection_with_mut(handle, |conn| {
            conn.url = Some(url);
            conn.wire.clear();
        }) else {
            return raise_exception::<_>(
                _py,
                BuiltinExc::RuntimeError,
                "connection handle is invalid",
            );
        };
        line.into_bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_validate_header_name(header_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(bytes) = (unsafe { bytes_like_slice(header_bits) }) else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "header must be bytes-like");
        };
        match http_client_validate_header_name(_py, bytes) {
            Ok(()) => MoltObject::none().bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_validate_header_value(value_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(bytes) = (unsafe { bytes_like_slice(value_bits) }) else {
            return raise_exception::<_>(
                _py,
                BuiltinExc::TypeError,
                "header value must be bytes-like",
            );
        };
        match http_client_validate_header_value(_py, bytes) {
            Ok(()) => MoltObject::none().bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_connection_send(handle_bits: u64, data_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let handle = match http_client_connection_handle_from_bits(_py, handle_bits) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let data = match socketserver_extract_bytes(_py, data_bits, "data") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let Some(started) = http_client_connection_with_mut(handle, |conn| {
            if conn.url.is_none() {
                return false;
            }
            conn.wire.extend_from_slice(&data);
            true
        }) else {
            return raise_exception::<_>(
                _py,
                BuiltinExc::RuntimeError,
                "connection handle is invalid",
            );
        };
        if !started {
            return raise_exception::<_>(_py, BuiltinExc::OSError, "request not started");
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_connection_getresponse(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let handle = match http_client_connection_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        // Retain pending wire on transport failure and keep the map unlocked
        // across local-dispatch callbacks, TLS and blocking I/O.
        let state = http_client_connection_with_mut(handle, |conn| {
            let Some(url) = conn.url.clone() else {
                return Err("no request pending");
            };
            Ok((
                conn.host.clone(),
                conn.port,
                conn.timeout,
                url,
                conn.wire.clone(),
                conn.use_tls,
            ))
        });
        let (host, port, timeout, url, wire, use_tls) = match state {
            Some(Ok(value)) => value,
            Some(Err(msg)) => return raise_exception::<_>(_py, BuiltinExc::OSError, msg),
            None => {
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::RuntimeError,
                    "connection handle is invalid",
                );
            }
        };
        let response_handle =
            match http_client_execute_wire(_py, &host, port, timeout, &url, &wire, use_tls) {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let _ = http_client_connection_with_mut(handle, http_client_connection_reset_pending);
        MoltObject::from_int(response_handle).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_connection_close(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let handle = match http_client_connection_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(()) =
            http_client_connection_with_mut(handle, http_client_connection_reset_pending)
        else {
            return raise_exception::<_>(
                _py,
                BuiltinExc::RuntimeError,
                "connection handle is invalid",
            );
        };
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_connection_drop(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let handle = match http_client_connection_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        http_client_connection_drop(handle);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_response_read(handle_bits: u64, size_bits: u64) -> u64 {
    molt_urllib_request_response_read(handle_bits, size_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_response_close(handle_bits: u64) -> u64 {
    molt_urllib_request_response_close(handle_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_response_drop(handle_bits: u64) -> u64 {
    molt_urllib_request_response_drop(handle_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_response_getstatus(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let handle = match http_client_response_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(code) = urllib_response_with(handle, |resp| resp.code) else {
            return raise_exception::<_>(
                _py,
                BuiltinExc::RuntimeError,
                "response handle is invalid",
            );
        };
        let status = if code < 0 { 0 } else { code };
        MoltObject::from_int(status).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_response_getreason(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let handle = match http_client_response_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(out) = urllib_response_with(handle, |resp| {
            let ptr = alloc_string(_py, resp.reason.as_bytes());
            if ptr.is_null() {
                Err(MoltObject::none().bits())
            } else {
                Ok(MoltObject::from_ptr(ptr).bits())
            }
        }) else {
            return raise_exception::<_>(
                _py,
                BuiltinExc::RuntimeError,
                "response handle is invalid",
            );
        };
        match out {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_response_getheader(
    handle_bits: u64,
    name_bits: u64,
    default_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let handle = match http_client_response_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "header name must be str");
        };
        let Some(out) = urllib_response_with(handle, |resp| {
            let Some(joined) = urllib_response_joined_header(resp, name.as_str()) else {
                return Ok(None);
            };
            let ptr = alloc_string(_py, joined.as_bytes());
            if ptr.is_null() {
                Err(MoltObject::none().bits())
            } else {
                Ok(Some(MoltObject::from_ptr(ptr).bits()))
            }
        }) else {
            return raise_exception::<_>(
                _py,
                BuiltinExc::RuntimeError,
                "response handle is invalid",
            );
        };
        match out {
            Ok(Some(bits)) => bits,
            Ok(None) => {
                inc_ref_bits(_py, default_bits);
                default_bits
            }
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_response_getheaders(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let handle = match http_client_response_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(out) =
            urllib_response_with_mut(handle, |resp| urllib_response_headers_list_bits(_py, resp))
        else {
            return raise_exception::<_>(
                _py,
                BuiltinExc::RuntimeError,
                "response handle is invalid",
            );
        };
        match out {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_response_message(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let handle = match http_client_response_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        urllib_response_message_bits(_py, handle)
    })
}
