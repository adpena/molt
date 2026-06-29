use serde::{Deserialize, Deserializer};

use crate::ir_schema;
use crate::json_boundary::{
    expect_object, optional_bool, optional_bytes, optional_f64, optional_i64, optional_string,
    optional_string_list, required_field, required_string, required_string_list,
};
use serde_json::Value as JsonValue;
use std::collections::{BTreeSet, VecDeque};

#[derive(Debug, Default, Clone, Deserialize, serde::Serialize)]
#[serde(default)]
pub struct PgoProfileIR {
    pub version: Option<String>,
    pub hash: Option<String>,
    pub hot_functions: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SimpleIR {
    pub functions: Vec<FunctionIR>,
    pub profile: Option<PgoProfileIR>,
}

#[derive(Debug, Default, Deserialize, Clone, serde::Serialize)]
pub struct FunctionIR {
    pub name: String,
    pub params: Vec<String>,
    pub ops: Vec<OpIR>,
    pub param_types: Option<Vec<String>>,
    /// Source file path for traceback formatting.
    #[serde(default)]
    pub source_file: Option<String>,
    /// When true, this function's body was stripped (already compiled into
    /// stdlib_shared.o).  The backend emits a declaration (no body) so
    /// the linker resolves the symbol from the shared object.
    #[serde(default)]
    pub is_extern: bool,
}

#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
#[serde(default)]
pub struct OpIR {
    pub kind: String,
    pub value: Option<i64>,
    pub f_value: Option<f64>,
    pub s_value: Option<String>,
    pub bytes: Option<Vec<u8>>,
    pub var: Option<String>,
    pub args: Option<Vec<String>>,
    pub out: Option<String>,
    /// Transitional transport compatibility hint for legacy consumers.
    /// Not the canonical backend representation contract.
    pub fast_int: Option<bool>,
    /// Transitional transport compatibility hint for legacy consumers.
    /// Not the canonical backend representation contract.
    pub fast_float: Option<bool>,
    pub stack_eligible: Option<bool>,
    /// When true, this allocation should use the function-scoped
    /// `ScopeArena` (bump allocator) instead of individual heap alloc/free.
    /// Set by escape analysis when an `Alloc` is `NoEscape` and the arena
    /// integration is active.
    #[serde(default)]
    pub arena_eligible: Option<bool>,
    /// When true on an `object_new_bound` op, the instance's class defines a
    /// `__del__` finalizer (directly or via its MRO, excluding `object`). The
    /// frontend resolves this statically and the backend escape pass must keep
    /// such an instance heap-allocated with a live refcount — never stack-promote
    /// it (which stamps it IMMORTAL) and never strip its IncRef/DecRef — so the
    /// finalizer-aware `dec_ref_ptr` dispatches `__del__` at the last reference
    /// drop. Without this the refcount-zero transition never occurs and the
    /// finalizer silently never runs (the standing LLVM/WASM parity hole).
    #[serde(default)]
    pub defines_del: Option<bool>,
    /// Named-local fact (#58 ordering keystone): the op's result is bound to
    /// a plain function-local NAME. CPython holds a named local in the frame
    /// until `del`/rebinding/scope exit; an unnamed expression temp dies at
    /// the statement. The ownership-lattice release deferral consumes this to
    /// defer ONLY named finalizer-sensitive values to the scope boundary.
    #[serde(default)]
    pub bound_local: Option<bool>,
    pub task_kind: Option<String>,
    pub container_type: Option<String>,
    pub native_callable_export: Option<String>,
    pub native_callable_binding: Option<String>,
    pub native_callable_symbol: Option<String>,
    pub native_callable_abi: Option<String>,
    /// Transitional semantic hint preserved on the transport surface for
    /// compatibility consumers. The canonical representation contract lives in
    /// TIR/LIR, not this field.
    pub type_hint: Option<String>,
    /// Inline-cache site index for attribute access acceleration.
    /// Assigned by the frontend for `get_attr_generic_ptr` ops.
    pub ic_index: Option<i64>,
    /// Stable source operation index for backend inline-cache site identity.
    /// This is assigned by the SimpleIR -> TIR lift and transported back
    /// through TIR -> SimpleIR so backends do not reconstruct IC identity from
    /// their local stream order.
    pub source_op_idx: Option<i64>,
    /// Column offset (0-based) for traceback caret annotations.
    /// Carried by `line` ops from the frontend AST node's `col_offset`.
    pub col_offset: Option<i64>,
    /// End column offset (0-based) for traceback caret annotations.
    pub end_col_offset: Option<i64>,
    /// Source line number (1-based) for structural source-site attribution.
    /// Frontend JSON annotates executable ops from the active `line` marker;
    /// the SimpleIR -> TIR lift preserves it as the TIR source-site authority.
    pub source_line: Option<i64>,
    /// When true, the bounds-check elimination pass has proven this index
    /// operation is in-range.  Codegen can skip the runtime bounds check
    /// and emit a straight-line element access.
    pub bce_safe: Option<bool>,
    /// Named proof that a normally observable effect/exception edge has
    /// already been discharged by an earlier semantic analysis.
    ///
    /// This is not a representation hint: consumers must validate the proof
    /// name against the op kind before weakening effect semantics.
    pub effect_proof: Option<String>,
    /// The concrete user-class name whose fixed instance layout authored the
    /// `value` byte-offset of a typed-slot field op (`store` / `store_init` /
    /// `load` / `guarded_field_get` / `guarded_field_set` / `guarded_field_init`).
    ///
    /// The frontend emits these offset-based forms ONLY when the object's class
    /// is proven at the op — either by a preceding runtime version-guard (the
    /// `guarded_field_*` forms deopt/raise on class mismatch) or by static type
    /// inference (the plain `store`/`load` forms). Either way the class is the
    /// authority for `offset`, so the alias oracle can assign a class+offset
    /// `TypedField` memory region that disjoint-aliases other classes' fields,
    /// container elements, and module-dict slots (S5-1.5).
    ///
    /// Wire name is `"class"` (the frontend JSON / msgpack key); the manual
    /// `from_json_value` parser and the serde derive (rmp/cbor path) agree on it.
    #[serde(rename = "class")]
    pub class_name: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OpSourceSite {
    pub source_line: Option<i64>,
    pub col_offset: Option<i64>,
    pub end_col_offset: Option<i64>,
}

impl OpSourceSite {
    pub fn from_op(op: &OpIR) -> Self {
        Self {
            source_line: op.source_line,
            col_offset: op.col_offset,
            end_col_offset: op.end_col_offset,
        }
    }

    pub fn apply_to_op(self, op: &mut OpIR) {
        op.source_line = self.source_line;
        op.col_offset = self.col_offset;
        op.end_col_offset = self.end_col_offset;
    }
}

impl OpIR {
    pub fn source_site(&self) -> OpSourceSite {
        OpSourceSite::from_op(self)
    }

    pub fn inherit_source_site_from(&mut self, other: &OpIR) {
        other.source_site().apply_to_op(self);
    }
}

impl PgoProfileIR {
    fn from_json_value(value: &JsonValue, ctx: &str) -> Result<Self, String> {
        let obj = expect_object(value, ctx)?;
        Ok(Self {
            version: optional_string(obj, "version", ctx)?,
            hash: optional_string(obj, "hash", ctx)?,
            hot_functions: optional_string_list(obj, "hot_functions", ctx)?.unwrap_or_default(),
        })
    }
}

impl SimpleIR {
    pub fn from_json_str(input: &str) -> Result<Self, String> {
        let value: JsonValue =
            serde_json::from_str(input).map_err(|err| format!("invalid IR JSON: {err}"))?;
        Self::from_json_value(&value)
    }

    pub fn from_json_value(value: &JsonValue) -> Result<Self, String> {
        let obj = expect_object(value, "ir")?;
        let functions_value = required_field(obj, "functions", "ir")?;
        let function_values = functions_value
            .as_array()
            .ok_or_else(|| "ir.functions must be an array".to_string())?;
        let mut functions = Vec::with_capacity(function_values.len());
        for (idx, function_value) in function_values.iter().enumerate() {
            functions.push(FunctionIR::from_json_value(
                function_value,
                &format!("ir.functions[{idx}]"),
            )?);
        }
        let profile = match obj.get("profile") {
            None | Some(JsonValue::Null) => None,
            Some(profile_value) => {
                Some(PgoProfileIR::from_json_value(profile_value, "ir.profile")?)
            }
        };
        Self::new_validated(functions, profile)
    }

    pub fn from_ndjson_reader<R: std::io::BufRead>(reader: R) -> Result<Self, String> {
        let mut functions = Vec::new();
        let mut profile = None;
        for line in reader.lines() {
            let line = line.map_err(|e| format!("NDJSON read error: {e}"))?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let value: JsonValue =
                serde_json::from_str(trimmed).map_err(|e| format!("NDJSON parse error: {e}"))?;
            match value.get("kind").and_then(|v| v.as_str()) {
                Some("ir_stream_start") => {
                    if let Some(p) = value.get("profile")
                        && !p.is_null()
                    {
                        profile = Some(PgoProfileIR::from_json_value(p, "stream.profile")?);
                    }
                }
                Some("function") => {
                    functions.push(FunctionIR::from_json_value(&value, "stream.function")?);
                }
                Some("ir_stream_end") => break,
                _ => {} // skip unknown kinds for forward compat
            }
        }
        Self::new_validated(functions, profile)
    }

    fn new_validated(
        functions: Vec<FunctionIR>,
        profile: Option<PgoProfileIR>,
    ) -> Result<Self, String> {
        let ir = Self { functions, profile };
        validate_simple_ir_transport_contract(&ir)
            .map_err(|err| format!("invalid SimpleIR contract: {err}"))?;
        Ok(ir)
    }

    pub fn tree_shake_source_emission(&mut self) {
        for func in &mut self.functions {
            match func.name.as_str() {
                "molt_main" => {
                    func.ops.retain(|op| {
                        !(op.kind == "call" && op.s_value.as_deref() == Some("molt_runtime_init"))
                    });
                }
                "molt_init___main__" => {
                    for op in &mut func.ops {
                        if op.kind == "call" && op.s_value.as_deref() == Some("molt_init_sys") {
                            *op = OpIR {
                                kind: "nop".to_string(),
                                ..OpIR::default()
                            };
                        }
                    }
                }
                _ => {}
            }
        }

        let known_functions: BTreeSet<&str> = self
            .functions
            .iter()
            .map(|func| func.name.as_str())
            .collect();
        let mut reachable: BTreeSet<String> = BTreeSet::from([String::from("molt_main")]);
        let mut worklist: VecDeque<String> = VecDeque::from([String::from("molt_main")]);

        while let Some(func_name) = worklist.pop_front() {
            let Some(func) = self
                .functions
                .iter()
                .find(|candidate| candidate.name == func_name)
            else {
                continue;
            };
            for op in &func.ops {
                // Follow direct call targets.
                if op.kind == "call" {
                    let Some(target) = op.s_value.as_deref() else {
                        continue;
                    };
                    if matches!(target, "molt_runtime_init" | "molt_init_sys") {
                        continue;
                    }
                    if !known_functions.contains(target) {
                        continue;
                    }
                    if reachable.insert(target.to_string()) {
                        worklist.push_back(target.to_string());
                    }
                    continue;
                }
                // Follow function object creation ops — these reference
                // user-defined function bodies by name in s_value.  Without
                // this, source emitters prune user functions that are only
                // referenced through func_new (not direct calls).
                if matches!(
                    op.kind.as_str(),
                    "func_new" | "func_new_closure" | "code_new" | "call_internal"
                ) {
                    let Some(target) = op.s_value.as_deref() else {
                        continue;
                    };
                    if !known_functions.contains(target) {
                        continue;
                    }
                    if reachable.insert(target.to_string()) {
                        worklist.push_back(target.to_string());
                    }
                }
            }
        }

        self.functions
            .retain(|func| reachable.contains(func.name.as_str()));
    }

    pub fn tree_shake_luau(&mut self) {
        self.tree_shake_source_emission();
    }
}

impl<'de> Deserialize<'de> for SimpleIR {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct SimpleIRWire {
            functions: Vec<FunctionIR>,
            #[serde(default)]
            profile: Option<PgoProfileIR>,
        }

        let wire = SimpleIRWire::deserialize(deserializer)?;
        Self::new_validated(wire.functions, wire.profile).map_err(serde::de::Error::custom)
    }
}

impl FunctionIR {
    fn from_json_value(value: &JsonValue, ctx: &str) -> Result<Self, String> {
        let obj = expect_object(value, ctx)?;
        let ops_value = required_field(obj, "ops", ctx)?;
        let op_values = ops_value
            .as_array()
            .ok_or_else(|| format!("{ctx}.ops must be an array"))?;
        let mut ops = Vec::with_capacity(op_values.len());
        for (idx, op_value) in op_values.iter().enumerate() {
            ops.push(OpIR::from_json_value(
                op_value,
                &format!("{ctx}.ops[{idx}]"),
            )?);
        }
        Ok(Self {
            name: required_string(obj, "name", ctx)?,
            params: required_string_list(obj, "params", ctx)?,
            ops,
            param_types: optional_string_list(obj, "param_types", ctx)?,
            source_file: obj
                .get("source_file")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            is_extern: false,
        })
    }
}

impl OpIR {
    fn from_json_value(value: &JsonValue, ctx: &str) -> Result<Self, String> {
        let obj = expect_object(value, ctx)?;
        // Extract ic_index from nested "metadata" object if present.
        let ic_index = obj
            .get("metadata")
            .and_then(|m| m.as_object())
            .and_then(|m| m.get("ic_index"))
            .and_then(|v| v.as_i64());
        Ok(Self {
            kind: required_string(obj, "kind", ctx)?,
            value: optional_i64(obj, "value", ctx)?,
            f_value: optional_f64(obj, "f_value", ctx)?,
            s_value: optional_string(obj, "s_value", ctx)?,
            bytes: optional_bytes(obj, "bytes", ctx)?,
            var: optional_string(obj, "var", ctx)?,
            args: optional_string_list(obj, "args", ctx)?,
            out: optional_string(obj, "out", ctx)?,
            fast_int: optional_bool(obj, "fast_int", ctx)?,
            fast_float: optional_bool(obj, "fast_float", ctx)?,
            stack_eligible: optional_bool(obj, "stack_eligible", ctx)?,
            task_kind: optional_string(obj, "task_kind", ctx)?,
            container_type: optional_string(obj, "container_type", ctx)?,
            native_callable_export: optional_string(obj, "native_callable_export", ctx)?,
            native_callable_binding: optional_string(obj, "native_callable_binding", ctx)?,
            native_callable_symbol: optional_string(obj, "native_callable_symbol", ctx)?,
            native_callable_abi: optional_string(obj, "native_callable_abi", ctx)?,
            type_hint: optional_string(obj, "type_hint", ctx)?,
            ic_index,
            source_op_idx: optional_i64(obj, "source_op_idx", ctx)?,
            col_offset: optional_i64(obj, "col_offset", ctx)?,
            end_col_offset: optional_i64(obj, "end_col_offset", ctx)?,
            source_line: optional_i64(obj, "source_line", ctx)?,
            bce_safe: optional_bool(obj, "bce_safe", ctx)?,
            arena_eligible: optional_bool(obj, "arena_eligible", ctx)?,
            defines_del: optional_bool(obj, "defines_del", ctx)?,
            bound_local: optional_bool(obj, "bound_local", ctx)?,
            effect_proof: optional_string(obj, "effect_proof", ctx)?,
            class_name: optional_string(obj, "class", ctx)?,
        })
    }
}

fn op_uses(op: &OpIR) -> impl Iterator<Item = (&str, usize)> {
    op.args
        .iter()
        .flat_map(|args| {
            args.iter()
                .enumerate()
                .map(|(idx, value)| (value.as_str(), idx))
        })
        .chain(op.var.iter().map(|value| (value.as_str(), usize::MAX)))
}

fn is_defined_value_name(name: &str) -> bool {
    !name.is_empty() && name != "none"
}

fn allows_undefined_value(op: &OpIR, name: &str, position: usize) -> bool {
    if !name.starts_with('v') {
        return false;
    }
    position == 0 && matches!(op.kind.as_str(), "dict_set" | "index")
}

pub fn validate_simple_ir(ir: &SimpleIR) -> Result<(), String> {
    validate_simple_ir_transport_contract(ir)?;
    for func in &ir.functions {
        let mut defined: BTreeSet<&str> = func.params.iter().map(String::as_str).collect();
        for op in &func.ops {
            for (name, position) in op_uses(op) {
                if !is_defined_value_name(name) {
                    continue;
                }
                if defined.contains(name) || allows_undefined_value(op, name, position) {
                    continue;
                }
                return Err(format!("op `{}` uses undefined value `{}`", op.kind, name));
            }
            if let Some(out) = op.out.as_deref()
                && is_defined_value_name(out)
            {
                defined.insert(out);
            }
        }
    }
    Ok(())
}

fn validate_simple_ir_transport_contract(ir: &SimpleIR) -> Result<(), String> {
    for func in &ir.functions {
        ir_schema::validate_function_param_types(
            &func.name,
            &func.params,
            func.param_types.as_deref(),
        )?;
        for op in &func.ops {
            ir_schema::validate_required_fields(op)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod json_parse_tests {
    use super::SimpleIR;

    #[test]
    fn simple_ir_from_json_str_applies_optional_defaults() {
        let ir = SimpleIR::from_json_str(
            r#"{
                "functions": [
                    {
                        "name": "__main__",
                        "params": [],
                        "ops": [{"kind": "ret_void"}]
                    }
                ]
            }"#,
        )
        .expect("ir parse");

        assert_eq!(ir.functions.len(), 1);
        assert!(ir.profile.is_none());
        assert!(ir.functions[0].param_types.is_none());
        assert!(ir.functions[0].ops[0].args.is_none());
        assert!(ir.functions[0].ops[0].fast_int.is_none());
    }

    #[test]
    fn simple_ir_from_json_str_parses_profile_without_hot_functions() {
        let ir = SimpleIR::from_json_str(
            r#"{
                "functions": [
                    {
                        "name": "__main__",
                        "params": [],
                        "ops": [{"kind": "ret_void"}]
                    }
                ],
                "profile": {
                    "version": "v1"
                }
            }"#,
        )
        .expect("ir parse");

        let profile = ir.profile.expect("profile");
        assert_eq!(profile.version.as_deref(), Some("v1"));
        assert!(profile.hot_functions.is_empty());
    }

    #[test]
    fn simple_ir_from_json_str_rejects_contract_violations() {
        let err = SimpleIR::from_json_str(
            r#"{
                "functions": [
                    {
                        "name": "__main__",
                        "params": ["seq", "idx"],
                        "ops": [
                            {
                                "kind": "index",
                                "args": ["seq", "idx"],
                                "out": "item",
                                "container_type": "list_int",
                                "bce_safe": true
                            }
                        ]
                    }
                ]
            }"#,
        )
        .expect_err("contract-invalid JSON should fail at parse boundary");

        assert!(err.contains("invalid SimpleIR contract"));
        assert!(err.contains("unsupported container_type `list_int`"));
    }

    #[test]
    fn simple_ir_from_json_str_accepts_static_module_class_binding_effect_proof() {
        let ir = SimpleIR::from_json_str(
            r#"{
                "functions": [
                    {
                        "name": "__main__",
                        "params": [],
                        "ops": [
                            {"kind": "const_str", "s_value": "__main__", "out": "v0"},
                            {
                                "kind": "module_cache_get",
                                "args": ["v0"],
                                "out": "v1",
                                "effect_proof": "static_module_class_binding"
                            },
                            {"kind": "const_str", "s_value": "Point", "out": "v2"},
                            {
                                "kind": "module_get_attr",
                                "args": ["v1", "v2"],
                                "out": "v3",
                                "effect_proof": "static_module_class_binding"
                            }
                        ]
                    }
                ]
            }"#,
        )
        .expect("static module/class binding proof should validate on module reads");

        assert_eq!(
            ir.functions[0].ops[1].effect_proof.as_deref(),
            Some("static_module_class_binding")
        );
        assert_eq!(
            ir.functions[0].ops[3].effect_proof.as_deref(),
            Some("static_module_class_binding")
        );
    }

    #[test]
    fn simple_ir_from_json_str_rejects_effect_proof_on_non_module_read() {
        let err = SimpleIR::from_json_str(
            r#"{
                "functions": [
                    {
                        "name": "__main__",
                        "params": [],
                        "ops": [
                            {
                                "kind": "const_str",
                                "s_value": "Point",
                                "out": "v0",
                                "effect_proof": "static_module_class_binding"
                            }
                        ]
                    }
                ]
            }"#,
        )
        .expect_err("effect proof should be rejected on non-module reads");

        assert!(err.contains("cannot carry effect_proof `static_module_class_binding`"));
    }

    #[test]
    fn simple_ir_from_json_str_accepts_native_callable_invoke_ffi_metadata() {
        let ir = SimpleIR::from_json_str(
            r#"{
                "functions": [
                    {
                        "name": "__main__",
                        "params": ["arg0"],
                        "ops": [
                            {
                                "kind": "invoke_ffi",
                                "args": ["arg0"],
                                "out": "result",
                                "native_callable_export": "scipy.ndimage.distance_transform_edt",
                                "native_callable_binding": "direct_symbol",
                                "native_callable_symbol": "molt_scipy_ndimage_distance_transform_edt",
                                "native_callable_abi": "molt.forward_f32_v1"
                            }
                        ]
                    }
                ]
            }"#,
        )
        .expect("native callable invoke_ffi metadata should validate");

        let op = &ir.functions[0].ops[0];
        assert_eq!(
            op.native_callable_export.as_deref(),
            Some("scipy.ndimage.distance_transform_edt")
        );
        assert_eq!(op.native_callable_binding.as_deref(), Some("direct_symbol"));
        assert_eq!(
            op.native_callable_symbol.as_deref(),
            Some("molt_scipy_ndimage_distance_transform_edt")
        );
        assert_eq!(
            op.native_callable_abi.as_deref(),
            Some("molt.forward_f32_v1")
        );
    }

    #[test]
    fn simple_ir_from_json_str_rejects_native_callable_metadata_on_non_invoke_ffi() {
        let err = SimpleIR::from_json_str(
            r#"{
                "functions": [
                    {
                        "name": "__main__",
                        "params": ["x"],
                        "ops": [
                            {
                                "kind": "call",
                                "s_value": "scipy__ndimage__distance_transform_edt",
                                "args": ["x"],
                                "out": "result",
                                "native_callable_export": "scipy.ndimage.distance_transform_edt",
                                "native_callable_binding": "module_attr",
                                "native_callable_abi": "molt.forward_f32_v1"
                            }
                        ]
                    }
                ]
            }"#,
        )
        .expect_err("native callable metadata belongs only on invoke_ffi");

        assert!(err.contains("op `call` cannot carry native callable export metadata"));
    }

    #[test]
    fn simple_ir_from_json_str_rejects_unknown_native_callable_abi() {
        let err = SimpleIR::from_json_str(
            r#"{
                "functions": [
                    {
                        "name": "__main__",
                        "params": ["arg0"],
                        "ops": [
                            {
                                "kind": "invoke_ffi",
                                "args": ["arg0"],
                                "out": "result",
                                "native_callable_export": "scipy.ndimage.distance_transform_edt",
                                "native_callable_binding": "direct_symbol",
                                "native_callable_symbol": "molt_scipy_ndimage_distance_transform_edt",
                                "native_callable_abi": "molt.forward_f33_v1"
                            }
                        ]
                    }
                ]
            }"#,
        )
        .expect_err("native callable ABI tokens must be canonical");

        assert!(err.contains("unknown native_callable_abi `molt.forward_f33_v1`"));
        assert!(err.contains("molt.object_call_v1, molt.forward_f32_v1"));
    }

    #[test]
    fn simple_ir_from_json_str_rejects_direct_symbol_without_native_symbol() {
        let err = SimpleIR::from_json_str(
            r#"{
                "functions": [
                    {
                        "name": "__main__",
                        "params": ["arg0"],
                        "ops": [
                            {
                                "kind": "invoke_ffi",
                                "args": ["arg0"],
                                "out": "result",
                                "native_callable_export": "scipy.ndimage.distance_transform_edt",
                                "native_callable_binding": "direct_symbol",
                                "native_callable_abi": "molt.forward_f32_v1"
                            }
                        ]
                    }
                ]
            }"#,
        )
        .expect_err("direct_symbol callable exports require a native symbol");

        assert!(err.contains("direct_symbol requires native_callable_symbol"));
    }

    #[test]
    fn simple_ir_from_json_str_accepts_legacy_var_names_at_transport_boundary() {
        let ir = SimpleIR::from_json_str(
            r#"{
                "functions": [
                    {
                        "name": "__main__",
                        "params": [],
                        "ops": [
                            {"kind": "const", "value": 1, "out": "v0"},
                            {"kind": "store_var", "var": "future_features", "args": ["v0"]},
                            {"kind": "ret_void"}
                        ]
                    }
                ]
            }"#,
        )
        .expect("transport boundary should accept legacy variable identifiers");

        assert_eq!(ir.functions.len(), 1);
    }

    #[test]
    fn serde_deserialize_rejects_contract_violations() {
        let err = serde_json::from_str::<SimpleIR>(
            r#"{
                "functions": [
                    {
                        "name": "__main__",
                        "params": ["x"],
                        "param_types": ["int", "bool"],
                        "ops": [{"kind": "ret_void"}]
                    }
                ]
            }"#,
        )
        .expect_err("serde SimpleIR boundary should validate contracts");

        assert!(err.to_string().contains("invalid SimpleIR contract"));
        assert!(err.to_string().contains("has 1 params but 2 param_types"));
    }

    #[test]
    fn ndjson_reader_parses_stream() {
        let input = r#"{"kind":"ir_stream_start","profile":null}
{"kind":"function","name":"molt_main","params":[],"ops":[{"kind":"ret_void"}]}
{"kind":"function","name":"helper","params":["a"],"ops":[{"kind":"return","args":["a"]}]}
{"kind":"ir_stream_end"}
"#;
        let reader = std::io::BufReader::new(input.as_bytes());
        let ir = SimpleIR::from_ndjson_reader(reader).expect("ndjson parse");
        assert_eq!(ir.functions.len(), 2);
        assert_eq!(ir.functions[0].name, "molt_main");
        assert_eq!(ir.functions[1].name, "helper");
        assert!(ir.profile.is_none());
    }

    #[test]
    fn ndjson_reader_parses_profile() {
        let input = r#"{"kind":"ir_stream_start","profile":{"version":"v1","hot_functions":["f"]}}
{"kind":"function","name":"f","params":[],"ops":[{"kind":"ret_void"}]}
{"kind":"ir_stream_end"}
"#;
        let reader = std::io::BufReader::new(input.as_bytes());
        let ir = SimpleIR::from_ndjson_reader(reader).expect("ndjson parse");
        assert_eq!(ir.functions.len(), 1);
        let profile = ir.profile.expect("profile");
        assert_eq!(profile.version.as_deref(), Some("v1"));
        assert_eq!(profile.hot_functions, vec!["f"]);
    }

    #[test]
    fn ndjson_reader_skips_blank_lines_and_unknown_kinds() {
        let input = r#"{"kind":"ir_stream_start","profile":null}

{"kind":"unknown_future_thing","data":123}
{"kind":"function","name":"main","params":[],"ops":[{"kind":"ret_void"}]}
{"kind":"ir_stream_end"}
"#;
        let reader = std::io::BufReader::new(input.as_bytes());
        let ir = SimpleIR::from_ndjson_reader(reader).expect("ndjson parse");
        assert_eq!(ir.functions.len(), 1);
    }

    #[test]
    fn ndjson_reader_handles_empty_stream() {
        let input = r#"{"kind":"ir_stream_start","profile":null}
{"kind":"ir_stream_end"}
"#;
        let reader = std::io::BufReader::new(input.as_bytes());
        let ir = SimpleIR::from_ndjson_reader(reader).expect("ndjson parse");
        assert_eq!(ir.functions.len(), 0);
        assert!(ir.profile.is_none());
    }

    #[test]
    fn ndjson_reader_rejects_contract_violations() {
        let input = r#"{"kind":"ir_stream_start","profile":null}
{"kind":"function","name":"__main__","params":["x"],"param_types":["int","bool"],"ops":[{"kind":"ret_void"}]}
{"kind":"ir_stream_end"}
"#;
        let reader = std::io::BufReader::new(input.as_bytes());
        let err =
            SimpleIR::from_ndjson_reader(reader).expect_err("NDJSON boundary should validate");

        assert!(err.contains("invalid SimpleIR contract"));
        assert!(err.contains("has 1 params but 2 param_types"));
    }
}
