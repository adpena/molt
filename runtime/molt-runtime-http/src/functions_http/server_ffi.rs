use super::*;

#[unsafe(no_mangle)]
pub extern "C" fn molt_socketserver_register(server_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let mut runtime = socketserver_runtime()
            .lock()
            .expect("socketserver runtime poisoned");
        runtime.pending_by_server.entry(server_bits).or_default();
        runtime.closed_servers.remove(&server_bits);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_socketserver_unregister(server_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let mut runtime = socketserver_runtime()
            .lock()
            .expect("socketserver runtime poisoned");
        runtime.closed_servers.insert(server_bits);
        if let Some(mut ids) = runtime.pending_by_server.remove(&server_bits) {
            while let Some(request_id) = ids.pop_front() {
                runtime.pending_requests.remove(&request_id);
                runtime.request_server.remove(&request_id);
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_socketserver_dispatch_begin(server_bits: u64, request_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let request = match socketserver_extract_bytes(_py, request_bits, "request payload") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let mut runtime = socketserver_runtime()
            .lock()
            .expect("socketserver runtime poisoned");
        if runtime.closed_servers.contains(&server_bits) {
            return raise_exception::<_>(_py, "OSError", "server closed");
        }
        let request_id = runtime.next_request_id;
        runtime.next_request_id = runtime.next_request_id.saturating_add(1);
        runtime.pending_requests.insert(
            request_id,
            MoltSocketServerPending {
                request,
                response: None,
            },
        );
        runtime.request_server.insert(request_id, server_bits);
        runtime
            .pending_by_server
            .entry(server_bits)
            .or_default()
            .push_back(request_id);
        MoltObject::from_int(request_id as i64).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_socketserver_dispatch_poll(server_bits: u64, request_id_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let request_id = match socketserver_extract_request_id(_py, request_id_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let mut runtime = socketserver_runtime()
            .lock()
            .expect("socketserver runtime poisoned");
        let Some(owner) = runtime.request_server.get(&request_id).copied() else {
            return MoltObject::none().bits();
        };
        if owner != server_bits {
            return raise_exception::<_>(_py, "RuntimeError", "request id owner mismatch");
        }
        let Some(pending) = runtime.pending_requests.get_mut(&request_id) else {
            runtime.request_server.remove(&request_id);
            return MoltObject::none().bits();
        };
        let Some(response) = pending.response.take() else {
            return MoltObject::none().bits();
        };
        runtime.pending_requests.remove(&request_id);
        runtime.request_server.remove(&request_id);
        let ptr = crate::bridge::alloc_bytes(_py, &response);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_socketserver_dispatch_cancel(server_bits: u64, request_id_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let request_id = match socketserver_extract_request_id(_py, request_id_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let mut runtime = socketserver_runtime()
            .lock()
            .expect("socketserver runtime poisoned");
        if let Some(queue) = runtime.pending_by_server.get_mut(&server_bits) {
            queue.retain(|candidate| *candidate != request_id);
        }
        runtime.pending_requests.remove(&request_id);
        runtime.request_server.remove(&request_id);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_socketserver_get_request_poll(server_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let mut runtime = socketserver_runtime()
            .lock()
            .expect("socketserver runtime poisoned");
        if runtime.closed_servers.contains(&server_bits) {
            return raise_exception::<_>(_py, "OSError", "server closed");
        }
        let Some(queue) = runtime.pending_by_server.get_mut(&server_bits) else {
            return MoltObject::none().bits();
        };
        let Some(request_id) = queue.pop_front() else {
            return MoltObject::none().bits();
        };
        let Some(pending) = runtime.pending_requests.get(&request_id) else {
            runtime.request_server.remove(&request_id);
            return MoltObject::none().bits();
        };
        let request_ptr = crate::bridge::alloc_bytes(_py, &pending.request);
        if request_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let request_bits = MoltObject::from_ptr(request_ptr).bits();
        let request_id_bits = MoltObject::from_int(request_id as i64).bits();
        let tuple_ptr = alloc_tuple(_py, &[request_id_bits, request_bits]);
        dec_ref_bits(_py, request_bits);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_socketserver_set_response(
    server_bits: u64,
    request_id_bits: u64,
    response_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let request_id = match socketserver_extract_request_id(_py, request_id_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let response = match socketserver_extract_bytes(_py, response_bits, "response payload") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let mut runtime = socketserver_runtime()
            .lock()
            .expect("socketserver runtime poisoned");
        let Some(owner) = runtime.request_server.get(&request_id).copied() else {
            return MoltObject::none().bits();
        };
        if owner != server_bits {
            return raise_exception::<_>(_py, "RuntimeError", "request id owner mismatch");
        }
        let Some(pending) = runtime.pending_requests.get_mut(&request_id) else {
            runtime.request_server.remove(&request_id);
            return MoltObject::none().bits();
        };
        pending.response = Some(response);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_socketserver_serve_forever(
    server_bits: u64,
    poll_interval_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let poll_interval = to_f64(obj_from_bits(poll_interval_bits))
            .unwrap_or(0.5)
            .max(0.0);
        loop {
            let shutdown_requested =
                match urllib_attr_truthy(_py, server_bits, b"_molt_shutdown_request") {
                    Ok(value) => value,
                    Err(bits) => return bits,
                };
            if shutdown_requested {
                break;
            }
            let Some(name_bits) = attr_name_bits_from_bytes(_py, b"handle_request") else {
                return MoltObject::none().bits();
            };
            let missing = missing_bits(_py);
            let handle_request_bits = molt_getattr_builtin(server_bits, name_bits, missing);
            dec_ref_bits(_py, name_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if handle_request_bits == missing {
                return raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    "socketserver server is missing handle_request",
                );
            }
            let _ = unsafe { call_callable0(_py, handle_request_bits) };
            dec_ref_bits(_py, handle_request_bits);
            if !exception_pending(_py) {
                if let Err(bits) = socketserver_call_service_actions(_py, server_bits) {
                    return bits;
                }
                continue;
            }
            let kind = urllib_request_pending_exception_kind_name(_py).unwrap_or_default();
            if kind == "TimeoutError" {
                clear_exception(_py);
                if poll_interval > 0.0 {
                    std::thread::sleep(Duration::from_secs_f64(poll_interval.min(0.05)));
                }
                if let Err(bits) = socketserver_call_service_actions(_py, server_bits) {
                    return bits;
                }
                continue;
            }
            if kind == "OSError" {
                let shutdown_now =
                    match urllib_attr_truthy(_py, server_bits, b"_molt_shutdown_request") {
                        Ok(value) => value,
                        Err(bits) => return bits,
                    };
                let closed_now = match urllib_attr_truthy(_py, server_bits, b"_closed") {
                    Ok(value) => value,
                    Err(bits) => return bits,
                };
                if shutdown_now || closed_now {
                    clear_exception(_py);
                    break;
                }
            }
            let handled_kind = !kind.is_empty()
                && kind != "SystemExit"
                && kind != "KeyboardInterrupt"
                && kind != "GeneratorExit"
                && kind != "BaseExceptionGroup";
            if handled_kind {
                clear_exception(_py);
                let Some(name_bits) = attr_name_bits_from_bytes(_py, b"handle_error") else {
                    return MoltObject::none().bits();
                };
                let missing = missing_bits(_py);
                let handle_error_bits = molt_getattr_builtin(server_bits, name_bits, missing);
                dec_ref_bits(_py, name_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                if handle_error_bits != missing {
                    let none_bits = MoltObject::none().bits();
                    let _ = unsafe { call_callable2(_py, handle_error_bits, none_bits, none_bits) };
                    dec_ref_bits(_py, handle_error_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                }
                if let Err(bits) = socketserver_call_service_actions(_py, server_bits) {
                    return bits;
                }
                continue;
            }
            return MoltObject::none().bits();
        }
        if let Err(bits) = socketserver_call_service_actions(_py, server_bits) {
            return bits;
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_socketserver_handle_request(server_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(get_request_name_bits) = attr_name_bits_from_bytes(_py, b"get_request") else {
            return MoltObject::none().bits();
        };
        let missing = missing_bits(_py);
        let get_request_bits = molt_getattr_builtin(server_bits, get_request_name_bits, missing);
        dec_ref_bits(_py, get_request_name_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if get_request_bits == missing || !molt_is_callable(get_request_bits) {
            if get_request_bits != missing {
                dec_ref_bits(_py, get_request_bits);
            }
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "socketserver server is missing get_request",
            );
        }
        let request_tuple_bits = unsafe { call_callable0(_py, get_request_bits) };
        dec_ref_bits(_py, get_request_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let (request_bits, client_address_bits, request_id) =
            match socketserver_extract_handle_request_tuple(_py, request_tuple_bits) {
                Ok(parts) => parts,
                Err(bits) => {
                    dec_ref_bits(_py, request_tuple_bits);
                    return bits;
                }
            };

        let mut deferred_exception_bits: Option<u64> = None;
        let mut should_process = true;

        if let Some(verify_request_bits) = match attr_optional(_py, server_bits, b"verify_request")
        {
            Ok(bits) => bits,
            Err(bits) => {
                dec_ref_bits(_py, request_tuple_bits);
                return bits;
            }
        } {
            if !molt_is_callable(verify_request_bits) {
                dec_ref_bits(_py, verify_request_bits);
                dec_ref_bits(_py, request_tuple_bits);
                return raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    "socketserver server verify_request must be callable",
                );
            }
            let verify_bits = unsafe {
                call_callable2(_py, verify_request_bits, request_bits, client_address_bits)
            };
            dec_ref_bits(_py, verify_request_bits);
            if exception_pending(_py) {
                dec_ref_bits(_py, request_tuple_bits);
                return MoltObject::none().bits();
            }
            should_process = is_truthy(_py, obj_from_bits(verify_bits));
            dec_ref_bits(_py, verify_bits);
        }

        if should_process {
            let Some(process_request_name_bits) =
                attr_name_bits_from_bytes(_py, b"process_request")
            else {
                dec_ref_bits(_py, request_tuple_bits);
                return MoltObject::none().bits();
            };
            let process_request_bits =
                molt_getattr_builtin(server_bits, process_request_name_bits, missing);
            dec_ref_bits(_py, process_request_name_bits);
            if exception_pending(_py) {
                dec_ref_bits(_py, request_tuple_bits);
                return MoltObject::none().bits();
            }
            if process_request_bits == missing || !molt_is_callable(process_request_bits) {
                if process_request_bits != missing {
                    dec_ref_bits(_py, process_request_bits);
                }
                dec_ref_bits(_py, request_tuple_bits);
                return raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    "socketserver server is missing process_request",
                );
            }
            let _ = unsafe {
                call_callable2(_py, process_request_bits, request_bits, client_address_bits)
            };
            dec_ref_bits(_py, process_request_bits);
            if exception_pending(_py) {
                let kind = urllib_request_pending_exception_kind_name(_py).unwrap_or_default();
                let handled_kind = !kind.is_empty()
                    && kind != "SystemExit"
                    && kind != "KeyboardInterrupt"
                    && kind != "GeneratorExit"
                    && kind != "BaseExceptionGroup";
                if handled_kind {
                    clear_exception(_py);
                    let Some(handle_error_name_bits) =
                        attr_name_bits_from_bytes(_py, b"handle_error")
                    else {
                        dec_ref_bits(_py, request_tuple_bits);
                        return MoltObject::none().bits();
                    };
                    let handle_error_bits =
                        molt_getattr_builtin(server_bits, handle_error_name_bits, missing);
                    dec_ref_bits(_py, handle_error_name_bits);
                    if exception_pending(_py) {
                        dec_ref_bits(_py, request_tuple_bits);
                        return MoltObject::none().bits();
                    }
                    if handle_error_bits != missing {
                        let _ = unsafe {
                            call_callable2(
                                _py,
                                handle_error_bits,
                                request_bits,
                                client_address_bits,
                            )
                        };
                        dec_ref_bits(_py, handle_error_bits);
                        if exception_pending(_py) {
                            let exc_bits = molt_exception_last();
                            clear_exception(_py);
                            deferred_exception_bits = Some(exc_bits);
                        }
                    }
                } else {
                    let exc_bits = molt_exception_last();
                    clear_exception(_py);
                    deferred_exception_bits = Some(exc_bits);
                }
            }
        }

        let Some(close_request_name_bits) = attr_name_bits_from_bytes(_py, b"close_request") else {
            if let Some(exc_bits) = deferred_exception_bits.take() {
                dec_ref_bits(_py, exc_bits);
            }
            dec_ref_bits(_py, request_tuple_bits);
            return MoltObject::none().bits();
        };
        let close_request_bits =
            molt_getattr_builtin(server_bits, close_request_name_bits, missing);
        dec_ref_bits(_py, close_request_name_bits);
        if exception_pending(_py) {
            if let Some(exc_bits) = deferred_exception_bits.take() {
                dec_ref_bits(_py, exc_bits);
            }
            dec_ref_bits(_py, request_tuple_bits);
            return MoltObject::none().bits();
        }
        if close_request_bits != missing && molt_is_callable(close_request_bits) {
            let _ = unsafe { call_callable1(_py, close_request_bits, request_bits) };
            dec_ref_bits(_py, close_request_bits);
            if exception_pending(_py) {
                if let Some(exc_bits) = deferred_exception_bits.take() {
                    dec_ref_bits(_py, exc_bits);
                }
                dec_ref_bits(_py, request_tuple_bits);
                return MoltObject::none().bits();
            }
        } else if close_request_bits != missing {
            dec_ref_bits(_py, close_request_bits);
        }

        if request_id >= 0 {
            let Some(response_bytes_name_bits) = attr_name_bits_from_bytes(_py, b"response_bytes")
            else {
                if let Some(exc_bits) = deferred_exception_bits.take() {
                    let out = crate::bridge::molt_raise(exc_bits);
                    dec_ref_bits(_py, request_tuple_bits);
                    return out;
                }
                dec_ref_bits(_py, request_tuple_bits);
                return MoltObject::none().bits();
            };
            let response_bytes_bits =
                molt_getattr_builtin(request_bits, response_bytes_name_bits, missing);
            dec_ref_bits(_py, response_bytes_name_bits);
            if exception_pending(_py) {
                if let Some(exc_bits) = deferred_exception_bits.take() {
                    dec_ref_bits(_py, exc_bits);
                }
                dec_ref_bits(_py, request_tuple_bits);
                return MoltObject::none().bits();
            }
            if response_bytes_bits != missing && molt_is_callable(response_bytes_bits) {
                let response_bits = unsafe { call_callable0(_py, response_bytes_bits) };
                dec_ref_bits(_py, response_bytes_bits);
                if exception_pending(_py) {
                    if let Some(exc_bits) = deferred_exception_bits.take() {
                        dec_ref_bits(_py, exc_bits);
                    }
                    dec_ref_bits(_py, request_tuple_bits);
                    return MoltObject::none().bits();
                }
                let response =
                    match socketserver_extract_bytes(_py, response_bits, "response payload") {
                        Ok(value) => value,
                        Err(bits) => {
                            dec_ref_bits(_py, response_bits);
                            if let Some(exc_bits) = deferred_exception_bits.take() {
                                dec_ref_bits(_py, exc_bits);
                            }
                            dec_ref_bits(_py, request_tuple_bits);
                            return bits;
                        }
                    };
                dec_ref_bits(_py, response_bits);
                let mut runtime = socketserver_runtime()
                    .lock()
                    .expect("socketserver runtime poisoned");
                let request_id_u64 = request_id as u64;
                let Some(owner) = runtime.request_server.get(&request_id_u64).copied() else {
                    dec_ref_bits(_py, request_tuple_bits);
                    if let Some(exc_bits) = deferred_exception_bits.take() {
                        return crate::bridge::molt_raise(exc_bits);
                    }
                    return MoltObject::none().bits();
                };
                if owner != server_bits {
                    if let Some(exc_bits) = deferred_exception_bits.take() {
                        dec_ref_bits(_py, exc_bits);
                    }
                    dec_ref_bits(_py, request_tuple_bits);
                    return raise_exception::<_>(_py, "RuntimeError", "request id owner mismatch");
                }
                let Some(pending) = runtime.pending_requests.get_mut(&request_id_u64) else {
                    runtime.request_server.remove(&request_id_u64);
                    dec_ref_bits(_py, request_tuple_bits);
                    if let Some(exc_bits) = deferred_exception_bits.take() {
                        return crate::bridge::molt_raise(exc_bits);
                    }
                    return MoltObject::none().bits();
                };
                pending.response = Some(response);
            } else if response_bytes_bits != missing {
                dec_ref_bits(_py, response_bytes_bits);
            }
        }

        dec_ref_bits(_py, request_tuple_bits);
        if let Some(exc_bits) = deferred_exception_bits.take() {
            return crate::bridge::molt_raise(exc_bits);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_socketserver_shutdown(server_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if !urllib_request_set_attr(
            _py,
            server_bits,
            b"_molt_shutdown_request",
            MoltObject::from_bool(true).bits(),
        ) || !urllib_request_set_attr(
            _py,
            server_bits,
            b"_closed",
            MoltObject::from_bool(true).bits(),
        ) {
            return MoltObject::none().bits();
        }
        MoltObject::none().bits()
    })
}

pub(super) fn http_server_read_request_impl(
    _py: &molt_runtime_core::CoreGilToken,
    handler_bits: u64,
) -> Result<i64, u64> {
    let request_line = http_server_readline(_py, handler_bits, 65537)?;
    if request_line.is_empty() {
        return Ok(0);
    }
    let line_text = String::from_utf8_lossy(&request_line)
        .trim_end_matches(['\r', '\n'])
        .to_string();
    http_server_set_attr_string(_py, handler_bits, b"requestline", &line_text)?;
    http_server_set_attr_string(
        _py,
        handler_bits,
        b"request_version",
        HTTP_SERVER_DEFAULT_REQUEST_VERSION,
    )?;
    http_server_set_attr_string(_py, handler_bits, b"command", "")?;
    http_server_set_attr_string(_py, handler_bits, b"path", "")?;
    http_server_set_attr_string(_py, handler_bits, b"_molt_connection_header", "")?;

    let parts: Vec<&str> = line_text.split_whitespace().collect();
    if parts.is_empty() {
        return Ok(0);
    }
    let command: String;
    let path: String;
    let mut request_version = HTTP_SERVER_DEFAULT_REQUEST_VERSION.to_string();

    if parts.len() >= 3 {
        command = parts[0].to_string();
        path = parts[1].to_string();
        if parts.len() > 3 {
            http_server_send_error_impl(
                _py,
                handler_bits,
                400,
                Some(format!(
                    "Bad request version ({})",
                    http_server_repr_single_quoted(parts[3])
                )),
            )?;
            return Ok(2);
        }
        let version = parts[2];
        let Some(version_tail) = version.strip_prefix("HTTP/") else {
            http_server_send_error_impl(
                _py,
                handler_bits,
                400,
                Some(format!(
                    "Bad request version ({})",
                    http_server_repr_single_quoted(version)
                )),
            )?;
            return Ok(2);
        };
        let mut chunks = version_tail.split('.');
        let major = chunks.next().unwrap_or_default();
        let minor = chunks.next().unwrap_or_default();
        if major.is_empty()
            || minor.is_empty()
            || chunks.next().is_some()
            || !major.chars().all(|ch| ch.is_ascii_digit())
            || !minor.chars().all(|ch| ch.is_ascii_digit())
        {
            http_server_send_error_impl(
                _py,
                handler_bits,
                400,
                Some(format!(
                    "Bad request version ({})",
                    http_server_repr_single_quoted(version)
                )),
            )?;
            return Ok(2);
        }
        request_version = version.to_string();
    } else if parts.len() == 2 {
        command = parts[0].to_string();
        path = parts[1].to_string();
        if command != "GET" {
            http_server_send_error_impl(
                _py,
                handler_bits,
                400,
                Some(format!(
                    "Bad HTTP/0.9 request type ({})",
                    http_server_repr_single_quoted(&command)
                )),
            )?;
            return Ok(2);
        }
    } else {
        http_server_send_error_impl(
            _py,
            handler_bits,
            400,
            Some(format!(
                "Bad request syntax ({})",
                http_server_repr_single_quoted(&line_text)
            )),
        )?;
        return Ok(2);
    }

    http_server_set_attr_string(_py, handler_bits, b"command", &command)?;
    http_server_set_attr_string(_py, handler_bits, b"path", &path)?;
    http_server_set_attr_string(_py, handler_bits, b"request_version", &request_version)?;

    let mut headers: Vec<(String, String)> = Vec::new();
    let mut connection_header = String::new();
    loop {
        let line = http_server_readline(_py, handler_bits, 65537)?;
        if line.is_empty() || line == b"\r\n" || line == b"\n" {
            break;
        }
        let line_text = String::from_utf8_lossy(&line)
            .trim_end_matches(['\r', '\n'])
            .to_string();
        if let Some((key, value)) = line_text.split_once(':') {
            let key_text = key.trim().to_string();
            let value_text = value.trim_start().to_string();
            if key_text.eq_ignore_ascii_case("Connection") {
                connection_header = value_text.to_ascii_lowercase();
            }
            headers.push((key_text, value_text));
        }
    }
    let headers_bits = urllib_http_headers_to_list(_py, &headers)?;
    if !urllib_request_set_attr(_py, handler_bits, b"_molt_header_pairs", headers_bits) {
        dec_ref_bits(_py, headers_bits);
        return Err(MoltObject::none().bits());
    }
    if !urllib_request_set_attr(_py, handler_bits, b"headers", headers_bits) {
        dec_ref_bits(_py, headers_bits);
        return Err(MoltObject::none().bits());
    }
    dec_ref_bits(_py, headers_bits);
    http_server_set_attr_string(
        _py,
        handler_bits,
        b"_molt_connection_header",
        &connection_header,
    )?;
    Ok(1)
}

pub(super) fn http_server_compute_close_connection_impl(
    _py: &molt_runtime_core::CoreGilToken,
    handler_bits: u64,
) -> Result<bool, u64> {
    let connection =
        match http_server_get_optional_attr_string(_py, handler_bits, b"_molt_connection_header") {
            Ok(Some(value)) => value.to_ascii_lowercase(),
            Ok(None) => String::new(),
            Err(bits) => return Err(bits),
        };
    if connection == "close" {
        return Ok(true);
    }
    if connection == "keep-alive" {
        return Ok(false);
    }
    let request_version =
        match http_server_get_optional_attr_string(_py, handler_bits, b"request_version") {
            Ok(Some(value)) => value,
            Ok(None) => HTTP_SERVER_DEFAULT_REQUEST_VERSION.to_string(),
            Err(bits) => return Err(bits),
        };
    Ok(request_version != HTTP_SERVER_HTTP11)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_server_read_request(handler_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        match http_server_read_request_impl(_py, handler_bits) {
            Ok(state) => MoltObject::from_int(state).bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_server_compute_close_connection(handler_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        match http_server_compute_close_connection_impl(_py, handler_bits) {
            Ok(close) => MoltObject::from_bool(close).bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_server_handle_one_request(handler_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        match http_server_handle_one_request_impl(_py, handler_bits) {
            Ok(keep_running) => MoltObject::from_bool(keep_running).bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_server_send_response(
    handler_bits: u64,
    code_bits: u64,
    message_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(code) = to_i64(obj_from_bits(code_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "code must be int");
        };
        let message = if obj_from_bits(message_bits).is_none() {
            None
        } else {
            Some(crate::bridge::format_obj_str(
                _py,
                obj_from_bits(message_bits),
            ))
        };
        match http_server_send_response_impl(_py, handler_bits, code, message) {
            Ok(()) => MoltObject::none().bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_server_send_response_only(
    handler_bits: u64,
    code_bits: u64,
    message_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(code) = to_i64(obj_from_bits(code_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "code must be int");
        };
        let message = if obj_from_bits(message_bits).is_none() {
            None
        } else {
            Some(crate::bridge::format_obj_str(
                _py,
                obj_from_bits(message_bits),
            ))
        };
        match http_server_send_response_only_impl(_py, handler_bits, code, message) {
            Ok(()) => MoltObject::none().bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_server_send_header(
    handler_bits: u64,
    keyword_bits: u64,
    value_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let keyword = crate::bridge::format_obj_str(_py, obj_from_bits(keyword_bits));
        let value = crate::bridge::format_obj_str(_py, obj_from_bits(value_bits));
        match http_server_send_header_impl(_py, handler_bits, &keyword, &value) {
            Ok(()) => MoltObject::none().bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_server_end_headers(handler_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        match http_server_end_headers_impl(_py, handler_bits) {
            Ok(()) => MoltObject::none().bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_server_send_error(
    handler_bits: u64,
    code_bits: u64,
    message_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(code) = to_i64(obj_from_bits(code_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "code must be int");
        };
        let message = if obj_from_bits(message_bits).is_none() {
            None
        } else {
            Some(crate::bridge::format_obj_str(
                _py,
                obj_from_bits(message_bits),
            ))
        };
        match http_server_send_error_impl(_py, handler_bits, code, message) {
            Ok(()) => MoltObject::none().bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_server_version_string(
    server_version_bits: u64,
    sys_version_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let server_version = crate::bridge::format_obj_str(_py, obj_from_bits(server_version_bits));
        let sys_version = if obj_from_bits(sys_version_bits).is_none() {
            String::new()
        } else {
            crate::bridge::format_obj_str(_py, obj_from_bits(sys_version_bits))
        };
        let out = http_server_version_string_impl(&server_version, &sys_version);
        let Some(bits) = alloc_string_bits(_py, &out) else {
            return MoltObject::none().bits();
        };
        bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_server_date_time_string(timestamp_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let out = match http_server_date_time_string_from_bits(_py, timestamp_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(bits) = alloc_string_bits(_py, &out) else {
            return MoltObject::none().bits();
        };
        bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_status_reason(code_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(code) = to_i64(obj_from_bits(code_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "status code must be int");
        };
        let phrase = http_server_reason_phrase(code);
        let ptr = alloc_string(_py, phrase.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_status_constants() -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let entries = http_status_constants();
        let mut pairs: Vec<u64> = Vec::with_capacity(entries.len() * 2);
        let mut owned_bits: Vec<u64> = Vec::with_capacity(entries.len() * 2);
        for (name, code) in entries.iter().copied() {
            let key_ptr = alloc_string(_py, name.as_bytes());
            if key_ptr.is_null() {
                for bits in owned_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            let value_bits = MoltObject::from_int(code).bits();
            pairs.push(key_bits);
            pairs.push(value_bits);
            owned_bits.push(key_bits);
            owned_bits.push(value_bits);
        }
        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in owned_bits {
            dec_ref_bits(_py, bits);
        }
        if dict_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(dict_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_status_responses() -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let entries = http_status_constants();
        let mut seen_codes: HashSet<i64> = HashSet::new();
        let mut pairs: Vec<u64> = Vec::new();
        let mut owned_bits: Vec<u64> = Vec::new();
        for (_, code) in entries.iter().copied() {
            if !seen_codes.insert(code) {
                continue;
            }
            let key_bits = MoltObject::from_int(code).bits();
            let value_ptr = alloc_string(_py, http_server_reason_phrase(code).as_bytes());
            if value_ptr.is_null() {
                for bits in owned_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let value_bits = MoltObject::from_ptr(value_ptr).bits();
            pairs.push(key_bits);
            pairs.push(value_bits);
            owned_bits.push(key_bits);
            owned_bits.push(value_bits);
        }
        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in owned_bits {
            dec_ref_bits(_py, bits);
        }
        if dict_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(dict_ptr).bits()
    })
}
