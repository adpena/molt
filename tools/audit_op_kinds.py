#!/usr/bin/env python3
"""Op-kind single-source-of-truth audit (molt task #57, phase 1).

Cross-component "kind string" drift is molt's most prolific silent-miscompile
bug class. The frontend serializes each ``MoltOp`` into a JSON op whose ``kind``
string is the wire contract between the Python frontend and the Rust backend.
Five independent components must agree on that vocabulary, but each keeps its own
copy of the table:

  1. the frontend ``map_ops_to_json`` if/elif chain (the EMITTER — authoritative
     wire vocabulary; ``serialization.py``),
  2. the TIR SSA mapper ``kind_to_opcode`` (string -> ``OpCode``; ``ssa.rs``) —
     any kind it does not recognize is silently lifted to ``OpCode::Copy`` with
     the spelling stashed in ``_original_kind`` (the ``_ => OpCode::Copy`` arm),
  3. the LLVM ``lower_preserved_simpleir_op`` dedicated arms + its generic
     ``molt_<kind>`` by-symbol fallback (``llvm_backend/lowering.rs``),
  4. the RC/alias ``CopyLowering`` classifier ``classify_copy_kind`` /
     ``copy_kind_mints_fresh_owned_ref`` / ``copy_kind_is_explicit_no_heap_move``
     (``alias_analysis.rs``) — whose ``_ => TransparentAlias`` default is the
     UAF-escalation precondition,
  5. the native + WASM SimpleIR dispatch (``function_compiler.rs`` / ``wasm.rs``),
     reached via the ``lower_to_simple`` ``_original_kind`` restoration.

The proven failure: ``serialization.py`` emits ``"floordiv"`` while ``ssa.rs``
recognized only ``"floor_div"`` -> silent lift to ``Copy{_original_kind}``; and
``"matmul"`` had no mapper arm at all. On the LLVM lane those would have become a
copy of operand 0 (``a // b`` -> ``a``) and, under drop insertion, a UAF.

This tool EXTRACTS each component's table directly from source (AST for the
Python emitter; a line-anchored brace/comment-aware Rust ``match`` parser
validated against floordiv/floor_div/matmul) and prints the drift matrix +
dangerous-cell list. It is the machine-generated enumeration that phase 2's
``op_kinds.toml`` single source of truth must mirror.

Usage::

    python3 tools/audit_op_kinds.py                # human report (drift matrix)
    python3 tools/audit_op_kinds.py --json         # machine-readable matrix
    python3 tools/audit_op_kinds.py --check        # CI: exit 1 on NEW danger
    python3 tools/audit_op_kinds.py --write-baseline

THE AUTHORITATIVE LAYER. The ``MoltOp.kind`` vocabulary (~1777 uppercase
``MoltOp(kind=...)`` construction sites in the visitors) is an INTERNAL frontend
detail fully consumed by ``map_ops_to_json``; the audit's source of truth for the
cross-component contract is therefore the JSON ``"kind"`` STRING that
``map_ops_to_json`` emits (lowercase), because that is exactly what every backend
component keys on. Phase 2's table is keyed by the emitted JSON kind.
"""

from __future__ import annotations

import argparse
import ast
import json
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

SERIALIZATION_PY = ROOT / "src/molt/frontend/lowering/serialization.py"
SSA_RS = ROOT / "runtime/molt-backend/src/tir/ssa.rs"
LLVM_RS = ROOT / "runtime/molt-backend/src/llvm_backend/lowering.rs"
ALIAS_RS = ROOT / "runtime/molt-backend/src/tir/passes/alias_analysis.rs"
NATIVE_RS = ROOT / "runtime/molt-backend/src/native_backend/function_compiler.rs"
WASM_RS = ROOT / "runtime/molt-backend/src/wasm.rs"
RUNTIME_SRC = ROOT / "runtime/molt-runtime/src"

BASELINE_PATH = ROOT / "tools/op_kinds_baseline.json"


# ---------------------------------------------------------------------------
# Rust `match` arm extraction
# ---------------------------------------------------------------------------
#
# Method: locate `fn NAME`, find the requested `match X {`, brace-match its body,
# then a char-level state machine walks the body collecting the string literals
# of every TOP-LEVEL arm pattern (the text left of `=>`). It skips `//` and
# `/* */` comments and `"..."` strings, and after each `=>` it skips the arm body
# whether it is a `{ ... }` block (balanced-brace skip) or a comma-terminated
# expression (paren/bracket/brace-balanced skip to the top-level `,`).
#
# Validated below against three known kinds (floordiv / floor_div / matmul) plus
# `index` (a `{}`-block-bodied arm that follows another `{}`-block arm) so the
# block/comma boundary handling is exercised.
#
# Failure modes (each absent in the parsed functions, asserted or documented):
#   * a `=>` INSIDE a pattern string literal -> impossible (kinds are identifiers,
#     never contain `=>`);
#   * raw string literals `r"..."` / `r#"..."#` in a pattern -> none used here
#     (all arms use plain `"..."`); a raw string would mis-skip, so the parser
#     asserts no `r"`/`r#"` precedes a captured literal in the scanned region;
#   * macro-generated arms (e.g. `seq!`/`paste!`) -> none in these functions;
#   * a nested `match` inside an arm body -> handled by the balanced-brace body
#     skip (the inner match's arms are never at the outer top level).


class RustMatchParseError(RuntimeError):
    pass


def _find_fn_start(lines: list[str], fn: str) -> int:
    pat = re.compile(r"\bfn\s+" + re.escape(fn) + r"\b")
    for i, line in enumerate(lines):
        if pat.search(line):
            return i
    raise RustMatchParseError(f"fn {fn} not found")


def _string_literals(text: str) -> list[str]:
    return re.findall(r'"((?:[^"\\]|\\.)*)"', text)


def extract_match_arms(path: Path, fn: str, match_on: str) -> list[str]:
    """Return, in source order (deduped), the string-literal patterns of every
    top-level arm of the `match_on` match inside function `fn` of `path`."""
    lines = path.read_text().splitlines(keepends=True)
    fs = _find_fn_start(lines, fn)
    ms = None
    for i in range(fs, len(lines)):
        if match_on in lines[i]:
            ms = i
            break
    if ms is None:
        raise RustMatchParseError(f"`{match_on}` not found in fn {fn}")

    region = "".join(lines[ms:])
    open_idx = region.index("{")
    depth = 0
    end = None
    for idx in range(open_idx, len(region)):
        ch = region[idx]
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0:
                end = idx
                break
    if end is None:
        raise RustMatchParseError(f"unbalanced match braces in fn {fn}")
    body = region[open_idx + 1 : end]

    # Guard against raw strings inside the scanned region (would defeat the plain
    # "..." scanner). None exist in the parsed functions; assert it stays so. The
    # `r` of a raw-string prefix must NOT be preceded by an identifier char (else
    # we falsely match the closing quote of an identifier-content string such as
    # `"bit_or"`, whose final `r` abuts the closing `"`).
    if re.search(r'(?<![A-Za-z0-9_])r#*"', body):
        raise RustMatchParseError(
            f"raw string literal in match body of fn {fn}; parser unsupported"
        )

    arms: list[str] = []
    i = 0
    n = len(body)
    pat = ""
    in_pattern = True

    def skip_balanced(i: int) -> int:
        d = 0
        while i < n:
            c = body[i]
            two = body[i : i + 2]
            if two == "//":
                j = body.find("\n", i)
                i = j if j != -1 else n
                continue
            if two == "/*":
                j = body.find("*/", i)
                i = j + 2 if j != -1 else n
                continue
            if c == '"':
                i += 1
                while i < n and body[i] != '"':
                    if body[i] == "\\":
                        i += 1
                    i += 1
                i += 1
                continue
            if c == "{":
                d += 1
                i += 1
                continue
            if c == "}":
                d -= 1
                i += 1
                if d == 0:
                    return i
                continue
            i += 1
        return i

    def skip_expr(i: int) -> int:
        bd = 0
        while i < n:
            c = body[i]
            two = body[i : i + 2]
            if two == "//":
                j = body.find("\n", i)
                i = j if j != -1 else n
                continue
            if two == "/*":
                j = body.find("*/", i)
                i = j + 2 if j != -1 else n
                continue
            if c == '"':
                i += 1
                while i < n and body[i] != '"':
                    if body[i] == "\\":
                        i += 1
                    i += 1
                i += 1
                continue
            if c in "([{":
                bd += 1
                i += 1
                continue
            if c in ")]}":
                bd -= 1
                i += 1
                continue
            if c == "," and bd == 0:
                return i + 1
            i += 1
        return i

    while i < n:
        c = body[i]
        two = body[i : i + 2]
        if in_pattern:
            if two == "//":
                j = body.find("\n", i)
                i = j if j != -1 else n
                continue
            if two == "/*":
                j = body.find("*/", i)
                i = j + 2 if j != -1 else n
                continue
            if two == "=>":
                arms.extend(_string_literals(pat))
                pat = ""
                in_pattern = False
                i += 2
                while i < n and body[i] in " \t\r\n":
                    i += 1
                if i < n and body[i] == "{":
                    i = skip_balanced(i)
                    while i < n and body[i] in " \t\r\n":
                        i += 1
                    if i < n and body[i] == ",":
                        i += 1
                else:
                    i = skip_expr(i)
                in_pattern = True
                pat = ""
                continue
            pat += c
            i += 1
            continue

    return list(dict.fromkeys(arms))


def extract_matches_macro(path: Path, fn: str) -> list[str]:
    """Return string literals of the first `matches!(...)` in function `fn`."""
    src = path.read_text()
    m = re.search(r"\bfn\s+" + re.escape(fn) + r"\b", src)
    if m is None:
        raise RustMatchParseError(f"fn {fn} not found")
    mm = re.search(r"matches!\s*\(", src[m.start():])
    if mm is None:
        raise RustMatchParseError(f"matches!() not found in fn {fn}")
    start = m.start() + mm.end()
    depth = 1
    i = start
    while i < len(src) and depth > 0:
        c = src[i]
        if c == "(":
            depth += 1
        elif c == ")":
            depth -= 1
        i += 1
    block = src[start : i - 1]
    return list(dict.fromkeys(_string_literals(block)))


def extract_prefix_rules(path: Path, fn: str) -> list[str]:
    """Return `kind.starts_with("PREFIX")` prefixes used in function `fn`."""
    src = path.read_text()
    m = re.search(r"\bfn\s+" + re.escape(fn) + r"\b", src)
    if m is None:
        return []
    # scope to the function body (balance from its opening brace)
    oi = src.index("{", m.end())
    depth = 0
    end = None
    for idx in range(oi, len(src)):
        if src[idx] == "{":
            depth += 1
        elif src[idx] == "}":
            depth -= 1
            if depth == 0:
                end = idx
                break
    body = src[oi : end if end else len(src)]
    return re.findall(r'\.starts_with\(\s*"([^"]+)"\s*\)', body)


# ---------------------------------------------------------------------------
# Frontend (Python) JSON-kind extraction via AST
# ---------------------------------------------------------------------------


def _attach_parents(tree: ast.AST) -> None:
    for node in ast.walk(tree):
        for child in ast.iter_child_nodes(node):
            child._parent = node  # type: ignore[attr-defined]


def _kinds_from_test(test: ast.expr) -> set[str] | None:
    """`op.kind == "X"` -> {"X"}; `op.kind in (...)` -> {literals}."""
    if isinstance(test, ast.Compare) and len(test.ops) == 1:
        left = test.left
        if isinstance(left, ast.Attribute) and left.attr == "kind":
            op = test.ops[0]
            comp = test.comparators[0]
            if isinstance(op, ast.Eq) and isinstance(comp, ast.Constant):
                return {comp.value}
            if isinstance(op, ast.In) and isinstance(
                comp, (ast.Tuple, ast.List, ast.Set)
            ):
                return {
                    e.value
                    for e in comp.elts
                    if isinstance(e, ast.Constant) and isinstance(e.value, str)
                }
    return None


def _enclosing_kind_guard(node: ast.AST) -> set[str] | None:
    cur = node
    while getattr(cur, "_parent", None) is not None:
        par = cur._parent  # type: ignore[attr-defined]
        if isinstance(par, ast.If):
            kinds = _kinds_from_test(par.test)
            if kinds is not None:
                return kinds
        cur = par
    return None


def _enclosing_function(node: ast.AST) -> ast.AST | None:
    cur = node
    while getattr(cur, "_parent", None) is not None:
        par = cur._parent  # type: ignore[attr-defined]
        if isinstance(par, (ast.FunctionDef, ast.AsyncFunctionDef)):
            return par
        cur = par
    return None


def _resolve_name_assignment(
    func: ast.AST, name: str, guard_kinds: set[str] | None
) -> set[str] | None:
    """Resolve a local `name = <expr>` inside `func` to the kind string(s) it can
    take, for the two structural transforms used by the emitter:

      * `name = op.kind.lower()`            -> {k.lower() for k in guard_kinds}
      * `name = {"A": "a", ...}[op.kind]`   -> the dict's string values
    """
    for sub in ast.walk(func):
        if isinstance(sub, ast.Assign):
            if any(
                isinstance(t, ast.Name) and t.id == name for t in sub.targets
            ):
                val = sub.value
                # op.kind.lower()
                if (
                    isinstance(val, ast.Call)
                    and isinstance(val.func, ast.Attribute)
                    and val.func.attr == "lower"
                    and isinstance(val.func.value, ast.Attribute)
                    and val.func.value.attr == "kind"
                ):
                    if guard_kinds is None:
                        return None
                    return {k.lower() for k in guard_kinds}
                # {DICT}[op.kind]
                if isinstance(val, ast.Subscript) and isinstance(
                    val.value, ast.Dict
                ):
                    return {
                        v.value
                        for v in val.value.values
                        if isinstance(v, ast.Constant) and isinstance(v.value, str)
                    }
    return None


@dataclass
class FrontendKinds:
    constant: set[str] = field(default_factory=set)
    computed: dict[int, set[str]] = field(default_factory=dict)  # line -> kinds
    unresolved: list[tuple[int, str]] = field(default_factory=list)

    @property
    def all(self) -> set[str]:
        out = set(self.constant)
        for ks in self.computed.values():
            out |= ks
        return out


def extract_frontend_kinds() -> FrontendKinds:
    src = SERIALIZATION_PY.read_text()
    tree = ast.parse(src)
    _attach_parents(tree)
    fk = FrontendKinds()
    for node in ast.walk(tree):
        if not isinstance(node, ast.Dict):
            continue
        for k, v in zip(node.keys, node.values):
            if not (isinstance(k, ast.Constant) and k.value == "kind"):
                continue
            if isinstance(v, ast.Constant) and isinstance(v.value, str):
                fk.constant.add(v.value)
                continue
            ln = getattr(v, "lineno", -1)
            guard = _enclosing_kind_guard(node)
            func = _enclosing_function(node)
            resolved: set[str] | None = None
            if isinstance(v, ast.Attribute) and v.attr == "kind":
                # bare `op.kind` under a guard with (lowercase) literals
                resolved = set(guard) if guard else None
            elif isinstance(v, ast.Name) and func is not None:
                resolved = _resolve_name_assignment(func, v.id, guard)
            if resolved:
                fk.computed[ln] = resolved
            else:
                fk.unresolved.append((ln, ast.dump(v)[:60]))
    return fk


# ---------------------------------------------------------------------------
# Runtime `molt_<kind>` export surface (the LLVM by-symbol fallback rule)
# ---------------------------------------------------------------------------


def extract_runtime_molt_symbols() -> set[str]:
    """All `pub extern "C" fn molt_*` exports in molt-runtime. This is the surface
    the LLVM generic fallback (`try_lower_preserved_runtime_call`) probes: a
    preserved `Copy{_original_kind=k}` with no dedicated arm is lowered as
    `molt_<k>(boxed operands...)` iff `molt_<k>` is a linked runtime symbol."""
    syms: set[str] = set()
    pat = re.compile(r'pub\s+extern\s+"C"\s+fn\s+(molt_[A-Za-z0-9_]+)')
    for p in RUNTIME_SRC.rglob("*.rs"):
        try:
            text = p.read_text()
        except OSError:
            continue
        for m in pat.finditer(text):
            syms.add(m.group(1))
    return syms


# ---------------------------------------------------------------------------
# Native / WASM SimpleIR-dispatch arm presence
# ---------------------------------------------------------------------------
#
# Native (function_compiler.rs) and WASM (wasm.rs) consume SimpleIR DIRECTLY. A
# preserved `Copy{_original_kind=k}` is restored to a SimpleIR op `kind=k` by
# `lower_to_simple` (the `_original_kind` passthrough), then dispatched by these
# backends. So the relevant per-backend coverage question is "does the backend's
# SimpleIR dispatch contain a `"k" =>` arm?". These dispatches are sprawling
# multi-thousand-line match-on-string functions (and there are several such
# matches per file — `op.kind`, container-specialization, etc.), so rather than
# locate and parse each giant match we scan the whole file for arm-shaped tokens:
# a run of `"lit"` alternatives joined by `|` and terminated by `=>`. EVERY
# literal in the OR-chain is captured (so `"inc_ref" | "borrow" =>` yields both).
#
# CAVEAT (advisory column). This is a TEXTUAL heuristic, not a parse of the
# dispatch's control flow: it can OVER-count (a `"k" =>` match arm in an unrelated
# helper) and, for arms whose pattern spans constructs other than a bare `|`-chain
# of string literals (e.g. guards `"k" if cond =>`, or a binding `Foo("k") =>`),
# it can UNDER-count. The `native_arm` / `wasm_arm` columns are therefore ADVISORY
# — they corroborate the authoritative LLVM/mapper/classifier columns and flag
# kinds for scrutiny; a disposition is never decided on them alone.


def extract_simpleir_arm_kinds(path: Path) -> set[str]:
    text = path.read_text()
    out: set[str] = set()
    # Match an arm pattern: one-or-more `"lit"` separated by `|`, then `=>`.
    arm = re.compile(
        r'("[a-z][a-z0-9_]*"(?:\s*\|\s*"[a-z][a-z0-9_]*")*)\s*(?:if\b[^=]*?)?=>'
    )
    for m in arm.finditer(text):
        out.update(re.findall(r'"([a-z][a-z0-9_]*)"', m.group(1)))
    return out


# ---------------------------------------------------------------------------
# Matrix assembly
# ---------------------------------------------------------------------------

# Kinds that are NOT cross-component op kinds in the `kind_to_opcode` sense — the
# CFG/SSA layer consumes them STRUCTURALLY (block leaders, terminators, branch
# conditions) rather than dispatching them through `kind_to_opcode`. They
# legitimately have no mapper arm and are excluded from the "emitted-but-unmapped"
# danger categories.
#
# DERIVED FROM SOURCE (drift-proof): the union of the five authoritative Rust
# classifiers `is_structural` (tir/mod.rs), `is_terminator`, `is_block_leader`,
# `is_block_ender`, `is_conditional_branch` (tir/cfg.rs). A new structural kind
# added to those functions automatically leaves this audit's "unmapped" alarm,
# and a new EMITTED kind that is NOT in those functions and NOT in `kind_to_opcode`
# is flagged — exactly the drift contract. (`phi` is the SSA block-argument op the
# converter materializes internally; it is added explicitly because it is
# consumed by the SSA builder, not `kind_to_opcode`, but is not in the CFG
# leader/terminator helpers.)
_STRUCTURAL_CLASSIFIER_FNS = (
    (Path("runtime/molt-backend/src/tir/mod.rs"), "is_structural"),
    (Path("runtime/molt-backend/src/tir/cfg.rs"), "is_terminator"),
    (Path("runtime/molt-backend/src/tir/cfg.rs"), "is_block_leader"),
    (Path("runtime/molt-backend/src/tir/cfg.rs"), "is_block_ender"),
    (Path("runtime/molt-backend/src/tir/cfg.rs"), "is_conditional_branch"),
)
_EXTRA_STRUCTURAL = {"phi"}


def derive_structural_kinds() -> set[str]:
    out: set[str] = set(_EXTRA_STRUCTURAL)
    for rel, fn in _STRUCTURAL_CLASSIFIER_FNS:
        out |= set(extract_matches_macro(ROOT / rel, fn))
    return out


def extract_vec_reduction_ops() -> set[str]:
    """The LLVM `VEC_REDUCTION_OPS` exact table (kind, arity). The vec-* family is
    lowered on LLVM by `vec_reduction_runtime_symbol(kind)` BEFORE the dedicated
    `match`, so membership here is real LLVM coverage the arm-extractor misses."""
    src = LLVM_RS.read_text()
    m = re.search(r"VEC_REDUCTION_OPS\s*:\s*&\[\(&str, usize\)\]\s*=\s*&\[", src)
    if m is None:
        return set()
    start = m.end()
    depth = 1
    i = start
    while i < len(src) and depth > 0:
        c = src[i]
        if c == "[":
            depth += 1
        elif c == "]":
            depth -= 1
        i += 1
    block = src[start : i - 1]
    return set(re.findall(r'\(\s*"([a-z0-9_]+)"\s*,', block))


@dataclass
class KindRow:
    kind: str
    frontend_emits: bool
    mapper_maps: bool
    llvm_dedicated_arm: bool
    llvm_vec_table: bool  # in VEC_REDUCTION_OPS (lowered before the match)
    llvm_symbol_exists: bool  # molt_<kind> is a runtime export (generic fallback)
    classifier_class: str  # FreshValue / TransparentAlias / InertMarker
    native_arm: bool
    wasm_arm: bool
    structural: bool

    @property
    def llvm_covered(self) -> bool:
        """A `Copy`-carried kind is soundly lowered on the LLVM lane iff it has a
        dedicated arm, is in the vec table, or `molt_<kind>` exists for the generic
        by-symbol fallback. Otherwise the LLVM `Copy` arm FAILS LOUD at build."""
        return (
            self.llvm_dedicated_arm
            or self.llvm_vec_table
            or self.llvm_symbol_exists
        )

    def as_dict(self) -> dict:
        return {
            "kind": self.kind,
            "frontend_emits": self.frontend_emits,
            "mapper_maps": self.mapper_maps,
            "llvm_dedicated_arm": self.llvm_dedicated_arm,
            "llvm_vec_table": self.llvm_vec_table,
            "llvm_symbol_exists": self.llvm_symbol_exists,
            "llvm_covered": self.llvm_covered,
            "classifier_class": self.classifier_class,
            "native_arm": self.native_arm,
            "wasm_arm": self.wasm_arm,
            "structural": self.structural,
        }


@dataclass
class AuditResult:
    rows: dict[str, KindRow]
    frontend: FrontendKinds
    mapper_kinds: set[str]
    llvm_arms: set[str]
    llvm_vec_table: set[str]
    fresh_value: set[str]
    fresh_value_prefixes: list[str]
    inert_marker: set[str]
    no_heap_move: set[str]
    runtime_symbols: set[str]
    structural_kinds: set[str]

    def dangerous(self) -> dict[str, list[str]]:
        """Categorize dangerous cells by the PRECISE bug preconditions.

        NB: a kind being merely "emitted but unmapped" is BY DESIGN — the
        architecture deliberately lifts most value/effect ops to
        `Copy{_original_kind}` and restores them via `lower_to_simple` (native /
        WASM) or lowers them via the `molt_<kind>` fallback / dedicated arm
        (LLVM). The dangerous cells are the ones where that safety net has a HOLE.
        """
        cats: dict[str, list[str]] = {
            # D1 — LLVM-coverage gap (the floordiv-class precondition). Emitted,
            # not structural, NOT mapped to a first-class opcode, and NOT covered
            # on the LLVM lane (no dedicated arm, not in the vec table, no
            # `molt_<kind>` symbol). On LLVM this hits the `Copy` fail-loud guard
            # = a HARD BUILD ERROR for any program that reaches the op. (Loud, not
            # silent — but still a real compile gap for that op on LLVM.)
            "llvm_coverage_gap": [],
            # D2 — UAF precondition (the worst class). Classified `FreshValue`
            # (the drop pass emits an independent DecRef on its result) but NOT
            # covered on the LLVM lane. If it reached LLVM codegen it would be a
            # silent operand-0 passthrough AND a drop-insertion double-free. The
            # LLVM fatal gate (`copy_kind_reaches_no_incref_passthrough`) is
            # designed to make this set EMPTY; a non-empty result is classifier <->
            # backend drift.
            "freshvalue_llvm_gap": [],
            # D3 — silent-alias precondition (the alias_analysis.rs `_ =>`
            # fallthrough = the UAF-escalation root). Emitted, not structural,
            # unmapped, AND the classifier did NOT place it in an EXPLICIT class
            # (it fell through to the `_ => TransparentAlias` default), yet it is a
            # value/heap producer (heuristic: a `molt_<kind>` runtime symbol
            # exists, i.e. it is a real runtime op, not a pure SSA move). Such a
            # kind is unioned-by-default into operand 0's alias root; if it ever
            # mints a fresh ref the drop pass leaks it (today) and a future
            # promotion to FreshValue without a backend arm escalates to UAF.
            "classifier_silent_fallthrough": [],
            # D4 — no SimpleIR-lane lowering. Emitted, not structural, unmapped,
            # AND neither native nor WASM has a dispatch arm AND no `molt_<kind>`
            # symbol. Nothing can lower it on the native/WASM lanes (subject to
            # the arm-detector's over-counting caveat — see extract_simpleir_arm_kinds).
            "simpleir_lane_gap": [],
            # D5 — dead mapper vocabulary. A first-class opcode mapping the
            # frontend never emits (a STALE-BASE smell: the spelling the emitter
            # uses may have diverged, à la floor_div).
            "mapped_never_emitted": [],
            # D6 — dead FreshValue allow-list entry the frontend never emits.
            "freshvalue_never_emitted": [],
        }
        for kind, row in self.rows.items():
            if row.structural:
                # Structural kinds are CFG/SSA-consumed; a `mapped_never_emitted`
                # or coverage check does not apply.
                continue
            emitted_unmapped = row.frontend_emits and not row.mapper_maps
            explicit_classified = (
                kind in self.fresh_value
                or any(kind.startswith(p) for p in self.fresh_value_prefixes)
                or kind in self.inert_marker
                or kind in self.no_heap_move
            )
            if emitted_unmapped and not row.llvm_covered:
                cats["llvm_coverage_gap"].append(kind)
            if row.classifier_class == "FreshValue" and not row.llvm_covered:
                cats["freshvalue_llvm_gap"].append(kind)
            if (
                emitted_unmapped
                and not explicit_classified
                and row.llvm_symbol_exists
            ):
                cats["classifier_silent_fallthrough"].append(kind)
            if (
                emitted_unmapped
                and not row.native_arm
                and not row.wasm_arm
                and not row.llvm_symbol_exists
            ):
                cats["simpleir_lane_gap"].append(kind)
            if row.mapper_maps and not row.frontend_emits:
                cats["mapped_never_emitted"].append(kind)
            if row.classifier_class == "FreshValue" and not row.frontend_emits:
                cats["freshvalue_never_emitted"].append(kind)
        return {k: sorted(v) for k, v in cats.items()}


def classify(
    kind: str,
    fresh_value: set[str],
    fresh_prefixes: list[str],
    inert: set[str],
    no_heap_move: set[str],
) -> str:
    if kind in fresh_value or any(kind.startswith(p) for p in fresh_prefixes):
        return "FreshValue"
    if kind in inert:
        return "InertMarker"
    if kind in no_heap_move:
        return "TransparentAlias"
    # The classifier's `_ =>` default. Every kind reaching here is treated as a
    # transparent alias of operand 0 by `classify_copy_kind`.
    return "TransparentAlias"


def run_audit() -> AuditResult:
    fk = extract_frontend_kinds()
    mapper = set(extract_match_arms(SSA_RS, "kind_to_opcode", "match kind {"))
    llvm_arms = set(
        extract_match_arms(LLVM_RS, "lower_preserved_simpleir_op", "match kind {")
    )
    llvm_vec = extract_vec_reduction_ops()
    fresh = set(extract_matches_macro(ALIAS_RS, "copy_kind_mints_fresh_owned_ref"))
    fresh_prefixes = extract_prefix_rules(ALIAS_RS, "copy_kind_mints_fresh_owned_ref")
    inert = set(extract_match_arms(ALIAS_RS, "classify_copy_kind", "match k {"))
    no_heap = set(
        extract_matches_macro(ALIAS_RS, "copy_kind_is_explicit_no_heap_move")
    )
    runtime_syms = extract_runtime_molt_symbols()
    native_arms = extract_simpleir_arm_kinds(NATIVE_RS)
    wasm_arms = extract_simpleir_arm_kinds(WASM_RS)
    structural = derive_structural_kinds()

    universe = fk.all | mapper | llvm_arms | llvm_vec | fresh | inert | no_heap

    rows: dict[str, KindRow] = {}
    for kind in sorted(universe):
        rows[kind] = KindRow(
            kind=kind,
            frontend_emits=kind in fk.all,
            mapper_maps=kind in mapper,
            llvm_dedicated_arm=kind in llvm_arms,
            llvm_vec_table=kind in llvm_vec,
            llvm_symbol_exists=f"molt_{kind}" in runtime_syms,
            classifier_class=classify(kind, fresh, fresh_prefixes, inert, no_heap),
            native_arm=kind in native_arms,
            wasm_arm=kind in wasm_arms,
            structural=kind in structural,
        )

    return AuditResult(
        rows=rows,
        frontend=fk,
        mapper_kinds=mapper,
        llvm_arms=llvm_arms,
        llvm_vec_table=llvm_vec,
        fresh_value=fresh,
        fresh_value_prefixes=fresh_prefixes,
        inert_marker=inert,
        no_heap_move=no_heap,
        runtime_symbols=runtime_syms,
        structural_kinds=structural,
    )


# ---------------------------------------------------------------------------
# Self-validation: the parser must agree with known ground truth
# ---------------------------------------------------------------------------


def self_validate(res: AuditResult) -> list[str]:
    """Assert the extraction matches hand-verified ground truth (the floordiv /
    floor_div / matmul triple plus a few structural anchors). Returns failures."""
    fails: list[str] = []

    def check(cond: bool, msg: str) -> None:
        if not cond:
            fails.append(msg)

    # The proven drift: frontend emits floordiv; mapper has floor_div, not floordiv.
    check("floordiv" in res.frontend.all, "frontend must emit 'floordiv'")
    check("floordiv" not in res.mapper_kinds, "mapper must NOT map 'floordiv'")
    check("floor_div" in res.mapper_kinds, "mapper must map 'floor_div'")
    check(
        "floor_div" not in res.frontend.all,
        "frontend must NOT emit 'floor_div' (dead mapper vocab)",
    )
    # matmul: emitted, unmapped, but LLVM covers via molt_matmul symbol.
    check("matmul" in res.frontend.all, "frontend must emit 'matmul'")
    check("matmul" not in res.mapper_kinds, "mapper must NOT map 'matmul'")
    check(
        "molt_matmul" in res.runtime_symbols,
        "runtime must export molt_matmul (LLVM fallback)",
    )
    # floordiv has an explicit LLVM dedicated arm (the landed fix).
    check(
        "floordiv" in res.llvm_arms,
        "LLVM must have a dedicated 'floordiv' arm",
    )
    # Anchor a few mapper arms and the structural extraction.
    for k in ("add", "copy", "index", "module_import_from", "get_iter"):
        check(k in res.mapper_kinds, f"mapper must map '{k}'")
    # Classifier anchors.
    check(
        res.rows["slice"].classifier_class == "FreshValue",
        "'slice' must classify FreshValue",
    )
    check(
        res.rows.get("guard_int") is not None
        and res.rows["guard_int"].classifier_class == "InertMarker",
        "'guard_int' must classify InertMarker",
    )
    return fails


# ---------------------------------------------------------------------------
# Reporting
# ---------------------------------------------------------------------------


def _b(v: bool) -> str:
    return "Y" if v else "."


def print_report(res: AuditResult) -> None:
    fk = res.frontend
    print("=" * 100)
    print("OP-KIND DRIFT AUDIT (molt task #57, phase 1)")
    print("=" * 100)
    print()
    print("SOURCE TABLE SIZES")
    print(f"  frontend emitted kinds (JSON wire vocab) : {len(fk.all)}")
    print(f"    constant literals                      : {len(fk.constant)}")
    print(f"    computed (resolved) sites              : {len(fk.computed)}")
    if fk.unresolved:
        print(f"    UNRESOLVED computed sites              : {len(fk.unresolved)}")
        for ln, dump in fk.unresolved:
            print(f"      line {ln}: {dump}")
    print(f"  ssa.rs kind_to_opcode arms               : {len(res.mapper_kinds)}")
    print(f"  llvm dedicated arms                      : {len(res.llvm_arms)}")
    print(f"  llvm VEC_REDUCTION_OPS table              : {len(res.llvm_vec_table)}")
    print(f"  classifier FreshValue allow-list         : {len(res.fresh_value)}")
    print(f"    + prefix rules                         : {res.fresh_value_prefixes}")
    print(f"  classifier InertMarker arms              : {len(res.inert_marker)}")
    print(f"  classifier no-heap-move (alias) set      : {len(res.no_heap_move)}")
    print(f"  structural (CFG/SSA-consumed) kinds      : {len(res.structural_kinds)}")
    print(f"  runtime molt_* exports (symbol surface)  : {len(res.runtime_symbols)}")
    print()

    dangerous = res.dangerous()
    print("DANGEROUS-CELL SUMMARY (counts)")
    for cat, items in dangerous.items():
        print(f"  {cat:34s} : {len(items)}")
    print()

    for cat, items in dangerous.items():
        if not items:
            continue
        print(f"-- {cat} ({len(items)}) --")
        for k in items:
            row = res.rows[k]
            print(
                f"   {k:32s} mapper={_b(row.mapper_maps)} "
                f"llvm_arm={_b(row.llvm_dedicated_arm)} "
                f"llvm_vec={_b(row.llvm_vec_table)} "
                f"llvm_sym={_b(row.llvm_symbol_exists)} "
                f"class={row.classifier_class:16s} "
                f"native={_b(row.native_arm)} wasm={_b(row.wasm_arm)}"
            )
        print()

    print("FULL DRIFT MATRIX  (fe=frontend-emits map=mapper-arm la=llvm-arm "
          "lv=llvm-vec ls=llvm-sym st=structural)")
    hdr = (
        f"{'kind':34s} fe map  la lv ls {'classifier':16s} nat wasm st"
    )
    print(hdr)
    print("-" * len(hdr))
    for kind, row in res.rows.items():
        print(
            f"{kind:34s} {_b(row.frontend_emits)}   {_b(row.mapper_maps)}   "
            f"{_b(row.llvm_dedicated_arm)}  {_b(row.llvm_vec_table)}  "
            f"{_b(row.llvm_symbol_exists)}  "
            f"{row.classifier_class:16s} {_b(row.native_arm)}   {_b(row.wasm_arm)}   "
            f"{_b(row.structural)}"
        )


def to_baseline(res: AuditResult) -> dict:
    """The committed baseline = the dangerous-cell sets. CI fails on NEW members."""
    return {"dangerous": res.dangerous()}


def check_against_baseline(res: AuditResult) -> int:
    if not BASELINE_PATH.exists():
        print(
            f"ERROR: baseline {BASELINE_PATH} missing; run --write-baseline first",
            file=sys.stderr,
        )
        return 2
    baseline = json.loads(BASELINE_PATH.read_text())
    base = baseline.get("dangerous", {})
    current = res.dangerous()
    rc = 0
    for cat, items in current.items():
        new = sorted(set(items) - set(base.get(cat, [])))
        if new:
            rc = 1
            print(
                f"NEW dangerous-cell in '{cat}': {new}",
                file=sys.stderr,
            )
    if rc == 0:
        print("op-kind drift check: OK (no new dangerous cells vs baseline)")
    else:
        print(
            "\nA new op kind drifted across the frontend/backend boundary. "
            "Add a mapper arm in ssa.rs kind_to_opcode (or, for a CFG/SSA-consumed "
            "control kind, add it to is_structural/the cfg.rs leader/terminator "
            "helpers), classify it in alias_analysis.rs, ensure LLVM coverage "
            "(dedicated arm or molt_<kind> symbol), and refresh the baseline once "
            "the fix lands.",
            file=sys.stderr,
        )
    return rc


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--json", action="store_true", help="emit the matrix as JSON")
    ap.add_argument(
        "--check",
        action="store_true",
        help="CI mode: exit 1 if new dangerous cells appear vs the baseline",
    )
    ap.add_argument(
        "--write-baseline",
        action="store_true",
        help="(re)write the committed dangerous-cell baseline",
    )
    ap.add_argument(
        "--no-validate",
        action="store_true",
        help="skip the parser self-validation (debug only)",
    )
    args = ap.parse_args(argv)

    res = run_audit()

    if not args.no_validate:
        fails = self_validate(res)
        if fails:
            print("PARSER SELF-VALIDATION FAILED:", file=sys.stderr)
            for f in fails:
                print(f"  - {f}", file=sys.stderr)
            return 3

    if res.frontend.unresolved:
        # An unresolved computed kind means the extractor cannot prove the wire
        # vocabulary; that is itself a drift hazard. Fail loud.
        print(
            "ERROR: unresolved computed kind emission sites "
            f"({len(res.frontend.unresolved)}); extend the resolver",
            file=sys.stderr,
        )
        for ln, dump in res.frontend.unresolved:
            print(f"  line {ln}: {dump}", file=sys.stderr)
        return 3

    if args.write_baseline:
        BASELINE_PATH.write_text(json.dumps(to_baseline(res), indent=2) + "\n")
        print(f"wrote baseline -> {BASELINE_PATH}")
        return 0

    if args.check:
        return check_against_baseline(res)

    if args.json:
        out = {
            "rows": [r.as_dict() for r in res.rows.values()],
            "dangerous": res.dangerous(),
        }
        print(json.dumps(out, indent=2))
        return 0

    print_report(res)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
