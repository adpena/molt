use super::*;

#[cfg(feature = "native-backend")]
#[allow(dead_code)]
pub(in crate::native_backend::simple_backend) fn parse_inst_id(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i + 4 <= bytes.len() {
        if bytes[i..].starts_with(b"inst") {
            let mut j = i + 4;
            let mut value: usize = 0;
            let mut found = false;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                found = true;
                value = value * 10 + (bytes[j] - b'0') as usize;
                j += 1;
            }
            if found {
                return Some(value);
            }
        }
        i += 1;
    }
    None
}

#[cfg(feature = "native-backend")]
pub(crate) struct TraceOpsConfig {
    pub(crate) stride: usize,
}

#[cfg(feature = "native-backend")]
pub(crate) fn should_trace_ops(func_name: &str) -> Option<TraceOpsConfig> {
    static RAW: OnceLock<Option<String>> = OnceLock::new();
    let raw = RAW
        .get_or_init(|| {
            std::env::var("MOLT_TRACE_OP_PROGRESS")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .as_ref()?;
    let (filter_part, stride_part) = match raw.split_once(':') {
        Some((left, right)) => (left.trim(), Some(right.trim())),
        None => (raw.as_str(), None),
    };
    let stride = stride_part
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(5_000);
    let matches = filter_part == "1"
        || filter_part.eq_ignore_ascii_case("all")
        || func_name == filter_part
        || func_name.contains(filter_part);
    if matches {
        Some(TraceOpsConfig { stride })
    } else {
        None
    }
}
