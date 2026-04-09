//! `molt-rs` — runtime types and helpers for [Molt](https://github.com/adpena/molt)-generated Rust code.
//!
//! When you compile a Python file with `molt build --target rust`, the backend
//! emits a self-contained `.rs` file that includes a copy of this runtime as an
//! inline prelude.  If you prefer a proper crate dependency instead, pass the
//! `--use-crate` flag:
//!
//! ```bash
//! molt build --target rust --use-crate my_script.py -o my_script.rs
//! ```
//!
//! Then add to your `Cargo.toml`:
//!
//! ```toml
//! [dependencies]
//! molt-rs = "0.1"
//! ```
//!
//! And the generated file will begin with `use molt_rs::*;` rather than the inline prelude.
//!
//! # Core type
//!
//! [`MoltValue`] is the universal dynamic type used by Molt-generated code.
//! It mirrors Python's dynamic type system at the value level.

use std::sync::Arc;

// ─── MoltValue ────────────────────────────────────────────────────────────────

/// The universal dynamic value type for Molt-generated Rust code.
///
/// Maps one-to-one with Python's runtime types:
///
/// | Python          | MoltValue variant                                  |
/// |---|---|
/// | `None`          | `MoltValue::None`                                  |
/// | `bool`          | `MoltValue::Bool(bool)`                            |
/// | `int`           | `MoltValue::Int(i64)`                              |
/// | `float`         | `MoltValue::Float(f64)`                            |
/// | `str`           | `MoltValue::Str(String)`                           |
/// | `list`          | `MoltValue::List(Vec<MoltValue>)`                  |
/// | `dict`          | `MoltValue::Dict(Vec<(MoltValue, MoltValue)>)`     |
/// | function        | `MoltValue::Func(Arc<dyn Fn…>)`                    |
///
/// `Dict` uses `Vec<(K,V)>` rather than `HashMap` to:
/// - Preserve insertion order (Python 3.7+ semantics)
/// - Avoid `Hash` bounds (floats can't implement `Hash` in Rust)
/// - Keep the crate dependency-free
#[derive(Clone)]
pub enum MoltValue {
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<MoltValue>),
    Dict(Vec<(MoltValue, MoltValue)>),
    Func(Arc<dyn Fn(Vec<MoltValue>) -> MoltValue + Send + Sync>),
}

impl std::fmt::Debug for MoltValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MoltValue::None => write!(f, "None"),
            MoltValue::Bool(b) => write!(f, "{}", if *b { "True" } else { "False" }),
            MoltValue::Int(i) => write!(f, "{}", i),
            MoltValue::Float(v) => write!(f, "{}", v),
            MoltValue::Str(s) => write!(f, "{:?}", s),
            MoltValue::List(l) => write!(f, "{:?}", l),
            MoltValue::Dict(d) => {
                write!(f, "{{")?;
                for (i, (k, v)) in d.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{:?}: {:?}", k, v)?;
                }
                write!(f, "}}")
            }
            MoltValue::Func(_) => write!(f, "<function>"),
        }
    }
}

impl PartialEq for MoltValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (MoltValue::None, MoltValue::None) => true,
            (MoltValue::Bool(a), MoltValue::Bool(b)) => a == b,
            (MoltValue::Int(a), MoltValue::Int(b)) => a == b,
            (MoltValue::Float(a), MoltValue::Float(b)) => a == b,
            (MoltValue::Int(a), MoltValue::Float(b)) => (*a as f64) == *b,
            (MoltValue::Float(a), MoltValue::Int(b)) => *a == (*b as f64),
            (MoltValue::Str(a), MoltValue::Str(b)) => a == b,
            (MoltValue::List(a), MoltValue::List(b)) => a == b,
            (MoltValue::Dict(a), MoltValue::Dict(b)) => a == b,
            (MoltValue::Func(_), MoltValue::Func(_)) => false, // functions never equal
            _ => false,
        }
    }
}

// ─── Type coercions ────────────────────────────────────────────────────────────

/// Python `bool(x)` — truthy conversion matching Python semantics.
///
/// Falsy: `None`, `False`, `0`, `0.0`, empty str/list/dict.
pub fn molt_bool(x: &MoltValue) -> bool {
    match x {
        MoltValue::None => false,
        MoltValue::Bool(b) => *b,
        MoltValue::Int(i) => *i != 0,
        MoltValue::Float(f) => *f != 0.0,
        MoltValue::Str(s) => !s.is_empty(),
        MoltValue::List(l) => !l.is_empty(),
        MoltValue::Dict(d) => !d.is_empty(),
        MoltValue::Func(_) => true,
    }
}

/// Python `int(x)` — coerce to `i64`.
pub fn molt_int(x: &MoltValue) -> i64 {
    match x {
        MoltValue::Int(i) => *i,
        MoltValue::Float(f) => *f as i64,
        MoltValue::Bool(b) => {
            if *b {
                1
            } else {
                0
            }
        }
        MoltValue::Str(s) => s.trim().parse::<i64>().unwrap_or(0),
        _ => 0,
    }
}

/// Python `float(x)` — coerce to `f64`.
pub fn molt_float(x: &MoltValue) -> f64 {
    match x {
        MoltValue::Float(f) => *f,
        MoltValue::Int(i) => *i as f64,
        MoltValue::Bool(b) => {
            if *b {
                1.0
            } else {
                0.0
            }
        }
        MoltValue::Str(s) => s.trim().parse::<f64>().unwrap_or(0.0),
        _ => 0.0,
    }
}

/// Python `str(x)` — human-readable string, matching Python's `str()` output.
pub fn molt_str(x: &MoltValue) -> String {
    match x {
        MoltValue::None => "None".to_string(),
        MoltValue::Bool(b) => {
            if *b {
                "True".to_string()
            } else {
                "False".to_string()
            }
        }
        MoltValue::Int(i) => i.to_string(),
        MoltValue::Float(f) => {
            if f.fract() == 0.0 && f.abs() < 1e15 {
                format!("{:.1}", f)
            } else {
                format!("{}", f)
            }
        }
        MoltValue::Str(s) => s.clone(),
        MoltValue::List(l) => {
            let inner: Vec<String> = l.iter().map(molt_repr_inner).collect();
            format!("[{}]", inner.join(", "))
        }
        MoltValue::Dict(d) => {
            let inner: Vec<String> = d
                .iter()
                .map(|(k, v)| format!("{}: {}", molt_repr_inner(k), molt_repr_inner(v)))
                .collect();
            format!("{{{}}}", inner.join(", "))
        }
        MoltValue::Func(_) => "<function>".to_string(),
    }
}

/// Python `repr(x)` — adds quotes around strings, used in container display.
pub fn molt_repr_inner(x: &MoltValue) -> String {
    match x {
        MoltValue::Str(s) => format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'")),
        _ => molt_str(x),
    }
}

/// Python `repr(x)` — returns `MoltValue::Str`.
pub fn molt_repr(x: &MoltValue) -> MoltValue {
    MoltValue::Str(molt_repr_inner(x))
}

// ─── print / len ──────────────────────────────────────────────────────────────

/// Python `print(*args)` — space-joined `str()` of each argument.
pub fn molt_print(args: &[MoltValue]) {
    let parts: Vec<String> = args.iter().map(molt_str).collect();
    println!("{}", parts.join(" "));
}

/// Python `len(x)`.
pub fn molt_len(x: &MoltValue) -> MoltValue {
    match x {
        MoltValue::Str(s) => MoltValue::Int(s.chars().count() as i64),
        MoltValue::List(l) => MoltValue::Int(l.len() as i64),
        MoltValue::Dict(d) => MoltValue::Int(d.len() as i64),
        _ => MoltValue::Int(0),
    }
}

// ─── range ────────────────────────────────────────────────────────────────────

/// Python `range(start, stop, step)` — returns a `MoltValue::List`.
pub fn molt_range(start: i64, stop: i64, step: i64) -> MoltValue {
    if step == 0 {
        return MoltValue::List(vec![]);
    }
    let mut result = Vec::new();
    let mut i = start;
    if step > 0 {
        while i < stop {
            result.push(MoltValue::Int(i));
            i += step;
        }
    } else {
        while i > stop {
            result.push(MoltValue::Int(i));
            i += step;
        }
    }
    MoltValue::List(result)
}

/// Returns an iterator over integer values for `for i in range(start, stop, step)`.
pub fn molt_range_iter(start: i64, stop: i64, step: i64) -> impl Iterator<Item = MoltValue> {
    let mut i = start;
    let forward = step > 0;
    std::iter::from_fn(move || {
        if step == 0 {
            return None;
        }
        if (forward && i < stop) || (!forward && i > stop) {
            let v = i;
            i += step;
            Some(MoltValue::Int(v))
        } else {
            None
        }
    })
}

/// Returns an iterator suitable for `for x in iterable`.
pub fn molt_iter(x: &MoltValue) -> Vec<MoltValue> {
    match x {
        MoltValue::List(l) => l.clone(),
        MoltValue::Dict(d) => d.iter().map(|(k, _)| k.clone()).collect(),
        MoltValue::Str(s) => s.chars().map(|c| MoltValue::Str(c.to_string())).collect(),
        _ => vec![],
    }
}

// ─── Arithmetic ───────────────────────────────────────────────────────────────

/// Python `a + b` — int/float/str/list concatenation.
pub fn molt_add(a: MoltValue, b: MoltValue) -> MoltValue {
    match (&a, &b) {
        (MoltValue::Int(x), MoltValue::Int(y)) => MoltValue::Int(x.wrapping_add(*y)),
        (MoltValue::Float(x), MoltValue::Float(y)) => MoltValue::Float(x + y),
        (MoltValue::Int(x), MoltValue::Float(y)) => MoltValue::Float(*x as f64 + y),
        (MoltValue::Float(x), MoltValue::Int(y)) => MoltValue::Float(x + *y as f64),
        (MoltValue::Str(x), MoltValue::Str(y)) => MoltValue::Str(x.clone() + y),
        (MoltValue::List(x), MoltValue::List(y)) => {
            let mut v = x.clone();
            v.extend(y.iter().cloned());
            MoltValue::List(v)
        }
        _ => MoltValue::None,
    }
}

/// Python `a - b`.
pub fn molt_sub(a: MoltValue, b: MoltValue) -> MoltValue {
    match (&a, &b) {
        (MoltValue::Int(x), MoltValue::Int(y)) => MoltValue::Int(x.wrapping_sub(*y)),
        (MoltValue::Float(x), MoltValue::Float(y)) => MoltValue::Float(x - y),
        (MoltValue::Int(x), MoltValue::Float(y)) => MoltValue::Float(*x as f64 - y),
        (MoltValue::Float(x), MoltValue::Int(y)) => MoltValue::Float(x - *y as f64),
        _ => MoltValue::None,
    }
}

/// Python `a * b`.
pub fn molt_mul(a: MoltValue, b: MoltValue) -> MoltValue {
    match (&a, &b) {
        (MoltValue::Int(x), MoltValue::Int(y)) => MoltValue::Int(x.wrapping_mul(*y)),
        (MoltValue::Float(x), MoltValue::Float(y)) => MoltValue::Float(x * y),
        (MoltValue::Int(x), MoltValue::Float(y)) => MoltValue::Float(*x as f64 * y),
        (MoltValue::Float(x), MoltValue::Int(y)) => MoltValue::Float(x * *y as f64),
        (MoltValue::Str(s), MoltValue::Int(n)) => MoltValue::Str(s.repeat(*n as usize)),
        (MoltValue::List(l), MoltValue::Int(n)) => {
            let mut v = Vec::with_capacity(l.len() * (*n as usize).max(0));
            for _ in 0..(*n as usize) {
                v.extend(l.iter().cloned());
            }
            MoltValue::List(v)
        }
        _ => MoltValue::None,
    }
}

/// Python `a / b` — always float division.
pub fn molt_div(a: MoltValue, b: MoltValue) -> MoltValue {
    let af = molt_float(&a);
    let bf = molt_float(&b);
    MoltValue::Float(af / bf)
}

/// Python `a // b` — floor division.
pub fn molt_floor_div(a: MoltValue, b: MoltValue) -> MoltValue {
    match (&a, &b) {
        (MoltValue::Int(x), MoltValue::Int(y)) => {
            if *y == 0 {
                MoltValue::None
            } else {
                MoltValue::Int(x.div_euclid(*y))
            }
        }
        _ => {
            let af = molt_float(&a);
            let bf = molt_float(&b);
            MoltValue::Float((af / bf).floor())
        }
    }
}

/// Python `a % b` — modulo.
pub fn molt_mod(a: MoltValue, b: MoltValue) -> MoltValue {
    match (&a, &b) {
        (MoltValue::Int(x), MoltValue::Int(y)) => {
            if *y == 0 {
                MoltValue::None
            } else {
                MoltValue::Int(x.rem_euclid(*y))
            }
        }
        _ => {
            let af = molt_float(&a);
            let bf = molt_float(&b);
            MoltValue::Float(af.rem_euclid(bf))
        }
    }
}

/// Python `a ** b` — exponentiation.
pub fn molt_pow(a: MoltValue, b: MoltValue) -> MoltValue {
    match (&a, &b) {
        (MoltValue::Int(x), MoltValue::Int(y)) if *y >= 0 => {
            MoltValue::Int((*x as i128).pow(*y as u32) as i64)
        }
        _ => MoltValue::Float(molt_float(&a).powf(molt_float(&b))),
    }
}

/// Python unary `-x`.
pub fn molt_neg(a: MoltValue) -> MoltValue {
    match a {
        MoltValue::Int(i) => MoltValue::Int(i.wrapping_neg()),
        MoltValue::Float(f) => MoltValue::Float(-f),
        _ => MoltValue::None,
    }
}

/// Python `not x`.
pub fn molt_not(a: &MoltValue) -> MoltValue {
    MoltValue::Bool(!molt_bool(a))
}

// ─── Comparison ───────────────────────────────────────────────────────────────

fn molt_numeric_cmp(a: &MoltValue, b: &MoltValue) -> std::cmp::Ordering {
    match (a, b) {
        (MoltValue::Int(x), MoltValue::Int(y)) => x.cmp(y),
        (MoltValue::Float(x), MoltValue::Float(y)) => {
            x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal)
        }
        (MoltValue::Int(x), MoltValue::Float(y)) => (*x as f64)
            .partial_cmp(y)
            .unwrap_or(std::cmp::Ordering::Equal),
        (MoltValue::Float(x), MoltValue::Int(y)) => x
            .partial_cmp(&(*y as f64))
            .unwrap_or(std::cmp::Ordering::Equal),
        (MoltValue::Str(x), MoltValue::Str(y)) => x.cmp(y),
        _ => std::cmp::Ordering::Equal,
    }
}

/// Python `a == b`.
pub fn molt_eq(a: &MoltValue, b: &MoltValue) -> bool {
    a == b
}
/// Python `a != b`.
pub fn molt_ne(a: &MoltValue, b: &MoltValue) -> bool {
    a != b
}
/// Python `a < b`.
pub fn molt_lt(a: &MoltValue, b: &MoltValue) -> bool {
    matches!(molt_numeric_cmp(a, b), std::cmp::Ordering::Less)
}
/// Python `a <= b`.
pub fn molt_le(a: &MoltValue, b: &MoltValue) -> bool {
    !matches!(molt_numeric_cmp(a, b), std::cmp::Ordering::Greater)
}
/// Python `a > b`.
pub fn molt_gt(a: &MoltValue, b: &MoltValue) -> bool {
    matches!(molt_numeric_cmp(a, b), std::cmp::Ordering::Greater)
}
/// Python `a >= b`.
pub fn molt_ge(a: &MoltValue, b: &MoltValue) -> bool {
    !matches!(molt_numeric_cmp(a, b), std::cmp::Ordering::Less)
}

// ─── Collection operations ────────────────────────────────────────────────────

/// Python `obj[key]` — index into list or dict.
pub fn molt_get_item(obj: &MoltValue, key: &MoltValue) -> MoltValue {
    match (obj, key) {
        (MoltValue::List(l), MoltValue::Int(i)) => {
            let idx = if *i < 0 {
                (l.len() as i64 + i) as usize
            } else {
                *i as usize
            };
            l.get(idx).cloned().unwrap_or(MoltValue::None)
        }
        (MoltValue::Dict(d), k) => d
            .iter()
            .find(|(ek, _)| ek == k)
            .map(|(_, v)| v.clone())
            .unwrap_or(MoltValue::None),
        (MoltValue::Str(s), MoltValue::Int(i)) => {
            let idx = if *i < 0 {
                (s.chars().count() as i64 + i) as usize
            } else {
                *i as usize
            };
            s.chars()
                .nth(idx)
                .map(|c| MoltValue::Str(c.to_string()))
                .unwrap_or(MoltValue::None)
        }
        _ => MoltValue::None,
    }
}

/// Python `obj[key] = val`.
pub fn molt_set_item(obj: &mut MoltValue, key: MoltValue, val: MoltValue) {
    match obj {
        MoltValue::List(l) => {
            if let MoltValue::Int(i) = key {
                let idx = if i < 0 {
                    (l.len() as i64 + i) as usize
                } else {
                    i as usize
                };
                if idx < l.len() {
                    l[idx] = val;
                }
            }
        }
        MoltValue::Dict(d) => {
            if let Some(entry) = d.iter_mut().find(|(k, _)| k == &key) {
                entry.1 = val;
            } else {
                d.push((key, val));
            }
        }
        _ => {}
    }
}

/// Python `list.append(val)`.
pub fn molt_list_append(list: &mut MoltValue, val: MoltValue) {
    if let MoltValue::List(l) = list {
        l.push(val);
    }
}

/// Python `obj.attr` — attribute access (keys(), values(), items(), etc.).
pub fn molt_get_attr(obj: &MoltValue, attr: &str) -> MoltValue {
    match (obj, attr) {
        (MoltValue::Dict(_), "keys") => MoltValue::Func(Arc::new({
            let obj = obj.clone();
            move |_| molt_dict_keys(&obj)
        })),
        (MoltValue::Dict(_), "values") => MoltValue::Func(Arc::new({
            let obj = obj.clone();
            move |_| molt_dict_values(&obj)
        })),
        (MoltValue::Dict(_), "items") => MoltValue::Func(Arc::new({
            let obj = obj.clone();
            move |_| molt_dict_items(&obj)
        })),
        _ => MoltValue::None,
    }
}

/// Merge class layout metadata for transpiled Rust class objects.
pub fn molt_class_merge_layout(
    class_obj: &mut MoltValue,
    offsets: MoltValue,
    size: MoltValue,
) -> MoltValue {
    let class_dict = match class_obj {
        MoltValue::Dict(d) => d,
        _ => panic!("class layout merge expects type"),
    };
    let hinted_size = match size {
        MoltValue::Int(v) if v >= 0 => v as usize,
        _ => panic!("__molt_layout_size__ must be int"),
    };
    let mut merged_offsets: Option<Vec<(MoltValue, MoltValue)>> = None;
    match offsets {
        MoltValue::None => {
            if let Some((_, MoltValue::Dict(existing))) = class_dict
                .iter()
                .find(
                    |(key, _)| matches!(key, MoltValue::Str(name) if name == "__molt_field_offsets__"),
                )
            {
                merged_offsets = Some(existing.clone());
            }
        }
        MoltValue::Dict(source_offsets) => {
            let target_index = if let Some(index) = class_dict
                .iter()
                .position(
                    |(key, _)| matches!(key, MoltValue::Str(name) if name == "__molt_field_offsets__"),
                )
            {
                index
            } else {
                class_dict.push((
                    MoltValue::Str("__molt_field_offsets__".to_string()),
                    MoltValue::Dict(vec![]),
                ));
                class_dict.len() - 1
            };
            let target_offsets = match &mut class_dict[target_index].1 {
                MoltValue::Dict(d) => d,
                _ => panic!("__molt_field_offsets__ must be dict"),
            };
            for (name, offset) in source_offsets {
                if target_offsets.iter().any(|(existing, _)| existing == &name) {
                    continue;
                }
                target_offsets.push((name, offset));
            }
            merged_offsets = Some(target_offsets.clone());
        }
        _ => panic!("__molt_field_offsets__ must be dict or None"),
    }

    let mut layout_size = class_dict
        .iter()
        .find_map(|(key, value)| match (key, value) {
            (MoltValue::Str(name), MoltValue::Int(existing))
                if name == "__molt_layout_size__" && *existing > 0 =>
            {
                Some(*existing as usize)
            }
            _ => None,
        })
        .unwrap_or(0);
    layout_size = layout_size.max(hinted_size);
    if let Some(offsets_dict) = merged_offsets.as_ref() {
        let mut max_end = 0usize;
        for (_, offset) in offsets_dict.iter() {
            if let MoltValue::Int(value) = offset {
                if *value < 0 {
                    continue;
                }
                let end = (*value as usize).saturating_add(std::mem::size_of::<u64>());
                if end > max_end {
                    max_end = end;
                }
            }
        }
        layout_size = layout_size.max(max_end.saturating_add(std::mem::size_of::<u64>()));
    }
    if layout_size == 0 {
        layout_size = std::mem::size_of::<u64>();
    }

    if let Some((_, value)) = class_dict
        .iter_mut()
        .find(
            |(key, _)| matches!(key, MoltValue::Str(name) if name == "__molt_layout_size__"),
        )
    {
        *value = MoltValue::Int(layout_size as i64);
    } else {
        class_dict.push((
            MoltValue::Str("__molt_layout_size__".to_string()),
            MoltValue::Int(layout_size as i64),
        ));
    }
    MoltValue::None
}

/// Python `elem in container`.
pub fn molt_in(elem: &MoltValue, container: &MoltValue) -> bool {
    match container {
        MoltValue::List(l) => l.iter().any(|x| x == elem),
        MoltValue::Dict(d) => d.iter().any(|(k, _)| k == elem),
        MoltValue::Str(s) => {
            if let MoltValue::Str(sub) = elem {
                s.contains(sub.as_str())
            } else {
                false
            }
        }
        _ => false,
    }
}

// ─── Built-in functions ───────────────────────────────────────────────────────

/// Python `enumerate(t, start)`.
pub fn molt_enumerate(t: &MoltValue, start: i64) -> MoltValue {
    let items = molt_iter(t);
    MoltValue::List(
        items
            .into_iter()
            .enumerate()
            .map(|(i, v)| MoltValue::List(vec![MoltValue::Int(start + i as i64), v]))
            .collect(),
    )
}

/// Python `zip(a, b)`.
pub fn molt_zip(a: &MoltValue, b: &MoltValue) -> MoltValue {
    let av = molt_iter(a);
    let bv = molt_iter(b);
    MoltValue::List(
        av.into_iter()
            .zip(bv)
            .map(|(x, y)| MoltValue::List(vec![x, y]))
            .collect(),
    )
}

/// Python `sorted(t)`.
pub fn molt_sorted(t: &MoltValue) -> MoltValue {
    let mut items = molt_iter(t);
    items.sort_by(molt_numeric_cmp);
    MoltValue::List(items)
}

/// Python `reversed(t)`.
pub fn molt_reversed(t: &MoltValue) -> MoltValue {
    let mut items = molt_iter(t);
    items.reverse();
    MoltValue::List(items)
}

/// Python `sum(t)`.
pub fn molt_sum(t: &MoltValue) -> MoltValue {
    molt_iter(t).into_iter().fold(MoltValue::Int(0), molt_add)
}

/// Python `any(t)`.
pub fn molt_any(t: &MoltValue) -> bool {
    molt_iter(t).iter().any(molt_bool)
}

/// Python `all(t)`.
pub fn molt_all(t: &MoltValue) -> bool {
    molt_iter(t).iter().all(molt_bool)
}

/// Python `abs(x)`.
pub fn molt_abs(x: MoltValue) -> MoltValue {
    match x {
        MoltValue::Int(i) => MoltValue::Int(i.wrapping_abs()),
        MoltValue::Float(f) => MoltValue::Float(f.abs()),
        _ => MoltValue::None,
    }
}

/// Python `min(a, b)`.
pub fn molt_min(a: MoltValue, b: MoltValue) -> MoltValue {
    if molt_le(&a, &b) {
        a
    } else {
        b
    }
}

/// Python `max(a, b)`.
pub fn molt_max(a: MoltValue, b: MoltValue) -> MoltValue {
    if molt_ge(&a, &b) {
        a
    } else {
        b
    }
}

/// Python `chr(x)`.
pub fn molt_chr(x: &MoltValue) -> MoltValue {
    let i = molt_int(x);
    char::from_u32(i as u32)
        .map(|c| MoltValue::Str(c.to_string()))
        .unwrap_or(MoltValue::None)
}

/// Python `ord(x)`.
pub fn molt_ord(x: &MoltValue) -> MoltValue {
    if let MoltValue::Str(s) = x {
        s.chars()
            .next()
            .map(|c| MoltValue::Int(c as i64))
            .unwrap_or(MoltValue::None)
    } else {
        MoltValue::None
    }
}

// ─── Dict helpers ─────────────────────────────────────────────────────────────

/// Python `dict.keys()`.
pub fn molt_dict_keys(d: &MoltValue) -> MoltValue {
    if let MoltValue::Dict(pairs) = d {
        MoltValue::List(pairs.iter().map(|(k, _)| k.clone()).collect())
    } else {
        MoltValue::List(vec![])
    }
}

/// Python `dict.values()`.
pub fn molt_dict_values(d: &MoltValue) -> MoltValue {
    if let MoltValue::Dict(pairs) = d {
        MoltValue::List(pairs.iter().map(|(_, v)| v.clone()).collect())
    } else {
        MoltValue::List(vec![])
    }
}

/// Python `dict.items()`.
pub fn molt_dict_items(d: &MoltValue) -> MoltValue {
    if let MoltValue::Dict(pairs) = d {
        MoltValue::List(
            pairs
                .iter()
                .map(|(k, v)| MoltValue::List(vec![k.clone(), v.clone()]))
                .collect(),
        )
    } else {
        MoltValue::List(vec![])
    }
}

// ─── map / filter ─────────────────────────────────────────────────────────────

/// Python `map(f, t)`.
pub fn molt_map(f: &MoltValue, t: &MoltValue) -> MoltValue {
    if let MoltValue::Func(func) = f {
        MoltValue::List(molt_iter(t).into_iter().map(|x| func(vec![x])).collect())
    } else {
        MoltValue::List(vec![])
    }
}

/// Python `filter(f, t)`.
pub fn molt_filter(f: &MoltValue, t: &MoltValue) -> MoltValue {
    if let MoltValue::Func(func) = f {
        MoltValue::List(
            molt_iter(t)
                .into_iter()
                .filter(|x| molt_bool(&func(vec![x.clone()])))
                .collect(),
        )
    } else {
        MoltValue::List(vec![])
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bool_truthiness() {
        assert!(!molt_bool(&MoltValue::None));
        assert!(!molt_bool(&MoltValue::Bool(false)));
        assert!(!molt_bool(&MoltValue::Int(0)));
        assert!(!molt_bool(&MoltValue::Float(0.0)));
        assert!(!molt_bool(&MoltValue::Str(String::new())));
        assert!(!molt_bool(&MoltValue::List(vec![])));
        assert!(!molt_bool(&MoltValue::Dict(vec![])));
        assert!(molt_bool(&MoltValue::Bool(true)));
        assert!(molt_bool(&MoltValue::Int(1)));
        assert!(molt_bool(&MoltValue::Str("x".to_string())));
    }

    #[test]
    fn test_arithmetic() {
        assert_eq!(
            molt_add(MoltValue::Int(3), MoltValue::Int(4)),
            MoltValue::Int(7)
        );
        assert_eq!(
            molt_mul(MoltValue::Int(6), MoltValue::Int(7)),
            MoltValue::Int(42)
        );
        assert_eq!(
            molt_sub(MoltValue::Int(10), MoltValue::Int(3)),
            MoltValue::Int(7)
        );
        assert_eq!(
            molt_floor_div(MoltValue::Int(7), MoltValue::Int(2)),
            MoltValue::Int(3)
        );
        assert_eq!(
            molt_mod(MoltValue::Int(7), MoltValue::Int(3)),
            MoltValue::Int(1)
        );
        assert_eq!(
            molt_pow(MoltValue::Int(2), MoltValue::Int(10)),
            MoltValue::Int(1024)
        );
    }

    #[test]
    fn test_str_conversion() {
        assert_eq!(molt_str(&MoltValue::None), "None");
        assert_eq!(molt_str(&MoltValue::Bool(true)), "True");
        assert_eq!(molt_str(&MoltValue::Bool(false)), "False");
        assert_eq!(molt_str(&MoltValue::Int(42)), "42");
        assert_eq!(molt_str(&MoltValue::Str("hello".to_string())), "hello");
    }

    #[test]
    fn test_list_ops() {
        let mut l = MoltValue::List(vec![MoltValue::Int(1), MoltValue::Int(2)]);
        molt_list_append(&mut l, MoltValue::Int(3));
        assert_eq!(molt_len(&l), MoltValue::Int(3));
        assert_eq!(molt_get_item(&l, &MoltValue::Int(2)), MoltValue::Int(3));
        assert_eq!(molt_get_item(&l, &MoltValue::Int(-1)), MoltValue::Int(3));
    }

    #[test]
    fn test_dict_ops() {
        let mut d = MoltValue::Dict(vec![]);
        molt_set_item(&mut d, MoltValue::Str("x".to_string()), MoltValue::Int(42));
        assert_eq!(
            molt_get_item(&d, &MoltValue::Str("x".to_string())),
            MoltValue::Int(42)
        );
        molt_set_item(&mut d, MoltValue::Str("x".to_string()), MoltValue::Int(99));
        assert_eq!(
            molt_get_item(&d, &MoltValue::Str("x".to_string())),
            MoltValue::Int(99)
        );
    }

    #[test]
    fn test_class_merge_layout_merges_offsets_and_layout_size() {
        let mut class_obj = MoltValue::Dict(vec![]);
        let result = molt_class_merge_layout(
            &mut class_obj,
            MoltValue::Dict(vec![
                (MoltValue::Str("x".to_string()), MoltValue::Int(0)),
                (MoltValue::Str("y".to_string()), MoltValue::Int(8)),
            ]),
            MoltValue::Int(8),
        );
        assert_eq!(result, MoltValue::None);
        assert_eq!(
            molt_get_item(
                &class_obj,
                &MoltValue::Str("__molt_layout_size__".to_string()),
            ),
            MoltValue::Int(24)
        );
        assert_eq!(
            molt_get_item(
                &class_obj,
                &MoltValue::Str("__molt_field_offsets__".to_string()),
            ),
            MoltValue::Dict(vec![
                (MoltValue::Str("x".to_string()), MoltValue::Int(0)),
                (MoltValue::Str("y".to_string()), MoltValue::Int(8)),
            ])
        );
    }

    #[test]
    fn test_range() {
        let r = molt_range(0, 5, 1);
        assert_eq!(molt_len(&r), MoltValue::Int(5));
        assert_eq!(molt_get_item(&r, &MoltValue::Int(4)), MoltValue::Int(4));
    }

    #[test]
    fn test_comparisons() {
        assert!(molt_lt(&MoltValue::Int(1), &MoltValue::Int(2)));
        assert!(molt_le(&MoltValue::Int(2), &MoltValue::Int(2)));
        assert!(molt_gt(&MoltValue::Int(3), &MoltValue::Int(2)));
        assert!(molt_ge(&MoltValue::Int(2), &MoltValue::Int(2)));
        assert!(molt_eq(&MoltValue::Int(42), &MoltValue::Int(42)));
        assert!(molt_ne(&MoltValue::Int(1), &MoltValue::Int(2)));
        // int/float cross comparison
        assert!(molt_eq(&MoltValue::Int(2), &MoltValue::Float(2.0)));
    }

    #[test]
    fn test_enumerate_zip() {
        let l = MoltValue::List(vec![MoltValue::Int(10), MoltValue::Int(20)]);
        let en = molt_enumerate(&l, 0);
        if let MoltValue::List(items) = &en {
            assert_eq!(items.len(), 2);
            assert_eq!(
                items[0],
                MoltValue::List(vec![MoltValue::Int(0), MoltValue::Int(10)])
            );
        }
        let a = MoltValue::List(vec![MoltValue::Int(1), MoltValue::Int(2)]);
        let b = MoltValue::List(vec![MoltValue::Int(3), MoltValue::Int(4)]);
        let z = molt_zip(&a, &b);
        assert_eq!(molt_len(&z), MoltValue::Int(2));
    }

    #[test]
    fn test_sum_any_all() {
        let l = MoltValue::List(vec![
            MoltValue::Int(1),
            MoltValue::Int(2),
            MoltValue::Int(3),
        ]);
        assert_eq!(molt_sum(&l), MoltValue::Int(6));
        assert!(molt_any(&l));
        assert!(molt_all(&l));
        let empty = MoltValue::List(vec![]);
        assert!(!molt_any(&empty));
        assert!(molt_all(&empty)); // Python: all([]) == True
    }
}
