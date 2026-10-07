"""Source-bound branch domains for the existing Rust refusal audit.

This is a closed recognizer of immutable admission, generated projections and
guard implications, not a second opcode support list. An unfamiliar source
shape grants no exclusion. Offsets identify individual unreachable call sites;
there is no method waiver. Both graph directions must use these same edges.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
import re

from molt.rust_source_scan import (
    scan_memo,
    mask_rust_comments_and_strings,
    mask_rust_test_items,
    rust_source_tokens as _tokens,
    rust_token_range as _locate,
    rust_delimiter_end as _closing,
    rust_block_region as _block,
    rust_match_arms as match_arms,
    rust_literal_pattern_names as _literal_pattern,
)
from tools.structural_audit_rust_admission import (
    AdmittedWireDomain,
    _body,
    _compact,
    _generated_literal_table,
)


def _words(text: str) -> tuple[str, ...]:
    return tuple(token for token, _, _ in _tokens(text))


def _same(left: str | None, right: str) -> bool:
    return left is not None and _words(left) == _words(right)


def _starts(text: str, prefix: str) -> bool:
    wanted = _words(prefix)
    return _words(text)[: len(wanted)] == wanted


def _no_success_bypass(text: str) -> bool:
    code = mask_rust_comments_and_strings(text)
    return not re.search(r"\b(?:continue|break|unsafe)\b|\breturn\s+(?!Err\s*\()", code)


def _unshadowed(text: str, names: tuple[str, ...]) -> bool:
    code = mask_rust_comments_and_strings(text)
    return not any(
        re.search(
            rf"\blet\s+(?:mut\s+)?{name}\b|\b{name}\s*=(?!=)|&\s*mut\s+{name}\b|\b{name}\s*\.\s*\w+\s*=(?!=)",
            code,
        )
        for name in names
    ) and "unsafe" not in _words(text)


def _bool_table(text: str, name: str) -> frozenset[str] | None:
    body = _body(text, name)
    if body is None:
        return None
    tokens = _words(body)
    if tokens[:5] != ("matches", "!", "(", "kind", ",") or tokens[-1:] != (")",):
        return None
    return _literal_pattern(" ".join(tokens[5:-1]))


def _enum_table(text: str, name: str, enum: str, default: str) -> dict[str, str] | None:
    return _generated_literal_table(
        _body(text, name), rf"{enum}::(?P<value>[A-Za-z]+)", f"_=>{enum}::{default},"
    )


def _literal_shapes(generated: str, literal: str) -> dict[str, str] | None:
    kinds = _generated_literal_table(
        _body(generated, "kind_to_opcode_table"),
        r"Some\(OpCode::(?P<value>[A-Za-z0-9]+)\)",
        "_=>None,",
    )
    body = _body(generated, "opcode_literal_payload_kind_table")
    arms = match_arms(body or "", "opcode")
    if (
        kinds is None
        or arms is None
        or _words(body or "")[:3] != ("match", "opcode", "{")
    ):
        return None
    shapes = {}
    for arm in arms:
        pattern = re.fullmatch(r"OpCode::([A-Za-z0-9]+)", "".join(_words(arm.pattern)))
        rhs = "".join(_words(arm.body))
        if pattern is None or pattern[1] in shapes:
            return None
        if rhs == "None":
            shapes[pattern[1]] = None
            continue
        scalar = re.fullmatch(
            r"Some\(LiteralPayloadKind::(?P<value>Int|Float|Bool|None),?\)", rhs
        )
        owned = re.fullmatch(
            r"Some\(LiteralPayloadKind::Owned\(OwnedLiteralPayloadKind::(?P<value>String|Bytes|BigintDecimal),?\),?\)",
            rhs,
        )
        if scalar is not None:
            shapes[pattern[1]] = scalar["value"]
        elif owned is not None:
            shapes[pattern[1]] = f"Owned(OwnedLiteralPayloadKind::{owned['value']})"
        else:
            return None
    if not set(kinds.values()) <= shapes.keys():
        return None
    # There are two from_simple methods. Resolve the exact value owner, never
    # choose one by its unqualified name or by incidental source order.
    owner = _block(literal, "impl<'a> SimpleLiteral<'a>")
    constructor = _body(literal[slice(*owner)] if owner else "", "from_simple")
    if _compact(constructor or "") != _compact("""
        let Some(shape) = kind_to_opcode_table(&op.kind).and_then(opcode_literal_payload_kind_table)
        else { return Ok(None); };
        let literal = match shape {
            LiteralPayloadKind::Int => Self::Int(op.value.ok_or_else(|| format!("", op.kind))?,),
            LiteralPayloadKind::Float => Self::Float(op.f_value.ok_or_else(|| format!("", op.kind))?,),
            LiteralPayloadKind::Bool => Self::Bool(op.value.ok_or_else(|| format!("", op.kind))? != 0,),
            LiteralPayloadKind::None => Self::None,
            LiteralPayloadKind::Owned(kind) => Self::Owned(kind, LiteralPayload::from_simple(op)?),
        };
        Ok(Some(literal))
    """):
        return None
    return {
        kind: shapes[opcode]
        for kind, opcode in kinds.items()
        if shapes[opcode] is not None
    }


def _validation_dominates(admission: str, ir: str, schema: str) -> bool:
    target = _body(admission, "validate_target_contract_with_representation_plan") or ""
    validate = _body(ir, "validate_simple_ir") or ""
    transport = _body(ir, "validate_simple_ir_transport_contract") or ""
    required = _body(schema, "validate_required_fields") or ""
    if not _compact(target).startswith(
        _compact("""
        let target = target_info.target.as_str();
        crate::validate_simple_ir(ir).map_err(|error| format!(""))?;
    """)
    ) or not _starts(validate, "validate_simple_ir_transport_contract(ir)?;"):
        return False
    functions = _block(transport, "for func in &ir.functions", depth=0)
    if functions is None or not _no_success_bypass(transport):
        return False
    function_body = transport[slice(*functions)]
    operations = _block(
        function_body, "for (op_index, op) in func.ops.iter().enumerate()", depth=0
    )
    if operations is None or not _unshadowed(transport, ("ir", "func", "op")):
        return False
    operation_body = function_body[slice(*operations)]
    if not _compact(operation_body).startswith(
        _compact("""
        ir_schema::validate_required_fields(op)
            .map_err(|error| format!("", func.name))?;
    """)
    ):
        return False
    # Mandatory straight-line tail: earlier returns may fail, but may not
    # succeed, skip the op, replace it, or bypass any of these shared validators.
    return (
        _no_success_bypass(required)
        and _unshadowed(required, ("op",))
        and _compact(required).endswith(
            _compact("""
                validate_simple_op_shape(op).map_err(|error| error.to_string())?;
                crate::literal_payload::validate_simple_literal(op)?;
                validate_value_transport(op)?;
                validate_representation_fields(op)
            """)
        )
    )


def _flow_exclusion(
    methods: dict[str, str], generated: str, domain: AdmittedWireDomain
) -> tuple[int, int] | None:
    references = _bool_table(generated, "simpleir_kind_is_verifier_label_reference")
    definitions = _bool_table(generated, "simpleir_kind_is_verifier_label_definition")
    if (
        references is None
        or definitions is None
        or domain.possible & (references | definitions)
    ):
        return None
    body = methods.get("emit_function", "")
    dispatch = _locate(
        body,
        """
        let dispatch = func.ops.iter().any(|op| {
            molt_ir::tir::op_kinds_generated::simpleir_kind_is_verifier_label_reference(&op.kind)
            || molt_ir::tir::op_kinds_generated::simpleir_kind_is_verifier_label_definition(&op.kind,)
        });
    """,
    )
    normalized = _block(body, "let ops = if dispatch", depth=0)
    # The same header occurs in phi analysis and emission. Select only a block
    # whose body is the actual flow call, and leave all other calls untouched.
    flow_call = _locate(
        body, "if let Some(flow) = &flow { self.emit_logical_flow(&ops, flow); }"
    )
    if dispatch is None or normalized is None or flow_call is None:
        return None
    after_normalization = body[normalized[1] + 1 :]
    if not _starts(after_normalization, "else { func.ops.clone() };"):
        return None
    flow = _locate(
        body,
        "let flow = dispatch.then(|| molt_ir::simple_verify::simple_ir_logical_flow(&ops));",
    )
    if (
        flow is None
        or not dispatch[1] < normalized[0] < normalized[1] < flow[0] < flow_call[0]
    ):
        return None
    for name in ("dispatch", "ops", "flow"):
        code = mask_rust_comments_and_strings(body)
        if len(re.findall(rf"\blet\s+(?:mut\s+)?{name}\s*=", code)) != 1:
            return None
        if re.search(
            rf"&\s*mut\s+{name}\b|\b{name}\s*\.\s*(?:iter_mut|push|insert|extend|clear)\s*\(",
            code,
        ):
            return None
    return flow_call


def _immutable_emission(
    methods: dict[str, str], flow_range: tuple[int, int] | None
) -> bool:
    if flow_range is None:
        return False
    assembler = methods.get("assemble_source", "")
    loop = _block(assembler, "for func in &ir.functions", depth=0)
    if loop is None or not _same(
        assembler[slice(*loop)],
        r"""
        self.emit_function(func, plans.and_then(|plans| plans.get(&func.name)));
        self.output.push('\n');
    """,
    ):
        return False
    if not _unshadowed(assembler, ("ir", "func")):
        return False
    for caller, text in methods.items():
        code = mask_rust_comments_and_strings(text)
        for call in re.finditer(
            r"\b(?:self\s*\.|Self\s*::)\s*emit_logical_flow\b", code
        ):
            if (
                caller != "emit_function"
                or not flow_range[0] <= call.start() < flow_range[1]
            ):
                return False
    # Check every incoming operation call, not just the historical dispatcher
    # spelling. A fabricated op or alternate function caller cancels the proof.
    expected = {
        "emit_function": ("assemble_source", r"func\s*,"),
        "emit_op": ("emit_function", r"&\s*ops\s*\[\s*i\s*\]\s*\)"),
    }
    for callee, (owner, arguments) in expected.items():
        for caller, text in methods.items():
            code = mask_rust_comments_and_strings(text)
            for match in re.finditer(rf"\b(?:self\s*\.|Self\s*::)\s*{callee}\b", code):
                if caller == "emit_logical_flow" and callee == "emit_op":
                    continue
                if (
                    caller != owner
                    or "Self" in match[0]
                    or not re.match(r"\s*\(\s*" + arguments, code[match.end() :])
                ):
                    return False
    return _unshadowed(methods.get("emit_op", ""), ("op",))


@dataclass
class BranchProjection:
    excluded: dict[str, list[tuple[int, int]]] = field(default_factory=dict)
    dispatch: list[tuple[int, int]] = field(default_factory=list)
    inbound: dict[str, frozenset[str]] = field(default_factory=dict)
    residual: frozenset[str] | None = None

    def add(self, method: str, region: tuple[int, int]) -> None:
        self.excluded.setdefault(method, []).append(region)

    def excludes(self, method: str, offset: int) -> bool:
        return any(
            start <= offset < end for start, end in self.excluded.get(method, ())
        )


def _dispatch_projection(
    body: str,
    literal_body: str,
    generated: str,
    shapes: dict[str, str] | None,
    domain: AdmittedWireDomain,
    projection: BranchProjection,
) -> None:
    arms = match_arms(body, "op.kind.as_str()")
    if arms is None or _block(body, "match op.kind.as_str()", depth=0) is None:
        return
    possible = set(domain.possible)
    # The helper consumes Some and Err and returns false only for Ok(None).
    # Prove its exact protocol; the rhs arms can still contain refusal edges.
    first = _block(
        literal_body, "let literal = match SimpleLiteral::from_simple(op)", depth=0
    )
    literal_protocol = (
        _starts(
            literal_body,
            """
        use molt_ir::literal_payload::SimpleLiteral;
        use molt_ir::tir::op_kinds_generated::OwnedLiteralPayloadKind;
    """,
        )
        and first is not None
        and _compact(literal_body[slice(*first)])
        == _compact("""
        Ok(Some(literal)) => literal,
        Ok(None) => return false,
        Err(reason) => { self.emit_unsupported_op(op, reason); return true; }
    """)
    )
    tail = match_arms(literal_body, "rhs")
    literal_protocol = (
        literal_protocol
        and tail is not None
        and len(tail) == 2
        and _same(tail[0].pattern, "Ok(rhs)")
        and _same(tail[0].body, "self.emit_literal_value(op, &rhs)")
        and _same(tail[1].pattern, "Err(reason)")
        and _same(tail[1].body, "self.emit_unsupported_op(op, reason)")
        and _words(literal_body)[-1:] == ("true",)
        and len(re.findall(r"\breturn\b", mask_rust_comments_and_strings(literal_body)))
        == 2
    )
    predispatch = _locate(body, "if self.emit_op_literal(op) { return; }")
    if (
        shapes is not None
        and literal_protocol
        and predispatch
        and predispatch[1] <= arms[0].start
        and _block(body, "if self.emit_op_literal(op)", depth=0) is not None
    ):
        possible.difference_update(shapes)
    returns = _enum_table(
        generated, "simpleir_return_shape", "SimpleIrReturnShape", "NotReturn"
    )
    unknown_guard = False
    for index, arm in enumerate(arms):
        kinds = _literal_pattern(arm.pattern)
        if kinds is None:
            for shape in ("Value", "Void"):
                if returns is not None and _same(
                    arm.pattern,
                    f"""
                    kind if molt_ir::tir::op_kinds_generated::simpleir_return_shape(kind)
                    == molt_ir::tir::op_kinds_generated::SimpleIrReturnShape::{shape}
                """,
                ):
                    kinds = frozenset(
                        kind for kind, value in returns.items() if value == shape
                    )
                    break
        if (
            kinds is None
            and _same(arm.pattern, "_")
            and index == len(arms) - 1
            and not unknown_guard
        ):
            kinds = frozenset(possible)
            projection.residual = kinds
        if kinds is None:
            unknown_guard = True
            continue
        inbound = frozenset(possible & kinds)
        if not inbound:
            projection.dispatch.append((arm.start, arm.end))
            projection.add("emit_op", (arm.start, arm.end))
        call = re.fullmatch(
            r"\{?self\.([A-Za-z_]\w*)\(op\);?\}?", "".join(_words(arm.body))
        )
        if call:
            projection.inbound[call[1]] = (
                projection.inbound.get(call[1], frozenset()) | inbound
            )
        possible.difference_update(kinds)
    projection.inbound["emit_op_literal"] = domain.possible


def _value_contract(schema: str) -> bool:
    body = _body(schema, "validate_value_transport") or ""
    return _locate(body, 'name.is_empty() || name == "none"') is not None and _compact(
        body
    ) == _compact("""
        use crate::tir::op_kinds_generated::{
            copy_kind_is_explicit_no_heap_move_table, copy_kind_mints_owned_alias_ref_table,
            opcode_fixed_result_count_table, simpleir_kind_is_structural,
        };
        use crate::tir::simple_def_use::{
            simple_ir_binding, visit_simple_ir_reads, visit_simple_ir_result_names,
        };
        let binding = simpleir_var_field_role_table(&op.kind) == SimpleIrVarFieldRole::Definition;
        if binding {
            let destination = simple_ir_binding(op).map(|binding| binding.destination);
            if destination.is_none_or(|name| name.is_empty() || name == "none") {
                return Err(format!("", op.kind));
            }
        }
        if copy_kind_is_explicit_no_heap_move_table(&op.kind)
            || copy_kind_mints_owned_alias_ref_table(&op.kind)
        {
            let expected = simpleir_op_shape(&op.kind).map_or(1, |shape| shape.operands);
            let mut actual = 0;
            visit_simple_ir_reads(op, |_| actual += 1);
            if actual != expected { return Err(format!("", op.kind)); }
        }
        if simpleir_kind_is_structural(&op.kind)
            || kind_to_opcode_table(&op.kind).and_then(opcode_fixed_result_count_table) == Some(0)
        {
            let mut result = false;
            visit_simple_ir_result_names(op, |_| result = true);
            if result { return Err(format!("", op.kind)); }
        }
        Ok(())
    """)


def _operand_shapes(schema: str, generated: str) -> dict[str, int] | None:
    # Registration is a dominating fallible check, not a second shape table.
    # Bind its current producer too: accepting only the call spelling would
    # conceal a producer changed to admit every kind or discard its error.
    registered = _bool_table(generated, "simpleir_kind_is_registered")
    if registered is None or _compact(
        _body(schema, "validate_registered_op_kind") or ""
    ) != _compact("""
        validate_op_not_retired(kind)?;
        if !simpleir_kind_is_registered(kind) {
            return Err(OpShapeDiagnostic {
                family: "registration", kind: kind.into(),
                violation: OpShapeViolation::UnregisteredKind,
            });
        }
        Ok(())
    """):
        return None
    if _compact(_body(schema, "validate_op_shape") or "") != _compact("""
        validate_op_not_retired(kind)?;
        let Some(shape) = simpleir_op_shape(kind) else { return Ok(()); };
        let violation = if operands.unwrap_or(0) != shape.operands {
            Some(OpShapeViolation::OperandCount { expected: shape.operands, actual: operands, })
        } else if shape.value_rule == SimpleIrOpValueRule::NonNegative
            && value.is_none_or(|value| value < 0)
        { Some(OpShapeViolation::NonNegativeValue { actual: value }) }
        else { None };
        match violation {
            Some(violation) => Err(OpShapeDiagnostic { family: shape.family, kind: shape.kind.into(), violation, }),
            None => Ok(()),
        }
    """) or _compact(_body(schema, "validate_simple_op_shape") or "") != _compact("""
        validate_registered_op_kind(&op.kind)?;
        validate_op_shape(&op.kind, op.args.as_ref().map(Vec::len), op.value)?;
        if let Some(shape) = simpleir_op_shape(&op.kind)
            && simpleir_var_field_role_table(&op.kind) == SimpleIrVarFieldRole::Forbidden
            && op.var.is_some()
        { return Err(OpShapeDiagnostic { family: shape.family, kind: shape.kind.into(), violation: OpShapeViolation::ForbiddenVar, }); }
        Ok(())
    """):
        return None
    prefix = _locate(generated, "pub const SIMPLEIR_OP_SHAPES: &[SimpleIrOpShape] = &[")
    if prefix is None:
        return None
    closing = _closing(generated, prefix[1] - 1)
    if closing is None:
        return None
    rows = generated[prefix[1] : closing - 1]
    pattern = re.compile(
        r'SimpleIrOpShape\s*\{\s*kind:\s*"(?P<kind>[a-z_][a-z0-9_]*)",\s*family:\s*"[a-z_]+",\s*operands:\s*(?P<arity>\d+),\s*value_rule:\s*SimpleIrOpValueRule::(?:Unconstrained|NonNegative),\s*\},'
    )
    values = list(pattern.finditer(rows))
    if pattern.sub("", rows).strip():
        return None
    table = _generated_literal_table(
        _body(generated, "simpleir_op_shape"),
        r"Some\(&SIMPLEIR_OP_SHAPES\[(?P<value>\d+)\]\)",
        "_=>None,",
    )
    if (
        table is None
        or not table.keys() <= registered
        or any(
            int(index) >= len(values) or values[int(index)]["kind"] != kind
            for kind, index in table.items()
        )
    ):
        return None
    return {kind: int(values[int(index)]["arity"]) for kind, index in table.items()}


def _unary_reads(defuse: str, generated: str, unary: frozenset[str]) -> frozenset[str]:
    # Shared exact arity plus forbidden var implies one semantic read only if
    # the actual read walker and trailing-result projection retain that fact.
    bodies = {
        "simple_ir_single_read": """
            let mut source = None; let mut count = 0;
            visit_simple_ir_reads(op, |read| { count += 1; source = Some(read); });
            source.filter(|_| count == 1)
        """,
        "simple_ir_read_fields": """
            let arg_count = op.args.as_ref().map_or(0, Vec::len);
            let read_arity = simpleir_first_trailing_result_arg_table(op.kind.as_str())
                .unwrap_or(arg_count).min(arg_count);
            (0..read_arity).map(SimpleIrReadField::Arg).chain(
                (simple_ir_var_field_is_read(op) && op.var.is_some()).then_some(SimpleIrReadField::Var),
            )
        """,
        "visit_simple_ir_reads": """
            for field in simple_ir_read_fields(op) {
                let name = match field {
                    SimpleIrReadField::Arg(index) => { &op.args.as_ref().expect("")[index] }
                    SimpleIrReadField::Var => op.var.as_ref().expect(""),
                };
                visit(SimpleIrRead { name, field });
            }
        """,
    }
    if any(
        _compact(_body(defuse, name) or "") != _compact(expected)
        for name, expected in bodies.items()
    ):
        return frozenset()
    roles = _enum_table(
        generated, "simpleir_var_field_role_table", "SimpleIrVarFieldRole", "Read"
    )
    trailing = _generated_literal_table(
        _body(generated, "simpleir_first_trailing_result_arg_table"),
        r"Some\((?P<value>\d+)\)",
        "_=>None,",
    )
    if roles is None or trailing is None:
        return frozenset()
    return frozenset(
        kind
        for kind in unary
        if roles.get(kind) == "Forbidden"
        and (kind not in trailing or int(trailing[kind]) >= 1)
    )


def _resultless_out(
    defuse: str, generated: str, backend: str, helpers: str
) -> frozenset[str]:
    structural = _bool_table(generated, "simpleir_kind_is_structural")
    metadata = _bool_table(generated, "simpleir_out_field_is_metadata")
    roles = _enum_table(
        generated, "simpleir_var_field_role_table", "SimpleIrVarFieldRole", "Read"
    )
    if structural is None or metadata is None or roles is None:
        return frozenset()
    if not _same(
        _body(helpers, "out_var"), 'rust_ident(op.out.as_deref().unwrap_or("_"))'
    ):
        return frozenset()
    if not _starts(
        _body(backend, "rust_ident") or "",
        'if name.is_empty() || name == "none" || name == "_" { return "_".to_string(); }',
    ):
        return frozenset()
    if not _same(
        _body(defuse, "visit_simple_ir_result_names"),
        """
        visit_simple_ir_results(op, |result| { if let Some(name) = result.name { visit(name); } });
    """,
    ):
        return frozenset()
    if not _same(
        _body(defuse, "visit_simple_ir_results"),
        """
        if simpleir_var_field_role_table(op.kind.as_str()) == SimpleIrVarFieldRole::Result {
            visit(SimpleIrResult { name: op.var.as_deref().filter(|name| *name != "none"), field: SimpleIrResultField::Var, });
        }
        if !simpleir_out_field_is_metadata(op.kind.as_str()) {
            let name = if let Some(binding) = simple_ir_binding(op) { binding.result }
            else { op.out.as_deref().filter(|name| *name != "none") };
            visit(SimpleIrResult { name, field: SimpleIrResultField::Out, });
        }
        if let Some(first_result) = simpleir_first_trailing_result_arg_table(op.kind.as_str())
            && let Some(args) = op.args.as_deref()
        {
            for (index, name) in args.iter().enumerate().skip(first_result) {
                visit(SimpleIrResult { name: (name != "none").then_some(name.as_str()), field: SimpleIrResultField::Arg(index), });
            }
        }
    """,
    ) or not _starts(
        _body(defuse, "simple_ir_binding") or "",
        """
        if simpleir_var_field_role_table(op.kind.as_str()) != SimpleIrVarFieldRole::Definition { return None; }
    """,
    ):
        return frozenset()
    return frozenset(
        kind
        for kind in structural - metadata
        if roles.get(kind, "Read") != "Definition"
    )


def _guard_blocks(body: str, header: str) -> list[tuple[int, int]]:
    region = _block(body, header)
    return [region] if region is not None else []


def _literal_refusal_exclusions(
    body: str, kinds: frozenset[str], shapes: dict[str, str], literal_source: str
) -> list[tuple[int, int]]:
    if not _same(
        _body(literal_source, "validate_simple_literal"),
        "SimpleLiteral::from_simple(op).map(|_| ())",
    ) or not _starts(
        body,
        """
            use molt_ir::literal_payload::SimpleLiteral;
            use molt_ir::tir::op_kinds_generated::OwnedLiteralPayloadKind;
        """,
    ):
        return []
    first = match_arms(body, "SimpleLiteral::from_simple(op)")
    rhs = match_arms(body, "rhs")
    values = match_arms(body, "literal")
    if first is None or rhs is None or values is None:
        return []
    # Validation and emission call the same deterministic, immutable operation
    # projection. This grants only the first error arm, not every later failure.
    code = mask_rust_comments_and_strings(body)
    if (
        not _unshadowed(body, ("op",))
        or any(
            len(re.findall(rf"\blet\s+{name}\b", code)) != 1
            for name in ("literal", "rhs")
        )
        or re.search(
            r"\blet\s+mut\s+(?:literal|rhs)\b|&\s*mut\s+(?:literal|rhs)\b", code
        )
    ):
        return []
    result = [
        (arm.start, arm.end) for arm in first if _same(arm.pattern, "Err(reason)")
    ]
    required = {shapes[kind] for kind in kinds if kind in shapes}
    total = set()
    for arm in values:
        pattern = "".join(_words(arm.pattern))
        expected = {
            "SimpleLiteral::Int(value)": ("Int", 'Ok(format!(""))'),
            "SimpleLiteral::Float(value)": (
                "Float",
                'Ok(format!("", value.to_bits()))',
            ),
            "SimpleLiteral::Bool(value)": ("Bool", 'Ok(format!(""))'),
            "SimpleLiteral::None": ("None", 'Ok("".into())'),
            "SimpleLiteral::Owned(OwnedLiteralPayloadKind::String,payload)": (
                "Owned(OwnedLiteralPayloadKind::String)",
                '{ let bytes = payload.as_bytes(); Ok(format!("")) }',
            ),
        }.get(pattern)
        if expected and _compact(arm.body) == _compact(expected[1]):
            total.add(expected[0])
    if required <= total and len({"".join(_words(a.pattern)) for a in values}) == len(
        values
    ):
        result.extend(
            (arm.start, arm.end) for arm in rhs if _same(arm.pattern, "Err(reason)")
        )
    return result


@scan_memo()
def proven_branch_projection(
    root: Path, methods: dict[str, str], domain: AdmittedWireDomain
) -> BranchProjection:
    projection = BranchProjection()
    paths = {
        "generated": "runtime/molt-ir/src/tir/op_kinds_generated.rs",
        "literal": "runtime/molt-ir/src/literal_payload.rs",
        "admission": "runtime/molt-tir/src/target_admission.rs",
        "ir": "runtime/molt-ir/src/ir.rs",
        "schema": "runtime/molt-ir/src/ir_schema.rs",
        "defuse": "runtime/molt-ir/src/tir/simple_def_use.rs",
        "backend": "runtime/molt-backend-rust/src/rust.rs",
        "helpers": "runtime/molt-backend-rust/src/rust/emit_helpers.rs",
    }
    try:
        source = {
            key: mask_rust_test_items((root / path).read_text(encoding="utf-8"))
            for key, path in paths.items()
        }
    except (OSError, ValueError, RuntimeError):
        return projection
    flow = _flow_exclusion(methods, source["generated"], domain)
    if not _immutable_emission(methods, flow):
        return projection
    projection.add("emit_function", flow)
    shapes = _literal_shapes(source["generated"], source["literal"])
    _dispatch_projection(
        methods.get("emit_op", ""),
        methods.get("emit_op_literal", ""),
        source["generated"],
        shapes,
        domain,
        projection,
    )
    if not _validation_dominates(source["admission"], source["ir"], source["schema"]):
        return projection
    value = _value_contract(source["schema"])
    moves = _bool_table(source["generated"], "copy_kind_is_explicit_no_heap_move_table")
    aliases = _bool_table(source["generated"], "copy_kind_mints_owned_alias_ref_table")
    operands = _operand_shapes(source["schema"], source["generated"])
    single_read = (
        frozenset(kind for kind in moves | aliases if operands.get(kind, 1) == 1)
        if value and moves is not None and aliases is not None and operands is not None
        else frozenset()
    )
    unary = frozenset(kind for kind, arity in (operands or {}).items() if arity == 1)
    single_read |= _unary_reads(source["defuse"], source["generated"], unary)
    roles = _enum_table(
        source["generated"],
        "simpleir_var_field_role_table",
        "SimpleIrVarFieldRole",
        "Read",
    )
    resultless = (
        _resultless_out(
            source["defuse"], source["generated"], source["backend"], source["helpers"]
        )
        if value
        else frozenset()
    )
    # Input kinds are justified by actual direct dispatch calls. Any lateral
    # caller with a potentially different op revokes the callee's guard proof.
    for name, kinds in projection.inbound.items():
        if not kinds or name not in methods:
            continue
        bad_caller = False
        for caller, body in methods.items():
            code = mask_rust_comments_and_strings(body)
            for call in re.finditer(
                rf"\b(?:self\s*\.|Self\s*::)\s*{re.escape(name)}\b", code
            ):
                if projection.excludes(caller, call.start()):
                    continue
                if (
                    caller != "emit_op"
                    or "Self" in call[0]
                    or not re.match(r"\s*\(\s*op\s*\)", code[call.end() :])
                ):
                    bad_caller = True
        body = methods[name]
        if bad_caller or not _unshadowed(body, ("op",)):
            continue
        if kinds <= single_read:
            for variable in ("source",):
                for region in _guard_blocks(
                    body,
                    f"let Some({variable}) = molt_tir::tir::simple_def_use::simple_ir_single_read(op) else",
                ):
                    projection.add(name, region)
        if kinds <= unary:
            for region in _guard_blocks(
                body, "let Some([source]) = op.args.as_deref() else"
            ):
                projection.add(name, region)
        if (
            value
            and roles is not None
            and all(roles.get(kind) == "Definition" for kind in kinds)
        ):
            binding = _block(
                body,
                "let Some(binding) = molt_tir::tir::simple_def_use::simple_ir_binding(op) else",
                depth=0,
            )
            if binding is not None:
                projection.add(name, binding)
                if _starts(
                    body[binding[1] + 1 :],
                    '; if binding.destination.is_empty() || binding.destination == "none" {',
                ):
                    for region in _guard_blocks(
                        body,
                        'if binding.destination.is_empty() || binding.destination == "none"',
                    ):
                        projection.add(name, region)
        out_binding = _locate(body, "let out = out_var(op);")
        if (
            kinds <= resultless
            and _starts(body, "let out = out_var(op);")
            and out_binding is not None
            and _unshadowed(body[out_binding[1] :], ("out",))
        ):
            for region in _guard_blocks(
                body, 'if out != "_" && out != "none" && !out.is_empty()'
            ):
                projection.add(name, region)
        if name == "emit_op_literal" and shapes is not None:
            for region in _literal_refusal_exclusions(
                body, kinds, shapes, source["literal"]
            ):
                projection.add(name, region)
    return projection
