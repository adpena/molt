//! Intrinsics for `xml.sax` stdlib module.
//!
//! Coverage: SAX-style event-driven XML parsing (parseString, parse, ContentHandler),
//! saxutils (escape, unescape, quoteattr).

use crate::bridge::{
    alloc_list, alloc_string, alloc_tuple,
    int_bits_from_i64, raise_exception, string_obj_to_owned,
    to_i64,
};
use molt_obj_model::MoltObject;
use molt_runtime_core::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};

static NEXT_HANDLE_ID: AtomicI64 = AtomicI64::new(1);

fn next_handle_id() -> i64 {
    NEXT_HANDLE_ID.fetch_add(1, Ordering::Relaxed)
}

fn mk_str(py: &CoreGilToken, s: &str) -> u64 {
    let ptr = alloc_string(py, s.as_bytes());
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

fn mk_list(py: &CoreGilToken, elems: &[u64]) -> u64 {
    let ptr = alloc_list(py, elems);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

fn mk_tuple(py: &CoreGilToken, elems: &[u64]) -> u64 {
    let ptr = alloc_tuple(py, elems);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

/// A single SAX event.
#[derive(Debug, Clone)]
enum SaxEvent {
    StartDocument,
    EndDocument,
    StartElement {
        name: String,
        attrs: Vec<(String, String)>,
    },
    EndElement {
        name: String,
    },
    Characters {
        content: String,
    },
    ProcessingInstruction {
        target: String,
        data: String,
    },
}

fn unescape_xml(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&apos;", "'")
        .replace("&quot;", "\"")
}

fn skip_ws(data: &[u8], mut pos: usize) -> usize {
    while pos < data.len() && matches!(data[pos], b' ' | b'\t' | b'\n' | b'\r') {
        pos += 1;
    }
    pos
}

fn parse_name(data: &[u8], mut pos: usize) -> Result<(String, usize), String> {
    let start = pos;
    while pos < data.len()
        && !matches!(data[pos], b' ' | b'\t' | b'\n' | b'\r' | b'>' | b'/' | b'=')
    {
        pos += 1;
    }
    if pos == start {
        return Err("expected element name".to_string());
    }
    Ok((String::from_utf8_lossy(&data[start..pos]).into_owned(), pos))
}

fn parse_attr_value(data: &[u8], mut pos: usize) -> Result<(String, usize), String> {
    if pos >= data.len() {
        return Err("unexpected end of attribute".to_string());
    }
    let quote = data[pos];
    if quote != b'"' && quote != b'\'' {
        return Err("expected quote".to_string());
    }
    pos += 1;
    let start = pos;
    while pos < data.len() && data[pos] != quote {
        pos += 1;
    }
    if pos >= data.len() {
        return Err("unterminated attribute value".to_string());
    }
    let val = unescape_xml(&String::from_utf8_lossy(&data[start..pos]));
    pos += 1;
    Ok((val, pos))
}

/// Parse XML into a list of SAX events.
fn sax_parse(xml: &str) -> Result<Vec<SaxEvent>, String> {
    let mut events = Vec::new();
    events.push(SaxEvent::StartDocument);
    let bytes = xml.as_bytes();
    sax_parse_content(bytes, 0, &mut events)?;
    events.push(SaxEvent::EndDocument);
    Ok(events)
}

fn sax_parse_content(data: &[u8], mut pos: usize, events: &mut Vec<SaxEvent>) -> Result<usize, String> {
    while pos < data.len() {
        if data[pos] == b'<' {
            if pos + 1 < data.len() && data[pos + 1] == b'/' {
                return Ok(pos);
            }
            if pos + 1 < data.len() && data[pos + 1] == b'?' {
                pos = sax_skip_pi(data, pos, events)?;
                continue;
            }
            if pos + 3 < data.len() && data[pos + 1] == b'!' && data[pos + 2] == b'-' && data[pos + 3] == b'-' {
                pos = sax_skip_comment(data, pos)?;
                continue;
            }
            if pos + 1 < data.len() && data[pos + 1] == b'!' {
                pos = sax_skip_decl(data, pos)?;
                continue;
            }
            pos = sax_parse_element(data, pos, events)?;
        } else {
            let start = pos;
            while pos < data.len() && data[pos] != b'<' {
                pos += 1;
            }
            let text = String::from_utf8_lossy(&data[start..pos]).into_owned();
            let text = unescape_xml(&text);
            if !text.trim().is_empty() {
                events.push(SaxEvent::Characters { content: text });
            }
        }
    }
    Ok(pos)
}

fn sax_parse_element(data: &[u8], mut pos: usize, events: &mut Vec<SaxEvent>) -> Result<usize, String> {
    pos += 1;
    let (name, new_pos) = parse_name(data, pos)?;
    pos = new_pos;

    let mut attrs = Vec::new();
    loop {
        pos = skip_ws(data, pos);
        if pos >= data.len() {
            return Err("unexpected end of tag".to_string());
        }
        if data[pos] == b'>' || (data[pos] == b'/' && pos + 1 < data.len() && data[pos + 1] == b'>') {
            break;
        }
        let (attr_name, new_pos) = parse_name(data, pos)?;
        pos = skip_ws(data, new_pos);
        if pos < data.len() && data[pos] == b'=' {
            pos = skip_ws(data, pos + 1);
            let (attr_val, new_pos) = parse_attr_value(data, pos)?;
            pos = new_pos;
            attrs.push((attr_name, attr_val));
        } else {
            attrs.push((attr_name, String::new()));
        }
    }

    events.push(SaxEvent::StartElement {
        name: name.clone(),
        attrs,
    });

    if data[pos] == b'/' {
        pos += 2;
        events.push(SaxEvent::EndElement { name });
        return Ok(pos);
    }
    pos += 1;

    pos = sax_parse_content(data, pos, events)?;

    if pos + 1 < data.len() && data[pos] == b'<' && data[pos + 1] == b'/' {
        pos += 2;
        while pos < data.len() && data[pos] != b'>' {
            pos += 1;
        }
        if pos < data.len() {
            pos += 1;
        }
    }
    events.push(SaxEvent::EndElement { name });
    Ok(pos)
}

fn sax_skip_pi(data: &[u8], mut pos: usize, events: &mut Vec<SaxEvent>) -> Result<usize, String> {
    pos += 2;
    let start = pos;
    while pos + 1 < data.len() && !(data[pos] == b'?' && data[pos + 1] == b'>') {
        pos += 1;
    }
    let content = String::from_utf8_lossy(&data[start..pos]).into_owned();
    let parts: Vec<&str> = content.splitn(2, char::is_whitespace).collect();
    if parts.len() == 2 && parts[0] != "xml" {
        events.push(SaxEvent::ProcessingInstruction {
            target: parts[0].to_string(),
            data: parts[1].trim().to_string(),
        });
    }
    if pos + 1 < data.len() {
        pos += 2;
    }
    Ok(pos)
}

fn sax_skip_comment(data: &[u8], mut pos: usize) -> Result<usize, String> {
    pos += 4;
    while pos + 2 < data.len() && !(data[pos] == b'-' && data[pos + 1] == b'-' && data[pos + 2] == b'>') {
        pos += 1;
    }
    if pos + 2 < data.len() {
        pos += 3;
    }
    Ok(pos)
}

fn sax_skip_decl(data: &[u8], mut pos: usize) -> Result<usize, String> {
    while pos < data.len() && data[pos] != b'>' {
        pos += 1;
    }
    if pos < data.len() {
        pos += 1;
    }
    Ok(pos)
}

struct SaxParserState {
    events: Vec<SaxEvent>,
    index: usize,
}

thread_local! {
    static SAX_PARSERS: RefCell<HashMap<i64, SaxParserState>> = RefCell::new(HashMap::new());
}

// ---------------------------------------------------------------------------
// Public extern "C" intrinsics
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn molt_xml_sax_parsestring(xml_bits: u64) -> u64 {
    with_core_gil!(_py, {
        let xml = match string_obj_to_owned(obj_from_bits(xml_bits)) {
            Some(s) => s,
            None => {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "argument must be str or bytes",
                );
            }
        };
        match sax_parse(&xml) {
            Ok(events) => {
                let id = next_handle_id();
                SAX_PARSERS.with(|m| {
                    m.borrow_mut().insert(id, SaxParserState { events, index: 0 });
                });
                int_bits_from_i64(_py, id)
            }
            Err(msg) => {
                raise_exception::<u64>(_py, "SAXParseException", &msg)
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_xml_sax_next_event(handle_bits: u64) -> u64 {
    with_core_gil!(_py, {
        let handle = to_i64(obj_from_bits(handle_bits)).unwrap_or(0);

        SAX_PARSERS.with(|m| {
            let mut map = m.borrow_mut();
            let state = match map.get_mut(&handle) {
                Some(s) => s,
                None => return MoltObject::none().bits(),
            };
            if state.index >= state.events.len() {
                return MoltObject::none().bits();
            }
            let event = state.events[state.index].clone();
            state.index += 1;

            match event {
                SaxEvent::StartDocument => {
                    mk_tuple(_py, &[mk_str(_py, "startDocument")])
                }
                SaxEvent::EndDocument => {
                    mk_tuple(_py, &[mk_str(_py, "endDocument")])
                }
                SaxEvent::StartElement { name, attrs } => {
                    let name_bits = mk_str(_py, &name);
                    let mut attr_pairs = Vec::with_capacity(attrs.len());
                    for (k, v) in &attrs {
                        attr_pairs.push(mk_tuple(_py, &[mk_str(_py, k), mk_str(_py, v)]));
                    }
                    let attrs_bits = mk_list(_py, &attr_pairs);
                    mk_tuple(_py, &[mk_str(_py, "startElement"), name_bits, attrs_bits])
                }
                SaxEvent::EndElement { name } => {
                    mk_tuple(_py, &[mk_str(_py, "endElement"), mk_str(_py, &name)])
                }
                SaxEvent::Characters { content } => {
                    mk_tuple(_py, &[mk_str(_py, "characters"), mk_str(_py, &content)])
                }
                SaxEvent::ProcessingInstruction { target, data } => {
                    mk_tuple(_py, &[mk_str(_py, "processingInstruction"), mk_str(_py, &target), mk_str(_py, &data)])
                }
            }
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_xml_sax_drop(handle_bits: u64) {
    let handle = to_i64(obj_from_bits(handle_bits)).unwrap_or(0);
    SAX_PARSERS.with(|m| m.borrow_mut().remove(&handle));
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_xml_sax_escape(text_bits: u64, _entities_bits: u64) -> u64 {
    with_core_gil!(_py, {
        let text = match string_obj_to_owned(obj_from_bits(text_bits)) {
            Some(s) => s,
            None => return MoltObject::none().bits(),
        };
        let result = text
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        mk_str(_py, &result)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_xml_sax_unescape(text_bits: u64, _entities_bits: u64) -> u64 {
    with_core_gil!(_py, {
        let text = match string_obj_to_owned(obj_from_bits(text_bits)) {
            Some(s) => s,
            None => return MoltObject::none().bits(),
        };
        let result = unescape_xml(&text);
        mk_str(_py, &result)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_xml_sax_quoteattr(text_bits: u64) -> u64 {
    with_core_gil!(_py, {
        let text = match string_obj_to_owned(obj_from_bits(text_bits)) {
            Some(s) => s,
            None => return MoltObject::none().bits(),
        };
        let escaped = text
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;");
        let result = format!("\"{}\"", escaped);
        mk_str(_py, &result)
    })
}
