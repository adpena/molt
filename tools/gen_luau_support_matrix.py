#!/usr/bin/env python3
"""Generate the Luau backend OpIR support matrix.

The matrix is derived from the Luau backend's `op_emitter.rs` so support claims
stay tied to the actual emitter. It classifies each `emit_op` match arm into a
small set of gateable statuses.
"""

from __future__ import annotations

import re
import sys
import tomllib
from collections import Counter
from dataclasses import dataclass
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from tools.generator_io import generator_main  # noqa: E402
from molt.rust_source_scan import (  # noqa: E402
    mask_rust_comments_and_strings,
    rust_delimiter_end,
    rust_match_arms,
    scan_memo,
)
from tools.structural_audit_rust import (  # noqa: E402
    kind_pattern_names,
    op_kind_wire_names,
    token_text,
)


ROOT = Path(__file__).resolve().parents[1]

from tools.op_kinds.runtime_requirements import (  # noqa: E402 - direct-script bootstrap above
    integer_semantics_by_kind,
    registered_runtime_kinds,
    runtime_kind_requirement_masks,
    target_runtime_requirement_masks,
)
from tools.op_kinds.runtime_calls import (  # noqa: E402
    runtime_call_rows,
    runtime_call_wire_kinds,
)

SOURCE = ROOT / "runtime" / "molt-backend-luau" / "src" / "luau"
WIRE_SOURCE = ROOT / "runtime" / "molt-ir" / "src" / "tir" / "op_kinds_generated.rs"
OUTPUT = (
    ROOT / "docs" / "spec" / "areas" / "compiler" / "luau_support_matrix.generated.md"
)

STATUSES = {
    "implemented-exact",
    "implemented-target-limited",
    "compile-error",
    "not-admitted",
}

_OP_KIND_TABLE = tomllib.loads(
    (ROOT / "runtime" / "molt-ir" / "src" / "tir" / "op_kinds.toml").read_text(
        encoding="utf-8"
    )
)


def _kind_set(*keys: str) -> frozenset[str]:
    return frozenset(kind for key in keys for kind in _OP_KIND_TABLE.get(key, []))


_RUNTIME_MASKS = runtime_kind_requirement_masks(_OP_KIND_TABLE)
_LUAU_RUNTIME_BITS = target_runtime_requirement_masks(_OP_KIND_TABLE)["luau"]
_INTEGER_ROLES = integer_semantics_by_kind(_OP_KIND_TABLE)


def _numeric_kinds(*roles: str) -> frozenset[str]:
    return frozenset(kind for kind, role in _INTEGER_ROLES.items() if role in roles)


_LUAU_RUNTIME_NOT_ADMITTED = frozenset(
    kind for kind, bits in _RUNTIME_MASKS.items() if bits & ~_LUAU_RUNTIME_BITS
)
_PRE_SOURCE_NOT_ADMITTED = (
    _numeric_kinds("DynamicDivmod", "DynamicPower", "IntegerOnly", "IntegerProducer")
    | _LUAU_RUNTIME_NOT_ADMITTED
)
_PRE_SOURCE_LITERAL_LIMITED = _numeric_kinds("IntegerLiteral")
_PRE_SOURCE_INSTANCE_LIMITED = _kind_set("async_work_poll_marker_kinds")
_PRE_SOURCE_ORDERED_MAPPING_LIMITED = _kind_set("simpleir_luau_ordered_mapping_kinds")
_PRE_SOURCE_TYPE_LIMITED = _numeric_kinds(
    "DynamicAdd", "DynamicNumeric", "DynamicTrueDiv", "DynamicUnaryNumeric"
)
_REGISTERED_SIMPLEIR_KINDS = registered_runtime_kinds(_OP_KIND_TABLE)
_RUNTIME_CALLS = {
    kind: row
    for row in runtime_call_rows(_OP_KIND_TABLE)
    for kind in runtime_call_wire_kinds(_OP_KIND_TABLE, row)
}


@dataclass(frozen=True)
class Row:
    op: str
    status: str
    note: str


_EMIT_OP_FN_RE = re.compile(
    r"\bfn\s+(?P<name>emit[A-Za-z0-9_]*)\s*\(\s*&mut\s+self\s*,\s*op:\s*&OpIR\s*\)"
)
_KIND_MATCH_RE = re.compile(
    r"\bmatch\s+(?P<expression>op\s*\.\s*kind|"
    r"molt_ir\s*::\s*tir\s*::\s*op_kinds_generated\s*::\s*"
    r"[A-Za-z_][A-Za-z0-9_]*\s*\(\s*op\s*\.\s*kind\s*\))\s*\{"
)
_UNSUPPORTED_EMISSION_CALL = re.compile(
    r"\bself\s*\.\s*emit_unsupported_op(?:_with_reason)?\s*\("
)


def _unsupported_emission_kind(body: str) -> str | None:
    """Distinguish rejection-only arms from branch-dependent checked rejection.

    The shared helper records compile_checked failure rather than emitting an
    inline unsupported marker. Conditional calls are not proof of universal
    rejection, but cannot attest unrestricted support either. Lexical masking
    prevents comments, raw strings and diagnostic text from becoming calls.
    """
    code = mask_rust_comments_and_strings(body)
    if not _UNSUPPORTED_EMISSION_CALL.search(code):
        return None
    expression = code.split("=>", 1)[-1].strip().rstrip(",").strip()
    if expression.startswith("{") and expression.endswith("}"):
        expression = expression[1:-1].strip()
    call = _UNSUPPORTED_EMISSION_CALL.match(expression)
    if call is None:
        return "conditional"
    depth = 1
    index = call.end()
    while index < len(expression) and depth:
        depth += (expression[index] == "(") - (expression[index] == ")")
        index += 1
    if depth:
        raise ValueError("unterminated unsupported-emission helper call")
    tail = expression[index:].strip()
    if re.fullmatch(r";?\s*(?:return\s+(?:true|false)\s*;)?", tail):
        return "only"
    return "conditional"


@dataclass(frozen=True)
class EmitterMatch:
    function: str
    expression: str
    source: str


def _extract_emit_op_matches(text: str) -> list[EmitterMatch]:
    """Project typed dispatches, excluding nested operator/name matches."""
    code = mask_rust_comments_and_strings(text)
    matches: list[EmitterMatch] = []
    for fn_match in _EMIT_OP_FN_RE.finditer(code):
        fn_open = code.find("{", fn_match.end())
        fn_end = rust_delimiter_end(text, fn_open)
        if fn_end is None:
            raise ValueError(f"unclosed Luau emitter {fn_match['name']}")
        cursor = fn_open
        while match := _KIND_MATCH_RE.search(code, cursor, fn_end):
            end = rust_delimiter_end(text, match.end() - 1)
            if end is None or end > fn_end:
                raise ValueError(f"unclosed typed dispatch in {fn_match['name']}")
            matches.append(
                EmitterMatch(
                    fn_match["name"],
                    token_text(match["expression"]),
                    text[match.start() : end],
                )
            )
            cursor = end
    return matches


def _selector_domains(
    expression: str, generated: str, names: dict[str, str]
) -> tuple[str, dict[str, frozenset[str]]]:
    """Join a generated typed selector to its canonical OpKind input domain."""
    selector = re.fullmatch(
        r"molt_ir::tir::op_kinds_generated::(?P<name>[A-Za-z_][A-Za-z0-9_]*)"
        r"\(op\.kind\)",
        expression,
    )
    if selector is None:
        raise ValueError(f"unsupported Luau kind selector: {expression}")
    code = mask_rust_comments_and_strings(generated)
    declaration = re.search(
        rf"\bpub\s+const\s+fn\s+{selector['name']}\s*"
        r"\(\s*kind\s*:\s*OpKind\s*\)\s*->\s*"
        r"(?P<result>[A-Za-z_][A-Za-z0-9_]*)\s*\{",
        code,
    )
    if declaration is None:
        raise ValueError(f"missing generated typed selector: {selector['name']}")
    end = rust_delimiter_end(generated, declaration.end() - 1)
    if end is None:
        raise ValueError(f"unclosed generated selector: {selector['name']}")
    arms = rust_match_arms(generated[declaration.end() : end - 1], "kind", depth=0)
    if not arms:
        raise ValueError(f"missing generated selector arms: {selector['name']}")
    domains: dict[str, set[str]] = {}
    seen: set[str] = set()
    for arm in arms:
        kinds = kind_pattern_names(arm.pattern, names)
        result = re.fullmatch(
            rf"{declaration['result']}::(?P<variant>[A-Za-z_][A-Za-z0-9_]*)",
            token_text(arm.body).removeprefix("{").removesuffix("}"),
        )
        if kinds is None or seen & kinds or result is None:
            raise ValueError(f"invalid generated selector arm: {selector['name']}")
        seen.update(kinds)
        domains.setdefault(result["variant"], set()).update(kinds)
    if seen != set(names.values()):
        raise ValueError(f"incomplete generated selector: {selector['name']}")
    return declaration["result"], {
        variant: frozenset(kinds) for variant, kinds in domains.items()
    }


def _iter_arms(
    match: EmitterMatch, generated: str | None = None
) -> list[tuple[list[str], str]]:
    if generated is None:
        generated = WIRE_SOURCE.read_text(encoding="utf-8")
    names = op_kind_wire_names(generated)
    if names is None:
        raise ValueError("could not project canonical OpKind serde names")
    arms = rust_match_arms(match.source, match.expression, depth=0)
    if not arms:
        raise ValueError(f"could not project typed arms in {match.function}")
    selector = (
        _selector_domains(match.expression, generated, names)
        if match.expression != "op.kind"
        else None
    )
    result: list[tuple[list[str], str]] = []
    for arm in arms:
        pattern = token_text(arm.pattern)
        body = token_text(arm.body).removeprefix("{").removesuffix("}")
        declined = body.rstrip(";") == "returnfalse"
        if pattern == "_":
            if not declined:
                raise ValueError(f"unclassified wildcard in {match.function}")
            continue
        if selector is None:
            # This pattern is generated from the same rows as RuntimeCall's
            # closed dispatch. Do not restore a hand-written list of its ops.
            kinds = (
                frozenset(_RUNTIME_CALLS)
                if pattern == "molt_ir::runtime_call_op_kinds!()"
                else kind_pattern_names(arm.pattern, names)
            )
        else:
            enum, domains = selector
            variants = [
                re.fullmatch(
                    rf"(?:molt_ir::tir::op_kinds_generated::)?{enum}::"
                    r"(?P<variant>[A-Za-z_][A-Za-z0-9_]*)",
                    part,
                )
                for part in pattern.split("|")
            ]
            if any(part is None or part["variant"] not in domains for part in variants):
                raise ValueError(f"unclassified selector pattern in {match.function}")
            kinds = frozenset(
                kind
                for part in variants
                if part is not None
                for kind in domains[part["variant"]]
            )
        if kinds is None:
            raise ValueError(
                f"unclassified typed pattern in {match.function}: {pattern}"
            )
        if not declined:
            result.append((sorted(kinds), arm.body))
    return result


def _classify(op: str, body: str) -> Row:
    if op not in _REGISTERED_SIMPLEIR_KINDS:
        return Row(
            op,
            "not-admitted",
            "Operation is unclassified in the generated target-contract authority.",
        )
    unsupported_emission = _unsupported_emission_kind(body)
    if unsupported_emission == "only" and op not in _PRE_SOURCE_NOT_ADMITTED:
        return Row(
            op,
            "compile-error",
            "Checked Luau emission rejects this arm through the shared unsupported-operation helper.",
        )
    if op in _PRE_SOURCE_ORDERED_MAPPING_LIMITED:
        return Row(
            op,
            "implemented-target-limited",
            "Pre-source admission requires canonical ordered-dict provenance; generic Python keys/getitem mappings are not implemented.",
        )
    if op in _PRE_SOURCE_INSTANCE_LIMITED:
        return Row(
            op,
            "implemented-target-limited",
            "Unmarked observer is lowered; the generated target contract rejects the pending-call/eval-breaker-marked variant.",
        )
    if op in _PRE_SOURCE_NOT_ADMITTED:
        return Row(
            op,
            "not-admitted",
            "Shared generated target contract rejects this semantic family before source generation.",
        )
    if contract := _RUNTIME_CALLS.get(op):
        realization = contract["luau"]
        match realization:
            case "luau_iter_carriers":
                status = "implemented-target-limited"
                precondition = (
                    "Requires a proven deterministic scalar/container carrier and "
                    "the Python numeric source type; unknown carriers are rejected."
                )
            case "luau_dict_storage":
                status = "implemented-target-limited"
                precondition = (
                    "Requires checked deterministic dict storage; insertion bypasses "
                    "assignment hooks and returns the same dict."
                )
            case "unsupported":
                status = "not-admitted"
                precondition = (
                    "The generated runtime-call contract refuses this target."
                )
            case "runtime":
                status = "implemented-exact"
                precondition = "Uses the declared reference runtime call."
            case _:
                raise ValueError(
                    f"unclassified Luau runtime-call realization: {realization}"
                )
        return Row(
            op,
            status,
            f"Runtime-call contract `{contract['function']}`. {precondition}",
        )
    if op in _PRE_SOURCE_LITERAL_LIMITED:
        return Row(
            op,
            "implemented-target-limited",
            "Shared target contract admits only concrete integer literals exactly representable by Luau's numeric carrier.",
        )
    if op in _PRE_SOURCE_TYPE_LIMITED:
        return Row(
            op,
            "implemented-target-limited",
            "Shared target contract admits only representation-proven non-integer scalar domains.",
        )
    if "-- [unsupported op:" in body or 'error(\\"[unsupported op:' in body:
        return Row(
            op, "compile-error", "Checked Luau emission rejects unsupported markers."
        )
    if unsupported_emission == "conditional" and op == "builtin_func":
        return Row(
            op,
            "implemented-target-limited",
            "The raw emitter accepts its explicit builtin-name whitelist; otherwise valid unlisted builtin forms reach the shared unsupported-operation helper.",
        )
    semantic_markers = (
        "-- [async:",
        "-- [context:",
        "-- [internal:",
        "-- [stub:",
        "-- [class op:",
        "-- [try_start]",
        "-- [try_end]",
        "-- [",
    )
    if any(marker in body for marker in semantic_markers):
        allowed = (
            "-- [exception_last]",
            "-- [missing]",
            "-- [vectorized:",
        )
        if not any(marker in body for marker in allowed):
            return Row(
                op,
                "not-admitted",
                "Checked Luau emission rejects semantic stub markers.",
            )

    return Row(
        op,
        "implemented-exact",
        "Lowered and outside every generated target-contract limitation.",
    )


@scan_memo()
def collect_rows_from_text(text: str) -> list[Row]:
    by_op: dict[str, Row] = {}
    match_texts = _extract_emit_op_matches(text)
    if not match_texts:
        raise ValueError("could not find Luau emitter op-kind match arms")
    generated = WIRE_SOURCE.read_text(encoding="utf-8")
    delegated: set[str] = set()
    # emit_op tries the family emitters before its exhaustive final refusal.
    # A family arm therefore owns its kind even though the fallback names it.
    # Source filename order must not turn that fallback into a support claim or
    # overwrite an actual lowering with a refusal it never reaches.
    for match_text in sorted(
        match_texts, key=lambda match: match.function != "emit_op"
    ):
        for ops, body in _iter_arms(match_text, generated):
            for op in ops:
                if match_text.function != "emit_op":
                    if op in delegated:
                        raise ValueError(f"duplicate Luau family emitter for {op}")
                    delegated.add(op)
                by_op[op] = _classify(op, body)

    # A generated pre-source rejection is itself the backend's implementation
    # of that target contract; it deliberately has no emitter arm. Include
    # these rows from the semantic authority so deleting an unreachable arm
    # cannot also erase the operation from the conformance matrix.
    for op in _PRE_SOURCE_NOT_ADMITTED:
        by_op.setdefault(op, _classify(op, ""))

    rows = sorted(by_op.values(), key=lambda row: row.op)
    bad_statuses = sorted({row.status for row in rows} - STATUSES)
    if bad_statuses:
        raise ValueError(f"unknown statuses produced: {', '.join(bad_statuses)}")
    return rows


def _source_files(source: Path) -> list[Path]:
    if source.is_dir():
        return sorted(
            path
            for path in source.rglob("*.rs")
            if "tests" not in path.relative_to(source).parts and path.name != "tests.rs"
        )
    return [source]


def _render(rows: list[Row], source: Path) -> str:
    counts = Counter(row.status for row in rows)
    source_display = source.relative_to(ROOT) if source.is_relative_to(ROOT) else source
    lines: list[str] = [
        "# Luau Backend OpIR Support Matrix",
        "",
        "**Status:** Generated",
        f"**Source:** `{source_display.as_posix()}`",
        "**Target:** current/future Luau surface; Molt does not add legacy Lua compatibility shims.",
        "",
        "**Scope:** raw OpIR emitter-arm classification joined with generated pre-source contracts; these rows do not attest whole-function acceptance or execution.",
        "",
        "Validated structured CFG uses `function_body.rs` and `flow_dispatch.rs` separately: jump/branch edges and exception-check edges can bypass raw `emit_op`. A rejected raw fallback is not evidence that every structured program containing that operation is unsupported; that route requires its own validation.",
        "",
        "## Summary",
        "",
    ]
    for status in sorted(STATUSES):
        lines.append(f"- `{status}`: `{counts.get(status, 0)}`")
    lines.extend(
        [
            f"- `total`: `{len(rows)}`",
            "",
            "## Matrix",
            "",
            "| OpIR kind | Status | Note |",
            "| --- | --- | --- |",
        ]
    )
    for row in rows:
        lines.append(f"| `{row.op}` | `{row.status}` | {row.note} |")
    lines.extend(
        [
            "",
            "## Status Definitions",
            "",
            "- `implemented-exact`: emitted without known Luau target limitation or checked-output stub marker.",
            "- `implemented-target-limited`: emitted for an admitted subset with an explicit Luau/Python semantic limit.",
            "- `compile-error`: the checked raw emitter arm rejects this operation; separate structured CFG dispatch is outside this classification.",
            "- `not-admitted`: current lowering is intentionally rejected by checked Luau emission.",
            "",
        ]
    )
    return "\n".join(lines)


def build_output(source: Path) -> str:
    text = "\n".join(path.read_text(encoding="utf-8") for path in _source_files(source))
    return _render(collect_rows_from_text(text), source)


def generated_outputs() -> dict[Path, str]:
    """Each output path mapped to its exact generated text."""
    return {OUTPUT: build_output(SOURCE)}


def main(argv: list[str] | None = None) -> int:
    return generator_main(generated_outputs, argv, description=__doc__)


if __name__ == "__main__":
    raise SystemExit(main())
