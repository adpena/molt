#!/usr/bin/env python3
"""Op-kind single-source-of-truth audit (molt task #57, phase 1).

Cross-component "kind string" drift is molt's most prolific silent-miscompile
bug class. The frontend serializes each ``MoltOp`` into a JSON op whose ``kind``
string is the wire contract between the Python frontend and the Rust backend.
Five independent components must agree on that vocabulary, but each keeps its own
copy of the table:

  1. the frontend ``map_ops_to_json`` dispatcher and extracted serialization
     handlers (the EMITTER — authoritative wire vocabulary),
  2. the TIR SSA mapper ``kind_to_opcode`` (string -> ``OpCode``; ``ssa.rs``) —
     any kind it does not recognize is silently lifted to ``OpCode::Copy`` with
     the spelling stashed in ``_original_kind`` (the ``_ => OpCode::Copy`` arm),
  3. the LLVM ``lower_preserved_simpleir_op`` dedicated arms + its ABI-exact
     ``molt_<kind>`` runtime fallback (``llvm_backend/lowering.rs``),
  4. the RC/alias ``CopyLowering`` classifier ``classify_copy_kind`` /
     ``copy_kind_mints_fresh_owned_ref`` / ``copy_kind_mints_owned_alias_ref`` /
     ``copy_kind_is_explicit_no_heap_move``
     (``alias_analysis.rs``) — whose ``_ => TransparentAlias`` default is the
     UAF-escalation precondition,
  5. the native + WASM SimpleIR dispatch (``function_compiler.rs`` / ``wasm.rs``),
     reached via the ``lower_to_simple`` ``_original_kind`` restoration. The native
     dispatch routes purely via each ``fc/*`` handler's ``HANDLED_KINDS`` slice
     (``op_family::native_op_family``); a codegen-reaching kind that the frontend
     emits but that NO slice claims is dead at dispatch. Result-producing gaps
     hit the loud catch-all panic, while no-result gaps silently skip side effects
     or control metadata. The audit therefore enforces native-routing
     COMPLETENESS for every non-structural emitted kind (the
     ``native_codegen_gap`` cell) and separately compares each handler's
     ``match`` arms to its ``HANDLED_KINDS`` slice — closing the ``copy``
     instance, where ``value_transfer.rs`` matched ``"copy"`` but omitted it from
     ``value_transfer::HANDLED_KINDS``.

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
``map_ops_to_json`` and its handler modules emit (lowercase), because that is
exactly what every backend component keys on. Phase 2's table is keyed by the
emitted JSON kind.
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
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from tools.op_kinds.paths import TABLE as OP_KINDS_TOML  # noqa: E402
from tools.op_kinds.paths import TIR_SRC as OP_KIND_TIR_SRC  # noqa: E402
from tools.op_kinds.paths import tir_path  # noqa: E402

SERIALIZATION_DIR = ROOT / "src/molt/frontend/lowering"
SERIALIZATION_PY = SERIALIZATION_DIR / "serialization.py"
SERIALIZATION_MODULES = (
    SERIALIZATION_PY,
    SERIALIZATION_DIR / "serialization_basic_ops.py",
    SERIALIZATION_DIR / "serialization_collection_ops.py",
    SERIALIZATION_DIR / "serialization_exception_ops.py",
    SERIALIZATION_DIR / "serialization_function_ops.py",
    SERIALIZATION_DIR / "serialization_loop_string_async_ops.py",
    SERIALIZATION_DIR / "serialization_object_attr_ops.py",
)
SSA_RS = OP_KIND_TIR_SRC / "ssa.rs"
LLVM_RS = ROOT / "runtime/molt-backend/src/llvm_backend/lowering.rs"
LLVM_RUNTIME_IMPORTS_RS = ROOT / "runtime/molt-backend/src/llvm_backend/runtime_imports.rs"
LLVM_PRESERVED_OPS_RS = (
    ROOT / "runtime/molt-backend/src/llvm_backend/lowering/preserved_ops.rs"
)
LLVM_PRESERVED_OPS_DIR = LLVM_PRESERVED_OPS_RS.with_suffix("")
LLVM_VEC_REDUCTIONS_RS = LLVM_PRESERVED_OPS_DIR / "vector_reductions.rs"
ALIAS_RS = tir_path("passes/alias_analysis.rs")
NATIVE_RS = ROOT / "runtime/molt-backend/src/native_backend/function_compiler.rs"
NATIVE_FC_DIR = ROOT / "runtime/molt-backend/src/native_backend/function_compiler/fc"
# The SSA -> SimpleIR lowering. `fn lower_op` is the per-OpCode authority for
# whether a kind's lowered SimpleIR op carries a result (`out: Some`) or is a
# no-result statement op (`out` absent / `None`). This remains a report fact, but
# no-result ops are no longer a native-routing exemption: they may carry side
# effects or control metadata and must be claimed by a handler slice.
LOWER_TO_SIMPLE_RS = tir_path("lower_to_simple.rs")
LOWER_TO_SIMPLE_OP_LOWERING_RS = tir_path("lower_to_simple/op_lowering.rs")
WASM_RS = ROOT / "runtime/molt-backend/src/wasm.rs"
RUNTIME_SRC_ROOTS = tuple(
    sorted(
        path / "src"
        for path in (ROOT / "runtime").glob("molt-runtime*")
        if (path / "src").is_dir()
    )
)

# The op-kind single-source-of-truth registry (task #57, phase 2). Since phase 2
# landed, the backend's mapper and CopyLowering-classifier vocabularies are
# GENERATED from this table (op_kinds_generated.rs delegated to by
# ssa::kind_to_opcode / alias_analysis); the hand-written Rust functions no longer
# carry the inline `match`/`matches!` arms the original audit parsed. The audit
# therefore sources the BACKEND-side tables from this registry (the source of
# truth) and keeps AST-extracting the FRONTEND emitter — so the drift matrix is
# exactly "does the frontend emit a kind the registry does not cover?". The
# registry⇄generated-Rust direction is pinned separately by
# tests/test_gen_op_kinds.py; the registry⇄enum exhaustiveness by the Rust
# compiler. (LLVM arms, runtime symbols, and native/WASM SimpleIR dispatch are
# still extracted directly from source — they are not generated.)
BASELINE_PATH = ROOT / "tools/op_kinds_baseline.json"


def read_rust_module_cluster(root_file: Path) -> str:
    """Read a Rust module root plus its extracted sibling module tree."""
    parts: list[str] = []
    module_dir = root_file.with_suffix("")
    if module_dir.is_dir():
        for child in sorted(module_dir.rglob("*.rs")):
            if "tests" in child.relative_to(module_dir).parts:
                continue
            parts.append(child.read_text(encoding="utf-8"))
    parts.append(root_file.read_text(encoding="utf-8"))
    return "\n".join(parts)


def _load_op_kinds_toml() -> dict:
    """Parse the op-kind registry. Fail loud if absent — the audit's backend-side
    vocabulary depends on it post phase-2."""
    try:
        import tomllib  # Python 3.11+
    except ModuleNotFoundError:  # pragma: no cover
        import tomli as tomllib  # type: ignore[no-redef]
    if not OP_KINDS_TOML.exists():
        raise RustMatchParseError(f"op-kind registry missing: {OP_KINDS_TOML}")
    return tomllib.loads(OP_KINDS_TOML.read_text(encoding="utf-8"))


def mapper_kinds_from_registry(data: dict) -> set[str]:
    """The kind_to_opcode mapper vocabulary = every canonical + alias spelling of
    every [[kind]] row in the registry."""
    out: set[str] = set()
    for row in data.get("kind", []):
        out.add(row["canonical"])
        out.update(row.get("aliases", []))
    return out


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
    lines = path.read_text(encoding="utf-8").splitlines(keepends=True)
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
    src = path.read_text(encoding="utf-8")
    m = re.search(r"\bfn\s+" + re.escape(fn) + r"\b", src)
    if m is None:
        raise RustMatchParseError(f"fn {fn} not found")
    mm = re.search(r"matches!\s*\(", src[m.start() :])
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


def extract_llvm_preserved_op_kinds() -> set[str]:
    """LLVM SimpleIR-preserved-op coverage after preserved_ops.rs decomposition.

    The root dispatcher owns direct guards and delegates to role-specific child
    modules. Treat those child match arms as the same LLVM lowering authority;
    otherwise the audit regresses to the old monolithic-file shape and reports
    false coverage gaps after structural extraction.
    """
    kinds = set(
        extract_match_arms(
            LLVM_PRESERVED_OPS_RS, "lower_preserved_simpleir_op", "match kind {"
        )
    )
    kinds.update(re.findall(r'kind\s*==\s*"([^"]+)"', LLVM_PRESERVED_OPS_RS.read_text(encoding="utf-8")))
    delegated = [
        (LLVM_PRESERVED_OPS_DIR / "callable_ops.rs", "lower_preserved_callable_op"),
        (LLVM_PRESERVED_OPS_DIR / "container_ops.rs", "lower_preserved_container_op"),
    ]
    for path, fn in delegated:
        kinds.update(extract_match_arms(path, fn, "match kind {"))
    return kinds


def extract_prefix_rules(path: Path, fn: str) -> list[str]:
    """Return `kind.starts_with("PREFIX")` prefixes used in function `fn`."""
    src = path.read_text(encoding="utf-8")
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
            if any(isinstance(t, ast.Name) and t.id == name for t in sub.targets):
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
                if isinstance(val, ast.Subscript) and isinstance(val.value, ast.Dict):
                    return {
                        v.value
                        for v in val.value.values
                        if isinstance(v, ast.Constant) and isinstance(v.value, str)
                    }
    return None


@dataclass
class FrontendKinds:
    constant: set[str] = field(default_factory=set)
    computed: dict[str, set[str]] = field(default_factory=dict)  # file:line -> kinds
    unresolved: list[tuple[str, int, str]] = field(default_factory=list)

    @property
    def all(self) -> set[str]:
        out = set(self.constant)
        for ks in self.computed.values():
            out |= ks
        return out


def _static_kind_strings(expr: ast.expr) -> set[str] | None:
    """Resolve a fully-static `"kind"` value expression to the set of string
    literals it can evaluate to. Handles the two static emission idioms:
    a bare string constant, and a (possibly nested) conditional expression
    whose branches are themselves static — e.g. the shared binary/inplace
    arms `"div" if op.kind == "DIV" else "inplace_div"`. Returns None for
    anything dynamic (those flow to the guard/assignment resolvers)."""
    if isinstance(expr, ast.Constant) and isinstance(expr.value, str):
        return {expr.value}
    if isinstance(expr, ast.IfExp):
        body = _static_kind_strings(expr.body)
        orelse = _static_kind_strings(expr.orelse)
        if body is not None and orelse is not None:
            return body | orelse
    return None


def extract_frontend_kinds() -> FrontendKinds:
    fk = FrontendKinds()
    for path in SERIALIZATION_MODULES:
        src = path.read_text(encoding="utf-8")
        tree = ast.parse(src, filename=str(path))
        _attach_parents(tree)
        rel_path = path.relative_to(ROOT).as_posix()
        for node in ast.walk(tree):
            if not isinstance(node, ast.Dict):
                continue
            for k, v in zip(node.keys, node.values):
                if not (isinstance(k, ast.Constant) and k.value == "kind"):
                    continue
                static = _static_kind_strings(v)
                if static is not None:
                    fk.constant.update(static)
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
                    fk.computed[f"{rel_path}:{ln}"] = resolved
                else:
                    fk.unresolved.append((rel_path, ln, ast.dump(v)[:60]))
    return fk


# ---------------------------------------------------------------------------
# Runtime `molt_<kind>` ABI surface (the LLVM ABI-exact fallback rule)
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class RuntimeExtern:
    symbol: str
    params: tuple[str, ...]
    return_ty: str
    path: Path


@dataclass(frozen=True)
class ClassifiedRuntimeImport:
    symbol: str
    param_count: int
    return_abi: str


def extract_runtime_type_aliases(src_root: Path) -> dict[str, str]:
    aliases: dict[str, str] = {}
    pat = re.compile(r"\btype\s+([A-Za-z_][A-Za-z0-9_]*)\s*=\s*([^;]+);")
    for p in src_root.rglob("*.rs"):
        try:
            text = p.read_text(encoding="utf-8")
        except OSError:
            continue
        for m in pat.finditer(text):
            aliases[m.group(1)] = m.group(2).strip()
    return aliases


def _runtime_param_types(args_src: str) -> tuple[str, ...]:
    params: list[str] = []
    for raw in args_src.split(","):
        raw = raw.strip()
        if not raw:
            continue
        if ":" not in raw:
            return ()
        params.append(raw.rsplit(":", 1)[1].strip())
    return tuple(params)


def _normalize_runtime_type(ty: str, aliases: dict[str, str]) -> str:
    ty = re.sub(r"\s+", " ", ty.strip())
    seen: set[str] = set()
    while ty in aliases and ty not in seen:
        seen.add(ty)
        ty = re.sub(r"\s+", " ", aliases[ty].strip())
    return ty


def extract_runtime_molt_externs() -> dict[str, RuntimeExtern]:
    """All `pub (unsafe)? extern "C" fn molt_*` exports in runtime leaf crates.

    The LLVM generic fallback may only claim symbols whose ABI is positional
    boxed integers; pointer/string/function-pointer ABIs require dedicated
    lowering arms and must stay red in this audit. Runtime symbols now live in
    leaf crates (`molt-runtime-math`, `molt-runtime-text`, ...), so scanning only
    the root runtime crate would recreate the pre-decomposition monolith.
    """
    out: dict[str, RuntimeExtern] = {}
    pat = re.compile(
        r'pub\s+(?:unsafe\s+)?extern\s+"C"\s+fn\s+'
        r"(molt_[A-Za-z0-9_]+)\s*\((.*?)\)\s*(?:->\s*([^{\n]+))?\s*\{",
        re.S,
    )
    for src_root in RUNTIME_SRC_ROOTS:
        aliases = extract_runtime_type_aliases(src_root)
        for p in src_root.rglob("*.rs"):
            try:
                text = p.read_text(encoding="utf-8")
            except OSError:
                continue
            for m in pat.finditer(text):
                symbol = m.group(1)
                params = tuple(
                    _normalize_runtime_type(t, aliases)
                    for t in _runtime_param_types(m.group(2))
                )
                ret = _normalize_runtime_type((m.group(3) or "()").strip(), aliases)
                out[symbol] = RuntimeExtern(symbol, params, ret, p.relative_to(ROOT))
    return out


def extract_runtime_molt_symbols() -> set[str]:
    return set(extract_runtime_molt_externs())


def extract_llvm_classified_runtime_imports() -> dict[str, ClassifiedRuntimeImport]:
    """Runtime symbols the LLVM generic preserved-op fallback may declare.

    `try_lower_preserved_runtime_call` requires a real runtime symbol *and* an
    entry in `CLASSIFIED_RUNTIME_IMPORTS`; this audit must join both authorities
    or it can bless a runtime export that LLVM would still fail loudly.
    """
    text = LLVM_RUNTIME_IMPORTS_RS.read_text(encoding="utf-8")
    out: dict[str, ClassifiedRuntimeImport] = {}
    for symbol, param_count, return_abi in re.findall(
        r'runtime_sig\(\s*"([^"]+)"\s*,\s*(\d+)\s*,\s*RuntimeReturnAbi::(I64|Void)\s*\)',
        text,
        re.S,
    ):
        out[symbol] = ClassifiedRuntimeImport(symbol, int(param_count), return_abi)
    return out


_BOXED_RUNTIME_TYPES = {"u64", "i64"}


def _is_boxed_runtime_type(ty: str) -> bool:
    return ty in _BOXED_RUNTIME_TYPES


def runtime_extern_has_boxed_params(ext: RuntimeExtern, arity: int) -> bool:
    return len(ext.params) == arity and all(
        _is_boxed_runtime_type(t) for t in ext.params
    )


def runtime_extern_is_boxed_i64_fallback_eligible(ext: RuntimeExtern) -> bool:
    return all(
        _is_boxed_runtime_type(t) for t in ext.params
    ) and _is_boxed_runtime_type(ext.return_ty)


def runtime_extern_classified_fallback_eligible(
    ext: RuntimeExtern, classified: ClassifiedRuntimeImport
) -> bool:
    if len(ext.params) != classified.param_count:
        return False
    if not all(_is_boxed_runtime_type(t) for t in ext.params):
        return False
    if classified.return_abi == "I64":
        return _is_boxed_runtime_type(ext.return_ty)
    if classified.return_abi == "Void":
        return ext.return_ty == "()"
    return False


def runtime_extern_is_boxed_void_fallback_eligible(
    ext: RuntimeExtern, arity: int
) -> bool:
    return ext.return_ty == "()" and runtime_extern_has_boxed_params(ext, arity)


def llvm_void_runtime_abi_mismatches(
    void_runtime_ops: dict[str, tuple[str, int]],
    runtime_externs: dict[str, RuntimeExtern],
) -> list[str]:
    out: list[str] = []
    for kind, (symbol, arity) in sorted(void_runtime_ops.items()):
        ext = runtime_externs.get(symbol)
        if ext is None:
            out.append(f"{kind}:{symbol}:missing-extern")
            continue
        if ext.return_ty != "()":
            out.append(f"{kind}:{symbol}:return={ext.return_ty}")
            continue
        if len(ext.params) != arity:
            out.append(f"{kind}:{symbol}:arity={arity}:extern_params={len(ext.params)}")
            continue
        bad_params = [ty for ty in ext.params if not _is_boxed_runtime_type(ty)]
        if bad_params:
            out.append(f"{kind}:{symbol}:non-boxed-params={','.join(bad_params)}")
    return out


def extract_llvm_void_runtime_ops() -> dict[str, tuple[str, int]]:
    src = LLVM_RS.read_text(encoding="utf-8")
    m = re.search(
        r"PRESERVED_VOID_RUNTIME_OPS\s*:\s*&\[\(&str,\s*&str,\s*usize\)\]\s*=\s*&\[",
        src,
    )
    if m is None:
        return {}
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
    out: dict[str, tuple[str, int]] = {}
    for kind, symbol, arity in re.findall(
        r'\(\s*"([a-z0-9_]+)"\s*,\s*"([A-Za-z0-9_]+)"\s*,\s*(\d+)\s*\)',
        block,
    ):
        out[kind] = (symbol, int(arity))
    return out


# ---------------------------------------------------------------------------
# Native / WASM SimpleIR-dispatch arm presence
# ---------------------------------------------------------------------------
#
# Native (function_compiler.rs) and WASM (wasm.rs) consume SimpleIR DIRECTLY. A
# preserved `Copy{_original_kind=k}` is restored to a SimpleIR op `kind=k` by
# `lower_to_simple` (the `_original_kind` passthrough), then dispatched by these
# backends. So the relevant per-backend coverage question is "does the backend's
# SimpleIR dispatch contain a `"k" =>` arm or generated/extracted routing fact?".
#
# Native has moved away from one monolithic dispatch match: extracted
# `function_compiler/fc/*` handlers declare `HANDLED_KINDS` slices, and
# `fc/op_family.rs` derives the dispatch from those single authorities. The
# native audit therefore consumes those slices directly, plus the inline dispatch
# slices in `op_family.rs`, and keeps the old monolithic arm scan as a
# compatibility/advisory backstop while the decomposition is in flight.
#
# WASM remains a sprawling multi-thousand-line match-on-string file (and there
# are several such matches per file — `op.kind`, container-specialization, etc.),
# so rather than locate and parse each giant match we scan the whole file for
# arm-shaped tokens: a run of `"lit"` alternatives joined by `|` and terminated
# by `=>`. EVERY literal in the OR-chain is captured (so `"inc_ref" | "borrow"
# =>` yields both).
#
# CAVEAT (advisory column). This is a TEXTUAL heuristic, not a parse of the
# dispatch's control flow: it can OVER-count (a `"k" =>` match arm in an unrelated
# helper) and, for arms whose pattern spans constructs other than a bare `|`-chain
# of string literals (e.g. guards `"k" if cond =>`, or a binding `Foo("k") =>`),
# it can UNDER-count. The `native_arm` / `wasm_arm` columns are therefore ADVISORY
# — they corroborate the authoritative LLVM/mapper/classifier columns and flag
# kinds for scrutiny; a disposition is never decided on them alone.


def extract_simpleir_arm_kinds(path: Path) -> set[str]:
    text = read_rust_module_cluster(path)
    out: set[str] = set()
    # Match an arm pattern: one-or-more `"lit"` separated by `|`, then `=>`.
    arm = re.compile(
        r'("[a-z][a-z0-9_]*"(?:\s*\|\s*"[a-z][a-z0-9_]*")*)\s*(?:if\b[^=]*?)?=>'
    )
    for m in arm.finditer(text):
        out.update(re.findall(r'"([a-z][a-z0-9_]*)"', m.group(1)))
    return out


def extract_rust_str_slice_consts(
    path: Path, names: set[str] | None = None
) -> dict[str, set[str]]:
    """Extract Rust `const NAME: &[&str] = &[...]` string-slice definitions.

    This intentionally parses only flat string slices: it is used for native
    op-family routing facts whose authority is precisely the local
    `HANDLED_KINDS` const, not arbitrary Rust expressions.
    """
    text = path.read_text(encoding="utf-8")
    const_re = re.compile(
        r"(?:pub(?:\([^)]*\))?\s+)?const\s+([A-Z0-9_]+)\s*:\s*&\s*\[\s*&str\s*\]"
        r"\s*=\s*&\s*\[",
        re.MULTILINE,
    )
    out: dict[str, set[str]] = {}
    for match in const_re.finditer(text):
        name = match.group(1)
        if names is not None and name not in names:
            continue
        end = text.find("];", match.end())
        if end == -1:
            raise RuntimeError(f"unterminated Rust string-slice const {name} in {path}")
        body = text[match.end() : end]
        out[name] = set(re.findall(r'"([a-z][a-z0-9_]*)"', body))
    return out


def extract_native_routing_slice_kinds() -> set[str]:
    """The EXACT native routing authority: the union of every
    `fc/*::HANDLED_KINDS`, `INLINE_DISPATCH_KINDS`, and
    `NATIVE_NO_CODEGEN_RESULT_KINDS` slice.

    This is precisely the set `op_family::native_op_family` routes (plus the
    inline-dispatch and legitimate-no-codegen allowlists). A kind NOT in this set
    is dead at dispatch: result-producing ops hit the loud catch-all panic, while
    no-result side-effect/control-flow ops silently disappear. Unlike
    `extract_native_simpleir_arm_kinds`,
    this does NOT include the advisory textual `function_compiler.rs` arm scan,
    which over-counts (it picks up `"k" =>` arms in unrelated pre-analysis helpers
    — e.g. `"copy" =>` in a `op.var` reader at function_compiler.rs:1919 — that do
    NOT route the value op). The D8 native_codegen_gap check MUST key on this exact
    set, not the advisory union, or the very `copy` bug it exists to catch is
    masked by the textual over-count.
    """
    out: set[str] = set()
    if NATIVE_FC_DIR.exists():
        for path in sorted(NATIVE_FC_DIR.glob("*.rs")):
            consts = extract_rust_str_slice_consts(path)
            for name, kinds in consts.items():
                if (
                    name == "INLINE_DISPATCH_KINDS"
                    or name.endswith("HANDLED_KINDS")
                    or name == "NATIVE_NO_CODEGEN_RESULT_KINDS"
                ):
                    out.update(kinds)
    return out


def extract_native_family_dispatch_slices() -> dict[str, list[tuple[str, str]]]:
    """Parse `FAMILY_DISPATCH_TABLE`: family -> referenced handler slices."""
    text = NATIVE_FC_DIR.joinpath("op_family.rs").read_text(encoding="utf-8")
    rows: dict[str, list[tuple[str, str]]] = {}
    for match in re.finditer(
        r"\(\s*NativeOpFamily::(?P<family>[A-Za-z0-9_]+)\s*,\s*"
        r"super::(?P<module>[A-Za-z0-9_]+)::(?P<const>[A-Z0-9_]+)\s*\)",
        text,
    ):
        rows.setdefault(match.group("family"), []).append(
            (match.group("module"), match.group("const"))
        )
    return rows


def extract_native_family_handlers() -> dict[str, tuple[str, str]]:
    """Parse compile dispatch: family -> (`fc` module, handler function)."""
    text = NATIVE_RS.read_text(encoding="utf-8")
    handlers: dict[str, tuple[str, str]] = {}
    guard_re = re.compile(r"op_family == Some\(fc::NativeOpFamily::([A-Za-z0-9_]+)\)")
    matches = list(guard_re.finditer(text))
    for index, match in enumerate(matches):
        family = match.group(1)
        block_end = matches[index + 1].start() if index + 1 < len(matches) else len(text)
        block = text[match.end() : block_end]
        call = re.search(r"\bfc::([A-Za-z0-9_]+)::(handle_[A-Za-z0-9_]+)\s*\(", block)
        if call is None:
            raise RustMatchParseError(
                f"NativeOpFamily::{family} dispatch does not call an fc handler"
            )
        handlers[family] = (call.group(1), call.group(2))
    return handlers


def extract_native_handler_routing_drifts() -> list[str]:
    """Compare each extracted native handler's real match arms to routed slices.

    `FAMILY_DISPATCH_TABLE` is the dispatch authority. Most families reference a
    single local `HANDLED_KINDS` slice, but delegated families may reference
    sibling slices too (`vec_reductions::HANDLED_KINDS` routes into
    `handle_arith_op`). The union of slices routed to a family and that family's
    handler `match op.kind.as_str()` arms must be exact peers.
    """
    drifts: list[str] = []
    dispatch_slices = extract_native_family_dispatch_slices()
    family_handlers = extract_native_family_handlers()
    for family, slice_refs in sorted(dispatch_slices.items()):
        handler = family_handlers.get(family)
        if handler is None:
            drifts.append(
                f"NativeOpFamily::{family}:FAMILY_DISPATCH_TABLE-has-no-handler"
            )
            continue
        module, fn_name = handler
        path = NATIVE_FC_DIR / f"{module}.rs"
        slice_kinds: set[str] = set()
        slice_labels: list[str] = []
        for slice_module, const_name in slice_refs:
            const_path = NATIVE_FC_DIR / f"{slice_module}.rs"
            consts = extract_rust_str_slice_consts(const_path, names={const_name})
            kinds = consts.get(const_name, set())
            slice_kinds.update(kinds)
            slice_labels.append(f"{slice_module}::{const_name}")
        arm_kinds = set(extract_match_arms(path, fn_name, "match op.kind.as_str()"))
        const_label = "+".join(slice_labels)
        for kind in sorted(arm_kinds - slice_kinds):
            drifts.append(
                f"{path.relative_to(ROOT).as_posix()}:{fn_name}:{kind}:"
                f"arm-not-in-{const_label}"
            )
        for kind in sorted(slice_kinds - arm_kinds):
            drifts.append(
                f"{path.relative_to(ROOT).as_posix()}:{fn_name}:{kind}:"
                f"{const_label}-not-in-arm"
            )
    return drifts


def extract_native_simpleir_arm_kinds() -> set[str]:
    """Native SimpleIR coverage from extracted op-family authorities (ADVISORY).

    `fc/*::HANDLED_KINDS` is now the native routing source of truth. The old
    `function_compiler.rs` arm scan remains included so inline arms and any
    not-yet-extracted residual arms stay visible during decomposition. Because the
    textual scan can over-count (see extract_simpleir_arm_kinds's caveat), this
    union is the ADVISORY `native_arm` column; the EXACT routing authority used by
    the D8 native_codegen_gap check is `extract_native_routing_slice_kinds`.
    """
    out = extract_simpleir_arm_kinds(NATIVE_RS)
    out.update(extract_native_routing_slice_kinds())
    return out


def extract_native_lower_nonresult_kinds() -> set[str]:
    """Kinds whose lowered SimpleIR op carries NO result (`out` absent/`None`).

    The single source of truth for "does this kind's SimpleIR op carry a result?"
    is ``fn lower_op`` in ``lower_to_simple/op_lowering.rs`` (the per-OpCode SSA->SimpleIR
    lowering): each ``OpCode::X => Some(OpIR { kind: "K", … })`` arm sets ``out``
    to ``out_var`` / ``Some(..)`` / a ``*_var`` (result-producing) or omits it /
    sets ``None`` (no result). This extractor returns exactly the kinds for which
    EVERY ``lower_op`` arm omits/``None``s ``out`` — the precise mirror of the
    catch-all's ``op.out.is_some()`` gate.

    A kind that ``lower_op`` does not mention (e.g. the ``copy``/``add`` family,
    which reach SimpleIR via the ``_original_kind`` passthrough in
    ``lower_preserved_op``, not ``lower_op``) is NOT returned, i.e. it is reported
    as result-producing. D8 no longer uses this set as an exemption: no-result
    side-effect/control-flow ops must still be explicitly routed by native
    ``HANDLED_KINDS`` slices.
    """
    src = LOWER_TO_SIMPLE_OP_LOWERING_RS.read_text(encoding="utf-8")
    fm = re.search(r"\bfn\s+lower_op\b", src)
    if fm is None:
        raise RustMatchParseError(
            "fn lower_op not found in lower_to_simple/op_lowering.rs"
        )
    open_idx = src.index("{", fm.end())
    depth = 0
    end = None
    for idx in range(open_idx, len(src)):
        c = src[idx]
        if c == "{":
            depth += 1
        elif c == "}":
            depth -= 1
            if depth == 0:
                end = idx
                break
    if end is None:
        raise RustMatchParseError("unbalanced braces in fn lower_op")
    body = src[open_idx + 1 : end]

    # kind -> set of `out:` dispositions seen across that kind's OpIR struct
    # literal(s). A struct with no top-level `out:` defaults to `out: None`
    # (`..OpIR::default()`), recorded as the "<absent>" disposition.
    dispositions: dict[str, set[str]] = {}
    for km in re.finditer(r'kind:\s*"([a-z][a-z0-9_]*)"\.to_string\(\)', body):
        kind = km.group(1)
        # The enclosing OpIR struct literal is the nearest `{` before `kind:`.
        brace = body.rfind("{", 0, km.start())
        if brace == -1:
            continue
        d = 0
        j = brace
        close = None
        while j < len(body):
            ch = body[j]
            if ch == "{":
                d += 1
            elif ch == "}":
                d -= 1
                if d == 0:
                    close = j
                    break
            j += 1
        struct = body[brace : close if close is not None else len(body)]
        om = re.search(r"\bout:\s*([A-Za-z_][A-Za-z0-9_:]*|Some|None)", struct)
        dispositions.setdefault(kind, set()).add(om.group(1) if om else "<absent>")
    return {kind for kind, disp in dispositions.items() if disp <= {"None", "<absent>"}}


# ---------------------------------------------------------------------------
# Matrix assembly
# ---------------------------------------------------------------------------

# Kinds that are NOT cross-component op kinds in the `kind_to_opcode` sense: the
# CFG/SSA or pre-SSA lowering layer consumes them structurally. They legitimately
# have no mapper arm and are excluded from the emitted-but-unmapped danger cells.
#
# The authority is the registry's [[simpleir_control_kind]] section, not the Rust
# helper bodies. Runtime helpers delegate to generated tables; this audit reads
# the same table directly so implementation shape cannot alter the semantic
# exemption set.
_SIMPLEIR_CONTROL_CONSUMED_FIELDS = (
    "structural",
    "pre_ssa_rewritten",
    "ssa_only",
)


def structural_kinds_from_registry(data: dict) -> set[str]:
    out: set[str] = set()
    for row in data.get("simpleir_control_kind", []):
        kind = row.get("kind")
        if not isinstance(kind, str):
            raise RuntimeError(f"malformed simpleir_control_kind row: {row}")
        if any(row.get(field, False) for field in _SIMPLEIR_CONTROL_CONSUMED_FIELDS):
            out.add(kind)
    return out


def extract_vec_reduction_ops() -> set[str]:
    """The LLVM `VEC_REDUCTION_OPS` exact table (kind, arity). The vec-* family is
    lowered on LLVM by `vec_reduction_runtime_symbol(kind)` BEFORE the dedicated
    `match`, so membership here is real LLVM coverage the arm-extractor misses."""
    src = LLVM_VEC_REDUCTIONS_RS.read_text(encoding="utf-8")
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
    llvm_runtime_fallback_eligible: bool
    classifier_class: str  # FreshValue / OwnedAlias / TransparentAlias / InertMarker
    native_arm: bool  # ADVISORY: textual function_compiler.rs scan ∪ routing slices
    native_routing_slice: (
        bool  # EXACT: in a fc/*::HANDLED_KINDS/INLINE/NO_CODEGEN slice
    )
    wasm_arm: bool
    structural: bool
    produces_result: (
        bool  # lowered SimpleIR op carries `out: Some` (not a no-result stmt)
    )

    @property
    def llvm_covered(self) -> bool:
        """A `Copy`-carried kind is soundly lowered on the LLVM lane iff it has a
        dedicated arm, is in the vec table, or the runtime-call fallback can emit
        its exact ABI. Otherwise the LLVM `Copy` arm FAILS LOUD at build."""
        return (
            self.llvm_dedicated_arm
            or self.llvm_vec_table
            or self.llvm_runtime_fallback_eligible
        )

    def as_dict(self) -> dict:
        return {
            "kind": self.kind,
            "frontend_emits": self.frontend_emits,
            "mapper_maps": self.mapper_maps,
            "llvm_dedicated_arm": self.llvm_dedicated_arm,
            "llvm_vec_table": self.llvm_vec_table,
            "llvm_runtime_fallback_eligible": self.llvm_runtime_fallback_eligible,
            "llvm_covered": self.llvm_covered,
            "classifier_class": self.classifier_class,
            "native_arm": self.native_arm,
            "native_routing_slice": self.native_routing_slice,
            "wasm_arm": self.wasm_arm,
            "structural": self.structural,
            "produces_result": self.produces_result,
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
    owned_alias: set[str]
    inert_marker: set[str]
    transparent_alias: set[str]
    no_heap_move: set[str]
    runtime_symbols: set[str]
    structural_kinds: set[str]
    llvm_void_runtime_abi_mismatch: list[str]
    native_handler_routing_drift: list[str]

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
            # ABI-eligible runtime fallback). On LLVM this hits the `Copy` fail-loud guard
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
            # value/heap producer (heuristic: an ABI-eligible `molt_<kind>`
            # runtime fallback exists, i.e. it is a real boxed runtime op). Such a
            # kind is unioned-by-default into operand 0's alias root; if it ever
            # mints a fresh ref the drop pass leaks it (today) and a future
            # promotion to FreshValue without a backend arm escalates to UAF.
            "classifier_silent_fallthrough": [],
            # D4 — no SimpleIR-lane lowering. Emitted, not structural, unmapped,
            # AND neither native nor WASM has a dispatch arm AND no ABI-eligible
            # runtime fallback. Nothing can lower it on the native/WASM lanes (subject to
            # the arm-detector's over-counting caveat — see extract_simpleir_arm_kinds).
            "simpleir_lane_gap": [],
            # D5 — dead mapper vocabulary. A first-class opcode mapping the
            # frontend never emits (a STALE-BASE smell: the spelling the emitter
            # uses may have diverged, à la floor_div).
            "mapped_never_emitted": [],
            # D6 — dead FreshValue allow-list entry the frontend never emits.
            "freshvalue_never_emitted": [],
            # D7 — explicit LLVM void-runtime fallback table drift. These entries
            # are backend source data, so a missing symbol, wrong return, wrong
            # arity, or non-boxed parameter must fail the audit before emission
            # reaches the stale ABI row.
            "llvm_void_runtime_abi_mismatch": list(self.llvm_void_runtime_abi_mismatch),
            # D8 — native codegen gap (the `copy`-instance precondition, fixed
            # 2026-06-24). A kind the frontend EMITS that REACHES native codegen
            # as SimpleIR, but that NO native routing slice claims
            # (not in any fc/*::HANDLED_KINDS, INLINE_DISPATCH_KINDS, or
            # NATIVE_NO_CODEGEN_RESULT_KINDS). Such a kind is dead at dispatch:
            # `native_op_family()` returns None and the handler never sees it.
            # This is a hard failure for result-producing ops and a silent
            # side-effect skip for no-result ops, so both classes are audited.
            # "Claimed by native routing" is keyed on the EXACT routing slices
            # (`native_routing_slice`), NOT the advisory textual `native_arm`
            # union — the textual scan over-counts `copy` from an unrelated
            # pre-analysis helper, which would otherwise MASK the very bug this
            # closes.
            "native_codegen_gap": [],
            # D9 — local handler routing drift. The adjacent `HANDLED_KINDS`
            # const and `match op.kind.as_str()` body must be exact peers; this
            # catches both unreachable arms and routed kinds that have no arm.
            "native_handler_routing_drift": list(self.native_handler_routing_drift),
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
                or kind in self.owned_alias
                or kind in self.inert_marker
                or kind in self.transparent_alias
                or kind in self.no_heap_move
            )
            if emitted_unmapped and not row.llvm_covered:
                cats["llvm_coverage_gap"].append(kind)
            if (
                row.classifier_class in {"FreshValue", "OwnedAlias"}
                and not row.llvm_covered
            ):
                cats["freshvalue_llvm_gap"].append(kind)
            if (
                emitted_unmapped
                and not explicit_classified
                and row.llvm_runtime_fallback_eligible
            ):
                cats["classifier_silent_fallthrough"].append(kind)
            if (
                emitted_unmapped
                and not row.native_arm
                and not row.wasm_arm
                and not row.llvm_runtime_fallback_eligible
            ):
                cats["simpleir_lane_gap"].append(kind)
            if row.mapper_maps and not row.frontend_emits:
                cats["mapped_never_emitted"].append(kind)
            if row.classifier_class == "FreshValue" and not row.frontend_emits:
                cats["freshvalue_never_emitted"].append(kind)
            # D8 — native codegen gap. A frontend-emitted, non-structural kind
            # with no native routing SLICE is either a result-producing panic or
            # a no-result side-effect/control-flow skip. Keyed on the exact
            # routing slices, not the advisory textual native_arm.
            if row.frontend_emits and not row.native_routing_slice:
                cats["native_codegen_gap"].append(kind)
        return {k: sorted(v) for k, v in cats.items()}


def classify(
    kind: str,
    fresh_value: set[str],
    fresh_prefixes: list[str],
    owned_alias: set[str],
    inert: set[str],
    transparent_alias: set[str],
    no_heap_move: set[str],
) -> str:
    if kind in fresh_value or any(kind.startswith(p) for p in fresh_prefixes):
        return "FreshValue"
    if kind in owned_alias:
        return "OwnedAlias"
    if kind in inert:
        return "InertMarker"
    if kind in transparent_alias:
        return "TransparentAlias"
    if kind in no_heap_move:
        return "TransparentAlias"
    # The classifier's `_ =>` default. Every kind reaching here is treated as a
    # transparent alias of operand 0 by `classify_copy_kind`.
    return "TransparentAlias"


def run_audit() -> AuditResult:
    fk = extract_frontend_kinds()
    # Backend mapper + CopyLowering-classifier vocabularies come from the registry
    # (the single source of truth; the hand-written Rust delegates to its
    # generated tables). See the OP_KINDS_TOML note above.
    registry = _load_op_kinds_toml()
    mapper = mapper_kinds_from_registry(registry)
    fresh = set(registry.get("classifier_fresh_value", []))
    fresh_prefixes = list(registry.get("classifier_fresh_value_prefixes", []))
    owned_alias = set(registry.get("classifier_owned_alias", []))
    inert = set(registry.get("classifier_inert_marker", []))
    transparent_alias = set(registry.get("classifier_transparent_alias", []))
    no_heap = set(registry.get("classifier_no_heap_move", []))
    # LLVM arms, the vec-reduction table, and runtime extern ABIs are NOT
    # generated — extract them from source as before.
    llvm_arms = extract_llvm_preserved_op_kinds()
    llvm_vec = extract_vec_reduction_ops()
    runtime_externs = extract_runtime_molt_externs()
    classified_runtime_imports = extract_llvm_classified_runtime_imports()
    runtime_syms = set(runtime_externs)
    classified_runtime_fallback = {
        symbol.removeprefix("molt_")
        for symbol, ext in runtime_externs.items()
        if (classified := classified_runtime_imports.get(symbol)) is not None
        if symbol.startswith("molt_")
        and runtime_extern_classified_fallback_eligible(ext, classified)
    }
    void_runtime_ops = extract_llvm_void_runtime_ops()
    void_runtime_mismatches = llvm_void_runtime_abi_mismatches(
        void_runtime_ops, runtime_externs
    )
    llvm_runtime_fallback = {
        kind
        for kind, (symbol, arity) in void_runtime_ops.items()
        if symbol in runtime_externs
        and runtime_extern_is_boxed_void_fallback_eligible(
            runtime_externs[symbol], arity
        )
    }
    llvm_runtime_fallback |= classified_runtime_fallback
    native_arms = extract_native_simpleir_arm_kinds()
    native_routing = extract_native_routing_slice_kinds()
    native_handler_routing_drift = extract_native_handler_routing_drifts()
    wasm_arms = extract_simpleir_arm_kinds(WASM_RS)
    structural = structural_kinds_from_registry(registry)
    # Kinds PROVEN no-result by lower_to_simple.rs `fn lower_op` (their SimpleIR op
    # carries no `out`). This remains a report column, but D8 now audits no-result
    # native routing too: side-effect/control-flow ops must be explicitly routed.
    native_nonresult = extract_native_lower_nonresult_kinds()

    universe = (
        fk.all
        | mapper
        | llvm_arms
        | llvm_vec
        | llvm_runtime_fallback
        | fresh
        | owned_alias
        | inert
        | transparent_alias
        | no_heap
    )

    rows: dict[str, KindRow] = {}
    for kind in sorted(universe):
        rows[kind] = KindRow(
            kind=kind,
            frontend_emits=kind in fk.all,
            mapper_maps=kind in mapper,
            llvm_dedicated_arm=kind in llvm_arms,
            llvm_vec_table=kind in llvm_vec,
            llvm_runtime_fallback_eligible=kind in llvm_runtime_fallback,
            classifier_class=classify(
                kind,
                fresh,
                fresh_prefixes,
                owned_alias,
                inert,
                transparent_alias,
                no_heap,
            ),
            native_arm=kind in native_arms,
            native_routing_slice=kind in native_routing,
            wasm_arm=kind in wasm_arms,
            structural=kind in structural,
            produces_result=kind not in native_nonresult,
        )

    return AuditResult(
        rows=rows,
        frontend=fk,
        mapper_kinds=mapper,
        llvm_arms=llvm_arms,
        llvm_vec_table=llvm_vec,
        fresh_value=fresh,
        fresh_value_prefixes=fresh_prefixes,
        owned_alias=owned_alias,
        inert_marker=inert,
        transparent_alias=transparent_alias,
        no_heap_move=no_heap,
        runtime_symbols=runtime_syms,
        structural_kinds=structural,
        llvm_void_runtime_abi_mismatch=void_runtime_mismatches,
        native_handler_routing_drift=native_handler_routing_drift,
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

    # The floordiv/floor_div spelling schism (the proven bug #5) is COLLAPSED
    # (task #57 commit 2): the registry now maps BOTH the frontend spelling
    # `floordiv` (canonical) and the round-trip spelling `floor_div` (alias) to
    # OpCode::FloorDiv, so a frontend `//` reaches the first-class arith/overflow
    # path instead of the boxed Copy{floordiv} slow path. Both must be mapped now;
    # the frontend must still emit the canonical `floordiv`.
    check("floordiv" in res.frontend.all, "frontend must emit 'floordiv'")
    check(
        "floordiv" in res.mapper_kinds,
        "mapper must map 'floordiv' (the collapse routes // to OpCode::FloorDiv)",
    )
    check(
        "floor_div" in res.mapper_kinds,
        "mapper must still map 'floor_div' (round-trip/legacy alias of floordiv)",
    )
    check(
        "floor_div" not in res.frontend.all,
        "frontend must NOT emit 'floor_div' (it is the round-trip alias, not the "
        "frontend spelling)",
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
    for k in ("loop_index_start", "loop_index_next"):
        check(
            k in res.structural_kinds,
            f"pre-SSA loop-IV kind '{k}' must be extracted as structural",
        )
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
    check(
        res.rows.get("binding_alias") is not None
        and res.rows["binding_alias"].classifier_class == "OwnedAlias",
        "'binding_alias' must classify OwnedAlias",
    )
    # D8 native_codegen_gap ground-truth anchors. `copy` is the 2026-06-24
    # instance: frontend-emitted, result-producing (a reassigned-local ternary
    # `r = a if c else b` produces a value), and now claimed by
    # value_transfer::HANDLED_KINDS. These anchors fail LOUD if the check were ever
    # silently disabled or mis-scoped.
    check("copy" in res.frontend.all, "frontend must emit 'copy'")
    check(
        res.rows["copy"].produces_result,
        "'copy' must be result-producing (else D8 would never check it — the "
        "exact blind spot that hid the copy bug)",
    )
    check(
        res.rows["copy"].native_routing_slice,
        "'copy' must be claimed by a native routing SLICE "
        "(value_transfer::HANDLED_KINDS) — else native_codegen_gap regresses. "
        "NB: this asserts the EXACT slice authority, not the advisory textual "
        "native_arm (which over-counts copy from a pre-analysis helper).",
    )
    # The value-custody/alias family (the same family as the copy bug, emitted as
    # registry-bypassing literals by serialization.py) must all be claimed by a
    # native routing slice.
    for k in ("borrow", "release", "cast", "widen", "identity_alias", "binding_alias"):
        check(
            res.rows.get(k) is not None and res.rows[k].native_routing_slice,
            f"value-custody kind '{k}' must be claimed by a native routing slice",
        )
    # No-result statement ops still need explicit native routing: otherwise the
    # catch-all silently skips their side effects/control metadata. Keep the
    # non-result extractor as a report fact, but do not use it as a D8 exemption.
    for k in ("del_boundary", "try_start", "try_end"):
        check(
            res.rows.get(k) is not None and not res.rows[k].produces_result,
            f"no-result statement kind '{k}' must be extracted as non-result",
        )
        check(
            res.rows.get(k) is not None and res.rows[k].native_routing_slice,
            f"no-result statement kind '{k}' must be claimed by a native routing slice",
        )
    check(
        res.dangerous()["native_codegen_gap"] == [],
        "native_codegen_gap must be empty on a healthy tree",
    )
    check(
        res.dangerous()["native_handler_routing_drift"] == [],
        "native handler match arms and HANDLED_KINDS slices must agree",
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
        for rel_path, ln, dump in fk.unresolved:
            print(f"      {rel_path}:{ln}: {dump}")
    print(f"  ssa.rs kind_to_opcode arms               : {len(res.mapper_kinds)}")
    print(f"  llvm dedicated arms                      : {len(res.llvm_arms)}")
    print(f"  llvm VEC_REDUCTION_OPS table              : {len(res.llvm_vec_table)}")
    print(f"  classifier FreshValue allow-list         : {len(res.fresh_value)}")
    print(f"    + prefix rules                         : {res.fresh_value_prefixes}")
    print(f"  classifier OwnedAlias allow-list         : {len(res.owned_alias)}")
    print(f"  classifier InertMarker arms              : {len(res.inert_marker)}")
    print(f"  classifier transparent-alias set         : {len(res.transparent_alias)}")
    print(f"  classifier no-heap-move (alias) set      : {len(res.no_heap_move)}")
    print(f"  structural/pre-SSA consumed kinds        : {len(res.structural_kinds)}")
    print(f"  runtime molt_* extern exports            : {len(res.runtime_symbols)}")
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
            if k not in res.rows:
                print(f"   {k}")
                continue
            row = res.rows[k]
            print(
                f"   {k:32s} mapper={_b(row.mapper_maps)} "
                f"llvm_arm={_b(row.llvm_dedicated_arm)} "
                f"llvm_vec={_b(row.llvm_vec_table)} "
                f"llvm_abi={_b(row.llvm_runtime_fallback_eligible)} "
                f"class={row.classifier_class:16s} "
                f"native={_b(row.native_arm)} wasm={_b(row.wasm_arm)}"
            )
        print()

    print(
        "FULL DRIFT MATRIX  (fe=frontend-emits map=mapper-arm la=llvm-arm "
        "lv=llvm-vec ls=llvm-sym st=structural/pre-SSA)"
    )
    hdr = f"{'kind':34s} fe map  la lv ls {'classifier':16s} nat wasm st"
    print(hdr)
    print("-" * len(hdr))
    for kind, row in res.rows.items():
        print(
            f"{kind:34s} {_b(row.frontend_emits)}   {_b(row.mapper_maps)}   "
            f"{_b(row.llvm_dedicated_arm)}  {_b(row.llvm_vec_table)}  "
            f"{_b(row.llvm_runtime_fallback_eligible)}  "
            f"{row.classifier_class:16s} {_b(row.native_arm)}   {_b(row.wasm_arm)}   "
            f"{_b(row.structural)}"
        )


def to_baseline(res: AuditResult) -> dict:
    """The committed baseline = the current dangerous-cell sets.

    CI fails on either new current danger or stale baseline-only danger. Stale
    entries are not harmless bookkeeping: they mask a future regression that
    reintroduces a previously removed dangerous cell.
    """
    return {"dangerous": res.dangerous()}


def check_against_baseline(res: AuditResult) -> int:
    if not BASELINE_PATH.exists():
        print(
            f"ERROR: baseline {BASELINE_PATH} missing; run --write-baseline first",
            file=sys.stderr,
        )
        return 2
    baseline = json.loads(BASELINE_PATH.read_text(encoding="utf-8"))
    base = baseline.get("dangerous", {})
    current = res.dangerous()
    rc = 0
    for cat in sorted(set(current) | set(base)):
        current_items = set(current.get(cat, []))
        base_items = set(base.get(cat, []))
        new = sorted(current_items - base_items)
        stale = sorted(base_items - current_items)
        if new:
            rc = 1
            print(
                f"NEW dangerous-cell in '{cat}': {new}",
                file=sys.stderr,
            )
        if stale:
            rc = 1
            print(
                f"STALE dangerous-cell baseline in '{cat}': {stale}",
                file=sys.stderr,
            )
    if rc == 0:
        print("op-kind drift check: OK (dangerous-cell baseline is exact)")
    else:
        print(
            "\nA new op kind drifted across the frontend/backend boundary. "
            "Add a mapper arm in ssa.rs kind_to_opcode (or, for a CFG/SSA-consumed "
            "control kind, add a [[simpleir_control_kind]] row in op_kinds.toml), "
            "classify it in alias_analysis.rs, ensure LLVM coverage "
            "(dedicated arm or molt_<kind> symbol), and refresh the baseline once "
            "the fix lands. If the error is stale baseline-only danger, refresh "
            "the baseline from the current audit after verifying the removal is "
            "intentional.\n"
            "If the failure is in 'native_codegen_gap', a frontend-emitted kind "
            "has no native routing slice: result-producing ops panic in the "
            "native dispatch catch-all, while no-result side-effect/control-flow "
            "ops silently disappear. Add the kind to the owning "
            "`function_compiler/fc/*::HANDLED_KINDS` (or, if it legitimately "
            "needs no native codegen, to `op_family::NATIVE_NO_CODEGEN_RESULT_KINDS`). "
            "If the failure is in 'native_handler_routing_drift', align the "
            "handler's `HANDLED_KINDS` const with its adjacent "
            "`match op.kind.as_str()` arms. Refresh the baseline only after the "
            "structural fix lands.",
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
        for rel_path, ln, dump in res.frontend.unresolved:
            print(f"  {rel_path}:{ln}: {dump}", file=sys.stderr)
        return 3

    if args.write_baseline:
        BASELINE_PATH.write_text(
            json.dumps(to_baseline(res), indent=2) + "\n",
            encoding="utf-8",
            newline="\n",
        )
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
