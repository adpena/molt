use super::*;

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_request_init(
    self_bits: u64,
    url_bits: u64,
    data_bits: u64,
    headers_bits: u64,
    method_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let url_text = crate::bridge::format_obj_str(_py, obj_from_bits(url_bits));
        let url_ptr = alloc_string(_py, url_text.as_bytes());
        if url_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let full_url_bits = MoltObject::from_ptr(url_ptr).bits();
        let mut headers_value = headers_bits;
        if obj_from_bits(headers_bits).is_none() {
            let dict_bits = crate::bridge::molt_dict_new(0);
            if obj_from_bits(dict_bits).is_none() {
                return MoltObject::none().bits();
            }
            headers_value = dict_bits;
        }
        if !urllib_request_set_attr(_py, self_bits, b"full_url", full_url_bits)
            || !urllib_request_set_attr(_py, self_bits, b"data", data_bits)
            || !urllib_request_set_attr(_py, self_bits, b"headers", headers_value)
            || !urllib_request_set_attr(_py, self_bits, b"method", method_bits)
        {
            return MoltObject::none().bits();
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_opener_init(self_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let list_ptr = alloc_list_with_capacity(_py, &[], 0);
        if list_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let handlers_bits = MoltObject::from_ptr(list_ptr).bits();
        if !urllib_request_set_attr(_py, self_bits, b"_molt_handlers", handlers_bits)
            || !urllib_request_set_cursor(_py, self_bits, 0)
        {
            return MoltObject::none().bits();
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_add_handler(opener_bits: u64, handler_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let list_bits = match urllib_request_ensure_handlers_list(_py, opener_bits) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        if !urllib_request_set_attr(_py, handler_bits, b"parent", opener_bits) {
            return MoltObject::none().bits();
        }
        let new_order = match urllib_request_handler_order(_py, handler_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(list_ptr) = obj_from_bits(list_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "opener handler registry is invalid");
        };
        let existing: Vec<u64> = unsafe { seq_vec_ref(list_ptr).to_vec() };
        let mut insert_at = existing.len();
        for (idx, existing_bits) in existing.iter().copied().enumerate() {
            let existing_order = match urllib_request_handler_order(_py, existing_bits) {
                Ok(value) => value,
                Err(bits) => return bits,
            };
            if new_order < existing_order {
                insert_at = idx;
                break;
            }
        }
        let index_bits = MoltObject::from_int(insert_at as i64).bits();
        let _ = molt_list_insert(list_bits, index_bits, handler_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_open(opener_bits: u64, request_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let mut owned_request_refs: Vec<u64> = Vec::new();
        let out_bits = (|| -> u64 {
            let mut active_request_bits = request_bits;
            let mut full_url = {
                let Some(full_url_bits) =
                    (match attr_optional(_py, active_request_bits, b"full_url") {
                        Ok(bits) => bits,
                        Err(bits) => return bits,
                    })
                else {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "request object is missing full_url",
                    );
                };
                let Some(text) = string_obj_to_owned(obj_from_bits(full_url_bits)) else {
                    dec_ref_bits(_py, full_url_bits);
                    return raise_exception::<_>(_py, "TypeError", "request.full_url must be str");
                };
                dec_ref_bits(_py, full_url_bits);
                text
            };
            let mut scheme = urllib_split_scheme(&full_url, "").0;

            let previous_cursor = match urllib_request_get_cursor(_py, opener_bits) {
                Ok(value) => value.max(0),
                Err(bits) => return bits,
            };
            let list_bits = match urllib_request_ensure_handlers_list(_py, opener_bits) {
                Ok(bits) => bits,
                Err(bits) => return bits,
            };
            let Some(list_ptr) = obj_from_bits(list_bits).as_ptr() else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "opener handler registry is invalid",
                );
            };
            let handlers: Vec<u64> = unsafe { seq_vec_ref(list_ptr).to_vec() };
            let start_idx = (previous_cursor as usize).min(handlers.len());

            let request_method_name = format!("{}_request", scheme);
            for (idx, handler_bits) in handlers.iter().copied().enumerate().skip(start_idx) {
                let Some(method_bits) =
                    (match attr_optional(_py, handler_bits, request_method_name.as_bytes()) {
                        Ok(bits) => bits,
                        Err(bits) => return bits,
                    })
                else {
                    continue;
                };
                if !molt_is_callable(method_bits) {
                    dec_ref_bits(_py, method_bits);
                    continue;
                }
                if !urllib_request_set_cursor(_py, opener_bits, (idx + 1) as i64) {
                    dec_ref_bits(_py, method_bits);
                    return MoltObject::none().bits();
                }
                let out_bits = unsafe { call_callable1(_py, method_bits, active_request_bits) };
                dec_ref_bits(_py, method_bits);
                if !urllib_request_set_cursor(_py, opener_bits, previous_cursor) {
                    if !obj_from_bits(out_bits).is_none() {
                        dec_ref_bits(_py, out_bits);
                    }
                    return MoltObject::none().bits();
                }
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                if !obj_from_bits(out_bits).is_none() {
                    active_request_bits = out_bits;
                    owned_request_refs.push(out_bits);
                } else {
                    dec_ref_bits(_py, out_bits);
                }
            }

            full_url = {
                let Some(full_url_bits) =
                    (match attr_optional(_py, active_request_bits, b"full_url") {
                        Ok(bits) => bits,
                        Err(bits) => return bits,
                    })
                else {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "request object is missing full_url",
                    );
                };
                let Some(text) = string_obj_to_owned(obj_from_bits(full_url_bits)) else {
                    dec_ref_bits(_py, full_url_bits);
                    return raise_exception::<_>(_py, "TypeError", "request.full_url must be str");
                };
                dec_ref_bits(_py, full_url_bits);
                text
            };
            scheme = urllib_split_scheme(&full_url, "").0;

            let method_name = format!("{}_open", scheme);
            for (idx, handler_bits) in handlers.iter().copied().enumerate().skip(start_idx) {
                let Some(method_bits) =
                    (match attr_optional(_py, handler_bits, method_name.as_bytes()) {
                        Ok(bits) => bits,
                        Err(bits) => return bits,
                    })
                else {
                    continue;
                };
                if !molt_is_callable(method_bits) {
                    dec_ref_bits(_py, method_bits);
                    continue;
                }
                if !urllib_request_set_cursor(_py, opener_bits, (idx + 1) as i64) {
                    dec_ref_bits(_py, method_bits);
                    return MoltObject::none().bits();
                }
                let out_bits = unsafe { call_callable1(_py, method_bits, active_request_bits) };
                dec_ref_bits(_py, method_bits);
                if !urllib_request_set_cursor(_py, opener_bits, previous_cursor) {
                    return MoltObject::none().bits();
                }
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                if !obj_from_bits(out_bits).is_none() {
                    return out_bits;
                }
            }
            if !urllib_request_set_cursor(_py, opener_bits, previous_cursor) {
                return MoltObject::none().bits();
            }

            let allow_data_fallback =
                match attr_optional(_py, opener_bits, b"_molt_allow_data_fallback") {
                    Ok(Some(bits)) => {
                        let value = is_truthy(_py, obj_from_bits(bits));
                        dec_ref_bits(_py, bits);
                        value
                    }
                    Ok(None) => false,
                    Err(bits) => return bits,
                };

            let mut response_bits = if scheme == "data" && allow_data_fallback {
                let payload = match urllib_request_decode_data_url(&full_url) {
                    Ok(value) => value,
                    Err(msg) => return raise_exception::<_>(_py, "ValueError", &msg),
                };
                let Some(handle) = urllib_response_store(urllib_response_from_parts(
                    payload,
                    full_url.clone(),
                    // CPython's data: handler returns an addinfourl without HTTP status metadata.
                    -1,
                    String::new(),
                    vec![("Content-Type".to_string(), "text/plain".to_string())],
                )) else {
                    return MoltObject::none().bits();
                };
                urllib_http_make_response_bits(_py, handle)
            } else if scheme == "http" || scheme == "https" {
                let split = urllib_urlsplit_impl(&full_url, "", true);
                let netloc = split[1].clone();
                if netloc.is_empty() {
                    return urllib_raise_url_error(_py, "no host given");
                }
                let default_port = if scheme == "https" { 443 } else { 80 };
                let (target_host, _target_port) =
                    urllib_http_parse_host_port(&netloc, default_port);
                if target_host.is_empty() {
                    return urllib_raise_url_error(_py, "no host given");
                }
                let timeout = match urllib_http_request_timeout(_py, active_request_bits) {
                    Ok(value) => value,
                    Err(bits) => return bits,
                };
                let (mut method, mut body) =
                    match urllib_http_extract_method_and_body(_py, active_request_bits) {
                        Ok(value) => value,
                        Err(bits) => return bits,
                    };
                let mut base_headers =
                    match urllib_http_extract_request_headers(_py, active_request_bits) {
                        Ok(value) => value,
                        Err(bits) => return bits,
                    };
                let proxy = match urllib_http_find_proxy_for_scheme(
                    _py,
                    opener_bits,
                    &scheme,
                    &target_host,
                ) {
                    Ok(value) => value,
                    Err(bits) => return bits,
                };
                let mut proxy_auth_header: Option<String> = None;
                let mut proxy_auth_attempted = false;
                let mut current_url = full_url.clone();
                let mut redirects = 0usize;
                let cookiejar_handles = match urllib_cookiejar_handles_from_handlers(_py, &handlers)
                {
                    Ok(value) => value,
                    Err(bits) => return bits,
                };
                loop {
                    let parts = urllib_urlsplit_impl(&current_url, "", true);
                    let netloc_now = parts[1].clone();
                    let mut path = parts[2].clone();
                    if path.is_empty() {
                        path = "/".to_string();
                    }
                    if !parts[3].is_empty() {
                        path.push('?');
                        path.push_str(&parts[3]);
                    }
                    let (host_now, port_now) =
                        urllib_http_parse_host_port(&netloc_now, default_port);
                    if host_now.is_empty() {
                        return urllib_raise_url_error(_py, "no host given");
                    }
                    let mut effective_headers = base_headers.clone();
                    urllib_cookiejar_apply_header_for_url(
                        _py,
                        &cookiejar_handles,
                        &current_url,
                        &mut effective_headers,
                    );
                    if let Some(proxy_auth_value) = proxy_auth_header.as_ref() {
                        let mut replaced = false;
                        for (name, value) in &mut effective_headers {
                            if name.eq_ignore_ascii_case("Proxy-Authorization") {
                                *value = proxy_auth_value.clone();
                                replaced = true;
                                break;
                            }
                        }
                        if !replaced {
                            effective_headers.push((
                                "Proxy-Authorization".to_string(),
                                proxy_auth_value.clone(),
                            ));
                        }
                    }
                    let req = if let Some(proxy_url) = proxy.as_deref() {
                        let proxy_parts = urllib_urlsplit_impl(proxy_url, "", true);
                        let proxy_netloc = proxy_parts[1].clone();
                        let (proxy_host, proxy_port) =
                            urllib_http_parse_host_port(&proxy_netloc, 80);
                        if proxy_host.is_empty() {
                            return urllib_raise_url_error(_py, "proxy URL is invalid");
                        }
                        if scheme == "https" {
                            return urllib_raise_url_error(_py, "https proxies are not supported");
                        }
                        UrllibHttpRequest {
                            host: proxy_host,
                            port: proxy_port,
                            path: current_url.clone(),
                            method: method.clone(),
                            headers: {
                                let mut out = Vec::new();
                                out.append(&mut effective_headers);
                                out
                            },
                            body: body.clone(),
                            timeout,
                            // Proxies always speak plain HTTP to the proxy peer
                            // (CONNECT tunneling for https proxies is rejected
                            // above), so no TLS termination at this hop.
                            tls_server_name: None,
                        }
                    } else {
                        UrllibHttpRequest {
                            host: host_now.clone(),
                            port: port_now,
                            path: path.clone(),
                            method: method.clone(),
                            headers: {
                                let mut out = Vec::new();
                                out.append(&mut effective_headers);
                                out
                            },
                            body: body.clone(),
                            timeout,
                            tls_server_name: if scheme == "https" {
                                Some(host_now.clone())
                            } else {
                                None
                            },
                        }
                    };
                    let host_header = if port_now == default_port {
                        host_now.clone()
                    } else {
                        format!("{host_now}:{port_now}")
                    };
                    let (code, reason, resp_headers, resp_body) =
                        match urllib_http_try_inmemory_dispatch(_py, &req, &req.path, &host_header)
                        {
                            Ok(Some(value)) => value,
                            Ok(None) => {
                                match urllib_http_send_request(&req, &req.path, &host_header) {
                                    Ok(value) => value,
                                    Err(err) => {
                                        if err.kind() == ErrorKind::TimedOut
                                            || err.kind() == ErrorKind::WouldBlock
                                        {
                                            return urllib_http_timeout_error(_py);
                                        }
                                        return urllib_raise_url_error(_py, &err.to_string());
                                    }
                                }
                            }
                            Err(bits) => return bits,
                        };
                    urllib_cookiejar_store_headers_for_url(
                        &cookiejar_handles,
                        &current_url,
                        &resp_headers,
                    );
                    if code == 407 && proxy.is_some() {
                        if proxy_auth_attempted {
                            return urllib_raise_url_error(_py, "proxy authentication required");
                        }
                        let proxy_url = proxy.as_deref().unwrap_or_default();
                        let challenge =
                            urllib_http_find_header(&resp_headers, "Proxy-Authenticate")
                                .unwrap_or_default();
                        let realm = urllib_http_parse_basic_realm(challenge);
                        let creds = match urllib_proxy_find_basic_credentials(
                            _py,
                            &handlers,
                            proxy_url,
                            realm.as_deref(),
                        ) {
                            Ok(value) => value,
                            Err(bits) => return bits,
                        };
                        let Some((username, password)) = creds else {
                            return urllib_raise_url_error(_py, "proxy authentication required");
                        };
                        let token =
                            urllib_base64_encode(format!("{username}:{password}").as_bytes());
                        proxy_auth_header = Some(format!("Basic {token}"));
                        proxy_auth_attempted = true;
                        continue;
                    }
                    let location =
                        urllib_http_find_header(&resp_headers, "Location").map(str::to_string);
                    if (code == 301 || code == 302 || code == 303 || code == 307 || code == 308)
                        && location.is_some()
                    {
                        if redirects >= 10 {
                            return urllib_raise_url_error(_py, "redirect loop");
                        }
                        redirects += 1;
                        let next =
                            urllib_http_join_url(&current_url, location.as_deref().unwrap_or(""));
                        current_url = next;
                        if code == 303 || ((code == 301 || code == 302) && method != "HEAD") {
                            method = "GET".to_string();
                            body.clear();
                            base_headers.retain(|(name, _)| {
                                !name.eq_ignore_ascii_case("Content-Length")
                                    && !name.eq_ignore_ascii_case("Content-Type")
                            });
                        }
                        continue;
                    }
                    let Some(handle) = urllib_response_store(urllib_response_from_parts(
                        resp_body,
                        current_url.clone(),
                        code,
                        reason,
                        resp_headers,
                    )) else {
                        return MoltObject::none().bits();
                    };
                    break urllib_http_make_response_bits(_py, handle);
                }
            } else {
                return MoltObject::none().bits();
            };

            let response_method_name = format!("{}_response", scheme);
            for handler_bits in handlers {
                let Some(method_bits) =
                    (match attr_optional(_py, handler_bits, response_method_name.as_bytes()) {
                        Ok(bits) => bits,
                        Err(bits) => return bits,
                    })
                else {
                    continue;
                };
                if !molt_is_callable(method_bits) {
                    dec_ref_bits(_py, method_bits);
                    continue;
                }
                let out_bits =
                    unsafe { call_callable2(_py, method_bits, active_request_bits, response_bits) };
                dec_ref_bits(_py, method_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                if !obj_from_bits(out_bits).is_none() {
                    if out_bits == response_bits {
                        // A handler may return the same response object it was passed.
                        // Keep the existing owned `response_bits` reference intact.
                    } else {
                        dec_ref_bits(_py, response_bits);
                        response_bits = out_bits;
                    }
                } else {
                    dec_ref_bits(_py, out_bits);
                }
            }
            response_bits
        })();
        for bits in owned_request_refs {
            dec_ref_bits(_py, bits);
        }
        out_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_process_http_error(
    request_bits: u64,
    response_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let handle = match urllib_request_response_handle_from_bits(_py, response_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some((code, reason, headers, url)) = urllib_response_with(handle, |resp| {
            (
                resp.code,
                resp.reason.clone(),
                resp.headers.clone(),
                resp.url.clone(),
            )
        }) else {
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
        };
        if code >= 400 {
            return urllib_raise_http_error(_py, &url, code, &reason, &headers, response_bits);
        }
        let _ = request_bits;
        response_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_response_read(handle_bits: u64, size_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        let size_opt = if obj_from_bits(size_bits).is_none() {
            None
        } else {
            to_i64(obj_from_bits(size_bits))
        };
        let Some(out) =
            urllib_response_with_mut(handle, |resp| urllib_response_read_vec(resp, size_opt))
        else {
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
        };
        match out {
            Ok(data) => {
                let ptr = crate::bridge::alloc_bytes(_py, data.as_slice());
                if ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(ptr).bits()
                }
            }
            Err(msg) => raise_exception::<_>(_py, "ValueError", &msg),
        }
    })
}

#[inline]
pub(super) fn urllib_response_is_data(resp: &MoltUrllibResponse) -> bool {
    resp.code < 0
}

pub(super) fn urllib_response_read_vec(
    resp: &mut MoltUrllibResponse,
    size_opt: Option<i64>,
) -> Result<Vec<u8>, String> {
    if resp.closed {
        if urllib_response_is_data(resp) {
            return Err("I/O operation on closed file.".to_string());
        }
        return Ok(Vec::new());
    }
    let total = resp.body.len();
    let start = resp.pos.min(total);
    let end = match size_opt {
        Some(value) if value >= 0 => {
            let wanted = usize::try_from(value).unwrap_or(0);
            total.min(start.saturating_add(wanted))
        }
        _ => total,
    };
    resp.pos = end;
    Ok(resp.body[start..end].to_vec())
}

pub(super) fn urllib_response_readinto_len(
    resp: &mut MoltUrllibResponse,
    out_buf: &mut [u8],
) -> Result<usize, String> {
    if resp.closed {
        if urllib_response_is_data(resp) {
            return Err("I/O operation on closed file.".to_string());
        }
        return Ok(0);
    }
    let out_len = out_buf.len();
    let total = resp.body.len();
    let start = resp.pos.min(total);
    let end = total.min(start.saturating_add(out_len));
    let read_len = end.saturating_sub(start);
    if read_len > 0 {
        out_buf[..read_len].copy_from_slice(&resp.body[start..end]);
    }
    resp.pos = end;
    Ok(read_len)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_response_readinto(handle_bits: u64, buffer_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        let mut export = crate::bridge::BufferExport {
            ptr: 0,
            len: 0,
            readonly: 0,
            stride: 0,
            itemsize: 0,
        };
        if crate::bridge::molt_buffer_export(buffer_bits, &mut export)
            || export.readonly != 0
            || export.itemsize != 1
        {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "readinto() argument must be a writable bytes-like object",
            );
        }
        let out_len = export.len as usize;
        if out_len == 0 {
            return MoltObject::from_int(0).bits();
        }
        let out_buf = unsafe { std::slice::from_raw_parts_mut(export.ptr as *mut u8, out_len) };
        let Some(out) =
            urllib_response_with_mut(handle, |resp| urllib_response_readinto_len(resp, out_buf))
        else {
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
        };
        match out {
            Ok(read_len) => MoltObject::from_int(read_len as i64).bits(),
            Err(msg) => raise_exception::<_>(_py, "ValueError", &msg),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_response_read1(handle_bits: u64, size_bits: u64) -> u64 {
    molt_urllib_request_response_read(handle_bits, size_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_response_readinto1(
    handle_bits: u64,
    buffer_bits: u64,
) -> u64 {
    molt_urllib_request_response_readinto(handle_bits, buffer_bits)
}

pub(super) fn urllib_response_readline_vec(
    resp: &mut MoltUrllibResponse,
    size_opt: Option<i64>,
) -> Result<Vec<u8>, String> {
    if resp.closed {
        if urllib_response_is_data(resp) {
            return Err("I/O operation on closed file.".to_string());
        }
        return Ok(Vec::new());
    }
    let total = resp.body.len();
    let start = resp.pos.min(total);
    let max_end = match size_opt {
        Some(value) if value >= 0 => {
            let wanted = usize::try_from(value).unwrap_or(0);
            total.min(start.saturating_add(wanted))
        }
        _ => total,
    };
    if start >= max_end {
        return Ok(Vec::new());
    }
    let slice = &resp.body[start..max_end];
    let end = match slice.iter().position(|b| *b == b'\n') {
        Some(offset) => start.saturating_add(offset).saturating_add(1),
        None => max_end,
    };
    resp.pos = end;
    Ok(resp.body[start..end].to_vec())
}

pub(super) fn urllib_response_seek_pos(
    resp: &MoltUrllibResponse,
    offset: i64,
    whence: i64,
) -> Result<usize, String> {
    let base = match whence {
        0 => 0_i128,
        1 => i128::try_from(resp.pos).unwrap_or(i128::MAX),
        2 => i128::try_from(resp.body.len()).unwrap_or(i128::MAX),
        _ => return Err(format!("whence value {whence} unsupported")),
    };
    let target = base.saturating_add(i128::from(offset));
    if target < 0 {
        return Err(format!("negative seek value {target}"));
    }
    let as_u128 = target as u128;
    if as_u128 > (usize::MAX as u128) {
        return Err("seek position out of range".to_string());
    }
    Ok(as_u128 as usize)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_response_readline(handle_bits: u64, size_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        let size_opt = if obj_from_bits(size_bits).is_none() {
            None
        } else {
            to_i64(obj_from_bits(size_bits))
        };
        let Some(out) =
            urllib_response_with_mut(handle, |resp| urllib_response_readline_vec(resp, size_opt))
        else {
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
        };
        match out {
            Ok(data) => {
                let ptr = crate::bridge::alloc_bytes(_py, data.as_slice());
                if ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(ptr).bits()
                }
            }
            Err(msg) => raise_exception::<_>(_py, "ValueError", &msg),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_response_readlines(handle_bits: u64, hint_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        let hint_obj = obj_from_bits(hint_bits);
        let hint = if hint_obj.is_none() {
            None
        } else {
            match to_i64(hint_obj) {
                Some(value) if value <= 0 => None,
                Some(value) => Some(usize::try_from(value).unwrap_or(usize::MAX)),
                None => return raise_exception::<_>(_py, "TypeError", "hint must be int or None"),
            }
        };
        let Some(out) = urllib_response_with_mut(handle, |resp| {
            if resp.closed {
                if urllib_response_is_data(resp) {
                    return Err(raise_exception::<u64>(
                        _py,
                        "ValueError",
                        "I/O operation on closed file.",
                    ));
                }
                let list_ptr = alloc_list_with_capacity(_py, &[], 0);
                if list_ptr.is_null() {
                    return Err(MoltObject::none().bits());
                }
                return Ok(MoltObject::from_ptr(list_ptr).bits());
            }
            let mut lines: Vec<u64> = Vec::new();
            let mut total = 0usize;
            loop {
                let line = match urllib_response_readline_vec(resp, None) {
                    Ok(data) => data,
                    Err(msg) => return Err(raise_exception::<u64>(_py, "ValueError", &msg)),
                };
                if line.is_empty() {
                    break;
                }
                total = total.saturating_add(line.len());
                let line_ptr = alloc_bytes(_py, line.as_slice());
                if line_ptr.is_null() {
                    for bits in lines {
                        dec_ref_bits(_py, bits);
                    }
                    return Err(MoltObject::none().bits());
                }
                lines.push(MoltObject::from_ptr(line_ptr).bits());
                if let Some(limit) = hint
                    && total >= limit
                {
                    break;
                }
            }
            let list_ptr = alloc_list_with_capacity(_py, lines.as_slice(), lines.len());
            if list_ptr.is_null() {
                for bits in lines {
                    dec_ref_bits(_py, bits);
                }
                return Err(MoltObject::none().bits());
            }
            for bits in lines {
                dec_ref_bits(_py, bits);
            }
            Ok(MoltObject::from_ptr(list_ptr).bits())
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
pub extern "C" fn molt_urllib_request_response_readable(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        let Some(out) = urllib_response_with(handle, |resp| {
            if resp.closed && urllib_response_is_data(resp) {
                return Err("I/O operation on closed file.".to_string());
            }
            Ok(true)
        }) else {
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
        };
        match out {
            Ok(value) => MoltObject::from_bool(value).bits(),
            Err(msg) => raise_exception::<_>(_py, "ValueError", &msg),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_response_writable(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        let Some(out) = urllib_response_with(handle, |resp| {
            if resp.closed && urllib_response_is_data(resp) {
                return Err("I/O operation on closed file.".to_string());
            }
            Ok(urllib_response_is_data(resp))
        }) else {
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
        };
        match out {
            Ok(value) => MoltObject::from_bool(value).bits(),
            Err(msg) => raise_exception::<_>(_py, "ValueError", &msg),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_response_seekable(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        let Some(out) = urllib_response_with(handle, |resp| {
            if resp.closed && urllib_response_is_data(resp) {
                return Err("I/O operation on closed file.".to_string());
            }
            Ok(urllib_response_is_data(resp))
        }) else {
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
        };
        match out {
            Ok(value) => MoltObject::from_bool(value).bits(),
            Err(msg) => raise_exception::<_>(_py, "ValueError", &msg),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_response_tell(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        let Some(out) = urllib_response_with(handle, |resp| {
            if !urllib_response_is_data(resp) {
                return Err(raise_exception::<u64>(_py, "UnsupportedOperation", "seek"));
            }
            if resp.closed {
                return Err(raise_exception::<u64>(
                    _py,
                    "ValueError",
                    "I/O operation on closed file.",
                ));
            }
            Ok(resp.pos)
        }) else {
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
        };
        match out {
            Ok(pos) => MoltObject::from_int(pos as i64).bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_response_seek(
    handle_bits: u64,
    offset_bits: u64,
    whence_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        let Some(offset) = to_i64(obj_from_bits(offset_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "offset must be int");
        };
        let Some(whence) = to_i64(obj_from_bits(whence_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "whence must be int");
        };
        let Some(out) = urllib_response_with_mut(handle, |resp| {
            if !urllib_response_is_data(resp) {
                return Err(raise_exception::<u64>(_py, "UnsupportedOperation", "seek"));
            }
            if resp.closed {
                return Err(raise_exception::<u64>(
                    _py,
                    "ValueError",
                    "I/O operation on closed file.",
                ));
            }
            let pos = match urllib_response_seek_pos(resp, offset, whence) {
                Ok(pos) => pos,
                Err(msg) => return Err(raise_exception::<u64>(_py, "ValueError", &msg)),
            };
            resp.pos = pos;
            Ok(pos)
        }) else {
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
        };
        match out {
            Ok(pos) => MoltObject::from_int(pos as i64).bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_response_close(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        let Some(()) = urllib_response_with_mut(handle, |resp| {
            resp.closed = true;
        }) else {
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
        };
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_response_drop(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        urllib_response_drop(_py, handle);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_response_geturl(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        let Some(out) = urllib_response_with(handle, |resp| {
            let ptr = alloc_string(_py, resp.url.as_bytes());
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
pub extern "C" fn molt_urllib_request_response_getcode(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        let Some(code) = urllib_response_with(handle, |resp| resp.code) else {
            return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
        };
        if code < 0 {
            MoltObject::none().bits()
        } else {
            MoltObject::from_int(code).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_response_getreason(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        let Some(out) = urllib_response_with(handle, |resp| {
            if resp.reason.is_empty() {
                return Ok(MoltObject::none().bits());
            }
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
pub extern "C" fn molt_urllib_request_response_getheader(
    handle_bits: u64,
    name_bits: u64,
    default_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        let name = crate::bridge::format_obj_str(_py, obj_from_bits(name_bits));
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
pub extern "C" fn molt_urllib_request_response_getheaders(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        let Some(out) =
            urllib_response_with_mut(handle, |resp| urllib_response_headers_dict_bits(_py, resp))
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
pub extern "C" fn molt_urllib_request_response_getheaders_list(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
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

pub(super) fn urllib_response_message_bits(
    _py: &molt_runtime_core::CoreGilToken,
    handle: i64,
) -> u64 {
    let Some(headers) = urllib_response_with(handle, |resp| resp.headers.clone()) else {
        return raise_exception::<_>(_py, "RuntimeError", "response handle is invalid");
    };
    let Some(message_handle) = http_message_store(headers) else {
        return MoltObject::none().bits();
    };
    MoltObject::from_int(message_handle).bits()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_response_message(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        urllib_response_message_bits(_py, handle)
    })
}
