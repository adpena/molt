//! Intrinsics for `xml.etree.ElementTree` stdlib module.
//!
//! Coverage: Element, SubElement, ElementTree, parse, fromstring, tostring,
//! iterparse, indent, Comment, ProcessingInstruction, register_namespace.

use crate::{
    MoltObject, alloc_list, alloc_string, alloc_tuple, inc_ref_bits, int_bits_from_i64, is_truthy,
    obj_from_bits, raise_exception, string_obj_to_owned, to_i64,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};

// ---------------------------------------------------------------------------
// Handle registry
// ---------------------------------------------------------------------------

static NEXT_HANDLE_ID: AtomicI64 = AtomicI64::new(1);

fn next_handle_id() -> i64 {
    NEXT_HANDLE_ID.fetch_add(1, Ordering::Relaxed)
}

fn mk_str(py: &crate::PyToken<'_>, s: &str) -> u64 {
    let ptr = alloc_string(py, s.as_bytes());
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

fn mk_list(py: &crate::PyToken<'_>, elems: &[u64]) -> u64 {
    let ptr = alloc_list(py, elems);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

fn mk_tuple(py: &crate::PyToken<'_>, elems: &[u64]) -> u64 {
    let ptr = alloc_tuple(py, elems);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

// ---------------------------------------------------------------------------
// Element representation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct XmlElement {
    tag: String,
    text: Option<String>,
    tail: Option<String>,
    attrib: HashMap<String, String>,
    children: Vec<i64>,
}

impl XmlElement {
    fn new(tag: String, attrib: HashMap<String, String>) -> Self {
        XmlElement {
            tag,
            text: None,
            tail: None,
            attrib,
            children: Vec::new(),
        }
    }
}

thread_local! {
    static ELEMENTS: RefCell<HashMap<i64, XmlElement>> = RefCell::new(HashMap::new());
    static NS_MAP: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
}

fn store_element(elem: XmlElement) -> i64 {
    let id = next_handle_id();
    ELEMENTS.with(|m| m.borrow_mut().insert(id, elem));
    id
}

fn with_element<F, R>(handle: i64, f: F) -> Option<R>
where
    F: FnOnce(&XmlElement) -> R,
{
    ELEMENTS.with(|m| m.borrow().get(&handle).map(f))
}

fn with_element_mut<F, R>(handle: i64, f: F) -> Option<R>
where
    F: FnOnce(&mut XmlElement) -> R,
{
    ELEMENTS.with(|m| m.borrow_mut().get_mut(&handle).map(f))
}

// ---------------------------------------------------------------------------
// Simple XML parser (minimal well-formed XML subset)
// ---------------------------------------------------------------------------

fn parse_xml_string(xml: &str) -> Result<i64, String> {
    let xml = xml.trim();
    if xml.is_empty() {
        return Err("no element found".to_string());
    }
    let bytes = xml.as_bytes();
    let (handle, _) = parse_element(bytes, 0)?;
    Ok(handle)
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
        return Err("expected quote in attribute value".to_string());
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

fn unescape_xml(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&apos;", "'")
        .replace("&quot;", "\"")
}

fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

fn parse_element(data: &[u8], mut pos: usize) -> Result<(i64, usize), String> {
    pos = skip_ws(data, pos);
    // skip XML declaration / processing instructions
    while pos + 1 < data.len() && data[pos] == b'<' && data[pos + 1] == b'?' {
        while pos < data.len() {
            if pos + 1 < data.len() && data[pos] == b'?' && data[pos + 1] == b'>' {
                pos += 2;
                break;
            }
            pos += 1;
        }
        pos = skip_ws(data, pos);
    }
    // skip comments
    while pos + 3 < data.len()
        && data[pos] == b'<'
        && data[pos + 1] == b'!'
        && data[pos + 2] == b'-'
        && data[pos + 3] == b'-'
    {
        while pos + 2 < data.len() {
            if data[pos] == b'-' && data[pos + 1] == b'-' && data[pos + 2] == b'>' {
                pos += 3;
                break;
            }
            pos += 1;
        }
        pos = skip_ws(data, pos);
    }

    if pos >= data.len() || data[pos] != b'<' {
        return Err("expected '<'".to_string());
    }
    pos += 1;
    let (tag, new_pos) = parse_name(data, pos)?;
    pos = new_pos;

    let mut attrib = HashMap::new();
    loop {
        pos = skip_ws(data, pos);
        if pos >= data.len() {
            return Err("unexpected end of tag".to_string());
        }
        if data[pos] == b'>' || (data[pos] == b'/' && pos + 1 < data.len() && data[pos + 1] == b'>')
        {
            break;
        }
        let (attr_name, new_pos) = parse_name(data, pos)?;
        pos = skip_ws(data, new_pos);
        if pos < data.len() && data[pos] == b'=' {
            pos = skip_ws(data, pos + 1);
            let (attr_val, new_pos) = parse_attr_value(data, pos)?;
            pos = new_pos;
            attrib.insert(attr_name, attr_val);
        } else {
            attrib.insert(attr_name, String::new());
        }
    }

    let mut elem = XmlElement::new(tag.clone(), attrib);

    if data[pos] == b'/' {
        pos += 2;
        let handle = store_element(elem);
        return Ok((handle, pos));
    }
    pos += 1;

    let text_start = pos;
    while pos < data.len() && data[pos] != b'<' {
        pos += 1;
    }
    if pos > text_start {
        let text = unescape_xml(&String::from_utf8_lossy(&data[text_start..pos]));
        if !text.is_empty() {
            elem.text = Some(text);
        }
    }

    loop {
        if pos >= data.len() {
            break;
        }
        if pos + 1 < data.len() && data[pos] == b'<' && data[pos + 1] == b'/' {
            pos += 2;
            while pos < data.len() && data[pos] != b'>' {
                pos += 1;
            }
            if pos < data.len() {
                pos += 1;
            }
            break;
        }
        if data[pos] == b'<' {
            let (child_handle, new_pos) = parse_element(data, pos)?;
            elem.children.push(child_handle);
            pos = new_pos;

            let tail_start = pos;
            while pos < data.len() && data[pos] != b'<' {
                pos += 1;
            }
            if pos > tail_start {
                let tail = unescape_xml(&String::from_utf8_lossy(&data[tail_start..pos]));
                if !tail.is_empty() {
                    with_element_mut(child_handle, |e| {
                        e.tail = Some(tail);
                    });
                }
            }
        } else {
            pos += 1;
        }
    }

    let handle = store_element(elem);
    Ok((handle, pos))
}

fn serialize_element(handle: i64, short_empty: bool) -> String {
    let (tag, text, tail, attrib, children) = match with_element(handle, |e| {
        (
            e.tag.clone(),
            e.text.clone(),
            e.tail.clone(),
            e.attrib.clone(),
            e.children.clone(),
        )
    }) {
        Some(v) => v,
        None => return String::new(),
    };

    let mut out = String::new();
    out.push('<');
    out.push_str(&tag);
    let mut sorted_attrs: Vec<_> = attrib.iter().collect();
    sorted_attrs.sort_by_key(|(k, _)| (*k).clone());
    for (k, v) in &sorted_attrs {
        out.push(' ');
        out.push_str(k);
        out.push_str("=\"");
        out.push_str(&escape_xml(v));
        out.push('"');
    }

    if children.is_empty() && text.is_none() && short_empty {
        out.push_str(" />");
    } else {
        out.push('>');
        if let Some(t) = &text {
            out.push_str(&escape_xml(t));
        }
        for &child_h in &children {
            out.push_str(&serialize_element(child_h, short_empty));
        }
        out.push_str("</");
        out.push_str(&tag);
        out.push('>');
    }

    if let Some(t) = &tail {
        out.push_str(&escape_xml(t));
    }

    out
}

fn indent_element(handle: i64, space: &str, level: usize) {
    let children = match with_element(handle, |e| e.children.clone()) {
        Some(c) => c,
        None => return,
    };
    if children.is_empty() {
        return;
    }
    let indent = format!("\n{}", space.repeat(level + 1));
    let child_indent = format!("\n{}", space.repeat(level));

    with_element_mut(handle, |e| {
        if e.text.is_none() || e.text.as_deref() == Some("") {
            e.text = Some(indent.clone());
        }
    });

    let last_idx = children.len() - 1;
    for (i, &child_h) in children.iter().enumerate() {
        indent_element(child_h, space, level + 1);
        if i < last_idx {
            with_element_mut(child_h, |e| {
                if e.tail.is_none() || e.tail.as_deref() == Some("") {
                    e.tail = Some(indent.clone());
                }
            });
        } else {
            with_element_mut(child_h, |e| {
                if e.tail.is_none() || e.tail.as_deref() == Some("") {
                    e.tail = Some(child_indent.clone());
                }
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Public extern "C" intrinsics
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn molt_xml_element_new(tag_bits: u64, _attrib_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let tag = match string_obj_to_owned(obj_from_bits(tag_bits)) {
            Some(s) => s,
            None => return MoltObject::none().bits(),
        };
        let attrib = HashMap::new();
        let handle = store_element(XmlElement::new(tag, attrib));
        int_bits_from_i64(_py, handle)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_xml_element_tag(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = to_i64(obj_from_bits(handle_bits)).unwrap_or(0);
        match with_element(handle, |e| e.tag.clone()) {
            Some(tag) => mk_str(_py, &tag),
            None => MoltObject::none().bits(),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_xml_element_set_tag(handle_bits: u64, tag_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = to_i64(obj_from_bits(handle_bits)).unwrap_or(0);
        let tag = match string_obj_to_owned(obj_from_bits(tag_bits)) {
            Some(s) => s,
            None => return MoltObject::none().bits(),
        };
        with_element_mut(handle, |e| e.tag = tag);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_xml_element_text(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = to_i64(obj_from_bits(handle_bits)).unwrap_or(0);
        match with_element(handle, |e| e.text.clone()) {
            Some(Some(t)) => mk_str(_py, &t),
            _ => MoltObject::none().bits(),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_xml_element_set_text(handle_bits: u64, text_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = to_i64(obj_from_bits(handle_bits)).unwrap_or(0);
        let text_obj = obj_from_bits(text_bits);
        let text = if text_obj.is_none() {
            None
        } else {
            string_obj_to_owned(text_obj)
        };
        with_element_mut(handle, |e| e.text = text);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_xml_element_tail(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = to_i64(obj_from_bits(handle_bits)).unwrap_or(0);
        match with_element(handle, |e| e.tail.clone()) {
            Some(Some(t)) => mk_str(_py, &t),
            _ => MoltObject::none().bits(),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_xml_element_set_tail(handle_bits: u64, tail_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = to_i64(obj_from_bits(handle_bits)).unwrap_or(0);
        let tail_obj = obj_from_bits(tail_bits);
        let tail = if tail_obj.is_none() {
            None
        } else {
            string_obj_to_owned(tail_obj)
        };
        with_element_mut(handle, |e| e.tail = tail);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_xml_element_get_attrib(
    handle_bits: u64,
    key_bits: u64,
    default_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = to_i64(obj_from_bits(handle_bits)).unwrap_or(0);
        let key = match string_obj_to_owned(obj_from_bits(key_bits)) {
            Some(s) => s,
            None => {
                inc_ref_bits(_py, default_bits);
                return default_bits;
            }
        };
        match with_element(handle, |e| e.attrib.get(&key).cloned()) {
            Some(Some(v)) => mk_str(_py, &v),
            _ => {
                inc_ref_bits(_py, default_bits);
                default_bits
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_xml_element_set_attrib(
    handle_bits: u64,
    key_bits: u64,
    value_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = to_i64(obj_from_bits(handle_bits)).unwrap_or(0);
        let key = match string_obj_to_owned(obj_from_bits(key_bits)) {
            Some(s) => s,
            None => return MoltObject::none().bits(),
        };
        let value = match string_obj_to_owned(obj_from_bits(value_bits)) {
            Some(s) => s,
            None => return MoltObject::none().bits(),
        };
        with_element_mut(handle, |e| {
            e.attrib.insert(key, value);
        });
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_xml_element_attrib_items(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = to_i64(obj_from_bits(handle_bits)).unwrap_or(0);
        match with_element(handle, |e| {
            let mut items: Vec<(String, String)> = e
                .attrib
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            items.sort_by(|a, b| a.0.cmp(&b.0));
            items
        }) {
            Some(items) => {
                let mut pair_bits = Vec::with_capacity(items.len());
                for (k, v) in items {
                    let k_bits = mk_str(_py, &k);
                    let v_bits = mk_str(_py, &v);
                    pair_bits.push(mk_tuple(_py, &[k_bits, v_bits]));
                }
                mk_list(_py, &pair_bits)
            }
            None => MoltObject::none().bits(),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_xml_element_append(parent_bits: u64, child_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let parent = to_i64(obj_from_bits(parent_bits)).unwrap_or(0);
        let child = to_i64(obj_from_bits(child_bits)).unwrap_or(0);
        with_element_mut(parent, |e| e.children.push(child));
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_xml_element_remove(parent_bits: u64, child_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let parent = to_i64(obj_from_bits(parent_bits)).unwrap_or(0);
        let child = to_i64(obj_from_bits(child_bits)).unwrap_or(0);
        with_element_mut(parent, |e| {
            e.children.retain(|&h| h != child);
        });
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_xml_element_children(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = to_i64(obj_from_bits(handle_bits)).unwrap_or(0);
        match with_element(handle, |e| e.children.clone()) {
            Some(children) => {
                let bits: Vec<u64> = children
                    .iter()
                    .map(|&h| int_bits_from_i64(_py, h))
                    .collect();
                mk_list(_py, &bits)
            }
            None => MoltObject::none().bits(),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_xml_element_len(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = to_i64(obj_from_bits(handle_bits)).unwrap_or(0);
        match with_element(handle, |e| e.children.len() as i64) {
            Some(n) => int_bits_from_i64(_py, n),
            None => int_bits_from_i64(_py, 0),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_xml_element_find(handle_bits: u64, path_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = to_i64(obj_from_bits(handle_bits)).unwrap_or(0);
        let path = match string_obj_to_owned(obj_from_bits(path_bits)) {
            Some(s) => s,
            None => return MoltObject::none().bits(),
        };
        match with_element(handle, |e| {
            for &child_h in &e.children {
                let tag_matches = with_element(child_h, |c| c.tag == path).unwrap_or(false);
                if tag_matches {
                    return Some(child_h);
                }
            }
            None
        }) {
            Some(Some(h)) => int_bits_from_i64(_py, h),
            _ => MoltObject::none().bits(),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_xml_element_findall(handle_bits: u64, path_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = to_i64(obj_from_bits(handle_bits)).unwrap_or(0);
        let path = match string_obj_to_owned(obj_from_bits(path_bits)) {
            Some(s) => s,
            None => return MoltObject::none().bits(),
        };
        match with_element(handle, |e| {
            let mut found = Vec::new();
            for &child_h in &e.children {
                let tag_matches =
                    with_element(child_h, |c| c.tag == path || path == "*").unwrap_or(false);
                if tag_matches {
                    found.push(child_h);
                }
            }
            found
        }) {
            Some(found) => {
                let bits: Vec<u64> = found.iter().map(|&h| int_bits_from_i64(_py, h)).collect();
                mk_list(_py, &bits)
            }
            None => MoltObject::none().bits(),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_xml_element_findtext(
    handle_bits: u64,
    path_bits: u64,
    default_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = to_i64(obj_from_bits(handle_bits)).unwrap_or(0);
        let path = match string_obj_to_owned(obj_from_bits(path_bits)) {
            Some(s) => s,
            None => {
                inc_ref_bits(_py, default_bits);
                return default_bits;
            }
        };
        match with_element(handle, |e| {
            for &child_h in &e.children {
                let tag_matches = with_element(child_h, |c| c.tag == path).unwrap_or(false);
                if tag_matches {
                    return with_element(child_h, |c| c.text.clone());
                }
            }
            None
        }) {
            Some(Some(Some(t))) => mk_str(_py, &t),
            _ => {
                inc_ref_bits(_py, default_bits);
                default_bits
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_xml_element_iter(handle_bits: u64, tag_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = to_i64(obj_from_bits(handle_bits)).unwrap_or(0);
        let tag_filter = if obj_from_bits(tag_bits).is_none() {
            None
        } else {
            string_obj_to_owned(obj_from_bits(tag_bits))
        };

        fn collect(handle: i64, tag_filter: &Option<String>, result: &mut Vec<i64>) {
            let matches = match tag_filter {
                None => true,
                Some(t) => with_element(handle, |e| e.tag == *t || t == "*").unwrap_or(false),
            };
            if matches {
                result.push(handle);
            }
            if let Some(children) = with_element(handle, |e| e.children.clone()) {
                for &child_h in &children {
                    collect(child_h, tag_filter, result);
                }
            }
        }

        let mut result = Vec::new();
        collect(handle, &tag_filter, &mut result);

        let bits: Vec<u64> = result.iter().map(|&h| int_bits_from_i64(_py, h)).collect();
        mk_list(_py, &bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_xml_element_drop(handle_bits: u64) {
    let handle = to_i64(obj_from_bits(handle_bits)).unwrap_or(0);
    ELEMENTS.with(|m| m.borrow_mut().remove(&handle));
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_xml_fromstring(xml_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let xml = match string_obj_to_owned(obj_from_bits(xml_bits)) {
            Some(s) => s,
            None => {
                return raise_exception::<u64>(_py, "TypeError", "argument must be str");
            }
        };
        match parse_xml_string(&xml) {
            Ok(handle) => int_bits_from_i64(_py, handle),
            Err(msg) => raise_exception::<u64>(_py, "ParseError", &msg),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_xml_tostring(
    handle_bits: u64,
    encoding_bits: u64,
    short_empty_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = to_i64(obj_from_bits(handle_bits)).unwrap_or(0);
        let short_empty = is_truthy(_py, obj_from_bits(short_empty_bits));
        let encoding = string_obj_to_owned(obj_from_bits(encoding_bits));

        let xml_str = serialize_element(handle, short_empty);

        match encoding.as_deref() {
            Some("unicode") | None => mk_str(_py, &xml_str),
            Some(enc) => {
                let with_decl = format!("<?xml version='1.0' encoding='{}'?>\n{}", enc, xml_str);
                mk_str(_py, &with_decl)
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_xml_indent(handle_bits: u64, space_bits: u64, level_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = to_i64(obj_from_bits(handle_bits)).unwrap_or(0);
        let space =
            string_obj_to_owned(obj_from_bits(space_bits)).unwrap_or_else(|| "  ".to_string());
        let level = to_i64(obj_from_bits(level_bits)).unwrap_or(0) as usize;
        indent_element(handle, &space, level);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_xml_register_namespace(prefix_bits: u64, uri_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let prefix = match string_obj_to_owned(obj_from_bits(prefix_bits)) {
            Some(s) => s,
            None => return MoltObject::none().bits(),
        };
        let uri = match string_obj_to_owned(obj_from_bits(uri_bits)) {
            Some(s) => s,
            None => return MoltObject::none().bits(),
        };
        NS_MAP.with(|m| m.borrow_mut().insert(uri, prefix));
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_xml_element_clear(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let handle = to_i64(obj_from_bits(handle_bits)).unwrap_or(0);
        with_element_mut(handle, |e| {
            e.text = None;
            e.tail = None;
            e.attrib.clear();
            e.children.clear();
        });
        MoltObject::none().bits()
    })
}
