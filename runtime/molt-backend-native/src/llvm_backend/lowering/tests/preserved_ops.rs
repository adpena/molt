use super::*;

/// The preserved-op passthrough-class closure: each kind that previously
/// fell to the `Copy` operand-0 passthrough (a silent miscompile / dropped
/// side effect) must now lower to its dedicated runtime call. This pins the
/// specific dedicated arms whose runtime symbol DIFFERS from `molt_<kind>`
/// (so the generic fallback would have declined) or which are result-less.
#[test]
fn lower_preserved_passthrough_class_routes_to_runtime() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    // (kind, n_operands, with_result, s_value, expected runtime symbol)
    let cases: &[(&str, usize, bool, Option<&str>, &str)] = &[
        ("abs", 1, true, None, "molt_abs_builtin"),
        ("const_ellipsis", 0, true, None, "molt_ellipsis"),
        (
            "const_not_implemented",
            0,
            true,
            None,
            "molt_not_implemented",
        ),
        ("gen_throw", 2, true, None, "molt_generator_throw"),
        ("gen_close", 1, true, None, "molt_generator_close"),
        (
            "exception_set_cause",
            2,
            false,
            None,
            "molt_exception_set_cause",
        ),
        (
            "get_attr_special_obj",
            1,
            true,
            Some("__class__"),
            "molt_get_attr_special",
        ),
        ("borrow", 1, true, None, "molt_inc_ref_obj"),
        ("identity_alias", 1, true, None, "molt_inc_ref_obj"),
        ("binding_alias", 1, true, None, "molt_inc_ref_obj"),
        ("release", 1, true, None, "molt_dec_ref_obj"),
        ("guard_tag", 2, false, None, "molt_guard_type"),
        ("guard_layout", 3, true, None, "molt_guard_layout_ptr"),
        ("guard_dict_shape", 3, true, None, "molt_guard_layout_ptr"),
        ("dataclass_new", 4, true, None, "molt_dataclass_new"),
        ("json_parse", 1, true, None, "molt_json_parse_scalar_obj"),
        (
            "msgpack_parse",
            1,
            true,
            None,
            "molt_msgpack_parse_scalar_obj",
        ),
        ("cbor_parse", 1, true, None, "molt_cbor_parse_scalar_obj"),
        (
            "gen_locals_register",
            2,
            false,
            Some("gen_fn"),
            "molt_gen_locals_register",
        ),
    ];
    for &(kind, nops, with_result, s_value, sym) in cases {
        let ir = lower_preserved_kind_ir(&backend, kind, nops, with_result, s_value)
            .unwrap_or_else(|e| {
                panic!(
                    "preserved `{kind}` must lower, got error: {:?}",
                    e.diagnostics()
                )
            });
        assert!(
            ir.contains(sym),
            "preserved `{kind}` must lower to `{sym}` (not an operand-0 \
                 passthrough); IR:\n{ir}"
        );
    }
}

/// Repr-identity preserved ops (`cast`, `widen`, `store_var`, `copy_var`) are the
/// explicit exception to the terminal preserved-op fail-loud rule: they
/// carry no runtime semantics and must alias operand 0 exactly, matching
/// native/WASM identity lowering over the NaN-boxed value format.
#[test]
fn lower_preserved_repr_identity_ops_pass_operand_through() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    for kind in ["cast", "widen", "store_var", "copy_var"] {
        let mut func = TirFunction::new(
            format!("preserved_{kind}_identity"),
            vec![TirType::DynBox],
            TirType::DynBox,
        );
        let src = func
            .blocks
            .get(&func.entry_block)
            .and_then(|block| block.args.first())
            .map(|arg| arg.id)
            .expect("identity test function must have one entry argument");
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        let mut attrs = AttrDict::new();
        attrs.insert("_original_kind".into(), AttrValue::Str(kind.to_string()));
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![src],
            results: vec![result],
            attrs,
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let ir = try_lower_tir_to_llvm(&func, &backend)
            .map(|f| f.print_to_string().to_string())
            .unwrap_or_else(|e| {
                panic!(
                    "repr-identity preserved `{kind}` must lower as operand-0 \
                         passthrough, got error: {:?}",
                    e.diagnostics()
                )
            });
        assert!(
            !ir.contains("call "),
            "repr-identity preserved `{kind}` must not lower through a runtime call:\n{ir}"
        );
        assert!(
            ir.contains("ret i64 %0"),
            "repr-identity preserved `{kind}` must return operand 0 exactly:\n{ir}"
        );
    }
}

/// Terminal fail-loud state: a preserved `Copy` carrying an `_original_kind`
/// that NO arm and NO `molt_<kind>` runtime intrinsic claims must be a hard
/// `record_fatal` lowering error — never a silent operand-0 passthrough.
/// `__ppaudit_unmapped__` is a synthetic kind that cannot resolve to any
/// `molt_*` symbol, so it must reach the terminal guard.
#[test]
fn lower_preserved_unmapped_kind_fails_loud() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let err = lower_preserved_kind_ir(&backend, "__ppaudit_unmapped__", 1, true, None).expect_err(
        "an unhandled preserved op must fail the lowering, not silently \
                 pass operand 0 through",
    );
    assert_lowering_error_contains(&err, "unhandled preserved SimpleIR op");
    assert_lowering_error_contains(&err, "__ppaudit_unmapped__");
}

/// RESULT-LESS preserved side-effect ops (`print_newline`, `set_update`,
/// `dict_str_int_inc`, …) whose `molt_<kind>` symbol IS in the linked
/// intrinsic surface must lower to that runtime call via the generic
/// fallback — NOT be dropped as a `Copy` "0 results → no-op". The
/// passthrough enumeration found these reaching the no-op branch (a missing
/// newline / a set or dict mutation that never happened). This pins the
/// result-less generic-fallback path; the symbols are injected because the
/// unit-test backend has an empty intrinsic surface by default.
#[test]
fn lower_preserved_resultless_side_effect_routes_to_runtime() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);
    // (kind, n_operands, expected runtime symbol). All result-less (res=0).
    let cases: &[(&str, usize, &str)] = &[
        ("print_newline", 0, "molt_print_newline"),
        ("set_update", 2, "molt_set_update"),
        ("dict_str_int_inc", 3, "molt_dict_str_int_inc"),
        ("spawn", 1, "molt_spawn"),
    ];
    for &(_, _, sym) in cases {
        backend.runtime_callable_symbols.insert(sym.to_string());
    }
    for &(kind, nops, sym) in cases {
        let ir = lower_preserved_kind_ir(&backend, kind, nops, false, None).unwrap_or_else(|e| {
            panic!(
                "result-less preserved `{kind}` must lower, got error: {:?}",
                e.diagnostics()
            )
        });
        assert!(
            ir.contains(sym),
            "result-less preserved `{kind}` must lower to `{sym}` (not a \
                 dropped no-op); IR:\n{ir}"
        );
        if sym == "molt_print_newline" {
            assert!(
                ir.contains("call void @molt_print_newline()"),
                "print_newline must use the runtime's void ABI; IR:\n{ir}"
            );
        }
    }
}

#[test]
fn lower_preserved_chan_new_uses_dedicated_handle_lowering() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let ir = lower_preserved_kind_ir(&backend, "chan_new", 1, true, None).unwrap_or_else(|e| {
        panic!(
            "chan_new returns an opaque channel handle and must lower through \
             its dedicated LLVM arm, got error: {:?}",
            e.diagnostics()
        )
    });
    assert!(
        ir.contains("call i64 @molt_chan_new(i64"),
        "chan_new must call the centrally declared handle constructor; IR:\n{ir}"
    );
}

#[test]
fn lower_preserved_void_runtime_result_shape_fails_loud() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);
    backend
        .runtime_callable_symbols
        .insert("molt_spawn".to_string());
    let err = lower_preserved_kind_ir(&backend, "spawn", 1, true, None)
        .expect_err("void preserved runtime ops must not bind a boxed result");
    assert_lowering_error_contains(&err, "unhandled preserved SimpleIR op");
    assert_lowering_error_contains(&err, "spawn");
}

/// The dual safety check: a result-less preserved op whose `molt_<kind>`
/// symbol is ABSENT from the intrinsic surface must STILL fail loud (never a
/// silent dropped side effect). Without the symbol the generic fallback
/// declines and the terminal guard must fire.
#[test]
fn lower_preserved_resultless_unmapped_fails_loud() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let err = lower_preserved_kind_ir(&backend, "__ppaudit_resultless__", 2, false, None)
        .expect_err("an unhandled result-less preserved op must fail the lowering");
    assert_lowering_error_contains(&err, "unhandled preserved SimpleIR op");
    assert_lowering_error_contains(&err, "__ppaudit_resultless__");
}

/// A bare `Copy` (no `_original_kind` — a genuine SSA value copy such as
/// `copy`/`load_var`/`store_var`) must STILL take the benign operand-0
/// passthrough. The terminal fail-loud guard keys on `_original_kind`, so it
/// must not fire here.
#[test]
fn lower_bare_copy_without_original_kind_passes_through() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("bare_copy".into(), vec![], TirType::DynBox);
    let src = func.fresh_value();
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_none_def(src));
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![src],
        results: vec![result],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };
    // Must lower cleanly (no fatal); the result aliases the source.
    let ir = try_lower_tir_to_llvm(&func, &backend)
        .map(|f| f.print_to_string().to_string())
        .expect("a bare Copy without _original_kind must lower as a passthrough");
    assert!(
        !ir.contains("unhandled preserved"),
        "bare Copy must not trigger the preserved-op fail-loud: {ir}"
    );
}

#[test]
fn lower_preserved_len_ignores_transport_container_type() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("len_preserved".into(), vec![], TirType::DynBox);
    let obj = func.fresh_value();
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_none_def(obj));
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![obj],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert("_original_kind".into(), AttrValue::Str("len".into()));
            attrs.insert("container_type".into(), AttrValue::Str("tuple".into()));
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("call i64 @molt_len("), "{ir}");
    assert!(!ir.contains("call i64 @molt_len_tuple("), "{ir}");
}

#[test]
fn lower_preserved_len_uses_tir_tuple_fact() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "len_typed_tuple".into(),
        vec![TirType::Tuple(vec![TirType::DynBox, TirType::DynBox])],
        TirType::DynBox,
    );
    let obj = ValueId(0);
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![obj],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert("_original_kind".into(), AttrValue::Str("len".into()));
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("call i64 @molt_len_tuple("), "{ir}");
}

#[test]
fn lower_preserved_list_append_calls_runtime() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("list_append_preserved".into(), vec![], TirType::DynBox);
    let list_bits = func.fresh_value();
    let item_bits = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .extend([const_none_def(list_bits), const_none_def(item_bits)]);
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![list_bits, item_bits],
        results: vec![],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert(
                "_original_kind".into(),
                AttrValue::Str("list_append".into()),
            );
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return { values: vec![] };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_list_append"), "{ir}");
}

#[test]
fn lower_del_boundary_calls_dec_ref_runtime() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("del_boundary_release".into(), vec![], TirType::DynBox);
    let owned = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_none_def(owned));
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::DelBoundary,
        operands: vec![owned],
        results: vec![],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return { values: vec![] };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_dec_ref_obj"), "{ir}");
}

#[test]
fn lower_preserved_list_pop_calls_runtime() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);
    backend
        .runtime_callable_symbols
        .insert("molt_list_pop".to_string());
    let ir = lower_preserved_kind_ir(&backend, "list_pop", 2, true, None)
        .expect("list_pop must lower through the boxed runtime call");
    assert!(ir.contains("molt_list_pop"), "{ir}");
}

#[test]
fn lower_preserved_dataclass_new_values_calls_runtime_with_value_slice() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let ir = lower_preserved_kind_ir(&backend, "dataclass_new_values", 5, true, None)
        .expect("dataclass_new_values must lower through its value-slice runtime call");
    assert!(ir.contains("molt_dataclass_new_from_values"), "{ir}");
    assert!(ir.contains("alloca i64, i64 2"), "{ir}");
}

#[test]
fn lower_preserved_tuple_from_list_calls_runtime() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("tuple_from_list_preserved".into(), vec![], TirType::DynBox);
    let list_bits = func.fresh_value();
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_none_def(list_bits));
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![list_bits],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert(
                "_original_kind".into(),
                AttrValue::Str("tuple_from_list".into()),
            );
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_tuple_from_list"), "{ir}");
}

#[test]
fn lower_preserved_set_add_calls_runtime() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("set_add_preserved".into(), vec![], TirType::DynBox);
    let set_bits = func.fresh_value();
    let item_bits = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .extend([const_none_def(set_bits), const_none_def(item_bits)]);
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![set_bits, item_bits],
        results: vec![],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert("_original_kind".into(), AttrValue::Str("set_add".into()));
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return { values: vec![] };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_set_add"), "{ir}");
}

#[test]
fn lower_preserved_list_extend_calls_runtime() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("list_extend_preserved".into(), vec![], TirType::DynBox);
    let list_bits = func.fresh_value();
    let other_bits = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .extend([const_none_def(list_bits), const_none_def(other_bits)]);
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![list_bits, other_bits],
        results: vec![],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert(
                "_original_kind".into(),
                AttrValue::Str("list_extend".into()),
            );
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return { values: vec![] };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_list_extend"), "{ir}");
}

#[test]
fn lower_preserved_aiter_calls_runtime() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("aiter_preserved".into(), vec![], TirType::DynBox);
    let obj_bits = func.fresh_value();
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_none_def(obj_bits));
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![obj_bits],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert("_original_kind".into(), AttrValue::Str("aiter".into()));
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_aiter"), "{ir}");
}

#[test]
fn lower_preserved_gen_send_calls_runtime() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("gen_send_preserved".into(), vec![], TirType::DynBox);
    let gen_bits = func.fresh_value();
    let send_bits = func.fresh_value();
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .extend([const_none_def(gen_bits), const_none_def(send_bits)]);
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![gen_bits, send_bits],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert("_original_kind".into(), AttrValue::Str("gen_send".into()));
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_generator_send"), "{ir}");
}

#[test]
fn lower_preserved_context_exit_calls_runtime() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("context_exit_preserved".into(), vec![], TirType::DynBox);
    let ctx_bits = func.fresh_value();
    let exc_bits = func.fresh_value();
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .extend([const_none_def(ctx_bits), const_none_def(exc_bits)]);
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![ctx_bits, exc_bits],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert(
                "_original_kind".into(),
                AttrValue::Str("context_exit".into()),
            );
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_context_exit"), "{ir}");
}

#[test]
fn lower_preserved_super_new_calls_runtime() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("super_new_preserved".into(), vec![], TirType::DynBox);
    let type_bits = func.fresh_value();
    let obj_bits = func.fresh_value();
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .extend([const_none_def(type_bits), const_none_def(obj_bits)]);
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![type_bits, obj_bits],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert("_original_kind".into(), AttrValue::Str("super_new".into()));
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_super_new"), "{ir}");
}
