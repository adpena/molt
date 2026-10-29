"""Closed lexical projections of the generated wire-kind type.

The serde names on OpKind are the wire authority. These projections let the
refusal audit reason about dynamic payload guards; they do not re-check Rust
membership or stand in for the compiler's exhaustiveness checks.
"""

from __future__ import annotations

from collections.abc import Mapping
import re

from molt.rust_source_scan import (
    rust_block_region,
    rust_delimiter_end,
    rust_match_arms,
    rust_source_tokens,
    scan_memo,
)


def token_text(text: str) -> str:
    return "".join(token.text for token in rust_source_tokens(text))


@scan_memo()
def op_kind_wire_names(generated: str) -> dict[str, str] | None:
    region = rust_block_region(generated, "pub enum OpKind")
    if region is None:
        return None
    body = token_text(generated[slice(*region)])
    row = re.compile(
        r'#\[serde\(rename="(?P<wire>[a-z_][a-z0-9_]*)"\)\]'
        r"(?P<variant>[A-Za-z_][A-Za-z0-9_]*),"
    )
    names: dict[str, str] = {}
    cursor = 0
    while match := row.match(body, cursor):
        if match["variant"] in names or match["wire"] in names.values():
            return None
        names[match["variant"]] = match["wire"]
        cursor = match.end()
    return names if names and cursor == len(body) else None


def kind_pattern_names(
    pattern: str, names: Mapping[str, str], *, self_type: bool = False
) -> frozenset[str] | None:
    code = token_text(pattern)
    if self_type:
        code = re.sub(r"(?<![A-Za-z0-9_:])Self::", "OpKind::", code)
    values = code.split("|")
    variants = []
    for value in values:
        match = re.fullmatch(
            r"(?:(?:crate|molt_ir)::)?OpKind::([A-Za-z_][A-Za-z0-9_]*)", value
        )
        if match is None or match[1] not in names:
            return None
        variants.append(match[1])
    if len(set(variants)) != len(variants):
        return None
    return frozenset(names[variant] for variant in variants)


def runtime_call_pattern_names(generated: str) -> frozenset[str] | None:
    """Project the generated pattern only when its selector has the same domain.

    This is a lexical bridge for the refusal audit's operand applicability,
    not a second runtime-call registry or an exhaustiveness proof.
    """
    names = op_kind_wire_names(generated)
    owner = rust_block_region(generated, "pub mod runtime_calls")
    if names is None or owner is None:
        return None
    source = generated[slice(*owner)]
    selector = rust_block_region(
        source, "pub const fn from_op(kind: OpKind) -> Option<Self>"
    )
    pattern = rust_block_region(source, "macro_rules! runtime_call_op_kinds")
    if selector is None or pattern is None:
        return None
    macro = re.fullmatch(
        r"\(\)=>\{(?P<pattern>.+)\};", token_text(source[slice(*pattern)])
    )
    if macro is None:
        return None
    kinds = kind_pattern_names(macro["pattern"].replace("$crate::", "molt_ir::"), names)
    if kinds is None:
        return None
    body = source[slice(*selector)]
    code = token_text(body)
    if not code.startswith("matchkind{") or rust_delimiter_end(
        code, code.index("{")
    ) != len(code):
        return None
    arms = rust_match_arms(body, "kind")
    if (
        not arms
        or token_text(arms[-1].pattern) != "_"
        or token_text(arms[-1].body) != "None"
    ):
        return None
    selected: set[str] = set()
    for arm in arms[:-1]:
        domain = kind_pattern_names(arm.pattern, names)
        if (
            domain is None
            or selected & domain
            or not re.fullmatch(
                r"Some\(Self::[A-Za-z_][A-Za-z0-9_]*\)", token_text(arm.body)
            )
        ):
            return None
        selected.update(domain)
    return kinds if selected == kinds else None


def generated_kind_table(
    body: str | None,
    result_pattern: str,
    default_value: str,
    names: Mapping[str, str] | None,
) -> dict[str, str] | None:
    if body is None or names is None:
        return None
    code = token_text(body)
    expression = "self" if code.startswith("matchself{") else "kind"
    if not code.startswith(f"match{expression}{{") or rust_delimiter_end(
        code, code.index("{")
    ) != len(code):
        return None
    arms = rust_match_arms(body, expression)
    if arms is None:
        return None
    result: dict[str, str] = {}
    seen: set[str] = set()
    for arm in arms:
        kinds = kind_pattern_names(arm.pattern, names, self_type=expression == "self")
        if kinds is None or seen & kinds:
            return None
        seen.update(kinds)
        rhs = token_text(arm.body)
        if rhs.startswith("{") and rhs.endswith("}"):
            rhs = rhs[1:-1]
        if rhs == default_value:
            continue
        match = re.fullmatch(result_pattern, rhs)
        if match is None:
            return None
        for kind in kinds:
            result[kind] = match["value"]
    return result
