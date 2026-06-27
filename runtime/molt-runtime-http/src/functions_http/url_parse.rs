use super::*;

pub(super) fn urllib_is_alpha(ch: char) -> bool {
    ch.is_ascii_alphabetic()
}

pub(super) fn urllib_is_alnum(ch: char) -> bool {
    ch.is_ascii_alphanumeric()
}

pub(super) fn urllib_split_scheme(url: &str, default: &str) -> (String, String) {
    for (idx, ch) in url.char_indices() {
        if ch == ':' {
            let scheme = &url[..idx];
            if !scheme.is_empty()
                && scheme.chars().next().is_some_and(urllib_is_alpha)
                && scheme
                    .chars()
                    .all(|c| urllib_is_alnum(c) || matches!(c, '+' | '-' | '.'))
            {
                return (scheme.to_ascii_lowercase(), url[idx + 1..].to_string());
            }
            break;
        }
        if matches!(ch, '/' | '?' | '#') {
            break;
        }
    }
    (default.to_string(), url.to_string())
}

pub(super) fn urllib_split_netloc(rest: &str) -> (String, String) {
    for (idx, ch) in rest.char_indices() {
        if matches!(ch, '/' | '?' | '#') {
            return (rest[..idx].to_string(), rest[idx..].to_string());
        }
    }
    (rest.to_string(), String::new())
}

pub(super) fn urllib_split_query_fragment(
    rest: &str,
    allow_fragments: bool,
) -> (String, String, String) {
    let mut working = rest.to_string();
    let mut fragment = String::new();
    if allow_fragments && let Some(idx) = working.find('#') {
        fragment = working[idx + 1..].to_string();
        working.truncate(idx);
    }
    let mut query = String::new();
    if let Some(idx) = working.find('?') {
        query = working[idx + 1..].to_string();
        working.truncate(idx);
    }
    (working, query, fragment)
}

pub(super) fn urllib_urlsplit_impl(url: &str, scheme: &str, allow_fragments: bool) -> [String; 5] {
    let (parsed_scheme, mut rest) = urllib_split_scheme(url, scheme);
    let mut netloc = String::new();
    if rest.starts_with("//") {
        let (out_netloc, out_rest) = urllib_split_netloc(&rest[2..]);
        netloc = out_netloc;
        rest = out_rest;
    }
    let (path, query, fragment) = urllib_split_query_fragment(&rest, allow_fragments);
    [parsed_scheme, netloc, path, query, fragment]
}

pub(super) fn urllib_urlparse_impl(url: &str, scheme: &str, allow_fragments: bool) -> [String; 6] {
    let split = urllib_urlsplit_impl(url, scheme, allow_fragments);
    let mut path = split[2].clone();
    let mut params = String::new();
    if let Some(idx) = path.find(';') {
        params = path[idx + 1..].to_string();
        path.truncate(idx);
    }
    [
        split[0].clone(),
        split[1].clone(),
        path,
        params,
        split[3].clone(),
        split[4].clone(),
    ]
}

pub(super) fn urllib_unsplit_impl(
    scheme: &str,
    netloc: &str,
    path: &str,
    query: &str,
    fragment: &str,
) -> String {
    let mut out = String::new();
    if !scheme.is_empty() {
        out.push_str(scheme);
        out.push(':');
    }
    if !netloc.is_empty() {
        out.push_str("//");
        out.push_str(netloc);
    }
    out.push_str(path);
    if !query.is_empty() {
        out.push('?');
        out.push_str(query);
    }
    if !fragment.is_empty() {
        out.push('#');
        out.push_str(fragment);
    }
    out
}

pub(super) fn urllib_quote_impl(string: &str, safe: &str) -> String {
    pub(super) const ALWAYS_SAFE: &str =
        "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_.-~";
    let safe_set: std::collections::HashSet<char> =
        ALWAYS_SAFE.chars().chain(safe.chars()).collect();
    let mut out = String::new();
    for ch in string.chars() {
        if safe_set.contains(&ch) {
            out.push(ch);
            continue;
        }
        let mut buf = [0u8; 4];
        for byte in ch.encode_utf8(&mut buf).as_bytes() {
            out.push('%');
            out.push_str(&format!("{byte:02X}"));
        }
    }
    out
}

pub(super) fn urllib_quote_plus_impl(string: &str, safe: &str) -> String {
    urllib_quote_impl(string, safe).replace("%20", "+")
}

pub(super) fn urllib_unquote_impl(string: &str) -> String {
    if !string.contains('%') {
        return string.to_string();
    }
    let chars: Vec<char> = string.chars().collect();
    let mut out: Vec<u8> = Vec::with_capacity(string.len());
    let mut idx = 0usize;
    while idx < chars.len() {
        let ch = chars[idx];
        if ch == '%' && idx + 2 < chars.len() {
            let h1 = chars[idx + 1];
            let h2 = chars[idx + 2];
            if let (Some(a), Some(b)) = (h1.to_digit(16), h2.to_digit(16)) {
                out.push(((a << 4) | b) as u8);
                idx += 3;
                continue;
            }
        }
        let mut buf = [0u8; 4];
        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
        idx += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub(super) fn urllib_urlencode_impl(
    _py: &molt_runtime_core::CoreGilToken,
    query_bits: u64,
    doseq: bool,
    safe: &str,
) -> Result<String, u64> {
    let iter_bits = molt_iter(query_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let mut out_pairs: Vec<String> = Vec::new();
    loop {
        let (item_bits, done) = iter_next_pair(_py, iter_bits)?;
        if done {
            break;
        }
        let item_obj = obj_from_bits(item_bits);
        let Some(item_ptr) = item_obj.as_ptr() else {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "not a valid non-string sequence or mapping object",
            ));
        };
        let item_type = unsafe { object_type_id(item_ptr) };
        if item_type != crate::bridge::type_id_list() && item_type != crate::bridge::type_id_tuple()
        {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "not a valid non-string sequence or mapping object",
            ));
        }
        let item_fields = unsafe { seq_vec_ref(item_ptr) };
        if item_fields.len() != 2 {
            if item_fields.len() < 2 {
                return Err(raise_exception::<_>(
                    _py,
                    "ValueError",
                    &format!(
                        "not enough values to unpack (expected 2, got {})",
                        item_fields.len()
                    ),
                ));
            }
            return Err(raise_exception::<_>(
                _py,
                "ValueError",
                "too many values to unpack (expected 2)",
            ));
        }
        let key_text = crate::bridge::format_obj_str(_py, obj_from_bits(item_fields[0]));
        let key_enc = urllib_quote_plus_impl(&key_text, safe);
        let value_obj = obj_from_bits(item_fields[1]);
        let mut wrote_pair = false;
        if doseq && let Some(value_ptr) = value_obj.as_ptr() {
            let value_type = unsafe { object_type_id(value_ptr) };
            if value_type == crate::bridge::type_id_list()
                || value_type == crate::bridge::type_id_tuple()
            {
                let seq = unsafe { seq_vec_ref(value_ptr) };
                for value_bits in seq.iter().copied() {
                    let value_text = crate::bridge::format_obj_str(_py, obj_from_bits(value_bits));
                    let value_enc = urllib_quote_plus_impl(&value_text, safe);
                    out_pairs.push(format!("{key_enc}={value_enc}"));
                }
                wrote_pair = true;
            }
        }
        if !wrote_pair {
            let value_text = crate::bridge::format_obj_str(_py, value_obj);
            let value_enc = urllib_quote_plus_impl(&value_text, safe);
            out_pairs.push(format!("{key_enc}={value_enc}"));
        }
    }
    Ok(out_pairs.join("&"))
}

pub(super) fn urllib_error_set_attr(
    _py: &molt_runtime_core::CoreGilToken,
    self_bits: u64,
    name: &str,
    value_bits: u64,
) -> bool {
    let Some(name_bits) = attr_name_bits_from_bytes(_py, name.as_bytes()) else {
        return false;
    };
    crate::bridge::molt_object_setattr(self_bits, name_bits, value_bits);
    dec_ref_bits(_py, name_bits);
    !exception_pending(_py)
}

pub(super) fn urllib_error_init_args(
    _py: &molt_runtime_core::CoreGilToken,
    self_bits: u64,
    args: &[u64],
) -> bool {
    let args_ptr = alloc_tuple(_py, args);
    if args_ptr.is_null() {
        return false;
    }
    let args_bits = MoltObject::from_ptr(args_ptr).bits();
    let _ = crate::bridge::molt_exception_init(self_bits, args_bits);
    !exception_pending(_py)
}

pub(super) fn urllib_parse_qsl_impl(
    qs: &str,
    keep_blank_values: bool,
    strict_parsing: bool,
) -> Result<Vec<(String, String)>, String> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    if qs.is_empty() {
        return Ok(pairs);
    }
    for chunk in qs.split('&') {
        if chunk.is_empty() && !keep_blank_values {
            continue;
        }
        let (key, value) = if let Some((k, v)) = chunk.split_once('=') {
            (k, v)
        } else if strict_parsing {
            return Err("bad query field".to_string());
        } else {
            (chunk, "")
        };
        if !value.is_empty() || keep_blank_values {
            let key_text = urllib_unquote_impl(&key.replace('+', " "));
            let value_text = urllib_unquote_impl(&value.replace('+', " "));
            pairs.push((key_text, value_text));
        }
    }
    Ok(pairs)
}

pub(super) fn urllib_request_pending_exception_kind_name(
    _py: &molt_runtime_core::CoreGilToken,
) -> Option<String> {
    if !exception_pending(_py) {
        return None;
    }
    let exc_bits = molt_exception_last();
    let out = maybe_ptr_from_bits(exc_bits)
        .and_then(|ptr| string_obj_to_owned(obj_from_bits(unsafe { exception_kind_bits(ptr) })));
    if !obj_from_bits(exc_bits).is_none() {
        dec_ref_bits(_py, exc_bits);
    }
    out
}
