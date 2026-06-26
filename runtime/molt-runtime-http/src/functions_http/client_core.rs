use super::*;

pub(super) fn urllib_request_set_attr(
    _py: &molt_runtime_core::CoreGilToken,
    obj_bits: u64,
    name: &[u8],
    value_bits: u64,
) -> bool {
    let Some(name_bits) = attr_name_bits_from_bytes(_py, name) else {
        return false;
    };
    crate::bridge::molt_object_setattr(obj_bits, name_bits, value_bits);
    dec_ref_bits(_py, name_bits);
    !exception_pending(_py)
}

pub(super) fn urllib_request_handler_order(
    _py: &molt_runtime_core::CoreGilToken,
    handler_bits: u64,
) -> Result<i64, u64> {
    let Some(order_bits) = urllib_request_attr_optional(_py, handler_bits, b"handler_order")?
    else {
        return Ok(500);
    };
    let out = to_i64(obj_from_bits(order_bits)).unwrap_or(500);
    dec_ref_bits(_py, order_bits);
    Ok(out)
}

pub(super) fn urllib_request_ensure_handlers_list(
    _py: &molt_runtime_core::CoreGilToken,
    opener_bits: u64,
) -> Result<u64, u64> {
    if let Some(list_bits) = urllib_request_attr_optional(_py, opener_bits, b"_molt_handlers")? {
        let Some(list_ptr) = obj_from_bits(list_bits).as_ptr() else {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "opener handler registry is invalid",
            ));
        };
        unsafe {
            if object_type_id(list_ptr) != crate::bridge::type_id_list() {
                return Err(raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "opener handler registry is invalid",
                ));
            }
        }
        return Ok(list_bits);
    }
    let list_ptr = alloc_list_with_capacity(_py, &[], 0);
    if list_ptr.is_null() {
        return Err(MoltObject::none().bits());
    }
    let list_bits = MoltObject::from_ptr(list_ptr).bits();
    if !urllib_request_set_attr(_py, opener_bits, b"_molt_handlers", list_bits) {
        return Err(MoltObject::none().bits());
    }
    Ok(list_bits)
}

pub(super) fn urllib_request_set_cursor(
    _py: &molt_runtime_core::CoreGilToken,
    opener_bits: u64,
    cursor: i64,
) -> bool {
    urllib_request_set_attr(
        _py,
        opener_bits,
        b"_molt_open_cursor",
        MoltObject::from_int(cursor).bits(),
    )
}

pub(super) fn urllib_request_get_cursor(
    _py: &molt_runtime_core::CoreGilToken,
    opener_bits: u64,
) -> Result<i64, u64> {
    let Some(bits) = urllib_request_attr_optional(_py, opener_bits, b"_molt_open_cursor")? else {
        return Ok(0);
    };
    let out = to_i64(obj_from_bits(bits)).unwrap_or(0);
    dec_ref_bits(_py, bits);
    Ok(out)
}

pub(super) fn urllib_data_percent_decode(input: &str) -> Vec<u8> {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut idx = 0usize;
    while idx < bytes.len() {
        if bytes[idx] == b'%' && idx + 2 < bytes.len() {
            let h1 = (bytes[idx + 1] as char).to_digit(16);
            let h2 = (bytes[idx + 2] as char).to_digit(16);
            if let (Some(a), Some(b)) = (h1, h2) {
                out.push(((a << 4) | b) as u8);
                idx += 3;
                continue;
            }
        }
        out.push(bytes[idx]);
        idx += 1;
    }
    out
}

pub(super) fn urllib_data_base64_val(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

pub(super) fn urllib_data_base64_decode(input: &[u8]) -> Result<Vec<u8>, String> {
    let compact: Vec<u8> = input
        .iter()
        .copied()
        .filter(|b| !(*b as char).is_ascii_whitespace())
        .collect();
    if compact.is_empty() {
        return Ok(Vec::new());
    }
    if !compact.len().is_multiple_of(4) {
        return Err("Invalid base64 data URL payload".to_string());
    }
    let mut out: Vec<u8> = Vec::with_capacity(compact.len() / 4 * 3);
    let mut idx = 0usize;
    while idx < compact.len() {
        let c0 = compact[idx];
        let c1 = compact[idx + 1];
        let c2 = compact[idx + 2];
        let c3 = compact[idx + 3];
        let Some(v0) = urllib_data_base64_val(c0) else {
            return Err("Invalid base64 data URL payload".to_string());
        };
        let Some(v1) = urllib_data_base64_val(c1) else {
            return Err("Invalid base64 data URL payload".to_string());
        };
        let pad2 = c2 == b'=';
        let pad3 = c3 == b'=';
        let v2 = if pad2 {
            0
        } else if let Some(v) = urllib_data_base64_val(c2) {
            v
        } else {
            return Err("Invalid base64 data URL payload".to_string());
        };
        let v3 = if pad3 {
            0
        } else if let Some(v) = urllib_data_base64_val(c3) {
            v
        } else {
            return Err("Invalid base64 data URL payload".to_string());
        };
        out.push((v0 << 2) | (v1 >> 4));
        if !pad2 {
            out.push(((v1 & 0x0F) << 4) | (v2 >> 2));
        }
        if !pad3 {
            out.push(((v2 & 0x03) << 6) | v3);
        }
        if pad2 && !pad3 {
            return Err("Invalid base64 data URL payload".to_string());
        }
        idx += 4;
    }
    Ok(out)
}

pub(super) fn urllib_request_decode_data_url(url: &str) -> Result<Vec<u8>, String> {
    let Some(payload) = url.strip_prefix("data:") else {
        return Err("unsupported URL scheme".to_string());
    };
    let Some((meta, raw_data)) = payload.split_once(',') else {
        return Err("Malformed data URL".to_string());
    };
    let percent_decoded = urllib_data_percent_decode(raw_data);
    let is_base64 = meta
        .split(';')
        .any(|item| item.eq_ignore_ascii_case("base64"));
    if is_base64 {
        urllib_data_base64_decode(&percent_decoded)
    } else {
        Ok(percent_decoded)
    }
}

pub(super) fn urllib_response_registry() -> &'static Mutex<HashMap<u64, MoltUrllibResponse>> {
    URLLIB_RESPONSE_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn urllib_response_from_parts(
    body: Vec<u8>,
    url: String,
    code: i64,
    reason: String,
    headers: Vec<(String, String)>,
) -> MoltUrllibResponse {
    let mut header_joined: HashMap<String, String> = HashMap::with_capacity(headers.len());
    for (name, value) in headers.iter() {
        let key = http_message_header_key(name);
        match header_joined.entry(key) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(value.clone());
            }
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                let joined = entry.get_mut();
                joined.push_str(", ");
                joined.push_str(value);
            }
        }
    }
    MoltUrllibResponse {
        body,
        pos: 0,
        closed: false,
        url,
        code,
        reason,
        headers,
        header_joined,
        headers_dict_cache: None,
        headers_list_cache: None,
    }
}

pub(super) fn urllib_response_joined_header<'a>(
    resp: &'a MoltUrllibResponse,
    name: &str,
) -> Option<&'a str> {
    resp.header_joined
        .get(&http_message_header_key(name))
        .map(String::as_str)
}

pub(super) fn urllib_response_headers_dict_bits(
    _py: &molt_runtime_core::CoreGilToken,
    resp: &mut MoltUrllibResponse,
) -> Result<u64, u64> {
    if let Some(bits) = resp.headers_dict_cache {
        inc_ref_bits(_py, bits);
        return Ok(bits);
    }
    let bits = urllib_http_headers_to_dict(_py, &resp.headers)?;
    resp.headers_dict_cache = Some(bits);
    inc_ref_bits(_py, bits);
    Ok(bits)
}

pub(super) fn urllib_response_headers_list_bits(
    _py: &molt_runtime_core::CoreGilToken,
    resp: &mut MoltUrllibResponse,
) -> Result<u64, u64> {
    if resp.headers_list_cache.is_none() {
        let bits = urllib_http_headers_to_list(_py, &resp.headers)?;
        resp.headers_list_cache = Some(bits);
    }
    let Some(cached_bits) = resp.headers_list_cache else {
        return Err(MoltObject::none().bits());
    };
    let Some(cached_ptr) = obj_from_bits(cached_bits).as_ptr() else {
        return Err(MoltObject::none().bits());
    };
    if unsafe { object_type_id(cached_ptr) } != crate::bridge::type_id_list() {
        return Err(raise_exception::<u64>(
            _py,
            "RuntimeError",
            "response headers cache is invalid",
        ));
    }
    let items = unsafe { seq_vec_ref(cached_ptr) };
    let list_ptr = alloc_list_with_capacity(_py, items.as_slice(), items.len());
    if list_ptr.is_null() {
        Err(MoltObject::none().bits())
    } else {
        Ok(MoltObject::from_ptr(list_ptr).bits())
    }
}

pub(super) fn urllib_response_store(response: MoltUrllibResponse) -> Option<i64> {
    let id = URLLIB_RESPONSE_NEXT.fetch_add(1, Ordering::Relaxed);
    let Ok(mut guard) = urllib_response_registry().lock() else {
        return None;
    };
    guard.insert(id, response);
    i64::try_from(id).ok()
}

pub(super) fn urllib_response_with_mut<T>(
    handle: i64,
    f: impl FnOnce(&mut MoltUrllibResponse) -> T,
) -> Option<T> {
    let Ok(mut guard) = urllib_response_registry().lock() else {
        return None;
    };
    guard.get_mut(&(handle as u64)).map(f)
}

pub(super) fn urllib_response_with<T>(
    handle: i64,
    f: impl FnOnce(&MoltUrllibResponse) -> T,
) -> Option<T> {
    let Ok(guard) = urllib_response_registry().lock() else {
        return None;
    };
    guard.get(&(handle as u64)).map(f)
}

pub(super) fn urllib_response_drop(_py: &molt_runtime_core::CoreGilToken, handle: i64) {
    if let Ok(mut guard) = urllib_response_registry().lock()
        && let Some(mut response) = guard.remove(&(handle as u64))
    {
        if let Some(bits) = response.headers_dict_cache.take() {
            dec_ref_bits(_py, bits);
        }
        if let Some(bits) = response.headers_list_cache.take() {
            dec_ref_bits(_py, bits);
        }
    }
}

pub(super) fn http_client_connection_runtime() -> &'static Mutex<MoltHttpClientConnectionRuntime> {
    HTTP_CLIENT_CONNECTION_RUNTIME.get_or_init(|| {
        Mutex::new(MoltHttpClientConnectionRuntime {
            next_handle: 1,
            connections: HashMap::new(),
        })
    })
}

pub(super) fn http_client_connection_store(
    host: String,
    port: u16,
    timeout: Option<f64>,
    use_tls: bool,
) -> Option<i64> {
    let Ok(mut guard) = http_client_connection_runtime().lock() else {
        return None;
    };
    let handle = guard.next_handle;
    guard.next_handle = guard.next_handle.saturating_add(1);
    guard.connections.insert(
        handle,
        MoltHttpClientConnection {
            host,
            port,
            timeout,
            method: None,
            url: None,
            headers: Vec::new(),
            body: Vec::new(),
            buffer: Vec::new(),
            skip_host: false,
            skip_accept_encoding: false,
            use_tls,
        },
    );
    i64::try_from(handle).ok()
}

pub(super) fn http_client_connection_with_mut<T>(
    handle: i64,
    f: impl FnOnce(&mut MoltHttpClientConnection) -> T,
) -> Option<T> {
    let Ok(mut guard) = http_client_connection_runtime().lock() else {
        return None;
    };
    guard.connections.get_mut(&(handle as u64)).map(f)
}

pub(super) fn http_client_connection_with<T>(
    handle: i64,
    f: impl FnOnce(&MoltHttpClientConnection) -> T,
) -> Option<T> {
    let Ok(guard) = http_client_connection_runtime().lock() else {
        return None;
    };
    guard.connections.get(&(handle as u64)).map(f)
}

pub(super) fn http_client_connection_drop(handle: i64) {
    if let Ok(mut guard) = http_client_connection_runtime().lock() {
        guard.connections.remove(&(handle as u64));
    }
}

pub(super) fn http_client_connection_reset_pending(conn: &mut MoltHttpClientConnection) {
    conn.method = None;
    conn.url = None;
    conn.headers.clear();
    conn.body.clear();
    conn.buffer.clear();
    conn.skip_host = false;
    conn.skip_accept_encoding = false;
}

pub(super) fn http_client_apply_default_headers(
    headers: &mut Vec<(String, String)>,
    host: &str,
    port: u16,
    skip_host: bool,
    skip_accept_encoding: bool,
) {
    if !skip_host
        && !headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("host"))
    {
        let host_value = if port == 80 {
            host.to_string()
        } else {
            format!("{host}:{port}")
        };
        headers.insert(0, ("Host".to_string(), host_value));
    }
    if !skip_accept_encoding
        && !headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("accept-encoding"))
    {
        headers.push(("Accept-Encoding".to_string(), "identity".to_string()));
    }
}

pub(super) fn http_client_alloc_buffer_list(
    _py: &molt_runtime_core::CoreGilToken,
    buffer: &[Vec<u8>],
) -> u64 {
    let mut item_bits: Vec<u64> = Vec::with_capacity(buffer.len());
    for chunk in buffer {
        let item_ptr = alloc_bytes(_py, chunk.as_slice());
        if item_ptr.is_null() {
            for bits in item_bits {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        item_bits.push(MoltObject::from_ptr(item_ptr).bits());
    }
    let list_ptr = alloc_list_with_capacity(_py, item_bits.as_slice(), item_bits.len());
    for bits in item_bits {
        dec_ref_bits(_py, bits);
    }
    if list_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(list_ptr).bits()
    }
}

pub(super) fn http_message_runtime() -> &'static Mutex<MoltHttpMessageRuntime> {
    HTTP_MESSAGE_RUNTIME.get_or_init(|| {
        Mutex::new(MoltHttpMessageRuntime {
            next_handle: 1,
            messages: HashMap::new(),
        })
    })
}

#[inline]
pub(super) fn http_message_header_key(name: &str) -> String {
    name.to_ascii_lowercase()
}

pub(super) fn http_message_from_headers(headers: Vec<(String, String)>) -> MoltHttpMessage {
    let mut index: HashMap<String, Vec<usize>> = HashMap::with_capacity(headers.len());
    for (idx, (name, _)) in headers.iter().enumerate() {
        index
            .entry(http_message_header_key(name))
            .or_default()
            .push(idx);
    }
    MoltHttpMessage {
        headers,
        index,
        items_list_cache: None,
    }
}

pub(super) fn http_message_push_header(
    _py: &molt_runtime_core::CoreGilToken,
    message: &mut MoltHttpMessage,
    name: String,
    value: String,
) {
    if let Some(cache_bits) = message.items_list_cache.take()
        && !obj_from_bits(cache_bits).is_none()
    {
        dec_ref_bits(_py, cache_bits);
    }
    let idx = message.headers.len();
    let key = http_message_header_key(name.as_str());
    message.headers.push((name, value));
    message.index.entry(key).or_default().push(idx);
}

pub(super) fn http_message_store(headers: Vec<(String, String)>) -> Option<i64> {
    let Ok(mut guard) = http_message_runtime().lock() else {
        return None;
    };
    let handle = guard.next_handle;
    guard.next_handle = guard.next_handle.saturating_add(1);
    guard
        .messages
        .insert(handle, http_message_from_headers(headers));
    i64::try_from(handle).ok()
}

pub(super) fn http_message_store_new() -> Option<i64> {
    http_message_store(Vec::new())
}

pub(super) fn http_message_with_mut<T>(
    handle: i64,
    f: impl FnOnce(&mut MoltHttpMessage) -> T,
) -> Option<T> {
    let Ok(mut guard) = http_message_runtime().lock() else {
        return None;
    };
    guard.messages.get_mut(&(handle as u64)).map(f)
}

pub(super) fn http_message_with<T>(
    handle: i64,
    f: impl FnOnce(&MoltHttpMessage) -> T,
) -> Option<T> {
    let Ok(guard) = http_message_runtime().lock() else {
        return None;
    };
    guard.messages.get(&(handle as u64)).map(f)
}

pub(super) fn http_message_drop(_py: &molt_runtime_core::CoreGilToken, handle: i64) {
    if let Ok(mut guard) = http_message_runtime().lock()
        && let Some(message) = guard.messages.remove(&(handle as u64))
        && let Some(cache_bits) = message.items_list_cache
        && !obj_from_bits(cache_bits).is_none()
    {
        dec_ref_bits(_py, cache_bits);
    }
}

pub(super) fn http_message_handle_from_bits(
    _py: &molt_runtime_core::CoreGilToken,
    handle_bits: u64,
) -> Result<i64, u64> {
    let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "http message handle is invalid",
        ));
    };
    if handle <= 0 {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "http message handle is invalid",
        ));
    }
    Ok(handle)
}

pub(super) fn http_message_values_to_list_from_indices(
    _py: &molt_runtime_core::CoreGilToken,
    message: &MoltHttpMessage,
    indices: &[usize],
) -> Result<u64, u64> {
    let mut item_bits: Vec<u64> = Vec::with_capacity(indices.len());
    for &idx in indices {
        let value_ptr = alloc_string(_py, message.headers[idx].1.as_bytes());
        if value_ptr.is_null() {
            for bits in item_bits {
                dec_ref_bits(_py, bits);
            }
            return Err(MoltObject::none().bits());
        }
        item_bits.push(MoltObject::from_ptr(value_ptr).bits());
    }
    let list_ptr = alloc_list_with_capacity(_py, item_bits.as_slice(), item_bits.len());
    for bits in item_bits {
        dec_ref_bits(_py, bits);
    }
    if list_ptr.is_null() {
        Err(MoltObject::none().bits())
    } else {
        Ok(MoltObject::from_ptr(list_ptr).bits())
    }
}

pub(super) fn cookiejar_registry() -> &'static Mutex<HashMap<u64, MoltCookieJar>> {
    COOKIEJAR_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn cookiejar_store_new() -> Option<i64> {
    let id = COOKIEJAR_NEXT.fetch_add(1, Ordering::Relaxed);
    let Ok(mut guard) = cookiejar_registry().lock() else {
        return None;
    };
    guard.insert(id, MoltCookieJar::default());
    i64::try_from(id).ok()
}

pub(super) fn cookiejar_with_mut<T>(
    handle: i64,
    f: impl FnOnce(&mut MoltCookieJar) -> T,
) -> Option<T> {
    let Ok(mut guard) = cookiejar_registry().lock() else {
        return None;
    };
    guard.get_mut(&(handle as u64)).map(f)
}

pub(super) fn cookiejar_with<T>(handle: i64, f: impl FnOnce(&MoltCookieJar) -> T) -> Option<T> {
    let Ok(guard) = cookiejar_registry().lock() else {
        return None;
    };
    guard.get(&(handle as u64)).map(f)
}

pub(super) fn urllib_cookiejar_domain_matches(host: &str, domain: &str) -> bool {
    let host = host.to_ascii_lowercase();
    let domain = domain.trim_start_matches('.').to_ascii_lowercase();
    host == domain || host.ends_with(&format!(".{domain}"))
}

pub(super) fn urllib_cookiejar_path_matches(request_path: &str, cookie_path: &str) -> bool {
    if cookie_path == "/" {
        return true;
    }
    if request_path == cookie_path {
        return true;
    }
    if !request_path.starts_with(cookie_path) {
        return false;
    }
    cookie_path.ends_with('/')
        || request_path
            .as_bytes()
            .get(cookie_path.len())
            .copied()
            .is_some_and(|b| b == b'/')
}

pub(super) fn urllib_cookiejar_default_scope(url: &str) -> (String, String) {
    let parts = urllib_urlsplit_impl(url, "", true);
    let host = urllib_http_parse_host_port(&parts[1], 80)
        .0
        .to_ascii_lowercase();
    let raw_path = if parts[2].is_empty() {
        "/".to_string()
    } else if parts[2].starts_with('/') {
        parts[2].clone()
    } else {
        format!("/{}", parts[2])
    };
    let path = match raw_path.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(idx) => raw_path[..idx].to_string(),
    };
    (host, path)
}

pub(super) fn urllib_cookiejar_parse_set_cookie(
    set_cookie_value: &str,
    default_domain: &str,
    default_path: &str,
) -> Option<(MoltCookieEntry, bool)> {
    let mut parts = set_cookie_value.split(';');
    let first = parts.next()?.trim();
    let (name_raw, value_raw) = first.split_once('=')?;
    let name = name_raw.trim();
    if name.is_empty() {
        return None;
    }
    let mut domain = default_domain.to_ascii_lowercase();
    let mut path = if default_path.is_empty() {
        "/".to_string()
    } else {
        default_path.to_string()
    };
    let mut delete_cookie = false;
    for attr in parts {
        let attr = attr.trim();
        if attr.is_empty() {
            continue;
        }
        let (key, value_opt) = match attr.split_once('=') {
            Some((k, v)) => (k.trim().to_ascii_lowercase(), Some(v.trim())),
            None => (attr.to_ascii_lowercase(), None),
        };
        match key.as_str() {
            "domain" => {
                if let Some(value) = value_opt {
                    let normalized = value.trim().trim_start_matches('.').to_ascii_lowercase();
                    if !normalized.is_empty() {
                        domain = normalized;
                    }
                }
            }
            "path" => {
                if let Some(value) = value_opt
                    && !value.is_empty()
                {
                    path = if value.starts_with('/') {
                        value.to_string()
                    } else {
                        format!("/{value}")
                    };
                }
            }
            "max-age" => {
                if let Some(value) = value_opt
                    && value == "0"
                {
                    delete_cookie = true;
                }
            }
            _ => {}
        }
    }
    Some((
        MoltCookieEntry {
            name: name.to_string(),
            value: value_raw.trim().to_string(),
            domain,
            path,
        },
        delete_cookie,
    ))
}

pub(super) fn urllib_cookiejar_store_from_headers(
    handle: i64,
    request_url: &str,
    headers: &[(String, String)],
) {
    let (default_domain, default_path) = urllib_cookiejar_default_scope(request_url);
    for (header_name, header_value) in headers {
        if !header_name.eq_ignore_ascii_case("Set-Cookie") {
            continue;
        }
        let Some((cookie, delete_cookie)) =
            urllib_cookiejar_parse_set_cookie(header_value, &default_domain, &default_path)
        else {
            continue;
        };
        let _ = cookiejar_with_mut(handle, |jar| {
            let same_cookie = |entry: &MoltCookieEntry| {
                entry.name == cookie.name
                    && entry.domain == cookie.domain
                    && entry.path == cookie.path
            };
            if delete_cookie || cookie.value.is_empty() {
                jar.cookies.retain(|entry| !same_cookie(entry));
                return;
            }
            if let Some(existing) = jar.cookies.iter_mut().find(|entry| same_cookie(entry)) {
                *existing = cookie;
            } else {
                jar.cookies.push(cookie);
            }
        });
    }
}

pub(super) fn urllib_cookiejar_header_for_url(handle: i64, request_url: &str) -> Option<String> {
    let parts = urllib_urlsplit_impl(request_url, "", true);
    let host = urllib_http_parse_host_port(&parts[1], 80)
        .0
        .to_ascii_lowercase();
    let path = if parts[2].is_empty() {
        "/".to_string()
    } else if parts[2].starts_with('/') {
        parts[2].clone()
    } else {
        format!("/{}", parts[2])
    };
    cookiejar_with(handle, |jar| {
        let mut pairs: Vec<String> = Vec::new();
        for entry in &jar.cookies {
            if urllib_cookiejar_domain_matches(&host, &entry.domain)
                && urllib_cookiejar_path_matches(&path, &entry.path)
            {
                pairs.push(format!("{}={}", entry.name, entry.value));
            }
        }
        if pairs.is_empty() {
            None
        } else {
            Some(pairs.join("; "))
        }
    })
    .flatten()
}

pub(super) fn http_cookies_parse_pairs(cookie_header: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for segment in cookie_header.split(';') {
        let segment = segment.trim();
        if segment.is_empty() {
            continue;
        }
        let Some((name_raw, value_raw)) = segment.split_once('=') else {
            continue;
        };
        let name = name_raw.trim();
        if name.is_empty() {
            continue;
        }
        out.push((name.to_string(), value_raw.trim().to_string()));
    }
    out
}

pub(super) fn http_cookies_attr_text(
    _py: &molt_runtime_core::CoreGilToken,
    value_bits: u64,
) -> Option<String> {
    if obj_from_bits(value_bits).is_none() {
        return None;
    }
    let text = crate::bridge::format_obj_str(_py, obj_from_bits(value_bits));
    if text.is_empty() { None } else { Some(text) }
}

pub(super) fn http_cookies_expires_text(
    _py: &molt_runtime_core::CoreGilToken,
    expires_bits: u64,
) -> Option<String> {
    if obj_from_bits(expires_bits).is_none() {
        return None;
    }
    if let Some(offset_seconds) = to_i64(obj_from_bits(expires_bits)) {
        let now = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => i64::try_from(duration.as_secs()).unwrap_or(i64::MAX),
            Err(_) => 0,
        };
        let absolute = now.saturating_add(offset_seconds);
        return Some(http_server_format_gmt_timestamp(absolute));
    }
    http_cookies_attr_text(_py, expires_bits)
}

pub(super) struct HttpCookieMorselInput {
    pub(super) name_bits: u64,
    pub(super) value_bits: u64,
    pub(super) path_bits: u64,
    pub(super) secure_bits: u64,
    pub(super) httponly_bits: u64,
    pub(super) max_age_bits: u64,
    pub(super) expires_bits: u64,
}

pub(super) fn http_cookies_render_morsel_impl(
    _py: &molt_runtime_core::CoreGilToken,
    input: HttpCookieMorselInput,
) -> String {
    let name = crate::bridge::format_obj_str(_py, obj_from_bits(input.name_bits));
    let value = crate::bridge::format_obj_str(_py, obj_from_bits(input.value_bits));
    let mut segments: Vec<String> = vec![format!("{name}={value}")];

    if let Some(expires_value) = http_cookies_expires_text(_py, input.expires_bits) {
        segments.push(format!("expires={expires_value}"));
    }

    if !obj_from_bits(input.httponly_bits).is_none()
        && is_truthy(_py, obj_from_bits(input.httponly_bits))
    {
        segments.push("HttpOnly".to_string());
    }

    if !obj_from_bits(input.max_age_bits).is_none() {
        if let Some(max_age_int) = to_i64(obj_from_bits(input.max_age_bits)) {
            segments.push(format!("Max-Age={max_age_int}"));
        } else if let Some(max_age_text) = http_cookies_attr_text(_py, input.max_age_bits) {
            segments.push(format!("Max-Age={max_age_text}"));
        }
    }

    if let Some(path_value) = http_cookies_attr_text(_py, input.path_bits) {
        segments.push(format!("Path={path_value}"));
    }

    if !obj_from_bits(input.secure_bits).is_none()
        && is_truthy(_py, obj_from_bits(input.secure_bits))
    {
        segments.push("Secure".to_string());
    }

    segments.join("; ")
}

pub(super) struct HttpClientExecuteInput {
    pub(super) host: String,
    pub(super) port: u16,
    pub(super) timeout: Option<f64>,
    pub(super) method: String,
    pub(super) url: String,
    pub(super) headers: Vec<(String, String)>,
    pub(super) body: Vec<u8>,
    pub(super) skip_host: bool,
    pub(super) skip_accept_encoding: bool,
    /// When true, the request is sent over TLS (HTTPS). The SNI server name is
    /// taken from `host`.
    pub(super) use_tls: bool,
}

pub(super) fn urllib_http_extract_headers_mapping(
    _py: &molt_runtime_core::CoreGilToken,
    mapping_bits: u64,
) -> Result<Vec<(String, String)>, u64> {
    if obj_from_bits(mapping_bits).is_none() {
        return Ok(Vec::new());
    }
    let Some(items_name_bits) = attr_name_bits_from_bytes(_py, b"items") else {
        return Err(MoltObject::none().bits());
    };
    let missing = missing_bits(_py);
    let items_method_bits = molt_getattr_builtin(mapping_bits, items_name_bits, missing);
    dec_ref_bits(_py, items_name_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if items_method_bits == missing || !molt_is_callable(items_method_bits) {
        if items_method_bits != missing {
            dec_ref_bits(_py, items_method_bits);
        }
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "headers must be a mapping",
        ));
    }
    let iterable_bits = unsafe { call_callable0(_py, items_method_bits) };
    dec_ref_bits(_py, items_method_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let iter_bits = molt_iter(iterable_bits);
    dec_ref_bits(_py, iterable_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let mut out: Vec<(String, String)> = Vec::new();
    loop {
        let (item_bits, done) = iter_next_pair(_py, iter_bits)?;
        if done {
            break;
        }
        let Some(item_ptr) = obj_from_bits(item_bits).as_ptr() else {
            dec_ref_bits(_py, item_bits);
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "headers mapping items must be pairs",
            ));
        };
        let item_type = unsafe { object_type_id(item_ptr) };
        if item_type != crate::bridge::type_id_list() && item_type != crate::bridge::type_id_tuple()
        {
            dec_ref_bits(_py, item_bits);
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "headers mapping items must be pairs",
            ));
        }
        let fields = unsafe { seq_vec_ref(item_ptr) };
        if fields.len() != 2 {
            dec_ref_bits(_py, item_bits);
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "headers mapping items must be pairs",
            ));
        }
        out.push((
            crate::bridge::format_obj_str(_py, obj_from_bits(fields[0])),
            crate::bridge::format_obj_str(_py, obj_from_bits(fields[1])),
        ));
        dec_ref_bits(_py, item_bits);
    }
    Ok(out)
}

pub(super) fn urllib_cookiejar_handles_from_handlers(
    _py: &molt_runtime_core::CoreGilToken,
    handlers: &[u64],
) -> Result<Vec<i64>, u64> {
    let mut out: Vec<i64> = Vec::new();
    let mut seen: HashSet<i64> = HashSet::new();
    for handler_bits in handlers {
        let Some(cookiejar_bits) = urllib_request_attr_optional(_py, *handler_bits, b"cookiejar")?
        else {
            continue;
        };
        if obj_from_bits(cookiejar_bits).is_none() {
            dec_ref_bits(_py, cookiejar_bits);
            continue;
        }
        let handle_opt =
            match urllib_request_attr_optional(_py, cookiejar_bits, b"_molt_cookiejar_handle") {
                Ok(value) => value,
                Err(bits) => {
                    dec_ref_bits(_py, cookiejar_bits);
                    return Err(bits);
                }
            };
        dec_ref_bits(_py, cookiejar_bits);
        let Some(handle_bits) = handle_opt else {
            continue;
        };
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            dec_ref_bits(_py, handle_bits);
            continue;
        };
        dec_ref_bits(_py, handle_bits);
        if seen.insert(handle) {
            out.push(handle);
        }
    }
    Ok(out)
}

pub(super) fn urllib_cookiejar_apply_header_for_url(
    _py: &molt_runtime_core::CoreGilToken,
    cookiejar_handles: &[i64],
    request_url: &str,
    headers: &mut Vec<(String, String)>,
) {
    for handle in cookiejar_handles {
        let Some(cookie_header) = urllib_cookiejar_header_for_url(*handle, request_url) else {
            continue;
        };
        let mut replaced = false;
        for (name, value) in headers.iter_mut() {
            if name.eq_ignore_ascii_case("Cookie") {
                if value.is_empty() {
                    *value = cookie_header.clone();
                } else {
                    *value = format!("{value}; {cookie_header}");
                }
                replaced = true;
                break;
            }
        }
        if !replaced {
            headers.push(("Cookie".to_string(), cookie_header));
        }
    }
}

pub(super) fn urllib_cookiejar_store_headers_for_url(
    cookiejar_handles: &[i64],
    request_url: &str,
    response_headers: &[(String, String)],
) {
    for handle in cookiejar_handles {
        urllib_cookiejar_store_from_headers(*handle, request_url, response_headers);
    }
}

pub(super) fn urllib_http_timeout_error(_py: &molt_runtime_core::CoreGilToken) -> u64 {
    raise_exception::<_>(_py, "TimeoutError", "timed out")
}

pub(super) fn urllib_http_request_timeout(
    _py: &molt_runtime_core::CoreGilToken,
    request_bits: u64,
) -> Result<Option<f64>, u64> {
    let Some(timeout_bits) = urllib_request_attr_optional(_py, request_bits, b"timeout")? else {
        return Ok(None);
    };
    if obj_from_bits(timeout_bits).is_none() {
        dec_ref_bits(_py, timeout_bits);
        return Ok(None);
    }
    let timeout = to_f64(obj_from_bits(timeout_bits))
        .or_else(|| to_i64(obj_from_bits(timeout_bits)).map(|v| v as f64));
    dec_ref_bits(_py, timeout_bits);
    let Some(value) = timeout else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "timeout must be a number",
        ));
    };
    if value < 0.0 {
        return Err(raise_exception::<u64>(
            _py,
            "ValueError",
            "timeout value out of range",
        ));
    }
    Ok(Some(value))
}

pub(super) fn urllib_http_host_matches_no_proxy(host: &str, no_proxy: &str) -> bool {
    let normalized_host = host.trim().to_ascii_lowercase();
    if normalized_host.is_empty() {
        return false;
    }
    for raw in no_proxy.split(',') {
        let token = raw.trim().to_ascii_lowercase();
        if token.is_empty() {
            continue;
        }
        if token == "*" {
            return true;
        }
        let needle = token.strip_prefix('.').unwrap_or(&token);
        if needle.is_empty() {
            continue;
        }
        if normalized_host == needle || normalized_host.ends_with(&format!(".{needle}")) {
            return true;
        }
    }
    false
}

pub(super) fn urllib_http_parse_host_port(netloc: &str, default_port: u16) -> (String, u16) {
    let without_user = netloc.rsplit('@').next().unwrap_or(netloc);
    if without_user.starts_with('[')
        && let Some(end) = without_user.find(']')
    {
        let host = without_user[1..end].to_string();
        if let Some(port_part) = without_user[end + 1..].strip_prefix(':')
            && let Ok(port) = port_part.parse::<u16>()
        {
            return (host, port);
        }
        return (host, default_port);
    }
    if let Some((host, port_part)) = without_user.rsplit_once(':')
        && !host.is_empty()
        && !port_part.is_empty()
        && !host.contains(':')
        && let Ok(port) = port_part.parse::<u16>()
    {
        return (host.to_string(), port);
    }
    (without_user.to_string(), default_port)
}

pub(super) fn urllib_http_join_url(base: &str, target: &str) -> String {
    if target.starts_with("http://")
        || target.starts_with("https://")
        || target.starts_with("data:")
    {
        return target.to_string();
    }
    let base_parts = urllib_urlsplit_impl(base, "", true);
    if target.starts_with("//") {
        return format!("{}:{}", base_parts[0], target);
    }
    if target.starts_with('/') {
        return urllib_unsplit_impl(&base_parts[0], &base_parts[1], target, "", "");
    }
    let base_path = &base_parts[2];
    let base_dir = match base_path.rsplit_once('/') {
        Some((dir, _)) => dir,
        None => "",
    };
    let joined = if base_dir.is_empty() {
        format!("/{}", target)
    } else {
        format!("{base_dir}/{target}")
    };
    urllib_unsplit_impl(&base_parts[0], &base_parts[1], &joined, "", "")
}

pub(super) fn urllib_http_headers_to_dict(
    _py: &molt_runtime_core::CoreGilToken,
    headers: &[(String, String)],
) -> Result<u64, u64> {
    let mut pair_bits: Vec<u64> = Vec::with_capacity(headers.len().saturating_mul(2));
    for (name, value) in headers {
        let name_ptr = alloc_string(_py, name.as_bytes());
        if name_ptr.is_null() {
            for bits in pair_bits {
                dec_ref_bits(_py, bits);
            }
            return Err(MoltObject::none().bits());
        }
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let value_ptr = alloc_string(_py, value.as_bytes());
        if value_ptr.is_null() {
            dec_ref_bits(_py, name_bits);
            for bits in pair_bits {
                dec_ref_bits(_py, bits);
            }
            return Err(MoltObject::none().bits());
        }
        let value_bits = MoltObject::from_ptr(value_ptr).bits();
        pair_bits.push(name_bits);
        pair_bits.push(value_bits);
    }
    let dict = alloc_dict_with_pairs(_py, &pair_bits);
    for bits in pair_bits {
        dec_ref_bits(_py, bits);
    }
    if dict.is_null() {
        Err(MoltObject::none().bits())
    } else {
        Ok(MoltObject::from_ptr(dict).bits())
    }
}

pub(super) fn urllib_http_headers_to_list(
    _py: &molt_runtime_core::CoreGilToken,
    headers: &[(String, String)],
) -> Result<u64, u64> {
    let mut tuple_bits: Vec<u64> = Vec::with_capacity(headers.len());
    for (name, value) in headers {
        let name_ptr = alloc_string(_py, name.as_bytes());
        if name_ptr.is_null() {
            for bits in tuple_bits {
                dec_ref_bits(_py, bits);
            }
            return Err(MoltObject::none().bits());
        }
        let value_ptr = alloc_string(_py, value.as_bytes());
        if value_ptr.is_null() {
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            dec_ref_bits(_py, name_bits);
            for bits in tuple_bits {
                dec_ref_bits(_py, bits);
            }
            return Err(MoltObject::none().bits());
        }
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let value_bits = MoltObject::from_ptr(value_ptr).bits();
        let pair_ptr = alloc_tuple(_py, &[name_bits, value_bits]);
        dec_ref_bits(_py, name_bits);
        dec_ref_bits(_py, value_bits);
        if pair_ptr.is_null() {
            for bits in tuple_bits {
                dec_ref_bits(_py, bits);
            }
            return Err(MoltObject::none().bits());
        }
        tuple_bits.push(MoltObject::from_ptr(pair_ptr).bits());
    }
    let list_ptr = alloc_list_with_capacity(_py, tuple_bits.as_slice(), tuple_bits.len());
    for bits in tuple_bits {
        dec_ref_bits(_py, bits);
    }
    if list_ptr.is_null() {
        Err(MoltObject::none().bits())
    } else {
        Ok(MoltObject::from_ptr(list_ptr).bits())
    }
}

pub(super) fn http_client_extract_headers(
    _py: &molt_runtime_core::CoreGilToken,
    headers_bits: u64,
) -> Result<Vec<(String, String)>, u64> {
    if obj_from_bits(headers_bits).is_none() {
        return Ok(Vec::new());
    }
    let iter_bits = molt_iter(headers_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let mut out: Vec<(String, String)> = Vec::new();
    loop {
        let (item_bits, done) = iter_next_pair(_py, iter_bits)?;
        if done {
            break;
        }
        let Some(item_ptr) = obj_from_bits(item_bits).as_ptr() else {
            dec_ref_bits(_py, item_bits);
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "headers must be an iterable of pairs",
            ));
        };
        let is_sequence = unsafe {
            let ty = object_type_id(item_ptr);
            ty == crate::bridge::type_id_tuple() || ty == crate::bridge::type_id_list()
        };
        if !is_sequence {
            dec_ref_bits(_py, item_bits);
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "header entries must be (name, value) pairs",
            ));
        }
        let pair = unsafe { seq_vec_ref(item_ptr) };
        if pair.len() < 2 {
            dec_ref_bits(_py, item_bits);
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "header entries must be (name, value) pairs",
            ));
        }
        let Some(name) = string_obj_to_owned(obj_from_bits(pair[0])) else {
            dec_ref_bits(_py, item_bits);
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "header name must be str",
            ));
        };
        let value = crate::bridge::format_obj_str(_py, obj_from_bits(pair[1]));
        dec_ref_bits(_py, item_bits);
        out.push((name, value));
    }
    Ok(out)
}

pub(super) fn http_client_response_handle_from_bits(
    _py: &molt_runtime_core::CoreGilToken,
    handle_bits: u64,
) -> Result<i64, u64> {
    let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "response handle is invalid",
        ));
    };
    Ok(handle)
}

pub(super) fn http_client_connection_handle_from_bits(
    _py: &molt_runtime_core::CoreGilToken,
    handle_bits: u64,
) -> Result<i64, u64> {
    let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "connection handle is invalid",
        ));
    };
    if handle <= 0 {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "connection handle is invalid",
        ));
    }
    Ok(handle)
}

pub(super) fn http_client_execute_request(
    _py: &molt_runtime_core::CoreGilToken,
    mut input: HttpClientExecuteInput,
) -> Result<i64, u64> {
    http_client_apply_default_headers(
        &mut input.headers,
        input.host.as_str(),
        input.port,
        input.skip_host,
        input.skip_accept_encoding,
    );
    let request_target = if input.url.is_empty() {
        "/".to_string()
    } else {
        input.url.clone()
    };
    let default_port: u16 = if input.use_tls { 443 } else { 80 };
    let host_header = if input.port == default_port {
        input.host.clone()
    } else {
        format!("{}:{}", input.host, input.port)
    };
    let tls_server_name = if input.use_tls {
        Some(input.host.clone())
    } else {
        None
    };
    let req = UrllibHttpRequest {
        host: input.host.clone(),
        port: input.port,
        path: request_target.clone(),
        method: input.method,
        headers: input.headers,
        body: input.body,
        timeout: input.timeout,
        tls_server_name,
    };
    let (code, reason, resp_headers, resp_body) =
        match urllib_http_try_inmemory_dispatch(_py, &req, &request_target, &host_header) {
            Ok(Some(value)) => value,
            Ok(None) => match urllib_http_send_request(&req, &request_target, &host_header) {
                Ok(value) => value,
                Err(err) => {
                    if err.kind() == ErrorKind::TimedOut || err.kind() == ErrorKind::WouldBlock {
                        return Err(raise_exception::<u64>(_py, "TimeoutError", "timed out"));
                    }
                    return Err(raise_exception::<u64>(_py, "OSError", &err.to_string()));
                }
            },
            Err(bits) => return Err(bits),
        };
    let scheme_prefix = if input.use_tls { "https" } else { "http" };
    let response_url = if input.url.starts_with("http://") || input.url.starts_with("https://") {
        input.url
    } else if request_target.starts_with('/') {
        format!("{scheme_prefix}://{host_header}{request_target}")
    } else {
        format!("{scheme_prefix}://{host_header}/{request_target}")
    };
    let Some(handle) = urllib_response_store(urllib_response_from_parts(
        resp_body,
        response_url,
        code,
        reason,
        resp_headers,
    )) else {
        return Err(MoltObject::none().bits());
    };
    Ok(handle)
}

pub(super) fn urllib_http_extract_request_headers(
    _py: &molt_runtime_core::CoreGilToken,
    request_bits: u64,
) -> Result<Vec<(String, String)>, u64> {
    let Some(headers_bits) = urllib_request_attr_optional(_py, request_bits, b"headers")? else {
        return Ok(Vec::new());
    };
    if obj_from_bits(headers_bits).is_none() {
        dec_ref_bits(_py, headers_bits);
        return Ok(Vec::new());
    }
    let Some(items_name_bits) = attr_name_bits_from_bytes(_py, b"items") else {
        dec_ref_bits(_py, headers_bits);
        return Err(MoltObject::none().bits());
    };
    let missing = missing_bits(_py);
    let items_method_bits = molt_getattr_builtin(headers_bits, items_name_bits, missing);
    dec_ref_bits(_py, headers_bits);
    dec_ref_bits(_py, items_name_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if items_method_bits == missing || !molt_is_callable(items_method_bits) {
        if items_method_bits != missing {
            dec_ref_bits(_py, items_method_bits);
        }
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "headers must be a mapping",
        ));
    }
    let iterable_bits = unsafe { call_callable0(_py, items_method_bits) };
    dec_ref_bits(_py, items_method_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let iter_bits = molt_iter(iterable_bits);
    dec_ref_bits(_py, iterable_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let mut out: Vec<(String, String)> = Vec::new();
    loop {
        let (item_bits, done) = iter_next_pair(_py, iter_bits)?;
        if done {
            break;
        }
        let Some(item_ptr) = obj_from_bits(item_bits).as_ptr() else {
            dec_ref_bits(_py, item_bits);
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "headers mapping items must be pairs",
            ));
        };
        let item_type = unsafe { object_type_id(item_ptr) };
        if item_type != crate::bridge::type_id_list() && item_type != crate::bridge::type_id_tuple()
        {
            dec_ref_bits(_py, item_bits);
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "headers mapping items must be pairs",
            ));
        }
        let fields = unsafe { seq_vec_ref(item_ptr) };
        if fields.len() != 2 {
            dec_ref_bits(_py, item_bits);
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "headers mapping items must be pairs",
            ));
        }
        out.push((
            crate::bridge::format_obj_str(_py, obj_from_bits(fields[0])),
            crate::bridge::format_obj_str(_py, obj_from_bits(fields[1])),
        ));
        dec_ref_bits(_py, item_bits);
    }
    Ok(out)
}

pub(super) fn urllib_http_extract_method_and_body(
    _py: &molt_runtime_core::CoreGilToken,
    request_bits: u64,
) -> Result<(String, Vec<u8>), u64> {
    let body = match urllib_request_attr_optional(_py, request_bits, b"data")? {
        Some(bits) if !obj_from_bits(bits).is_none() => {
            let Some(ptr) = obj_from_bits(bits).as_ptr() else {
                dec_ref_bits(_py, bits);
                return Err(raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "Request data must be bytes-like",
                ));
            };
            let Some(bytes) = (unsafe { bytes_like_slice(MoltObject::from_ptr(ptr).bits()) })
            else {
                dec_ref_bits(_py, bits);
                return Err(raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "Request data must be bytes-like",
                ));
            };
            let payload = bytes.to_vec();
            dec_ref_bits(_py, bits);
            payload
        }
        Some(bits) => {
            dec_ref_bits(_py, bits);
            Vec::new()
        }
        None => Vec::new(),
    };
    let method = match urllib_request_attr_optional(_py, request_bits, b"method")? {
        Some(bits) if !obj_from_bits(bits).is_none() => {
            let value = crate::bridge::format_obj_str(_py, obj_from_bits(bits));
            dec_ref_bits(_py, bits);
            value
        }
        Some(bits) => {
            dec_ref_bits(_py, bits);
            String::new()
        }
        None => String::new(),
    };
    let normalized = if method.trim().is_empty() {
        if body.is_empty() {
            "GET".to_string()
        } else {
            "POST".to_string()
        }
    } else {
        method
    };
    Ok((normalized, body))
}

pub(super) fn urllib_http_find_proxy_for_scheme(
    _py: &molt_runtime_core::CoreGilToken,
    opener_bits: u64,
    scheme: &str,
    host: &str,
) -> Result<Option<String>, u64> {
    let mut proxy: Option<String> = None;
    let mut saw_proxy_handler = false;
    let list_bits = urllib_request_ensure_handlers_list(_py, opener_bits)?;
    let Some(list_ptr) = obj_from_bits(list_bits).as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "opener handler registry is invalid",
        ));
    };
    let handlers: Vec<u64> = unsafe { seq_vec_ref(list_ptr).to_vec() };
    for handler_bits in handlers {
        let Some(proxies_bits) = urllib_request_attr_optional(_py, handler_bits, b"proxies")?
        else {
            continue;
        };
        saw_proxy_handler = true;
        if obj_from_bits(proxies_bits).is_none() {
            dec_ref_bits(_py, proxies_bits);
            continue;
        }
        let Some(get_name_bits) = attr_name_bits_from_bytes(_py, b"get") else {
            dec_ref_bits(_py, proxies_bits);
            return Err(MoltObject::none().bits());
        };
        let missing = missing_bits(_py);
        let get_method_bits = molt_getattr_builtin(proxies_bits, get_name_bits, missing);
        dec_ref_bits(_py, get_name_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, proxies_bits);
            return Err(MoltObject::none().bits());
        }
        if get_method_bits == missing || !molt_is_callable(get_method_bits) {
            if get_method_bits != missing {
                dec_ref_bits(_py, get_method_bits);
            }
            dec_ref_bits(_py, proxies_bits);
            continue;
        }
        let key_ptr = alloc_string(_py, scheme.as_bytes());
        if key_ptr.is_null() {
            dec_ref_bits(_py, get_method_bits);
            dec_ref_bits(_py, proxies_bits);
            return Err(MoltObject::none().bits());
        }
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        let out_bits = unsafe { call_callable1(_py, get_method_bits, key_bits) };
        dec_ref_bits(_py, key_bits);
        dec_ref_bits(_py, get_method_bits);
        dec_ref_bits(_py, proxies_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        if !obj_from_bits(out_bits).is_none() {
            proxy = Some(crate::bridge::format_obj_str(_py, obj_from_bits(out_bits)));
            dec_ref_bits(_py, out_bits);
            break;
        }
        dec_ref_bits(_py, out_bits);
    }
    if proxy.is_none() && !saw_proxy_handler {
        let env_key = format!("{}_proxy", scheme.to_ascii_lowercase());
        proxy = env_state_get(&env_key).or_else(|| env_state_get(&env_key.to_ascii_uppercase()));
    }
    let no_proxy = env_state_get("no_proxy").or_else(|| env_state_get("NO_PROXY"));
    if let (Some(rule), Some(_proxy_url)) = (no_proxy.as_deref(), proxy.as_ref())
        && urllib_http_host_matches_no_proxy(host, rule)
    {
        proxy = None;
    }
    Ok(proxy)
}

pub(super) fn urllib_base64_encode(input: &[u8]) -> String {
    pub(super) const TABLE: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    if input.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    let mut idx = 0usize;
    while idx < input.len() {
        let b0 = input[idx];
        let b1 = if idx + 1 < input.len() {
            input[idx + 1]
        } else {
            0
        };
        let b2 = if idx + 2 < input.len() {
            input[idx + 2]
        } else {
            0
        };
        let n = ((b0 as u32) << 16) | ((b1 as u32) << 8) | (b2 as u32);
        out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
        if idx + 1 < input.len() {
            out.push(TABLE[((n >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if idx + 2 < input.len() {
            out.push(TABLE[(n & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        idx += 3;
    }
    out
}

pub(super) fn urllib_http_parse_basic_realm(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if !trimmed.to_ascii_lowercase().starts_with("basic") {
        return None;
    }
    let rest = trimmed.get(5..)?.trim();
    if rest.is_empty() {
        return None;
    }
    for part in rest.split(',') {
        let Some((key, val)) = part.split_once('=') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case("realm") {
            let raw = val.trim();
            if raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"') {
                return Some(raw[1..raw.len() - 1].to_string());
            }
            return Some(raw.to_string());
        }
    }
    None
}

pub(super) fn urllib_proxy_find_basic_credentials(
    _py: &molt_runtime_core::CoreGilToken,
    handlers: &[u64],
    proxy_url: &str,
    realm: Option<&str>,
) -> Result<Option<(String, String)>, u64> {
    for handler_bits in handlers {
        let Some(passwd_bits) = urllib_request_attr_optional(_py, *handler_bits, b"passwd")? else {
            continue;
        };
        if obj_from_bits(passwd_bits).is_none() {
            dec_ref_bits(_py, passwd_bits);
            continue;
        }
        let Some(find_name_bits) = attr_name_bits_from_bytes(_py, b"find_user_password") else {
            dec_ref_bits(_py, passwd_bits);
            return Err(MoltObject::none().bits());
        };
        let missing = missing_bits(_py);
        let find_bits = molt_getattr_builtin(passwd_bits, find_name_bits, missing);
        dec_ref_bits(_py, find_name_bits);
        dec_ref_bits(_py, passwd_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        if find_bits == missing || !molt_is_callable(find_bits) {
            if find_bits != missing {
                dec_ref_bits(_py, find_bits);
            }
            continue;
        }
        let realm_bits = if let Some(realm_value) = realm {
            let realm_ptr = alloc_string(_py, realm_value.as_bytes());
            if realm_ptr.is_null() {
                dec_ref_bits(_py, find_bits);
                return Err(MoltObject::none().bits());
            }
            MoltObject::from_ptr(realm_ptr).bits()
        } else {
            MoltObject::none().bits()
        };
        let proxy_ptr = alloc_string(_py, proxy_url.as_bytes());
        if proxy_ptr.is_null() {
            if !obj_from_bits(realm_bits).is_none() {
                dec_ref_bits(_py, realm_bits);
            }
            dec_ref_bits(_py, find_bits);
            return Err(MoltObject::none().bits());
        }
        let proxy_bits = MoltObject::from_ptr(proxy_ptr).bits();
        let creds_bits = unsafe { call_callable2(_py, find_bits, realm_bits, proxy_bits) };
        dec_ref_bits(_py, proxy_bits);
        if !obj_from_bits(realm_bits).is_none() {
            dec_ref_bits(_py, realm_bits);
        }
        dec_ref_bits(_py, find_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        if obj_from_bits(creds_bits).is_none() {
            continue;
        }
        let Some(creds_ptr) = obj_from_bits(creds_bits).as_ptr() else {
            dec_ref_bits(_py, creds_bits);
            continue;
        };
        let ty = unsafe { object_type_id(creds_ptr) };
        if ty != crate::bridge::type_id_tuple() && ty != crate::bridge::type_id_list() {
            dec_ref_bits(_py, creds_bits);
            continue;
        }
        let fields = unsafe { seq_vec_ref(creds_ptr) };
        if fields.len() != 2
            || obj_from_bits(fields[0]).is_none()
            || obj_from_bits(fields[1]).is_none()
        {
            dec_ref_bits(_py, creds_bits);
            continue;
        }
        let user = crate::bridge::format_obj_str(_py, obj_from_bits(fields[0]));
        let pass = crate::bridge::format_obj_str(_py, obj_from_bits(fields[1]));
        dec_ref_bits(_py, creds_bits);
        return Ok(Some((user, pass)));
    }
    Ok(None)
}

pub(super) fn urllib_http_find_header<'a>(
    headers: &'a [(String, String)],
    name: &str,
) -> Option<&'a str> {
    for (key, value) in headers.iter().rev() {
        if key.eq_ignore_ascii_case(name) {
            return Some(value.as_str());
        }
    }
    None
}

type HttpResponseParts = (i64, String, Vec<(String, String)>, Vec<u8>);

pub(super) fn urllib_http_parse_response_bytes(raw: &[u8]) -> Result<HttpResponseParts, String> {
    let marker = b"\r\n\r\n";
    let Some(split) = raw.windows(marker.len()).position(|w| w == marker) else {
        return Err("Malformed HTTP response".to_string());
    };
    let head = &raw[..split];
    let mut body = raw[split + marker.len()..].to_vec();
    let head_text = String::from_utf8_lossy(head);
    let mut lines = head_text.split("\r\n");
    let Some(status_line) = lines.next() else {
        return Err("Malformed HTTP response".to_string());
    };
    let mut parts = status_line.splitn(3, ' ');
    let _http_version = parts.next().unwrap_or("");
    let code = parts
        .next()
        .and_then(|v| v.parse::<i64>().ok())
        .ok_or_else(|| "Malformed HTTP status line".to_string())?;
    let reason = parts.next().unwrap_or("").to_string();
    let mut headers: Vec<(String, String)> = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
    }
    if let Some(length) =
        urllib_http_find_header(&headers, "Content-Length").and_then(|v| v.parse::<usize>().ok())
    {
        if body.len() < length {
            return Err("Incomplete HTTP response body".to_string());
        }
        if body.len() > length {
            body.truncate(length);
        }
    }
    Ok((code, reason, headers, body))
}

pub(super) fn http_parse_header_pairs(raw: &[u8]) -> Vec<(String, String)> {
    let text = String::from_utf8_lossy(raw);
    let mut out: Vec<(String, String)> = Vec::new();
    let mut current_name: Option<String> = None;
    let mut current_value = String::new();

    for raw_line in text.split('\n') {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if line.is_empty() {
            break;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            if current_name.is_some() {
                if !current_value.is_empty() {
                    current_value.push(' ');
                }
                current_value.push_str(line.trim());
            }
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            if let Some(prev_name) = current_name.take() {
                out.push((prev_name, current_value.trim().to_string()));
            }
            current_name = Some(name.trim().to_string());
            current_value.clear();
            current_value.push_str(value.trim_start());
        }
    }

    if let Some(last_name) = current_name {
        out.push((last_name, current_value.trim().to_string()));
    }
    out
}

pub(super) fn urllib_http_build_request_bytes(
    req: &UrllibHttpRequest,
    request_target: &str,
    host_header: &str,
) -> Vec<u8> {
    let mut request = Vec::<u8>::new();
    request.extend_from_slice(format!("{} {} HTTP/1.1\r\n", req.method, request_target).as_bytes());
    let mut has_host = false;
    let mut has_connection = false;
    let mut has_content_length = false;
    for (name, value) in &req.headers {
        if name.eq_ignore_ascii_case("Host") {
            has_host = true;
        }
        if name.eq_ignore_ascii_case("Connection") {
            has_connection = true;
        }
        if name.eq_ignore_ascii_case("Content-Length") {
            has_content_length = true;
        }
        request.extend_from_slice(name.as_bytes());
        request.extend_from_slice(b": ");
        request.extend_from_slice(value.as_bytes());
        request.extend_from_slice(b"\r\n");
    }
    if !has_host {
        request.extend_from_slice(b"Host: ");
        request.extend_from_slice(host_header.as_bytes());
        request.extend_from_slice(b"\r\n");
    }
    if !has_connection {
        request.extend_from_slice(b"Connection: close\r\n");
    }
    if !req.body.is_empty() && !has_content_length {
        request.extend_from_slice(format!("Content-Length: {}\r\n", req.body.len()).as_bytes());
    }
    request.extend_from_slice(b"\r\n");
    request.extend_from_slice(&req.body);
    request
}

pub(super) fn urllib_http_try_inmemory_dispatch(
    _py: &molt_runtime_core::CoreGilToken,
    req: &UrllibHttpRequest,
    request_target: &str,
    host_header: &str,
) -> Result<Option<HttpResponseParts>, u64> {
    let module_name_ptr = alloc_string(_py, b"socketserver");
    if module_name_ptr.is_null() {
        return Err(MoltObject::none().bits());
    }
    let module_name_bits = MoltObject::from_ptr(module_name_ptr).bits();
    let module_bits = crate::bridge::molt_module_import(module_name_bits);
    dec_ref_bits(_py, module_name_bits);
    if exception_pending(_py) {
        let kind = urllib_request_pending_exception_kind_name(_py).unwrap_or_default();
        if kind == "ImportError" || kind == "TypeError" {
            clear_exception(_py);
            if !obj_from_bits(module_bits).is_none() {
                dec_ref_bits(_py, module_bits);
            }
            return Ok(None);
        }
        return Err(MoltObject::none().bits());
    }
    let Some(module_ptr) = obj_from_bits(module_bits).as_ptr() else {
        if !obj_from_bits(module_bits).is_none() {
            dec_ref_bits(_py, module_bits);
        }
        return Ok(None);
    };
    if unsafe { object_type_id(module_ptr) } != crate::bridge::type_id_module() {
        dec_ref_bits(_py, module_bits);
        return Ok(None);
    }

    let Some(lookup_name_bits) = attr_name_bits_from_bytes(_py, b"_lookup_server") else {
        dec_ref_bits(_py, module_bits);
        return Err(MoltObject::none().bits());
    };
    let missing = missing_bits(_py);
    let lookup_bits = molt_getattr_builtin(module_bits, lookup_name_bits, missing);
    dec_ref_bits(_py, lookup_name_bits);
    if exception_pending(_py) {
        dec_ref_bits(_py, module_bits);
        return Err(MoltObject::none().bits());
    }
    if lookup_bits == missing {
        dec_ref_bits(_py, module_bits);
        return Ok(None);
    }

    let host_ptr = alloc_string(_py, req.host.as_bytes());
    if host_ptr.is_null() {
        dec_ref_bits(_py, lookup_bits);
        dec_ref_bits(_py, module_bits);
        return Err(MoltObject::none().bits());
    }
    let host_bits = MoltObject::from_ptr(host_ptr).bits();
    let port_bits = MoltObject::from_int(req.port as i64).bits();
    let server_bits = unsafe { call_callable2(_py, lookup_bits, host_bits, port_bits) };
    dec_ref_bits(_py, host_bits);
    dec_ref_bits(_py, lookup_bits);
    dec_ref_bits(_py, module_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if obj_from_bits(server_bits).is_none() {
        return Ok(None);
    }

    let Some(dispatch_bits) = (match urllib_request_attr_optional(_py, server_bits, b"_dispatch") {
        Ok(value) => value,
        Err(bits) => {
            dec_ref_bits(_py, server_bits);
            return Err(bits);
        }
    }) else {
        dec_ref_bits(_py, server_bits);
        return Ok(None);
    };
    if !molt_is_callable(dispatch_bits) {
        dec_ref_bits(_py, dispatch_bits);
        dec_ref_bits(_py, server_bits);
        return Ok(None);
    }

    let request = urllib_http_build_request_bytes(req, request_target, host_header);
    let request_ptr = crate::bridge::alloc_bytes(_py, &request);
    if request_ptr.is_null() {
        dec_ref_bits(_py, dispatch_bits);
        dec_ref_bits(_py, server_bits);
        return Err(MoltObject::none().bits());
    }
    let request_bits = MoltObject::from_ptr(request_ptr).bits();
    let timeout_bits = MoltObject::from_float(req.timeout.unwrap_or(5.0)).bits();
    let response_bits = unsafe { call_callable2(_py, dispatch_bits, request_bits, timeout_bits) };
    dec_ref_bits(_py, request_bits);
    dec_ref_bits(_py, dispatch_bits);
    dec_ref_bits(_py, server_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }

    let Some(response_ptr) = obj_from_bits(response_bits).as_ptr() else {
        if !obj_from_bits(response_bits).is_none() {
            dec_ref_bits(_py, response_bits);
        }
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "socketserver dispatch must return bytes-like payload",
        ));
    };
    let Some(raw_bytes) = (unsafe { bytes_like_slice(MoltObject::from_ptr(response_ptr).bits()) })
    else {
        dec_ref_bits(_py, response_bits);
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "socketserver dispatch must return bytes-like payload",
        ));
    };
    let raw = raw_bytes.to_vec();
    dec_ref_bits(_py, response_bits);
    match urllib_http_parse_response_bytes(&raw) {
        Ok(parsed) => Ok(Some(parsed)),
        Err(msg) => Err(raise_exception::<u64>(_py, "ValueError", &msg)),
    }
}

pub(super) fn urllib_http_send_request(
    req: &UrllibHttpRequest,
    request_target: &str,
    host_header: &str,
) -> Result<HttpResponseParts, std::io::Error> {
    let request = urllib_http_build_request_bytes(req, request_target, host_header);
    let mut raw = Vec::new();
    {
        let _release = crate::bridge::GilReleaseGuard::new();
        let mut stream = TcpStream::connect((req.host.as_str(), req.port))?;
        if let Some(timeout) = req.timeout {
            let timeout = Duration::from_secs_f64(timeout);
            stream.set_read_timeout(Some(timeout))?;
            stream.set_write_timeout(Some(timeout))?;
        }
        if let Some(server_name) = req.tls_server_name.as_deref() {
            #[cfg(all(feature = "tls", not(target_arch = "wasm32")))]
            {
                urllib_https_send_over_tls(stream, server_name, &request, &mut raw)?;
            }
            #[cfg(not(all(feature = "tls", not(target_arch = "wasm32"))))]
            {
                let _ = (stream, server_name);
                return Err(std::io::Error::new(
                    ErrorKind::Unsupported,
                    "https requires native rustls TLS support; this target has no TLS transport",
                ));
            }
        } else {
            stream.write_all(&request)?;
            if let Err(err) = stream.read_to_end(&mut raw) {
                if (err.kind() == ErrorKind::TimedOut || err.kind() == ErrorKind::WouldBlock)
                    && !raw.is_empty()
                    && let Ok(parsed) = urllib_http_parse_response_bytes(&raw)
                {
                    return Ok(parsed);
                }
                return Err(err);
            }
        }
    }
    match urllib_http_parse_response_bytes(&raw) {
        Ok(parsed) => Ok(parsed),
        Err(msg) => Err(std::io::Error::new(ErrorKind::InvalidData, msg)),
    }
}

/// Send an HTTP request over TLS using rustls and read the full response into `out`.
///
/// Uses `webpki-roots` for trust anchors and the supplied `server_name` for SNI
/// and certificate hostname verification (default-secure rustls config).
#[cfg(all(feature = "tls", not(target_arch = "wasm32")))]
pub(super) fn urllib_https_send_over_tls(
    tcp: TcpStream,
    server_name: &str,
    request: &[u8],
    out: &mut Vec<u8>,
) -> std::io::Result<()> {
    use std::sync::Arc;

    use rustls::pki_types::ServerName;
    use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

    pub(super) fn shared_client_config() -> Arc<ClientConfig> {
        use std::sync::OnceLock;
        pub(super) static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
        CONFIG
            .get_or_init(|| {
                let mut roots = RootCertStore::empty();
                roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
                let cfg = ClientConfig::builder()
                    .with_root_certificates(roots)
                    .with_no_client_auth();
                Arc::new(cfg)
            })
            .clone()
    }

    let server_name_owned = ServerName::try_from(server_name.to_string())
        .map_err(|e| std::io::Error::new(ErrorKind::InvalidInput, format!("{e}")))?;
    let conn = ClientConnection::new(shared_client_config(), server_name_owned)
        .map_err(|e| std::io::Error::other(format!("TLS init failed: {e}")))?;
    let mut tls = StreamOwned::new(conn, tcp);

    tls.write_all(request)?;
    if let Err(err) = tls.read_to_end(out) {
        if (err.kind() == ErrorKind::UnexpectedEof
            || err.kind() == ErrorKind::TimedOut
            || err.kind() == ErrorKind::WouldBlock
            || err.kind() == ErrorKind::ConnectionAborted
            || err.kind() == ErrorKind::ConnectionReset)
            && !out.is_empty()
            && urllib_http_parse_response_bytes(out).is_ok()
        {
            return Ok(());
        }
        return Err(err);
    }
    Ok(())
}

pub(super) fn urllib_http_make_response_bits(
    _py: &molt_runtime_core::CoreGilToken,
    handle: i64,
) -> u64 {
    let marker_ptr = alloc_string(_py, b"__molt_urllib_response__");
    if marker_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let marker_bits = MoltObject::from_ptr(marker_ptr).bits();
    let handle_bits = MoltObject::from_int(handle).bits();
    let tuple_ptr = alloc_tuple(_py, &[marker_bits, handle_bits]);
    dec_ref_bits(_py, marker_bits);
    if tuple_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(tuple_ptr).bits()
    }
}

pub(super) fn urllib_request_response_handle_from_bits(
    _py: &molt_runtime_core::CoreGilToken,
    response_bits: u64,
) -> Result<i64, u64> {
    let Some(ptr) = obj_from_bits(response_bits).as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "response object is invalid",
        ));
    };
    let ty = unsafe { object_type_id(ptr) };
    if ty != crate::bridge::type_id_tuple() {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "response object is invalid",
        ));
    }
    let fields = unsafe { seq_vec_ref(ptr) };
    if fields.len() != 2 {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "response object is invalid",
        ));
    }
    let Some(tag) = string_obj_to_owned(obj_from_bits(fields[0])) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "response object is invalid",
        ));
    };
    if tag != "__molt_urllib_response__" {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "response object is invalid",
        ));
    }
    let Some(handle) = to_i64(obj_from_bits(fields[1])) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "response object handle is invalid",
        ));
    };
    Ok(handle)
}

pub(super) fn urllib_error_class_bits(
    _py: &molt_runtime_core::CoreGilToken,
    class_name: &[u8],
) -> Result<u64, u64> {
    let module_name_ptr = alloc_string(_py, b"urllib.error");
    if module_name_ptr.is_null() {
        return Err(MoltObject::none().bits());
    }
    let module_name_bits = MoltObject::from_ptr(module_name_ptr).bits();
    let module_bits = crate::bridge::molt_module_import(module_name_bits);
    dec_ref_bits(_py, module_name_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let Some(name_bits) = attr_name_bits_from_bytes(_py, class_name) else {
        dec_ref_bits(_py, module_bits);
        return Err(MoltObject::none().bits());
    };
    let missing = missing_bits(_py);
    let class_bits = molt_getattr_builtin(module_bits, name_bits, missing);
    dec_ref_bits(_py, name_bits);
    dec_ref_bits(_py, module_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if class_bits == missing {
        return Err(raise_exception::<u64>(
            _py,
            "RuntimeError",
            "urllib.error class is unavailable",
        ));
    }
    Ok(class_bits)
}

pub(super) fn urllib_raise_url_error(_py: &molt_runtime_core::CoreGilToken, reason: &str) -> u64 {
    let class_bits = match urllib_error_class_bits(_py, b"URLError") {
        Ok(bits) => bits,
        Err(bits) => return bits,
    };
    let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
        dec_ref_bits(_py, class_bits);
        return raise_exception::<u64>(_py, "TypeError", "URLError class is invalid");
    };
    let reason_ptr = alloc_string(_py, reason.as_bytes());
    if reason_ptr.is_null() {
        dec_ref_bits(_py, class_bits);
        return MoltObject::none().bits();
    }
    let reason_bits = MoltObject::from_ptr(reason_ptr).bits();
    let exc_bits = unsafe {
        call_class_init_with_args(_py, MoltObject::from_ptr(class_ptr).bits(), &[reason_bits])
    };
    dec_ref_bits(_py, reason_bits);
    dec_ref_bits(_py, class_bits);
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }
    crate::bridge::molt_raise(exc_bits)
}

pub(super) fn urllib_raise_http_error(
    _py: &molt_runtime_core::CoreGilToken,
    url: &str,
    code: i64,
    reason: &str,
    headers: &[(String, String)],
    fp_bits: u64,
) -> u64 {
    let class_bits = match urllib_error_class_bits(_py, b"HTTPError") {
        Ok(bits) => bits,
        Err(bits) => return bits,
    };
    let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
        dec_ref_bits(_py, class_bits);
        return raise_exception::<u64>(_py, "TypeError", "HTTPError class is invalid");
    };
    let url_ptr = alloc_string(_py, url.as_bytes());
    if url_ptr.is_null() {
        dec_ref_bits(_py, class_bits);
        return MoltObject::none().bits();
    }
    let reason_ptr = alloc_string(_py, reason.as_bytes());
    if reason_ptr.is_null() {
        let url_bits = MoltObject::from_ptr(url_ptr).bits();
        dec_ref_bits(_py, url_bits);
        dec_ref_bits(_py, class_bits);
        return MoltObject::none().bits();
    }
    let url_bits = MoltObject::from_ptr(url_ptr).bits();
    let reason_bits = MoltObject::from_ptr(reason_ptr).bits();
    let code_bits = MoltObject::from_int(code).bits();
    let headers_bits = match urllib_http_headers_to_dict(_py, headers) {
        Ok(bits) => bits,
        Err(bits) => {
            dec_ref_bits(_py, url_bits);
            dec_ref_bits(_py, reason_bits);
            dec_ref_bits(_py, class_bits);
            return bits;
        }
    };
    let exc_bits = unsafe {
        call_class_init_with_args(
            _py,
            MoltObject::from_ptr(class_ptr).bits(),
            &[url_bits, code_bits, reason_bits, headers_bits, fp_bits],
        )
    };
    dec_ref_bits(_py, headers_bits);
    dec_ref_bits(_py, reason_bits);
    dec_ref_bits(_py, url_bits);
    dec_ref_bits(_py, class_bits);
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }
    crate::bridge::molt_raise(exc_bits)
}
