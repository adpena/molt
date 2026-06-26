use super::*;

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
        let name = crate::bridge::format_obj_str(_py, obj_from_bits(name_bits));
        let value = crate::bridge::format_obj_str(_py, obj_from_bits(value_bits));
        let Some(()) = http_message_with_mut(handle, |message| {
            http_message_push_header(_py, message, name, value);
        }) else {
            return raise_exception::<_>(_py, "RuntimeError", "http message handle is invalid");
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
        let needle = http_message_header_key(
            crate::bridge::format_obj_str(_py, obj_from_bits(name_bits)).as_str(),
        );
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
            return raise_exception::<_>(_py, "RuntimeError", "http message handle is invalid");
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
        let needle = http_message_header_key(
            crate::bridge::format_obj_str(_py, obj_from_bits(name_bits)).as_str(),
        );
        let Some(out) = http_message_with(handle, |message| {
            let indices = message
                .index
                .get(&needle)
                .map(Vec::as_slice)
                .unwrap_or_default();
            http_message_values_to_list_from_indices(_py, message, indices)
        }) else {
            return raise_exception::<_>(_py, "RuntimeError", "http message handle is invalid");
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
            return raise_exception::<_>(_py, "RuntimeError", "http message handle is invalid");
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
        let needle = http_message_header_key(
            crate::bridge::format_obj_str(_py, obj_from_bits(name_bits)).as_str(),
        );
        let Some(found) = http_message_with(handle, |message| message.index.contains_key(&needle))
        else {
            return raise_exception::<_>(_py, "RuntimeError", "http message handle is invalid");
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
            return raise_exception::<_>(_py, "RuntimeError", "http message handle is invalid");
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
            return raise_exception::<_>(_py, "TypeError", "host must be str");
        };
        let Some(port_value) = to_i64(obj_from_bits(port_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "port must be int");
        };
        if !(0..=u16::MAX as i64).contains(&port_value) {
            return raise_exception::<_>(_py, "ValueError", "port out of range");
        }
        let timeout = if obj_from_bits(timeout_bits).is_none() {
            None
        } else {
            let Some(value) = to_f64(obj_from_bits(timeout_bits))
                .or_else(|| to_i64(obj_from_bits(timeout_bits)).map(|v| v as f64))
            else {
                return raise_exception::<_>(_py, "TypeError", "timeout must be float or None");
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
    skip_host_bits: u64,
    skip_accept_encoding_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let handle = match http_client_connection_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(method) = string_obj_to_owned(obj_from_bits(method_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "method must be str");
        };
        let Some(url) = string_obj_to_owned(obj_from_bits(url_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "url must be str");
        };
        let skip_host = is_truthy(_py, obj_from_bits(skip_host_bits));
        let skip_accept_encoding = is_truthy(_py, obj_from_bits(skip_accept_encoding_bits));
        let Some(buffer) = http_client_connection_with_mut(handle, |conn| {
            conn.method = Some(method.clone());
            conn.url = Some(url.clone());
            conn.headers.clear();
            conn.body.clear();
            conn.buffer.clear();
            conn.skip_host = skip_host;
            conn.skip_accept_encoding = skip_accept_encoding;
            conn.buffer
                .push(format!("{method} {url} HTTP/1.1\r\n").into_bytes());
            conn.buffer.clone()
        }) else {
            return raise_exception::<_>(_py, "RuntimeError", "connection handle is invalid");
        };
        http_client_alloc_buffer_list(_py, &buffer)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_connection_putheader(
    handle_bits: u64,
    header_bits: u64,
    value_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let handle = match http_client_connection_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(header) = string_obj_to_owned(obj_from_bits(header_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "header must be str");
        };
        let Some(value) = string_obj_to_owned(obj_from_bits(value_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "header value must be str");
        };
        let state = http_client_connection_with_mut(handle, |conn| {
            if conn.method.is_none() || conn.url.is_none() {
                return Err("request not started");
            }
            conn.headers.push((header, value));
            Ok(conn.buffer.clone())
        });
        match state {
            Some(Ok(buffer)) => http_client_alloc_buffer_list(_py, &buffer),
            Some(Err(msg)) => raise_exception::<_>(_py, "OSError", msg),
            None => raise_exception::<_>(_py, "RuntimeError", "connection handle is invalid"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_connection_endheaders(handle_bits: u64, body_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let handle = match http_client_connection_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let body = if obj_from_bits(body_bits).is_none() {
            None
        } else {
            match socketserver_extract_bytes(_py, body_bits, "message_body") {
                Ok(value) => Some(value),
                Err(bits) => return bits,
            }
        };
        let state = http_client_connection_with_mut(handle, |conn| {
            if conn.method.is_none() || conn.url.is_none() {
                return Err("request not started");
            }
            if conn
                .buffer
                .last()
                .is_none_or(|line| line.as_slice() != b"\r\n")
            {
                http_client_apply_default_headers(
                    &mut conn.headers,
                    conn.host.as_str(),
                    conn.port,
                    conn.skip_host,
                    conn.skip_accept_encoding,
                );
                for (name, value) in &conn.headers {
                    conn.buffer
                        .push(format!("{name}: {value}\r\n").into_bytes());
                }
                conn.buffer.push(b"\r\n".to_vec());
            }
            if let Some(chunk) = body.as_ref() {
                conn.body.extend_from_slice(chunk.as_slice());
                conn.buffer.push(chunk.clone());
            }
            Ok(conn.buffer.clone())
        });
        match state {
            Some(Ok(buffer)) => http_client_alloc_buffer_list(_py, &buffer),
            Some(Err(msg)) => raise_exception::<_>(_py, "OSError", msg),
            None => raise_exception::<_>(_py, "RuntimeError", "connection handle is invalid"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_connection_send(handle_bits: u64, data_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let handle = match http_client_connection_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let data = match socketserver_extract_bytes(_py, data_bits, "data") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let state = http_client_connection_with_mut(handle, |conn| {
            if conn.method.is_none() || conn.url.is_none() {
                return Err("request not started");
            }
            conn.body.extend_from_slice(data.as_slice());
            conn.buffer.push(data);
            Ok(conn.buffer.clone())
        });
        match state {
            Some(Ok(buffer)) => http_client_alloc_buffer_list(_py, &buffer),
            Some(Err(msg)) => raise_exception::<_>(_py, "OSError", msg),
            None => raise_exception::<_>(_py, "RuntimeError", "connection handle is invalid"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_connection_request(
    handle_bits: u64,
    method_bits: u64,
    url_bits: u64,
    body_bits: u64,
    headers_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let handle = match http_client_connection_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(method) = string_obj_to_owned(obj_from_bits(method_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "method must be str");
        };
        let Some(url) = string_obj_to_owned(obj_from_bits(url_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "url must be str");
        };
        let mut headers = if obj_from_bits(headers_bits).is_none() {
            Vec::new()
        } else {
            match urllib_http_extract_headers_mapping(_py, headers_bits) {
                Ok(value) => value,
                Err(bits) => return bits,
            }
        };
        let body = if obj_from_bits(body_bits).is_none() {
            None
        } else {
            match socketserver_extract_bytes(_py, body_bits, "body") {
                Ok(value) => Some(value),
                Err(bits) => return bits,
            }
        };
        if let Some(payload) = body.as_ref()
            && !headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        {
            headers.push(("Content-Length".to_string(), payload.len().to_string()));
        }
        let state = http_client_connection_with_mut(handle, |conn| {
            conn.method = Some(method.clone());
            conn.url = Some(url.clone());
            conn.headers = headers;
            conn.body = body.unwrap_or_default();
            conn.skip_host = false;
            conn.skip_accept_encoding = true;
            conn.buffer.clear();
            conn.buffer
                .push(format!("{method} {url} HTTP/1.1\r\n").into_bytes());
            conn.buffer.clone()
        });
        match state {
            Some(buffer) => http_client_alloc_buffer_list(_py, &buffer),
            None => raise_exception::<_>(_py, "RuntimeError", "connection handle is invalid"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_connection_getresponse(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let handle = match http_client_connection_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let state = http_client_connection_with_mut(handle, |conn| {
            let Some(method) = conn.method.clone() else {
                return Err("no request pending");
            };
            let Some(url) = conn.url.clone() else {
                return Err("no request pending");
            };
            http_client_apply_default_headers(
                &mut conn.headers,
                conn.host.as_str(),
                conn.port,
                conn.skip_host,
                conn.skip_accept_encoding,
            );
            Ok((
                conn.host.clone(),
                conn.port,
                conn.timeout,
                method,
                url,
                conn.headers.clone(),
                conn.body.clone(),
                conn.use_tls,
            ))
        });
        let (host, port, timeout, method, url, headers, body, use_tls) = match state {
            Some(Ok(value)) => value,
            Some(Err(msg)) => return raise_exception::<_>(_py, "OSError", msg),
            None => {
                return raise_exception::<_>(_py, "RuntimeError", "connection handle is invalid");
            }
        };
        let response_handle = match http_client_execute_request(
            _py,
            HttpClientExecuteInput {
                host,
                port,
                timeout,
                method,
                url,
                headers,
                body,
                skip_host: true,
                skip_accept_encoding: true,
                use_tls,
            },
        ) {
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
            return raise_exception::<_>(_py, "RuntimeError", "connection handle is invalid");
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
pub extern "C" fn molt_http_client_connection_get_buffer(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let handle = match http_client_connection_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(buffer) = http_client_connection_with(handle, |conn| conn.buffer.clone()) else {
            return raise_exception::<_>(_py, "RuntimeError", "connection handle is invalid");
        };
        http_client_alloc_buffer_list(_py, &buffer)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_execute(
    host_bits: u64,
    port_bits: u64,
    timeout_bits: u64,
    method_bits: u64,
    url_bits: u64,
    headers_bits: u64,
    body_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(host) = string_obj_to_owned(obj_from_bits(host_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "host must be str");
        };
        let Some(port_value) = to_i64(obj_from_bits(port_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "port must be int");
        };
        if !(0..=u16::MAX as i64).contains(&port_value) {
            return raise_exception::<_>(_py, "ValueError", "port out of range");
        }
        let port = port_value as u16;
        let timeout = if obj_from_bits(timeout_bits).is_none() {
            None
        } else {
            let Some(value) = to_f64(obj_from_bits(timeout_bits)) else {
                return raise_exception::<_>(_py, "TypeError", "timeout must be float or None");
            };
            Some(value)
        };
        let Some(method) = string_obj_to_owned(obj_from_bits(method_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "method must be str");
        };
        let Some(url) = string_obj_to_owned(obj_from_bits(url_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "url must be str");
        };
        let headers = match http_client_extract_headers(_py, headers_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let body = if obj_from_bits(body_bits).is_none() {
            Vec::new()
        } else {
            match socketserver_extract_bytes(_py, body_bits, "body") {
                Ok(value) => value,
                Err(bits) => return bits,
            }
        };
        match http_client_execute_request(
            _py,
            HttpClientExecuteInput {
                host,
                port,
                timeout,
                method,
                url,
                headers,
                body,
                skip_host: true,
                skip_accept_encoding: true,
                use_tls: false,
            },
        ) {
            Ok(handle) => MoltObject::from_int(handle).bits(),
            Err(bits) => bits,
        }
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
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
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
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
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
            return raise_exception::<_>(_py, "TypeError", "header name must be str");
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
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
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
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
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
