use super::*;

pub(super) fn socketserver_runtime() -> &'static Mutex<MoltSocketServerRuntime> {
    SOCKETSERVER_RUNTIME.get_or_init(|| {
        Mutex::new(MoltSocketServerRuntime {
            next_request_id: 1,
            pending_by_server: HashMap::new(),
            pending_requests: HashMap::new(),
            request_server: HashMap::new(),
            closed_servers: HashSet::new(),
        })
    })
}

pub(super) fn socketserver_extract_bytes(
    _py: &molt_runtime_core::CoreGilToken,
    bits: u64,
    label: &str,
) -> Result<Vec<u8>, u64> {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            &format!("{label} must be bytes-like"),
        ));
    };
    let Some(bytes) = (unsafe { bytes_like_slice(MoltObject::from_ptr(ptr).bits()) }) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            &format!("{label} must be bytes-like"),
        ));
    };
    Ok(bytes.to_vec())
}

pub(super) fn socketserver_extract_request_id(
    _py: &molt_runtime_core::CoreGilToken,
    bits: u64,
) -> Result<u64, u64> {
    let Some(value) = to_i64(obj_from_bits(bits)) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "request id must be int",
        ));
    };
    if value <= 0 {
        return Err(raise_exception::<u64>(
            _py,
            "ValueError",
            "request id must be positive",
        ));
    }
    Ok(value as u64)
}

pub(super) fn socketserver_extract_handle_request_tuple(
    _py: &molt_runtime_core::CoreGilToken,
    bits: u64,
) -> Result<(u64, u64, i64), u64> {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "get_request() must return a 3-item tuple",
        ));
    };
    let ty = unsafe { object_type_id(ptr) };
    if ty != crate::bridge::type_id_tuple() && ty != crate::bridge::type_id_list() {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "get_request() must return a 3-item tuple",
        ));
    }
    let fields = unsafe { seq_vec_ref(ptr) };
    if fields.len() != 3 {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "get_request() must return a 3-item tuple",
        ));
    }
    let Some(request_id) = to_i64(obj_from_bits(fields[2])) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "request id must be int",
        ));
    };
    Ok((fields[0], fields[1], request_id))
}

pub(super) fn socketserver_call_service_actions(
    _py: &molt_runtime_core::CoreGilToken,
    server_bits: u64,
) -> Result<(), u64> {
    let Some(method_bits) = attr_optional(_py, server_bits, b"service_actions")? else {
        return Ok(());
    };
    if !molt_is_callable(method_bits) {
        dec_ref_bits(_py, method_bits);
        return Ok(());
    }
    let _ = unsafe { call_callable0(_py, method_bits) };
    dec_ref_bits(_py, method_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(())
}

pub(super) const HTTP_SERVER_DEFAULT_REQUEST_VERSION: &str = "HTTP/0.9";
pub(super) const HTTP_SERVER_HTTP11: &str = "HTTP/1.1";

pub(super) fn http_server_reason_phrase(code: i64) -> &'static str {
    match code {
        100 => "Continue",
        101 => "Switching Protocols",
        102 => "Processing",
        103 => "Early Hints",
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        203 => "Non-Authoritative Information",
        204 => "No Content",
        205 => "Reset Content",
        206 => "Partial Content",
        207 => "Multi-Status",
        208 => "Already Reported",
        226 => "IM Used",
        300 => "Multiple Choices",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        305 => "Use Proxy",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        402 => "Payment Required",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        406 => "Not Acceptable",
        407 => "Proxy Authentication Required",
        408 => "Request Timeout",
        409 => "Conflict",
        410 => "Gone",
        411 => "Length Required",
        412 => "Precondition Failed",
        413 => "Request Entity Too Large",
        414 => "Request-URI Too Long",
        415 => "Unsupported Media Type",
        416 => "Requested Range Not Satisfiable",
        417 => "Expectation Failed",
        418 => "I'm a Teapot",
        421 => "Misdirected Request",
        422 => "Unprocessable Entity",
        423 => "Locked",
        424 => "Failed Dependency",
        425 => "Too Early",
        426 => "Upgrade Required",
        428 => "Precondition Required",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        451 => "Unavailable For Legal Reasons",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        505 => "HTTP Version Not Supported",
        506 => "Variant Also Negotiates",
        507 => "Insufficient Storage",
        508 => "Loop Detected",
        510 => "Not Extended",
        511 => "Network Authentication Required",
        _ => "",
    }
}

pub(super) fn http_status_constants() -> &'static [(&'static str, i64)] {
    &[
        ("CONTINUE", 100),
        ("SWITCHING_PROTOCOLS", 101),
        ("PROCESSING", 102),
        ("EARLY_HINTS", 103),
        ("OK", 200),
        ("CREATED", 201),
        ("ACCEPTED", 202),
        ("NON_AUTHORITATIVE_INFORMATION", 203),
        ("NO_CONTENT", 204),
        ("RESET_CONTENT", 205),
        ("PARTIAL_CONTENT", 206),
        ("MULTI_STATUS", 207),
        ("ALREADY_REPORTED", 208),
        ("IM_USED", 226),
        ("MULTIPLE_CHOICES", 300),
        ("MOVED_PERMANENTLY", 301),
        ("FOUND", 302),
        ("SEE_OTHER", 303),
        ("NOT_MODIFIED", 304),
        ("USE_PROXY", 305),
        ("TEMPORARY_REDIRECT", 307),
        ("PERMANENT_REDIRECT", 308),
        ("BAD_REQUEST", 400),
        ("UNAUTHORIZED", 401),
        ("PAYMENT_REQUIRED", 402),
        ("FORBIDDEN", 403),
        ("NOT_FOUND", 404),
        ("METHOD_NOT_ALLOWED", 405),
        ("NOT_ACCEPTABLE", 406),
        ("PROXY_AUTHENTICATION_REQUIRED", 407),
        ("REQUEST_TIMEOUT", 408),
        ("CONFLICT", 409),
        ("GONE", 410),
        ("LENGTH_REQUIRED", 411),
        ("PRECONDITION_FAILED", 412),
        ("REQUEST_ENTITY_TOO_LARGE", 413),
        ("REQUEST_URI_TOO_LONG", 414),
        ("UNSUPPORTED_MEDIA_TYPE", 415),
        ("REQUESTED_RANGE_NOT_SATISFIABLE", 416),
        ("EXPECTATION_FAILED", 417),
        ("IM_A_TEAPOT", 418),
        ("MISDIRECTED_REQUEST", 421),
        ("UNPROCESSABLE_ENTITY", 422),
        ("LOCKED", 423),
        ("FAILED_DEPENDENCY", 424),
        ("TOO_EARLY", 425),
        ("UPGRADE_REQUIRED", 426),
        ("PRECONDITION_REQUIRED", 428),
        ("TOO_MANY_REQUESTS", 429),
        ("REQUEST_HEADER_FIELDS_TOO_LARGE", 431),
        ("UNAVAILABLE_FOR_LEGAL_REASONS", 451),
        ("INTERNAL_SERVER_ERROR", 500),
        ("NOT_IMPLEMENTED", 501),
        ("BAD_GATEWAY", 502),
        ("SERVICE_UNAVAILABLE", 503),
        ("GATEWAY_TIMEOUT", 504),
        ("HTTP_VERSION_NOT_SUPPORTED", 505),
        ("VARIANT_ALSO_NEGOTIATES", 506),
        ("INSUFFICIENT_STORAGE", 507),
        ("LOOP_DETECTED", 508),
        ("NOT_EXTENDED", 510),
        ("NETWORK_AUTHENTICATION_REQUIRED", 511),
        // CPython 3.12+ compatibility aliases.
        ("CONTENT_TOO_LARGE", 413),
        ("URI_TOO_LONG", 414),
        ("RANGE_NOT_SATISFIABLE", 416),
        ("UNPROCESSABLE_CONTENT", 422),
    ]
}

pub(super) fn http_server_error_explain(code: i64) -> &'static str {
    match code {
        400 => "Bad request syntax or unsupported method",
        404 => "Nothing matches the given URI",
        500 => "Server got itself in trouble",
        501 => "Server does not support this operation",
        _ => "",
    }
}

pub(super) fn http_server_html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub(super) fn http_server_repr_single_quoted(text: &str) -> String {
    let escaped = text.replace('\\', "\\\\").replace('\'', "\\'");
    format!("'{escaped}'")
}

pub(super) fn http_server_set_attr_string(
    _py: &molt_runtime_core::CoreGilToken,
    obj_bits: u64,
    name: &[u8],
    value: &str,
) -> Result<(), u64> {
    let Some(bits) = alloc_string_bits(_py, value) else {
        return Err(MoltObject::none().bits());
    };
    if !urllib_request_set_attr(_py, obj_bits, name, bits) {
        dec_ref_bits(_py, bits);
        return Err(MoltObject::none().bits());
    }
    dec_ref_bits(_py, bits);
    Ok(())
}

pub(super) fn http_server_get_required_attr_bits(
    _py: &molt_runtime_core::CoreGilToken,
    obj_bits: u64,
    name: &[u8],
    label: &str,
) -> Result<u64, u64> {
    let Some(bits) = attr_optional(_py, obj_bits, name)? else {
        return Err(raise_exception::<u64>(_py, "RuntimeError", label));
    };
    if obj_from_bits(bits).is_none() {
        dec_ref_bits(_py, bits);
        return Err(raise_exception::<u64>(_py, "RuntimeError", label));
    }
    Ok(bits)
}

pub(super) fn http_server_get_optional_attr_string(
    _py: &molt_runtime_core::CoreGilToken,
    obj_bits: u64,
    name: &[u8],
) -> Result<Option<String>, u64> {
    let Some(bits) = attr_optional(_py, obj_bits, name)? else {
        return Ok(None);
    };
    if obj_from_bits(bits).is_none() {
        dec_ref_bits(_py, bits);
        return Ok(None);
    }
    let out = crate::bridge::format_obj_str(_py, obj_from_bits(bits));
    dec_ref_bits(_py, bits);
    Ok(Some(out))
}

pub(super) fn http_server_write_bytes(
    _py: &molt_runtime_core::CoreGilToken,
    handler_bits: u64,
    payload: &[u8],
) -> Result<(), u64> {
    let wfile_bits = http_server_get_required_attr_bits(
        _py,
        handler_bits,
        b"wfile",
        "http handler is missing wfile",
    )?;
    let Some(write_name_bits) = attr_name_bits_from_bytes(_py, b"write") else {
        dec_ref_bits(_py, wfile_bits);
        return Err(MoltObject::none().bits());
    };
    let missing = missing_bits(_py);
    let write_bits = molt_getattr_builtin(wfile_bits, write_name_bits, missing);
    dec_ref_bits(_py, write_name_bits);
    dec_ref_bits(_py, wfile_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if write_bits == missing || !molt_is_callable(write_bits) {
        if write_bits != missing {
            dec_ref_bits(_py, write_bits);
        }
        return Err(raise_exception::<u64>(
            _py,
            "RuntimeError",
            "http handler wfile.write is unavailable",
        ));
    }
    let data_ptr = crate::bridge::alloc_bytes(_py, payload);
    if data_ptr.is_null() {
        dec_ref_bits(_py, write_bits);
        return Err(MoltObject::none().bits());
    }
    let data_bits = MoltObject::from_ptr(data_ptr).bits();
    let _ = unsafe { call_callable1(_py, write_bits, data_bits) };
    dec_ref_bits(_py, data_bits);
    dec_ref_bits(_py, write_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(())
}

pub(super) fn http_server_flush(
    _py: &molt_runtime_core::CoreGilToken,
    handler_bits: u64,
) -> Result<(), u64> {
    let Some(wfile_bits) = attr_optional(_py, handler_bits, b"wfile")? else {
        return Ok(());
    };
    if obj_from_bits(wfile_bits).is_none() {
        dec_ref_bits(_py, wfile_bits);
        return Ok(());
    }
    let Some(flush_name_bits) = attr_name_bits_from_bytes(_py, b"flush") else {
        dec_ref_bits(_py, wfile_bits);
        return Err(MoltObject::none().bits());
    };
    let missing = missing_bits(_py);
    let flush_bits = molt_getattr_builtin(wfile_bits, flush_name_bits, missing);
    dec_ref_bits(_py, flush_name_bits);
    dec_ref_bits(_py, wfile_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if flush_bits == missing || !molt_is_callable(flush_bits) {
        if flush_bits != missing {
            dec_ref_bits(_py, flush_bits);
        }
        return Ok(());
    }
    let _ = unsafe { call_callable0(_py, flush_bits) };
    dec_ref_bits(_py, flush_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(())
}

pub(super) fn http_server_readline(
    _py: &molt_runtime_core::CoreGilToken,
    handler_bits: u64,
    limit: i64,
) -> Result<Vec<u8>, u64> {
    let rfile_bits = http_server_get_required_attr_bits(
        _py,
        handler_bits,
        b"rfile",
        "http handler is missing rfile",
    )?;
    let Some(readline_name_bits) = attr_name_bits_from_bytes(_py, b"readline") else {
        dec_ref_bits(_py, rfile_bits);
        return Err(MoltObject::none().bits());
    };
    let missing = missing_bits(_py);
    let readline_bits = molt_getattr_builtin(rfile_bits, readline_name_bits, missing);
    dec_ref_bits(_py, readline_name_bits);
    dec_ref_bits(_py, rfile_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if readline_bits == missing || !molt_is_callable(readline_bits) {
        if readline_bits != missing {
            dec_ref_bits(_py, readline_bits);
        }
        return Err(raise_exception::<u64>(
            _py,
            "RuntimeError",
            "http handler rfile.readline is unavailable",
        ));
    }
    let line_bits =
        unsafe { call_callable1(_py, readline_bits, MoltObject::from_int(limit).bits()) };
    dec_ref_bits(_py, readline_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let out = socketserver_extract_bytes(_py, line_bits, "request line");
    dec_ref_bits(_py, line_bits);
    out
}

pub(super) fn http_server_version_string_impl(server_version: &str, sys_version: &str) -> String {
    if sys_version.is_empty() {
        server_version.to_string()
    } else {
        format!("{server_version} {sys_version}")
    }
}

pub(super) fn http_server_format_gmt_timestamp(timestamp: i64) -> String {
    pub(super) const WEEKDAY: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    pub(super) const MONTH: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    #[cfg(target_arch = "wasm32")]
    {
        // Pure-arithmetic UTC calendar decomposition (no libc dependency).
        let secs = if timestamp < 0 { 0i64 } else { timestamp };
        let day_secs = secs % 86400;
        let hour = (day_secs / 3600) as u32;
        let minute = ((day_secs % 3600) / 60) as u32;
        let second = (day_secs % 60) as u32;
        // Days since epoch → civil date (Howard Hinnant algorithm)
        let z = secs / 86400 + 719468;
        let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
        let doe = (z - era * 146097) as u64;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let y = yoe as i64 + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = if m <= 2 { y + 1 } else { y };
        // Weekday: epoch (1970-01-01) was Thursday (4)
        let total_days = secs / 86400;
        let wday = ((total_days % 7 + 4) % 7) as usize;
        let month_idx = if m >= 1 && m <= 12 {
            (m - 1) as usize
        } else {
            0
        };
        format!(
            "{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
            WEEKDAY[wday.min(6)],
            d,
            MONTH[month_idx.min(11)],
            year,
            hour,
            minute,
            second,
        )
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let secs: libc::time_t = if timestamp < 0 {
            0
        } else {
            timestamp as libc::time_t
        };
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        let ok = unsafe {
            #[cfg(windows)]
            {
                libc::gmtime_s(&mut tm, &secs) == 0
            }
            #[cfg(not(windows))]
            {
                !libc::gmtime_r(&secs, &mut tm).is_null()
            }
        };
        if !ok {
            return "Thu, 01 Jan 1970 00:00:00 GMT".to_string();
        }
        let wday = usize::try_from(tm.tm_wday).unwrap_or(0).min(6);
        let month = usize::try_from(tm.tm_mon).unwrap_or(0).min(11);
        format!(
            "{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
            WEEKDAY[wday],
            tm.tm_mday,
            MONTH[month],
            tm.tm_year + 1900,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec
        )
    }
}

pub(super) fn http_server_date_time_string_from_bits(
    _py: &molt_runtime_core::CoreGilToken,
    timestamp_bits: u64,
) -> Result<String, u64> {
    let ts = if obj_from_bits(timestamp_bits).is_none() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        i64::try_from(now.as_secs()).unwrap_or(i64::MAX)
    } else {
        let Some(value) = to_f64(obj_from_bits(timestamp_bits)) else {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "timestamp must be float or None",
            ));
        };
        if !value.is_finite() || value < 0.0 {
            0
        } else {
            value as i64
        }
    };
    Ok(http_server_format_gmt_timestamp(ts))
}

pub(super) fn http_server_send_response_only_impl(
    _py: &molt_runtime_core::CoreGilToken,
    handler_bits: u64,
    code: i64,
    message: Option<String>,
) -> Result<(), u64> {
    let request_version =
        http_server_get_optional_attr_string(_py, handler_bits, b"request_version")?
            .unwrap_or_else(|| HTTP_SERVER_DEFAULT_REQUEST_VERSION.to_string());
    if request_version == HTTP_SERVER_DEFAULT_REQUEST_VERSION {
        return Ok(());
    }
    let reason = message.unwrap_or_else(|| http_server_reason_phrase(code).to_string());
    let status = format!("HTTP/1.1 {} {}\r\n", code, reason);
    http_server_write_bytes(_py, handler_bits, status.as_bytes())
}

pub(super) fn http_server_send_response_impl(
    _py: &molt_runtime_core::CoreGilToken,
    handler_bits: u64,
    code: i64,
    message: Option<String>,
) -> Result<(), u64> {
    http_server_send_response_only_impl(_py, handler_bits, code, message)?;
    let request_version =
        http_server_get_optional_attr_string(_py, handler_bits, b"request_version")?
            .unwrap_or_else(|| HTTP_SERVER_DEFAULT_REQUEST_VERSION.to_string());
    if request_version == HTTP_SERVER_DEFAULT_REQUEST_VERSION {
        return Ok(());
    }
    let server_version =
        http_server_get_optional_attr_string(_py, handler_bits, b"server_version")?
            .unwrap_or_else(|| "BaseHTTP/0.6".to_string());
    let sys_version = http_server_get_optional_attr_string(_py, handler_bits, b"sys_version")?
        .unwrap_or_default();
    let version = http_server_version_string_impl(&server_version, &sys_version);
    let date = http_server_format_gmt_timestamp(
        i64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        )
        .unwrap_or(i64::MAX),
    );
    http_server_send_header_impl(_py, handler_bits, "Server", &version)?;
    http_server_send_header_impl(_py, handler_bits, "Date", &date)?;
    Ok(())
}

pub(super) fn http_server_send_header_impl(
    _py: &molt_runtime_core::CoreGilToken,
    handler_bits: u64,
    keyword: &str,
    value: &str,
) -> Result<(), u64> {
    let line = format!("{keyword}: {value}\r\n");
    http_server_write_bytes(_py, handler_bits, line.as_bytes())
}

pub(super) fn http_server_end_headers_impl(
    _py: &molt_runtime_core::CoreGilToken,
    handler_bits: u64,
) -> Result<(), u64> {
    http_server_write_bytes(_py, handler_bits, b"\r\n")?;
    http_server_flush(_py, handler_bits)
}

pub(super) fn http_server_send_error_impl(
    _py: &molt_runtime_core::CoreGilToken,
    handler_bits: u64,
    code: i64,
    message: Option<String>,
) -> Result<(), u64> {
    let short = http_server_reason_phrase(code).to_string();
    let text = message.unwrap_or_else(|| short.clone());
    let explain = http_server_error_explain(code).to_string();
    let escaped_message = http_server_html_escape(&text);
    let escaped_explain = http_server_html_escape(&explain);
    let body = format!(
        "<!DOCTYPE HTML>\n<html lang=\"en\">\n    <head>\n        <meta charset=\"utf-8\">\n        <title>Error response</title>\n    </head>\n    <body>\n        <h1>Error response</h1>\n        <p>Error code: {code}</p>\n        <p>Message: {escaped_message}.</p>\n        <p>Error code explanation: {code} - {escaped_explain}.</p>\n    </body>\n</html>\n"
    );

    http_server_send_response_impl(_py, handler_bits, code, Some(text))?;
    let request_version =
        http_server_get_optional_attr_string(_py, handler_bits, b"request_version")?
            .unwrap_or_else(|| HTTP_SERVER_DEFAULT_REQUEST_VERSION.to_string());
    if request_version != HTTP_SERVER_DEFAULT_REQUEST_VERSION {
        http_server_send_header_impl(_py, handler_bits, "Content-Type", "text/html;charset=utf-8")?;
        http_server_send_header_impl(_py, handler_bits, "Content-Length", &body.len().to_string())?;
        http_server_end_headers_impl(_py, handler_bits)?;
    }
    http_server_write_bytes(_py, handler_bits, body.as_bytes())?;
    let _ = urllib_request_set_attr(
        _py,
        handler_bits,
        b"close_connection",
        MoltObject::from_bool(true).bits(),
    );
    Ok(())
}

pub(super) fn http_server_handle_one_request_impl(
    _py: &molt_runtime_core::CoreGilToken,
    handler_bits: u64,
) -> Result<bool, u64> {
    let state = http_server_read_request_impl(_py, handler_bits)?;
    if state == 0 {
        return Ok(false);
    }
    if state == 2 {
        let close = urllib_attr_truthy(_py, handler_bits, b"close_connection")?;
        return Ok(!close);
    }
    if state != 1 {
        return Err(raise_exception::<u64>(
            _py,
            "RuntimeError",
            "http server request parser returned invalid state",
        ));
    }

    let close_connection = http_server_compute_close_connection_impl(_py, handler_bits)?;
    if !urllib_request_set_attr(
        _py,
        handler_bits,
        b"close_connection",
        MoltObject::from_bool(close_connection).bits(),
    ) {
        return Err(MoltObject::none().bits());
    }

    let Some(prepare_headers_name_bits) = attr_name_bits_from_bytes(_py, b"_molt_prepare_headers")
    else {
        return Err(MoltObject::none().bits());
    };
    let missing = missing_bits(_py);
    let prepare_headers_bits =
        molt_getattr_builtin(handler_bits, prepare_headers_name_bits, missing);
    dec_ref_bits(_py, prepare_headers_name_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if prepare_headers_bits != missing && molt_is_callable(prepare_headers_bits) {
        let _ = unsafe { call_callable0(_py, prepare_headers_bits) };
        dec_ref_bits(_py, prepare_headers_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
    } else if prepare_headers_bits != missing {
        dec_ref_bits(_py, prepare_headers_bits);
    }

    let command =
        http_server_get_optional_attr_string(_py, handler_bits, b"command")?.unwrap_or_default();
    let method_name = format!("do_{command}");
    let Some(method_name_bits) = alloc_string_bits(_py, &method_name) else {
        return Err(MoltObject::none().bits());
    };
    let method_bits = molt_getattr_builtin(handler_bits, method_name_bits, missing);
    dec_ref_bits(_py, method_name_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if method_bits == missing {
        let message = format!(
            "Unsupported method ({})",
            http_server_repr_single_quoted(&command)
        );
        http_server_send_error_impl(_py, handler_bits, 501, Some(message))?;
    } else {
        let _ = unsafe { call_callable0(_py, method_bits) };
        dec_ref_bits(_py, method_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
    }
    let close = urllib_attr_truthy(_py, handler_bits, b"close_connection")?;
    Ok(!close)
}
