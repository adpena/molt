//! Typed function identity (HF-847 slice 1, design 78 "Slice 1: typed
//! identity").
//!
//! A [`FunctionId`] is a dense index into one document's function vector:
//! `SimpleIR::functions`, bodies and extern declarations alike. That vector
//! is the function table; no second registry repeats its rows (I1). Decode
//! resolves each function-reference op's wire symbol to an id once
//! ([`resolve_function_references`]); after decode no op carries a callee
//! name (I2). A transformation that removes, reorders or partitions the
//! vector rewrites every id through [`remap_function_ids`] (I4), so an id is
//! always valid in the document that holds it, and a row a batch appends is
//! visible only in its own batch document (C2).

use std::collections::HashMap;

use crate::FunctionIR;

/// Dense identity of one function in its document (I1, I3, I4).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct FunctionId(pub u32);

impl FunctionId {
    pub fn index(self) -> usize {
        self.0 as usize
    }

    pub fn from_index(index: usize) -> Self {
        Self(u32::try_from(index).expect("function table exceeds the u32 FunctionId domain"))
    }
}

/// Whether a row has a body in this document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FunctionRowKind {
    Body,
    Extern,
}

/// A projection of one document's function vector for consumers that do not
/// hold the vector (the TIR module, a backend's call lowering). Structure of
/// arrays indexed by [`FunctionId`]. Built at a boundary by [`Self::of`]; it
/// never diverges from the vector it was built from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FunctionTable {
    symbols: Vec<String>,
    kinds: Vec<FunctionRowKind>,
}

impl FunctionTable {
    /// One row per function, in vector order.
    pub fn of(functions: &[FunctionIR]) -> Self {
        Self {
            symbols: functions.iter().map(|f| f.name.clone()).collect(),
            kinds: functions
                .iter()
                .map(|f| {
                    if f.is_extern {
                        FunctionRowKind::Extern
                    } else {
                        FunctionRowKind::Body
                    }
                })
                .collect(),
        }
    }

    /// Rows from `(symbol, kind)` pairs in document order, for a consumer that
    /// holds lifted bodies rather than the vector itself.
    pub fn from_rows<'a>(rows: impl IntoIterator<Item = (&'a str, FunctionRowKind)>) -> Self {
        let (symbols, kinds) = rows
            .into_iter()
            .map(|(symbol, kind)| (symbol.to_string(), kind))
            .unzip();
        Self { symbols, kinds }
    }

    /// The linker symbol of `id` (I7). Panics on an id from another
    /// document (I3).
    pub fn symbol(&self, id: FunctionId) -> &str {
        self.symbols.get(id.index()).unwrap_or_else(|| {
            panic!(
                "FunctionId {} is outside its table of {}",
                id.0,
                self.symbols.len()
            )
        })
    }

    pub fn kind(&self, id: FunctionId) -> FunctionRowKind {
        self.kinds[id.index()]
    }

    /// Whether `id` has a body in this document.
    pub fn defines(&self, id: FunctionId) -> bool {
        self.kind(id) == FunctionRowKind::Body
    }

    pub fn len(&self) -> usize {
        self.symbols.len()
    }

    pub fn is_empty(&self) -> bool {
        self.symbols.is_empty()
    }

    pub fn ids(&self) -> impl Iterator<Item = FunctionId> + '_ {
        (0..self.symbols.len()).map(FunctionId::from_index)
    }

    /// Append the row of a function a pass creates (megafunction split).
    pub fn push(&mut self, function: &FunctionIR) -> FunctionId {
        let id = FunctionId::from_index(self.symbols.len());
        self.symbols.push(function.name.clone());
        self.kinds.push(if function.is_extern {
            FunctionRowKind::Extern
        } else {
            FunctionRowKind::Body
        });
        id
    }
}

/// A set of functions as a bitset over one table.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FunctionSet {
    words: Vec<u64>,
}

impl FunctionSet {
    pub fn with_capacity(len: usize) -> Self {
        Self {
            words: vec![0; len.div_ceil(64)],
        }
    }

    pub fn insert(&mut self, id: FunctionId) -> bool {
        let (word, bit) = (id.index() / 64, id.index() % 64);
        if word >= self.words.len() {
            self.words.resize(word + 1, 0);
        }
        let mask = 1u64 << bit;
        let fresh = self.words[word] & mask == 0;
        self.words[word] |= mask;
        fresh
    }

    pub fn contains(&self, id: FunctionId) -> bool {
        let (word, bit) = (id.index() / 64, id.index() % 64);
        self.words
            .get(word)
            .is_some_and(|value| value & (1u64 << bit) != 0)
    }

    pub fn iter(&self) -> impl Iterator<Item = FunctionId> + '_ {
        self.words.iter().enumerate().flat_map(|(word, value)| {
            (0..64)
                .filter(move |bit| value & (1u64 << bit) != 0)
                .map(move |bit| FunctionId::from_index(word * 64 + bit))
        })
    }

    pub fn len(&self) -> usize {
        self.words.iter().map(|w| w.count_ones() as usize).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.words.iter().all(|w| *w == 0)
    }
}

/// Resolve each function-reference op's wire symbol to a [`FunctionId`] in
/// `functions`, once, at decode (D2). A resolved op carries `callee` and no
/// `s_value`. An op whose generated role requires a compiled function
/// (`call_internal`, `call_guarded`, `func_new`, `func_new_closure`; the
/// generated `simpleir_function_reference_roles` table) and names no row is a
/// decode error: a caller that spells a callee instead of
/// reading the definer's symbol fails here, not in a late lowering. An
/// optional reference (`call`, `alloc_task`, ...) whose symbol names no row
/// keeps it: that is its runtime or external target, decoded at TIR ingest
/// (HF-821 B1).
///
/// Requires: `functions` as the wire delivered them; an op that already
/// carries `callee` came from an internal transport and is left as is.
/// Ensures: I2 and I3 for every op.
/// Raises: Err naming the function, the op kind and the unresolved symbol.
pub fn resolve_function_references(functions: &mut [FunctionIR]) -> Result<(), String> {
    resolve_function_references_in(functions, OutsideSymbols::Reject)
}

/// What decode does with a required reference whose symbol names no row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutsideSymbols {
    /// A whole program: the symbol is a spelled callee no definer compiled.
    Reject,
    /// A fragment checked on its own (a megafunction split's stub and
    /// chunks): the symbol names a row of the enclosing document.
    Keep,
}

/// [`resolve_function_references`] with an explicit policy for symbols
/// outside `functions`.
pub fn resolve_function_references_in(
    functions: &mut [FunctionIR],
    outside: OutsideSymbols,
) -> Result<(), String> {
    let by_symbol: HashMap<&str, FunctionId> = functions
        .iter()
        .enumerate()
        .map(|(index, f)| (f.name.as_str(), FunctionId::from_index(index)))
        .collect();
    let mut resolved: Vec<(usize, usize, FunctionId)> = Vec::new();
    for (function_index, function) in functions.iter().enumerate() {
        for (op_index, op) in function.ops.iter().enumerate() {
            let role = op.kind.function_reference_role();
            if role == crate::FunctionReferenceRole::None {
                continue;
            }
            if op.callee.is_some() {
                // Already decoded: an internal transport of a resolved document.
                continue;
            }
            let Some(symbol) = op.s_value.as_deref().filter(|s| !s.is_empty()) else {
                continue;
            };
            match by_symbol.get(symbol) {
                Some(&id) => resolved.push((function_index, op_index, id)),
                None if role == crate::FunctionReferenceRole::Required
                    && outside == OutsideSymbols::Reject =>
                {
                    return Err(format!(
                        "{}: `{}` names `{symbol}`, which no function in the program defines or declares",
                        function.name, op.kind
                    ));
                }
                None => {}
            }
        }
    }
    for (function_index, op_index, id) in resolved {
        let op = &mut functions[function_index].ops[op_index];
        op.callee = Some(id);
        op.s_value = None;
    }
    crate::class_identity::check_class_identities(functions)
}

/// Rewrite every `callee` in `functions` through `map` (old index -> new
/// id). The one helper for a transformation that removes, reorders or
/// partitions a document's function vector (I4).
///
/// Requires: `map[old]` is `Some` for every id an op of `functions` names.
/// Raises: panics naming the function and the dropped id otherwise; a
/// dangling reference is a broken transformation, never a fallback.
pub fn remap_function_ids(functions: &mut [FunctionIR], map: &[Option<FunctionId>]) {
    for function in functions.iter_mut() {
        for op in &mut function.ops {
            if let Some(old) = op.callee {
                op.callee = Some(map.get(old.index()).copied().flatten().unwrap_or_else(|| {
                    panic!(
                        "{}: callee FunctionId {} has no row after the transformation",
                        function.name, old.0
                    )
                }));
            }
        }
    }
}

/// Keep the functions `keep` selects and compact ids (dead-function
/// elimination). Returns the old-index -> new-id map it applied.
pub fn retain_functions(
    functions: &mut Vec<FunctionIR>,
    mut keep: impl FnMut(&FunctionIR) -> bool,
) -> Vec<Option<FunctionId>> {
    let mut map = Vec::with_capacity(functions.len());
    let mut next = 0usize;
    let mut kept = Vec::with_capacity(functions.len());
    for function in functions.drain(..) {
        if keep(&function) {
            map.push(Some(FunctionId::from_index(next)));
            next += 1;
            kept.push(function);
        } else {
            map.push(None);
        }
    }
    remap_function_ids(&mut kept, &map);
    *functions = kept;
    map
}

/// Keep the rows `keep` names, with `side[i]` the fact of row `i`, and compact
/// ids through [`retain_functions`]. Returns the old-index -> new-id map.
pub fn retain_rows_with<T>(
    functions: &mut Vec<FunctionIR>,
    side: &mut Vec<T>,
    keep: &FunctionSet,
) -> Vec<Option<FunctionId>> {
    assert_eq!(
        functions.len(),
        side.len(),
        "row facts must align with rows"
    );
    let mut index = 0usize;
    *side = std::mem::take(side)
        .into_iter()
        .enumerate()
        .filter_map(|(row, fact)| keep.contains(FunctionId::from_index(row)).then_some(fact))
        .collect();
    retain_functions(functions, |_| {
        index += 1;
        keep.contains(FunctionId::from_index(index - 1))
    })
}

/// Build a sub-document (a batch, a partition, an object job) from bodies of
/// one document: `bodies[i]` (with its id `old` in the source document)
/// becomes row `i`; then each source row the bodies reference that is not
/// among them becomes one extern declaration, in first-use order,
/// `declaration(old)` supplying it. Every callee is rewritten to the
/// sub-document's ids, so an id never crosses documents (I4, C2).
///
/// Requires: each body's callees index the source document.
/// Ensures: the result resolves every callee in itself.
pub fn project_document(
    bodies: Vec<(FunctionId, FunctionIR)>,
    mut declaration: impl FnMut(FunctionId) -> FunctionIR,
) -> Vec<FunctionIR> {
    let mut map: HashMap<FunctionId, FunctionId> = bodies
        .iter()
        .enumerate()
        .map(|(index, (old, _))| (*old, FunctionId::from_index(index)))
        .collect();
    let mut functions: Vec<FunctionIR> = bodies.into_iter().map(|(_, body)| body).collect();
    let mut externs: Vec<FunctionIR> = Vec::new();
    let base = functions.len();
    for function in &mut functions {
        for op in &mut function.ops {
            let Some(old) = op.callee else {
                continue;
            };
            let next = FunctionId::from_index(base + externs.len());
            let id = *map.entry(old).or_insert_with(|| {
                let mut row = declaration(old);
                if !row.is_extern {
                    row = row
                        .extern_declaration()
                        .unwrap_or_else(|error| panic!("invalid projected declaration: {error}"));
                }
                externs.push(row);
                next
            });
            op.callee = Some(id);
        }
    }
    functions.extend(externs);
    functions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FunctionReturnAbi, OpIR, OpKind};

    fn function(name: &str, ops: Vec<OpIR>) -> FunctionIR {
        FunctionIR {
            module: None,
            name: name.into(),
            return_abi: FunctionReturnAbi::Void,
            params: vec![],
            ops,
            param_types: None,
            source_file: None,
            is_extern: false,
            execution_context: Default::default(),
            codegen_partition: false,
            parameter_custody: vec![],
        }
    }

    fn named(kind: OpKind, symbol: &str) -> OpIR {
        OpIR {
            s_value: Some(symbol.into()),
            ..OpIR::new(kind)
        }
    }

    #[test]
    fn decode_moves_each_defined_symbol_into_a_typed_callee() {
        let mut functions = vec![
            function(
                "entry",
                vec![
                    named(OpKind::CallInternal, "helper"),
                    named(OpKind::Call, "external_entry"),
                ],
            ),
            function("helper", vec![]),
        ];
        resolve_function_references(&mut functions).unwrap();
        let ops = &functions[0].ops;
        assert_eq!(ops[0].callee, Some(FunctionId(1)));
        // A symbol outside the table stays: TIR ingest decodes a runtime
        // target from it (HF-821 B1).
        assert_eq!(ops[1].callee, None);
        assert_eq!(ops[1].s_value.as_deref(), Some("external_entry"));
    }

    #[test]
    fn decode_refuses_a_spelled_callee_that_no_definer_compiled() {
        // `import m; m.main()`: the definer compiled `main` under a counter
        // suffix, so the caller's spelled `py.m.main` names nothing.
        let mut functions = vec![
            function("entry", vec![named(OpKind::CallGuarded, "py.m.main")]),
            function("py.m.main_1", vec![]),
        ];
        let error = resolve_function_references(&mut functions).unwrap_err();
        assert!(error.contains("py.m.main"), "{error}");
        assert!(error.contains("call_guarded"), "{error}");
    }

    #[test]
    fn retain_compacts_ids_and_rewrites_every_callee() {
        let mut functions = vec![
            function("entry", vec![named(OpKind::CallInternal, "kept")]),
            function("dead", vec![]),
            function("kept", vec![]),
        ];
        resolve_function_references(&mut functions).unwrap();
        assert_eq!(functions[0].ops[0].callee, Some(FunctionId(2)));
        let map = retain_functions(&mut functions, |f| f.name != "dead");
        assert_eq!(map, vec![Some(FunctionId(0)), None, Some(FunctionId(1))]);
        assert_eq!(functions[0].ops[0].callee, Some(FunctionId(1)));
        assert_eq!(FunctionTable::of(&functions).symbol(FunctionId(1)), "kept");
    }

    #[test]
    #[should_panic(expected = "has no row after the transformation")]
    fn retain_refuses_to_drop_a_referenced_function() {
        let mut functions = vec![
            function("entry", vec![named(OpKind::CallInternal, "helper")]),
            function("helper", vec![]),
        ];
        resolve_function_references(&mut functions).unwrap();
        retain_functions(&mut functions, |f| f.name != "helper");
    }

    #[test]
    fn function_set_is_a_bitset_over_ids() {
        let mut set = FunctionSet::with_capacity(3);
        assert!(set.insert(FunctionId(70)));
        assert!(!set.insert(FunctionId(70)));
        assert!(set.insert(FunctionId(1)));
        assert!(set.contains(FunctionId(1)) && !set.contains(FunctionId(2)));
        assert_eq!(
            set.iter().collect::<Vec<_>>(),
            [FunctionId(1), FunctionId(70)]
        );
        assert_eq!(set.len(), 2);
    }
}
