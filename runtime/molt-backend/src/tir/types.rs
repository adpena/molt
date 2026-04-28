/// The TIR type system. Designed for progressive refinement:
/// values start as DynBox and get refined to concrete types.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TirType {
    // Unboxed scalars (register-resident)
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
    // Boxed
    /// NaN-boxed with known inner type.
    Box(Box<TirType>),
    /// NaN-boxed, type unknown.
    DynBox,
    /// A user-defined class instance, identified by the qualified
    /// class name (matching the frontend's `_type_hint` and
    /// `res_hint` conventions).  Carries the same NaN-boxed
    /// representation as `DynBox` today, but the type-refine pass
    /// can use it to:
    ///   - prove monomorphic method receivers for direct dispatch
    ///     (skip CallMethod IC lookup),
    ///   - prove static field offsets for direct load/store (skip
    ///     `class_layout_size` runtime lookup),
    ///   - tighten escape analysis (instances of a class with no
    ///     `__del__` and no weakref support can be stack-allocated
    ///     without per-instance cold-header allocation — Phase 5
    ///     step 3 prepared the runtime side; future commits wire
    ///     codegen).
    ///
    /// Two `UserClass` values meet to themselves when their ids
    /// match, otherwise they fall through to the standard Union /
    /// DynBox lattice machinery.
    ///
    /// Class identity is the qualified class name (e.g.
    /// `"mymodule.Point"`); the frontend already deduplicates these.
    UserClass(String),
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
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FuncSignature {
    pub params: Vec<TirType>,
    pub return_type: Box<TirType>,
}

impl TirType {
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
    ///   - `"list"`, `"dict"`, `"set"`, `"tuple"` → container with
    ///     `DynBox` element type (the frontend doesn't carry
    ///     parameter types in the hint string)
    ///   - `"None"`, `"NoneType"` → `None`
    ///   - `"BigInt"` → `BigInt`
    ///
    /// Compound hints fall back to `DynBox` for safety:
    ///   - `"Func:<symbol>"`, `"BoundMethod:<class>:<method>"`,
    ///     `"type"`, `"Any"`, `"Unknown"`, the empty string, and
    ///     anything containing punctuation that the simple-
    ///     identifier check rejects.
    ///
    /// **User-class refinement**: an identifier-shaped hint that
    /// is NOT a builtin tag refines to `UserClass(hint)`.  This
    /// is the live use of the variant — once the frontend's
    /// inferred class names propagate through the SimpleIR
    /// `type_hint` field, the type-refine pass can act on them
    /// (direct dispatch, static field offsets, tighter escape
    /// analysis).
    ///
    /// Safety: returns `DynBox` for any input that fails the
    /// identifier shape check (must be non-empty, ASCII
    /// alphanumeric or `_`, and not start with a digit).  This
    /// keeps badly-formed hints from creating spurious user
    /// classes in the type system.
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
                return TirType::Dict(
                    Box::new(TirType::DynBox),
                    Box::new(TirType::DynBox),
                );
            }
            "set" => return TirType::Set(Box::new(TirType::DynBox)),
            "tuple" => return TirType::Tuple(Vec::new()),
            "None" | "NoneType" => return TirType::None,
            "BigInt" | "bigint" => return TirType::BigInt,
            "Any" | "Unknown" | "" | "type" => return TirType::DynBox,
            _ => {}
        }
        // Compound hints (Func:..., BoundMethod:...) — defer to
        // a future commit that adds proper signature parsing.
        if hint.contains(':') || hint.contains('[') || hint.contains('(')
        {
            return TirType::DynBox;
        }
        // Identifier shape check: ASCII alphanumeric or `_`, not
        // empty, doesn't start with a digit.  Anything else is a
        // malformed or unrecognized hint — fall back to DynBox.
        let mut chars = hint.chars();
        let first = match chars.next() {
            Some(c) => c,
            None => return TirType::DynBox,
        };
        if !(first.is_ascii_alphabetic() || first == '_') {
            return TirType::DynBox;
        }
        if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return TirType::DynBox;
        }
        TirType::UserClass(hint.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn is_unboxed_and_numeric() {
        assert!(TirType::I64.is_unboxed());
        assert!(TirType::Bool.is_numeric());
        assert!(!TirType::Str.is_unboxed());
        assert!(!TirType::Str.is_numeric());
        assert!(!TirType::DynBox.is_unboxed());
    }

    /// Same-id `UserClass` meets to itself — the existing `self
    /// == other` early return in `meet()` handles this without a
    /// dedicated arm.  Pin the contract so future refactors don't
    /// drop the `PartialEq` derive that makes it work.
    #[test]
    fn meet_user_class_same_id_preserves() {
        let a = TirType::UserClass("Point".into());
        let b = TirType::UserClass("Point".into());
        assert_eq!(a.meet(&b), TirType::UserClass("Point".into()));
    }

    /// Different `UserClass` ids fall through to the existing
    /// Union/DynBox lattice machinery — no special-case logic.
    /// Two distinct user classes form a 2-member union; canonical
    /// ordering uses Debug-string sort so the result is
    /// deterministic regardless of operand order.
    #[test]
    fn meet_user_class_different_ids_unions() {
        let a = TirType::UserClass("Point".into());
        let b = TirType::UserClass("Line".into());
        let result = a.meet(&b);
        // "UserClass(\"Line\")" < "UserClass(\"Point\")" by Debug
        // string sort, so Line comes first.
        assert_eq!(
            result,
            TirType::Union(vec![
                TirType::UserClass("Line".into()),
                TirType::UserClass("Point".into()),
            ])
        );
        // Commutativity guard.
        assert_eq!(b.meet(&a), result);
    }

    /// `UserClass` meet `DynBox` collapses to `DynBox` — the
    /// existing absorption rule applies.  Critical: a refined
    /// type joining a path that doesn't refine must lose
    /// precision, otherwise the type-refine pass could promote
    /// type-erased exception handler args from DynBox to a
    /// specific class and miscompile the catch site.
    #[test]
    fn meet_user_class_with_dynbox_collapses() {
        let cls = TirType::UserClass("Point".into());
        assert_eq!(cls.meet(&TirType::DynBox), TirType::DynBox);
        assert_eq!(TirType::DynBox.meet(&cls), TirType::DynBox);
    }

    /// `UserClass` is **not unboxed** — instances are NaN-boxed
    /// today (Phase 5 step 3 stack-allocates the *backing*, but
    /// the value carried at the SSA level is still a tagged 64-bit
    /// pointer).  When direct stack-passable representation lands
    /// (analogous to Mojo's `@register_passable("trivial")`), this
    /// will flip — and `is_unboxed` must be revisited at every
    /// site that branches on it for register allocation choices.
    #[test]
    fn user_class_is_neither_unboxed_nor_numeric() {
        let cls = TirType::UserClass("Point".into());
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
        set.insert(TirType::UserClass("Point".into()));
        assert!(set.contains(&TirType::UserClass("Point".into())));
        assert!(!set.contains(&TirType::UserClass("Line".into())));
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
            TirType::Dict(
                Box::new(TirType::DynBox),
                Box::new(TirType::DynBox),
            )
        );
        assert_eq!(
            TirType::from_type_hint("set"),
            TirType::Set(Box::new(TirType::DynBox))
        );
        assert_eq!(
            TirType::from_type_hint("tuple"),
            TirType::Tuple(Vec::new())
        );
        assert_eq!(TirType::from_type_hint("None"), TirType::None);
        assert_eq!(TirType::from_type_hint("NoneType"), TirType::None);
        assert_eq!(TirType::from_type_hint("BigInt"), TirType::BigInt);
    }

    /// Compound or unknown hints fall back to DynBox — soundness
    /// over precision.  A compound hint contains punctuation
    /// (`:`, `[`, `(`) that the simple-identifier check would
    /// otherwise erroneously promote to UserClass.
    #[test]
    fn from_type_hint_compound_falls_back_to_dynbox() {
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
        assert_eq!(
            TirType::from_type_hint("list[int]"),
            TirType::DynBox,
            "Parameterized hints contain `[` — defer to DynBox"
        );
        assert_eq!(
            TirType::from_type_hint("Optional(Point)"),
            TirType::DynBox,
        );
        // Hints that look almost-identifier but aren't valid
        // (start with digit, contain whitespace) fall back.
        assert_eq!(TirType::from_type_hint("1Point"), TirType::DynBox);
        assert_eq!(TirType::from_type_hint("My Class"), TirType::DynBox);
    }

    /// Identifier-shaped non-builtin hints refine to UserClass.
    /// This is the *live* use of the new variant: the frontend's
    /// `class Point: ...` produces type_hint="Point" on the
    /// `OBJECT_NEW_BOUND` op, and the SSA lift then refines that
    /// value's type from DynBox to UserClass("Point").
    #[test]
    fn from_type_hint_user_class_refines() {
        assert_eq!(
            TirType::from_type_hint("Point"),
            TirType::UserClass("Point".into()),
        );
        assert_eq!(
            TirType::from_type_hint("MyClass"),
            TirType::UserClass("MyClass".into()),
        );
        // Underscore + digit at non-leading positions are valid
        // identifier characters.
        assert_eq!(
            TirType::from_type_hint("Snake_case_123"),
            TirType::UserClass("Snake_case_123".into()),
        );
        assert_eq!(
            TirType::from_type_hint("_private"),
            TirType::UserClass("_private".into()),
        );
    }
}
