#![allow(dead_code, unused_imports)]
// === FILE: runtime/molt-runtime/src/builtins/argparse.rs ===
//! Intrinsics for the `argparse` stdlib module.
//!
//! Implements a handle-based ArgumentParser that mirrors CPython's argparse.ArgumentParser
//! public API.  All heavy parsing work happens in Rust; the Python stdlib shim
//! (`src/molt/stdlib/argparse.py`) just calls these intrinsics.
//!
//! Action support: store, store_const, store_true, store_false, append, count, help, version.
//! nargs support: None (one), '?', '*', '+', N (integer), REMAINDER.

use crate::{
    MoltObject, PyToken, alloc_dict_with_pairs, alloc_list, alloc_string, alloc_tuple,
    dec_ref_bits, is_truthy, obj_from_bits, raise_exception, string_obj_to_owned, to_i64,
    type_name,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write as FmtWrite;
use std::sync::atomic::{AtomicI64, Ordering};

// ---------------------------------------------------------------------------
// Handle ID counter
// ---------------------------------------------------------------------------

static NEXT_HANDLE_ID: AtomicI64 = AtomicI64::new(1);

fn next_handle_id() -> i64 {
    NEXT_HANDLE_ID.fetch_add(1, Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// Data model
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
enum Action {
    Store,
    StoreConst,
    StoreTrue,
    StoreFalse,
    Append,
    Count,
    Help,
    Version,
}

impl Action {
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "store" => Some(Self::Store),
            "store_const" => Some(Self::StoreConst),
            "store_true" => Some(Self::StoreTrue),
            "store_false" => Some(Self::StoreFalse),
            "append" => Some(Self::Append),
            "count" => Some(Self::Count),
            "help" => Some(Self::Help),
            "version" => Some(Self::Version),
            _ => None,
        }
    }
}

/// `nargs` specification.
#[derive(Clone, Debug)]
enum NArgs {
    /// Exactly one value (default).
    One,
    /// `?` — zero or one.
    Optional,
    /// `*` — zero or more.
    ZeroOrMore,
    /// `+` — one or more.
    OneOrMore,
    /// Exact count N.
    Exact(usize),
    /// REMAINDER — everything that is left.
    Remainder,
}

impl NArgs {
    fn from_str_or_int(s: Option<&str>, n: Option<i64>) -> Self {
        if let Some(k) = n.filter(|&k| k >= 0) {
            return Self::Exact(k as usize);
        }
        match s {
            Some("?") => Self::Optional,
            Some("*") => Self::ZeroOrMore,
            Some("+") => Self::OneOrMore,
            Some("A...") | Some("REMAINDER") => Self::Remainder,
            _ => Self::One,
        }
    }
}

/// One argument definition.
#[derive(Clone, Debug)]
struct ArgDef {
    /// Long flag name like `--foo` or positional name `foo`.
    name: String,
    /// Short flag name like `-f`, if any.
    short: Option<String>,
    /// `dest` key in the result namespace.
    dest: String,
    nargs: NArgs,
    default: Option<String>,
    const_val: Option<String>,
    type_name: Option<String>, // "int" | "float" | "str" | None
    help: Option<String>,
    required: bool,
    action: Action,
    choices: Vec<String>,
    /// True for positional (no leading `-`).
    positional: bool,
    /// Which mutually-exclusive group this belongs to (-1 = none).
    mutex_group: i64,
}

impl ArgDef {
    fn is_flag(&self) -> bool {
        !self.positional
    }
}

/// A mutually-exclusive group.
#[derive(Clone, Debug)]
struct MutexGroup {
    required: bool,
    args: Vec<usize>, // indices into ParserState.args
}

/// A sub-command group (add_subparsers).
#[derive(Clone, Debug)]
struct SubGroup {
    title: String,
    dest: Option<String>,
    /// sub-command name → sub-parser handle id
    parsers: HashMap<String, i64>,
}

/// Per-parser state.
struct ParserState {
    prog: Option<String>,
    description: Option<String>,
    epilog: Option<String>,
    args: Vec<ArgDef>,
    mutex_groups: Vec<MutexGroup>,
    sub_group: Option<SubGroup>,
    add_help: bool,
}

impl ParserState {
    fn new(prog: Option<String>, description: Option<String>, epilog: Option<String>) -> Self {
        Self {
            prog,
            description,
            epilog,
            args: Vec::new(),
            mutex_groups: Vec::new(),
            sub_group: None,
            add_help: true,
        }
    }

    /// Derive dest from a flag name: strip leading `--` or `-`, replace `-` with `_`.
    fn derive_dest(name: &str) -> String {
        let stripped = name.trim_start_matches('-');
        stripped.replace('-', "_")
    }

    fn add_argument(&mut self, def: ArgDef) {
        self.args.push(def);
    }

    /// Format usage line.
    fn usage(&self) -> String {
        let prog = self.prog.as_deref().unwrap_or("program");
        let mut parts: Vec<String> = Vec::new();
        for a in &self.args {
            if matches!(a.action, Action::Help) {
                parts.push("[-h]".to_string());
                continue;
            }
            if a.positional {
                let nargs_str = match &a.nargs {
                    NArgs::One => a.dest.clone(),
                    NArgs::Optional => format!("[{}]", a.dest),
                    NArgs::ZeroOrMore => format!("[{} ...]", a.dest),
                    NArgs::OneOrMore => format!("{} [...]", a.dest),
                    NArgs::Exact(n) => (0..*n)
                        .map(|_| a.dest.clone())
                        .collect::<Vec<_>>()
                        .join(" "),
                    NArgs::Remainder => format!("[{} ...]", a.dest),
                };
                parts.push(if a.required {
                    nargs_str.clone()
                } else {
                    format!("[{nargs_str}]")
                });
            } else {
                let flag = a.short.as_deref().unwrap_or(&a.name);
                let value_placeholder = match &a.nargs {
                    NArgs::One => format!(" {}", a.dest.to_uppercase()),
                    _ => format!(" [{}]", a.dest.to_uppercase()),
                };
                let seg = match a.action {
                    Action::StoreTrue
                    | Action::StoreFalse
                    | Action::Count
                    | Action::Help
                    | Action::Version => flag.to_string(),
                    _ => format!("{flag}{value_placeholder}"),
                };
                parts.push(if a.required { seg } else { format!("[{seg}]") });
            }
        }
        format!("usage: {prog} {}", parts.join(" "))
    }

    /// Format the full help text.
    fn help(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "{}", self.usage());
        if let Some(desc) = &self.description {
            let _ = writeln!(out, "\n{desc}");
        }
        // Positional arguments section
        let positionals: Vec<&ArgDef> = self.args.iter().filter(|a| a.positional).collect();
        if !positionals.is_empty() {
            let _ = writeln!(out, "\npositional arguments:");
            for a in positionals {
                let help = a.help.as_deref().unwrap_or("");
                let _ = writeln!(out, "  {:<20} {help}", a.dest);
            }
        }
        // Optional arguments section
        let optionals: Vec<&ArgDef> = self.args.iter().filter(|a| !a.positional).collect();
        if !optionals.is_empty() {
            let _ = writeln!(out, "\noptions:");
            for a in optionals {
                let flag_str = if let Some(short) = &a.short {
                    format!("{short}, {}", a.name)
                } else {
                    a.name.clone()
                };
                let metavar = match a.action {
                    Action::StoreTrue
                    | Action::StoreFalse
                    | Action::Count
                    | Action::Help
                    | Action::Version => String::new(),
                    _ => format!(" {}", a.dest.to_uppercase()),
                };
                let help = a.help.as_deref().unwrap_or("");
                let mut help_full = help.to_string();
                if let Some(def) = &a.default {
                    help_full = format!("{help_full} (default: {def})");
                }
                let _ = writeln!(out, "  {flag_str}{metavar:<16} {help_full}");
            }
        }
        if let Some(epilog) = &self.epilog {
            let _ = writeln!(out, "\n{epilog}");
        }
        out
    }

    /// Parse a list of string arguments and return a flat map of dest → value_str | list.
    ///
    /// On success returns `Ok(HashMap<String, ParsedVal>)`.
    /// On error returns `Err(error_message)`.
    fn parse_args_vec(&self, raw: &[String]) -> Result<HashMap<String, ParsedVal>, String> {
        let mut result: HashMap<String, ParsedVal> = HashMap::new();

        // Install defaults
        for a in &self.args {
            let default = match a.action {
                Action::StoreTrue => ParsedVal::Bool(false),
                Action::StoreFalse => ParsedVal::Bool(true),
                Action::Count => ParsedVal::Int(0),
                Action::Append => ParsedVal::List(Vec::new()),
                _ => {
                    if let Some(def) = &a.default {
                        ParsedVal::Str(def.clone())
                    } else {
                        ParsedVal::None
                    }
                }
            };
            result.insert(a.dest.clone(), default);
        }

        let mut i = 0usize;
        let mut positional_idx = 0usize;

        // Collect positionals in order
        let positionals: Vec<usize> = self
            .args
            .iter()
            .enumerate()
            .filter(|(_, a)| a.positional)
            .map(|(i, _)| i)
            .collect();
        let mut option_index: HashMap<&str, usize> = HashMap::new();
        for (idx, arg) in self.args.iter().enumerate() {
            if arg.positional {
                continue;
            }
            option_index.entry(arg.name.as_str()).or_insert(idx);
            if let Some(short) = arg.short.as_deref() {
                option_index.entry(short).or_insert(idx);
            }
        }

        while i < raw.len() {
            let tok = &raw[i];

            if tok == "--" {
                // Everything after -- is positional
                i += 1;
                while i < raw.len() && positional_idx < positionals.len() {
                    let def = &self.args[positionals[positional_idx]];
                    result.insert(def.dest.clone(), ParsedVal::Str(raw[i].clone()));
                    positional_idx += 1;
                    i += 1;
                }
                break;
            }

            // Does this look like a flag?
            if tok.starts_with('-') && tok.len() > 1 {
                // Find matching ArgDef in O(1) average time.
                let def_idx = option_index.get(tok.as_str()).copied();

                let Some(idx) = def_idx else {
                    // Try --flag=value form
                    if let Some(eq_pos) = tok.find('=') {
                        let flag = &tok[..eq_pos];
                        let val = tok[eq_pos + 1..].to_string();
                        let def_idx2 = option_index.get(flag).copied();
                        if let Some(idx2) = def_idx2 {
                            let def = &self.args[idx2];
                            apply_flag_value(&mut result, def, Some(&val))?;
                            i += 1;
                            continue;
                        }
                    }
                    return Err(format!("unrecognized arguments: {tok}"));
                };

                let def = &self.args[idx];
                match def.action {
                    Action::StoreTrue => {
                        result.insert(def.dest.clone(), ParsedVal::Bool(true));
                        i += 1;
                    }
                    Action::StoreFalse => {
                        result.insert(def.dest.clone(), ParsedVal::Bool(false));
                        i += 1;
                    }
                    Action::Count => {
                        let cur = match result.get(&def.dest) {
                            Some(ParsedVal::Int(n)) => *n,
                            _ => 0,
                        };
                        result.insert(def.dest.clone(), ParsedVal::Int(cur + 1));
                        i += 1;
                    }
                    Action::StoreConst => {
                        let val = def.const_val.clone().unwrap_or_default();
                        result.insert(def.dest.clone(), ParsedVal::Str(val));
                        i += 1;
                    }
                    Action::Help | Action::Version => {
                        i += 1;
                        // Return a sentinel so the caller knows to print help/version and exit.
                        result.insert(
                            "__action__".to_string(),
                            ParsedVal::Str(format!("{:?}", def.action)),
                        );
                    }
                    Action::Store | Action::Append => {
                        // Consume value(s) according to nargs
                        i += 1;
                        let values = consume_nargs(raw, &mut i, &def.nargs)?;
                        if !def.choices.is_empty() {
                            for v in &values {
                                if !def.choices.contains(v) {
                                    return Err(format!(
                                        "argument {}: invalid choice: {} (choose from {})",
                                        def.name,
                                        v,
                                        def.choices.join(", ")
                                    ));
                                }
                            }
                        }
                        apply_flag_value_multi(&mut result, def, values)?;
                    }
                }
            } else {
                // Positional
                if positional_idx >= positionals.len() {
                    // Could be a sub-command
                    if let Some(sg) = &self.sub_group {
                        if let Some(sub_id) = sg.parsers.get(tok.as_str()) {
                            if let Some(dest) = &sg.dest {
                                result.insert(dest.clone(), ParsedVal::Str(tok.clone()));
                            }
                            result.insert("__sub_id__".to_string(), ParsedVal::Int(*sub_id));
                        } else {
                            return Err(format!("invalid choice: {tok}"));
                        }
                        i += 1;
                        continue;
                    }
                    return Err(format!("unrecognized arguments: {tok}"));
                }
                let def = &self.args[positionals[positional_idx]];
                result.insert(def.dest.clone(), ParsedVal::Str(tok.clone()));
                positional_idx += 1;
                i += 1;
            }
        }

        // Validate required arguments
        for a in &self.args {
            if a.required {
                match result.get(&a.dest) {
                    None | Some(ParsedVal::None) => {
                        return Err(format!("required argument missing: {}", a.name));
                    }
                    _ => {}
                }
            }
        }

        Ok(result)
    }
}

// ---------------------------------------------------------------------------
// Parsing helpers
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum ParsedVal {
    None,
    Str(String),
    Int(i64),
    Bool(bool),
    List(Vec<String>),
}

/// Consume values from `raw[*i..]` according to nargs, advancing `*i`.
fn consume_nargs(raw: &[String], i: &mut usize, nargs: &NArgs) -> Result<Vec<String>, String> {
    match nargs {
        NArgs::One => {
            if *i >= raw.len() {
                return Err("expected one argument".to_string());
            }
            let val = raw[*i].clone();
            *i += 1;
            Ok(vec![val])
        }
        NArgs::Optional => {
            if *i < raw.len() && !raw[*i].starts_with('-') {
                let val = raw[*i].clone();
                *i += 1;
                Ok(vec![val])
            } else {
                Ok(vec![])
            }
        }
        NArgs::ZeroOrMore => {
            let mut vals = Vec::new();
            while *i < raw.len() && !raw[*i].starts_with('-') {
                vals.push(raw[*i].clone());
                *i += 1;
            }
            Ok(vals)
        }
        NArgs::OneOrMore => {
            if *i >= raw.len() || raw[*i].starts_with('-') {
                return Err("expected at least one argument".to_string());
            }
            let mut vals = Vec::new();
            while *i < raw.len() && !raw[*i].starts_with('-') {
                vals.push(raw[*i].clone());
                *i += 1;
            }
            Ok(vals)
        }
        NArgs::Exact(n) => {
            let mut vals = Vec::new();
            for _ in 0..*n {
                if *i >= raw.len() {
                    return Err(format!("expected {n} arguments"));
                }
                vals.push(raw[*i].clone());
                *i += 1;
            }
            Ok(vals)
        }
        NArgs::Remainder => {
            let vals: Vec<String> = raw[*i..].to_vec();
            *i = raw.len();
            Ok(vals)
        }
    }
}

fn apply_flag_value(
    result: &mut HashMap<String, ParsedVal>,
    def: &ArgDef,
    val: Option<&str>,
) -> Result<(), String> {
    let v = val.unwrap_or_default().to_string();
    result.insert(def.dest.clone(), ParsedVal::Str(v));
    Ok(())
}

fn apply_flag_value_multi(
    result: &mut HashMap<String, ParsedVal>,
    def: &ArgDef,
    values: Vec<String>,
) -> Result<(), String> {
    match def.action {
        Action::Append => {
            let cur = match result.remove(&def.dest) {
                Some(ParsedVal::List(mut v)) => {
                    v.extend(values);
                    v
                }
                _ => values,
            };
            result.insert(def.dest.clone(), ParsedVal::List(cur));
        }
        _ => {
            if values.len() == 1 {
                result.insert(
                    def.dest.clone(),
                    ParsedVal::Str(values.into_iter().next().unwrap()),
                );
            } else {
                result.insert(def.dest.clone(), ParsedVal::List(values));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Thread-local handle registry
// ---------------------------------------------------------------------------

thread_local! {
    static PARSER_HANDLES: RefCell<HashMap<i64, ParserState>> = RefCell::new(HashMap::new());
    static GROUP_HANDLES: RefCell<HashMap<i64, (i64, usize)>> = RefCell::new(HashMap::new());
    // Maps group_handle_id -> (parser_handle_id, mutex_group_idx)
}

// ---------------------------------------------------------------------------
// Object model helpers
// ---------------------------------------------------------------------------

fn alloc_str_or_err(_py: &PyToken<'_>, s: &str) -> Result<u64, u64> {
    let ptr = alloc_string(_py, s.as_bytes());
    if ptr.is_null() {
        Err(raise_exception::<u64>(_py, "MemoryError", "out of memory"))
    } else {
        Ok(MoltObject::from_ptr(ptr).bits())
    }
}

/// Convert a `ParsedVal` to a MoltObject bits value.
fn parsed_val_to_bits(_py: &PyToken<'_>, val: &ParsedVal) -> u64 {
    match val {
        ParsedVal::None => MoltObject::none().bits(),
        ParsedVal::Bool(b) => MoltObject::from_bool(*b).bits(),
        ParsedVal::Int(n) => MoltObject::from_int(*n).bits(),
        ParsedVal::Str(s) => {
            let ptr = alloc_string(_py, s.as_bytes());
            if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            }
        }
        ParsedVal::List(items) => {
            let mut bits: Vec<u64> = Vec::with_capacity(items.len());
            for item in items {
                let ptr = alloc_string(_py, item.as_bytes());
                let b = if ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(ptr).bits()
                };
                bits.push(b);
            }
            let list_ptr = alloc_list(_py, &bits);
            for b in &bits {
                dec_ref_bits(_py, *b);
            }
            if list_ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(list_ptr).bits()
            }
        }
    }
}

/// Convert a HashMap result to a Python dict.
fn result_to_dict(_py: &PyToken<'_>, map: HashMap<String, ParsedVal>) -> u64 {
    let mut pairs: Vec<u64> = Vec::with_capacity(map.len() * 2);
    for (k, v) in &map {
        let k_ptr = alloc_string(_py, k.as_bytes());
        if k_ptr.is_null() {
            for b in &pairs {
                dec_ref_bits(_py, *b);
            }
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        let v_bits = parsed_val_to_bits(_py, v);
        pairs.push(MoltObject::from_ptr(k_ptr).bits());
        pairs.push(v_bits);
    }
    let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
    for b in &pairs {
        dec_ref_bits(_py, *b);
    }
    if dict_ptr.is_null() {
        raise_exception::<u64>(_py, "MemoryError", "out of memory")
    } else {
        MoltObject::from_ptr(dict_ptr).bits()
    }
}

/// Extract an optional string from bits (None → None).
fn opt_str(bits: u64) -> Option<String> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        None
    } else {
        string_obj_to_owned(obj)
    }
}

/// Extract a list of strings from a Python list-of-str object.
fn list_to_string_vec(_py: &PyToken<'_>, bits: u64) -> Result<Vec<String>, u64> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return Ok(Vec::new());
    }
    let Some(ptr) = obj.as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "expected list of str",
        ));
    };
    let items: Vec<u64> = unsafe { crate::seq_vec_ref(ptr).to_vec() };
    let mut out = Vec::with_capacity(items.len());
    for item_bits in items {
        match string_obj_to_owned(obj_from_bits(item_bits)) {
            Some(s) => out.push(s),
            None => {
                return Err(raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "expected list of str",
                ));
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Public FFI — ArgumentParser
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn molt_argparse_parser_new(
    prog_bits: u64,
    description_bits: u64,
    epilog_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let prog = opt_str(prog_bits);
        let description = opt_str(description_bits);
        let epilog = opt_str(epilog_bits);
        let id = next_handle_id();
        PARSER_HANDLES.with(|map| {
            map.borrow_mut()
                .insert(id, ParserState::new(prog, description, epilog));
        });
        MoltObject::from_int(id).bits()
    })
}

/// Add an argument to a parser.
///
/// Parameters (positional, 10 total):
///   handle_bits, name_bits, nargs_bits, default_bits, type_bits,
///   help_bits, required_bits, action_bits, dest_bits, choices_bits
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn molt_argparse_add_argument(
    handle_bits: u64,
    name_bits: u64,
    nargs_bits: u64,
    default_bits: u64,
    type_bits: u64,
    help_bits: u64,
    required_bits: u64,
    action_bits: u64,
    dest_bits: u64,
    choices_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid argparse handle");
        };
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "argument name must be str");
        };

        let positional = !name.starts_with('-');
        let dest = opt_str(dest_bits).unwrap_or_else(|| ParserState::derive_dest(&name));

        // Parse nargs
        let nargs_obj = obj_from_bits(nargs_bits);
        let nargs = if nargs_obj.is_none() {
            NArgs::One
        } else if let Some(s) = string_obj_to_owned(nargs_obj) {
            NArgs::from_str_or_int(Some(&s), None)
        } else if let Some(n) = to_i64(nargs_obj) {
            NArgs::from_str_or_int(None, Some(n))
        } else {
            NArgs::One
        };

        let default = opt_str(default_bits);
        let type_name_str = opt_str(type_bits);
        let help = opt_str(help_bits);
        let required = if positional {
            // Positionals are required by default unless nargs is ?, *, REMAINDER
            !matches!(
                nargs,
                NArgs::Optional | NArgs::ZeroOrMore | NArgs::Remainder
            )
        } else {
            is_truthy(_py, obj_from_bits(required_bits))
        };

        let action_str = opt_str(action_bits);
        let action = action_str
            .as_deref()
            .and_then(Action::from_str)
            .unwrap_or(Action::Store);

        let choices = list_to_string_vec(_py, choices_bits).unwrap_or_default();

        // Determine short flag vs long flag
        let (short, real_name) = if name.starts_with("--") {
            (None, name.clone())
        } else if name.starts_with('-') && name.len() == 2 {
            (Some(name.clone()), name.clone())
        } else {
            (None, name.clone())
        };

        let def = ArgDef {
            name: real_name,
            short,
            dest,
            nargs,
            default,
            const_val: None,
            type_name: type_name_str,
            help,
            required,
            action,
            choices,
            positional,
            mutex_group: -1,
        };

        let ok = PARSER_HANDLES.with(|map| {
            let mut borrow = map.borrow_mut();
            if let Some(state) = borrow.get_mut(&id) {
                state.add_argument(def);
                true
            } else {
                false
            }
        });

        if ok {
            MoltObject::none().bits()
        } else {
            raise_exception::<u64>(_py, "ValueError", "argparse handle not found")
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_argparse_parse_args(handle_bits: u64, args_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid argparse handle");
        };

        let raw = match list_to_string_vec(_py, args_bits) {
            Ok(v) => v,
            Err(bits) => return bits,
        };

        let result = PARSER_HANDLES.with(|map| {
            let borrow = map.borrow();
            borrow.get(&id).map(|state| state.parse_args_vec(&raw))
        });

        let Some(parse_result) = result else {
            return raise_exception::<u64>(_py, "ValueError", "argparse handle not found");
        };

        match parse_result {
            Ok(map) => result_to_dict(_py, map),
            Err(msg) => raise_exception::<u64>(_py, "SystemExit", &msg),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_argparse_format_help(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid argparse handle");
        };
        let help = PARSER_HANDLES.with(|map| map.borrow().get(&id).map(|s| s.help()));
        let Some(text) = help else {
            return raise_exception::<u64>(_py, "ValueError", "argparse handle not found");
        };
        match alloc_str_or_err(_py, &text) {
            Ok(b) => b,
            Err(b) => b,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_argparse_format_usage(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid argparse handle");
        };
        let usage = PARSER_HANDLES.with(|map| map.borrow().get(&id).map(|s| s.usage()));
        let Some(text) = usage else {
            return raise_exception::<u64>(_py, "ValueError", "argparse handle not found");
        };
        match alloc_str_or_err(_py, &text) {
            Ok(b) => b,
            Err(b) => b,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_argparse_error(handle_bits: u64, message_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let _id = to_i64(obj_from_bits(handle_bits));
        let msg = string_obj_to_owned(obj_from_bits(message_bits))
            .unwrap_or_else(|| "argument error".to_string());
        raise_exception::<u64>(_py, "SystemExit", &msg)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_argparse_add_subparsers(
    handle_bits: u64,
    title_bits: u64,
    dest_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid argparse handle");
        };
        let title = string_obj_to_owned(obj_from_bits(title_bits))
            .unwrap_or_else(|| "subcommands".to_string());
        let dest = opt_str(dest_bits);

        let group_id = next_handle_id();
        let ok = PARSER_HANDLES.with(|map| {
            let mut borrow = map.borrow_mut();
            if let Some(state) = borrow.get_mut(&id) {
                state.sub_group = Some(SubGroup {
                    title,
                    dest,
                    parsers: HashMap::new(),
                });
                true
            } else {
                false
            }
        });
        if !ok {
            return raise_exception::<u64>(_py, "ValueError", "argparse handle not found");
        }
        // Encode (parser_id, "subgroup") in the group handle using a negative sentinel.
        GROUP_HANDLES.with(|map| {
            // We repurpose group_handles as: group_id -> (parser_id, usize::MAX)
            map.borrow_mut().insert(group_id, (id, usize::MAX));
        });
        MoltObject::from_int(group_id).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_argparse_add_parser(
    group_handle_bits: u64,
    name_bits: u64,
    help_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(group_id) = to_i64(obj_from_bits(group_handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid subparser group handle");
        };
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "subparser name must be str");
        };
        let help = string_obj_to_owned(obj_from_bits(help_bits));
        let _ = help; // stored in sub-parser description if desired

        // Look up which parser this group belongs to.
        let parent_id = GROUP_HANDLES.with(|map| map.borrow().get(&group_id).map(|(pid, _)| *pid));
        let Some(parent_id) = parent_id else {
            return raise_exception::<u64>(_py, "ValueError", "subparser group not found");
        };

        // Create a new sub-parser handle.
        let sub_id = next_handle_id();
        PARSER_HANDLES.with(|map| {
            map.borrow_mut()
                .insert(sub_id, ParserState::new(Some(name.clone()), None, None));
        });

        // Register in parent's sub_group.
        let ok = PARSER_HANDLES.with(|map| {
            let mut borrow = map.borrow_mut();
            if let Some(sg) = borrow
                .get_mut(&parent_id)
                .and_then(|state| state.sub_group.as_mut())
            {
                sg.parsers.insert(name, sub_id);
                return true;
            }
            false
        });
        if !ok {
            return raise_exception::<u64>(_py, "ValueError", "parent parser not found");
        }
        MoltObject::from_int(sub_id).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_argparse_add_mutually_exclusive(
    handle_bits: u64,
    required_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid argparse handle");
        };
        let required = is_truthy(_py, obj_from_bits(required_bits));
        let group_id = next_handle_id();
        let mutex_idx = PARSER_HANDLES.with(|map| {
            let mut borrow = map.borrow_mut();
            borrow.get_mut(&id).map(|state| {
                let idx = state.mutex_groups.len();
                state.mutex_groups.push(MutexGroup {
                    required,
                    args: Vec::new(),
                });
                idx
            })
        });
        let Some(idx) = mutex_idx else {
            return raise_exception::<u64>(_py, "ValueError", "argparse handle not found");
        };
        GROUP_HANDLES.with(|map| {
            map.borrow_mut().insert(group_id, (id, idx));
        });
        MoltObject::from_int(group_id).bits()
    })
}

/// Add an argument to a mutually-exclusive group.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn molt_argparse_group_add_argument(
    group_bits: u64,
    name_bits: u64,
    nargs_bits: u64,
    default_bits: u64,
    type_bits: u64,
    help_bits: u64,
    action_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(group_id) = to_i64(obj_from_bits(group_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid group handle");
        };
        let info = GROUP_HANDLES.with(|map| map.borrow().get(&group_id).copied());
        let Some((parser_id, mutex_idx)) = info else {
            return raise_exception::<u64>(_py, "ValueError", "group handle not found");
        };
        // For subparser groups (mutex_idx == usize::MAX), just delegate to add_argument.
        if mutex_idx == usize::MAX {
            return molt_argparse_add_argument(
                MoltObject::from_int(parser_id).bits(),
                name_bits,
                nargs_bits,
                default_bits,
                type_bits,
                help_bits,
                MoltObject::from_bool(false).bits(),
                action_bits,
                MoltObject::none().bits(),
                MoltObject::none().bits(),
            );
        }

        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "argument name must be str");
        };
        let positional = !name.starts_with('-');
        let dest = ParserState::derive_dest(&name);
        let nargs_obj = obj_from_bits(nargs_bits);
        let nargs = if nargs_obj.is_none() {
            NArgs::One
        } else if let Some(s) = string_obj_to_owned(nargs_obj) {
            NArgs::from_str_or_int(Some(&s), None)
        } else if let Some(n) = to_i64(nargs_obj) {
            NArgs::from_str_or_int(None, Some(n))
        } else {
            NArgs::One
        };
        let default = opt_str(default_bits);
        let type_name_str = opt_str(type_bits);
        let help = opt_str(help_bits);
        let action_str = opt_str(action_bits);
        let action = action_str
            .as_deref()
            .and_then(Action::from_str)
            .unwrap_or(Action::Store);

        let def = ArgDef {
            name: name.clone(),
            short: None,
            dest: dest.clone(),
            nargs,
            default,
            const_val: None,
            type_name: type_name_str,
            help,
            required: false,
            action,
            choices: Vec::new(),
            positional,
            mutex_group: mutex_idx as i64,
        };

        let ok = PARSER_HANDLES.with(|map| {
            let mut borrow = map.borrow_mut();
            if let Some(state) = borrow.get_mut(&parser_id) {
                let arg_idx = state.args.len();
                state.args.push(def);
                if mutex_idx < state.mutex_groups.len() {
                    state.mutex_groups[mutex_idx].args.push(arg_idx);
                }
                true
            } else {
                false
            }
        });

        if ok {
            MoltObject::none().bits()
        } else {
            raise_exception::<u64>(_py, "ValueError", "argparse handle not found")
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_argparse_parser_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Some(id) = to_i64(obj_from_bits(handle_bits)) {
            PARSER_HANDLES.with(|map| {
                map.borrow_mut().remove(&id);
            });
        }
        MoltObject::none().bits()
    })
}

// Suppress dead-code lint for opt_str / type_name used in various paths.
#[allow(dead_code)]
fn _type_name_use(_py: &PyToken<'_>, b: u64) {
    let _ = type_name(_py, obj_from_bits(b));
}

// Keep the alloc_tuple import used.
#[allow(dead_code)]
fn _alloc_tuple_use(_py: &PyToken<'_>) {
    let _ = alloc_tuple(_py, &[]);
}
