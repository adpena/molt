use super::*;

#[derive(Default)]
pub(super) struct WasmMultiReturnLayout {
    callee_return_count: Option<usize>,
    callee_value_locals: Vec<u32>,
    callee_tuple_vars: BTreeSet<String>,
    call_value_locals: BTreeMap<(String, i64), u32>,
    call_tuple_vars: BTreeSet<String>,
}

impl WasmMultiReturnLayout {
    pub(super) fn build(
        func_ir: &FunctionIR,
        multi_return_candidates: &BTreeMap<String, usize>,
        locals: &mut WasmFrameLocals,
        local_types: &mut Vec<ValType>,
        local_count: &mut u32,
    ) -> Self {
        let callee_return_count = multi_return_candidates.get(&func_ir.name).copied();
        let mut callee_value_locals = Vec::new();
        let mut callee_tuple_vars = BTreeSet::new();

        if let Some(ret_count) = callee_return_count {
            for i in 0..ret_count {
                let local_idx =
                    locals.ensure_multi_return_callee_value(i, local_types, local_count);
                callee_value_locals.push(local_idx);
            }
            for op in &func_ir.ops {
                if op.kind == "tuple_new"
                    && let Some(args) = &op.args
                    && args.len() == ret_count
                    && let Some(out) = &op.out
                {
                    callee_tuple_vars.insert(out.clone());
                }
            }
        }

        let mut call_value_locals = BTreeMap::new();
        let mut call_tuple_vars = BTreeSet::new();
        for (op_idx, op) in func_ir.ops.iter().enumerate() {
            if op.kind != "call_internal" {
                continue;
            }
            let Some(callee) = op.s_value.as_ref() else {
                continue;
            };
            let Some(&ret_count) = multi_return_candidates.get(callee) else {
                continue;
            };
            let Some(result_var) = op.out.as_ref() else {
                continue;
            };
            if !tuple_indexes_immediately_follow(func_ir, op_idx, result_var, ret_count) {
                continue;
            }
            call_tuple_vars.insert(result_var.clone());
            for k in 0..ret_count {
                let local_idx =
                    locals.ensure_multi_return_call_value(result_var, k, local_types, local_count);
                call_value_locals.insert((result_var.clone(), k as i64), local_idx);
            }
        }

        Self {
            callee_return_count,
            callee_value_locals,
            callee_tuple_vars,
            call_value_locals,
            call_tuple_vars,
        }
    }

    pub(super) fn is_callee_tuple_var(&self, out_name: &str) -> bool {
        self.callee_return_count.is_some() && self.callee_tuple_vars.contains(out_name)
    }

    pub(super) fn callee_value_locals(&self) -> &[u32] {
        &self.callee_value_locals
    }

    pub(super) fn is_promoted_call_tuple(&self, tuple_var: &str) -> bool {
        self.call_tuple_vars.contains(tuple_var)
    }

    pub(super) fn promoted_call_value_local(&self, tuple_var: &str, index: i64) -> Option<u32> {
        self.call_value_locals
            .get(&(tuple_var.to_string(), index))
            .copied()
    }
}

fn tuple_indexes_immediately_follow(
    func_ir: &FunctionIR,
    op_idx: usize,
    result_var: &str,
    ret_count: usize,
) -> bool {
    for k in 0..ret_count {
        let j = op_idx + 1 + k;
        let Some(next_op) = func_ir.ops.get(j) else {
            return false;
        };
        if next_op.kind != "tuple_index" {
            return false;
        }
        let Some(args) = next_op.args.as_ref() else {
            return false;
        };
        if args.len() < 2 || args[0] != result_var {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidates(entries: &[(&str, usize)]) -> BTreeMap<String, usize> {
        entries
            .iter()
            .map(|(name, count)| ((*name).to_string(), *count))
            .collect()
    }

    fn op(kind: &str) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            ..OpIR::default()
        }
    }

    fn tuple_new(args: &[&str], out: &str) -> OpIR {
        OpIR {
            kind: "tuple_new".to_string(),
            args: Some(args.iter().map(|arg| (*arg).to_string()).collect()),
            out: Some(out.to_string()),
            ..OpIR::default()
        }
    }

    fn call_internal(callee: &str, out: &str) -> OpIR {
        OpIR {
            kind: "call_internal".to_string(),
            s_value: Some(callee.to_string()),
            args: Some(Vec::new()),
            out: Some(out.to_string()),
            ..OpIR::default()
        }
    }

    fn tuple_index(tuple: &str, index: i64, out: &str) -> OpIR {
        OpIR {
            kind: "tuple_index".to_string(),
            value: Some(index),
            args: Some(vec![tuple.to_string(), index.to_string()]),
            out: Some(out.to_string()),
            ..OpIR::default()
        }
    }

    #[test]
    fn callee_layout_promotes_return_tuple_locals() {
        let func_ir = FunctionIR {
            name: "split_pair".to_string(),
            params: vec!["left".to_string(), "right".to_string()],
            ops: vec![
                tuple_new(&["left", "right"], "pair"),
                OpIR {
                    kind: "ret".to_string(),
                    var: Some("pair".to_string()),
                    ..OpIR::default()
                },
            ],
            ..FunctionIR::default()
        };
        let mut locals = WasmFrameLocals::from(BTreeMap::from([
            ("left".to_string(), 0),
            ("right".to_string(), 1),
        ]));
        let mut local_types = Vec::new();
        let mut local_count = 2;

        let layout = WasmMultiReturnLayout::build(
            &func_ir,
            &candidates(&[("split_pair", 2)]),
            &mut locals,
            &mut local_types,
            &mut local_count,
        );

        assert!(layout.is_callee_tuple_var("pair"));
        assert_eq!(layout.callee_value_locals(), &[2, 3]);
        assert_eq!(
            locals.local_kind("__multi_ret_0"),
            Some(WasmFrameLocalKind::MultiReturnCalleeValue)
        );
        assert_eq!(
            locals.local_kind("__multi_ret_1"),
            Some(WasmFrameLocalKind::MultiReturnCalleeValue)
        );
        assert_eq!(local_types, vec![ValType::I64, ValType::I64]);
        assert_eq!(local_count, 4);
    }

    #[test]
    fn caller_layout_promotes_immediate_tuple_indexes() {
        let func_ir = FunctionIR {
            name: "caller".to_string(),
            ops: vec![
                call_internal("split_pair", "pair"),
                tuple_index("pair", 0, "left"),
                tuple_index("pair", 1, "right"),
            ],
            ..FunctionIR::default()
        };
        let mut locals = WasmFrameLocals::new();
        let mut local_types = Vec::new();
        let mut local_count = 0;

        let layout = WasmMultiReturnLayout::build(
            &func_ir,
            &candidates(&[("split_pair", 2)]),
            &mut locals,
            &mut local_types,
            &mut local_count,
        );

        assert!(layout.is_promoted_call_tuple("pair"));
        assert_eq!(layout.promoted_call_value_local("pair", 0), Some(0));
        assert_eq!(layout.promoted_call_value_local("pair", 1), Some(1));
        assert_eq!(
            locals.local_kind("__multi_call_pair_0"),
            Some(WasmFrameLocalKind::MultiReturnCallValue)
        );
        assert_eq!(
            locals.local_kind("__multi_call_pair_1"),
            Some(WasmFrameLocalKind::MultiReturnCallValue)
        );
        assert_eq!(local_types, vec![ValType::I64, ValType::I64]);
        assert_eq!(local_count, 2);
    }

    #[test]
    fn caller_layout_requires_consecutive_tuple_indexes() {
        let func_ir = FunctionIR {
            name: "caller".to_string(),
            ops: vec![
                call_internal("split_pair", "pair"),
                op("const"),
                tuple_index("pair", 0, "left"),
                tuple_index("pair", 1, "right"),
            ],
            ..FunctionIR::default()
        };
        let mut locals = WasmFrameLocals::new();
        let mut local_types = Vec::new();
        let mut local_count = 0;

        let layout = WasmMultiReturnLayout::build(
            &func_ir,
            &candidates(&[("split_pair", 2)]),
            &mut locals,
            &mut local_types,
            &mut local_count,
        );

        assert!(!layout.is_promoted_call_tuple("pair"));
        assert_eq!(layout.promoted_call_value_local("pair", 0), None);
        assert!(local_types.is_empty());
        assert_eq!(local_count, 0);
    }
}
