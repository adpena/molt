#!/usr/bin/env python3
"""Verify LLVM runtime import ABI authority for preserved-Copy runtime calls.

`MOLT_RUNTIME_INTRINSIC_SYMBOLS` is an active-profile availability set, not an
ABI manifest. The LLVM generic preserved-op fallback may call `molt_<kind>` only
when `runtime_imports/abi_facts.rs` owns an explicit `(symbol, parameter ABI,
return ABI)` fact.

This audit derives the generic preserved runtime surface from the frontend wire
vocabulary and the generated TIR mapper:

* `serialization.py` emits JSON wire `kind` strings.
* `op_kinds_generated.rs::kind_to_opcode_table` maps first-class TIR kinds.
* emitted-but-unmapped kinds become preserved `Copy{_original_kind}` values.
* if a `runtime/molt-runtime*/src/**/*.rs` leaf exports `molt_<kind>`, LLVM's generic
  fallback can see it through the availability set.

Every boxed/i64 or void export in that surface must be owned by either the
fixed runtime import table or the residual conservative import table; non-boxed
C returns are explicitly fail-closed. Every ABI fact must also match the actual
Rust export parameter ABI and return ABI, because the LLVM fallback declaration
is derived from that fact.
"""

from __future__ import annotations

import argparse
import ast
import json
import re
import sys
from collections.abc import Iterable
from dataclasses import asdict, dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from tools.op_kinds.paths import OUT_RS as OP_KINDS_GENERATED_RS  # noqa: E402

SERIALIZATION_PY = ROOT / "src/molt/frontend/lowering/serialization.py"
RUNTIME_IMPORT_ABI_FACTS_RS = (
    ROOT / "runtime/molt-backend-native/src/llvm_backend/runtime_imports/abi_facts.rs"
)
RUNTIME_IMPORT_FIXED_RS = (
    ROOT / "runtime/molt-backend-native/src/llvm_backend/runtime_imports/fixed.rs"
)
RUNTIME_IMPORT_ABI_RS = ROOT / "runtime/molt-backend-native/src/runtime_import_abi.rs"

ABI_I64_RETURNS = {"u64", "i64"}
ABI_VOID_RETURNS = {"", "()", "void"}
ABI_I64_PARAMS = {"u64", "i64"}

# The generic preserved-runtime fallback may only use boxed/i64 or void ABI
# exports. Non-boxed returns must be owned by dedicated lowering arms; this
# allowlist stays empty unless a future dedicated fail-closed exception is
# explicitly proven.
ALLOWED_NON_BOXED_RETURNS: set[tuple[str, int, str]] = set()


def runtime_src_roots(root: Path = ROOT) -> tuple[Path, ...]:
    return tuple(
        sorted(
            path / "src"
            for path in (root / "runtime").glob("molt-runtime*")
            if (path / "src").is_dir()
        )
    )


RUNTIME_SRC_ROOTS = runtime_src_roots(ROOT)


def _iter_src_roots(roots: Path | Iterable[Path]) -> tuple[Path, ...]:
    if isinstance(roots, Path):
        return (roots,)
    return tuple(roots)


def _display_path(path: Path) -> str:
    try:
        return str(path.relative_to(ROOT))
    except ValueError:
        return str(path)


@dataclass(frozen=True, order=True)
class RuntimeSignature:
    symbol: str
    arity: int
    rust_return: str
    source: str
    kind: str = ""
    rust_params: tuple[str, ...] = ()


@dataclass(frozen=True, order=True)
class AbiFact:
    symbol: str
    arity: int
    return_abi: str
    param_abis: tuple[str, ...]


@dataclass(frozen=True, order=True)
class AbiIssue:
    symbol: str
    arity: int
    kind: str
    rust_return: str
    expected: str
    actual: str


@dataclass(frozen=True, order=True)
class DuplicateAbiFact:
    symbol: str
    arity: int
    first: str
    second: str


@dataclass(frozen=True, order=True)
class ClassifiedFactIssue:
    problem: str
    symbol: str
    classified_arity: int
    rust_arity: str
    rust_return: str
    expected: str
    actual: str
    source: str


@dataclass(frozen=True)
class AuditResult:
    missing: tuple[AbiIssue, ...]
    mismatched: tuple[AbiIssue, ...]
    duplicate_facts: tuple[DuplicateAbiFact, ...]
    classified_fact_issues: tuple[ClassifiedFactIssue, ...]
    unexpected_non_boxed: tuple[RuntimeSignature, ...]
    allowed_non_boxed: tuple[RuntimeSignature, ...]

    @property
    def ok(self) -> bool:
        return not (
            self.missing
            or self.mismatched
            or self.duplicate_facts
            or self.classified_fact_issues
            or self.unexpected_non_boxed
        )


def frontend_wire_kinds(path: Path = SERIALIZATION_PY) -> set[str]:
    tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
    kinds: set[str] = set()
    for node in ast.walk(tree):
        if not isinstance(node, ast.Dict):
            continue
        for key, value in zip(node.keys, node.values):
            if (
                isinstance(key, ast.Constant)
                and key.value == "kind"
                and isinstance(value, ast.Constant)
                and isinstance(value.value, str)
            ):
                kinds.add(value.value)
    return kinds


def mapped_tir_kinds(path: Path = OP_KINDS_GENERATED_RS) -> set[str]:
    source = path.read_text(encoding="utf-8")
    match = re.search(
        r"fn\s+kind_to_opcode_table\s*\([^)]*\).*?match\s+kind\s*\{(.*?)_ => None",
        source,
        re.DOTALL,
    )
    if not match:
        raise RuntimeError(f"could not locate kind_to_opcode_table in {path}")
    return set(re.findall(r'"([A-Za-z0-9_]+)"', match.group(1)))


def _split_top_level_commas(params: str) -> list[str]:
    parts: list[str] = []
    start = 0
    angle = paren = bracket = 0
    for idx, ch in enumerate(params):
        if ch == "<":
            angle += 1
        elif ch == ">" and angle:
            angle -= 1
        elif ch == "(":
            paren += 1
        elif ch == ")" and paren:
            paren -= 1
        elif ch == "[":
            bracket += 1
        elif ch == "]" and bracket:
            bracket -= 1
        elif ch == "," and angle == paren == bracket == 0:
            parts.append(params[start:idx].strip())
            start = idx + 1
    tail = params[start:].strip()
    if tail:
        parts.append(tail)
    return [part for part in parts if part]


def _rust_param_type(param: str) -> str:
    if ":" not in param:
        return param.strip()
    return param.split(":", 1)[1].strip()


def runtime_type_aliases(
    roots: Path | Iterable[Path] = RUNTIME_SRC_ROOTS,
) -> dict[str, str]:
    aliases: dict[str, str] = {}
    pattern = re.compile(r"\btype\s+([A-Za-z_][A-Za-z0-9_]*)\s*=\s*([^;]+);")
    for root in _iter_src_roots(roots):
        if not root.exists():
            continue
        for path in sorted(root.rglob("*.rs")):
            text = path.read_text(encoding="utf-8", errors="ignore")
            for name, target in pattern.findall(text):
                aliases[name] = target.strip()
    return aliases


def normalize_rust_type(rust_type: str, aliases: dict[str, str]) -> str:
    normalized = rust_type.strip()
    seen: set[str] = set()
    while normalized in aliases and normalized not in seen:
        seen.add(normalized)
        normalized = aliases[normalized].strip()
    return normalized


def runtime_exports(
    roots: Path | Iterable[Path] = RUNTIME_SRC_ROOTS,
) -> dict[str, RuntimeSignature]:
    pattern = re.compile(
        r"pub\s+(?:unsafe\s+)?extern\s+\"C\"\s+fn\s+"
        r"(?P<name>molt_[A-Za-z0-9_]+)\s*"
        r"\((?P<params>.*?)\)\s*"
        r"(?:->\s*(?P<ret>[^{\n]+))?\s*\{",
        re.DOTALL,
    )
    exports: dict[str, RuntimeSignature] = {}
    for root in _iter_src_roots(roots):
        if not root.exists():
            continue
        for path in sorted(root.rglob("*.rs")):
            rel = _display_path(path)
            text = path.read_text(encoding="utf-8", errors="ignore")
            for match in pattern.finditer(text):
                symbol = match.group("name")
                params = match.group("params").strip()
                rust_params = (
                    tuple(
                        _rust_param_type(param)
                        for param in _split_top_level_commas(params)
                    )
                    if params
                    else ()
                )
                arity = len(rust_params)
                rust_return = (match.group("ret") or "").strip()
                exports[symbol] = RuntimeSignature(
                    symbol, arity, rust_return, rel, "", rust_params
                )
    return exports


def _insert_abi_fact(
    facts: dict[tuple[str, int], AbiFact],
    duplicates: list[DuplicateAbiFact],
    fact: AbiFact,
) -> None:
    key = (fact.symbol, fact.arity)
    if key in facts:
        duplicates.append(
            DuplicateAbiFact(
                fact.symbol, fact.arity, facts[key].return_abi, fact.return_abi
            )
        )
    else:
        facts[key] = fact


def runtime_import_signature_constants(
    path: Path = RUNTIME_IMPORT_ABI_RS,
) -> dict[str, AbiFact]:
    text = path.read_text(encoding="utf-8")
    out: dict[str, AbiFact] = {}
    for const, symbol, arity, return_abi in re.findall(
        r"pub\(crate\)\s+const\s+([A-Z][A-Z0-9_]*)\s*:\s*"
        r"RuntimeImportSignature\s*=\s*runtime_sig\(\s*"
        r'"([^"]+)"\s*,\s*(\d+)\s*,\s*RuntimeReturnAbi::(I64|Void)\s*\)\s*;',
        text,
        re.DOTALL,
    ):
        out[const] = AbiFact(symbol, int(arity), return_abi, ("I64",) * int(arity))
    return out


def runtime_import_abi_facts(
    conservative_path: Path = RUNTIME_IMPORT_ABI_FACTS_RS,
    fixed_path: Path = RUNTIME_IMPORT_FIXED_RS,
    constants_path: Path = RUNTIME_IMPORT_ABI_RS,
) -> tuple[dict[tuple[str, int], AbiFact], tuple[DuplicateAbiFact, ...]]:
    conservative_text = conservative_path.read_text(encoding="utf-8")
    fixed_text = fixed_path.read_text(encoding="utf-8")
    constants = runtime_import_signature_constants(constants_path)
    facts: dict[tuple[str, int], AbiFact] = {}
    duplicates: list[DuplicateAbiFact] = []
    for match in re.finditer(
        r"runtime_sig\(\s*"
        r"\"(?P<name>molt_[^\"]+)\"\s*,\s*"
        r"(?P<arity>\d+)\s*,\s*"
        r"RuntimeReturnAbi::(?P<abi>I64|Void)\s*,?\s*\)",
        conservative_text,
        re.DOTALL,
    ):
        fact = AbiFact(
            match.group("name"),
            int(match.group("arity")),
            match.group("abi"),
            ("I64",) * int(match.group("arity")),
        )
        _insert_abi_fact(facts, duplicates, fact)
    for line in conservative_text.splitlines():
        stripped = line.split("//", maxsplit=1)[0].strip()
        match = re.fullmatch(r"([A-Z][A-Z0-9_]*)\s*,", stripped)
        if match is None:
            continue
        const = match.group(1)
        if const in constants:
            _insert_abi_fact(facts, duplicates, constants[const])
    for match in re.finditer(
        r"\b(?P<ctor>i64_ret|void_ret)\(\s*"
        r"\"(?P<name>molt_[^\"]+)\"\s*,\s*"
        r"(?P<arity>\d+)\s*,",
        fixed_text,
        re.DOTALL,
    ):
        return_abi = "I64" if match.group("ctor") == "i64_ret" else "Void"
        fact = AbiFact(
            match.group("name"),
            int(match.group("arity")),
            return_abi,
            ("I64",) * int(match.group("arity")),
        )
        _insert_abi_fact(facts, duplicates, fact)
    for match in re.finditer(
        r"\b(?P<ctor>i64_ret|void_ret)\(\s*"
        r"(?P<const>[A-Z][A-Z0-9_]*)\.name\s*,\s*"
        r"(?P<arity>\d+)\s*,",
        fixed_text,
        re.DOTALL,
    ):
        const = match.group("const")
        if const not in constants:
            continue
        fact = constants[const]
        expected_return = "I64" if match.group("ctor") == "i64_ret" else "Void"
        if (
            fact.arity == int(match.group("arity"))
            and fact.return_abi == expected_return
        ):
            _insert_abi_fact(facts, duplicates, fact)
    return facts, tuple(sorted(duplicates))


def rust_return_to_abi(
    rust_return: str, aliases: dict[str, str] | None = None
) -> str | None:
    normalized = normalize_rust_type(rust_return, aliases or {})
    if normalized in ABI_VOID_RETURNS:
        return "Void"
    if normalized in ABI_I64_RETURNS:
        return "I64"
    return None


def rust_param_to_abi(rust_param: str, aliases: dict[str, str]) -> str | None:
    normalized = normalize_rust_type(rust_param, aliases)
    if normalized in ABI_I64_PARAMS:
        return "I64"
    return None


def validate_classified_facts(
    exports: dict[str, RuntimeSignature],
    facts: dict[tuple[str, int], AbiFact],
    aliases: dict[str, str] | None = None,
) -> tuple[ClassifiedFactIssue, ...]:
    issues: list[ClassifiedFactIssue] = []
    aliases = aliases or {}
    for fact in sorted(facts.values()):
        export = exports.get(fact.symbol)
        if export is None:
            issues.append(
                ClassifiedFactIssue(
                    "missing-runtime-export",
                    fact.symbol,
                    fact.arity,
                    "<missing>",
                    "<missing>",
                    "<runtime-export>",
                    fact.return_abi,
                    "<missing>",
                )
            )
            continue

        if export.arity != fact.arity:
            issues.append(
                ClassifiedFactIssue(
                    "arity-mismatch",
                    fact.symbol,
                    fact.arity,
                    str(export.arity),
                    export.rust_return,
                    str(export.arity),
                    str(fact.arity),
                    export.source,
                )
            )

        actual_param_abis = tuple(
            rust_param_to_abi(param, aliases) or f"Unsupported({param})"
            for param in export.rust_params
        )
        if actual_param_abis != fact.param_abis:
            issues.append(
                ClassifiedFactIssue(
                    "param-mismatch",
                    fact.symbol,
                    fact.arity,
                    str(export.arity),
                    export.rust_return,
                    ",".join(fact.param_abis),
                    ",".join(actual_param_abis),
                    export.source,
                )
            )

        expected_return_abi = rust_return_to_abi(export.rust_return, aliases)
        if expected_return_abi is None:
            issues.append(
                ClassifiedFactIssue(
                    "unsupported-return",
                    fact.symbol,
                    fact.arity,
                    str(export.arity),
                    export.rust_return,
                    "<I64-or-Void>",
                    fact.return_abi,
                    export.source,
                )
            )
        elif expected_return_abi != fact.return_abi:
            issues.append(
                ClassifiedFactIssue(
                    "return-mismatch",
                    fact.symbol,
                    fact.arity,
                    str(export.arity),
                    export.rust_return,
                    expected_return_abi,
                    fact.return_abi,
                    export.source,
                )
            )

    return tuple(sorted(issues))


def run_audit(root: Path = ROOT) -> AuditResult:
    serialization = root / SERIALIZATION_PY.relative_to(ROOT)
    op_kinds = root / OP_KINDS_GENERATED_RS.relative_to(ROOT)
    conservative_imports = root / RUNTIME_IMPORT_ABI_FACTS_RS.relative_to(ROOT)
    fixed_imports = root / RUNTIME_IMPORT_FIXED_RS.relative_to(ROOT)
    runtime_import_abi = root / RUNTIME_IMPORT_ABI_RS.relative_to(ROOT)
    runtime_roots = runtime_src_roots(root)

    preserved_kinds = frontend_wire_kinds(serialization) - mapped_tir_kinds(op_kinds)
    exports = runtime_exports(runtime_roots)
    aliases = runtime_type_aliases(runtime_roots)
    facts, duplicates = runtime_import_abi_facts(
        conservative_imports, fixed_imports, runtime_import_abi
    )
    classified_fact_issues = validate_classified_facts(exports, facts, aliases)

    missing: list[AbiIssue] = []
    mismatched: list[AbiIssue] = []
    unexpected_non_boxed: list[RuntimeSignature] = []
    allowed_non_boxed: list[RuntimeSignature] = []

    for kind in sorted(preserved_kinds):
        symbol = f"molt_{kind}"
        export = exports.get(symbol)
        if export is None:
            continue
        expected = rust_return_to_abi(export.rust_return, aliases)
        export = RuntimeSignature(
            export.symbol,
            export.arity,
            export.rust_return,
            export.source,
            kind,
            export.rust_params,
        )
        if expected is None:
            key = (export.symbol, export.arity, export.rust_return)
            if key in ALLOWED_NON_BOXED_RETURNS:
                allowed_non_boxed.append(export)
            else:
                unexpected_non_boxed.append(export)
            continue

        actual = facts.get((symbol, export.arity))
        if actual is None:
            missing.append(
                AbiIssue(
                    symbol,
                    export.arity,
                    kind,
                    export.rust_return,
                    expected,
                    "<missing>",
                )
            )
        elif actual.return_abi != expected:
            mismatched.append(
                AbiIssue(
                    symbol,
                    export.arity,
                    kind,
                    export.rust_return,
                    expected,
                    actual.return_abi,
                )
            )

    return AuditResult(
        missing=tuple(sorted(missing)),
        mismatched=tuple(sorted(mismatched)),
        duplicate_facts=duplicates,
        classified_fact_issues=classified_fact_issues,
        unexpected_non_boxed=tuple(sorted(unexpected_non_boxed)),
        allowed_non_boxed=tuple(sorted(allowed_non_boxed)),
    )


def _jsonable(result: AuditResult) -> dict[str, object]:
    return asdict(result) | {"ok": result.ok}


def _format_issue(issue: AbiIssue) -> str:
    return (
        f"{issue.symbol}/{issue.arity} kind={issue.kind} rust_return={issue.rust_return} "
        f"expected={issue.expected} actual={issue.actual}"
    )


def format_report(result: AuditResult) -> str:
    lines = [f"LLVM runtime ABI audit: {'ok' if result.ok else 'FAILED'}"]
    if result.missing:
        lines.append("missing ABI facts:")
        lines.extend(f"  - {_format_issue(issue)}" for issue in result.missing)
    if result.mismatched:
        lines.append("mismatched ABI facts:")
        lines.extend(f"  - {_format_issue(issue)}" for issue in result.mismatched)
    if result.duplicate_facts:
        lines.append("duplicate ABI facts:")
        lines.extend(
            f"  - {dup.symbol}/{dup.arity} first={dup.first} second={dup.second}"
            for dup in result.duplicate_facts
        )
    if result.classified_fact_issues:
        lines.append("classified facts that do not match runtime exports:")
        lines.extend(
            "  - "
            f"{issue.problem}: {issue.symbol}/{issue.classified_arity} "
            f"rust_arity={issue.rust_arity} rust_return={issue.rust_return} "
            f"expected={issue.expected} actual={issue.actual} source={issue.source}"
            for issue in result.classified_fact_issues
        )
    if result.unexpected_non_boxed:
        lines.append("unexpected non-boxed preserved runtime returns:")
        lines.extend(
            f"  - {sig.symbol}/{sig.arity} kind={sig.kind} rust_return={sig.rust_return} source={sig.source}"
            for sig in result.unexpected_non_boxed
        )
    if result.allowed_non_boxed:
        lines.append("allowed fail-closed non-boxed returns:")
        lines.extend(
            f"  - {sig.symbol}/{sig.arity} kind={sig.kind} rust_return={sig.rust_return}"
            for sig in result.allowed_non_boxed
        )
    return "\n".join(lines)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--json", action="store_true", help="emit JSON audit output")
    parser.add_argument("--check", action="store_true", help="fail on ABI drift")
    args = parser.parse_args(argv)

    result = run_audit()
    if args.json:
        print(json.dumps(_jsonable(result), indent=2, sort_keys=True))
    else:
        print(format_report(result))
    return 0 if result.ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
