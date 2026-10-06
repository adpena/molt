"""Closed source projection of shared target admission for structural audits.

This proves only unconditional exclusion by the existing admission contract.
Unknown source shape, payload-dependent admission and dynamic dispatch remain
unresolved. It never supplies an independent backend support inventory.
"""

from __future__ import annotations

import ast
from dataclasses import dataclass
from pathlib import Path
import re
import tomllib

from molt.rust_source_scan import (
    mask_rust_comments_and_strings,
    mask_rust_test_items,
    read_rust_module_cluster,
    rust_source_tokens,
    rust_match_arms,
    rust_literal_pattern_names,
)
from tools.op_kinds.runtime_requirements import (
    integer_semantics_by_kind,
    runtime_kind_requirement_masks,
    target_runtime_requirement_masks,
)


def _compact(text: str) -> str:
    return re.sub(r"[\s\u200e\u200f]+", "", mask_rust_comments_and_strings(text))


def _body(text: str, name: str) -> str | None:
    code = mask_rust_comments_and_strings(mask_rust_test_items(text))
    matches = list(re.finditer(rf"\bfn\s+{re.escape(name)}\b", code))
    if len(matches) != 1:
        return None
    start = code.find("{", matches[0].end())
    if start < 0:
        return None
    depth = 1
    for end in range(start + 1, len(code)):
        depth += (code[end] == "{") - (code[end] == "}")
        if depth == 0:
            return text[start + 1 : end]
    return None


def _monotone_requirements(body: str | None, initial: str) -> bool:
    if body is None:
        return False
    code = _compact(body)
    if not code.startswith(initial) or not code.endswith("Some(requirements)"):
        return False
    if len(re.findall(r"let(?:mut)?requirements=", code)) != 1:
        return False
    if re.search(r"\b(?:return|unsafe)\b", body) or "&mutrequirements" in code:
        return False
    rest = code[len(initial) :]
    # Every subsequent assignment extends the existing lower bound by union.
    assignments = re.findall(r"(?<![\w.])requirements=(.*?);", rest)
    return all(value.startswith("requirements.union(") for value in assignments)


def _generated_literal_table(
    body: str | None, result_pattern: str, default: str
) -> dict[str, str] | None:
    if body is None:
        return None
    # Strip only lexical trivia. Whitespace inside a literal is data: turning
    # "n op" into "nop" would forge generated operation membership.
    code = "".join(token.text for token in rust_source_tokens(body))
    if not code.startswith("matchkind{") or not code.endswith("}"):
        return None
    arms = code[len("matchkind{") : -1]
    pattern = re.compile(
        r'(?P<kinds>"[A-Za-z_][A-Za-z0-9_]*"(?:\|"[A-Za-z_][A-Za-z0-9_]*")*)=>\{?'
        + result_pattern
        + r"\}?,?"
    )
    result = {}
    cursor = 0
    while match := pattern.match(arms, cursor):
        for kind in re.findall(r'"([^"]+)"', match["kinds"]):
            if kind in result:
                return None
            result[kind] = match["value"]
        cursor = match.end()
    return result if arms[cursor:] == default else None


@dataclass(frozen=True)
class AdmittedWireDomain:
    """An upper bound, not a backend support inventory or admission promise."""

    registered: frozenset[str]
    denied: frozenset[str]

    @property
    def possible(self) -> frozenset[str]:
        return self.registered - self.denied


def proven_admitted_wire_domain(
    root: Path, compile_body: str
) -> AdmittedWireDomain | None:
    """Return an unconditional denied domain only with source-bound dominance.

    Absence of evidence is None, never an empty support claim. The caller must
    separately prove private assembly and checked refusal publication.
    """
    consumer = _compact(compile_body)
    if not consumer.startswith(
        "letadmitted=admit_target_program(ir,&TargetInfo::rust_release_fast())?;"
        "letsource=self.emit_source(&admitted);"
    ):
        return None
    try:
        backend = (root / "runtime/molt-backend-rust/src/rust.rs").read_text(
            encoding="utf-8"
        )
        backend_family = read_rust_module_cluster(
            root / "runtime/molt-backend-rust/src/rust.rs"
        )
        admission = (root / "runtime/molt-tir/src/target_admission.rs").read_text(
            encoding="utf-8"
        )
        runtime = (root / "runtime/molt-tir/src/target_admission/runtime.rs").read_text(
            encoding="utf-8"
        )
        numeric = (root / "runtime/molt-tir/src/target_admission/numeric.rs").read_text(
            encoding="utf-8"
        )
        target = (root / "runtime/molt-ir/src/tir/target_info.rs").read_text(
            encoding="utf-8"
        )
        ir = (root / "runtime/molt-ir/src/ir.rs").read_text(encoding="utf-8")
        generated_rs = (
            root / "runtime/molt-ir/src/tir/op_kinds_generated.rs"
        ).read_text(encoding="utf-8")
        generated_py = (
            root / "src/molt/frontend/lowering/op_kinds_generated.py"
        ).read_text(encoding="utf-8")
        table = tomllib.loads(
            (root / "runtime/molt-ir/src/tir/op_kinds.toml").read_text(encoding="utf-8")
        )
    except (OSError, ValueError, RuntimeError):
        return None

    if _compact(_body(backend, "emit_source") or "") != _compact("""
        self.unsupported_ops.clear();
        self.assemble_source(admitted.ir(), Some(admitted.representation_plans()))
    """):
        return None
    # Only the witness consumer reaches the raw assembler in production.
    # The fixture-only entrypoint is masked by the shared module reader.
    family_code = mask_rust_comments_and_strings(backend_family)
    if len(re.findall(r"\bassemble_source\b", family_code)) != 2:
        return None

    witness = _body(admission, "admit_target_program")
    if witness is None or _compact(witness) != _compact("""
        let mut plans = BTreeMap::new();
        validate_target_contract_with_representation_plan(ir, target_info, |function, plan| {
            plans.insert(function.name.clone(), plan.clone());
            Ok(())
        })?;
        Ok(AdmittedTargetProgram { ir, plans })
    """):
        return None
    owner = _compact(mask_rust_test_items(admission))
    if (
        "pubstructAdmittedTargetProgram<'a>{ir:&'aSimpleIR,plans:BTreeMap<String,ScalarRepresentationPlan>,}"
        not in owner
    ):
        return None
    if _compact(_body(admission, "ir") or "") != "self.ir":
        return None
    if _compact(_body(admission, "representation_plans") or "") != "&self.plans":
        return None
    validator = _body(admission, "validate_target_contract_with_representation_plan")
    if validator is None:
        return None
    validation = _compact(validator)
    if not validation.endswith("validate_runtime_target_contract(ir,target_info)"):
        return None
    if re.search(r"\breturn\s+(?!Err\s*\()", mask_rust_comments_and_strings(validator)):
        return None
    # All execution requirements are checked against the complete descriptor
    # vocabulary before this function can return success. Message text is data.
    expected_runtime = """
        let target = target_info.target.as_str();
        let supported_requirements = target_info.supported_runtime_semantics;
        for function in &ir.functions {
            for (index, op) in function.ops.iter().enumerate() {
                if op.kind == "stack_alloc" {
                    return Err(format!("", function.name, op.kind,
                        crate::tir::target_info::BOXED_STACK_ALLOCATION_UNSUPPORTED,));
                }
                let Some(requirements) = op.runtime_requirements() else {
                    return Err(format!("", function.name, op.kind,));
                };
                let missing = requirements.difference(supported_requirements);
                for descriptor in SIMPLEIR_RUNTIME_REQUIREMENT_DESCRIPTORS {
                    if missing.contains(descriptor.requirement) {
                        return Err(format!("", function.name, op.kind, descriptor.reason,));
                    }
                }
            }
        }
        Ok(())
    """
    if _compact(_body(runtime, "validate_runtime_target_contract") or "") != _compact(
        expected_runtime
    ):
        return None
    if not _monotone_requirements(
        _body(ir, "execution_runtime_requirements"),
        "letmutrequirements=simpleir_runtime_requirements_table(self.kind.as_str())?;",
    ) or not _monotone_requirements(
        _body(ir, "runtime_requirements"),
        "letmutrequirements=self.execution_runtime_requirements()?;",
    ):
        return None
    if _compact(_body(generated_rs, "union") or "") != "Self(self.0|other.0)":
        return None
    if _compact(_body(generated_rs, "difference") or "") != "Self(self.0&!other.0)":
        return None
    if (
        _compact(_body(generated_rs, "contains") or "")
        != "self.0&requirement.0==requirement.0"
    ):
        return None
    descriptors = set(
        re.findall(
            r"requirement\s*:\s*SimpleIrRuntimeRequirements::([A-Z_]+)",
            mask_rust_comments_and_strings(generated_rs),
        )
    )
    if descriptors != {
        row["constant"] for row in table["simpleir_runtime_requirement_roles"]
    }:
        return None
    target_body = _compact(_body(target, "rust_release_fast") or "")
    if target_body != _compact("""
        TargetInfo {
            target: TargetKind::Rust,
            extern_function_linkage: false,
            supported_numeric_semantics: NumericTargetCapabilities::FIXED_WIDTH_FLOAT_ONLY,
            supported_runtime_semantics: Self::runtime_semantics_for(TargetKind::Rust),
            optimize_for_size: true,
            ..TargetInfo::native_release_fast()
        }
    """):
        return None
    if (
        _compact(_body(target, "runtime_semantics_for") or "")
        != "crate::tir::op_kinds_generated::simpleir_target_runtime_requirements(target)"
    ):
        return None

    expected = {
        "SIMPLEIR_RUNTIME_KIND_REQUIREMENTS": runtime_kind_requirement_masks(table),
        "SIMPLEIR_TARGET_RUNTIME_REQUIREMENTS": target_runtime_requirement_masks(table),
        "SIMPLEIR_INTEGER_SEMANTICS": integer_semantics_by_kind(table),
    }
    actual = {}
    try:
        for node in ast.parse(generated_py).body:
            if (
                isinstance(node, ast.AnnAssign)
                and isinstance(node.target, ast.Name)
                and node.target.id in expected
            ):
                actual[node.target.id] = ast.literal_eval(node.value)
    except (SyntaxError, ValueError, TypeError):
        return None
    if actual != expected:
        return None
    rust_masks = _generated_literal_table(
        _body(generated_rs, "simpleir_runtime_requirements_table"),
        r"Some\(SimpleIrRuntimeRequirements\((?P<value>\d+)\)\)",
        "_=>None,",
    )
    if (
        rust_masks is None
        or {kind: int(bits) for kind, bits in rust_masks.items()}
        != expected["SIMPLEIR_RUNTIME_KIND_REQUIREMENTS"]
    ):
        return None
    for role in table["simpleir_runtime_requirement_roles"]:
        if f"pubconst{role['constant']}:Self=Self(1<<{role['bit']});" not in _compact(
            generated_rs
        ):
            return None
    supported = expected["SIMPLEIR_TARGET_RUNTIME_REQUIREMENTS"]["rust"]
    profile = _compact(
        _body(generated_rs, "simpleir_target_runtime_requirements") or ""
    )
    if (
        f"super::target_info::TargetKind::Rust=>SimpleIrRuntimeRequirements({supported}),"
        not in profile
    ):
        return None
    denied = {
        kind
        for kind, bits in expected["SIMPLEIR_RUNTIME_KIND_REQUIREMENTS"].items()
        if bits & ~supported
    }

    # Infer only unconditional numeric exclusions. In this profile both integer
    # authorities are absent, so even small literals fail the actual branch.
    # Divmod and power also reject every operand domain when both their float
    # capability and arbitrary integers are absent. Other dynamic arithmetic
    # remains in the upper bound.
    integer_caps = _compact(target)
    numeric_body = _body(numeric, "numeric_admission_failure") or ""
    numeric_rule = _compact(numeric_body)
    numeric_loop = _compact(
        _body(numeric, "validate_numeric_function_target_contract") or ""
    )
    numeric_is_checked = (
        "numeric::validate_numeric_function_target_contract(function,target,target_info.supported_numeric_semantics,&plan,)?;"
        in validation
        and numeric_rule.startswith("useSimpleIrIntegerSemanticsasRole;matchrole{")
        and numeric_rule.endswith("}")
        and "_=>" not in numeric_rule
        and all(
            numeric_rule.count(f"Role::{role}") == 1
            for role in ("IntegerOnly", "IntegerProducer", "IntegerLiteral")
        )
        and "pubconstFIXED_WIDTH_FLOAT_ONLY:Self=Self{arbitrary_precision_integers:false,exact_integer_literal_max_magnitude:None,cpython_float_divmod:false,cpython_power:false,};"
        in integer_caps
        and "Role::IntegerOnly|Role::IntegerProducer=>(!capabilities.arbitrary_precision_integers).then_some(,),"
        in numeric_rule
        and numeric_loop.startswith(
            "for(index,op)infunction.ops.iter().enumerate(){letrole=simpleir_integer_semantics_table(op.kind.as_str());ifletSome(reason)=numeric_admission_failure(plan,op,role,capabilities){returnErr(format!(,function.name,op.kind,));}}Ok(())"
        )
        and _generated_literal_table(
            _body(generated_rs, "simpleir_integer_semantics_table"),
            r"SimpleIrIntegerSemantics::(?P<value>[A-Za-z]+)",
            "_=>SimpleIrIntegerSemantics::None,",
        )
        == expected["SIMPLEIR_INTEGER_SEMANTICS"]
    )
    if numeric_is_checked:
        roles = {"IntegerOnly", "IntegerProducer"}
        if (
            _compact("""
            Role::IntegerLiteral => {
                if capabilities.arbitrary_precision_integers
                    || capabilities.exact_integer_literal_max_magnitude
                        .is_some_and(|max| exact_integer_literal_value(op, max).is_some())
                { None } else { Some("",) }
            }
        """)
            in numeric_rule
        ):
            roles.add("IntegerLiteral")
        numeric_arms = rust_match_arms(numeric_body, "role") or []
        for role, float_capability in (
            ("DynamicDivmod", "cpython_float_divmod"),
            ("DynamicPower", "cpython_power"),
        ):
            matching = [
                arm for arm in numeric_arms if _compact(arm.pattern) == f"Role::{role}"
            ]
            # Three exhaustive input cases: proven float, proven integer,
            # neither. Both concrete capabilities were source-proved false
            # above, so each case returns Some(reason), never successful None.
            expected_branch = """{
                if operands_are_float(plan, op, 2) {
                    (!capabilities.FLOAT_CAPABILITY).then_some("",)
                } else if operands_are_integer(plan, op, 2) {
                    (!capabilities.arbitrary_precision_integers).then_some("",)
                } else if capabilities.arbitrary_precision_integers && capabilities.FLOAT_CAPABILITY {
                    None
                } else { Some("",) }
            }""".replace("FLOAT_CAPABILITY", float_capability)
            if (
                numeric_rule.count(f"Role::{role}") == 1
                and len(matching) == 1
                and _compact(matching[0].body) == _compact(expected_branch)
            ):
                roles.add(role)
        denied.update(
            kind
            for kind, role in expected["SIMPLEIR_INTEGER_SEMANTICS"].items()
            if role in roles
        )
    return AdmittedWireDomain(
        frozenset(expected["SIMPLEIR_RUNTIME_KIND_REQUIREMENTS"]), frozenset(denied)
    )


def proven_rejected_kinds(root: Path, compile_body: str) -> frozenset[str] | None:
    """Project the denied bound for callers that do not need registration."""
    domain = proven_admitted_wire_domain(root, compile_body)
    return domain.denied if domain is not None else None


def literal_dispatch_arm_ranges(text: str) -> list[tuple[int, int, frozenset[str]]]:
    """Project unguarded literal arms through the shared lexical arm parser.

    Wildcards, guards, raw or computed patterns grant no exclusion. The same
    parser owns these offsets and the admitted residual-domain projection.
    """
    arms = rust_match_arms(text, "op.kind.as_str()")
    if arms is None:
        return []
    return [
        (arm.start, arm.end, names)
        for arm in arms
        if (names := rust_literal_pattern_names(arm.pattern)) is not None
    ]
