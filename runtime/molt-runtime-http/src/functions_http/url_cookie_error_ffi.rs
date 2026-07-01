use super::*;
#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_quote(string_bits: u64, safe_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(string) = string_obj_to_owned(obj_from_bits(string_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "string must be str");
        };
        let Some(safe) = string_obj_to_owned(obj_from_bits(safe_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "safe must be str");
        };
        let out = urllib_quote_impl(&string, &safe);
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_quote_plus(string_bits: u64, safe_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(string) = string_obj_to_owned(obj_from_bits(string_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "string must be str");
        };
        let Some(safe) = string_obj_to_owned(obj_from_bits(safe_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "safe must be str");
        };
        let out = urllib_quote_impl(&string, &safe).replace("%20", "+");
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_unquote(string_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(string) = string_obj_to_owned(obj_from_bits(string_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "string must be str");
        };
        let out = urllib_unquote_impl(&string);
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_unquote_plus(string_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(string) = string_obj_to_owned(obj_from_bits(string_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "string must be str");
        };
        let out = urllib_unquote_impl(&string.replace('+', " "));
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_parse_qsl(
    qs_bits: u64,
    keep_blank_values_bits: u64,
    strict_parsing_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(qs) = string_obj_to_owned(obj_from_bits(qs_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "qs must be str");
        };
        let keep_blank_values = is_truthy(_py, obj_from_bits(keep_blank_values_bits));
        let strict_parsing = is_truthy(_py, obj_from_bits(strict_parsing_bits));
        let pairs = match urllib_parse_qsl_impl(&qs, keep_blank_values, strict_parsing) {
            Ok(pairs) => pairs,
            Err(msg) => return raise_exception::<_>(_py, "ValueError", &msg),
        };
        alloc_qsl_list(_py, &pairs)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_parse_qs(
    qs_bits: u64,
    keep_blank_values_bits: u64,
    strict_parsing_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(qs) = string_obj_to_owned(obj_from_bits(qs_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "qs must be str");
        };
        let keep_blank_values = is_truthy(_py, obj_from_bits(keep_blank_values_bits));
        let strict_parsing = is_truthy(_py, obj_from_bits(strict_parsing_bits));
        let pairs = match urllib_parse_qsl_impl(&qs, keep_blank_values, strict_parsing) {
            Ok(pairs) => pairs,
            Err(msg) => return raise_exception::<_>(_py, "ValueError", &msg),
        };
        let mut order: Vec<String> = Vec::new();
        let mut values: HashMap<String, Vec<String>> = HashMap::new();
        for (key, value) in pairs {
            if !values.contains_key(&key) {
                order.push(key.clone());
            }
            values.entry(key).or_default().push(value);
        }
        alloc_qs_dict(_py, &order, &values)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_urlencode(query_bits: u64, doseq_bits: u64, safe_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let doseq = is_truthy(_py, obj_from_bits(doseq_bits));
        let Some(safe) = string_obj_to_owned(obj_from_bits(safe_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "safe must be str");
        };
        let out = match urllib_urlencode_impl(_py, query_bits, doseq, &safe) {
            Ok(out) => out,
            Err(bits) => return bits,
        };
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_urlsplit(
    url_bits: u64,
    scheme_bits: u64,
    allow_fragments_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(url) = string_obj_to_owned(obj_from_bits(url_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "url must be str");
        };
        let Some(scheme) = string_obj_to_owned(obj_from_bits(scheme_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "scheme must be str");
        };
        let allow_fragments = is_truthy(_py, obj_from_bits(allow_fragments_bits));
        let split = urllib_urlsplit_impl(&url, &scheme, allow_fragments);
        alloc_string_tuple(_py, &split)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_client_urlsplit(
    url_bits: u64,
    scheme_bits: u64,
    allow_fragments_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(url) = string_obj_to_owned(obj_from_bits(url_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "url must be str");
        };
        let Some(scheme) = string_obj_to_owned(obj_from_bits(scheme_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "scheme must be str");
        };
        let allow_fragments = is_truthy(_py, obj_from_bits(allow_fragments_bits));
        let split = urllib_urlsplit_impl(&url, &scheme, allow_fragments);
        alloc_string_tuple(_py, &split)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_urlparse(
    url_bits: u64,
    scheme_bits: u64,
    allow_fragments_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(url) = string_obj_to_owned(obj_from_bits(url_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "url must be str");
        };
        let Some(scheme) = string_obj_to_owned(obj_from_bits(scheme_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "scheme must be str");
        };
        let allow_fragments = is_truthy(_py, obj_from_bits(allow_fragments_bits));
        let parsed = urllib_urlparse_impl(&url, &scheme, allow_fragments);
        alloc_string_tuple(_py, &parsed)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_urlunsplit(
    scheme_bits: u64,
    netloc_bits: u64,
    path_bits: u64,
    query_bits: u64,
    fragment_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(scheme) = string_obj_to_owned(obj_from_bits(scheme_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "scheme must be str");
        };
        let Some(netloc) = string_obj_to_owned(obj_from_bits(netloc_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "netloc must be str");
        };
        let Some(path) = string_obj_to_owned(obj_from_bits(path_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "path must be str");
        };
        let Some(query) = string_obj_to_owned(obj_from_bits(query_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "query must be str");
        };
        let Some(fragment) = string_obj_to_owned(obj_from_bits(fragment_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "fragment must be str");
        };
        let out = urllib_unsplit_impl(&scheme, &netloc, &path, &query, &fragment);
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_urlunparse(
    scheme_bits: u64,
    netloc_bits: u64,
    path_bits: u64,
    params_bits: u64,
    query_bits: u64,
    fragment_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(scheme) = string_obj_to_owned(obj_from_bits(scheme_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "scheme must be str");
        };
        let Some(netloc) = string_obj_to_owned(obj_from_bits(netloc_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "netloc must be str");
        };
        let Some(mut path) = string_obj_to_owned(obj_from_bits(path_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "path must be str");
        };
        let Some(params) = string_obj_to_owned(obj_from_bits(params_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "params must be str");
        };
        let Some(query) = string_obj_to_owned(obj_from_bits(query_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "query must be str");
        };
        let Some(fragment) = string_obj_to_owned(obj_from_bits(fragment_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "fragment must be str");
        };
        if !params.is_empty() {
            path.push(';');
            path.push_str(&params);
        }
        let out = urllib_unsplit_impl(&scheme, &netloc, &path, &query, &fragment);
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_urldefrag(url_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(url) = string_obj_to_owned(obj_from_bits(url_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "url must be str");
        };
        let (base, fragment) = if let Some((base, fragment)) = url.split_once('#') {
            (base.to_string(), fragment.to_string())
        } else {
            (url, String::new())
        };
        alloc_string_tuple(_py, &[base, fragment])
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_urljoin(base_bits: u64, url_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(base) = string_obj_to_owned(obj_from_bits(base_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "base must be str");
        };
        let Some(url) = string_obj_to_owned(obj_from_bits(url_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "url must be str");
        };
        if base.is_empty() {
            let out_ptr = alloc_string(_py, url.as_bytes());
            if out_ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(out_ptr).bits();
        }
        let target = urllib_urlsplit_impl(&url, "", true);
        let out = if !target[0].is_empty() {
            url
        } else {
            let base_parts = urllib_urlparse_impl(&base, "", true);
            if url.starts_with("//") {
                format!("{}:{url}", base_parts[0])
            } else if !target[1].is_empty() {
                urllib_unsplit_impl(
                    &base_parts[0],
                    &target[1],
                    &target[2],
                    &target[3],
                    &target[4],
                )
            } else {
                let mut path = target[2].clone();
                if path.is_empty() {
                    path = base_parts[2].clone();
                } else if !path.starts_with('/') {
                    let base_path = &base_parts[2];
                    let base_dir = match base_path.rsplit_once('/') {
                        Some((dir, _)) => dir.to_string(),
                        None => String::new(),
                    };
                    if base_dir.is_empty() {
                        path = format!("/{path}");
                    } else {
                        path = format!("{base_dir}/{path}");
                    }
                }
                urllib_unsplit_impl(
                    &base_parts[0],
                    &base_parts[1],
                    &path,
                    &target[3],
                    &target[4],
                )
            }
        };
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_cookiejar_new() -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = cookiejar_store_new() else {
            return raise_exception::<_>(_py, "RuntimeError", "cookie jar allocation failed");
        };
        MoltObject::from_int(handle).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_cookiejar_len(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "cookie jar handle is invalid");
        };
        let Some(size) = cookiejar_with(handle, |jar| jar.cookies.len()) else {
            return raise_exception::<_>(_py, "RuntimeError", "cookie jar handle is invalid");
        };
        MoltObject::from_int(i64::try_from(size).unwrap_or(i64::MAX)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_cookiejar_clear(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "cookie jar handle is invalid");
        };
        let Some(()) = cookiejar_with_mut(handle, |jar| {
            jar.cookies.clear();
        }) else {
            return raise_exception::<_>(_py, "RuntimeError", "cookie jar handle is invalid");
        };
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_cookiejar_extract(
    handle_bits: u64,
    request_url_bits: u64,
    headers_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "cookie jar handle is invalid");
        };
        let Some(request_url) = string_obj_to_owned(obj_from_bits(request_url_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "request url must be str");
        };
        let headers = match urllib_http_extract_headers_mapping(_py, headers_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        urllib_cookiejar_store_from_headers(handle, &request_url, &headers);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_cookiejar_header_for_url(
    handle_bits: u64,
    request_url_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "cookie jar handle is invalid");
        };
        let Some(request_url) = string_obj_to_owned(obj_from_bits(request_url_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "request url must be str");
        };
        let Some(header) = urllib_cookiejar_header_for_url(handle, &request_url) else {
            return MoltObject::none().bits();
        };
        let Some(bits) = alloc_string_bits(_py, &header) else {
            return MoltObject::none().bits();
        };
        bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_cookies_parse(cookie_header_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(cookie_header) = string_obj_to_owned(obj_from_bits(cookie_header_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "cookie header must be str");
        };
        let pairs = http_cookies_parse_pairs(&cookie_header);
        match urllib_http_headers_to_list(_py, &pairs) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_cookies_render_morsel(
    name_bits: u64,
    value_bits: u64,
    path_bits: u64,
    secure_bits: u64,
    httponly_bits: u64,
    max_age_bits: u64,
    expires_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let out = http_cookies_render_morsel_impl(
            _py,
            HttpCookieMorselInput {
                name_bits,
                value_bits,
                path_bits,
                secure_bits,
                httponly_bits,
                max_age_bits,
                expires_bits,
            },
        );
        let Some(bits) = alloc_string_bits(_py, &out) else {
            return MoltObject::none().bits();
        };
        bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_error_urlerror_init(
    self_bits: u64,
    reason_bits: u64,
    filename_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if !urllib_error_init_args(_py, self_bits, &[reason_bits]) {
            return MoltObject::none().bits();
        }
        if !urllib_error_set_attr(_py, self_bits, "reason", reason_bits) {
            return MoltObject::none().bits();
        }
        if !obj_from_bits(filename_bits).is_none()
            && !urllib_error_set_attr(_py, self_bits, "filename", filename_bits)
        {
            return MoltObject::none().bits();
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_error_urlerror_str(reason_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let reason_text = crate::bridge::format_obj_str(_py, obj_from_bits(reason_bits));
        let out = format!("<urlopen error {reason_text}>");
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_error_httperror_init(
    self_bits: u64,
    url_bits: u64,
    code_bits: u64,
    msg_bits: u64,
    hdrs_bits: u64,
    fp_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        // CPython's urllib.error.HTTPError does not populate BaseException.args
        // with constructor values in this path; normalize args to ().
        if !urllib_error_init_args(_py, self_bits, &[]) {
            return MoltObject::none().bits();
        }
        if !urllib_error_set_attr(_py, self_bits, "reason", msg_bits)
            || !urllib_error_set_attr(_py, self_bits, "code", code_bits)
            || !urllib_error_set_attr(_py, self_bits, "msg", msg_bits)
            || !urllib_error_set_attr(_py, self_bits, "hdrs", hdrs_bits)
            || !urllib_error_set_attr(_py, self_bits, "filename", url_bits)
            || !urllib_error_set_attr(_py, self_bits, "fp", fp_bits)
        {
            return MoltObject::none().bits();
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_error_httperror_str(code_bits: u64, msg_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let code_text = crate::bridge::format_obj_str(_py, obj_from_bits(code_bits));
        let msg_text = crate::bridge::format_obj_str(_py, obj_from_bits(msg_bits));
        let out = format!("HTTP Error {code_text}: {msg_text}");
        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_error_content_too_short_init(
    self_bits: u64,
    msg_bits: u64,
    content_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if !urllib_error_init_args(_py, self_bits, &[msg_bits]) {
            return MoltObject::none().bits();
        }
        if !urllib_error_set_attr(_py, self_bits, "reason", msg_bits)
            || !urllib_error_set_attr(_py, self_bits, "content", content_bits)
        {
            return MoltObject::none().bits();
        }
        MoltObject::none().bits()
    })
}
