#!/usr/bin/env python3
"""Verify LLVM runtime import ABI authority for preserved-Copy runtime calls.

`MOLT_RUNTIME_CALLABLE_SYMBOLS` is availability, not representation authority.
Generic direct/preserved calls require normalized runtime_boxed_call_specs:
object-value operands and an owned object-value or void result. Integer machine
carriers never prove this contract. Fixed and residual dedicated declarations
are independently checked against native Rust export shapes.

Runtime import eligibility is generated from normalized boxed-call contracts.
OpKind exhaustiveness and typed selectors replace source-spelling inference;
this audit verifies every declared contract against actual runtime exports.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from collections.abc import Iterable
from dataclasses import asdict, dataclass
from functools import lru_cache
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

ROOT = Path(__file__).resolve().parents[1]


RUNTIME_IMPORT_ABI_FACTS_RS = (
    ROOT / "runtime/molt-backend-native/src/llvm_backend/runtime_imports/abi_facts.rs"
)
RUNTIME_IMPORT_FIXED_RS = (
    ROOT / "runtime/molt-backend-native/src/llvm_backend/runtime_imports/fixed.rs"
)
RUNTIME_IMPORT_ABI_RS = ROOT / "runtime/molt-backend-native/src/runtime_import_abi.rs"

ABI_I64_RETURNS = {"u64", "i64"}
ABI_I32_RETURNS = {"u32", "i32"}
ABI_VOID_RETURNS = {"", "()", "void"}
ABI_I64_PARAMS = {"u64", "i64"}


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
    duplicate_facts: tuple[DuplicateAbiFact, ...]
    classified_fact_issues: tuple[ClassifiedFactIssue, ...]
    boxed_fact_issues: tuple[ClassifiedFactIssue, ...]

    @property
    def ok(self) -> bool:
        return not (
            self.duplicate_facts
            or self.classified_fact_issues
            or self.boxed_fact_issues
        )


@lru_cache(maxsize=1)
def runtime_boxed_abi_facts() -> dict[tuple[str, int], AbiFact]:
    """The normalized semantic projection used by the generator and both audits."""
    from tools.wasm_abi_gen.manifest import load_manifest, runtime_boxed_call_specs

    return {
        (row["runtime_name"], row["arity"]): AbiFact(
            row["runtime_name"],
            row["arity"],
            "I64" if row["result"] == "i64" else "Void",
            ("I64",) * row["arity"],
        )
        for row in runtime_boxed_call_specs(load_manifest())
    }


@lru_cache(maxsize=1)
def boxed_runtime_export_issues() -> tuple[ClassifiedFactIssue, ...]:
    """Validate boxed signatures using the generator's complete export scan.

    That scan owns runtime dependency leaves and conditional export variants;
    the native pointer/custom declaration audit remains separate below.
    """
    from tools.wasm_abi_gen.manifest import generator_runtime_export_signature_rows

    issues: list[ClassifiedFactIssue] = []
    exports: dict[str, list[RuntimeSignature]] = {}
    for symbol, params, result in generator_runtime_export_signature_rows():
        exports.setdefault(symbol, []).append(
            RuntimeSignature(
                symbol, len(params), result, "<normalized-runtime-export>", "", params
            )
        )
    for key, fact in runtime_boxed_abi_facts().items():
        candidates = exports.get(fact.symbol, [])
        if not candidates:
            issues.extend(validate_classified_facts({}, {key: fact}))
        for export in candidates:
            issues.extend(validate_classified_facts({fact.symbol: export}, {key: fact}))
    return tuple(sorted(issues))


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


def fixed_runtime_param_abi_constants(fixed_text: str) -> dict[str, tuple[str, ...]]:
    param_constants: dict[str, tuple[str, ...]] = {}
    for match in re.finditer(
        r"const\s+(?P<name>[A-Z][A-Z0-9_]*)\s*:\s*&\[FixedRuntimeParamAbi\]\s*=\s*&\[(?P<body>.*?)\]\s*;",
        fixed_text,
        re.DOTALL,
    ):
        param_constants[match.group("name")] = tuple(
            re.findall(r"FixedRuntimeParamAbi::(I64|Ptr)", match.group("body"))
        )
    return param_constants


def runtime_import_abi_facts(
    conservative_path: Path = RUNTIME_IMPORT_ABI_FACTS_RS,
    fixed_path: Path = RUNTIME_IMPORT_FIXED_RS,
    constants_path: Path = RUNTIME_IMPORT_ABI_RS,
    *,
    include_fixed: bool = True,
) -> tuple[dict[tuple[str, int], AbiFact], tuple[DuplicateAbiFact, ...]]:
    conservative_text = conservative_path.read_text(encoding="utf-8")
    fixed_text = fixed_path.read_text(encoding="utf-8") if include_fixed else ""
    constants = runtime_import_signature_constants(constants_path)
    fixed_param_constants = fixed_runtime_param_abi_constants(fixed_text)
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
    for match in re.finditer(
        r"\bcustom\(\s*"
        r"\"(?P<name>molt_[^\"]+)\"\s*,\s*"
        r"(?P<params>[A-Z][A-Z0-9_]*)\s*,\s*"
        r"FixedRuntimeReturnAbi::(?P<abi>I64|I32)\s*,",
        fixed_text,
        re.DOTALL,
    ):
        param_abis = fixed_param_constants.get(match.group("params"))
        if param_abis is None:
            continue
        fact = AbiFact(
            match.group("name"),
            len(param_abis),
            match.group("abi"),
            param_abis,
        )
        _insert_abi_fact(facts, duplicates, fact)
    return facts, tuple(sorted(duplicates))


def rust_return_to_abi(
    rust_return: str, aliases: dict[str, str] | None = None, *, allow_i32: bool = False
) -> str | None:
    normalized = normalize_rust_type(rust_return, aliases or {})
    if normalized in ABI_VOID_RETURNS:
        return "Void"
    if normalized in ABI_I64_RETURNS:
        return "I64"
    if allow_i32 and normalized in ABI_I32_RETURNS:
        return "I32"
    return None


def rust_param_to_abi(rust_param: str, aliases: dict[str, str]) -> str | None:
    normalized = normalize_rust_type(rust_param, aliases)
    if normalized in ABI_I64_PARAMS:
        return "I64"
    if normalized.startswith("*mut ") or normalized.startswith("*const "):
        return "Ptr"
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

        expected_return_abi = rust_return_to_abi(
            export.rust_return, aliases, allow_i32=True
        )
        if expected_return_abi is None:
            issues.append(
                ClassifiedFactIssue(
                    "unsupported-return",
                    fact.symbol,
                    fact.arity,
                    str(export.arity),
                    export.rust_return,
                    "<I64-I32-or-Void>",
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
    conservative_imports = root / RUNTIME_IMPORT_ABI_FACTS_RS.relative_to(ROOT)
    fixed_imports = root / RUNTIME_IMPORT_FIXED_RS.relative_to(ROOT)
    runtime_import_abi = root / RUNTIME_IMPORT_ABI_RS.relative_to(ROOT)
    runtime_roots = runtime_src_roots(root)
    exports = runtime_exports(runtime_roots)
    aliases = runtime_type_aliases(runtime_roots)
    facts, duplicates = runtime_import_abi_facts(
        conservative_imports, fixed_imports, runtime_import_abi
    )
    boxed = runtime_boxed_abi_facts()
    classified_fact_issues = list(validate_classified_facts(exports, facts, aliases))

    # Fixed declarations retain stronger attributes; matching generated/fixed
    # facts are therefore legitimate. Conservative mirrors must be deleted.
    conservative, _ = runtime_import_abi_facts(
        conservative_imports, fixed_imports, runtime_import_abi, include_fixed=False
    )
    duplicates = list(duplicates)
    for key, fact in conservative.items():
        if key in boxed:
            duplicates.append(
                DuplicateAbiFact(
                    fact.symbol, fact.arity, "conservative", "generated-boxed"
                )
            )
    for key in facts.keys() & boxed.keys():
        if facts[key] != boxed[key]:
            fact = facts[key]
            classified_fact_issues.append(
                ClassifiedFactIssue(
                    "boxed-declaration-mismatch",
                    fact.symbol,
                    fact.arity,
                    str(fact.arity),
                    boxed[key].return_abi,
                    str(boxed[key]),
                    str(fact),
                    "<native-declaration>",
                )
            )

    return AuditResult(
        duplicate_facts=tuple(sorted(duplicates)),
        classified_fact_issues=tuple(sorted(classified_fact_issues)),
        boxed_fact_issues=boxed_runtime_export_issues(),
    )


def _jsonable(result: AuditResult) -> dict[str, object]:
    return asdict(result) | {"ok": result.ok}


def format_report(result: AuditResult) -> str:
    lines = [f"LLVM runtime ABI audit: {'ok' if result.ok else 'FAILED'}"]
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
    if result.boxed_fact_issues:
        lines.append("boxed contracts that do not match runtime exports:")
        lines.extend(
            f"  - {issue.problem}: {issue.symbol}/{issue.classified_arity} "
            f"expected={issue.expected} actual={issue.actual}"
            for issue in result.boxed_fact_issues
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
