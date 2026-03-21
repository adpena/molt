use crate::ir_schema;
use crate::json_boundary::{
    expect_object, optional_bool, optional_bytes, optional_f64, optional_i64, optional_string,
    optional_string_list, required_field, required_string, required_string_list,
};
use serde_json::Value as JsonValue;
use std::collections::{HashMap, HashSet, VecDeque};

/// Per-branch PGO counter: how many times the true/false sides were taken.
#[derive(Debug, Clone)]
pub struct PgoBranchCount {
    pub taken: u64,
    pub not_taken: u64,
}

/// Per-function PGO counter: how many times it was called at runtime.
#[derive(Debug, Clone)]
pub struct PgoCallCount {
    pub calls: u64,
}

/// Per-loop PGO counter: average and max iteration counts.
#[derive(Debug, Clone)]
pub struct PgoLoopCount {
    pub avg_iterations: f64,
    pub max_iterations: u64,
}

#[derive(Debug, Default, Clone)]
pub struct PgoProfileIR {
    pub version: Option<String>,
    pub hash: Option<String>,
    pub hot_functions: Vec<String>,
    /// Branch counts keyed by "function_name:op_index" (e.g. "molt_main:42").
    pub branch_counts: HashMap<String, PgoBranchCount>,
    /// Call counts keyed by function name.
    pub call_counts: HashMap<String, PgoCallCount>,
    /// Loop iteration counts keyed by "function_name:op_index".
    pub loop_counts: HashMap<String, PgoLoopCount>,
}

#[derive(Debug)]
pub struct SimpleIR {
    pub functions: Vec<FunctionIR>,
    pub profile: Option<PgoProfileIR>,
}

#[derive(Debug)]
pub struct FunctionIR {
    pub name: String,
    pub params: Vec<String>,
    pub ops: Vec<OpIR>,
    pub param_types: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default)]
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
}

impl PgoBranchCount {
    /// Returns the probability (0.0..=1.0) that the true branch is taken.
    pub fn taken_ratio(&self) -> f64 {
        let total = self.taken + self.not_taken;
        if total == 0 {
            0.5
        } else {
            self.taken as f64 / total as f64
        }
    }
}

impl PgoProfileIR {
    fn from_json_value(value: &JsonValue, ctx: &str) -> Result<Self, String> {
        let obj = expect_object(value, ctx)?;
        let branch_counts = Self::parse_branch_counts(obj);
        let call_counts = Self::parse_call_counts(obj);
        let loop_counts = Self::parse_loop_counts(obj);
        Ok(Self {
            version: optional_string(obj, "version", ctx)?,
            hash: optional_string(obj, "hash", ctx)?,
            hot_functions: optional_string_list(obj, "hot_functions", ctx)?.unwrap_or_default(),
            branch_counts,
            call_counts,
            loop_counts,
        })
    }

    fn parse_branch_counts(
        obj: &serde_json::Map<String, JsonValue>,
    ) -> HashMap<String, PgoBranchCount> {
        let mut result = HashMap::new();
        let Some(JsonValue::Object(branches)) = obj.get("branch_counts") else {
            return result;
        };
        for (key, val) in branches {
            let Some(entry) = val.as_object() else {
                continue;
            };
            let taken = entry
                .get("taken")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let not_taken = entry
                .get("not_taken")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            result.insert(key.clone(), PgoBranchCount { taken, not_taken });
        }
        result
    }

    fn parse_call_counts(
        obj: &serde_json::Map<String, JsonValue>,
    ) -> HashMap<String, PgoCallCount> {
        let mut result = HashMap::new();
        let Some(JsonValue::Object(calls)) = obj.get("call_counts") else {
            return result;
        };
        for (key, val) in calls {
            let calls = if let Some(n) = val.as_u64() {
                n
            } else if let Some(entry) = val.as_object() {
                entry.get("calls").and_then(|v| v.as_u64()).unwrap_or(0)
            } else {
                continue;
            };
            result.insert(key.clone(), PgoCallCount { calls });
        }
        result
    }

    fn parse_loop_counts(
        obj: &serde_json::Map<String, JsonValue>,
    ) -> HashMap<String, PgoLoopCount> {
        let mut result = HashMap::new();
        let Some(JsonValue::Object(loops)) = obj.get("loop_counts") else {
            return result;
        };
        for (key, val) in loops {
            let Some(entry) = val.as_object() else {
                continue;
            };
            let avg_iterations = entry
                .get("avg_iterations")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let max_iterations = entry
                .get("max_iterations")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            result.insert(
                key.clone(),
                PgoLoopCount {
                    avg_iterations,
                    max_iterations,
                },
            );
        }
        result
    }

    /// Look up the branch count for a given function and op index.
    pub fn get_branch_count(&self, func_name: &str, op_idx: usize) -> Option<&PgoBranchCount> {
        let key = format!("{func_name}:{op_idx}");
        self.branch_counts.get(&key)
    }

    /// Look up call count for a given function.
    pub fn get_call_count(&self, func_name: &str) -> Option<u64> {
        self.call_counts.get(func_name).map(|c| c.calls)
    }

    /// Look up loop iteration counts for a given function and op index.
    pub fn get_loop_count(&self, func_name: &str, op_idx: usize) -> Option<&PgoLoopCount> {
        let key = format!("{func_name}:{op_idx}");
        self.loop_counts.get(&key)
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

        let known_functions: HashSet<&str> = self
            .functions
            .iter()
            .map(|func| func.name.as_str())
            .collect();
        let mut reachable: HashSet<String> = HashSet::from([String::from("molt_main")]);
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
        let mut defined: HashSet<&str> = func.params.iter().map(String::as_str).collect();
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
        assert!(profile.branch_counts.is_empty());
        assert!(profile.call_counts.is_empty());
        assert!(profile.loop_counts.is_empty());
    }

    #[test]
    fn simple_ir_from_json_str_parses_pgo_branch_counts() {
        let ir = SimpleIR::from_json_str(
            r#"{
                "functions": [
                    {
                        "name": "molt_main",
                        "params": [],
                        "ops": [{"kind": "ret_void"}]
                    }
                ],
                "profile": {
                    "version": "v1",
                    "branch_counts": {
                        "molt_main:5": {"taken": 9900, "not_taken": 100},
                        "molt_main:12": {"taken": 10, "not_taken": 990}
                    },
                    "call_counts": {
                        "helper_func": 5000,
                        "rare_func": {"calls": 2}
                    },
                    "loop_counts": {
                        "molt_main:8": {"avg_iterations": 100.5, "max_iterations": 500}
                    }
                }
            }"#,
        )
        .expect("ir parse");

        let profile = ir.profile.expect("profile");

        // Branch counts
        assert_eq!(profile.branch_counts.len(), 2);
        let bc = profile.get_branch_count("molt_main", 5).unwrap();
        assert_eq!(bc.taken, 9900);
        assert_eq!(bc.not_taken, 100);
        assert!(bc.taken_ratio() > 0.98);
        let bc2 = profile.get_branch_count("molt_main", 12).unwrap();
        assert_eq!(bc2.taken, 10);
        assert!(bc2.taken_ratio() < 0.02);

        // Call counts
        assert_eq!(profile.get_call_count("helper_func"), Some(5000));
        assert_eq!(profile.get_call_count("rare_func"), Some(2));
        assert_eq!(profile.get_call_count("nonexistent"), None);

        // Loop counts
        let lc = profile.get_loop_count("molt_main", 8).unwrap();
        assert!((lc.avg_iterations - 100.5).abs() < 0.01);
        assert_eq!(lc.max_iterations, 500);
        assert!(profile.get_loop_count("molt_main", 99).is_none());
    }

    #[test]
    fn pgo_branch_count_taken_ratio_handles_zero_total() {
        let bc = super::PgoBranchCount {
            taken: 0,
            not_taken: 0,
        };
        assert!((bc.taken_ratio() - 0.5).abs() < 0.001);
    }
}
