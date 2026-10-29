use super::*;
use crate::BuiltinExc;

pub(in crate::builtins::io) unsafe fn text_backend_read(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
    backend: &mut MoltFileBackend,
    size: Option<usize>,
) -> Result<String, u64> {
    let MoltFileBackend::Text(text_backend) = backend else {
        return Err(raise_exception::<_>(
            _py,
            BuiltinExc::UnsupportedOperation,
            "text backend missing",
        ));
    };
    let newline = handle.newline.as_deref();
    let limit = size.unwrap_or(usize::MAX);
    let mut out = String::new();
    let mut count = 0usize;
    let mut idx = text_backend.pos;
    let start = idx;
    let len = text_backend.data.len();
    while idx < len && count < limit {
        let ch = text_backend.data[idx];
        if newline.is_none() && ch == '\r' {
            if idx + 1 < len && text_backend.data[idx + 1] == '\n' {
                idx += 2;
            } else {
                idx += 1;
            }
            out.push('\n');
            count += 1;
            continue;
        }
        out.push(ch);
        idx += 1;
        count += 1;
    }
    text_backend.pos = idx;
    update_newlines_from_chars(handle, &text_backend.data[start..idx]);
    Ok(out)
}

pub(in crate::builtins::io) unsafe fn text_backend_readline(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
    backend: &mut MoltFileBackend,
    limit: Option<usize>,
) -> Result<String, u64> {
    let MoltFileBackend::Text(text_backend) = backend else {
        return Err(raise_exception::<_>(
            _py,
            BuiltinExc::UnsupportedOperation,
            "text backend missing",
        ));
    };
    let newline = handle.newline.as_deref();
    let max_len = limit.unwrap_or(usize::MAX);
    let mut out = String::new();
    let mut count = 0usize;
    let mut idx = text_backend.pos;
    let start = idx;
    let len = text_backend.data.len();
    while idx < len && count < max_len {
        let ch = text_backend.data[idx];
        match newline {
            None => {
                if ch == '\n' {
                    out.push('\n');
                    idx += 1;
                    break;
                }
                if ch == '\r' {
                    if idx + 1 < len && text_backend.data[idx + 1] == '\n' {
                        idx += 2;
                    } else {
                        idx += 1;
                    }
                    out.push('\n');
                    break;
                }
                out.push(ch);
                count += 1;
                idx += 1;
            }
            Some("") => {
                if ch == '\r' {
                    if count >= max_len {
                        break;
                    }
                    out.push('\r');
                    count += 1;
                    if count >= max_len {
                        idx += 1;
                        break;
                    }
                    if idx + 1 < len && text_backend.data[idx + 1] == '\n' {
                        out.push('\n');
                        idx += 2;
                    } else {
                        idx += 1;
                    }
                    break;
                }
                out.push(ch);
                count += 1;
                idx += 1;
                if ch == '\n' {
                    break;
                }
            }
            Some("\n") => {
                out.push(ch);
                count += 1;
                idx += 1;
                if ch == '\n' {
                    break;
                }
            }
            Some("\r") => {
                out.push(ch);
                count += 1;
                idx += 1;
                if ch == '\r' {
                    break;
                }
            }
            Some("\r\n") => {
                if ch == '\r' && idx + 1 < len && text_backend.data[idx + 1] == '\n' {
                    if count >= max_len {
                        break;
                    }
                    out.push('\r');
                    count += 1;
                    if count >= max_len {
                        idx += 1;
                        break;
                    }
                    out.push('\n');
                    idx += 2;
                    break;
                }
                out.push(ch);
                count += 1;
                idx += 1;
            }
            Some(_) => {
                out.push(ch);
                count += 1;
                idx += 1;
                if ch == '\n' {
                    break;
                }
            }
        }
    }
    text_backend.pos = idx;
    update_newlines_from_chars(handle, &text_backend.data[start..idx]);
    Ok(out)
}

pub(in crate::builtins::io) unsafe fn text_backend_write(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
    backend: &mut MoltFileBackend,
    text: &str,
) -> Result<usize, u64> {
    let MoltFileBackend::Text(text_backend) = backend else {
        return Err(raise_exception::<_>(
            _py,
            BuiltinExc::UnsupportedOperation,
            "text backend missing",
        ));
    };
    let translated = translate_write_newlines_str(text, handle.newline.as_deref());
    let chars: Vec<char> = translated.chars().collect();
    let pos = text_backend.pos;
    if pos > text_backend.data.len() {
        text_backend.data.resize(pos, '\0');
    }
    let end = pos.saturating_add(chars.len());
    if end > text_backend.data.len() {
        text_backend.data.resize(end, '\0');
    }
    text_backend.data[pos..end].copy_from_slice(&chars);
    text_backend.pos = end;
    Ok(chars.len())
}

pub(in crate::builtins::io) unsafe fn text_backend_getvalue(
    _py: &PyToken<'_>,
    backend: &mut MoltFileBackend,
) -> Result<String, u64> {
    let MoltFileBackend::Text(text_backend) = backend else {
        return Err(raise_exception::<_>(
            _py,
            BuiltinExc::UnsupportedOperation,
            "text backend missing",
        ));
    };
    Ok(text_backend.data.iter().collect())
}

pub(in crate::builtins::io) unsafe fn text_backend_seek(
    _py: &PyToken<'_>,
    backend: &mut MoltFileBackend,
    offset: i64,
    whence: i64,
) -> Result<i64, u64> {
    let MoltFileBackend::Text(text_backend) = backend else {
        return Err(raise_exception::<_>(
            _py,
            BuiltinExc::UnsupportedOperation,
            "text backend missing",
        ));
    };
    let offset = isize::try_from(offset).map_err(|_| {
        raise_exception::<u64>(
            _py,
            BuiltinExc::OverflowError,
            "Python int too large to convert to C ssize_t",
        )
    })?;
    if !matches!(whence, 0..=2) {
        return Err(raise_exception::<_>(
            _py,
            BuiltinExc::ValueError,
            "invalid whence",
        ));
    }
    if whence == 0 && offset < 0 {
        return Err(raise_exception::<_>(
            _py,
            BuiltinExc::ValueError,
            "negative seek position",
        ));
    }
    if whence != 0 && offset != 0 {
        return Err(raise_exception::<_>(
            _py,
            BuiltinExc::OSError,
            "Can't do nonzero cur-relative seeks",
        ));
    }
    let pos = match whence {
        0 => offset as usize,
        1 => text_backend.pos,
        2 => text_backend.data.len(),
        _ => unreachable!(),
    };
    text_backend.pos = pos;
    Ok(pos as i64)
}

pub(in crate::builtins::io) unsafe fn text_backend_truncate(
    _py: &PyToken<'_>,
    backend: &mut MoltFileBackend,
    size: usize,
) -> Result<(), u64> {
    let MoltFileBackend::Text(text_backend) = backend else {
        return Err(raise_exception::<_>(
            _py,
            BuiltinExc::UnsupportedOperation,
            "text backend missing",
        ));
    };
    if size < text_backend.data.len() {
        text_backend.data.truncate(size);
    } else if size > text_backend.data.len() {
        text_backend.data.resize(size, '\0');
    }
    if text_backend.pos > text_backend.data.len() {
        text_backend.pos = text_backend.data.len();
    }
    Ok(())
}

pub(in crate::builtins::io) unsafe fn text_backend_tell(
    _py: &PyToken<'_>,
    backend: &mut MoltFileBackend,
) -> Result<i64, u64> {
    let MoltFileBackend::Text(text_backend) = backend else {
        return Err(raise_exception::<_>(
            _py,
            BuiltinExc::UnsupportedOperation,
            "text backend missing",
        ));
    };
    Ok(text_backend.pos as i64)
}
