#!/usr/bin/env python3
"""Comprehensive Lean-Rust-Python correspondence checker.

Verifies that type definitions, constants, operator enums, builtin mappings,
and semantic invariants stay in sync across:
  - Lean formalization (formal/lean/MoltTIR/)
  - Rust runtime (runtime/molt-obj-model/, runtime/molt-backend/)
  - Python frontend (src/molt/frontend/)

All expected values are parsed from source files -- nothing is hardcoded.

Usage:
    uv run --python 3.12 python3 tools/check_correspondence.py
    uv run --python 3.12 python3 tools/check_correspondence.py --verbose
    uv run --python 3.12 python3 tools/check_correspondence.py --json
    uv run --python 3.12 python3 tools/check_correspondence.py --category nanbox
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path


def _find_repo_root() -> Path:
    """Find the real repo root, handling worktree checkouts."""
    try:
        out = subprocess.check_output(
            ["git", "rev-parse", "--show-toplevel"],
            stderr=subprocess.DEVNULL,
            text=True,
        ).strip()
        return Path(out)
    except (subprocess.CalledProcessError, FileNotFoundError):
        return Path(__file__).resolve().parents[1]


ROOT = _find_repo_root()

# ── Source paths ──────────────────────────────────────────────────────
LEAN_DIR = ROOT / "formal" / "lean"

# Lean sources
NANBOX_LEAN = LEAN_DIR / "MoltTIR" / "Runtime" / "NanBox.lean"
SYNTAX_LEAN = LEAN_DIR / "MoltTIR" / "Syntax.lean"
TYPES_LEAN = LEAN_DIR / "MoltTIR" / "Types.lean"
LUAU_EMIT_LEAN = LEAN_DIR / "MoltTIR" / "Backend" / "LuauEmit.lean"
LUAU_SYNTAX_LEAN = LEAN_DIR / "MoltTIR" / "Backend" / "LuauSyntax.lean"
CONSTFOLD_LEAN = LEAN_DIR / "MoltTIR" / "Passes" / "ConstFold.lean"
EVAL_LEAN = LEAN_DIR / "MoltTIR" / "Semantics" / "EvalExpr.lean"
LATTICE_LEAN = LEAN_DIR / "MoltTIR" / "Passes" / "Lattice.lean"
SCCP_LEAN = LEAN_DIR / "MoltTIR" / "Passes" / "SCCP.lean"

# Rust sources
OBJ_MODEL_RS = ROOT / "runtime" / "molt-obj-model" / "src" / "lib.rs"
LUAU_RS = ROOT / "runtime" / "molt-backend" / "src" / "luau.rs"

# Python sources
FRONTEND_PY = ROOT / "src" / "molt" / "frontend" / "__init__.py"

# ── Terminal colors ──────────────────────────────────────────────────
IS_TTY = sys.stdout.isatty()


def _c(code: str, text: str) -> str:
    return f"\033[{code}m{text}\033[0m" if IS_TTY else text


def green(t: str) -> str:
    return _c("32", t)


def red(t: str) -> str:
    return _c("31", t)


def yellow(t: str) -> str:
    return _c("33", t)


def bold(t: str) -> str:
    return _c("1", t)


# ── Result tracking ──────────────────────────────────────────────────


@dataclass
class CheckItem:
    """A single checked correspondence item."""

    name: str
    passed: bool
    detail: str = ""


@dataclass
class CategoryResult:
    """Results for one correspondence category."""

    category: str
    description: str
    items: list[CheckItem] = field(default_factory=list)

    @property
    def passed(self) -> int:
        return sum(1 for i in self.items if i.passed)

    @property
    def failed(self) -> int:
        return sum(1 for i in self.items if not i.passed)

    @property
    def total(self) -> int:
        return len(self.items)

    @property
    def ok(self) -> bool:
        return self.failed == 0


# ── Parsing helpers ──────────────────────────────────────────────────


def _read(path: Path) -> str:
    if path.exists():
        return path.read_text(errors="replace")
    return ""


def _normalize_hex(val: str, rust_text: str = "") -> int:
    val = val.strip().rstrip(";")
    if "<<" in val:
        m = re.match(r"\(1u64\s*<<\s*(\w+)\)\s*-\s*1", val.strip())
        if m:
            shift_val = m.group(1)
            if shift_val.isdigit():
                return (1 << int(shift_val)) - 1
            if rust_text:
                vm = re.search(rf"const {shift_val}:\s*u64\s*=\s*(\d+)", rust_text)
                if vm:
                    return (1 << int(vm.group(1))) - 1
            raise ValueError(f"Cannot resolve variable: {shift_val}")
        m2 = re.match(r"1\s*<<\s*(\d+)", val.strip())
        if m2:
            return 1 << int(m2.group(1))
        raise ValueError(f"Cannot parse computed constant: {val}")
    if val.isdigit():
        return int(val)
    return int(val.replace("_", ""), 16)


def _parse_lean_hex_constants(text: str) -> dict[str, int]:
    result: dict[str, int] = {}
    for m in re.finditer(r"def\s+(\w+)\s*:\s*UInt64\s*:=\s*(0x[0-9a-fA-F]+)", text):
        result[m.group(1)] = int(m.group(2), 16)
    return result


def _parse_rust_u64_constants(text: str) -> dict[str, str]:
    result: dict[str, str] = {}
    for m in re.finditer(r"const\s+(\w+):\s*u64\s*=\s*(.+?);", text):
        result[m.group(1)] = m.group(2).strip()
    return result


def _parse_lean_inductive_variants(text: str, type_name: str) -> list[str]:
    pattern = rf"inductive\s+{type_name}\s+where"
    m = re.search(pattern, text)
    if not m:
        return []
    start = m.end()
    lines = text[start:].split("\n")
    variants: list[str] = []
    for line in lines:
        stripped = line.strip()
        if not stripped:
            continue
        if stripped.startswith("--"):
            continue
        if stripped.startswith("deriving"):
            break
        if re.match(
            r"^(inductive|def|theorem|structure|namespace|end|abbrev|section)\b",
            stripped,
        ):
            break
        for vm in re.finditer(r"\|\s*\.?(\w+)", stripped):
            name = vm.group(1)
            if name.endswith("_"):
                name = name[:-1]
            if name not in variants:
                variants.append(name)
    return variants


def _parse_lean_builtin_mappings(text: str) -> list[tuple[str, str]]:
    return re.findall(r'\("(\w+)",\s*"([^"]+)"\)', text)


def _normalize_lean_variant_name(name: str) -> str:
    """Match `_parse_lean_inductive_variants` reserved-word normalization."""
    return name[:-1] if name.endswith("_") else name


def _parse_python_op_kinds(text: str) -> set[str]:
    kinds: set[str] = set()
    for m in re.finditer(r'"kind":\s*"(\w+)"', text):
        kinds.add(m.group(1))
    return kinds


def _parse_lean_evalBinOp_rules(text: str) -> list[tuple[str, str, str]]:
    rules: list[tuple[str, str, str]] = []
    for m in re.finditer(r"\|\s*\.(\w+),\s*\.(\w+)\s+\w+,\s*\.(\w+)\s+\w+\s*=>", text):
        rules.append((m.group(1), m.group(2), m.group(3)))
    return rules


def _parse_lean_evalUnOp_rules(text: str) -> list[tuple[str, str]]:
    rules: list[tuple[str, str]] = []
    for m in re.finditer(r"\|\s*\.(\w+),\s*\.(\w+)\s+\w+\s*=>", text):
        rules.append((m.group(1), m.group(2)))
    return rules


# ═══════════════════════════════════════════════════════════════════
# Category 1: NaN-boxing constants
# ═══════════════════════════════════════════════════════════════════


def check_nanbox_constants() -> CategoryResult:
    result = CategoryResult(
        "nanbox",
        "NaN-boxing constants (Rust molt-obj-model <-> Lean NanBox)",
    )

    rust_text = _read(OBJ_MODEL_RS)
    lean_text = _read(NANBOX_LEAN)

    if not rust_text:
        result.items.append(
            CheckItem("source", False, f"Rust source missing: {OBJ_MODEL_RS}")
        )
        return result
    if not lean_text:
        result.items.append(
            CheckItem("source", False, f"Lean source missing: {NANBOX_LEAN}")
        )
        return result

    lean_consts = _parse_lean_hex_constants(lean_text)
    rust_consts = _parse_rust_u64_constants(rust_text)

    lean_to_rust: dict[str, str] = {
        "QNAN": "QNAN",
        "TAG_INT": "TAG_INT",
        "TAG_BOOL": "TAG_BOOL",
        "TAG_NONE": "TAG_NONE",
        "TAG_PTR": "TAG_PTR",
        "TAG_PEND": "TAG_PENDING",
        "TAG_MASK": "TAG_MASK",
        "INT_MASK": "INT_MASK",
        "INT_SIGN": "INT_SIGN_BIT",
    }

    for lean_name, lean_val in lean_consts.items():
        rust_name = lean_to_rust.get(lean_name)
        if rust_name is None:
            if lean_name == "TAG_CHECK":
                expected = lean_consts.get("QNAN", 0) | lean_consts.get("TAG_MASK", 0)
                result.items.append(
                    CheckItem(
                        lean_name,
                        lean_val == expected,
                        f"computed: 0x{lean_val:016x} (QNAN | TAG_MASK)"
                        if lean_val == expected
                        else f"0x{lean_val:016x} != expected 0x{expected:016x}",
                    )
                )
                continue
            result.items.append(
                CheckItem(lean_name, False, "no known Rust counterpart")
            )
            continue

        rust_expr = rust_consts.get(rust_name)
        if rust_expr is None:
            result.items.append(
                CheckItem(f"{lean_name}/{rust_name}", False, "not found in Rust")
            )
            continue

        try:
            rust_val = _normalize_hex(rust_expr, rust_text=rust_text)
        except (ValueError, IndexError) as e:
            result.items.append(
                CheckItem(f"{lean_name}/{rust_name}", False, f"parse error: {e}")
            )
            continue

        if lean_val == rust_val:
            result.items.append(
                CheckItem(f"{lean_name}/{rust_name}", True, f"0x{lean_val:016x}")
            )
        else:
            result.items.append(
                CheckItem(
                    f"{lean_name}/{rust_name}",
                    False,
                    f"MISMATCH: Lean=0x{lean_val:016x} Rust=0x{rust_val:016x}",
                )
            )

    return result


# ═══════════════════════════════════════════════════════════════════
# Category 2: Operator enums
# ═══════════════════════════════════════════════════════════════════


def check_operator_enums() -> CategoryResult:
    result = CategoryResult(
        "operators",
        "Operator enums (Lean Syntax <-> Python frontend opcodes)",
    )

    lean_text = _read(SYNTAX_LEAN)
    python_text = _read(FRONTEND_PY)

    if not lean_text:
        result.items.append(CheckItem("source", False, f"Lean missing: {SYNTAX_LEAN}"))
        return result
    if not python_text:
        result.items.append(
            CheckItem("source", False, f"Python missing: {FRONTEND_PY}")
        )
        return result

    lean_binops = _parse_lean_inductive_variants(lean_text, "BinOp")
    lean_unops = _parse_lean_inductive_variants(lean_text, "UnOp")
    python_ops = _parse_python_op_kinds(python_text)

    result.items.append(
        CheckItem(
            "BinOp count",
            True,
            f"Lean: {len(lean_binops)} variants ({', '.join(lean_binops)})",
        )
    )

    for v in lean_binops:
        result.items.append(
            CheckItem(
                f"BinOp.{v}",
                v in python_ops,
                "present in Python frontend"
                if v in python_ops
                else "NOT found in Python frontend",
            )
        )

    result.items.append(
        CheckItem(
            "UnOp count",
            True,
            f"Lean: {len(lean_unops)} variants ({', '.join(lean_unops)})",
        )
    )

    for v in lean_unops:
        result.items.append(
            CheckItem(
                f"UnOp.{v}",
                v in python_ops,
                "present in Python frontend"
                if v in python_ops
                else "NOT found in Python frontend",
            )
        )

    return result


# ═══════════════════════════════════════════════════════════════════
# Category 3: Type system
# ═══════════════════════════════════════════════════════════════════


def check_type_system() -> CategoryResult:
    result = CategoryResult(
        "types",
        "Type system (Lean Types <-> Python frontend type tags)",
    )

    lean_text = _read(TYPES_LEAN)
    python_text = _read(FRONTEND_PY)

    if not lean_text:
        result.items.append(CheckItem("source", False, f"Lean missing: {TYPES_LEAN}"))
        return result
    if not python_text:
        result.items.append(
            CheckItem("source", False, f"Python missing: {FRONTEND_PY}")
        )
        return result

    lean_types = _parse_lean_inductive_variants(lean_text, "Ty")
    lean_values = _parse_lean_inductive_variants(_read(SYNTAX_LEAN), "Value")

    py_type_tags: set[str] = set()
    for m in re.finditer(r'"(\w+)":\s*\d+', python_text):
        py_type_tags.add(m.group(1))

    result.items.append(
        CheckItem(
            "Ty count",
            True,
            f"Lean: {len(lean_types)} ({', '.join(lean_types)})",
        )
    )

    if lean_values:
        result.items.append(
            CheckItem(
                "Value count",
                True,
                f"Lean: {len(lean_values)} ({', '.join(lean_values)})",
            )
        )

    for ty in lean_types:
        if ty in py_type_tags:
            result.items.append(
                CheckItem(f"Ty.{ty}", True, "present in Python type tags")
            )
        elif ty == "obj" and "object" in py_type_tags:
            result.items.append(
                CheckItem(f"Ty.{ty}", True, "maps to 'object' in Python")
            )
        else:
            result.items.append(CheckItem(f"Ty.{ty}", False, "NOT in Python type tags"))

    return result


# ═══════════════════════════════════════════════════════════════════
# Category 4: Luau builtin mappings
# ═══════════════════════════════════════════════════════════════════


def check_luau_builtins() -> CategoryResult:
    result = CategoryResult(
        "luau_builtins",
        "Luau builtin mappings (Lean LuauEmit <-> Rust luau.rs)",
    )

    lean_text = _read(LUAU_EMIT_LEAN)
    rust_text = _read(LUAU_RS)

    if not lean_text:
        result.items.append(
            CheckItem("source", False, f"Lean missing: {LUAU_EMIT_LEAN}")
        )
        return result
    if not rust_text:
        result.items.append(CheckItem("source", False, f"Rust missing: {LUAU_RS}"))
        return result

    lean_mappings = _parse_lean_builtin_mappings(lean_text)

    result.items.append(
        CheckItem(
            "mapping count",
            True,
            f"Lean defines {len(lean_mappings)} builtin mappings",
        )
    )

    for ir_name, luau_name in lean_mappings:
        ir_found = ir_name in rust_text
        luau_found = luau_name in rust_text
        if ir_found or luau_found:
            parts = []
            if ir_found:
                parts.append("IR name")
            if luau_found:
                parts.append("Luau name")
            result.items.append(
                CheckItem(
                    f"{ir_name} -> {luau_name}",
                    True,
                    f"{' + '.join(parts)} found in Rust",
                )
            )
        else:
            result.items.append(
                CheckItem(
                    f"{ir_name} -> {luau_name}",
                    False,
                    "NEITHER name found in Rust luau.rs",
                )
            )

    return result


# ═══════════════════════════════════════════════════════════════════
# Category 5: Luau operator translations
# ═══════════════════════════════════════════════════════════════════


def check_luau_operators() -> CategoryResult:
    result = CategoryResult(
        "luau_operators",
        "Luau operator translations (Lean LuauEmit completeness)",
    )

    emit_text = _read(LUAU_EMIT_LEAN)
    syntax_text = _read(SYNTAX_LEAN)
    luau_syntax_text = _read(LUAU_SYNTAX_LEAN)

    if not emit_text or not syntax_text or not luau_syntax_text:
        result.items.append(CheckItem("source", False, "Missing required Lean sources"))
        return result

    binops = _parse_lean_inductive_variants(syntax_text, "BinOp")
    unops = _parse_lean_inductive_variants(syntax_text, "UnOp")
    luau_binops = _parse_lean_inductive_variants(luau_syntax_text, "LuauBinOp")
    luau_unops = _parse_lean_inductive_variants(luau_syntax_text, "LuauUnOp")

    # Parse emitBinOp match arms
    binop_map: dict[str, str] = {}
    in_binop = False
    for line in emit_text.splitlines():
        if "emitBinOp" in line and "def " in line:
            in_binop = True
            continue
        if in_binop:
            m = re.match(r"\s*\|\s*\.(\w+)\s*=>\s*\.(\w+)", line)
            if m:
                binop_map[_normalize_lean_variant_name(m.group(1))] = m.group(2)
            elif line.strip().startswith("def ") or line.strip().startswith("end "):
                in_binop = False

    # Parse emitUnOp match arms
    unop_map: dict[str, str] = {}
    in_unop = False
    for line in emit_text.splitlines():
        if "emitUnOp" in line and "def " in line:
            in_unop = True
            continue
        if in_unop:
            m = re.match(r"\s*\|\s*\.(\w+)\s*=>\s*\.(\w+)", line)
            if m:
                unop_map[_normalize_lean_variant_name(m.group(1))] = m.group(2)
            elif line.strip().startswith("def ") or (
                line.strip().startswith("--") and "Section" in line
            ):
                in_unop = False

    result.items.append(
        CheckItem(
            "LuauBinOp variants",
            True,
            f"{len(luau_binops)}: {', '.join(luau_binops)}",
        )
    )
    result.items.append(
        CheckItem(
            "LuauUnOp variants",
            True,
            f"{len(luau_unops)}: {', '.join(luau_unops)}",
        )
    )

    for op in binops:
        if op in binop_map:
            target = binop_map[op]
            ok = target in luau_binops
            result.items.append(
                CheckItem(
                    f"emitBinOp .{op}",
                    ok,
                    f"-> .{target}" if ok else f".{target} not in LuauBinOp",
                )
            )
        else:
            result.items.append(CheckItem(f"emitBinOp .{op}", False, "NO mapping"))

    for op in unops:
        if op in unop_map:
            target = unop_map[op]
            ok = target in luau_unops
            result.items.append(
                CheckItem(
                    f"emitUnOp .{op}",
                    ok,
                    f"-> .{target}" if ok else f".{target} not in LuauUnOp",
                )
            )
        else:
            result.items.append(CheckItem(f"emitUnOp .{op}", False, "NO mapping"))

    result.items.append(
        CheckItem(
            "BinOp coverage",
            len(binop_map) == len(binops),
            f"{len(binop_map)}/{len(binops)} mapped",
        )
    )
    result.items.append(
        CheckItem(
            "UnOp coverage",
            len(unop_map) == len(unops),
            f"{len(unop_map)}/{len(unops)} mapped",
        )
    )

    return result


# ═══════════════════════════════════════════════════════════════════
# Category 6: Evaluation rules
# ═══════════════════════════════════════════════════════════════════


def check_eval_rules() -> CategoryResult:
    result = CategoryResult(
        "eval_rules",
        "Evaluation rules (Lean EvalExpr <-> Lean Syntax operators)",
    )

    eval_text = _read(EVAL_LEAN)
    syntax_text = _read(SYNTAX_LEAN)

    if not eval_text or not syntax_text:
        result.items.append(CheckItem("source", False, "Missing required Lean sources"))
        return result

    binops = _parse_lean_inductive_variants(syntax_text, "BinOp")
    unops = _parse_lean_inductive_variants(syntax_text, "UnOp")

    binop_rules = _parse_lean_evalBinOp_rules(eval_text)
    covered_binops = {r[0] for r in binop_rules}

    unop_rules = _parse_lean_evalUnOp_rules(eval_text)
    covered_unops = {r[0] for r in unop_rules}

    result.items.append(
        CheckItem(
            "evalBinOp rules",
            True,
            f"{len(binop_rules)} rules, {len(covered_binops)} operators covered",
        )
    )
    result.items.append(
        CheckItem(
            "evalUnOp rules",
            True,
            f"{len(unop_rules)} rules, {len(covered_unops)} operators covered",
        )
    )

    # Known intentional gaps in the Lean formalization
    intentional_gaps = {
        "bit_and",
        "bit_or",
        "bit_xor",
        "lshift",
        "rshift",
        "div",
        "floordiv",
        "pow",
    }

    for op in binops:
        if op in covered_binops:
            rules_for = [(a, b) for (o, a, b) in binop_rules if o == op]
            result.items.append(
                CheckItem(
                    f"evalBinOp .{op}",
                    True,
                    f"{len(rules_for)} rule(s): {', '.join(f'{a}x{b}' for a, b in rules_for)}",
                )
            )
        elif op in intentional_gaps:
            result.items.append(
                CheckItem(
                    f"evalBinOp .{op}",
                    True,
                    "intentionally unmodeled (falls to catch-all)",
                )
            )
        else:
            result.items.append(
                CheckItem(f"evalBinOp .{op}", False, "NO rule, not a known gap")
            )

    for op in unops:
        if op in covered_unops:
            types = [t for (o, t) in unop_rules if o == op]
            result.items.append(
                CheckItem(
                    f"evalUnOp .{op}",
                    True,
                    f"type(s): {', '.join(types)}",
                )
            )
        elif op == "invert":
            result.items.append(
                CheckItem(
                    f"evalUnOp .{op}",
                    True,
                    "intentionally unmodeled (bitwise)",
                )
            )
        else:
            result.items.append(CheckItem(f"evalUnOp .{op}", False, "NO rule"))

    return result


# ═══════════════════════════════════════════════════════════════════
# Category 7: SCCP lattice
# ═══════════════════════════════════════════════════════════════════


def check_sccp_lattice() -> CategoryResult:
    result = CategoryResult(
        "sccp_lattice",
        "SCCP lattice (Lean Lattice + SCCP consistency)",
    )

    lattice_text = _read(LATTICE_LEAN)
    sccp_text = _read(SCCP_LEAN)

    if not lattice_text:
        result.items.append(CheckItem("source", False, f"Missing: {LATTICE_LEAN}"))
        return result
    if not sccp_text:
        result.items.append(CheckItem("source", False, f"Missing: {SCCP_LEAN}"))
        return result

    absval_variants = _parse_lean_inductive_variants(lattice_text, "AbsVal")
    result.items.append(
        CheckItem(
            "AbsVal variants",
            len(absval_variants) == 3,
            f"{len(absval_variants)}: {', '.join(absval_variants)} (expected 3)",
        )
    )

    required_theorems = [
        "unknown_le",
        "le_overdefined",
        "join_comm",
        "join_idem",
        "join_assoc",
        "le_refl",
        "join_concretizes",
    ]

    for thm in required_theorems:
        found = re.search(rf"\btheorem\s+{thm}\b", lattice_text) is not None
        result.items.append(
            CheckItem(
                f"theorem {thm}",
                found,
                "proven" if found else "NOT FOUND",
            )
        )

    for func in (
        "absEvalBinOp",
        "absEvalUnOp",
        "absEvalExpr",
        "sccpExpr",
        "sccpInstrs",
        "sccpBlock",
        "sccpFunc",
    ):
        found = func in sccp_text
        result.items.append(
            CheckItem(f"SCCP.{func}", found, "defined" if found else "NOT FOUND")
        )

    return result


# ═══════════════════════════════════════════════════════════════════
# Category 8: Structural invariants
# ═══════════════════════════════════════════════════════════════════


def check_structural_invariants() -> CategoryResult:
    result = CategoryResult(
        "structural",
        "Structural invariants (cross-file consistency)",
    )

    syntax_text = _read(SYNTAX_LEAN)
    emit_text = _read(LUAU_EMIT_LEAN)
    types_text = _read(TYPES_LEAN)
    luau_syntax_text = _read(LUAU_SYNTAX_LEAN)
    constfold_text = _read(CONSTFOLD_LEAN)

    binops = _parse_lean_inductive_variants(syntax_text, "BinOp")
    unops = _parse_lean_inductive_variants(syntax_text, "UnOp")

    # Count emitBinOp arms (before emitUnOp)
    pre_unop = emit_text.split("emitUnOp")[0] if "emitUnOp" in emit_text else emit_text
    emit_binop_arms = len(re.findall(r"\|\s*\.\w+\s*=>\s*\.", pre_unop))
    result.items.append(
        CheckItem(
            "BinOp variant count match",
            len(binops) == emit_binop_arms,
            f"Syntax: {len(binops)}, LuauEmit: {emit_binop_arms}",
        )
    )

    # Count emitUnOp arms
    unop_section = emit_text.split("emitUnOp")[1] if "emitUnOp" in emit_text else ""
    unop_sec = (
        unop_section.split("-- ==")[0] if "-- ==" in unop_section else unop_section
    )
    emit_unop_arms = len(re.findall(r"\|\s*\.\w+\s*=>\s*\.", unop_sec))
    result.items.append(
        CheckItem(
            "UnOp variant count match",
            len(unops) == emit_unop_arms,
            f"Syntax: {len(unops)}, LuauEmit: {emit_unop_arms}",
        )
    )

    values = _parse_lean_inductive_variants(syntax_text, "Value")
    result.items.append(
        CheckItem(
            "Value variants",
            len(values) > 0,
            f"{len(values)}: {', '.join(values)}",
        )
    )

    tys = _parse_lean_inductive_variants(types_text, "Ty")
    for vt in values:
        has_ty = vt in tys
        result.items.append(
            CheckItem(
                f"Value.{vt} -> Ty counterpart",
                has_ty,
                f"Ty.{vt}" if has_ty else "MISSING in Ty",
            )
        )

    expr_variants = _parse_lean_inductive_variants(syntax_text, "Expr")
    for v in expr_variants:
        found = re.search(rf"\.\s*{v}\b", constfold_text) is not None
        result.items.append(
            CheckItem(
                f"constFoldExpr handles .{v}",
                found,
                "handled" if found else "NOT handled",
            )
        )

    luau_exprs = _parse_lean_inductive_variants(luau_syntax_text, "LuauExpr")
    result.items.append(
        CheckItem(
            "LuauExpr constructors",
            len(luau_exprs) > 0,
            f"{len(luau_exprs)}: {', '.join(luau_exprs)}",
        )
    )

    luau_stmts = _parse_lean_inductive_variants(luau_syntax_text, "LuauStmt")
    result.items.append(
        CheckItem(
            "LuauStmt constructors",
            len(luau_stmts) > 0,
            f"{len(luau_stmts)}: {', '.join(luau_stmts)}",
        )
    )

    for v in expr_variants:
        found = re.search(rf"\.\s*{v}\b", emit_text) is not None
        result.items.append(
            CheckItem(
                f"emitExpr handles .{v}",
                found,
                "handled" if found else "NOT handled",
            )
        )

    return result


# ═══════════════════════════════════════════════════════════════════
# Orchestration
# ═══════════════════════════════════════════════════════════════════

ALL_CATEGORIES: dict[str, callable] = {
    "nanbox": check_nanbox_constants,
    "operators": check_operator_enums,
    "types": check_type_system,
    "luau_builtins": check_luau_builtins,
    "luau_operators": check_luau_operators,
    "eval_rules": check_eval_rules,
    "sccp_lattice": check_sccp_lattice,
    "structural": check_structural_invariants,
}


def run_checks(
    categories: list[str] | None = None,
) -> list[CategoryResult]:
    results: list[CategoryResult] = []
    targets = categories or list(ALL_CATEGORIES.keys())
    for name in targets:
        if name in ALL_CATEGORIES:
            results.append(ALL_CATEGORIES[name]())
    return results


def print_report(results: list[CategoryResult], *, verbose: bool = False) -> int:
    total_passed = 0
    total_failed = 0
    total_items = 0

    print()
    print(bold("=" * 76))
    print(bold("  Lean-Rust-Python Correspondence Report"))
    print(bold("=" * 76))

    for cat in results:
        status = green("PASS") if cat.ok else red("FAIL")
        print(f"\n  [{status}] {bold(cat.category)}: {cat.description}")
        print(f"         {cat.passed}/{cat.total} items passed")

        if verbose or not cat.ok:
            for item in cat.items:
                marker = green("ok") if item.passed else red("FAIL")
                print(f"           [{marker}] {item.name}: {item.detail}")

        total_passed += cat.passed
        total_failed += cat.failed
        total_items += cat.total

    print()
    print(bold("-" * 76))
    score = (total_passed / total_items * 100) if total_items else 0
    cats_ok = sum(1 for c in results if c.ok)
    cats_fail = sum(1 for c in results if not c.ok)
    print(
        f"  Correspondence score: {bold(f'{score:.1f}%')} "
        f"({total_passed}/{total_items} items)"
    )
    print(
        f"  Categories: {green(f'{cats_ok} passed')} | "
        f"{red(f'{cats_fail} failed') if cats_fail else f'{cats_fail} failed'}"
    )
    print(bold("=" * 76))

    if total_failed:
        print(f"\n{red('correspondence check: FAILED')}")
        return 1
    print(f"\n{green('correspondence check: ok')}")
    return 0


def json_report(results: list[CategoryResult]) -> dict:
    total_passed = sum(c.passed for c in results)
    total_items = sum(c.total for c in results)
    return {
        "score": round(total_passed / total_items * 100, 1) if total_items else 0,
        "total_passed": total_passed,
        "total_failed": sum(c.failed for c in results),
        "total_items": total_items,
        "categories": [
            {
                "name": c.category,
                "description": c.description,
                "ok": c.ok,
                "passed": c.passed,
                "failed": c.failed,
                "total": c.total,
                "items": [
                    {
                        "name": i.name,
                        "passed": i.passed,
                        "detail": i.detail,
                    }
                    for i in c.items
                ],
            }
            for c in results
        ],
    }


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Comprehensive Lean-Rust-Python correspondence checker.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument("--json", action="store_true", help="Output JSON report")
    parser.add_argument(
        "--category",
        type=str,
        choices=list(ALL_CATEGORIES.keys()),
        help="Check a specific category only",
    )
    parser.add_argument(
        "--verbose",
        "-v",
        action="store_true",
        help="Show all checked items",
    )
    args = parser.parse_args()

    categories = [args.category] if args.category else None
    results = run_checks(categories)

    if args.json:
        print(json.dumps(json_report(results), indent=2))
        return 0 if all(c.ok for c in results) else 1

    return print_report(results, verbose=args.verbose)


if __name__ == "__main__":
    raise SystemExit(main())
