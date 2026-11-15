/// The TIR type system. Designed for progressive refinement:
/// values start as DynBox and get refined to concrete types.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize)]
pub enum TirType {
    // Semantic scalar families; exact provenance and Repr own physical storage.
    I64,
    F64,
    Bool,
    None,
    // Reference types
    Str,
    Bytes,
    List(Box<TirType>),
    Dict(Box<TirType>, Box<TirType>),
    Set(Box<TirType>),
    Tuple(Vec<TirType>),
    /// Iterator with a known yielded element type. This is a semantic iterator
    /// fact only; it does not prove any physical container storage layout.
    Iterator(Box<TirType>),
    // Boxed
    /// NaN-boxed with known inner type.
    Box(Box<TirType>),
    /// NaN-boxed, type unknown.
    DynBox,
    /// An instance whose class is exactly this class statement (I6,
    /// design 78 section 5.1). Only an allocation site produces it
    /// (`object_new_bound`, C1): an annotation admits subclasses and a
    /// constructor call may return another object, so neither does.
    /// NaN-boxed like `DynBox`. Two equal ids meet to themselves; distinct
    /// ids fall through to the Union / DynBox lattice.
    Class(crate::ClassId),
    // Callable
    Func(FuncSignature),
    // Special
    BigInt,
    Ptr(Box<TirType>),
    /// Union of up to 3 types; beyond that collapses to DynBox.
    Union(Vec<TirType>),
    /// Bottom type (unreachable).
    Never,
}

/// Function signature for `TirType::Func`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize)]
pub struct FuncSignature {
    pub params: Vec<TirType>,
    pub return_type: Box<TirType>,
}

impl TirType {
    /// Remove explicit storage wrappers without asserting exact builtin
    /// provenance or authorizing an unboxed physical representation.
    pub fn semantic_type(&self) -> &Self {
        let mut ty = self;
        while let Self::Box(inner) = ty {
            ty = inner;
        }
        ty
    }

    /// Lattice meet for SSA join points.
    ///
    /// Returns the most specific common supertype of `self` and `other`.
    /// If the types are identical, returns a clone. If incompatible scalars,
    /// produces a Union (up to 3 members) or collapses to DynBox.
    pub fn meet(&self, other: &TirType) -> TirType {
        if self == other {
            return self.clone();
        }

        // Never is the bottom — meet with anything yields the other.
        if matches!(self, TirType::Never) {
            return other.clone();
        }
        if matches!(other, TirType::Never) {
            return self.clone();
        }

        // DynBox absorbs everything.
        if matches!(self, TirType::DynBox) || matches!(other, TirType::DynBox) {
            return TirType::DynBox;
        }

        // Box(T) meet Box(U) = Box(meet(T, U))
        if let (TirType::Box(inner_a), TirType::Box(inner_b)) = (self, other) {
            return TirType::Box(Box::new(inner_a.meet(inner_b)));
        }

        // List(T) meet List(U) = List(meet(T, U))
        if let (TirType::List(a), TirType::List(b)) = (self, other) {
            return TirType::List(Box::new(a.meet(b)));
        }

        // Dict(K1,V1) meet Dict(K2,V2) = Dict(meet(K1,K2), meet(V1,V2))
        if let (TirType::Dict(k1, v1), TirType::Dict(k2, v2)) = (self, other) {
            return TirType::Dict(Box::new(k1.meet(k2)), Box::new(v1.meet(v2)));
        }

        // Set(T) meet Set(U) = Set(meet(T, U))
        if let (TirType::Set(a), TirType::Set(b)) = (self, other) {
            return TirType::Set(Box::new(a.meet(b)));
        }

        // Tuple meet: same arity → element-wise meet; different arity → Union/DynBox
        if let (TirType::Tuple(a), TirType::Tuple(b)) = (self, other)
            && a.len() == b.len()
        {
            let merged: Vec<TirType> = a.iter().zip(b.iter()).map(|(x, y)| x.meet(y)).collect();
            return TirType::Tuple(merged);
        }

        // Iterator(T) meet Iterator(U) = Iterator(meet(T, U)).
        if let (TirType::Iterator(a), TirType::Iterator(b)) = (self, other) {
            return TirType::Iterator(Box::new(a.meet(b)));
        }

        // Flatten unions when building the join.
        // Max possible size: 3 (self union) + 3 (other union) = 6, so this is bounded.
        let mut members = Vec::with_capacity(6);
        Self::collect_union_members(self, &mut members);
        Self::collect_union_members(other, &mut members);
        // Remove duplicates: since members are bounded at ≤6, a simple O(N²)
        // retain-based dedup is fine and avoids requiring Ord on TirType.
        let mut seen = Vec::with_capacity(6);
        members.retain(|m| {
            if seen.contains(m) {
                false
            } else {
                seen.push(m.clone());
                true
            }
        });

        if members.len() == 1 {
            return members.into_iter().next().unwrap();
        }
        if members.len() <= 3 {
            // Sort members for canonical ordering so that
            // I64.meet(&Str) == Str.meet(&I64) == Union([I64, Str]).
            // Uses Debug string as a stable ordering key since TirType
            // doesn't implement Ord (and shouldn't — it's a lattice, not a total order).
            members.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
            return TirType::Union(members);
        }
        TirType::DynBox
    }

    /// Flatten nested unions into a flat member list.
    /// Deduplication is handled by the caller via `dedup()` after collection,
    /// so we push unconditionally here — O(1) per element, no linear scan.
    fn collect_union_members(ty: &TirType, out: &mut Vec<TirType>) {
        match ty {
            TirType::Union(members) => {
                out.extend(members.iter().cloned());
            }
            _ => {
                out.push(ty.clone());
            }
        }
    }

    /// Returns true for types that live in machine registers (no heap allocation).
    pub fn is_unboxed(&self) -> bool {
        matches!(
            self,
            TirType::I64 | TirType::F64 | TirType::Bool | TirType::None
        )
    }

    /// Returns true for types that support arithmetic operations.
    pub fn is_numeric(&self) -> bool {
        matches!(self, TirType::I64 | TirType::F64 | TirType::Bool)
    }

    /// Map a frontend `_type_hint` attribute string to a `TirType`.
    ///
    /// This is the single source of truth for the SSA lift's type-
    /// refinement of result values: the frontend stores its
    /// inferred type as a string on the SimpleIR `type_hint` field
    /// (and the SSA lift round-trips it through the `_type_hint`
    /// attr at `tir/ssa.rs:1133`), and downstream type-refine wants
    /// to make decisions on a structured `TirType`.
    ///
    /// Builtin-tag mapping (these strings are produced by the
    /// frontend's `BUILTIN_TYPE_TAGS` set):
    ///   - `"int"`, `"bool"` → `I64` / `Bool` (canonical unboxed)
    ///   - `"float"` → `F64`
    ///   - `"str"`, `"bytes"` → `Str` / `Bytes`
    ///   - `"list"`, `"dict"`, `"set"`, `"tuple"` → containers with
    ///     `DynBox` members
    ///   - `"list[T]"`, `"dict[K, V]"`, `"set[T]"`, `"tuple[T, U]"` →
    ///     containers with recursively parsed member types.  Fixed-arity
    ///     tuple hints are represented exactly; variadic tuple hints defer to
    ///     `DynBox` until TIR has a first-class variadic tuple shape.
    ///   - `"None"`, `"NoneType"` → `None`
    ///   - `"BigInt"` → `BigInt`
    ///
    /// Unsupported compound hints fall back to `DynBox` for safety:
    ///   - `"Func:<symbol>"`, `"BoundMethod:<class>:<method>"`,
    ///     `"type"`, `"Any"`, `"Unknown"`, the empty string, and
    ///     anything containing punctuation that the simple-
    ///     identifier check rejects.
    ///
    /// A name is never class identity (HF-847 slice 1, D6): any other
    /// hint, a user class's name included, is `DynBox`. The exact
    /// `Class(id)` comes only from an allocation's class statement.
    pub fn from_type_hint(hint: &str) -> TirType {
        // Builtin-tag mapping — these match the frontend's
        // BUILTIN_TYPE_TAGS set 1:1.
        match hint {
            "int" => return TirType::I64,
            "float" => return TirType::F64,
            "bool" => return TirType::Bool,
            "str" => return TirType::Str,
            "bytes" => return TirType::Bytes,
            "list" => return TirType::List(Box::new(TirType::DynBox)),
            "dict" => {
                return TirType::Dict(Box::new(TirType::DynBox), Box::new(TirType::DynBox));
            }
            "set" => return TirType::Set(Box::new(TirType::DynBox)),
            "tuple" => return TirType::Tuple(Vec::new()),
            "None" | "NoneType" | "none" => return TirType::None,
            "BigInt" | "bigint" => return TirType::BigInt,
            "Any" | "Unknown" | "" | "type" => return TirType::DynBox,
            _ => {}
        }
        if let Some(parsed) = parse_compound_type_hint(hint) {
            return parsed;
        }
        // Callable/dynamic hints (Func:..., BoundMethod:...), unsupported
        // parameterized forms and every name are dynamic.
        TirType::DynBox
    }
}

fn parse_compound_type_hint(hint: &str) -> Option<TirType> {
    let (outer, inner) = split_outer_generic(hint)?;
    match outer {
        "list" | "List" => parse_single_generic_arg(inner)
            .map(|item| TirType::List(Box::new(item)))
            .or(Some(TirType::DynBox)),
        "set" | "Set" => parse_single_generic_arg(inner)
            .map(|item| TirType::Set(Box::new(item)))
            .or(Some(TirType::DynBox)),
        "dict" | "Dict" => {
            let parts = split_top_level_commas(inner);
            if parts.len() != 2 {
                return Some(TirType::DynBox);
            }
            let key = parse_type_arg(parts[0])?;
            let value = parse_type_arg(parts[1])?;
            Some(TirType::Dict(Box::new(key), Box::new(value)))
        }
        "tuple" | "Tuple" => {
            if inner.trim().is_empty() {
                return Some(TirType::DynBox);
            }
            let parts = split_top_level_commas(inner);
            if parts.contains(&"...") {
                return Some(TirType::DynBox);
            }
            let mut items = Vec::with_capacity(parts.len());
            for part in parts {
                items.push(parse_type_arg(part)?);
            }
            Some(TirType::Tuple(items))
        }
        _ => None,
    }
}

fn parse_single_generic_arg(inner: &str) -> Option<TirType> {
    let parts = split_top_level_commas(inner);
    if parts.len() == 1 {
        parse_type_arg(parts[0])
    } else {
        None
    }
}

fn parse_type_arg(arg: &str) -> Option<TirType> {
    let ty = TirType::from_type_hint(arg.trim());
    if matches!(ty, TirType::DynBox) {
        None
    } else {
        Some(ty)
    }
}

fn split_outer_generic(hint: &str) -> Option<(&str, &str)> {
    let open = hint.find('[')?;
    if !hint.ends_with(']') {
        return None;
    }
    let outer = hint[..open].trim();
    if outer.is_empty() {
        return None;
    }
    let inner = &hint[(open + 1)..(hint.len() - 1)];
    if !generic_brackets_balanced(inner) {
        return None;
    }
    Some((outer, inner))
}

fn generic_brackets_balanced(text: &str) -> bool {
    let mut depth = 0usize;
    for ch in text.chars() {
        match ch {
            '[' => depth += 1,
            ']' => {
                let Some(next_depth) = depth.checked_sub(1) else {
                    return false;
                };
                depth = next_depth;
            }
            _ => {}
        }
    }
    depth == 0
}

fn split_top_level_commas(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut depth = 0usize;
    for (idx, ch) in text.char_indices() {
        match ch {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(text[start..idx].trim());
                start = idx + ch.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(text[start..].trim());
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_type_removes_only_storage_wrappers() {
        for ty in [
            TirType::Bool,
            TirType::BigInt,
            TirType::Bytes,
            TirType::DynBox,
            class("class:m:Subclass"),
            TirType::List(Box::new(TirType::I64)),
        ] {
            let boxed = TirType::Box(Box::new(TirType::Box(Box::new(ty.clone()))));
            assert_eq!(boxed.semantic_type(), &ty);
            assert_eq!(ty.semantic_type(), &ty);
        }
    }

    #[test]
    fn meet_identical_types() {
        assert_eq!(TirType::I64.meet(&TirType::I64), TirType::I64);
    }

    #[test]
    fn meet_never_is_identity() {
        assert_eq!(TirType::Never.meet(&TirType::Str), TirType::Str);
        assert_eq!(TirType::F64.meet(&TirType::Never), TirType::F64);
    }

    #[test]
    fn meet_dynbox_absorbs() {
        assert_eq!(TirType::I64.meet(&TirType::DynBox), TirType::DynBox);
        assert_eq!(TirType::DynBox.meet(&TirType::Str), TirType::DynBox);
    }

    #[test]
    fn meet_different_scalars_produces_union() {
        let result = TirType::I64.meet(&TirType::Str);
        // Sorted canonically: "I64" < "Str" alphabetically
        assert_eq!(result, TirType::Union(vec![TirType::I64, TirType::Str]));
        // Verify commutativity: Str.meet(&I64) == I64.meet(&Str)
        assert_eq!(TirType::Str.meet(&TirType::I64), result);
    }

    #[test]
    fn meet_union_overflow_collapses_to_dynbox() {
        // Build a 3-member union, then meet with a 4th distinct type.
        let u3 = TirType::Union(vec![TirType::I64, TirType::F64, TirType::Str]);
        let result = u3.meet(&TirType::Bool);
        assert_eq!(result, TirType::DynBox);
    }

    #[test]
    fn meet_boxes_recurse() {
        let a = TirType::Box(Box::new(TirType::I64));
        let b = TirType::Box(Box::new(TirType::F64));
        let result = a.meet(&b);
        // After union canonicalization (M2 fix), members are sorted by Debug repr:
        // "F64" < "I64" alphabetically.
        assert_eq!(
            result,
            TirType::Box(Box::new(TirType::Union(vec![TirType::F64, TirType::I64])))
        );
    }

    #[test]
    fn meet_lists_recurse() {
        let a = TirType::List(Box::new(TirType::I64));
        let b = TirType::List(Box::new(TirType::I64));
        assert_eq!(a.meet(&b), TirType::List(Box::new(TirType::I64)));
    }

    #[test]
    fn meet_iterators_recurse() {
        let a = TirType::Iterator(Box::new(TirType::I64));
        let b = TirType::Iterator(Box::new(TirType::Str));
        assert_eq!(
            a.meet(&b),
            TirType::Iterator(Box::new(TirType::Union(vec![TirType::I64, TirType::Str])))
        );
    }

    #[test]
    fn is_unboxed_and_numeric() {
        assert!(TirType::I64.is_unboxed());
        assert!(TirType::Bool.is_numeric());
        assert!(!TirType::Str.is_unboxed());
        assert!(!TirType::Str.is_numeric());
        assert!(!TirType::DynBox.is_unboxed());
    }

    fn class(definition: &str) -> TirType {
        TirType::Class(crate::ClassId::of_definition(definition))
    }

    /// Same-id `Class` meets to itself through the `self == other` early
    /// return in `meet()`.
    #[test]
    fn meet_user_class_same_id_preserves() {
        let a = class("class:m:Point");
        let b = class("class:m:Point");
        assert_eq!(a.meet(&b), class("class:m:Point"));
    }

    /// Different class ids fall through to the Union/DynBox lattice: two
    /// classes form a 2-member union in a canonical order, whatever the
    /// operand order. Same-named classes of two modules stay distinct.
    #[test]
    fn meet_user_class_different_ids_unions() {
        let a = class("class:m:Point");
        let b = class("class:m:Line");
        let result = a.meet(&b);
        let TirType::Union(members) = &result else {
            panic!("distinct classes meet to a union: {result:?}");
        };
        assert_eq!(members.len(), 2);
        assert!(members.contains(&a) && members.contains(&b));
        assert_ne!(class("class:geometry:Point"), class("class:physics:Point"));
        // Commutativity guard.
        assert_eq!(b.meet(&a), result);
    }

    /// `Class` meet `DynBox` collapses to `DynBox` — the
    /// existing absorption rule applies.  Critical: a refined
    /// type joining a path that doesn't refine must lose
    /// precision, otherwise the type-refine pass could promote
    /// type-erased exception handler args from DynBox to a
    /// specific class and miscompile the catch site.
    #[test]
    fn meet_user_class_with_dynbox_collapses() {
        let cls = class("class:m:Point");
        assert_eq!(cls.meet(&TirType::DynBox), TirType::DynBox);
        assert_eq!(TirType::DynBox.meet(&cls), TirType::DynBox);
    }

    /// `Class` is **not unboxed** — instances are NaN-boxed
    /// today (Phase 5 step 3 stack-allocates the *backing*, but
    /// the value carried at the SSA level is still a tagged 64-bit
    /// pointer).  When direct stack-passable representation lands
    /// (analogous to Mojo's `@register_passable("trivial")`), this
    /// will flip — and `is_unboxed` must be revisited at every
    /// site that branches on it for register allocation choices.
    #[test]
    fn user_class_is_neither_unboxed_nor_numeric() {
        let cls = class("class:m:Point");
        assert!(!cls.is_unboxed());
        assert!(!cls.is_numeric());
    }

    /// Hash + Eq derives must round-trip identical class ids
    /// without surprises — the type lives in `HashMap<ValueId,
    /// TirType>` in the SSA value-types map and any divergence
    /// would silently desynchronize.
    #[test]
    fn user_class_eq_and_hash_match_on_id() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(class("class:m:Point"));
        assert!(set.contains(&class("class:m:Point")));
        assert!(!set.contains(&class("class:m:Line")));
    }

    /// Builtin-tag mapping: each frontend `BUILTIN_TYPE_TAGS`
    /// string round-trips to its canonical TirType.  Pin the
    /// contract — if anyone changes the frontend's tag spelling,
    /// this test catches the drift.
    #[test]
    fn from_type_hint_builtins() {
        assert_eq!(TirType::from_type_hint("int"), TirType::I64);
        assert_eq!(TirType::from_type_hint("float"), TirType::F64);
        assert_eq!(TirType::from_type_hint("bool"), TirType::Bool);
        assert_eq!(TirType::from_type_hint("str"), TirType::Str);
        assert_eq!(TirType::from_type_hint("bytes"), TirType::Bytes);
        assert_eq!(
            TirType::from_type_hint("list"),
            TirType::List(Box::new(TirType::DynBox))
        );
        assert_eq!(
            TirType::from_type_hint("dict"),
            TirType::Dict(Box::new(TirType::DynBox), Box::new(TirType::DynBox),)
        );
        assert_eq!(
            TirType::from_type_hint("set"),
            TirType::Set(Box::new(TirType::DynBox))
        );
        assert_eq!(TirType::from_type_hint("tuple"), TirType::Tuple(Vec::new()));
        assert_eq!(TirType::from_type_hint("None"), TirType::None);
        assert_eq!(TirType::from_type_hint("NoneType"), TirType::None);
        assert_eq!(TirType::from_type_hint("none"), TirType::None);
        assert_eq!(TirType::from_type_hint("BigInt"), TirType::BigInt);
    }

    /// Dynamic, malformed, or intentionally unsupported hints fall back to
    /// DynBox — soundness over precision.  Callable signatures and dynamic
    /// optional forms contain punctuation (`:`, `(`) that the simple-identifier
    /// check once promoted to a class.
    #[test]
    fn from_type_hint_dynamic_or_malformed_falls_back_to_dynbox() {
        assert_eq!(TirType::from_type_hint("Any"), TirType::DynBox);
        assert_eq!(TirType::from_type_hint("Unknown"), TirType::DynBox);
        assert_eq!(TirType::from_type_hint(""), TirType::DynBox);
        assert_eq!(TirType::from_type_hint("type"), TirType::DynBox);
        assert_eq!(
            TirType::from_type_hint("Func:foo_symbol"),
            TirType::DynBox,
            "Func:<symbol> hints defer to DynBox until proper \
             FuncSignature parsing is wired"
        );
        assert_eq!(
            TirType::from_type_hint("BoundMethod:list:append"),
            TirType::DynBox,
        );
        assert_eq!(TirType::from_type_hint("Optional(Point)"), TirType::DynBox,);
        assert_eq!(TirType::from_type_hint("list[]"), TirType::DynBox);
        assert_eq!(TirType::from_type_hint("list[int, str]"), TirType::DynBox);
        assert_eq!(TirType::from_type_hint("dict[str]"), TirType::DynBox);
        assert_eq!(TirType::from_type_hint("dict[str, Any]"), TirType::DynBox);
        assert_eq!(TirType::from_type_hint("tuple[]"), TirType::DynBox);
        assert_eq!(TirType::from_type_hint("tuple[int, ...]"), TirType::DynBox);
        assert_eq!(TirType::from_type_hint("Foo[int]"), TirType::DynBox);
        assert_eq!(TirType::from_type_hint("list[int"), TirType::DynBox);
        // Hints that look almost-identifier but aren't valid
        // (start with digit, contain whitespace) fall back.
        assert_eq!(TirType::from_type_hint("1Point"), TirType::DynBox);
        assert_eq!(TirType::from_type_hint("My Class"), TirType::DynBox);
    }

    /// Structured generic container hints are canonical TIR facts.  Backends
    /// must use these typed facts instead of SimpleIR transport flags when
    /// choosing scalar/container fast paths.
    #[test]
    fn from_type_hint_compound_containers_refine() {
        assert_eq!(
            TirType::from_type_hint("list[int]"),
            TirType::List(Box::new(TirType::I64))
        );
        assert_eq!(
            TirType::from_type_hint("list[list[int]]"),
            TirType::List(Box::new(TirType::List(Box::new(TirType::I64))))
        );
        assert_eq!(
            TirType::from_type_hint("dict[str, int]"),
            TirType::Dict(Box::new(TirType::Str), Box::new(TirType::I64))
        );
        assert_eq!(
            TirType::from_type_hint("set[bool]"),
            TirType::Set(Box::new(TirType::Bool))
        );
        assert_eq!(
            TirType::from_type_hint("tuple[int, str, bool]"),
            TirType::Tuple(vec![TirType::I64, TirType::Str, TirType::Bool])
        );
        assert_eq!(
            TirType::from_type_hint("List[dict[str, list[float]]]"),
            TirType::List(Box::new(TirType::Dict(
                Box::new(TirType::Str),
                Box::new(TirType::List(Box::new(TirType::F64))),
            )))
        );
    }

    /// A name is never class identity (HF-847 slice 1, D6): an
    /// identifier-shaped hint that is not a builtin tag is dynamic.
    #[test]
    fn from_type_hint_never_makes_a_class_from_a_name() {
        assert_eq!(TirType::from_type_hint("Point"), TirType::DynBox,);
        assert_eq!(TirType::from_type_hint("MyClass"), TirType::DynBox,);
        // Underscore + digit at non-leading positions are valid
        // identifier characters.
        assert_eq!(TirType::from_type_hint("Snake_case_123"), TirType::DynBox,);
        assert_eq!(TirType::from_type_hint("_private"), TirType::DynBox,);
    }
}
