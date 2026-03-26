use molt_obj_model::MoltObject;
#[cfg(feature = "stdlib_ast")]
use rustpython_parser::{Mode as ParseMode, ParseErrorType, ast as pyast, parse as parse_python};
use serde_json::{Map as JsonMap, Value as JsonValue};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::types::cell_class;
use crate::builtins::numbers::index_i64_with_overflow;
use crate::builtins::platform::env_state_get;
use crate::{
    TYPE_ID_BOUND_METHOD, TYPE_ID_BYTES, TYPE_ID_DICT, TYPE_ID_FUNCTION, TYPE_ID_LIST,
    TYPE_ID_MODULE, TYPE_ID_STRING, TYPE_ID_TUPLE,
    alloc_bound_method_obj, alloc_bytes, alloc_code_obj, alloc_dict_with_pairs,
    alloc_function_obj, alloc_list_with_capacity, alloc_string, alloc_tuple,
    attr_name_bits_from_bytes, bound_method_func_bits, builtin_classes,
    bytes_like_slice,
    call_callable0, call_callable1, call_callable2, call_callable3,
    call_class_init_with_args,
    clear_exception, dec_ref_bits,
    dict_get_in_place, ensure_function_code_bits,
    exception_kind_bits, exception_pending,
    format_obj, function_dict_bits, function_set_closure_bits, function_set_trampoline_ptr,
    inc_ref_bits, is_truthy, maybe_ptr_from_bits, missing_bits, module_dict_bits,
    molt_exception_last, molt_getattr_builtin, molt_getitem_method, molt_is_callable,
    molt_iter, molt_iter_next, molt_list_insert, molt_trace_enter_slot,
    obj_from_bits, object_class_bits, object_set_class_bits, object_type_id,
    raise_exception, seq_vec_ref, string_obj_to_owned,
    to_f64, to_i64, type_name, type_of_bits,
};
use memchr::{memchr, memmem};

#[allow(unused_imports)]
use super::functions::*;
#[allow(unused_imports)]
use super::functions_net::*;

#[unsafe(no_mangle)]
pub extern "C" fn molt_textwrap_wrap(text_bits: u64, width_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let Some(width) = to_i64(obj_from_bits(width_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "width must be int");
        };
        let options = textwrap_default_options(width);
        let lines = match textwrap_wrap_impl(&text, &options) {
            Ok(lines) => lines,
            Err(msg) => return raise_exception::<_>(_py, "ValueError", &msg),
        };
        alloc_string_list(_py, &lines)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_textwrap_wrap_ex(
    text_bits: u64,
    width_bits: u64,
    initial_indent_bits: u64,
    subsequent_indent_bits: u64,
    expand_tabs_bits: u64,
    replace_whitespace_bits: u64,
    fix_sentence_endings_bits: u64,
    break_long_words_bits: u64,
    drop_whitespace_bits: u64,
    break_on_hyphens_bits: u64,
    tabsize_bits: u64,
    max_lines_placeholder_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let options = match textwrap_parse_options_ex(
            _py,
            width_bits,
            initial_indent_bits,
            subsequent_indent_bits,
            expand_tabs_bits,
            replace_whitespace_bits,
            fix_sentence_endings_bits,
            break_long_words_bits,
            drop_whitespace_bits,
            break_on_hyphens_bits,
            tabsize_bits,
            max_lines_placeholder_bits,
        ) {
            Ok(options) => options,
            Err(bits) => return bits,
        };
        let lines = match textwrap_wrap_impl(&text, &options) {
            Ok(lines) => lines,
            Err(msg) => return raise_exception::<_>(_py, "ValueError", &msg),
        };
        alloc_string_list(_py, &lines)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_textwrap_fill(text_bits: u64, width_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let Some(width) = to_i64(obj_from_bits(width_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "width must be int");
        };
        let options = textwrap_default_options(width);
        let out = match textwrap_wrap_impl(&text, &options) {
            Ok(lines) => lines.join("\n"),
            Err(msg) => return raise_exception::<_>(_py, "ValueError", &msg),
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
pub extern "C" fn molt_textwrap_fill_ex(
    text_bits: u64,
    width_bits: u64,
    initial_indent_bits: u64,
    subsequent_indent_bits: u64,
    expand_tabs_bits: u64,
    replace_whitespace_bits: u64,
    fix_sentence_endings_bits: u64,
    break_long_words_bits: u64,
    drop_whitespace_bits: u64,
    break_on_hyphens_bits: u64,
    tabsize_bits: u64,
    max_lines_placeholder_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let options = match textwrap_parse_options_ex(
            _py,
            width_bits,
            initial_indent_bits,
            subsequent_indent_bits,
            expand_tabs_bits,
            replace_whitespace_bits,
            fix_sentence_endings_bits,
            break_long_words_bits,
            drop_whitespace_bits,
            break_on_hyphens_bits,
            tabsize_bits,
            max_lines_placeholder_bits,
        ) {
            Ok(options) => options,
            Err(bits) => return bits,
        };
        let out = match textwrap_wrap_impl(&text, &options) {
            Ok(lines) => lines.join("\n"),
            Err(msg) => return raise_exception::<_>(_py, "ValueError", &msg),
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
pub extern "C" fn molt_textwrap_indent(text_bits: u64, prefix_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let Some(prefix) = string_obj_to_owned(obj_from_bits(prefix_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "prefix must be str");
        };
        textwrap_indent_with_predicate(_py, &text, &prefix, None)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_textwrap_indent_ex(
    text_bits: u64,
    prefix_bits: u64,
    predicate_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let Some(prefix) = string_obj_to_owned(obj_from_bits(prefix_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "prefix must be str");
        };
        let predicate = if obj_from_bits(predicate_bits).is_none() {
            None
        } else {
            Some(predicate_bits)
        };
        textwrap_indent_with_predicate(_py, &text, &prefix, predicate)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_quote(string_bits: u64, safe_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
        let Some(handle) = cookiejar_store_new() else {
            return raise_exception::<_>(_py, "RuntimeError", "cookie jar allocation failed");
        };
        MoltObject::from_int(handle).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_cookiejar_len(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
        let reason_text = crate::format_obj_str(_py, obj_from_bits(reason_bits));
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
        let code_text = crate::format_obj_str(_py, obj_from_bits(code_bits));
        let msg_text = crate::format_obj_str(_py, obj_from_bits(msg_bits));
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
    crate::with_gil_entry!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_socketserver_register(server_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
        let ptr = crate::alloc_bytes(_py, &response);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_socketserver_dispatch_cancel(server_bits: u64, request_id_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
        let request_ptr = crate::alloc_bytes(_py, &pending.request);
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
        let Some(get_request_name_bits) = attr_name_bits_from_bytes(_py, b"get_request") else {
            return MoltObject::none().bits();
        };
        let missing = missing_bits(_py);
        let get_request_bits = molt_getattr_builtin(server_bits, get_request_name_bits, missing);
        dec_ref_bits(_py, get_request_name_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if get_request_bits == missing
            || !is_truthy(_py, obj_from_bits(molt_is_callable(get_request_bits)))
        {
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

        if let Some(verify_request_bits) =
            match urllib_request_attr_optional(_py, server_bits, b"verify_request") {
                Ok(bits) => bits,
                Err(bits) => {
                    dec_ref_bits(_py, request_tuple_bits);
                    return bits;
                }
            }
        {
            if !is_truthy(_py, obj_from_bits(molt_is_callable(verify_request_bits))) {
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
            if process_request_bits == missing
                || !is_truthy(_py, obj_from_bits(molt_is_callable(process_request_bits)))
            {
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
        if close_request_bits != missing
            && is_truthy(_py, obj_from_bits(molt_is_callable(close_request_bits)))
        {
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
                    let out = crate::molt_raise(exc_bits);
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
            if response_bytes_bits != missing
                && is_truthy(_py, obj_from_bits(molt_is_callable(response_bytes_bits)))
            {
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
                        return crate::molt_raise(exc_bits);
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
                        return crate::molt_raise(exc_bits);
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
            return crate::molt_raise(exc_bits);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_socketserver_shutdown(server_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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

pub(super) fn http_server_read_request_impl(_py: &crate::PyToken<'_>, handler_bits: u64) -> Result<i64, u64> {
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
    _py: &crate::PyToken<'_>,
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
    crate::with_gil_entry!(_py, {
        match http_server_read_request_impl(_py, handler_bits) {
            Ok(state) => MoltObject::from_int(state).bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_server_compute_close_connection(handler_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        match http_server_compute_close_connection_impl(_py, handler_bits) {
            Ok(close) => MoltObject::from_bool(close).bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_server_handle_one_request(handler_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
        let Some(code) = to_i64(obj_from_bits(code_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "code must be int");
        };
        let message = if obj_from_bits(message_bits).is_none() {
            None
        } else {
            Some(crate::format_obj_str(_py, obj_from_bits(message_bits)))
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
    crate::with_gil_entry!(_py, {
        let Some(code) = to_i64(obj_from_bits(code_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "code must be int");
        };
        let message = if obj_from_bits(message_bits).is_none() {
            None
        } else {
            Some(crate::format_obj_str(_py, obj_from_bits(message_bits)))
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
    crate::with_gil_entry!(_py, {
        let keyword = crate::format_obj_str(_py, obj_from_bits(keyword_bits));
        let value = crate::format_obj_str(_py, obj_from_bits(value_bits));
        match http_server_send_header_impl(_py, handler_bits, &keyword, &value) {
            Ok(()) => MoltObject::none().bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_server_end_headers(handler_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
        let Some(code) = to_i64(obj_from_bits(code_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "code must be int");
        };
        let message = if obj_from_bits(message_bits).is_none() {
            None
        } else {
            Some(crate::format_obj_str(_py, obj_from_bits(message_bits)))
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
    crate::with_gil_entry!(_py, {
        let server_version = crate::format_obj_str(_py, obj_from_bits(server_version_bits));
        let sys_version = if obj_from_bits(sys_version_bits).is_none() {
            String::new()
        } else {
            crate::format_obj_str(_py, obj_from_bits(sys_version_bits))
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_request_init(
    self_bits: u64,
    url_bits: u64,
    data_bits: u64,
    headers_bits: u64,
    method_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let url_text = crate::format_obj_str(_py, obj_from_bits(url_bits));
        let url_ptr = alloc_string(_py, url_text.as_bytes());
        if url_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let full_url_bits = MoltObject::from_ptr(url_ptr).bits();
        let mut headers_value = headers_bits;
        if obj_from_bits(headers_bits).is_none() {
            let dict_bits = crate::molt_dict_new(0);
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
        let mut owned_request_refs: Vec<u64> = Vec::new();
        let out_bits = (|| -> u64 {
            let mut active_request_bits = request_bits;
            let mut full_url = {
                let Some(full_url_bits) =
                    (match urllib_request_attr_optional(_py, active_request_bits, b"full_url") {
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
                let Some(method_bits) = (match urllib_request_attr_optional(
                    _py,
                    handler_bits,
                    request_method_name.as_bytes(),
                ) {
                    Ok(bits) => bits,
                    Err(bits) => return bits,
                }) else {
                    continue;
                };
                if !is_truthy(_py, obj_from_bits(molt_is_callable(method_bits))) {
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
                    (match urllib_request_attr_optional(_py, active_request_bits, b"full_url") {
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
                let Some(method_bits) = (match urllib_request_attr_optional(
                    _py,
                    handler_bits,
                    method_name.as_bytes(),
                ) {
                    Ok(bits) => bits,
                    Err(bits) => return bits,
                }) else {
                    continue;
                };
                if !is_truthy(_py, obj_from_bits(molt_is_callable(method_bits))) {
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

            let allow_data_fallback = match urllib_request_attr_optional(
                _py,
                opener_bits,
                b"_molt_allow_data_fallback",
            ) {
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
                let Some(method_bits) = (match urllib_request_attr_optional(
                    _py,
                    handler_bits,
                    response_method_name.as_bytes(),
                ) {
                    Ok(bits) => bits,
                    Err(bits) => return bits,
                }) else {
                    continue;
                };
                if !is_truthy(_py, obj_from_bits(molt_is_callable(method_bits))) {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
                let ptr = crate::alloc_bytes(_py, data.as_slice());
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
fn urllib_response_is_data(resp: &MoltUrllibResponse) -> bool {
    resp.code < 0
}

fn urllib_response_read_vec(
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

fn urllib_response_readinto_len(
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
    crate::with_gil_entry!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        let mut export = crate::BufferExport {
            ptr: 0,
            len: 0,
            readonly: 0,
            stride: 0,
            itemsize: 0,
        };
        if unsafe { crate::molt_buffer_export(buffer_bits, &mut export) } != 0
            || export.readonly != 0
            || export.itemsize != 1
            || export.stride != 1
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

fn urllib_response_readline_vec(
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

fn urllib_response_seek_pos(
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
    crate::with_gil_entry!(_py, {
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
                let ptr = crate::alloc_bytes(_py, data.as_slice());
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        urllib_response_drop(_py, handle);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_urllib_request_response_geturl(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        let name = crate::format_obj_str(_py, obj_from_bits(name_bits));
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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

fn urllib_response_message_bits(_py: &crate::PyToken<'_>, handle: i64) -> u64 {
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
    crate::with_gil_entry!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "response handle is invalid");
        };
        urllib_response_message_bits(_py, handle)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_parse_header_pairs(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
        let Some(handle) = http_message_store_new() else {
            return MoltObject::none().bits();
        };
        MoltObject::from_int(handle).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_message_parse(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
        let handle = match http_message_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let name = crate::format_obj_str(_py, obj_from_bits(name_bits));
        let value = crate::format_obj_str(_py, obj_from_bits(value_bits));
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
    crate::with_gil_entry!(_py, {
        let handle = match http_message_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let needle =
            http_message_header_key(crate::format_obj_str(_py, obj_from_bits(name_bits)).as_str());
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
    crate::with_gil_entry!(_py, {
        let handle = match http_message_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let needle =
            http_message_header_key(crate::format_obj_str(_py, obj_from_bits(name_bits)).as_str());
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
        let handle = match http_message_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let needle =
            http_message_header_key(crate::format_obj_str(_py, obj_from_bits(name_bits)).as_str());
        let Some(found) = http_message_with(handle, |message| message.index.contains_key(&needle))
        else {
            return raise_exception::<_>(_py, "RuntimeError", "http message handle is invalid");
        };
        MoltObject::from_bool(found).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_http_message_len(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
        let Some(handle) = http_client_connection_store(host, port_value as u16, timeout) else {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
            ))
        });
        let (host, port, timeout, method, url, headers, body) = match state {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
        let handle = match http_client_response_handle_from_bits(_py, handle_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        urllib_response_message_bits(_py, handle)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_pkgutil_iter_modules(path_bits: u64, prefix_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !crate::has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let Some(prefix) = string_obj_to_owned(obj_from_bits(prefix_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "prefix must be str");
        };
        let paths = if obj_from_bits(path_bits).is_none() {
            Vec::new()
        } else {
            match iterable_to_string_vec(_py, path_bits) {
                Ok(paths) => paths,
                Err(bits) => return bits,
            }
        };
        let out = pkgutil_iter_modules_impl(&paths, &prefix);
        alloc_pkgutil_module_info_list(_py, &out)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_pkgutil_walk_packages(path_bits: u64, prefix_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !crate::has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let Some(prefix) = string_obj_to_owned(obj_from_bits(prefix_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "prefix must be str");
        };
        let paths = if obj_from_bits(path_bits).is_none() {
            Vec::new()
        } else {
            match iterable_to_string_vec(_py, path_bits) {
                Ok(paths) => paths,
                Err(bits) => return bits,
            }
        };
        let out = pkgutil_walk_packages_impl(&paths, &prefix);
        alloc_pkgutil_module_info_list(_py, &out)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_shutil_copyfile(src_bits: u64, dst_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !crate::has_capability(_py, "fs.read") || !crate::has_capability(_py, "fs.write") {
            return raise_exception::<_>(
                _py,
                "PermissionError",
                "missing fs.read/fs.write capability",
            );
        }
        let Some(src) = string_obj_to_owned(obj_from_bits(src_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "src must be str");
        };
        let Some(dst) = string_obj_to_owned(obj_from_bits(dst_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "dst must be str");
        };
        if let Err(err) = fs::copy(&src, &dst) {
            return raise_os_error_from_io(_py, err);
        }
        let out_ptr = alloc_string(_py, dst.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_shutil_which(cmd_bits: u64, path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !crate::has_capability(_py, "fs.read") || !crate::has_capability(_py, "env.read") {
            return raise_exception::<_>(
                _py,
                "PermissionError",
                "missing fs.read/env.read capability",
            );
        }
        let Some(cmd) = string_obj_to_owned(obj_from_bits(cmd_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "cmd must be str");
        };
        if cmd.is_empty() {
            return MoltObject::none().bits();
        }
        let path = if obj_from_bits(path_bits).is_none() {
            env_state_get("PATH")
                .or_else(|| std::env::var("PATH").ok())
                .unwrap_or_default()
        } else {
            let Some(path) = string_obj_to_owned(obj_from_bits(path_bits)) else {
                return raise_exception::<_>(_py, "TypeError", "path must be str or None");
            };
            path
        };

        #[cfg(windows)]
        let pathexts: Vec<String> = {
            let raw = env_state_get("PATHEXT")
                .or_else(|| std::env::var("PATHEXT").ok())
                .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".to_string());
            raw.split(';')
                .filter_map(|entry| {
                    let trimmed = entry.trim();
                    if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed.to_string())
                    }
                })
                .collect()
        };

        let cmd_path = Path::new(&cmd);
        let has_path_sep =
            cmd.contains(std::path::MAIN_SEPARATOR) || (cfg!(windows) && cmd.contains('/'));

        let check_candidate = |candidate: PathBuf| -> Option<u64> {
            #[cfg(windows)]
            {
                if path_is_executable(&candidate) {
                    return Some(alloc_optional_path(_py, &candidate));
                }
                let ext_present = candidate
                    .extension()
                    .map(|ext| !ext.is_empty())
                    .unwrap_or(false);
                if !ext_present {
                    for ext in &pathexts {
                        let ext_clean = ext.trim_start_matches('.');
                        let with_ext = candidate.with_extension(ext_clean);
                        if path_is_executable(&with_ext) {
                            return Some(alloc_optional_path(_py, &with_ext));
                        }
                    }
                }
                None
            }
            #[cfg(not(windows))]
            {
                if path_is_executable(&candidate) {
                    Some(alloc_optional_path(_py, &candidate))
                } else {
                    None
                }
            }
        };

        if cmd_path.is_absolute() || has_path_sep {
            if let Some(bits) = check_candidate(PathBuf::from(&cmd)) {
                return bits;
            }
            return MoltObject::none().bits();
        }

        #[cfg(windows)]
        let path_sep = ';';
        #[cfg(not(windows))]
        let path_sep = ':';
        for entry in path.split(path_sep) {
            let dir = if entry.is_empty() { "." } else { entry };
            let candidate = Path::new(dir).join(&cmd);
            if let Some(bits) = check_candidate(candidate) {
                return bits;
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_py_compile_compile(file_bits: u64, cfile_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !crate::has_capability(_py, "fs.read") || !crate::has_capability(_py, "fs.write") {
            return raise_exception::<_>(
                _py,
                "PermissionError",
                "missing fs.read/fs.write capability",
            );
        }
        let Some(file) = string_obj_to_owned(obj_from_bits(file_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "file must be str");
        };
        let cfile = if obj_from_bits(cfile_bits).is_none() {
            format!("{file}c")
        } else {
            let Some(cfile) = string_obj_to_owned(obj_from_bits(cfile_bits)) else {
                return raise_exception::<_>(_py, "TypeError", "cfile must be str or None");
            };
            cfile
        };

        let mut in_file = match fs::File::open(&file) {
            Ok(handle) => handle,
            Err(err) => return raise_os_error_from_io(_py, err),
        };
        let mut one = [0u8; 1];
        if let Err(err) = in_file.read(&mut one) {
            return raise_os_error_from_io(_py, err);
        }
        if let Err(err) = fs::File::create(&cfile) {
            return raise_os_error_from_io(_py, err);
        }
        let abs = absolutize_path(&cfile);
        let out_ptr = alloc_string(_py, abs.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_compileall_compile_file(fullname_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !crate::has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let Some(fullname) = string_obj_to_owned(obj_from_bits(fullname_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "fullname must be str");
        };
        MoltObject::from_bool(compileall_compile_file_impl(&fullname)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_compileall_compile_dir(dir_bits: u64, maxlevels_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !crate::has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let Some(dir) = string_obj_to_owned(obj_from_bits(dir_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "dir must be str");
        };
        let Some(maxlevels) = to_i64(obj_from_bits(maxlevels_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "maxlevels must be int");
        };
        MoltObject::from_bool(compileall_compile_dir_impl(&dir, maxlevels)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_compileall_compile_path(
    paths_bits: u64,
    skip_curdir_bits: u64,
    maxlevels_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !crate::has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let paths = match iterable_to_string_vec(_py, paths_bits) {
            Ok(paths) => paths,
            Err(bits) => return bits,
        };
        let skip_curdir = is_truthy(_py, obj_from_bits(skip_curdir_bits));
        let Some(maxlevels) = to_i64(obj_from_bits(maxlevels_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "maxlevels must be int");
        };

        let mut success = true;
        for entry in paths {
            if skip_curdir && (entry.is_empty() || entry == ".") {
                continue;
            }
            if !compileall_compile_dir_impl(&entry, maxlevels) {
                success = false;
            }
        }
        MoltObject::from_bool(success).bits()
    })
}

// --- Begin stdlib_ast-gated compile infrastructure ---
#[cfg(feature = "stdlib_ast")]
fn compile_error_type(error: &ParseErrorType) -> &'static str {
    if error.is_tab_error() {
        "TabError"
    } else if error.is_indentation_error() {
        "IndentationError"
    } else {
        "SyntaxError"
    }
}

#[cfg(feature = "stdlib_ast")]
fn codeop_future_flag_for_name(name: &str) -> i64 {
    match name {
        "nested_scopes" => 0x0010,
        "generators" => 0,
        "division" => 0x20000,
        "absolute_import" => 0x40000,
        "with_statement" => 0x80000,
        "print_function" => 0x100000,
        "unicode_literals" => 0x200000,
        "barry_as_FLUFL" => 0x400000,
        "generator_stop" => 0x800000,
        "annotations" => 0x1000000,
        _ => 0,
    }
}

#[cfg(feature = "stdlib_ast")]
fn codeop_is_docstring_stmt(stmt: &pyast::Stmt) -> bool {
    match stmt {
        pyast::Stmt::Expr(node) => match node.value.as_ref() {
            pyast::Expr::Constant(expr) => matches!(expr.value, pyast::Constant::Str(_)),
            _ => false,
        },
        _ => false,
    }
}

#[cfg(feature = "stdlib_ast")]
fn codeop_future_flags_from_stmts(stmts: &[pyast::Stmt]) -> i64 {
    let mut idx = 0usize;
    if let Some(first) = stmts.first()
        && codeop_is_docstring_stmt(first)
    {
        idx = 1;
    }
    let mut out = 0i64;
    for stmt in &stmts[idx..] {
        let pyast::Stmt::ImportFrom(node) = stmt else {
            break;
        };
        let Some(module) = node.module.as_ref() else {
            break;
        };
        let level_is_zero = match node.level.as_ref() {
            None => true,
            Some(value) => value.to_u32() == 0,
        };
        if module.as_str() != "__future__" || !level_is_zero {
            break;
        }
        for alias in &node.names {
            out |= codeop_future_flag_for_name(alias.name.as_str());
        }
    }
    out
}

#[cfg(feature = "stdlib_ast")]
fn codeop_future_flags_from_parsed(parsed: &pyast::Mod) -> i64 {
    match parsed {
        pyast::Mod::Module(module) => codeop_future_flags_from_stmts(&module.body),
        pyast::Mod::Interactive(module) => codeop_future_flags_from_stmts(&module.body),
        _ => 0,
    }
}

#[cfg(feature = "stdlib_ast")]
fn codeop_stmt_is_compound(stmt: &pyast::Stmt) -> bool {
    matches!(
        stmt,
        pyast::Stmt::FunctionDef(_)
            | pyast::Stmt::AsyncFunctionDef(_)
            | pyast::Stmt::ClassDef(_)
            | pyast::Stmt::If(_)
            | pyast::Stmt::For(_)
            | pyast::Stmt::AsyncFor(_)
            | pyast::Stmt::While(_)
            | pyast::Stmt::With(_)
            | pyast::Stmt::AsyncWith(_)
            | pyast::Stmt::Try(_)
            | pyast::Stmt::TryStar(_)
            | pyast::Stmt::Match(_)
    )
}

#[cfg(feature = "stdlib_ast")]
fn codeop_source_incomplete_after_success(source: &str, mode: &str, parsed: &pyast::Mod) -> bool {
    if mode != "single" {
        return false;
    }
    if source.trim_end().ends_with(':') {
        return true;
    }
    if source.contains('\n')
        && !source.ends_with('\n')
        && let pyast::Mod::Interactive(module) = parsed
        && let Some(first) = module.body.first()
    {
        return codeop_stmt_is_compound(first);
    }
    false
}

#[cfg(feature = "stdlib_ast")]
fn codeop_source_has_missing_indented_suite(source: &str) -> bool {
    let lines: Vec<&str> = source.split('\n').collect();
    let leading_indent = |line: &str| -> usize {
        line.chars()
            .take_while(|ch| *ch == ' ' || *ch == '\t')
            .count()
    };

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if !trimmed.ends_with(':') {
            continue;
        }
        let indent = leading_indent(line);
        let mut next_idx = idx + 1;
        while next_idx < lines.len() {
            let next_line = lines[next_idx];
            let next_trimmed = next_line.trim();
            if next_trimmed.is_empty() || next_trimmed.starts_with('#') {
                next_idx += 1;
                continue;
            }
            if leading_indent(next_line) <= indent {
                return true;
            }
            break;
        }
    }
    false
}

#[cfg(feature = "stdlib_ast")]
fn codeop_parse_error_is_incomplete(error: &ParseErrorType, source: &str) -> bool {
    let trimmed = source.trim_end();
    let trailing_backslash_newline = source.ends_with("\\\n") || source.ends_with("\\\r\n");
    match error {
        ParseErrorType::Eof => !trailing_backslash_newline,
        ParseErrorType::UnrecognizedToken(_, expected) => expected.as_deref() == Some("Indent"),
        ParseErrorType::Lexical(lex) => {
            let text = lex.to_string();
            if text.contains("unexpected EOF") {
                return true;
            }
            if text.contains("line continuation") {
                return !trailing_backslash_newline;
            }
            if text.contains("unexpected string") {
                return true;
            }
            (text.contains("expected an indented block")
                || text.contains("unindent does not match any outer indentation level"))
                && trimmed.ends_with(':')
        }
        _ => false,
    }
}

#[cfg(feature = "stdlib_ast")]
enum CodeopCompileStatus {
    Compiled {
        next_flags: i64,
    },
    Incomplete,
    Error {
        error_type: &'static str,
        message: String,
    },
}

#[cfg(feature = "stdlib_ast")]
fn codeop_compile_status(
    source: &str,
    filename: &str,
    mode: &str,
    flags: i64,
    incomplete_input: bool,
) -> CodeopCompileStatus {
    let parse_mode = match mode {
        "exec" => ParseMode::Module,
        "eval" => ParseMode::Expression,
        "single" => ParseMode::Interactive,
        _ => {
            return CodeopCompileStatus::Error {
                error_type: "ValueError",
                message: "compile() mode must be 'exec', 'eval' or 'single'".to_string(),
            };
        }
    };
    match parse_python(source, parse_mode, filename) {
        Ok(parsed) => match compile_validate_nonlocal_semantics(&parsed) {
            Ok(()) => {
                if codeop_source_has_missing_indented_suite(source) {
                    return CodeopCompileStatus::Error {
                        error_type: "SyntaxError",
                        message: "expected an indented block".to_string(),
                    };
                }
                if incomplete_input && codeop_source_incomplete_after_success(source, mode, &parsed)
                {
                    return CodeopCompileStatus::Incomplete;
                }
                CodeopCompileStatus::Compiled {
                    next_flags: flags | codeop_future_flags_from_parsed(&parsed),
                }
            }
            Err(message) => CodeopCompileStatus::Error {
                error_type: "SyntaxError",
                message,
            },
        },
        Err(err) => {
            if incomplete_input && codeop_parse_error_is_incomplete(&err.error, source) {
                CodeopCompileStatus::Incomplete
            } else {
                CodeopCompileStatus::Error {
                    error_type: compile_error_type(&err.error),
                    message: err.error.to_string(),
                }
            }
        }
    }
}

#[cfg(feature = "stdlib_ast")]
fn codeobj_from_filename_bits(_py: &crate::PyToken<'_>, filename_bits: u64) -> u64 {
    let name_ptr = alloc_string(_py, b"<module>");
    if name_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let name_bits = MoltObject::from_ptr(name_ptr).bits();
    let varnames_ptr = alloc_tuple(_py, &[]);
    if varnames_ptr.is_null() {
        dec_ref_bits(_py, name_bits);
        return MoltObject::none().bits();
    }
    let varnames_bits = MoltObject::from_ptr(varnames_ptr).bits();
    let code_ptr = alloc_code_obj(
        _py,
        filename_bits,
        name_bits,
        1,
        MoltObject::none().bits(),
        varnames_bits,
        0,
        0,
        0,
    );
    dec_ref_bits(_py, varnames_bits);
    if code_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(code_ptr).bits()
    }
}

#[cfg(feature = "stdlib_ast")]
fn collect_bound_names_in_target(target: &pyast::Expr, out: &mut HashSet<String>) {
    match target {
        pyast::Expr::Name(node) => {
            out.insert(node.id.as_str().to_string());
        }
        pyast::Expr::Tuple(node) => {
            for elt in &node.elts {
                collect_bound_names_in_target(elt, out);
            }
        }
        pyast::Expr::List(node) => {
            for elt in &node.elts {
                collect_bound_names_in_target(elt, out);
            }
        }
        pyast::Expr::Starred(node) => {
            collect_bound_names_in_target(node.value.as_ref(), out);
        }
        _ => {}
    }
}

#[cfg(feature = "stdlib_ast")]
fn collect_import_binding(alias: &pyast::Alias, out: &mut HashSet<String>) {
    if let Some(asname) = alias.asname.as_ref() {
        out.insert(asname.as_str().to_string());
        return;
    }
    let raw = alias.name.as_str();
    let base = raw.split('.').next().unwrap_or(raw);
    if !base.is_empty() {
        out.insert(base.to_string());
    }
}

#[cfg(feature = "stdlib_ast")]
fn collect_arg_bindings(args: &pyast::Arguments, out: &mut HashSet<String>) {
    for arg in &args.posonlyargs {
        out.insert(arg.def.arg.as_str().to_string());
    }
    for arg in &args.args {
        out.insert(arg.def.arg.as_str().to_string());
    }
    if let Some(vararg) = args.vararg.as_ref() {
        out.insert(vararg.arg.as_str().to_string());
    }
    for arg in &args.kwonlyargs {
        out.insert(arg.def.arg.as_str().to_string());
    }
    if let Some(kwarg) = args.kwarg.as_ref() {
        out.insert(kwarg.arg.as_str().to_string());
    }
}

#[cfg(feature = "stdlib_ast")]
fn collect_function_scope_info(
    stmt: &pyast::Stmt,
    local_bindings: &mut HashSet<String>,
    nonlocal_decls: &mut HashSet<String>,
    global_decls: &mut HashSet<String>,
) {
    match stmt {
        pyast::Stmt::FunctionDef(node) => {
            local_bindings.insert(node.name.as_str().to_string());
        }
        pyast::Stmt::AsyncFunctionDef(node) => {
            local_bindings.insert(node.name.as_str().to_string());
        }
        pyast::Stmt::ClassDef(node) => {
            local_bindings.insert(node.name.as_str().to_string());
        }
        pyast::Stmt::Global(node) => {
            for name in &node.names {
                global_decls.insert(name.as_str().to_string());
            }
        }
        pyast::Stmt::Nonlocal(node) => {
            for name in &node.names {
                nonlocal_decls.insert(name.as_str().to_string());
            }
        }
        pyast::Stmt::Assign(node) => {
            for target in &node.targets {
                collect_bound_names_in_target(target, local_bindings);
            }
        }
        pyast::Stmt::AnnAssign(node) => {
            collect_bound_names_in_target(node.target.as_ref(), local_bindings);
        }
        pyast::Stmt::AugAssign(node) => {
            collect_bound_names_in_target(node.target.as_ref(), local_bindings);
        }
        pyast::Stmt::For(node) => {
            collect_bound_names_in_target(node.target.as_ref(), local_bindings);
            for child in &node.body {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
            for child in &node.orelse {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
        }
        pyast::Stmt::AsyncFor(node) => {
            collect_bound_names_in_target(node.target.as_ref(), local_bindings);
            for child in &node.body {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
            for child in &node.orelse {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
        }
        pyast::Stmt::With(node) => {
            for item in &node.items {
                if let Some(target) = item.optional_vars.as_ref() {
                    collect_bound_names_in_target(target.as_ref(), local_bindings);
                }
            }
            for child in &node.body {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
        }
        pyast::Stmt::AsyncWith(node) => {
            for item in &node.items {
                if let Some(target) = item.optional_vars.as_ref() {
                    collect_bound_names_in_target(target.as_ref(), local_bindings);
                }
            }
            for child in &node.body {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
        }
        pyast::Stmt::If(node) => {
            for child in &node.body {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
            for child in &node.orelse {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
        }
        pyast::Stmt::While(node) => {
            for child in &node.body {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
            for child in &node.orelse {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
        }
        pyast::Stmt::Try(node) => {
            for child in &node.body {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
            for handler in &node.handlers {
                let pyast::ExceptHandler::ExceptHandler(handler) = handler;
                if let Some(name) = handler.name.as_ref() {
                    local_bindings.insert(name.as_str().to_string());
                }
                for child in &handler.body {
                    collect_function_scope_info(
                        child,
                        local_bindings,
                        nonlocal_decls,
                        global_decls,
                    );
                }
            }
            for child in &node.orelse {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
            for child in &node.finalbody {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
        }
        pyast::Stmt::TryStar(node) => {
            for child in &node.body {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
            for handler in &node.handlers {
                let pyast::ExceptHandler::ExceptHandler(handler) = handler;
                if let Some(name) = handler.name.as_ref() {
                    local_bindings.insert(name.as_str().to_string());
                }
                for child in &handler.body {
                    collect_function_scope_info(
                        child,
                        local_bindings,
                        nonlocal_decls,
                        global_decls,
                    );
                }
            }
            for child in &node.orelse {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
            for child in &node.finalbody {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
        }
        pyast::Stmt::Match(node) => {
            for case in &node.cases {
                for child in &case.body {
                    collect_function_scope_info(
                        child,
                        local_bindings,
                        nonlocal_decls,
                        global_decls,
                    );
                }
            }
        }
        pyast::Stmt::Import(node) => {
            for alias in &node.names {
                collect_import_binding(alias, local_bindings);
            }
        }
        pyast::Stmt::ImportFrom(node) => {
            for alias in &node.names {
                collect_import_binding(alias, local_bindings);
            }
        }
        _ => {}
    }
}

#[cfg(feature = "stdlib_ast")]
fn walk_nested_function_scopes(
    stmts: &[pyast::Stmt],
    enclosing_function_bindings: &[HashSet<String>],
) -> Result<(), String> {
    for stmt in stmts {
        match stmt {
            pyast::Stmt::FunctionDef(node) => {
                validate_function_scope(&node.args, &node.body, enclosing_function_bindings)?;
            }
            pyast::Stmt::AsyncFunctionDef(node) => {
                validate_function_scope(&node.args, &node.body, enclosing_function_bindings)?;
            }
            pyast::Stmt::ClassDef(node) => {
                walk_nested_function_scopes(&node.body, enclosing_function_bindings)?;
            }
            pyast::Stmt::If(node) => {
                walk_nested_function_scopes(&node.body, enclosing_function_bindings)?;
                walk_nested_function_scopes(&node.orelse, enclosing_function_bindings)?;
            }
            pyast::Stmt::For(node) => {
                walk_nested_function_scopes(&node.body, enclosing_function_bindings)?;
                walk_nested_function_scopes(&node.orelse, enclosing_function_bindings)?;
            }
            pyast::Stmt::AsyncFor(node) => {
                walk_nested_function_scopes(&node.body, enclosing_function_bindings)?;
                walk_nested_function_scopes(&node.orelse, enclosing_function_bindings)?;
            }
            pyast::Stmt::While(node) => {
                walk_nested_function_scopes(&node.body, enclosing_function_bindings)?;
                walk_nested_function_scopes(&node.orelse, enclosing_function_bindings)?;
            }
            pyast::Stmt::With(node) => {
                walk_nested_function_scopes(&node.body, enclosing_function_bindings)?;
            }
            pyast::Stmt::AsyncWith(node) => {
                walk_nested_function_scopes(&node.body, enclosing_function_bindings)?;
            }
            pyast::Stmt::Try(node) => {
                walk_nested_function_scopes(&node.body, enclosing_function_bindings)?;
                for handler in &node.handlers {
                    let pyast::ExceptHandler::ExceptHandler(handler) = handler;
                    walk_nested_function_scopes(&handler.body, enclosing_function_bindings)?;
                }
                walk_nested_function_scopes(&node.orelse, enclosing_function_bindings)?;
                walk_nested_function_scopes(&node.finalbody, enclosing_function_bindings)?;
            }
            pyast::Stmt::TryStar(node) => {
                walk_nested_function_scopes(&node.body, enclosing_function_bindings)?;
                for handler in &node.handlers {
                    let pyast::ExceptHandler::ExceptHandler(handler) = handler;
                    walk_nested_function_scopes(&handler.body, enclosing_function_bindings)?;
                }
                walk_nested_function_scopes(&node.orelse, enclosing_function_bindings)?;
                walk_nested_function_scopes(&node.finalbody, enclosing_function_bindings)?;
            }
            pyast::Stmt::Match(node) => {
                for case in &node.cases {
                    walk_nested_function_scopes(&case.body, enclosing_function_bindings)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(feature = "stdlib_ast")]
fn validate_control_flow_stmt(
    stmt: &pyast::Stmt,
    in_function: bool,
    in_loop: bool,
) -> Result<(), String> {
    fn validate_delete_target(target: &pyast::Expr) -> Result<(), String> {
        match target {
            pyast::Expr::Name(_) | pyast::Expr::Attribute(_) | pyast::Expr::Subscript(_) => Ok(()),
            pyast::Expr::Tuple(node) => {
                for elt in &node.elts {
                    validate_delete_target(elt)?;
                }
                Ok(())
            }
            pyast::Expr::List(node) => {
                for elt in &node.elts {
                    validate_delete_target(elt)?;
                }
                Ok(())
            }
            pyast::Expr::Constant(_) => Err("cannot delete literal".to_string()),
            _ => Err("cannot delete expression".to_string()),
        }
    }

    fn validate_assign_target(target: &pyast::Expr) -> Result<(), String> {
        match target {
            pyast::Expr::ListComp(_) => Err(
                "cannot assign to list comprehension here. Maybe you meant '==' instead of '='?"
                    .to_string(),
            ),
            pyast::Expr::Tuple(node) => {
                for elt in &node.elts {
                    validate_assign_target(elt)?;
                }
                Ok(())
            }
            pyast::Expr::List(node) => {
                for elt in &node.elts {
                    validate_assign_target(elt)?;
                }
                Ok(())
            }
            pyast::Expr::Starred(node) => validate_assign_target(node.value.as_ref()),
            _ => Ok(()),
        }
    }

    match stmt {
        pyast::Stmt::Return(_) => {
            if !in_function {
                return Err("'return' outside function".to_string());
            }
        }
        pyast::Stmt::Break(_) => {
            if !in_loop {
                return Err("'break' outside loop".to_string());
            }
        }
        pyast::Stmt::Continue(_) => {
            if !in_loop {
                return Err("'continue' not properly in loop".to_string());
            }
        }
        pyast::Stmt::Delete(node) => {
            for target in &node.targets {
                validate_delete_target(target)?;
            }
        }
        pyast::Stmt::Assign(node) => {
            for target in &node.targets {
                validate_assign_target(target)?;
            }
        }
        pyast::Stmt::AnnAssign(node) => {
            validate_assign_target(node.target.as_ref())?;
        }
        pyast::Stmt::AugAssign(node) => {
            validate_assign_target(node.target.as_ref())?;
        }
        pyast::Stmt::FunctionDef(node) => {
            validate_control_flow_stmts(&node.body, true, false)?;
            return Ok(());
        }
        pyast::Stmt::AsyncFunctionDef(node) => {
            validate_control_flow_stmts(&node.body, true, false)?;
            return Ok(());
        }
        pyast::Stmt::ClassDef(node) => {
            validate_control_flow_stmts(&node.body, false, false)?;
            return Ok(());
        }
        pyast::Stmt::If(node) => {
            validate_control_flow_stmts(&node.body, in_function, in_loop)?;
            validate_control_flow_stmts(&node.orelse, in_function, in_loop)?;
            return Ok(());
        }
        pyast::Stmt::For(node) => {
            validate_control_flow_stmts(&node.body, in_function, true)?;
            validate_control_flow_stmts(&node.orelse, in_function, in_loop)?;
            return Ok(());
        }
        pyast::Stmt::AsyncFor(node) => {
            validate_control_flow_stmts(&node.body, in_function, true)?;
            validate_control_flow_stmts(&node.orelse, in_function, in_loop)?;
            return Ok(());
        }
        pyast::Stmt::While(node) => {
            validate_control_flow_stmts(&node.body, in_function, true)?;
            validate_control_flow_stmts(&node.orelse, in_function, in_loop)?;
            return Ok(());
        }
        pyast::Stmt::With(node) => {
            validate_control_flow_stmts(&node.body, in_function, in_loop)?;
            return Ok(());
        }
        pyast::Stmt::AsyncWith(node) => {
            validate_control_flow_stmts(&node.body, in_function, in_loop)?;
            return Ok(());
        }
        pyast::Stmt::Try(node) => {
            validate_control_flow_stmts(&node.body, in_function, in_loop)?;
            for handler in &node.handlers {
                let pyast::ExceptHandler::ExceptHandler(handler) = handler;
                validate_control_flow_stmts(&handler.body, in_function, in_loop)?;
            }
            validate_control_flow_stmts(&node.orelse, in_function, in_loop)?;
            validate_control_flow_stmts(&node.finalbody, in_function, in_loop)?;
            return Ok(());
        }
        pyast::Stmt::TryStar(node) => {
            validate_control_flow_stmts(&node.body, in_function, in_loop)?;
            for handler in &node.handlers {
                let pyast::ExceptHandler::ExceptHandler(handler) = handler;
                validate_control_flow_stmts(&handler.body, in_function, in_loop)?;
            }
            validate_control_flow_stmts(&node.orelse, in_function, in_loop)?;
            validate_control_flow_stmts(&node.finalbody, in_function, in_loop)?;
            return Ok(());
        }
        pyast::Stmt::Match(node) => {
            for case in &node.cases {
                validate_control_flow_stmts(&case.body, in_function, in_loop)?;
            }
            return Ok(());
        }
        _ => {}
    }
    Ok(())
}

#[cfg(feature = "stdlib_ast")]
fn validate_control_flow_stmts(
    stmts: &[pyast::Stmt],
    in_function: bool,
    in_loop: bool,
) -> Result<(), String> {
    for stmt in stmts {
        validate_control_flow_stmt(stmt, in_function, in_loop)?;
    }
    Ok(())
}

#[cfg(feature = "stdlib_ast")]
fn validate_function_scope(
    args: &pyast::Arguments,
    body: &[pyast::Stmt],
    enclosing_function_bindings: &[HashSet<String>],
) -> Result<(), String> {
    let mut local_bindings: HashSet<String> = HashSet::new();
    collect_arg_bindings(args, &mut local_bindings);
    let mut nonlocal_decls: HashSet<String> = HashSet::new();
    let mut global_decls: HashSet<String> = HashSet::new();
    for stmt in body {
        collect_function_scope_info(
            stmt,
            &mut local_bindings,
            &mut nonlocal_decls,
            &mut global_decls,
        );
    }
    for name in &nonlocal_decls {
        if global_decls.contains(name) {
            return Err(format!("name '{name}' is nonlocal and global"));
        }
        let mut found = false;
        for scope in enclosing_function_bindings.iter().rev() {
            if scope.contains(name) {
                found = true;
                break;
            }
        }
        if !found {
            return Err(format!("no binding for nonlocal '{name}' found"));
        }
    }
    for name in &nonlocal_decls {
        local_bindings.remove(name);
    }
    for name in &global_decls {
        local_bindings.remove(name);
    }
    let mut next_enclosing = enclosing_function_bindings.to_vec();
    next_enclosing.push(local_bindings);
    walk_nested_function_scopes(body, &next_enclosing)
}

#[cfg(feature = "stdlib_ast")]
fn compile_validate_nonlocal_semantics(parsed: &pyast::Mod) -> Result<(), String> {
    match parsed {
        pyast::Mod::Module(module) => {
            validate_control_flow_stmts(&module.body, false, false)?;
            walk_nested_function_scopes(&module.body, &[])
        }
        pyast::Mod::Interactive(module) => {
            validate_control_flow_stmts(&module.body, false, false)?;
            walk_nested_function_scopes(&module.body, &[])
        }
        _ => Ok(()),
    }
}

#[cfg(feature = "stdlib_ast")]
fn compile_validate_source(
    source: &str,
    filename: &str,
    mode: &str,
) -> Result<(), (&'static str, String)> {
    let parse_mode = match mode {
        "exec" => ParseMode::Module,
        "eval" => ParseMode::Expression,
        "single" => ParseMode::Interactive,
        _ => {
            return Err((
                "ValueError",
                "compile() mode must be 'exec', 'eval' or 'single'".to_string(),
            ));
        }
    };
    match parse_python(source, parse_mode, filename) {
        Ok(parsed) => match compile_validate_nonlocal_semantics(&parsed) {
            Ok(()) => Ok(()),
            Err(message) => Err(("SyntaxError", message)),
        },
        Err(err) => Err((compile_error_type(&err.error), err.error.to_string())),
    }
}

#[unsafe(no_mangle)]
#[cfg(feature = "stdlib_ast")]
pub extern "C" fn molt_compile_builtin(
    source_bits: u64,
    filename_bits: u64,
    mode_bits: u64,
    flags_bits: u64,
    dont_inherit_bits: u64,
    optimize_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let source = match string_obj_to_owned(obj_from_bits(source_bits)) {
            Some(val) => val,
            None => {
                return raise_exception::<_>(_py, "TypeError", "compile() arg 1 must be a string");
            }
        };
        let filename = match string_obj_to_owned(obj_from_bits(filename_bits)) {
            Some(val) => val,
            None => {
                return raise_exception::<_>(_py, "TypeError", "compile() arg 2 must be a string");
            }
        };
        let mode = match string_obj_to_owned(obj_from_bits(mode_bits)) {
            Some(val) => val,
            None => {
                return raise_exception::<_>(_py, "TypeError", "compile() arg 3 must be a string");
            }
        };
        if mode != "exec" && mode != "eval" && mode != "single" {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "compile() mode must be 'exec', 'eval' or 'single'",
            );
        }
        if to_i64(obj_from_bits(flags_bits)).is_none() {
            return raise_exception::<_>(_py, "TypeError", "compile() arg 4 must be int");
        }
        if to_i64(obj_from_bits(dont_inherit_bits)).is_none() {
            return raise_exception::<_>(_py, "TypeError", "compile() arg 5 must be int");
        }
        if to_i64(obj_from_bits(optimize_bits)).is_none() {
            return raise_exception::<_>(_py, "TypeError", "compile() arg 6 must be int");
        }
        if let Err((error_type, message)) = compile_validate_source(&source, &filename, &mode) {
            return raise_exception::<_>(_py, error_type, &message);
        }
        codeobj_from_filename_bits(_py, filename_bits)
    })
}

#[unsafe(no_mangle)]
#[cfg(feature = "stdlib_ast")]
pub extern "C" fn molt_codeop_compile(
    source_bits: u64,
    filename_bits: u64,
    mode_bits: u64,
    flags_bits: u64,
    incomplete_input_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let source = match string_obj_to_owned(obj_from_bits(source_bits)) {
            Some(value) => value,
            None => {
                return raise_exception::<_>(_py, "TypeError", "compile() arg 1 must be a string");
            }
        };
        let filename = match string_obj_to_owned(obj_from_bits(filename_bits)) {
            Some(value) => value,
            None => {
                return raise_exception::<_>(_py, "TypeError", "compile() arg 2 must be a string");
            }
        };
        let mode = match string_obj_to_owned(obj_from_bits(mode_bits)) {
            Some(value) => value,
            None => {
                return raise_exception::<_>(_py, "TypeError", "compile() arg 3 must be a string");
            }
        };
        let Some(flags) = to_i64(obj_from_bits(flags_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "compile() arg 4 must be int");
        };
        let incomplete_input = is_truthy(_py, obj_from_bits(incomplete_input_bits));
        match codeop_compile_status(&source, &filename, &mode, flags, incomplete_input) {
            CodeopCompileStatus::Compiled { next_flags } => {
                let code_bits = codeobj_from_filename_bits(_py, filename_bits);
                if obj_from_bits(code_bits).is_none() {
                    return MoltObject::none().bits();
                }
                let flags_out_bits = MoltObject::from_int(next_flags).bits();
                let result_ptr = alloc_tuple(_py, &[code_bits, flags_out_bits]);
                dec_ref_bits(_py, code_bits);
                if result_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                MoltObject::from_ptr(result_ptr).bits()
            }
            CodeopCompileStatus::Incomplete => {
                raise_exception::<_>(_py, "SyntaxError", "incomplete input")
            }
            CodeopCompileStatus::Error {
                error_type,
                message,
            } => raise_exception::<_>(_py, error_type, &message),
        }
    })
}

#[unsafe(no_mangle)]
#[cfg(feature = "stdlib_ast")]
pub extern "C" fn molt_codeop_compile_command(
    source_bits: u64,
    filename_bits: u64,
    mode_bits: u64,
    flags_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let mut source = match string_obj_to_owned(obj_from_bits(source_bits)) {
            Some(value) => value,
            None => {
                return raise_exception::<_>(_py, "TypeError", "compile() arg 1 must be a string");
            }
        };
        let filename = match string_obj_to_owned(obj_from_bits(filename_bits)) {
            Some(value) => value,
            None => {
                return raise_exception::<_>(_py, "TypeError", "compile() arg 2 must be a string");
            }
        };
        let mode = match string_obj_to_owned(obj_from_bits(mode_bits)) {
            Some(value) => value,
            None => {
                return raise_exception::<_>(_py, "TypeError", "compile() arg 3 must be a string");
            }
        };
        let Some(flags) = to_i64(obj_from_bits(flags_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "compile() arg 4 must be int");
        };

        let mut only_blank_or_comment = true;
        for line in source.split('\n') {
            let trimmed = line.trim();
            if !trimmed.is_empty() && !trimmed.starts_with('#') {
                only_blank_or_comment = false;
                break;
            }
        }
        if only_blank_or_comment && mode != "eval" {
            source = "pass".to_string();
        }
        if codeop_source_has_missing_indented_suite(&source) {
            return raise_exception::<_>(_py, "SyntaxError", "expected an indented block");
        }

        match codeop_compile_status(&source, &filename, &mode, flags, true) {
            CodeopCompileStatus::Compiled { next_flags } => {
                let code_bits = codeobj_from_filename_bits(_py, filename_bits);
                if obj_from_bits(code_bits).is_none() {
                    return MoltObject::none().bits();
                }
                let flags_out_bits = MoltObject::from_int(next_flags).bits();
                let result_ptr = alloc_tuple(_py, &[code_bits, flags_out_bits]);
                dec_ref_bits(_py, code_bits);
                if result_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(result_ptr).bits();
            }
            CodeopCompileStatus::Incomplete => {}
            CodeopCompileStatus::Error {
                error_type,
                message,
            } => {
                if error_type != "SyntaxError" {
                    return raise_exception::<_>(_py, error_type, &message);
                }
            }
        }

        let source_newline = format!("{source}\n");
        match codeop_compile_status(&source_newline, &filename, &mode, flags, true) {
            CodeopCompileStatus::Compiled { .. } | CodeopCompileStatus::Incomplete => {
                let flags_out_bits = MoltObject::from_int(flags).bits();
                let result_ptr = alloc_tuple(_py, &[MoltObject::none().bits(), flags_out_bits]);
                if result_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(result_ptr).bits();
            }
            CodeopCompileStatus::Error {
                error_type: "SyntaxError",
                ..
            } => {}
            CodeopCompileStatus::Error {
                error_type,
                message,
            } => return raise_exception::<_>(_py, error_type, &message),
        }

        match codeop_compile_status(&source, &filename, &mode, flags, false) {
            CodeopCompileStatus::Compiled { next_flags } => {
                let code_bits = codeobj_from_filename_bits(_py, filename_bits);
                if obj_from_bits(code_bits).is_none() {
                    return MoltObject::none().bits();
                }
                let flags_out_bits = MoltObject::from_int(next_flags).bits();
                let result_ptr = alloc_tuple(_py, &[code_bits, flags_out_bits]);
                dec_ref_bits(_py, code_bits);
                if result_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                MoltObject::from_ptr(result_ptr).bits()
            }
            CodeopCompileStatus::Incomplete => {
                raise_exception::<_>(_py, "SyntaxError", "incomplete input")
            }
            CodeopCompileStatus::Error {
                error_type,
                message,
            } => raise_exception::<_>(_py, error_type, &message),
        }
    })
}

// --- Stubs when stdlib_ast is disabled ---

#[cfg(not(feature = "stdlib_ast"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_compile_builtin(
    _source_bits: u64,
    _filename_bits: u64,
    _mode_bits: u64,
    _flags_bits: u64,
    _dont_inherit_bits: u64,
    _optimize_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_exception::<u64>(_py, "NotImplementedError", "compile() requires the stdlib_ast feature")
    })
}

#[cfg(not(feature = "stdlib_ast"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_codeop_compile(
    _source_bits: u64,
    _filename_bits: u64,
    _mode_bits: u64,
    _flags_bits: u64,
    _incomplete_input_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_exception::<u64>(_py, "NotImplementedError", "compile() requires the stdlib_ast feature")
    })
}

#[cfg(not(feature = "stdlib_ast"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_codeop_compile_command(
    _source_bits: u64,
    _filename_bits: u64,
    _mode_bits: u64,
    _flags_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_exception::<u64>(_py, "NotImplementedError", "compile() requires the stdlib_ast feature")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_func_new(fn_ptr: u64, trampoline_ptr: u64, arity: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = alloc_function_obj(_py, fn_ptr, arity);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            unsafe {
                function_set_trampoline_ptr(ptr, trampoline_ptr);
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_func_new_builtin(fn_ptr: u64, trampoline_ptr: u64, arity: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let trace = matches!(
            std::env::var("MOLT_TRACE_BUILTIN_FUNC").ok().as_deref(),
            Some("1")
        );
        let trace_enter_ptr = fn_addr!(molt_trace_enter_slot);
        if trace {
            eprintln!(
                "molt builtin_func new: fn_ptr=0x{fn_ptr:x} tramp_ptr=0x{trampoline_ptr:x} arity={arity}"
            );
        }
        if fn_ptr == 0 || trampoline_ptr == 0 {
            let msg = format!(
                "builtin func pointer missing: fn=0x{fn_ptr:x} tramp=0x{trampoline_ptr:x} arity={arity}"
            );
            return raise_exception::<_>(_py, "RuntimeError", &msg);
        }
        let ptr = alloc_function_obj(_py, fn_ptr, arity);
        if ptr.is_null() {
            return raise_exception::<_>(_py, "RuntimeError", "builtin func alloc failed");
        }
        unsafe {
            function_set_trampoline_ptr(ptr, trampoline_ptr);
            let builtin_bits = builtin_classes(_py).builtin_function_or_method;
            object_set_class_bits(_py, ptr, builtin_bits);
            inc_ref_bits(_py, builtin_bits);
        }
        let bits = MoltObject::from_ptr(ptr).bits();
        if trace && fn_ptr == trace_enter_ptr {
            eprintln!(
                "molt builtin_func trace_enter_slot bits=0x{bits:x} ptr=0x{:x}",
                ptr as usize
            );
        }
        bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_func_new_closure(
    fn_ptr: u64,
    trampoline_ptr: u64,
    arity: u64,
    closure_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = alloc_function_obj(_py, fn_ptr, arity);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        if closure_bits != 0 && !obj_from_bits(closure_bits).is_none() {
            let cell_bits = cell_class(_py);
            if cell_bits != 0 && !obj_from_bits(cell_bits).is_none() {
                let closure_obj = obj_from_bits(closure_bits);
                if let Some(closure_ptr) = closure_obj.as_ptr() {
                    unsafe {
                        if object_type_id(closure_ptr) == TYPE_ID_TUPLE {
                            for &entry_bits in seq_vec_ref(closure_ptr).iter() {
                                let entry_obj = obj_from_bits(entry_bits);
                                let Some(entry_ptr) = entry_obj.as_ptr() else {
                                    continue;
                                };
                                if object_type_id(entry_ptr) != TYPE_ID_LIST {
                                    continue;
                                }
                                if seq_vec_ref(entry_ptr).len() != 1 {
                                    continue;
                                }
                                let old_class_bits = object_class_bits(entry_ptr);
                                if old_class_bits == cell_bits {
                                    continue;
                                }
                                if old_class_bits != 0 {
                                    dec_ref_bits(_py, old_class_bits);
                                }
                                object_set_class_bits(_py, entry_ptr, cell_bits);
                                inc_ref_bits(_py, cell_bits);
                            }
                        }
                    }
                }
            }
        }
        unsafe {
            function_set_closure_bits(_py, ptr, closure_bits);
            function_set_trampoline_ptr(ptr, trampoline_ptr);
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_function_set_builtin(func_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(func_ptr) = obj_from_bits(func_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected function");
        };
        unsafe {
            if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
                return raise_exception::<_>(_py, "TypeError", "expected function");
            }
            let builtin_bits = builtin_classes(_py).builtin_function_or_method;
            let old_bits = object_class_bits(func_ptr);
            if old_bits != builtin_bits {
                if old_bits != 0 {
                    dec_ref_bits(_py, old_bits);
                }
                object_set_class_bits(_py, func_ptr, builtin_bits);
                inc_ref_bits(_py, builtin_bits);
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_function_get_code(func_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(func_ptr) = obj_from_bits(func_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected function");
        };
        unsafe {
            if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
                return raise_exception::<_>(_py, "TypeError", "expected function");
            }
            let code_bits = ensure_function_code_bits(_py, func_ptr);
            if obj_from_bits(code_bits).is_none() {
                return MoltObject::none().bits();
            }
            inc_ref_bits(_py, code_bits);
            code_bits
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_function_get_globals(func_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(func_ptr) = obj_from_bits(func_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected function");
        };
        unsafe {
            if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
                return raise_exception::<_>(_py, "TypeError", "expected function");
            }
            let dict_bits = function_dict_bits(func_ptr);
            if dict_bits == 0 {
                return MoltObject::none().bits();
            }
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return MoltObject::none().bits();
            }
            let Some(module_name_bits) = attr_name_bits_from_bytes(_py, b"__module__") else {
                return MoltObject::none().bits();
            };
            let Some(name_bits) = dict_get_in_place(_py, dict_ptr, module_name_bits) else {
                return MoltObject::none().bits();
            };
            let name = match string_obj_to_owned(obj_from_bits(name_bits)) {
                Some(val) => val,
                None => return MoltObject::none().bits(),
            };
            let cache = crate::builtins::exceptions::internals::module_cache(_py);
            let guard = cache.lock().unwrap();
            let Some(module_bits) = guard.get(&name) else {
                return MoltObject::none().bits();
            };
            let module_bits = *module_bits;
            inc_ref_bits(_py, module_bits);
            drop(guard);
            let Some(module_ptr) = obj_from_bits(module_bits).as_ptr() else {
                dec_ref_bits(_py, module_bits);
                return MoltObject::none().bits();
            };
            if object_type_id(module_ptr) != TYPE_ID_MODULE {
                dec_ref_bits(_py, module_bits);
                return MoltObject::none().bits();
            }
            let globals_bits = module_dict_bits(module_ptr);
            if obj_from_bits(globals_bits).is_none() {
                dec_ref_bits(_py, module_bits);
                return MoltObject::none().bits();
            }
            inc_ref_bits(_py, globals_bits);
            dec_ref_bits(_py, module_bits);
            globals_bits
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_code_new(
    filename_bits: u64,
    name_bits: u64,
    firstlineno_bits: u64,
    linetable_bits: u64,
    varnames_bits: u64,
    argcount_bits: u64,
    posonlyargcount_bits: u64,
    kwonlyargcount_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let filename_obj = obj_from_bits(filename_bits);
        let Some(filename_ptr) = filename_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "code filename must be str");
        };
        unsafe {
            if object_type_id(filename_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "code filename must be str");
            }
        }
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "code name must be str");
        };
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "code name must be str");
            }
        }
        if !obj_from_bits(linetable_bits).is_none() {
            let Some(table_ptr) = obj_from_bits(linetable_bits).as_ptr() else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "code linetable must be tuple or None",
                );
            };
            unsafe {
                if object_type_id(table_ptr) != TYPE_ID_TUPLE {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "code linetable must be tuple or None",
                    );
                }
            }
        }
        let Some(argcount) = to_i64(obj_from_bits(argcount_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "code argcount must be int");
        };
        let Some(posonlyargcount) = to_i64(obj_from_bits(posonlyargcount_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "code posonlyargcount must be int");
        };
        let Some(kwonlyargcount) = to_i64(obj_from_bits(kwonlyargcount_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "code kwonlyargcount must be int");
        };
        if argcount < 0 || posonlyargcount < 0 || kwonlyargcount < 0 {
            return raise_exception::<_>(_py, "ValueError", "code arg counts must be >= 0");
        }
        let mut varnames_bits = varnames_bits;
        let mut varnames_owned = false;
        if obj_from_bits(varnames_bits).is_none() {
            let tuple_ptr = alloc_tuple(_py, &[]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            varnames_bits = MoltObject::from_ptr(tuple_ptr).bits();
            varnames_owned = true;
        } else {
            let Some(varnames_ptr) = obj_from_bits(varnames_bits).as_ptr() else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "code varnames must be tuple or None",
                );
            };
            unsafe {
                if object_type_id(varnames_ptr) != TYPE_ID_TUPLE {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "code varnames must be tuple or None",
                    );
                }
            }
        }
        let firstlineno = to_i64(obj_from_bits(firstlineno_bits)).unwrap_or(0);
        let ptr = alloc_code_obj(
            _py,
            filename_bits,
            name_bits,
            firstlineno,
            linetable_bits,
            varnames_bits,
            argcount as u64,
            posonlyargcount as u64,
            kwonlyargcount as u64,
        );
        if varnames_owned {
            dec_ref_bits(_py, varnames_bits);
        }
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bound_method_new(func_bits: u64, self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let debug_bound = std::env::var_os("MOLT_DEBUG_BOUND_METHOD").is_some();
        let func_obj = obj_from_bits(func_bits);
        let Some(func_ptr) = func_obj.as_ptr() else {
            if debug_bound {
                let self_obj = obj_from_bits(self_bits);
                let self_label = self_obj
                    .as_ptr()
                    .map(|_| type_name(_py, self_obj).into_owned())
                    .unwrap_or_else(|| format!("immediate:{:#x}", self_bits));
                let self_type_id = self_obj
                    .as_ptr()
                    .map(|ptr| unsafe { object_type_id(ptr) })
                    .unwrap_or(0);
                eprintln!(
                    "molt_bound_method_new: non-object func_bits={:#x} self={} self_type_id={}",
                    func_bits, self_label, self_type_id
                );
                if let Some(name) = crate::builtins::attr::debug_last_attr_name() {
                    eprintln!("molt_bound_method_new last_attr={}", name);
                }
            }
            return raise_exception::<_>(_py, "TypeError", "bound method expects function object");
        };
        unsafe {
            // If func_bits is already a BOUND_METHOD, unwrap to its inner function
            // so we don't fail the TYPE_ID_FUNCTION check below. This happens when
            // inline int/float/bool attribute fallback passes a bound method through
            // the builtin_class_method_bits path.
            if object_type_id(func_ptr) == TYPE_ID_BOUND_METHOD {
                let inner_func_bits = bound_method_func_bits(func_ptr);
                return molt_bound_method_new(inner_func_bits, self_bits);
            }
            if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
                if debug_bound {
                    let type_label = type_name(_py, func_obj).into_owned();
                    let self_label = obj_from_bits(self_bits)
                        .as_ptr()
                        .map(|_| type_name(_py, obj_from_bits(self_bits)).into_owned())
                        .unwrap_or_else(|| format!("immediate:{:#x}", self_bits));
                    eprintln!(
                        "molt_bound_method_new: expected function got type_id={} type={} self={}",
                        object_type_id(func_ptr),
                        type_label,
                        self_label
                    );
                }
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "bound method expects function object",
                );
            }
        }
        let ptr = alloc_bound_method_obj(_py, func_bits, self_bits);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            let method_bits = {
                let func_class_bits = unsafe { object_class_bits(func_ptr) };
                if func_class_bits == builtin_classes(_py).builtin_function_or_method {
                    func_class_bits
                } else {
                    crate::builtins::types::method_class(_py)
                }
            };
            if method_bits != 0 {
                unsafe {
                    let old_bits = object_class_bits(ptr);
                    if old_bits != method_bits {
                        if old_bits != 0 {
                            dec_ref_bits(_py, old_bits);
                        }
                        object_set_class_bits(_py, ptr, method_bits);
                        inc_ref_bits(_py, method_bits);
                    }
                }
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

/// # Safety
/// `self_ptr` must point to a valid closure storage region and `offset` must be
/// within the allocated payload.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_closure_load(self_ptr: *mut u8, offset: u64) -> u64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            if self_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let slot = self_ptr.add(offset as usize) as *mut u64;
            let bits = *slot;
            inc_ref_bits(_py, bits);
            bits
        })
    }
}

/// # Safety
/// `self_ptr` must point to a valid closure storage region and `offset` must be
/// within the allocated payload.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_closure_store(self_ptr: *mut u8, offset: u64, bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            if self_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let slot = self_ptr.add(offset as usize) as *mut u64;
            let old_bits = *slot;
            dec_ref_bits(_py, old_bits);
            inc_ref_bits(_py, bits);
            *slot = bits;
            MoltObject::none().bits()
        })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_csv_runtime_ready() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(true).bits() })
}

fn logging_percent_lookup_mapping_value(
    _py: &crate::PyToken<'_>,
    mapping_ptr: *mut u8,
    key: &str,
) -> Option<u64> {
    let key_ptr = alloc_string(_py, key.as_bytes());
    if key_ptr.is_null() {
        return None;
    }
    let key_bits = MoltObject::from_ptr(key_ptr).bits();
    let value = unsafe { dict_get_in_place(_py, mapping_ptr, key_bits) };
    dec_ref_bits(_py, key_bits);
    value
}

fn logging_percent_render_str(_py: &crate::PyToken<'_>, value_bits: u64) -> Option<String> {
    let rendered_bits = crate::molt_str_from_obj(value_bits);
    if exception_pending(_py) {
        return None;
    }
    let rendered = string_obj_to_owned(obj_from_bits(rendered_bits));
    dec_ref_bits(_py, rendered_bits);
    rendered
}

fn logging_percent_render_repr(_py: &crate::PyToken<'_>, value_bits: u64) -> Option<String> {
    let rendered_bits = crate::molt_repr_from_obj(value_bits);
    if exception_pending(_py) {
        return None;
    }
    let rendered = string_obj_to_owned(obj_from_bits(rendered_bits));
    dec_ref_bits(_py, rendered_bits);
    rendered
}

fn logging_percent_render_value(
    _py: &crate::PyToken<'_>,
    spec: char,
    value_bits: u64,
) -> Option<String> {
    match spec {
        'd' => {
            if let Some(value) = to_i64(obj_from_bits(value_bits)) {
                return Some(value.to_string());
            }
            if exception_pending(_py) {
                clear_exception(_py);
            }
            logging_percent_render_str(_py, value_bits)
        }
        'f' => {
            if let Some(value) = to_f64(obj_from_bits(value_bits)) {
                return Some(format!("{value:.6}"));
            }
            if exception_pending(_py) {
                clear_exception(_py);
            }
            logging_percent_render_str(_py, value_bits)
        }
        'r' => logging_percent_render_repr(_py, value_bits),
        _ => logging_percent_render_str(_py, value_bits),
    }
}

fn logging_config_dict_lookup(
    _py: &crate::PyToken<'_>,
    dict_bits: u64,
    key: &str,
) -> Result<Option<u64>, u64> {
    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "logging config object must be dict",
        ));
    };
    if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "logging config object must be dict",
        ));
    }
    let Some(key_bits) = alloc_string_bits(_py, key) else {
        return Err(MoltObject::none().bits());
    };
    let value = unsafe { dict_get_in_place(_py, dict_ptr, key_bits) };
    dec_ref_bits(_py, key_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(value)
}

fn logging_config_dict_items(
    _py: &crate::PyToken<'_>,
    dict_bits: u64,
) -> Result<Vec<(u64, u64)>, u64> {
    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "logging config section must be dict",
        ));
    };
    if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "logging config section must be dict",
        ));
    }
    let Some(items_name_bits) = attr_name_bits_from_bytes(_py, b"items") else {
        return Err(MoltObject::none().bits());
    };
    let missing = missing_bits(_py);
    let items_method_bits = molt_getattr_builtin(dict_bits, items_name_bits, missing);
    dec_ref_bits(_py, items_name_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if items_method_bits == missing {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "logging config section missing items()",
        ));
    }
    let iterable_bits = unsafe { call_callable0(_py, items_method_bits) };
    dec_ref_bits(_py, items_method_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let list_bits = unsafe { call_callable1(_py, builtin_classes(_py).list, iterable_bits) };
    dec_ref_bits(_py, iterable_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let Some(list_ptr) = obj_from_bits(list_bits).as_ptr() else {
        dec_ref_bits(_py, list_bits);
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "logging config items() must produce an iterable of pairs",
        ));
    };
    if unsafe { object_type_id(list_ptr) } != TYPE_ID_LIST {
        dec_ref_bits(_py, list_bits);
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "logging config items() iterable materialization failed",
        ));
    }
    let entries: Vec<u64> = unsafe { seq_vec_ref(list_ptr).to_vec() };
    let mut pairs: Vec<(u64, u64)> = Vec::new();
    for item_bits in entries {
        let Some(item_ptr) = obj_from_bits(item_bits).as_ptr() else {
            dec_ref_bits(_py, list_bits);
            for (key_bits, value_bits) in pairs {
                dec_ref_bits(_py, key_bits);
                dec_ref_bits(_py, value_bits);
            }
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "logging config items must be pairs",
            ));
        };
        if unsafe { object_type_id(item_ptr) } != TYPE_ID_TUPLE {
            dec_ref_bits(_py, list_bits);
            for (key_bits, value_bits) in pairs {
                dec_ref_bits(_py, key_bits);
                dec_ref_bits(_py, value_bits);
            }
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "logging config items must be pairs",
            ));
        }
        let fields = unsafe { seq_vec_ref(item_ptr) };
        if fields.len() != 2 {
            dec_ref_bits(_py, list_bits);
            for (key_bits, value_bits) in pairs {
                dec_ref_bits(_py, key_bits);
                dec_ref_bits(_py, value_bits);
            }
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "logging config items must be key/value pairs",
            ));
        }
        let key_bits = fields[0];
        let value_bits = fields[1];
        inc_ref_bits(_py, key_bits);
        inc_ref_bits(_py, value_bits);
        pairs.push((key_bits, value_bits));
    }
    dec_ref_bits(_py, list_bits);
    Ok(pairs)
}

fn logging_config_name_list(_py: &crate::PyToken<'_>, seq_bits: u64) -> Result<Vec<String>, u64> {
    let list_bits = unsafe { call_callable1(_py, builtin_classes(_py).list, seq_bits) };
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let Some(list_ptr) = obj_from_bits(list_bits).as_ptr() else {
        dec_ref_bits(_py, list_bits);
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "logging config handler list must be iterable",
        ));
    };
    if unsafe { object_type_id(list_ptr) } != TYPE_ID_LIST {
        dec_ref_bits(_py, list_bits);
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "logging config handler list materialization failed",
        ));
    }
    let entries: Vec<u64> = unsafe { seq_vec_ref(list_ptr).to_vec() };
    let mut names: Vec<String> = Vec::new();
    for item_bits in entries {
        let Some(name) = string_obj_to_owned(obj_from_bits(item_bits)) else {
            dec_ref_bits(_py, list_bits);
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "logging config handler references must be strings",
            ));
        };
        names.push(name);
    }
    dec_ref_bits(_py, list_bits);
    Ok(names)
}

fn logging_config_call_method1(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    method_name: &[u8],
    arg_bits: u64,
) -> Result<u64, u64> {
    let Some(method_bits) = urllib_request_attr_optional(_py, obj_bits, method_name)? else {
        return Err(raise_exception::<u64>(
            _py,
            "AttributeError",
            "logging object method is missing",
        ));
    };
    let out_bits = unsafe { call_callable1(_py, method_bits, arg_bits) };
    dec_ref_bits(_py, method_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(out_bits)
}

fn logging_config_clear_logger_handlers(
    _py: &crate::PyToken<'_>,
    logger_bits: u64,
) -> Result<(), u64> {
    let Some(handlers_bits) = urllib_request_attr_optional(_py, logger_bits, b"handlers")? else {
        return Ok(());
    };
    let Some(handlers_ptr) = obj_from_bits(handlers_bits).as_ptr() else {
        dec_ref_bits(_py, handlers_bits);
        return Ok(());
    };
    let ty = unsafe { object_type_id(handlers_ptr) };
    let snapshot: Vec<u64> = if ty == TYPE_ID_LIST || ty == TYPE_ID_TUPLE {
        unsafe { seq_vec_ref(handlers_ptr).to_vec() }
    } else {
        dec_ref_bits(_py, handlers_bits);
        return Ok(());
    };
    dec_ref_bits(_py, handlers_bits);
    for handler_bits in snapshot {
        let out_bits =
            logging_config_call_method1(_py, logger_bits, b"removeHandler", handler_bits)?;
        if !obj_from_bits(out_bits).is_none() {
            dec_ref_bits(_py, out_bits);
        }
    }
    Ok(())
}

fn logging_config_resolve_ext_stream(
    _py: &crate::PyToken<'_>,
    value_bits: u64,
) -> Result<u64, u64> {
    let Some(text) = string_obj_to_owned(obj_from_bits(value_bits)) else {
        return Ok(value_bits);
    };
    if text == "ext://sys.stdout" {
        return pickle_resolve_global_bits(_py, "sys", "stdout");
    }
    if text == "ext://sys.stderr" {
        return pickle_resolve_global_bits(_py, "sys", "stderr");
    }
    if text == "ext://sys.stdin" {
        return pickle_resolve_global_bits(_py, "sys", "stdin");
    }
    Err(raise_exception::<u64>(
        _py,
        "ValueError",
        "unsupported logging stream ext target",
    ))
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_config_dict(config_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let version_bits = match logging_config_dict_lookup(_py, config_bits, "version") {
            Ok(Some(bits)) => bits,
            Ok(None) => {
                return raise_exception::<_>(_py, "ValueError", "logging config missing version");
            }
            Err(bits) => return bits,
        };
        let Some(version) = to_i64(obj_from_bits(version_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "logging config version must be int");
        };
        if version != 1 {
            return raise_exception::<_>(_py, "ValueError", "unsupported logging config version");
        }

        let formatter_class_bits = match pickle_resolve_global_bits(_py, "logging", "Formatter") {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        let stream_handler_class_bits =
            match pickle_resolve_global_bits(_py, "logging", "StreamHandler") {
                Ok(bits) => bits,
                Err(bits) => {
                    dec_ref_bits(_py, formatter_class_bits);
                    return bits;
                }
            };
        let file_handler_class_bits =
            match pickle_resolve_global_bits(_py, "logging", "FileHandler") {
                Ok(bits) => bits,
                Err(bits) => {
                    dec_ref_bits(_py, stream_handler_class_bits);
                    dec_ref_bits(_py, formatter_class_bits);
                    return bits;
                }
            };
        let get_logger_bits = match pickle_resolve_global_bits(_py, "logging", "getLogger") {
            Ok(bits) => bits,
            Err(bits) => {
                dec_ref_bits(_py, file_handler_class_bits);
                dec_ref_bits(_py, stream_handler_class_bits);
                dec_ref_bits(_py, formatter_class_bits);
                return bits;
            }
        };

        let mut formatter_map: HashMap<String, u64> = HashMap::new();
        let mut handler_map: HashMap<String, u64> = HashMap::new();

        if let Ok(Some(formatters_bits)) =
            logging_config_dict_lookup(_py, config_bits, "formatters")
        {
            let pairs = match logging_config_dict_items(_py, formatters_bits) {
                Ok(items) => items,
                Err(bits) => {
                    dec_ref_bits(_py, get_logger_bits);
                    dec_ref_bits(_py, file_handler_class_bits);
                    dec_ref_bits(_py, stream_handler_class_bits);
                    dec_ref_bits(_py, formatter_class_bits);
                    return bits;
                }
            };
            let Some(formatter_class_ptr) = obj_from_bits(formatter_class_bits).as_ptr() else {
                dec_ref_bits(_py, get_logger_bits);
                dec_ref_bits(_py, file_handler_class_bits);
                dec_ref_bits(_py, stream_handler_class_bits);
                dec_ref_bits(_py, formatter_class_bits);
                return raise_exception::<_>(_py, "TypeError", "logging.Formatter is invalid");
            };
            for (name_bits, cfg_bits) in pairs {
                let name = match string_obj_to_owned(obj_from_bits(name_bits)) {
                    Some(value) => value,
                    None => {
                        dec_ref_bits(_py, name_bits);
                        dec_ref_bits(_py, cfg_bits);
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "logging formatter name must be str",
                        );
                    }
                };
                let fmt_bits = match logging_config_dict_lookup(_py, cfg_bits, "format") {
                    Ok(Some(bits)) => bits,
                    Ok(None) => MoltObject::none().bits(),
                    Err(bits) => {
                        dec_ref_bits(_py, name_bits);
                        dec_ref_bits(_py, cfg_bits);
                        return bits;
                    }
                };
                let formatter_bits =
                    unsafe { call_class_init_with_args(_py, formatter_class_ptr, &[fmt_bits]) };
                if exception_pending(_py) {
                    dec_ref_bits(_py, name_bits);
                    dec_ref_bits(_py, cfg_bits);
                    return MoltObject::none().bits();
                }
                formatter_map.insert(name, formatter_bits);
                dec_ref_bits(_py, name_bits);
                dec_ref_bits(_py, cfg_bits);
            }
        } else if exception_pending(_py) {
            dec_ref_bits(_py, get_logger_bits);
            dec_ref_bits(_py, file_handler_class_bits);
            dec_ref_bits(_py, stream_handler_class_bits);
            dec_ref_bits(_py, formatter_class_bits);
            return MoltObject::none().bits();
        }

        if let Ok(Some(handlers_bits)) = logging_config_dict_lookup(_py, config_bits, "handlers") {
            let pairs = match logging_config_dict_items(_py, handlers_bits) {
                Ok(items) => items,
                Err(bits) => {
                    for (_, formatter_bits) in formatter_map {
                        dec_ref_bits(_py, formatter_bits);
                    }
                    dec_ref_bits(_py, get_logger_bits);
                    dec_ref_bits(_py, file_handler_class_bits);
                    dec_ref_bits(_py, stream_handler_class_bits);
                    dec_ref_bits(_py, formatter_class_bits);
                    return bits;
                }
            };
            let Some(stream_handler_class_ptr) = obj_from_bits(stream_handler_class_bits).as_ptr()
            else {
                for (_, formatter_bits) in formatter_map {
                    dec_ref_bits(_py, formatter_bits);
                }
                dec_ref_bits(_py, get_logger_bits);
                dec_ref_bits(_py, file_handler_class_bits);
                dec_ref_bits(_py, stream_handler_class_bits);
                dec_ref_bits(_py, formatter_class_bits);
                return raise_exception::<_>(_py, "TypeError", "logging.StreamHandler is invalid");
            };
            let Some(file_handler_class_ptr) = obj_from_bits(file_handler_class_bits).as_ptr()
            else {
                for (_, formatter_bits) in formatter_map {
                    dec_ref_bits(_py, formatter_bits);
                }
                dec_ref_bits(_py, get_logger_bits);
                dec_ref_bits(_py, file_handler_class_bits);
                dec_ref_bits(_py, stream_handler_class_bits);
                dec_ref_bits(_py, formatter_class_bits);
                return raise_exception::<_>(_py, "TypeError", "logging.FileHandler is invalid");
            };
            for (name_bits, cfg_bits) in pairs {
                let name = match string_obj_to_owned(obj_from_bits(name_bits)) {
                    Some(value) => value,
                    None => {
                        dec_ref_bits(_py, name_bits);
                        dec_ref_bits(_py, cfg_bits);
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "logging handler name must be str",
                        );
                    }
                };
                let class_bits = match logging_config_dict_lookup(_py, cfg_bits, "class") {
                    Ok(Some(bits)) => bits,
                    Ok(None) => {
                        dec_ref_bits(_py, name_bits);
                        dec_ref_bits(_py, cfg_bits);
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "logging handler config missing class",
                        );
                    }
                    Err(bits) => {
                        dec_ref_bits(_py, name_bits);
                        dec_ref_bits(_py, cfg_bits);
                        return bits;
                    }
                };
                let class_name = match string_obj_to_owned(obj_from_bits(class_bits)) {
                    Some(value) => value,
                    None => {
                        dec_ref_bits(_py, name_bits);
                        dec_ref_bits(_py, cfg_bits);
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "logging handler class must be str",
                        );
                    }
                };
                let handler_bits = if class_name == "logging.StreamHandler" {
                    let stream_arg_bits = match logging_config_dict_lookup(_py, cfg_bits, "stream")
                    {
                        Ok(Some(bits)) => match logging_config_resolve_ext_stream(_py, bits) {
                            Ok(resolved_bits) => resolved_bits,
                            Err(err_bits) => {
                                dec_ref_bits(_py, name_bits);
                                dec_ref_bits(_py, cfg_bits);
                                return err_bits;
                            }
                        },
                        Ok(None) => MoltObject::none().bits(),
                        Err(bits) => {
                            dec_ref_bits(_py, name_bits);
                            dec_ref_bits(_py, cfg_bits);
                            return bits;
                        }
                    };
                    unsafe {
                        call_class_init_with_args(_py, stream_handler_class_ptr, &[stream_arg_bits])
                    }
                } else if class_name == "logging.FileHandler" {
                    let filename_bits = match logging_config_dict_lookup(_py, cfg_bits, "filename")
                    {
                        Ok(Some(bits)) => bits,
                        Ok(None) => {
                            dec_ref_bits(_py, name_bits);
                            dec_ref_bits(_py, cfg_bits);
                            return raise_exception::<_>(
                                _py,
                                "ValueError",
                                "logging FileHandler config missing filename",
                            );
                        }
                        Err(bits) => {
                            dec_ref_bits(_py, name_bits);
                            dec_ref_bits(_py, cfg_bits);
                            return bits;
                        }
                    };
                    let mode_bits = match logging_config_dict_lookup(_py, cfg_bits, "mode") {
                        Ok(Some(bits)) => bits,
                        Ok(None) => match alloc_string_bits(_py, "a") {
                            Some(bits) => bits,
                            None => {
                                dec_ref_bits(_py, name_bits);
                                dec_ref_bits(_py, cfg_bits);
                                return MoltObject::none().bits();
                            }
                        },
                        Err(bits) => {
                            dec_ref_bits(_py, name_bits);
                            dec_ref_bits(_py, cfg_bits);
                            return bits;
                        }
                    };
                    let out_bits = unsafe {
                        call_class_init_with_args(
                            _py,
                            file_handler_class_ptr,
                            &[filename_bits, mode_bits],
                        )
                    };
                    if let Ok(None) = logging_config_dict_lookup(_py, cfg_bits, "mode") {
                        dec_ref_bits(_py, mode_bits);
                    }
                    out_bits
                } else {
                    dec_ref_bits(_py, name_bits);
                    dec_ref_bits(_py, cfg_bits);
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "unsupported logging handler class for intrinsic dictConfig",
                    );
                };
                if exception_pending(_py) {
                    dec_ref_bits(_py, name_bits);
                    dec_ref_bits(_py, cfg_bits);
                    return MoltObject::none().bits();
                }
                if let Ok(Some(level_bits)) = logging_config_dict_lookup(_py, cfg_bits, "level") {
                    let out_bits = match logging_config_call_method1(
                        _py,
                        handler_bits,
                        b"setLevel",
                        level_bits,
                    ) {
                        Ok(bits) => bits,
                        Err(bits) => {
                            dec_ref_bits(_py, name_bits);
                            dec_ref_bits(_py, cfg_bits);
                            dec_ref_bits(_py, handler_bits);
                            return bits;
                        }
                    };
                    if !obj_from_bits(out_bits).is_none() {
                        dec_ref_bits(_py, out_bits);
                    }
                }
                if let Ok(Some(formatter_name_bits)) =
                    logging_config_dict_lookup(_py, cfg_bits, "formatter")
                {
                    let Some(formatter_name) =
                        string_obj_to_owned(obj_from_bits(formatter_name_bits))
                    else {
                        dec_ref_bits(_py, name_bits);
                        dec_ref_bits(_py, cfg_bits);
                        dec_ref_bits(_py, handler_bits);
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "logging formatter reference must be str",
                        );
                    };
                    let Some(formatter_bits) = formatter_map.get(&formatter_name).copied() else {
                        dec_ref_bits(_py, name_bits);
                        dec_ref_bits(_py, cfg_bits);
                        dec_ref_bits(_py, handler_bits);
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "unknown formatter in logging handler config",
                        );
                    };
                    let out_bits = match logging_config_call_method1(
                        _py,
                        handler_bits,
                        b"setFormatter",
                        formatter_bits,
                    ) {
                        Ok(bits) => bits,
                        Err(bits) => {
                            dec_ref_bits(_py, name_bits);
                            dec_ref_bits(_py, cfg_bits);
                            dec_ref_bits(_py, handler_bits);
                            return bits;
                        }
                    };
                    if !obj_from_bits(out_bits).is_none() {
                        dec_ref_bits(_py, out_bits);
                    }
                }
                handler_map.insert(name, handler_bits);
                dec_ref_bits(_py, name_bits);
                dec_ref_bits(_py, cfg_bits);
            }
        } else if exception_pending(_py) {
            for (_, formatter_bits) in formatter_map {
                dec_ref_bits(_py, formatter_bits);
            }
            dec_ref_bits(_py, get_logger_bits);
            dec_ref_bits(_py, file_handler_class_bits);
            dec_ref_bits(_py, stream_handler_class_bits);
            dec_ref_bits(_py, formatter_class_bits);
            return MoltObject::none().bits();
        }

        if let Ok(Some(loggers_bits)) = logging_config_dict_lookup(_py, config_bits, "loggers") {
            let pairs = match logging_config_dict_items(_py, loggers_bits) {
                Ok(items) => items,
                Err(bits) => {
                    for (_, handler_bits) in handler_map {
                        dec_ref_bits(_py, handler_bits);
                    }
                    for (_, formatter_bits) in formatter_map {
                        dec_ref_bits(_py, formatter_bits);
                    }
                    dec_ref_bits(_py, get_logger_bits);
                    dec_ref_bits(_py, file_handler_class_bits);
                    dec_ref_bits(_py, stream_handler_class_bits);
                    dec_ref_bits(_py, formatter_class_bits);
                    return bits;
                }
            };
            for (name_bits, cfg_bits) in pairs {
                let logger_bits = unsafe { call_callable1(_py, get_logger_bits, name_bits) };
                if exception_pending(_py) {
                    dec_ref_bits(_py, name_bits);
                    dec_ref_bits(_py, cfg_bits);
                    return MoltObject::none().bits();
                }
                if let Err(bits) = logging_config_clear_logger_handlers(_py, logger_bits) {
                    dec_ref_bits(_py, logger_bits);
                    dec_ref_bits(_py, name_bits);
                    dec_ref_bits(_py, cfg_bits);
                    return bits;
                }
                if let Ok(Some(handler_list_bits)) =
                    logging_config_dict_lookup(_py, cfg_bits, "handlers")
                {
                    let handler_names = match logging_config_name_list(_py, handler_list_bits) {
                        Ok(value) => value,
                        Err(bits) => {
                            dec_ref_bits(_py, logger_bits);
                            dec_ref_bits(_py, name_bits);
                            dec_ref_bits(_py, cfg_bits);
                            return bits;
                        }
                    };
                    for handler_name in handler_names {
                        let Some(handler_bits) = handler_map.get(&handler_name).copied() else {
                            dec_ref_bits(_py, logger_bits);
                            dec_ref_bits(_py, name_bits);
                            dec_ref_bits(_py, cfg_bits);
                            return raise_exception::<_>(
                                _py,
                                "ValueError",
                                "unknown handler in logger config",
                            );
                        };
                        let out_bits = match logging_config_call_method1(
                            _py,
                            logger_bits,
                            b"addHandler",
                            handler_bits,
                        ) {
                            Ok(bits) => bits,
                            Err(bits) => {
                                dec_ref_bits(_py, logger_bits);
                                dec_ref_bits(_py, name_bits);
                                dec_ref_bits(_py, cfg_bits);
                                return bits;
                            }
                        };
                        if !obj_from_bits(out_bits).is_none() {
                            dec_ref_bits(_py, out_bits);
                        }
                    }
                }
                if let Ok(Some(level_bits)) = logging_config_dict_lookup(_py, cfg_bits, "level") {
                    let out_bits = match logging_config_call_method1(
                        _py,
                        logger_bits,
                        b"setLevel",
                        level_bits,
                    ) {
                        Ok(bits) => bits,
                        Err(bits) => {
                            dec_ref_bits(_py, logger_bits);
                            dec_ref_bits(_py, name_bits);
                            dec_ref_bits(_py, cfg_bits);
                            return bits;
                        }
                    };
                    if !obj_from_bits(out_bits).is_none() {
                        dec_ref_bits(_py, out_bits);
                    }
                }
                dec_ref_bits(_py, logger_bits);
                dec_ref_bits(_py, name_bits);
                dec_ref_bits(_py, cfg_bits);
            }
        } else if exception_pending(_py) {
            for (_, handler_bits) in handler_map {
                dec_ref_bits(_py, handler_bits);
            }
            for (_, formatter_bits) in formatter_map {
                dec_ref_bits(_py, formatter_bits);
            }
            dec_ref_bits(_py, get_logger_bits);
            dec_ref_bits(_py, file_handler_class_bits);
            dec_ref_bits(_py, stream_handler_class_bits);
            dec_ref_bits(_py, formatter_class_bits);
            return MoltObject::none().bits();
        }

        if let Ok(Some(root_bits)) = logging_config_dict_lookup(_py, config_bits, "root") {
            let root_logger_bits = unsafe { call_callable0(_py, get_logger_bits) };
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if let Err(bits) = logging_config_clear_logger_handlers(_py, root_logger_bits) {
                dec_ref_bits(_py, root_logger_bits);
                return bits;
            }
            if let Ok(Some(handler_list_bits)) =
                logging_config_dict_lookup(_py, root_bits, "handlers")
            {
                let handler_names = match logging_config_name_list(_py, handler_list_bits) {
                    Ok(value) => value,
                    Err(bits) => {
                        dec_ref_bits(_py, root_logger_bits);
                        return bits;
                    }
                };
                for handler_name in handler_names {
                    let Some(handler_bits) = handler_map.get(&handler_name).copied() else {
                        dec_ref_bits(_py, root_logger_bits);
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "unknown handler in root logger config",
                        );
                    };
                    let out_bits = match logging_config_call_method1(
                        _py,
                        root_logger_bits,
                        b"addHandler",
                        handler_bits,
                    ) {
                        Ok(bits) => bits,
                        Err(bits) => {
                            dec_ref_bits(_py, root_logger_bits);
                            return bits;
                        }
                    };
                    if !obj_from_bits(out_bits).is_none() {
                        dec_ref_bits(_py, out_bits);
                    }
                }
            }
            if let Ok(Some(level_bits)) = logging_config_dict_lookup(_py, root_bits, "level") {
                let out_bits = match logging_config_call_method1(
                    _py,
                    root_logger_bits,
                    b"setLevel",
                    level_bits,
                ) {
                    Ok(bits) => bits,
                    Err(bits) => {
                        dec_ref_bits(_py, root_logger_bits);
                        return bits;
                    }
                };
                if !obj_from_bits(out_bits).is_none() {
                    dec_ref_bits(_py, out_bits);
                }
            }
            dec_ref_bits(_py, root_logger_bits);
        } else if exception_pending(_py) {
            return MoltObject::none().bits();
        }

        for (_, handler_bits) in handler_map {
            dec_ref_bits(_py, handler_bits);
        }
        for (_, formatter_bits) in formatter_map {
            dec_ref_bits(_py, formatter_bits);
        }
        dec_ref_bits(_py, get_logger_bits);
        dec_ref_bits(_py, file_handler_class_bits);
        dec_ref_bits(_py, stream_handler_class_bits);
        dec_ref_bits(_py, formatter_class_bits);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_config_valid_ident(value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(value_bits)) else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "logging.config.valid_ident expects str",
            );
        };
        let mut chars = text.chars();
        let Some(first) = chars.next() else {
            return MoltObject::from_bool(false).bits();
        };
        let first_ok = first == '_' || first.is_ascii_alphabetic();
        if !first_ok {
            return MoltObject::from_bool(false).bits();
        }
        for ch in chars {
            if ch != '_' && !ch.is_ascii_alphanumeric() {
                return MoltObject::from_bool(false).bits();
            }
        }
        MoltObject::from_bool(true).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_config_file_config(
    config_file_bits: u64,
    defaults_bits: u64,
    disable_existing_loggers_bits: u64,
    encoding_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let _ = (
            config_file_bits,
            defaults_bits,
            disable_existing_loggers_bits,
            encoding_bits,
        );
        raise_exception::<_>(
            _py,
            "NotImplementedError",
            "logging.config.fileConfig is not implemented in Molt yet",
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_config_listen(port_bits: u64, verify_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let _ = (port_bits, verify_bits);
        raise_exception::<_>(
            _py,
            "NotImplementedError",
            "logging.config.listen is not implemented in Molt yet",
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_config_stop_listening() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::none().bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_percent_style_format(fmt_bits: u64, mapping_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(fmt) = string_obj_to_owned(obj_from_bits(fmt_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "logging format string must be str");
        };
        let Some(mapping_ptr) = obj_from_bits(mapping_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "logging mapping must be dict");
        };
        if unsafe { object_type_id(mapping_ptr) } != TYPE_ID_DICT {
            return raise_exception::<_>(_py, "TypeError", "logging mapping must be dict");
        }

        let chars: Vec<char> = fmt.chars().collect();
        let mut out = String::with_capacity(fmt.len());
        let mut idx = 0usize;

        while idx < chars.len() {
            let ch = chars[idx];
            if ch != '%' {
                out.push(ch);
                idx += 1;
                continue;
            }
            if idx + 1 >= chars.len() {
                out.push('%');
                break;
            }
            if chars[idx + 1] == '%' {
                out.push('%');
                idx += 2;
                continue;
            }
            if chars[idx + 1] != '(' {
                out.push('%');
                idx += 1;
                continue;
            }
            let mut close = idx + 2;
            while close < chars.len() && chars[close] != ')' {
                close += 1;
            }
            if close >= chars.len() || close + 1 >= chars.len() {
                for ch in &chars[idx..] {
                    out.push(*ch);
                }
                break;
            }

            let spec = chars[close + 1];
            let token: String = chars[idx..=close + 1].iter().collect();
            if !matches!(spec, 's' | 'd' | 'r' | 'f') {
                out.push_str(token.as_str());
                idx = close + 2;
                continue;
            }

            let key: String = chars[idx + 2..close].iter().collect();
            let Some(value_bits) =
                logging_percent_lookup_mapping_value(_py, mapping_ptr, key.as_str())
            else {
                out.push_str(token.as_str());
                idx = close + 2;
                continue;
            };

            let Some(rendered) = logging_percent_render_value(_py, spec, value_bits) else {
                return MoltObject::none().bits();
            };
            out.push_str(rendered.as_str());
            idx = close + 2;
        }

        let out_ptr = alloc_string(_py, out.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zipfile_crc32(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(data_ptr) = obj_from_bits(data_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "zipfile crc32 expects bytes-like");
        };
        let Some(bytes) = (unsafe { bytes_like_slice(data_ptr) }) else {
            return raise_exception::<_>(_py, "TypeError", "zipfile crc32 expects bytes-like");
        };

        let mut crc = 0xFFFF_FFFFu32;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                if (crc & 1) != 0 {
                    crc = (crc >> 1) ^ 0xEDB8_8320;
                } else {
                    crc >>= 1;
                }
            }
        }
        crc ^= 0xFFFF_FFFF;
        MoltObject::from_int(i64::from(crc)).bits()
    })
}

fn imghdr_detect_kind(header: &[u8]) -> Option<&'static str> {
    if header.len() >= 10 && (header[6..10] == *b"JFIF" || header[6..10] == *b"Exif")
        || header.starts_with(b"\xFF\xD8\xFF\xDB")
    {
        return Some("jpeg");
    }
    if header.starts_with(b"\x89PNG\r\n\x1A\n") {
        return Some("png");
    }
    if header.len() >= 6 && (header[..6] == *b"GIF87a" || header[..6] == *b"GIF89a") {
        return Some("gif");
    }
    if header.len() >= 2 && (header[..2] == *b"MM" || header[..2] == *b"II") {
        return Some("tiff");
    }
    if header.starts_with(b"\x01\xDA") {
        return Some("rgb");
    }
    if header.len() >= 3
        && header[0] == b'P'
        && matches!(header[1], b'1' | b'4')
        && matches!(header[2], b' ' | b'\t' | b'\n' | b'\r')
    {
        return Some("pbm");
    }
    if header.len() >= 3
        && header[0] == b'P'
        && matches!(header[1], b'2' | b'5')
        && matches!(header[2], b' ' | b'\t' | b'\n' | b'\r')
    {
        return Some("pgm");
    }
    if header.len() >= 3
        && header[0] == b'P'
        && matches!(header[1], b'3' | b'6')
        && matches!(header[2], b' ' | b'\t' | b'\n' | b'\r')
    {
        return Some("ppm");
    }
    if header.starts_with(b"\x59\xA6\x6A\x95") {
        return Some("rast");
    }
    if header.starts_with(b"#define ") {
        return Some("xbm");
    }
    if header.starts_with(b"BM") {
        return Some("bmp");
    }
    if header.starts_with(b"RIFF") && header.len() >= 12 && header[8..12] == *b"WEBP" {
        return Some("webp");
    }
    if header.starts_with(b"\x76\x2f\x31\x01") {
        return Some("exr");
    }
    None
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_imghdr_detect(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(data_ptr) = obj_from_bits(data_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "imghdr header must be bytes-like");
        };
        let Some(header) = (unsafe { bytes_like_slice(data_ptr) }) else {
            return raise_exception::<_>(_py, "TypeError", "imghdr header must be bytes-like");
        };
        let Some(kind) = imghdr_detect_kind(header) else {
            return MoltObject::none().bits();
        };
        let ptr = alloc_string(_py, kind.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

const ZIPFILE_CENTRAL_SIG: [u8; 4] = *b"PK\x01\x02";
const ZIPFILE_EOCD_SIG: [u8; 4] = *b"PK\x05\x06";
const ZIPFILE_ZIP64_EOCD_SIG: [u8; 4] = *b"PK\x06\x06";
const ZIPFILE_ZIP64_LOCATOR_SIG: [u8; 4] = *b"PK\x06\x07";
const ZIPFILE_ZIP64_LIMIT: u64 = 0xFFFF_FFFF;
const ZIPFILE_ZIP64_COUNT_LIMIT: u16 = 0xFFFF;
const ZIPFILE_ZIP64_EXTRA_ID: u16 = 0x0001;

fn zipfile_read_u16_le(data: &[u8], offset: usize, err: &'static str) -> Result<u16, &'static str> {
    let end = offset.checked_add(2).ok_or(err)?;
    let raw = data.get(offset..end).ok_or(err)?;
    Ok(u16::from_le_bytes([raw[0], raw[1]]))
}

fn zipfile_read_u32_le(data: &[u8], offset: usize, err: &'static str) -> Result<u32, &'static str> {
    let end = offset.checked_add(4).ok_or(err)?;
    let raw = data.get(offset..end).ok_or(err)?;
    Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

fn zipfile_read_u64_le(data: &[u8], offset: usize, err: &'static str) -> Result<u64, &'static str> {
    let end = offset.checked_add(8).ok_or(err)?;
    let raw = data.get(offset..end).ok_or(err)?;
    Ok(u64::from_le_bytes([
        raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
    ]))
}

fn zipfile_find_eocd_offset(data: &[u8]) -> Option<usize> {
    if data.len() < 22 {
        return None;
    }
    let start = data.len().saturating_sub(22 + 65_535);
    data[start..]
        .windows(4)
        .rposition(|window| window == ZIPFILE_EOCD_SIG)
        .map(|idx| start + idx)
}

fn zipfile_read_zip64_eocd(
    data: &[u8],
    eocd_offset: usize,
) -> Result<(usize, usize), &'static str> {
    let locator_offset = eocd_offset.checked_sub(20).ok_or("zip64 locator missing")?;
    let locator_sig = data
        .get(locator_offset..locator_offset + 4)
        .ok_or("zip64 locator missing")?;
    if locator_sig != ZIPFILE_ZIP64_LOCATOR_SIG {
        return Err("zip64 locator missing");
    }
    let zip64_eocd_offset = zipfile_read_u64_le(data, locator_offset + 8, "zip64 locator missing")?;
    let zip64_eocd_offset =
        usize::try_from(zip64_eocd_offset).map_err(|_| "zip64 eocd offset overflow")?;
    let eocd_sig = data
        .get(zip64_eocd_offset..zip64_eocd_offset + 4)
        .ok_or("zip64 eocd missing")?;
    if eocd_sig != ZIPFILE_ZIP64_EOCD_SIG {
        return Err("zip64 eocd missing");
    }
    let cd_size = zipfile_read_u64_le(data, zip64_eocd_offset + 40, "zip64 eocd missing")?;
    let cd_offset = zipfile_read_u64_le(data, zip64_eocd_offset + 48, "zip64 eocd missing")?;
    let cd_size = usize::try_from(cd_size).map_err(|_| "zip64 central directory too large")?;
    let cd_offset =
        usize::try_from(cd_offset).map_err(|_| "zip64 central directory offset too large")?;
    Ok((cd_offset, cd_size))
}

fn zipfile_parse_zip64_extra(
    extra: &[u8],
    mut comp_size: u64,
    mut uncomp_size: u64,
    mut local_offset: u64,
) -> Result<(u64, u64, u64), &'static str> {
    let mut pos = 0usize;
    while pos + 4 <= extra.len() {
        let header_id = zipfile_read_u16_le(extra, pos, "zip64 extra missing")?;
        let data_size = usize::from(zipfile_read_u16_le(extra, pos + 2, "zip64 extra missing")?);
        pos += 4;
        let Some(data_end) = pos.checked_add(data_size) else {
            return Err("zip64 extra missing");
        };
        if data_end > extra.len() {
            break;
        }
        if header_id == ZIPFILE_ZIP64_EXTRA_ID {
            let mut cursor = pos;
            if uncomp_size == ZIPFILE_ZIP64_LIMIT {
                if cursor + 8 > data_end {
                    return Err("zip64 extra missing size");
                }
                uncomp_size = zipfile_read_u64_le(extra, cursor, "zip64 extra missing size")?;
                cursor += 8;
            }
            if comp_size == ZIPFILE_ZIP64_LIMIT {
                if cursor + 8 > data_end {
                    return Err("zip64 extra missing comp size");
                }
                comp_size = zipfile_read_u64_le(extra, cursor, "zip64 extra missing comp size")?;
                cursor += 8;
            }
            if local_offset == ZIPFILE_ZIP64_LIMIT {
                if cursor + 8 > data_end {
                    return Err("zip64 extra missing offset");
                }
                local_offset = zipfile_read_u64_le(extra, cursor, "zip64 extra missing offset")?;
            }
            return Ok((comp_size, uncomp_size, local_offset));
        }
        pos = data_end;
    }
    Err("zip64 extra missing")
}

fn zipfile_parse_central_directory_impl(
    data: &[u8],
) -> Result<Vec<(String, [u64; 5])>, &'static str> {
    if data.len() < 22 {
        return Err("file is not a zip file");
    }
    let Some(eocd_offset) = zipfile_find_eocd_offset(data) else {
        return Err("end of central directory not found");
    };

    let mut cd_size = u64::from(zipfile_read_u32_le(
        data,
        eocd_offset + 12,
        "end of central directory not found",
    )?);
    let mut cd_offset = u64::from(zipfile_read_u32_le(
        data,
        eocd_offset + 16,
        "end of central directory not found",
    )?);
    let total_entries =
        zipfile_read_u16_le(data, eocd_offset + 10, "end of central directory not found")?;
    if total_entries == ZIPFILE_ZIP64_COUNT_LIMIT
        || cd_size == ZIPFILE_ZIP64_LIMIT
        || cd_offset == ZIPFILE_ZIP64_LIMIT
    {
        let (zip64_offset, zip64_size) = zipfile_read_zip64_eocd(data, eocd_offset)?;
        cd_offset = zip64_offset as u64;
        cd_size = zip64_size as u64;
    }

    let pos_start = usize::try_from(cd_offset).map_err(|_| "central directory offset overflow")?;
    let cd_size = usize::try_from(cd_size).map_err(|_| "central directory size overflow")?;
    let Some(end) = pos_start.checked_add(cd_size) else {
        return Err("central directory overflow");
    };
    if end > data.len() {
        return Err("end of central directory not found");
    }

    let mut out: Vec<(String, [u64; 5])> = Vec::new();
    let mut pos = pos_start;
    while pos + 46 <= end {
        if data[pos..pos + 4] != ZIPFILE_CENTRAL_SIG {
            break;
        }
        let comp_method = u64::from(zipfile_read_u16_le(
            data,
            pos + 10,
            "invalid central directory entry",
        )?);
        let mut comp_size = u64::from(zipfile_read_u32_le(
            data,
            pos + 20,
            "invalid central directory entry",
        )?);
        let mut uncomp_size = u64::from(zipfile_read_u32_le(
            data,
            pos + 24,
            "invalid central directory entry",
        )?);
        let name_len = usize::from(zipfile_read_u16_le(
            data,
            pos + 28,
            "invalid central directory entry",
        )?);
        let extra_len = usize::from(zipfile_read_u16_le(
            data,
            pos + 30,
            "invalid central directory entry",
        )?);
        let comment_len = usize::from(zipfile_read_u16_le(
            data,
            pos + 32,
            "invalid central directory entry",
        )?);
        let mut local_offset = u64::from(zipfile_read_u32_le(
            data,
            pos + 42,
            "invalid central directory entry",
        )?);

        let name_start = pos + 46;
        let Some(name_end) = name_start.checked_add(name_len) else {
            return Err("invalid central directory entry");
        };
        let Some(extra_end) = name_end.checked_add(extra_len) else {
            return Err("invalid central directory entry");
        };
        let Some(record_end) = extra_end.checked_add(comment_len) else {
            return Err("invalid central directory entry");
        };
        if record_end > end || record_end > data.len() {
            return Err("invalid central directory entry");
        }

        let name_bytes = &data[name_start..name_end];
        let name = match std::str::from_utf8(name_bytes) {
            Ok(value) => value.to_string(),
            Err(_) => String::from_utf8_lossy(name_bytes).into_owned(),
        };

        if comp_size == ZIPFILE_ZIP64_LIMIT
            || uncomp_size == ZIPFILE_ZIP64_LIMIT
            || local_offset == ZIPFILE_ZIP64_LIMIT
        {
            let extra = &data[name_end..extra_end];
            let (parsed_comp, parsed_uncomp, parsed_offset) =
                zipfile_parse_zip64_extra(extra, comp_size, uncomp_size, local_offset)?;
            comp_size = parsed_comp;
            uncomp_size = parsed_uncomp;
            local_offset = parsed_offset;
        }

        out.push((
            name,
            [
                local_offset,
                comp_size,
                comp_method,
                name_len as u64,
                uncomp_size,
            ],
        ));
        pos = record_end;
    }
    Ok(out)
}

fn zipfile_build_zip64_extra_impl(size: u64, comp_size: u64, offset: Option<u64>) -> Vec<u8> {
    let mut data: Vec<u8> = Vec::with_capacity(if offset.is_some() { 24 } else { 16 });
    data.extend_from_slice(size.to_le_bytes().as_slice());
    data.extend_from_slice(comp_size.to_le_bytes().as_slice());
    if let Some(offset) = offset {
        data.extend_from_slice(offset.to_le_bytes().as_slice());
    }
    let mut out: Vec<u8> = Vec::with_capacity(4 + data.len());
    out.extend_from_slice(ZIPFILE_ZIP64_EXTRA_ID.to_le_bytes().as_slice());
    out.extend_from_slice((data.len() as u16).to_le_bytes().as_slice());
    out.extend_from_slice(data.as_slice());
    out
}

fn zipfile_trim_trailing_slashes(path: &str) -> &str {
    let mut end = path.len();
    while end > 0 && path.as_bytes()[end - 1] == b'/' {
        end -= 1;
    }
    &path[..end]
}

fn zipfile_ancestry(path: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = zipfile_trim_trailing_slashes(path).to_string();
    while !zipfile_trim_trailing_slashes(current.as_str()).is_empty() {
        out.push(current.clone());
        if let Some(idx) = current.rfind('/') {
            current.truncate(idx);
            while current.ends_with('/') {
                current.pop();
            }
        } else {
            current.clear();
        }
    }
    out
}

fn zipfile_parent_of(path: &str) -> &str {
    let trimmed = zipfile_trim_trailing_slashes(path);
    if let Some(idx) = trimmed.rfind('/') {
        &trimmed[..idx]
    } else {
        ""
    }
}

fn zipfile_escape_regex_char(ch: char, out: &mut String) {
    if matches!(
        ch,
        '.' | '^' | '$' | '*' | '+' | '?' | '{' | '}' | '[' | ']' | '\\' | '|' | '(' | ')'
    ) {
        out.push('\\');
    }
    out.push(ch);
}

fn zipfile_escape_character_class(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if matches!(ch, '\\' | ']' | '^' | '-') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn zipfile_separate_pattern(pattern: &str) -> Vec<(bool, String)> {
    let bytes = pattern.as_bytes();
    let mut idx = 0usize;
    let mut out: Vec<(bool, String)> = Vec::new();
    while idx < bytes.len() {
        if bytes[idx] != b'[' {
            let start = idx;
            while idx < bytes.len() && bytes[idx] != b'[' {
                idx += 1;
            }
            out.push((false, pattern[start..idx].to_string()));
            continue;
        }
        let start = idx;
        idx += 1;
        while idx < bytes.len() && bytes[idx] != b']' {
            idx += 1;
        }
        if idx < bytes.len() && bytes[idx] == b']' {
            idx += 1;
            out.push((true, pattern[start..idx].to_string()));
            continue;
        }
        out.push((false, pattern[start..].to_string()));
        break;
    }
    out
}

fn zipfile_translate_chunk(chunk: &str, star_pattern: &str, qmark_pattern: &str) -> String {
    let chars: Vec<char> = chunk.chars().collect();
    let mut idx = 0usize;
    let mut out = String::new();
    while idx < chars.len() {
        match chars[idx] {
            '*' => {
                if idx + 1 < chars.len() && chars[idx + 1] == '*' {
                    out.push_str(".*");
                    idx += 2;
                } else {
                    out.push_str(star_pattern);
                    idx += 1;
                }
            }
            '?' => {
                out.push_str(qmark_pattern);
                idx += 1;
            }
            ch => {
                zipfile_escape_regex_char(ch, &mut out);
                idx += 1;
            }
        }
    }
    out
}

fn zipfile_star_not_empty(pattern: &str, seps: &str) -> String {
    let mut out = String::new();
    let mut segment = String::new();
    for ch in pattern.chars() {
        if seps.contains(ch) {
            if !segment.is_empty() {
                if segment == "*" {
                    out.push_str("?*");
                } else {
                    out.push_str(segment.as_str());
                }
                segment.clear();
            }
            out.push(ch);
        } else {
            segment.push(ch);
        }
    }
    if !segment.is_empty() {
        if segment == "*" {
            out.push_str("?*");
        } else {
            out.push_str(segment.as_str());
        }
    }
    out
}

fn zipfile_contains_invalid_rglob_segment(pattern: &str, seps: &str) -> bool {
    let mut segment = String::new();
    for ch in pattern.chars() {
        if seps.contains(ch) {
            if segment.contains("**") && segment != "**" {
                return true;
            }
            segment.clear();
        } else {
            segment.push(ch);
        }
    }
    segment.contains("**") && segment != "**"
}

fn zipfile_translate_glob_impl(
    pattern: &str,
    seps: &str,
    py313_plus: bool,
) -> Result<String, &'static str> {
    let mut effective_pattern = pattern.to_string();
    let mut star_pattern = "[^/]*".to_string();
    let mut qmark_pattern = ".".to_string();
    if py313_plus {
        if zipfile_contains_invalid_rglob_segment(pattern, seps) {
            return Err("** must appear alone in a path segment");
        }
        effective_pattern = zipfile_star_not_empty(pattern, seps);
        let escaped = zipfile_escape_character_class(seps);
        star_pattern = format!("[^{escaped}]*");
        qmark_pattern = "[^/]".to_string();
    }

    let mut core = String::new();
    for (is_set, chunk) in zipfile_separate_pattern(effective_pattern.as_str()) {
        if is_set {
            core.push_str(chunk.as_str());
            continue;
        }
        core.push_str(
            zipfile_translate_chunk(
                chunk.as_str(),
                star_pattern.as_str(),
                qmark_pattern.as_str(),
            )
            .as_str(),
        );
    }

    let with_dirs = format!("{core}[/]?");
    if py313_plus {
        Ok(format!("(?s:{with_dirs})\\Z"))
    } else {
        Ok(with_dirs)
    }
}

fn zipfile_normalize_member_path_impl(member: &str) -> Option<String> {
    let replaced = member.replace('\\', "/");
    let mut stack: Vec<String> = Vec::new();
    for segment in replaced.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            if let Some(last) = stack.last()
                && last != ".."
            {
                stack.pop();
                continue;
            }
            stack.push("..".to_string());
            continue;
        }
        stack.push(segment.to_string());
    }
    let normalized = stack.join("/");
    if normalized.is_empty() || normalized == "." {
        return None;
    }
    if normalized == ".." || normalized.starts_with("../") {
        return None;
    }
    Some(normalized)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zipfile_parse_central_directory(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(data_ptr) = obj_from_bits(data_bits).as_ptr() else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "zipfile central directory input must be bytes-like",
            );
        };
        let Some(data) = (unsafe { bytes_like_slice(data_ptr) }) else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "zipfile central directory input must be bytes-like",
            );
        };
        let entries = match zipfile_parse_central_directory_impl(data) {
            Ok(value) => value,
            Err(message) => return raise_exception::<_>(_py, "ValueError", message),
        };

        let mut pairs: Vec<u64> = Vec::with_capacity(entries.len() * 2);
        let mut owned_bits: Vec<u64> = Vec::with_capacity(entries.len() * 2);
        for (name, fields) in entries {
            let Some(name_bits) = alloc_string_bits(_py, name.as_str()) else {
                for bits in owned_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            };

            let mut item_bits: [u64; 5] = [0; 5];
            for (idx, field) in fields.iter().enumerate() {
                let Ok(value) = i64::try_from(*field) else {
                    dec_ref_bits(_py, name_bits);
                    for bits in owned_bits {
                        dec_ref_bits(_py, bits);
                    }
                    return raise_exception::<_>(
                        _py,
                        "OverflowError",
                        "zipfile central directory value overflow",
                    );
                };
                item_bits[idx] = MoltObject::from_int(value).bits();
            }
            let tuple_ptr = alloc_tuple(_py, &item_bits);
            if tuple_ptr.is_null() {
                dec_ref_bits(_py, name_bits);
                for bits in owned_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
            pairs.push(name_bits);
            pairs.push(tuple_bits);
            owned_bits.push(name_bits);
            owned_bits.push(tuple_bits);
        }

        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        if dict_ptr.is_null() {
            for bits in owned_bits {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        let out = MoltObject::from_ptr(dict_ptr).bits();
        for bits in owned_bits {
            dec_ref_bits(_py, bits);
        }
        out
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zipfile_build_zip64_extra(
    size_bits: u64,
    comp_size_bits: u64,
    offset_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(size) = to_i64(obj_from_bits(size_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "zipfile zip64 size must be int");
        };
        if size < 0 {
            return raise_exception::<_>(_py, "ValueError", "zipfile zip64 size must be >= 0");
        }
        let Some(comp_size) = to_i64(obj_from_bits(comp_size_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "zipfile zip64 comp_size must be int");
        };
        if comp_size < 0 {
            return raise_exception::<_>(_py, "ValueError", "zipfile zip64 comp_size must be >= 0");
        }
        let offset = if obj_from_bits(offset_bits).is_none() {
            None
        } else {
            let Some(value) = to_i64(obj_from_bits(offset_bits)) else {
                return raise_exception::<_>(_py, "TypeError", "zipfile zip64 offset must be int");
            };
            if value < 0 {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "zipfile zip64 offset must be >= 0",
                );
            }
            Some(value as u64)
        };

        let out = zipfile_build_zip64_extra_impl(size as u64, comp_size as u64, offset);
        let ptr = alloc_bytes(_py, out.as_slice());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zipfile_path_implied_dirs(names_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let names = match iterable_to_string_vec(_py, names_bits) {
            Ok(items) => items,
            Err(err) => return err,
        };
        let names_set: HashSet<String> = names.iter().cloned().collect();
        let mut seen: HashSet<String> = HashSet::new();
        let mut out: Vec<String> = Vec::new();

        for name in &names {
            let ancestry = zipfile_ancestry(name.as_str());
            for parent in ancestry.into_iter().skip(1) {
                let candidate = format!("{parent}/");
                if names_set.contains(candidate.as_str()) {
                    continue;
                }
                if seen.insert(candidate.clone()) {
                    out.push(candidate);
                }
            }
        }
        alloc_string_list(_py, out.as_slice())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zipfile_path_resolve_dir(name_bits: u64, names_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "zipfile path name must be str");
        };
        let names = match iterable_to_string_vec(_py, names_bits) {
            Ok(items) => items,
            Err(err) => return err,
        };
        let names_set: HashSet<String> = names.into_iter().collect();
        let mut resolved = name.clone();
        let dirname = format!("{name}/");
        if !names_set.contains(name.as_str()) && names_set.contains(dirname.as_str()) {
            resolved = dirname;
        }
        let ptr = alloc_string(_py, resolved.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zipfile_path_is_child(path_at_bits: u64, parent_at_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(path_at) = string_obj_to_owned(obj_from_bits(path_at_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "zipfile path candidate must be str");
        };
        let Some(parent_at) = string_obj_to_owned(obj_from_bits(parent_at_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "zipfile path parent must be str");
        };

        let candidate_parent = zipfile_parent_of(path_at.as_str());
        let parent_norm = zipfile_trim_trailing_slashes(parent_at.as_str());
        if candidate_parent == parent_norm {
            MoltObject::from_bool(true).bits()
        } else {
            MoltObject::from_bool(false).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zipfile_path_translate_glob(
    pattern_bits: u64,
    seps_bits: u64,
    py313_plus_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(pattern) = string_obj_to_owned(obj_from_bits(pattern_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "zipfile glob pattern must be str");
        };
        let Some(seps) = string_obj_to_owned(obj_from_bits(seps_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "zipfile glob separators must be str");
        };
        let py313_plus = is_truthy(_py, obj_from_bits(py313_plus_bits));

        let translated =
            match zipfile_translate_glob_impl(pattern.as_str(), seps.as_str(), py313_plus) {
                Ok(value) => value,
                Err(msg) => return raise_exception::<_>(_py, "ValueError", msg),
            };
        let ptr = alloc_string(_py, translated.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zipfile_normalize_member_path(member_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(member) = string_obj_to_owned(obj_from_bits(member_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "zipfile member path must be str");
        };
        let Some(normalized) = zipfile_normalize_member_path_impl(member.as_str()) else {
            return MoltObject::none().bits();
        };
        let ptr = alloc_string(_py, normalized.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[cfg(test)]
mod zipfile_path_lowering_tests {
    use super::{
        zipfile_ancestry, zipfile_build_zip64_extra_impl, zipfile_normalize_member_path_impl,
        zipfile_parse_central_directory_impl, zipfile_translate_glob_impl,
    };

    #[test]
    fn zipfile_ancestry_preserves_posix_structure() {
        assert_eq!(
            zipfile_ancestry("//b//d///f//"),
            vec![
                String::from("//b//d///f"),
                String::from("//b//d"),
                String::from("//b"),
            ]
        );
    }

    #[test]
    fn zipfile_translate_glob_legacy_matches_shape() {
        let translated =
            zipfile_translate_glob_impl("*.txt", "/", false).expect("legacy translation");
        assert_eq!(translated, String::from("[^/]*\\.txt[/]?"));
    }

    #[test]
    fn zipfile_translate_glob_modern_matches_shape() {
        let translated =
            zipfile_translate_glob_impl("**/*", "/", true).expect("modern translation");
        assert_eq!(translated, String::from("(?s:.*/[^/][^/]*[/]?)\\Z"));
    }

    #[test]
    fn zipfile_translate_glob_rejects_invalid_rglob_segment() {
        let err = zipfile_translate_glob_impl("**foo", "/", true).expect_err("invalid segment");
        assert_eq!(err, "** must appear alone in a path segment");
    }

    #[test]
    fn zipfile_normalize_member_path_blocks_traversal() {
        assert_eq!(
            zipfile_normalize_member_path_impl("safe/../leaf.txt"),
            Some(String::from("leaf.txt"))
        );
        assert_eq!(zipfile_normalize_member_path_impl("../escape.txt"), None);
        assert_eq!(zipfile_normalize_member_path_impl("./"), None);
    }

    #[test]
    fn zipfile_parse_central_directory_roundtrip_shape() {
        let name = b"a.txt";
        let payload = b"hello";
        let name_len = name.len() as u16;
        let payload_len = payload.len() as u32;

        let mut archive: Vec<u8> = Vec::new();
        archive.extend_from_slice(b"PK\x03\x04");
        archive.extend_from_slice(20u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u32.to_le_bytes().as_slice());
        archive.extend_from_slice(payload_len.to_le_bytes().as_slice());
        archive.extend_from_slice(payload_len.to_le_bytes().as_slice());
        archive.extend_from_slice(name_len.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(name);
        archive.extend_from_slice(payload);

        let cd_offset = archive.len() as u32;
        archive.extend_from_slice(b"PK\x01\x02");
        archive.extend_from_slice(20u16.to_le_bytes().as_slice());
        archive.extend_from_slice(20u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u32.to_le_bytes().as_slice());
        archive.extend_from_slice(payload_len.to_le_bytes().as_slice());
        archive.extend_from_slice(payload_len.to_le_bytes().as_slice());
        archive.extend_from_slice(name_len.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u32.to_le_bytes().as_slice());
        archive.extend_from_slice(0u32.to_le_bytes().as_slice());
        archive.extend_from_slice(name);

        let cd_size = (46 + name.len()) as u32;
        archive.extend_from_slice(b"PK\x05\x06");
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());
        archive.extend_from_slice(1u16.to_le_bytes().as_slice());
        archive.extend_from_slice(1u16.to_le_bytes().as_slice());
        archive.extend_from_slice(cd_size.to_le_bytes().as_slice());
        archive.extend_from_slice(cd_offset.to_le_bytes().as_slice());
        archive.extend_from_slice(0u16.to_le_bytes().as_slice());

        let parsed = zipfile_parse_central_directory_impl(archive.as_slice())
            .expect("central directory parse should succeed");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0, String::from("a.txt"));
        assert_eq!(
            parsed[0].1,
            [
                0,
                payload.len() as u64,
                0,
                name.len() as u64,
                payload.len() as u64
            ]
        );
    }

    #[test]
    fn zipfile_build_zip64_extra_shape() {
        let out = zipfile_build_zip64_extra_impl(7, 11, Some(13));
        assert_eq!(
            out,
            vec![
                0x01, 0x00, 24, 0, 7, 0, 0, 0, 0, 0, 0, 0, 11, 0, 0, 0, 0, 0, 0, 0, 13, 0, 0, 0, 0,
                0, 0, 0,
            ]
        );
    }
}

#[cfg(test)]
mod tokenize_encoding_tests {
    use super::{find_encoding_cookie, skip_encoding_ws};

    #[test]
    fn skip_encoding_ws_trims_python_prefix_whitespace() {
        assert_eq!(skip_encoding_ws(b" \t\x0ccoding"), b"coding");
    }

    #[test]
    fn find_encoding_cookie_handles_standard_cookie() {
        assert_eq!(find_encoding_cookie(b"# coding: utf-8"), Some("utf-8"));
        assert_eq!(
            find_encoding_cookie(b"# -*- coding: latin-1 -*-"),
            Some("latin-1")
        );
    }

    #[test]
    fn find_encoding_cookie_rejects_non_cookie_lines() {
        assert_eq!(find_encoding_cookie(b"print('hi')"), None);
        assert_eq!(find_encoding_cookie(b"# comment only"), None);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_imghdr_what(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(data_ptr) = obj_from_bits(data_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "imghdr header must be bytes-like");
        };
        let Some(header) = (unsafe { bytes_like_slice(data_ptr) }) else {
            return raise_exception::<_>(_py, "TypeError", "imghdr header must be bytes-like");
        };
        let Some(kind) = imghdr_detect_kind(header) else {
            return MoltObject::none().bits();
        };
        let ptr = alloc_string(_py, kind.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_imghdr_test(kind_bits: u64, data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(kind) = string_obj_to_owned(obj_from_bits(kind_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "imghdr kind must be str");
        };
        let Some(data_ptr) = obj_from_bits(data_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "imghdr header must be bytes-like");
        };
        let Some(header) = (unsafe { bytes_like_slice(data_ptr) }) else {
            return raise_exception::<_>(_py, "TypeError", "imghdr header must be bytes-like");
        };
        let matches = imghdr_detect_kind(header)
            .map(|detected| detected == kind.as_str())
            .unwrap_or(false);
        MoltObject::from_bool(matches).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_runtime_ready() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(true).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_wsgiref_runtime_ready() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(true).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zoneinfo_runtime_ready() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(true).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zipapp_runtime_ready() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(true).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_runtime_ready() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(true).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_xmlrpc_runtime_ready() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(true).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_datetime_runtime_ready() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(true).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tokenize_runtime_ready() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(true).bits() })
}

/// Tokenize a UTF-8 source string into a list of (type, string, start, end, line) tuples.
/// Token types: 0=ENDMARKER, 1=NAME, 2=NUMBER, 4=NEWLINE, 54=OP, 64=COMMENT, 65=NL, 67=ENCODING
#[unsafe(no_mangle)]
pub extern "C" fn molt_tokenize_scan(source_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let source_obj = crate::obj_from_bits(source_bits);
        let Some(source) = crate::string_obj_to_owned(source_obj) else {
            return crate::raise_exception::<_>(_py, "TypeError", "source must be str");
        };

        const ENDMARKER: i64 = 0;
        const NAME: i64 = 1;
        const NUMBER: i64 = 2;
        const NEWLINE: i64 = 4;
        const OP: i64 = 54;
        const COMMENT: i64 = 64;
        const NL: i64 = 65;

        fn is_name_start(ch: u8) -> bool {
            ch == b'_' || ch.is_ascii_alphabetic()
        }
        fn is_name_char(ch: u8) -> bool {
            is_name_start(ch) || ch.is_ascii_digit()
        }

        let mut tokens: Vec<u64> = Vec::new();
        let source_bytes = source.as_bytes();
        let mut line_no: i64 = 1;

        if !source_bytes.is_empty() {
            let mut start = 0usize;
            while start < source_bytes.len() {
                let line_end = memchr(b'\n', &source_bytes[start..])
                    .map(|rel| start + rel + 1)
                    .unwrap_or(source_bytes.len());
                let line = &source[start..line_end];
                let line_bytes = line.as_bytes();
                let line_len = line_bytes.len();
                let line_bits =
                    alloc_string_bits(_py, line).unwrap_or_else(|| MoltObject::none().bits());

                // Full-line comment check
                let trimmed_start = line_bytes.iter().position(|&b| b != b' ' && b != b'\t');
                if let Some(ts) = trimmed_start
                    && line_bytes[ts] == b'#'
                {
                    let comment = line.trim();
                    let tok = make_token_tuple(
                        _py,
                        COMMENT,
                        comment,
                        (line_no, 0),
                        (line_no, comment.len() as i64),
                        line_bits,
                    );
                    tokens.push(tok);
                    if line.ends_with('\n') {
                        let tok = make_token_tuple(
                            _py,
                            NL,
                            "\n",
                            (line_no, (line_len - 1) as i64),
                            (line_no, line_len as i64),
                            line_bits,
                        );
                        tokens.push(tok);
                    }
                    if line_bits != MoltObject::none().bits() {
                        dec_ref_bits(_py, line_bits);
                    }
                    line_no += 1;
                    start = line_end;
                    continue;
                }

                let mut col: usize = 0;
                while col < line_len {
                    let ch = line_bytes[col];
                    if ch == b' ' || ch == b'\t' || ch == b'\r' || ch == b'\n' {
                        col += 1;
                        continue;
                    }
                    if ch == b'#' {
                        let comment = line[col..].trim_end_matches(['\r', '\n']);
                        let tok = make_token_tuple(
                            _py,
                            COMMENT,
                            comment,
                            (line_no, col as i64),
                            (line_no, (col + comment.len()) as i64),
                            line_bits,
                        );
                        tokens.push(tok);
                        break;
                    }
                    if is_name_start(ch) {
                        let start_col = col;
                        col += 1;
                        while col < line_len && is_name_char(line_bytes[col]) {
                            col += 1;
                        }
                        let text = &line[start_col..col];
                        let tok = make_token_tuple(
                            _py,
                            NAME,
                            text,
                            (line_no, start_col as i64),
                            (line_no, col as i64),
                            line_bits,
                        );
                        tokens.push(tok);
                        continue;
                    }
                    if ch.is_ascii_digit() {
                        let start_col = col;
                        col += 1;
                        while col < line_len && line_bytes[col].is_ascii_digit() {
                            col += 1;
                        }
                        let text = &line[start_col..col];
                        let tok = make_token_tuple(
                            _py,
                            NUMBER,
                            text,
                            (line_no, start_col as i64),
                            (line_no, col as i64),
                            line_bits,
                        );
                        tokens.push(tok);
                        continue;
                    }
                    // OP
                    let ch_str = &line[col..col + 1];
                    let tok = make_token_tuple(
                        _py,
                        OP,
                        ch_str,
                        (line_no, col as i64),
                        (line_no, (col + 1) as i64),
                        line_bits,
                    );
                    tokens.push(tok);
                    col += 1;
                }

                if line.ends_with('\n') {
                    let stripped = line.trim();
                    let has_content = !stripped.is_empty() && !stripped.starts_with('#');
                    let tok_type = if has_content { NEWLINE } else { NL };
                    let tok = make_token_tuple(
                        _py,
                        tok_type,
                        "\n",
                        (line_no, (line_len - 1) as i64),
                        (line_no, line_len as i64),
                        line_bits,
                    );
                    tokens.push(tok);
                }
                if line_bits != MoltObject::none().bits() {
                    dec_ref_bits(_py, line_bits);
                }
                line_no += 1;
                if line_end == source_bytes.len() {
                    break;
                }
                start = line_end;
            }
        }

        // ENDMARKER
        let endmarker_line_bits =
            alloc_string_bits(_py, "").unwrap_or_else(|| MoltObject::none().bits());
        let tok = make_token_tuple(
            _py,
            ENDMARKER,
            "",
            (line_no, 0),
            (line_no, 0),
            endmarker_line_bits,
        );
        tokens.push(tok);
        if endmarker_line_bits != MoltObject::none().bits() {
            dec_ref_bits(_py, endmarker_line_bits);
        }

        let list_ptr = crate::alloc_list(_py, &tokens);
        for bits in &tokens {
            crate::dec_ref_bits(_py, *bits);
        }
        if list_ptr.is_null() {
            return crate::raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(list_ptr).bits()
    })
}

fn make_token_tuple(
    _py: &crate::PyToken<'_>,
    tok_type: i64,
    string: &str,
    start: (i64, i64),
    end: (i64, i64),
    line_bits: u64,
) -> u64 {
    let type_bits = MoltObject::from_int(tok_type).bits();
    let string_ptr = crate::alloc_string(_py, string.as_bytes());
    let string_bits = if string_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(string_ptr).bits()
    };
    let start_elems = [
        MoltObject::from_int(start.0).bits(),
        MoltObject::from_int(start.1).bits(),
    ];
    let start_ptr = crate::alloc_tuple(_py, &start_elems);
    let start_bits = if start_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(start_ptr).bits()
    };
    let end_elems = [
        MoltObject::from_int(end.0).bits(),
        MoltObject::from_int(end.1).bits(),
    ];
    let end_ptr = crate::alloc_tuple(_py, &end_elems);
    let end_bits = if end_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(end_ptr).bits()
    };
    let elems = [type_bits, string_bits, start_bits, end_bits, line_bits];
    let tuple_ptr = crate::alloc_tuple(_py, &elems);
    if tuple_ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(tuple_ptr).bits()
}

fn skip_encoding_ws(bytes: &[u8]) -> &[u8] {
    let mut idx = 0;
    while idx < bytes.len() {
        match bytes[idx] {
            b' ' | b'\t' | b'\x0c' => idx += 1,
            _ => break,
        }
    }
    &bytes[idx..]
}

fn find_encoding_cookie(line: &[u8]) -> Option<&str> {
    let stripped = skip_encoding_ws(line);
    if !stripped.starts_with(b"#") {
        return None;
    }
    let coding_idx = memmem::find(stripped, b"coding")?;
    let mut rest = &stripped[coding_idx + "coding".len()..];
    rest = skip_encoding_ws(rest);
    let (sep, rest) = rest.split_first()?;
    if *sep != b':' && *sep != b'=' {
        return None;
    }
    let rest = skip_encoding_ws(rest);
    if rest.is_empty() {
        return None;
    }
    let end = rest
        .iter()
        .position(|&b| !(b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.'))
        .unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    std::str::from_utf8(&rest[..end]).ok()
}

/// Detect Python source file encoding from the first two lines.
/// `first_bits`: first line bytes, `second_bits`: second line bytes
/// Returns (encoding_name, has_bom) tuple.
#[unsafe(no_mangle)]
pub extern "C" fn molt_linecache_detect_encoding(first_bits: u64, second_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let first_obj = crate::obj_from_bits(first_bits);
        let second_obj = crate::obj_from_bits(second_bits);

        let first_bytes = if let Some(ptr) = first_obj.as_ptr() {
            unsafe { crate::bytes_like_slice(ptr) }.unwrap_or(&[])
        } else {
            &[]
        };

        let second_bytes = if let Some(ptr) = second_obj.as_ptr() {
            unsafe { crate::bytes_like_slice(ptr) }.unwrap_or(&[])
        } else {
            &[]
        };

        let bom_utf8: &[u8] = &[0xEF, 0xBB, 0xBF];
        let mut bom_found = false;
        let mut effective_first = first_bytes;
        let mut default_enc = "utf-8";

        if effective_first.starts_with(bom_utf8) {
            bom_found = true;
            effective_first = &effective_first[3..];
            default_enc = "utf-8-sig";
        }

        if effective_first.is_empty() && second_bytes.is_empty() {
            let enc_ptr = crate::alloc_string(_py, default_enc.as_bytes());
            let bom_bits = MoltObject::from_bool(bom_found).bits();
            if enc_ptr.is_null() {
                return crate::raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            let elems = [MoltObject::from_ptr(enc_ptr).bits(), bom_bits];
            let tuple_ptr = crate::alloc_tuple(_py, &elems);
            if tuple_ptr.is_null() {
                return crate::raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            return MoltObject::from_ptr(tuple_ptr).bits();
        }

        // Check first line
        if let Some(encoding) = find_encoding_cookie(effective_first) {
            let encoding = if bom_found && encoding.eq_ignore_ascii_case("utf-8") {
                "utf-8-sig"
            } else {
                encoding
            };
            let enc_ptr = crate::alloc_string(_py, encoding.as_bytes());
            let bom_bits = MoltObject::from_bool(bom_found).bits();
            if enc_ptr.is_null() {
                return crate::raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            let elems = [MoltObject::from_ptr(enc_ptr).bits(), bom_bits];
            let tuple_ptr = crate::alloc_tuple(_py, &elems);
            if tuple_ptr.is_null() {
                return crate::raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            return MoltObject::from_ptr(tuple_ptr).bits();
        }

        // Check second line
        if !second_bytes.is_empty()
            && let Some(encoding) = find_encoding_cookie(second_bytes)
        {
            let encoding = if bom_found && encoding.eq_ignore_ascii_case("utf-8") {
                "utf-8-sig"
            } else {
                encoding
            };
            let enc_ptr = crate::alloc_string(_py, encoding.as_bytes());
            let bom_bits = MoltObject::from_bool(bom_found).bits();
            if enc_ptr.is_null() {
                return crate::raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            let elems = [MoltObject::from_ptr(enc_ptr).bits(), bom_bits];
            let tuple_ptr = crate::alloc_tuple(_py, &elems);
            if tuple_ptr.is_null() {
                return crate::raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            return MoltObject::from_ptr(tuple_ptr).bits();
        }

        // Default encoding
        let enc_ptr = crate::alloc_string(_py, default_enc.as_bytes());
        let bom_bits = MoltObject::from_bool(bom_found).bits();
        if enc_ptr.is_null() {
            return crate::raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        let elems = [MoltObject::from_ptr(enc_ptr).bits(), bom_bits];
        let tuple_ptr = crate::alloc_tuple(_py, &elems);
        if tuple_ptr.is_null() {
            return crate::raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tomllib_runtime_ready() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(true).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_trace_runtime_ready() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(true).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_unicodedata_runtime_ready() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(true).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_subprocess_runtime_ready() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(true).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_symtable_runtime_ready() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(true).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_import_smoke_runtime_ready() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(true).bits() })
}
