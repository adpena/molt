use serde::Deserialize;

use crate::ir_schema;
use crate::json_boundary::{
    expect_object, optional_bool, optional_bytes, optional_f64, optional_i64, optional_string,
    optional_string_list, required_field, required_string, required_string_list,
};
use serde_json::Value as JsonValue;
use std::collections::{BTreeSet, VecDeque};

#[derive(Debug, Default, Clone, Deserialize)]
#[cfg_attr(feature = "cbor", derive(serde::Serialize))]
#[serde(default)]
pub struct PgoProfileIR {
    pub version: Option<String>,
    pub hash: Option<String>,
    pub hot_functions: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "cbor", derive(serde::Serialize))]
pub struct SimpleIR {
    pub functions: Vec<FunctionIR>,
    pub profile: Option<PgoProfileIR>,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "cbor", derive(serde::Serialize))]
pub struct FunctionIR {
    pub name: String,
    pub params: Vec<String>,
    pub ops: Vec<OpIR>,
    pub param_types: Option<Vec<String>>,
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
    pub fast_int: Option<bool>,
    pub fast_float: Option<bool>,
    pub raw_int: Option<bool>,
    pub stack_eligible: Option<bool>,
    pub task_kind: Option<String>,
    pub container_type: Option<String>,
    pub type_hint: Option<String>,
    /// Inline-cache site index for attribute access acceleration.
    /// Assigned by the frontend for `get_attr_generic_ptr` ops.
    pub ic_index: Option<i64>,
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
        Ok(Self { functions, profile })
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
                    if let Some(p) = value.get("profile") {
                        if !p.is_null() {
                            profile = Some(PgoProfileIR::from_json_value(p, "stream.profile")?);
                        }
                    }
                }
                Some("function") => {
                    functions.push(FunctionIR::from_json_value(&value, "stream.function")?);
                }
                Some("ir_stream_end") => break,
                _ => {} // skip unknown kinds for forward compat
            }
        }
        Ok(Self { functions, profile })
    }

    pub fn tree_shake_luau(&mut self) {
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
                if op.kind != "call" {
                    continue;
                }
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
            }
        }

        self.functions
            .retain(|func| reachable.contains(func.name.as_str()));
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
            raw_int: optional_bool(obj, "raw_int", ctx)?,
            stack_eligible: optional_bool(obj, "stack_eligible", ctx)?,
            task_kind: optional_string(obj, "task_kind", ctx)?,
            container_type: optional_string(obj, "container_type", ctx)?,
            type_hint: optional_string(obj, "type_hint", ctx)?,
            ic_index,
        })
    }
}

const RAW_INT_ALLOWED_OP_KINDS: &[&str] = &[
    "add",
    "box_from_raw_int",
    "const",
    "loop_index_next",
    "loop_index_start",
    "lt",
    "unbox_to_raw_int",
];

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
    for func in &ir.functions {
        let mut defined: BTreeSet<&str> = func.params.iter().map(String::as_str).collect();
        for op in &func.ops {
            ir_schema::validate_required_fields(op)?;
            if op.fast_int == Some(true) && op.raw_int == Some(true) {
                return Err(format!(
                    "op `{}` cannot set both `fast_int` and `raw_int`",
                    op.kind
                ));
            }
            if op.raw_int == Some(true) && !RAW_INT_ALLOWED_OP_KINDS.contains(&op.kind.as_str()) {
                return Err(format!("op `{}` does not support `raw_int`", op.kind));
            }
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
}
