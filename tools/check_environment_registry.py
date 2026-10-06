#!/usr/bin/env python3
"""Environment-registry gate: every ``MOLT_*`` name the source reads is registered.

THE DEFECT CLASS THIS PREVENTS
------------------------------
A misspelled or renamed ``MOLT_*`` environment variable is silently ignored:
the reader sees "unset" and the operator sees nothing. Two names for one fact
(aliases) let that drift hide for years, and a hand-written reference table
goes stale the day it is written. This gate makes the registry
(``src/molt/environment_registry.toml``) the single authority and proves it
against the code with a structural scan, not a grep.

WHAT IT PROVES (``--check``)
----------------------------
1. The shipped projection and the reference page are current
   (``tools/gen_environment_registry.py --check``).
2. Every definite environment access of a ``MOLT_*`` name in first-party
   source is a registered variable, a family expansion, a prefix family, or a
   guard stem. Composed prefixes (``f"MOLT_RESOURCE_{x}"``) must prefix a
   registered name.
3. Every registered variable has a reader (a read or a programmatic
   reference, never only a message); an ``internal`` or ``ci`` row may instead
   have a set site when its consumer is outside the tree (the summary names
   it).
   Every family has a composed reader for its suffix; every stem and prefix
   family is used.
4. Every ``owner`` path exists and contains a site for its entry.
5. No retired name survives anywhere: source, tests, workflows, or docs. The
   one exception is declared in the registry: a retired row's ``rejected_by``
   lists the files of a separate process that fails closed on the name itself
   (it cannot load this registry) and that process's tests; each listed file
   must still name it.
6. No first-party source string or comment mentions an unregistered
   ``MOLT_*`` name (stale messages are drift too).

WHAT "DEFINITE ENVIRONMENT ACCESS" MEANS
----------------------------------------
Python (``ast``): a subscript, ``.get``, ``.pop``, ``.setdefault`` or ``in``
whose receiver is an environment mapping; ``os.getenv`` / ``putenv`` /
``unsetenv`` / ``setenv`` / ``delenv``; and a literal passed to a helper whose
parameter flows into such an access (``_env_bool(env, ("MOLT_X",))``). An
environment mapping is ``os.environ``, a name spelled ``env``, ``environ``,
``environment``, ``*_env`` or ``*_environment``, or a local assigned from
one of those. This is the repository's naming contract for environment
dicts; keep it when you add code. Keys resolve through module constants and
``from X import NAME``. A dict literal is a set site when it is passed as
``env=``, merged with ``<env>.update(...)``, or assigned to an env name.

Rust (token scan, comments skipped): a ``"MOLT_*"`` literal that is the first
argument of ``var`` / ``var_os`` / ``option_env!`` / ``env!`` /
``required_env`` (read), ``set_var`` / ``.env`` (set), ``remove_var`` /
``env_remove`` (unset).

Text (workflows, TOML, shell, Makefiles): ``<NAME>:`` in an ``env:`` map and
``<NAME>=`` assignments (set); ``${<NAME>}``, ``$<NAME>``, ``%<NAME>%``,
``env.<NAME>`` and ``vars.<NAME>`` (read); ``--prefix <NAME>`` (stem).

Everything else that names a ``MOLT_*`` string exactly (array elements,
constants, other call arguments) is a *reference*: it counts as reader
evidence for a registered name but is not enforced, because the same
spelling also names symbols, intrinsics and macros. ``--unclassified``
lists unregistered references for review.

Under ``tests/``, the harness modules that ``tools/proof_plan.toml`` lists in
``authority_inputs`` (``tests/molt_diff.py``, the conformance runners, the
process guards) are first-party readers and are enforced; every other test
module is reader evidence and a retired-name violation only, because negative
tests need unregistered names. Program corpora that Molt compiles are skipped.
The registry's own generated projection is not scanned: it is the registry.

Usage:
    python3 tools/check_environment_registry.py --check         # the gate
    python3 tools/check_environment_registry.py --inventory     # JSON of every site
    python3 tools/check_environment_registry.py --list          # names and site counts
    python3 tools/check_environment_registry.py --unclassified  # unregistered references

Exit: 0 = clean; 2 = violations; 3 = usage or registry-format error.
"""

from __future__ import annotations

import argparse
import ast
import builtins
from collections import defaultdict
from collections.abc import Iterable, Iterator
from concurrent.futures import ProcessPoolExecutor
from dataclasses import asdict, dataclass
import io
import json
import os
from pathlib import Path
import re
import sys
import tokenize
import tomllib

ROOT = Path(__file__).resolve().parents[1]
for _import_root in (ROOT / "tools", ROOT / "src"):
    if str(_import_root) not in sys.path:
        sys.path.insert(0, str(_import_root))

from _io_utf8 import force_utf8_stdio  # noqa: E402
import gen_environment_registry as _gen  # noqa: E402
from molt.environment_registry import (  # noqa: E402
    EnvironmentRegistry,
    EnvironmentRegistryError,
)

force_utf8_stdio()

# Roots whose definite accesses must be registered.
ENFORCED_PYTHON_ROOTS = ("src", "tools", "bench/scripts", "packaging")
ENFORCED_RUST_ROOTS = ("runtime", "tools/proof_supervisor")
ENFORCED_TEXT_ROOTS = (
    ".github/workflows",
    ".github/actions",
    "tools",
    "bench/scripts",
    "packaging",
    "wasm",
)
ENFORCED_TEXT_FILES = (
    "Makefile",
    "Makefile.pgo",
    "pyproject.toml",
    ".cargo/config.toml",
)
# Roots scanned for reader evidence and retired names only.
EVIDENCE_PYTHON_ROOTS = ("tests",)
EVIDENCE_TEXT_ROOTS = ("tests",)
# User-facing docs: the top-level pages. Design notes, plans, specs and agent
# ledgers under docs/<subdir>/ are history and are scanned for retired names
# only.
DOC_ENFORCED_GLOB = "docs/*.md"
DOC_FILES = ("README.md", "AGENTS.md", "CONTRIBUTING.md")
DOC_HISTORY_ROOTS = ("docs",)
# The registry's generated projection names every retired variable by design.
REGISTRY_PROJECTION = "src/molt/_environment_registry.py"
# Append-only ledgers this gate must not force edits to.
DOC_SKIP_FILES = frozenset(
    {"docs/agent/CLAIMS.md", "docs/agent/V1_HANDOFF_FINDINGS.md"}
)
SKIP_DIR_NAMES = frozenset(
    {".venv", "target", "__pycache__", "node_modules", ".git", "vendor"}
)
# Program corpora Molt compiles or mirrors: env reads there are target-program
# behaviour, not first-party readers.
SKIP_PREFIXES = (
    "tests/differential/",
    "tests/compliance/",
    "tests/molt_only/",
    "tests/harness/corpus/",
    "tests/fixtures/",
)
TEXT_SUFFIXES = frozenset(
    {".yml", ".yaml", ".toml", ".sh", ".ps1", ".bat", ".cmd", ".mjs", ".js"}
)
RUST_TEST_MARKERS = (
    "/tests/",
    "_tests/",
    "/tests.rs",
    "/test_",
    "_test.rs",
    "_tests.rs",
)

EXACT = re.compile(r"^MOLT_[A-Z0-9_]+$")
TOKEN = re.compile(r"MOLT_[A-Z0-9_]+")
PREFIX_LITERAL = re.compile(r"^MOLT_[A-Z0-9_]*_$")
ENV_LINE = re.compile(r"^(MOLT_[A-Z0-9_]+)=$")
# A file worth parsing: it names a MOLT_* token or composes a `{stem}_SUFFIX`.
WORTH_PARSING = re.compile(r"MOLT_|\}_[A-Z][A-Z0-9_]*[\"']")
SUFFIX_LITERAL = re.compile(r"^_[A-Z0-9_]+$")
BARE_SUFFIX = re.compile(r"^[A-Z][A-Z0-9]*(?:_[A-Z0-9]+)+$")
RUST_FORMAT_PREFIX = re.compile(r"^(MOLT_[A-Z0-9_]*_)\{")
RUST_FORMAT_SUFFIX = re.compile(r"^\{[^}]*\}(_[A-Z0-9_]+)$")
ENV_RECEIVER = re.compile(r"(^|[._])(env|environ|environment)$")
# A mention is "env-shaped" when the surrounding text says it is a variable.
ENV_SHAPED = re.compile(
    r"(?:\$\{?|%|\benv:|\b(?:set|export|unset|setenv|environment variables?|env vars?)\s+`?)"
    r"(MOLT_[A-Z0-9_]+)"
    r"|(MOLT_[A-Z0-9_]+)`?(?:=(?!=)|\s+(?:environment variable|env var|is (?:set|unset|not set|empty)))"
)
FORM_ENV_MENTION = "env-mention"

PY_ENV_FUNCTIONS = {
    "getenv": "read",
    "putenv": "set",
    "setenv": "set",
    "unsetenv": "unset",
    "delenv": "unset",
}
PY_READ_METHODS = frozenset({"get", "__contains__"})
PY_SET_METHODS = frozenset({"setdefault"})
PY_UNSET_METHODS = frozenset({"pop"})

# Generic names never resolve as env helpers: a class method called ``get``
# or ``insert`` must not lend its roles to every ``x.get(...)`` call.
_GENERIC_CALLEES = (
    frozenset(dir(builtins))
    | frozenset(
        name for kind in (dict, list, set, str, bytes, tuple) for name in dir(kind)
    )
    | frozenset(
        {
            "get",
            "insert",
            "remove",
            "push",
            "pop",
            "contains",
            "contains_key",
            "entry",
            "unwrap_or",
            "unwrap_or_else",
            "unwrap_or_default",
            "map",
            "and_then",
            "ok",
            "Some",
            "Ok",
            "Err",
            "format!",
            "vec!",
            "write!",
            "writeln!",
            "println!",
            "eprintln!",
            "bail!",
            "anyhow!",
            "context",
            "with_context",
            "expect",
            "new",
            "from",
            "into",
            "to_string",
            "as_str",
            "clone",
            "set",
            "update",
            "add",
        }
    )
)

ACCESS_READ = "read"
ACCESS_SET = "set"
ACCESS_UNSET = "unset"
ACCESS_REFERENCE = "reference"
ACCESS_MENTION = "mention"
ACCESS_STEM = "stem"
ACCESS_PREFIX = "composed-prefix"
ACCESS_SUFFIX = "composed-suffix"
DEFINITE_ACCESSES = frozenset({ACCESS_READ, ACCESS_SET, ACCESS_UNSET})
READER_ACCESSES = frozenset({ACCESS_READ, ACCESS_REFERENCE})


@dataclass(frozen=True, slots=True)
class Site:
    path: str
    line: int
    name: str
    access: str
    form: str
    language: str
    enforced: bool


@dataclass(frozen=True, slots=True)
class Violation:
    rule: str
    name: str
    path: str
    line: int
    message: str


@dataclass(frozen=True, slots=True)
class Scan:
    sites: tuple[Site, ...]
    unresolved: tuple[Site, ...]

    def by_name(self) -> dict[str, list[Site]]:
        grouped: dict[str, list[Site]] = defaultdict(list)
        for site in self.sites:
            grouped[site.name].append(site)
        return grouped


# --------------------------------------------------------------------------
# File discovery
# --------------------------------------------------------------------------


def _rel(path: Path, root: Path = ROOT) -> str:
    return path.relative_to(root).as_posix()


def _skipped(rel: str) -> bool:
    parts = rel.split("/")
    if any(part in SKIP_DIR_NAMES for part in parts):
        return True
    if ".generated." in parts[-1]:
        return True
    return rel.startswith(SKIP_PREFIXES)


def _iter_files(
    roots: Iterable[str], suffixes: frozenset[str], root: Path = ROOT
) -> Iterator[Path]:
    for entry in roots:
        base = root / entry
        if not base.exists():
            continue
        if base.is_file():
            yield base
            continue
        for path in sorted(base.rglob("*")):
            if path.suffix not in suffixes or not path.is_file():
                continue
            if _skipped(_rel(path, root)):
                continue
            yield path


def _is_rust_test_path(rel: str) -> bool:
    return any(marker in rel for marker in RUST_TEST_MARKERS)


# --------------------------------------------------------------------------
# Python scanner
# --------------------------------------------------------------------------
#
# Each module is processed once, in a worker process: parse, one walk for
# facts (env-derived locals per scope, env-key parameters, call edges, loops
# that interpolate suffixes), one walk for candidate sites. Workers return
# plain data; the parent runs the helper fixpoint over the call edges and
# finishes classifying the candidates that depend on it.


@dataclass(frozen=True, slots=True)
class _CallEdge:
    """``caller`` passes a parameter (or a loop variable over one) to ``callee``."""

    callee: str
    index: int | None
    keyword: str | None
    arg_param: str | None
    arg_loop_param: str | None


@dataclass(slots=True)
class _FunctionFacts:
    name: str
    params: tuple[str, ...]
    key_params: dict[str, set[str]]  # parameter -> accesses it reaches (grows only)
    key_list_params: set[str]
    suffix_list_params: set[str]
    edges: list[_CallEdge]


def _label(accesses: set[str]) -> str:
    """One access label for a parameter that may reach several forms."""

    if ACCESS_SET in accesses:
        return ACCESS_SET
    if ACCESS_UNSET in accesses and ACCESS_READ not in accesses:
        return ACCESS_UNSET
    return ACCESS_READ


@dataclass(frozen=True, slots=True)
class _Candidate:
    """A literal whose final access depends on helper roles (parent only)."""

    line: int
    text: str
    callee: str
    index: int | None
    keyword: str | None
    fallback_access: str
    fallback_form: str
    bare_suffix: bool


@dataclass(slots=True)
class _ModuleResult:
    rel: str
    enforced: bool
    constants: dict[str, str]
    imports: dict[str, tuple[str, str]]
    functions: list[_FunctionFacts]
    sites: list[Site]
    candidates: list[_Candidate]
    name_keys: list[tuple[int, str, str, str]]  # line, local name, access, form


def _module_name_to_rel(module: str, root: Path = ROOT) -> str | None:
    parts = module.split(".")
    candidates = [root / "src" / Path(*parts)]
    if parts[0] == "tools" and len(parts) > 1:
        candidates.append(root / "tools" / Path(*parts[1:]))
    candidates.append(root / "tools" / Path(*parts))
    for base in candidates:
        for path in (base.with_suffix(".py"), base / "__init__.py"):
            if path.is_file():
                return _rel(path, root)
    return None


def _is_os_environ(expr: ast.AST) -> bool:
    for node in ast.walk(expr):
        if isinstance(node, ast.Attribute) and node.attr == "environ":
            return True
        if isinstance(node, ast.Name) and node.id == "environ":
            return True
    return False


def _receiver_is_env(expr: ast.AST, env_locals: set[str]) -> bool:
    if isinstance(expr, ast.Name):
        return bool(ENV_RECEIVER.search(expr.id)) or expr.id in env_locals
    if isinstance(expr, ast.Attribute):
        return expr.attr == "environ" or bool(ENV_RECEIVER.search(expr.attr))
    if isinstance(expr, (ast.IfExp, ast.BoolOp)):
        return any(
            _receiver_is_env(part, env_locals)
            for part in ast.iter_child_nodes(expr)
            if isinstance(part, ast.expr)
        )
    return _is_os_environ(expr)


def _env_value(expr: ast.AST, known: set[str]) -> bool:
    for sub in ast.walk(expr):
        if isinstance(sub, ast.Name) and (
            ENV_RECEIVER.search(sub.id) or sub.id in known
        ):
            return True
        if isinstance(sub, ast.Attribute) and (
            sub.attr == "environ" or ENV_RECEIVER.search(sub.attr)
        ):
            return True
    return False


def mention_sites(
    rel: str,
    line: int,
    text: str,
    language: str,
    *,
    enforced: bool,
    form: str = "string",
) -> list[Site]:
    """One mention site per ``MOLT_*`` token; env-shaped mentions are marked."""

    shaped = {
        group
        for match in ENV_SHAPED.finditer(text)
        for group in match.groups()
        if group
    }
    return [
        Site(
            rel,
            line,
            name,
            ACCESS_MENTION,
            FORM_ENV_MENTION if name in shaped else form,
            language,
            enforced,
        )
        for name in TOKEN.findall(text)
    ]


def _interpolated_after_underscore(
    joined: ast.JoinedStr, part: ast.FormattedValue
) -> bool:
    """``f"{stem}_{part}"``: the value is a name suffix, not arbitrary text."""

    index = joined.values.index(part)
    if index == 0:
        return False
    before = joined.values[index - 1]
    return (
        isinstance(before, ast.Constant)
        and isinstance(before.value, str)
        and before.value.endswith("_")
    )


def _function_params(node: ast.FunctionDef | ast.AsyncFunctionDef) -> tuple[str, ...]:
    args = node.args
    names = [a.arg for a in (*args.posonlyargs, *args.args)]
    names.extend(a.arg for a in args.kwonlyargs)
    return tuple(names)


def _callee_name(call: ast.Call) -> str:
    func = call.func
    if isinstance(func, ast.Attribute):
        return func.attr
    if isinstance(func, ast.Name):
        return func.id
    return "<call>"


def _definite_access(
    node: ast.AST, parent: ast.AST, env_locals: set[str]
) -> tuple[str, str] | None:
    """``(access, form)`` when ``node`` is the key of a definite env access."""

    if isinstance(parent, ast.Subscript) and parent.slice is node:
        if not _receiver_is_env(parent.value, env_locals):
            return None
        if isinstance(parent.ctx, ast.Store):
            return ACCESS_SET, "subscript-store"
        if isinstance(parent.ctx, ast.Del):
            return ACCESS_UNSET, "subscript-del"
        return ACCESS_READ, "subscript"
    if isinstance(parent, ast.Call) and parent.args and parent.args[0] is node:
        callee = _callee_name(parent)
        if callee in PY_ENV_FUNCTIONS:
            return PY_ENV_FUNCTIONS[callee], f"call:{callee}"
        if isinstance(parent.func, ast.Attribute) and _receiver_is_env(
            parent.func.value, env_locals
        ):
            if callee in PY_READ_METHODS:
                return ACCESS_READ, f"call:{callee}"
            if callee in PY_SET_METHODS:
                return ACCESS_SET, f"call:{callee}"
            if callee in PY_UNSET_METHODS:
                return ACCESS_UNSET, f"call:{callee}"
        return None
    if (
        isinstance(parent, ast.Compare)
        and parent.left is node
        and len(parent.comparators) == 1
    ):
        if any(isinstance(op, (ast.In, ast.NotIn)) for op in parent.ops):
            if _receiver_is_env(parent.comparators[0], env_locals):
                return ACCESS_READ, "in"
    return None


def _dict_literal_is_set(parents: list[ast.AST]) -> bool:
    """A dict literal that flows into an environment is a set site."""

    if len(parents) < 2:
        return False
    owner = parents[-2]
    dict_node = parents[-1]
    if isinstance(owner, ast.keyword) and owner.arg in {"env", "environment"}:
        return True
    if isinstance(owner, ast.Call) and isinstance(owner.func, ast.Attribute):
        if owner.func.attr == "update" and _receiver_is_env(owner.func.value, set()):
            return True
    if isinstance(owner, ast.Assign) and owner.value is dict_node:
        return any(
            isinstance(t, ast.Name) and ENV_RECEIVER.search(t.id) for t in owner.targets
        )
    if isinstance(owner, ast.Dict) and len(parents) >= 3:
        return _dict_literal_is_set(parents[:-1])
    return False


def _call_position(
    call: ast.Call, arg: ast.AST
) -> tuple[int | None, str | None] | None:
    for index, candidate in enumerate(call.args):
        if candidate is arg:
            return index, None
    for keyword in call.keywords:
        if keyword.value is arg and keyword.arg:
            return None, keyword.arg
    return None


class _ModuleScanner:
    """Two walks over one parsed module; produces picklable facts and sites."""

    def __init__(self, rel: str, tree: ast.Module, *, enforced: bool) -> None:
        self.rel = rel
        self.tree = tree
        self.enforced = enforced
        self.constants: dict[str, str] = {}
        self.imports: dict[str, tuple[str, str]] = {}
        self.env_locals: dict[int, set[str]] = defaultdict(set)
        self.functions: list[_FunctionFacts] = []
        self.sites: list[Site] = []
        self.candidates: list[_Candidate] = []
        self.name_keys: list[tuple[int, str, str, str]] = []

    # -- walk A: constants, imports, env locals, function facts -------------

    def facts(self) -> None:
        for node in self.tree.body:
            if isinstance(node, ast.Assign) and len(node.targets) == 1:
                target = node.targets[0]
                if isinstance(target, ast.Name) and isinstance(
                    node.value, ast.Constant
                ):
                    if isinstance(node.value.value, str) and EXACT.match(
                        node.value.value
                    ):
                        self.constants[target.id] = node.value.value
            elif isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name):
                if isinstance(node.value, ast.Constant) and isinstance(
                    node.value.value, str
                ):
                    if EXACT.match(node.value.value):
                        self.constants[node.target.id] = node.value.value
            elif isinstance(node, ast.ImportFrom) and node.module and node.level == 0:
                for alias in node.names:
                    self.imports[alias.asname or alias.name] = (node.module, alias.name)
        self._facts_scope(self.tree, None)

    def _facts_scope(self, scope: ast.AST, facts: _FunctionFacts | None) -> None:
        """Collect env locals, key params and edges for one scope (not nested defs)."""

        known = self.env_locals[id(scope)]
        loops: dict[str, str] = {}
        params = facts.params if facts else ()
        nested: list[ast.FunctionDef | ast.AsyncFunctionDef] = []

        def visit(node: ast.AST) -> None:
            for child in ast.iter_child_nodes(node):
                if isinstance(child, (ast.FunctionDef, ast.AsyncFunctionDef)):
                    nested.append(child)
                    continue
                if (
                    isinstance(child, ast.Assign)
                    and len(child.targets) == 1
                    and isinstance(child.targets[0], ast.Name)
                ):
                    target = child.targets[0].id
                    if ENV_RECEIVER.search(target) or _env_value(child.value, known):
                        known.add(target)
                elif (
                    isinstance(child, ast.AnnAssign)
                    and isinstance(child.target, ast.Name)
                    and child.value is not None
                ):
                    if _env_value(child.value, known):
                        known.add(child.target.id)
                elif isinstance(child, (ast.For, ast.comprehension)):
                    if (
                        isinstance(child.target, ast.Name)
                        and isinstance(child.iter, ast.Name)
                        and child.iter.id in params
                    ):
                        loops[child.target.id] = child.iter.id
                if facts is not None:
                    if isinstance(child, ast.Name):
                        access = _definite_access(child, node, known)
                        if access is not None:
                            if child.id in params:
                                facts.key_params.setdefault(child.id, set()).add(
                                    access[0]
                                )
                            if child.id in loops:
                                facts.key_list_params.add(loops[child.id])
                    elif isinstance(child, ast.Call):
                        callee = _callee_name(child)
                        for index, arg in enumerate(child.args):
                            if isinstance(arg, ast.Name) and (
                                arg.id in params or arg.id in loops
                            ):
                                facts.edges.append(
                                    _CallEdge(
                                        callee,
                                        index,
                                        None,
                                        arg.id if arg.id in params else None,
                                        loops.get(arg.id),
                                    )
                                )
                        for keyword in child.keywords:
                            arg = keyword.value
                            if (
                                isinstance(arg, ast.Name)
                                and keyword.arg
                                and (arg.id in params or arg.id in loops)
                            ):
                                facts.edges.append(
                                    _CallEdge(
                                        callee,
                                        None,
                                        keyword.arg,
                                        arg.id if arg.id in params else None,
                                        loops.get(arg.id),
                                    )
                                )
                    elif isinstance(child, ast.JoinedStr):
                        for part in child.values:
                            if isinstance(part, ast.FormattedValue) and isinstance(
                                part.value, ast.Name
                            ):
                                source = loops.get(part.value.id)
                                if source is not None:
                                    facts.suffix_list_params.add(source)
                                elif (
                                    part.value.id in params
                                    and _interpolated_after_underscore(child, part)
                                ):
                                    facts.suffix_list_params.add(part.value.id)
                visit(child)

        visit(scope)
        for child in nested:
            child_facts = _FunctionFacts(
                child.name, _function_params(child), {}, set(), set(), []
            )
            self.functions.append(child_facts)
            self._facts_scope(child, child_facts)

    # -- walk B: sites ------------------------------------------------------

    def sites_walk(self) -> None:
        parents: list[ast.AST] = []
        scopes: list[ast.AST] = [self.tree]

        def add(line: int, name: str, access: str, form: str) -> None:
            self.sites.append(
                Site(self.rel, line, name, access, form, "python", self.enforced)
            )

        def mentions(line: int, text: str) -> None:
            self.sites.extend(
                mention_sites(self.rel, line, text, "python", enforced=self.enforced)
            )

        def known() -> set[str]:
            return self.env_locals.get(id(scopes[-1]), set())

        def candidate(
            node: ast.AST,
            line: int,
            text: str,
            call: ast.Call,
            arg: ast.AST,
            fallback: tuple[str, str],
            *,
            bare: bool = False,
        ) -> bool:
            position = _call_position(call, arg)
            if position is None:
                return False
            self.candidates.append(
                _Candidate(
                    line,
                    text,
                    _callee_name(call),
                    position[0],
                    position[1],
                    fallback[0],
                    fallback[1],
                    bare,
                )
            )
            return True

        def classify(node: ast.Constant, line: int, text: str) -> None:
            parent = parents[-1]
            grand = parents[-2] if len(parents) > 1 else None
            definite = _definite_access(node, parent, known())
            if definite is not None:
                add(line, text, *definite)
                return
            if isinstance(parent, ast.Call):
                if (
                    parent.args
                    and parent.args[0] is node
                    and _callee_name(parent) == "startswith"
                ):
                    add(line, text, ACCESS_PREFIX, "call:startswith")
                    return
                if candidate(
                    node,
                    line,
                    text,
                    parent,
                    node,
                    (ACCESS_REFERENCE, f"call:{_callee_name(parent)}"),
                ):
                    return
                add(line, text, ACCESS_REFERENCE, f"call:{_callee_name(parent)}")
                return
            if isinstance(parent, ast.keyword) and isinstance(grand, ast.Call):
                if candidate(
                    node,
                    line,
                    text,
                    grand,
                    parent.value,
                    (ACCESS_REFERENCE, f"call-kw:{parent.arg}:{_callee_name(grand)}"),
                ):
                    return
                add(
                    line,
                    text,
                    ACCESS_REFERENCE,
                    f"call-kw:{parent.arg}:{_callee_name(grand)}",
                )
                return
            if isinstance(parent, (ast.Tuple, ast.List, ast.Set)):
                if isinstance(grand, ast.Call) and candidate(
                    node,
                    line,
                    text,
                    grand,
                    parent,
                    (ACCESS_REFERENCE, "collection:Call"),
                ):
                    return
                if (
                    isinstance(grand, ast.keyword)
                    and len(parents) > 2
                    and isinstance(parents[-3], ast.Call)
                ):
                    if candidate(
                        node,
                        line,
                        text,
                        parents[-3],
                        grand.value,
                        (ACCESS_REFERENCE, f"collection-kw:{grand.arg}"),
                    ):
                        return
                if isinstance(grand, (ast.For, ast.comprehension)):
                    add(line, text, ACCESS_REFERENCE, "collection:iterated")
                    return
                add(
                    line,
                    text,
                    ACCESS_REFERENCE,
                    f"collection:{type(grand).__name__ if grand else 'module'}",
                )
                return
            if isinstance(parent, ast.Dict):
                if any(key is node for key in parent.keys):
                    add(
                        line,
                        text,
                        ACCESS_SET
                        if _dict_literal_is_set(parents)
                        else ACCESS_REFERENCE,
                        "dict-key",
                    )
                else:
                    add(line, text, ACCESS_REFERENCE, "dict-value")
                return
            if isinstance(parent, ast.Compare):
                add(line, text, ACCESS_REFERENCE, "compare")
                return
            if isinstance(parent, (ast.Assign, ast.AnnAssign)):
                add(line, text, ACCESS_REFERENCE, "constant")
                return
            if isinstance(parent, ast.Expr):
                add(line, text, ACCESS_MENTION, "docstring")
                return
            add(line, text, ACCESS_REFERENCE, f"other:{type(parent).__name__}")

        def visit(node: ast.AST) -> None:
            parent = parents[-1]
            line = getattr(node, "lineno", 0)
            if isinstance(node, ast.Constant) and isinstance(node.value, str):
                text = node.value
                if PREFIX_LITERAL.match(text):
                    add(line, text, ACCESS_PREFIX, "prefix-literal")
                elif EXACT.match(text):
                    classify(node, line, text)
                elif BARE_SUFFIX.match(text):
                    if (
                        isinstance(parent, (ast.Tuple, ast.List))
                        and len(parents) > 1
                        and isinstance(parents[-2], ast.Call)
                    ):
                        candidate(
                            node,
                            line,
                            f"_{text}",
                            parents[-2],
                            parent,
                            (ACCESS_REFERENCE, "bare"),
                            bare=True,
                        )
                    elif isinstance(parent, ast.Call):
                        candidate(
                            node,
                            line,
                            f"_{text}",
                            parent,
                            node,
                            (ACCESS_REFERENCE, "bare"),
                            bare=True,
                        )
                    elif (
                        isinstance(parent, ast.keyword)
                        and len(parents) > 1
                        and isinstance(parents[-2], ast.Call)
                    ):
                        candidate(
                            node,
                            line,
                            f"_{text}",
                            parents[-2],
                            parent.value,
                            (ACCESS_REFERENCE, "bare"),
                            bare=True,
                        )
                elif "MOLT_" in text:
                    mentions(line, text)
                return
            if isinstance(node, ast.JoinedStr):
                values = node.values
                for index, part in enumerate(values):
                    if isinstance(part, ast.Constant) and isinstance(part.value, str):
                        text = part.value
                        export = ENV_LINE.match(text) if index == 0 else None
                        if export:
                            # f"<NAME>={value}": an export line for GITHUB_ENV or a shell.
                            add(line, export.group(1), ACCESS_SET, "env-line")
                        elif index == 0 and PREFIX_LITERAL.match(text):
                            add(line, text, ACCESS_PREFIX, "f-string")
                        elif (
                            index > 0
                            and index == len(values) - 1
                            and SUFFIX_LITERAL.match(text)
                        ):
                            add(line, text, ACCESS_SUFFIX, "f-string")
                        elif "MOLT_" in text:
                            mentions(line, text)
                return
            if isinstance(node, ast.Name):
                access = _definite_access(node, parent, known())
                if access is not None:
                    self.name_keys.append((line, node.id, access[0], access[1]))

        def walk(node: ast.AST) -> None:
            for child in ast.iter_child_nodes(node):
                parents.append(node)
                is_scope = isinstance(child, (ast.FunctionDef, ast.AsyncFunctionDef))
                if is_scope:
                    scopes.append(child)
                try:
                    visit(child)
                    if isinstance(child, ast.JoinedStr):
                        for part in child.values:
                            if isinstance(part, ast.FormattedValue):
                                parents.append(child)
                                try:
                                    visit(part.value)
                                    walk(part.value)
                                finally:
                                    parents.pop()
                    else:
                        walk(child)
                finally:
                    if is_scope:
                        scopes.pop()
                    parents.pop()

        walk(self.tree)

    def result(self) -> _ModuleResult:
        return _ModuleResult(
            rel=self.rel,
            enforced=self.enforced,
            constants=self.constants,
            imports=self.imports,
            functions=self.functions,
            sites=self.sites,
            candidates=self.candidates,
            name_keys=self.name_keys,
        )


def _comment_mentions(text: str, rel: str, *, enforced: bool) -> list[Site]:
    sites: list[Site] = []
    try:
        for token in tokenize.generate_tokens(io.StringIO(text).readline):
            if token.type == tokenize.COMMENT and "MOLT_" in token.string:
                sites.extend(
                    mention_sites(
                        rel,
                        token.start[0],
                        token.string,
                        "python",
                        enforced=enforced,
                        form="comment",
                    )
                )
    except (tokenize.TokenError, SyntaxError):
        return sites
    return sites


def _scan_python_file(job: tuple[str, bool, str]) -> _ModuleResult | None:
    """Worker entry: parse and scan one module (``rel``, ``enforced``, ``root``)."""

    rel, enforced, root = job
    path = Path(root) / rel
    try:
        text = path.read_text(encoding="utf-8")
    except UnicodeDecodeError:
        return None
    if not WORTH_PARSING.search(text):
        return None
    try:
        tree = ast.parse(text, filename=rel)
    except (SyntaxError, ValueError):
        return None
    scanner = _ModuleScanner(rel, tree, enforced=enforced)
    scanner.facts()
    scanner.sites_walk()
    result = scanner.result()
    result.sites.extend(_comment_mentions(text, rel, enforced=enforced))
    return result


@dataclass(slots=True)
class _HelperRoles:
    key_by_index: dict[int, set[str]]
    key_by_name: dict[str, set[str]]
    list_index: set[int]
    list_name: set[str]
    suffix_index: set[int]
    suffix_name: set[str]

    def role(
        self, index: int | None, keyword: str | None
    ) -> tuple[set[str], bool, bool]:
        key = (
            self.key_by_index.get(index)
            if index is not None
            else self.key_by_name.get(keyword)
        )
        is_list = index in self.list_index or keyword in self.list_name
        is_suffix = index in self.suffix_index or keyword in self.suffix_name
        return set(key or ()), is_list, is_suffix


def _helper_fixpoint(results: list[_ModuleResult]) -> dict[str, _HelperRoles]:
    """Propagate env-key roles through call edges until nothing changes."""

    table: dict[str, list[_FunctionFacts]] = defaultdict(list)
    for result in results:
        for facts in result.functions:
            table[facts.name].append(facts)

    def roles(name: str) -> _HelperRoles:
        merged = _HelperRoles({}, {}, set(), set(), set(), set())
        for facts in table.get(name, ()):
            for index, param in enumerate(facts.params):
                if param in facts.key_params:
                    merged.key_by_index.setdefault(index, set()).update(
                        facts.key_params[param]
                    )
                    merged.key_by_name.setdefault(param, set()).update(
                        facts.key_params[param]
                    )
                if param in facts.key_list_params:
                    merged.list_index.add(index)
                    merged.list_name.add(param)
                if param in facts.suffix_list_params:
                    merged.suffix_index.add(index)
                    merged.suffix_name.add(param)
        return merged

    changed = True
    while changed:
        changed = False
        # Roles are read once per iteration; growth is picked up next round.
        cache: dict[str, _HelperRoles] = {}
        for result in results:
            for facts in result.functions:
                for edge in facts.edges:
                    if edge.callee in _GENERIC_CALLEES:
                        continue
                    merged = cache.get(edge.callee)
                    if merged is None:
                        merged = cache[edge.callee] = roles(edge.callee)
                    key, is_list, is_suffix = merged.role(edge.index, edge.keyword)
                    if key:
                        if edge.arg_param:
                            current = facts.key_params.setdefault(edge.arg_param, set())
                            if not key <= current:
                                current.update(key)
                                changed = True
                        if (
                            edge.arg_loop_param
                            and edge.arg_loop_param not in facts.key_list_params
                        ):
                            facts.key_list_params.add(edge.arg_loop_param)
                            changed = True
                    if (
                        is_list
                        and edge.arg_param
                        and edge.arg_param not in facts.key_list_params
                    ):
                        facts.key_list_params.add(edge.arg_param)
                        changed = True
                    if (
                        is_suffix
                        and edge.arg_param
                        and edge.arg_param not in facts.suffix_list_params
                    ):
                        facts.suffix_list_params.add(edge.arg_param)
                        changed = True
    helpers: dict[str, _HelperRoles] = {}
    for name in table:
        if name in _GENERIC_CALLEES:
            continue
        merged = roles(name)
        if merged.key_by_index or merged.list_index or merged.suffix_index:
            helpers[name] = merged
    return helpers


def _finish_results(
    results: list[_ModuleResult], root: Path = ROOT
) -> tuple[list[Site], list[Site]]:
    """Resolve helper candidates and ``Name`` keys once helper roles are known."""

    helpers = _helper_fixpoint(results)
    by_rel = {result.rel: result for result in results}
    sites: list[Site] = []
    unresolved: list[Site] = []
    for result in results:
        sites.extend(result.sites)
        for item in result.candidates:
            helper = helpers.get(item.callee)
            key, is_list, is_suffix = (
                helper.role(item.index, item.keyword)
                if helper
                else (set(), False, False)
            )
            if item.bare_suffix:
                if is_suffix:
                    sites.append(
                        Site(
                            result.rel,
                            item.line,
                            item.text,
                            ACCESS_SUFFIX,
                            f"helper:{item.callee}",
                            "python",
                            result.enforced,
                        )
                    )
                continue
            language = "rust" if result.rel.endswith(".rs") else "python"
            if key:
                sites.append(
                    Site(
                        result.rel,
                        item.line,
                        item.text,
                        _label(key),
                        f"helper:{item.callee}",
                        language,
                        result.enforced,
                    )
                )
            elif is_list:
                sites.append(
                    Site(
                        result.rel,
                        item.line,
                        item.text,
                        ACCESS_READ,
                        f"helper:{item.callee}",
                        language,
                        result.enforced,
                    )
                )
            elif is_suffix:
                sites.append(
                    Site(
                        result.rel,
                        item.line,
                        item.text,
                        ACCESS_SUFFIX,
                        f"helper:{item.callee}",
                        language,
                        result.enforced,
                    )
                )
            else:
                sites.append(
                    Site(
                        result.rel,
                        item.line,
                        item.text,
                        item.fallback_access,
                        item.fallback_form,
                        language,
                        result.enforced,
                    )
                )
        for line, local, access, form in result.name_keys:
            resolved = result.constants.get(local)
            if resolved is None and local in result.imports:
                module, remote = result.imports[local]
                target = _module_name_to_rel(module, root)
                if target in by_rel:
                    resolved = by_rel[target].constants.get(remote)
            if resolved is None:
                unresolved.append(
                    Site(
                        result.rel,
                        line,
                        local,
                        access,
                        f"dynamic:{form}",
                        "python",
                        result.enforced,
                    )
                )
            else:
                sites.append(
                    Site(
                        result.rel,
                        line,
                        resolved,
                        access,
                        f"{form}:{local}",
                        "python",
                        result.enforced,
                    )
                )
    return sites, unresolved


# --------------------------------------------------------------------------
# Rust scanner
# --------------------------------------------------------------------------
#
# A small lexer (comments skipped, strings and lifetimes recognised) feeds two
# analyses per file: function facts (which parameters reach ``env::var`` and
# friends, directly or through another helper) and literal sites. The parent
# runs the same call-edge fixpoint as for Python, then classifies the
# literals that depend on helper roles.

RUST_READ_CALLS = frozenset({"var", "var_os", "option_env!", "env!", "required_env"})
RUST_SET_CALLS = frozenset({"set_var", "env"})
RUST_UNSET_CALLS = frozenset({"remove_var", "env_remove"})
_RUST_PUNCT2 = frozenset(
    {"::", "->", "=>", "..", "&&", "||", "==", "!=", "<=", ">=", "+=", "-="}
)
_RUST_CLOSURE_ADAPTERS = frozenset(
    {"find_map", "filter_map", "map", "any", "all", "for_each", "find", "filter"}
)


@dataclass(frozen=True, slots=True)
class _Tok:
    kind: str  # ident | punct | string | number | lifetime
    text: str
    line: int
    offset: int


def _rust_lex(source: str) -> tuple[list[_Tok], list[tuple[int, str]]]:
    """Return ``(tokens, comments)``; comments are ``(line, text)``."""

    tokens: list[_Tok] = []
    comments: list[tuple[int, str]] = []
    i = 0
    n = len(source)
    line = 1
    while i < n:
        ch = source[i]
        if ch == "\n":
            line += 1
            i += 1
            continue
        if ch in " \t\r":
            i += 1
            continue
        if source.startswith("//", i):
            end = source.find("\n", i)
            end = n if end == -1 else end
            comments.append((line, source[i:end]))
            i = end
            continue
        if source.startswith("/*", i):
            depth = 1
            j = i + 2
            start_line = line
            while j < n and depth:
                if source.startswith("/*", j):
                    depth += 1
                    j += 2
                elif source.startswith("*/", j):
                    depth -= 1
                    j += 2
                else:
                    if source[j] == "\n":
                        line += 1
                    j += 1
            comments.append((start_line, source[i:j]))
            i = j
            continue
        if ch == "'":
            if i + 2 < n and (source[i + 2] == "'" or source[i + 1] == "\\"):
                j = source.find("'", i + 2)
                if j != -1 and j - i <= 6:
                    i = j + 1
                    continue
            j = i + 1
            while j < n and (source[j].isalnum() or source[j] == "_"):
                j += 1
            tokens.append(_Tok("lifetime", source[i:j], line, i))
            i = j
            continue
        if ch == "r" or (ch == "b" and i + 1 < n and source[i + 1] in 'r"'):
            j = i + (2 if ch == "b" and source[i + 1] == "r" else 1)
            if ch == "b" and source[i + 1] == '"':
                j = i + 1
            hashes = 0
            while j < n and source[j] == "#":
                hashes += 1
                j += 1
            word_before = i > 0 and (source[i - 1].isalnum() or source[i - 1] == "_")
            if (
                j < n
                and source[j] == '"'
                and not word_before
                and (ch != "b" or source[i + 1] == "r" or hashes == 0)
            ):
                if ch == "b" and source[i + 1] == '"':
                    # byte string: treat like a normal string
                    pass
                else:
                    start_line = line
                    closer = '"' + "#" * hashes
                    k = source.find(closer, j + 1)
                    k = n if k == -1 else k
                    body = source[j + 1 : k]
                    tokens.append(_Tok("string", body, start_line, j))
                    line += body.count("\n")
                    i = k + len(closer)
                    continue
        if ch == '"' or (ch == "b" and i + 1 < n and source[i + 1] == '"'):
            if ch == "b":
                i += 1
            j = i + 1
            start_line = line
            buf: list[str] = []
            while j < n:
                c = source[j]
                if c == "\\":
                    buf.append(source[j : j + 2])
                    j += 2
                    continue
                if c == '"':
                    break
                if c == "\n":
                    line += 1
                buf.append(c)
                j += 1
            tokens.append(_Tok("string", "".join(buf), start_line, i))
            i = j + 1
            continue
        if ch.isalpha() or ch == "_":
            j = i + 1
            while j < n and (source[j].isalnum() or source[j] == "_"):
                j += 1
            text = source[i:j]
            if j < n and source[j] == "!" and (j + 1 >= n or source[j + 1] != "="):
                text += "!"
                j += 1
            tokens.append(_Tok("ident", text, line, i))
            i = j
            continue
        if ch.isdigit():
            j = i + 1
            while j < n and (source[j].isalnum() or source[j] in "_."):
                j += 1
            tokens.append(_Tok("number", source[i:j], line, i))
            i = j
            continue
        two = source[i : i + 2]
        if two in _RUST_PUNCT2:
            tokens.append(_Tok("punct", two, line, i))
            i += 2
            continue
        tokens.append(_Tok("punct", ch, line, i))
        i += 1
    return tokens, comments


def _matching(tokens: list[_Tok], start: int, open_text: str, close_text: str) -> int:
    """Index of the token closing the group opened at ``start`` (or len)."""

    depth = 0
    for index in range(start, len(tokens)):
        tok = tokens[index]
        if tok.kind != "punct":
            continue
        if tok.text == open_text:
            depth += 1
        elif tok.text == close_text:
            depth -= 1
            if depth == 0:
                return index
    return len(tokens)


def _rust_param_names(tokens: list[_Tok], start: int, end: int) -> tuple[str, ...]:
    """Parameter names of ``fn f(<start+1> .. <end>)``.

    A ``self`` receiver is dropped so method-call argument positions line up.
    """

    names: list[str] = []
    depth = 0
    piece: list[_Tok] = []
    for tok in tokens[start + 1 : end]:
        if tok.kind == "punct" and tok.text in "([<":
            depth += 1
        elif tok.kind == "punct" and tok.text in ")]>":
            depth -= 1
        if tok.kind == "punct" and tok.text == "," and depth == 0:
            name = _rust_param_name(piece)
            if name != "self":
                names.append(name)
            piece = []
            continue
        piece.append(tok)
    if piece:
        name = _rust_param_name(piece)
        if name != "self":
            names.append(name)
    return tuple(names)


def _rust_param_name(piece: list[_Tok]) -> str:
    for tok in piece:
        if tok.kind == "ident" and tok.text == "self":
            return "self"
        if tok.kind == "ident" and tok.text not in {"mut", "ref"}:
            return tok.text
    return ""


def _rust_key_access(tokens: list[_Tok], index: int) -> tuple[str, str] | None:
    """Access when ``tokens[index]`` is the first argument of an env call."""

    if index < 2:
        return None
    if tokens[index - 1].kind != "punct" or tokens[index - 1].text != "(":
        return None
    callee = tokens[index - 2]
    if callee.kind != "ident":
        return None
    if callee.text in RUST_READ_CALLS:
        return ACCESS_READ, f"call:{callee.text}"
    if callee.text in RUST_SET_CALLS:
        return ACCESS_SET, f"call:{callee.text}"
    if callee.text in RUST_UNSET_CALLS:
        return ACCESS_UNSET, f"call:{callee.text}"
    return None


def _rust_callee_at(tokens: list[_Tok], open_index: int) -> str:
    """Callee ident for the ``(`` at ``open_index`` (``""`` for non-calls)."""

    if open_index >= 1 and tokens[open_index - 1].kind == "ident":
        return tokens[open_index - 1].text
    return ""


def _rust_arg_index(tokens: list[_Tok], open_index: int, target: int) -> int | None:
    """Positional index of the argument starting at ``target`` inside the call."""

    depth = 0
    index = 0
    for pos in range(open_index + 1, len(tokens)):
        if pos == target:
            return index if depth == 0 else None
        tok = tokens[pos]
        if tok.kind == "punct" and tok.text in "([{":
            depth += 1
        elif tok.kind == "punct" and tok.text in ")]}":
            if depth == 0:
                return None
            depth -= 1
        elif tok.kind == "punct" and tok.text == "," and depth == 0:
            index += 1
    return None


def _enclosing_call(tokens: list[_Tok], target: int, limit: int) -> int | None:
    """Index of the ``(`` whose argument list directly holds ``target``."""

    depth = 0
    pos = target - 1
    while pos >= limit:
        tok = tokens[pos]
        if tok.kind == "punct" and tok.text in ")]}":
            depth += 1
        elif tok.kind == "punct" and tok.text in "([{":
            if depth == 0:
                return pos if tok.text == "(" else None
            depth -= 1
        pos -= 1
    return None


def _scan_rust_file(job: tuple[str, str]) -> _ModuleResult | None:
    rel, root = job
    path = Path(root) / rel
    try:
        source = path.read_text(encoding="utf-8")
    except UnicodeDecodeError:
        return None
    if not WORTH_PARSING.search(source):
        return None
    enforced = not _is_rust_test_path(rel)
    tokens, comments = _rust_lex(source)
    sites: list[Site] = []
    candidates: list[_Candidate] = []
    functions: list[_FunctionFacts] = []
    for line, text in comments:
        if "MOLT_" in text:
            sites.extend(
                mention_sites(
                    rel, line, text, "rust", enforced=enforced, form="comment"
                )
            )

    # Function facts: params that reach env calls, loops over params, edges.
    spans: list[tuple[int, int, _FunctionFacts]] = []
    index = 0
    while index < len(tokens):
        tok = tokens[index]
        if (
            tok.kind == "ident"
            and tok.text == "fn"
            and index + 2 < len(tokens)
            and tokens[index + 1].kind == "ident"
        ):
            open_index = index + 2
            if tokens[open_index].kind == "punct" and tokens[open_index].text == "<":
                open_index = _matching(tokens, open_index, "<", ">") + 1
            if open_index < len(tokens) and tokens[open_index].text == "(":
                close_index = _matching(tokens, open_index, "(", ")")
                params = _rust_param_names(tokens, open_index, close_index)
                body_open = close_index + 1
                while body_open < len(tokens) and not (
                    tokens[body_open].kind == "punct" and tokens[body_open].text in "{;"
                ):
                    body_open += 1
                if body_open < len(tokens) and tokens[body_open].text == "{":
                    body_close = _matching(tokens, body_open, "{", "}")
                    facts = _FunctionFacts(
                        tokens[index + 1].text, params, {}, set(), set(), []
                    )
                    functions.append(facts)
                    spans.append((body_open, body_close, facts))
                    _rust_function_facts(tokens, body_open, body_close, facts)
                    index = body_open + 1
                    continue
        index += 1

    def facts_for(position: int) -> tuple[_FunctionFacts | None, int]:
        best: tuple[_FunctionFacts | None, int] = (None, 0)
        for open_index, close_index, facts in spans:
            if open_index <= position <= close_index and open_index >= best[1]:
                best = (facts, open_index)
        return best

    for index, tok in enumerate(tokens):
        if tok.kind != "string":
            continue
        text = tok.text
        if PREFIX_LITERAL.match(text):
            sites.append(
                Site(
                    rel,
                    tok.line,
                    text,
                    ACCESS_PREFIX,
                    "prefix-literal",
                    "rust",
                    enforced,
                )
            )
            continue
        if EXACT.match(text):
            access = _rust_key_access(tokens, index)
            if access is not None:
                sites.append(
                    Site(rel, tok.line, text, access[0], access[1], "rust", enforced)
                )
                continue
            facts, limit = facts_for(index)
            call = _enclosing_call(tokens, index, limit)
            if call is not None:
                callee = _rust_callee_at(tokens, call)
                position = _rust_arg_index(tokens, call, index)
                if callee and position is not None:
                    candidates.append(
                        _Candidate(
                            tok.line,
                            text,
                            callee,
                            position,
                            None,
                            ACCESS_REFERENCE,
                            f"call:{callee}",
                            False,
                        )
                    )
                    continue
            # Array literal element: ``[ "A", "B" ]`` passed to a helper, or
            # iterated by a closure that reaches an env call.
            array_open = _enclosing_group(tokens, index, limit, "[", "]")
            if array_open is not None:
                array_close = _matching(tokens, array_open, "[", "]")
                owner = _rust_array_owner(tokens, array_open, array_close, functions)
                if owner is not None:
                    if owner.callee == "<closure>":
                        sites.append(
                            Site(
                                rel,
                                tok.line,
                                text,
                                owner.access,
                                "closure",
                                "rust",
                                enforced,
                            )
                        )
                    else:
                        candidates.append(
                            _Candidate(
                                tok.line,
                                text,
                                owner.callee,
                                owner.position,
                                None,
                                ACCESS_REFERENCE,
                                "array",
                                False,
                            )
                        )
                    continue
            sites.append(
                Site(rel, tok.line, text, ACCESS_REFERENCE, "literal", "rust", enforced)
            )
            continue
        prefix_match = RUST_FORMAT_PREFIX.match(text)
        if prefix_match:
            sites.append(
                Site(
                    rel,
                    tok.line,
                    prefix_match.group(1),
                    ACCESS_PREFIX,
                    "format!",
                    "rust",
                    enforced,
                )
            )
            continue
        suffix_match = RUST_FORMAT_SUFFIX.match(text)
        if suffix_match:
            sites.append(
                Site(
                    rel,
                    tok.line,
                    suffix_match.group(1),
                    ACCESS_SUFFIX,
                    "format!",
                    "rust",
                    enforced,
                )
            )
            continue
        if "MOLT_" in text:
            sites.extend(mention_sites(rel, tok.line, text, "rust", enforced=enforced))
    return _ModuleResult(
        rel=rel,
        enforced=enforced,
        constants={},
        imports={},
        functions=functions,
        sites=sites,
        candidates=candidates,
        name_keys=[],
    )


@dataclass(frozen=True, slots=True)
class _ArrayOwner:
    callee: str  # helper name, or "<closure>"
    position: int | None  # argument index for a helper
    access: str  # access reached through the closure


def _enclosing_group(
    tokens: list[_Tok], target: int, limit: int, open_text: str, close_text: str
) -> int | None:
    depth = 0
    pos = target - 1
    while pos >= limit:
        tok = tokens[pos]
        if tok.kind == "punct" and tok.text in ")]}":
            depth += 1
        elif tok.kind == "punct" and tok.text in "([{":
            if depth == 0:
                return pos if tok.text == open_text else None
            depth -= 1
        pos -= 1
    return None


def _rust_array_owner(
    tokens: list[_Tok],
    array_open: int,
    array_close: int,
    functions: list[_FunctionFacts],
) -> _ArrayOwner | None:
    """How an array literal of names is consumed: a helper call or a closure."""

    # ``helper(&[ ... ])`` / ``helper([ ... ])``: the array is argument i.
    pos = array_open - 1
    if pos >= 0 and tokens[pos].kind == "punct" and tokens[pos].text == "&":
        pos -= 1
    if pos >= 0 and tokens[pos].kind == "punct" and tokens[pos].text in "(,":
        call = _enclosing_call(tokens, array_open, 0)
        if call is not None:
            callee = _rust_callee_at(tokens, call)
            position = _rust_arg_index(
                tokens,
                call,
                array_open if tokens[array_open - 1].text != "&" else array_open - 1,
            )
            if callee and position is not None:
                return _ArrayOwner(callee, position, "")
    # ``[ ... ].iter().find_map(|name| ...)``: the closure parameter reaches an env call.
    pos = array_close + 1
    while (
        pos + 1 < len(tokens)
        and tokens[pos].kind == "punct"
        and tokens[pos].text == "."
    ):
        method = tokens[pos + 1]
        if method.kind != "ident":
            break
        if pos + 2 < len(tokens) and tokens[pos + 2].text == "(":
            call_close = _matching(tokens, pos + 2, "(", ")")
            if (
                method.text in _RUST_CLOSURE_ADAPTERS
                and pos + 5 < len(tokens)
                and tokens[pos + 3].text == "|"
                and tokens[pos + 4].kind == "ident"
                and tokens[pos + 5].text == "|"
            ):
                closure_param = tokens[pos + 4].text
                access = _rust_param_access(
                    tokens, pos + 6, call_close, closure_param, functions
                )
                if access is not None:
                    return _ArrayOwner("<closure>", None, access)
                return None
            pos = call_close + 1
            continue
        break
    return None


def _rust_param_access(
    tokens: list[_Tok], start: int, end: int, name: str, functions: list[_FunctionFacts]
) -> str | None:
    """Access ``name`` reaches within ``tokens[start:end]`` (direct or via a helper)."""

    by_name = {facts.name: facts for facts in functions}
    for index in range(start, end):
        tok = tokens[index]
        if tok.kind != "ident" or tok.text != name:
            continue
        access = _rust_key_access(tokens, index)
        if access is not None:
            return access[0]
        if (
            index >= 2
            and tokens[index - 1].text == "("
            and tokens[index - 2].kind == "ident"
        ):
            helper = by_name.get(tokens[index - 2].text)
            if helper is not None and helper.params:
                role = helper.key_params.get(helper.params[0])
                if role:
                    return _label(role)
    return None


def _rust_function_facts(
    tokens: list[_Tok], body_open: int, body_close: int, facts: _FunctionFacts
) -> None:
    params = set(facts.params)
    loops: dict[str, str] = {}
    index = body_open + 1
    while index < body_close:
        tok = tokens[index]
        if (
            tok.kind == "ident"
            and tok.text == "for"
            and index + 3 < body_close
            and tokens[index + 1].kind == "ident"
            and tokens[index + 2].text == "in"
        ):
            source_index = index + 3
            if tokens[source_index].text == "&":
                source_index += 1
            if (
                tokens[source_index].kind == "ident"
                and tokens[source_index].text in params
            ):
                loops[tokens[index + 1].text] = tokens[source_index].text
        if (
            tok.kind == "ident"
            and tok.text in params
            and index + 2 < body_close
            and tokens[index + 1].text == "."
            and tokens[index + 2].kind == "ident"
        ):
            # ``param.iter().find_map(|x| ...)``
            pos = index + 1
            while (
                pos + 1 < body_close
                and tokens[pos].text == "."
                and tokens[pos + 1].kind == "ident"
            ):
                if pos + 2 < body_close and tokens[pos + 2].text == "(":
                    call_close = _matching(tokens, pos + 2, "(", ")")
                    if (
                        tokens[pos + 1].text in _RUST_CLOSURE_ADAPTERS
                        and pos + 5 < body_close
                        and tokens[pos + 3].text == "|"
                        and tokens[pos + 4].kind == "ident"
                        and tokens[pos + 5].text == "|"
                    ):
                        loops[tokens[pos + 4].text] = tok.text
                        break
                    pos = call_close + 1
                    continue
                break
        if tok.kind == "ident" and (tok.text in params or tok.text in loops):
            access = _rust_key_access(tokens, index)
            if access is not None:
                if tok.text in params:
                    facts.key_params.setdefault(tok.text, set()).add(access[0])
                else:
                    facts.key_list_params.add(loops[tok.text])
            elif index >= 1 and tokens[index - 1].text in "(,":
                call = _enclosing_call(tokens, index, body_open)
                if call is not None:
                    callee = _rust_callee_at(tokens, call)
                    position = _rust_arg_index(tokens, call, index)
                    if callee and position is not None:
                        facts.edges.append(
                            _CallEdge(
                                callee,
                                position,
                                None,
                                tok.text if tok.text in params else None,
                                loops.get(tok.text),
                            )
                        )
        index += 1


# --------------------------------------------------------------------------
# Text scanner (workflows, TOML, shell, Makefiles)
# --------------------------------------------------------------------------

_YAML_ENV_KEY = re.compile(r"^\s*(MOLT_[A-Z0-9_]+)\s*:")
_SHELL_ASSIGN = re.compile(r"(?:^|[\s\"'(;])(?:export\s+)?(MOLT_[A-Z0-9_]+)=")
_STEM_FLAG = re.compile(
    r"--(?:guard-|memory-guard-)?prefix(?:[= ]+)[\"']?(MOLT_[A-Z0-9_]+)"
)
_EXPANSION = re.compile(
    r"\$\{\{\s*(?:env|vars)\.(MOLT_[A-Z0-9_]+)\s*\}\}"
    r"|\$\{(MOLT_[A-Z0-9_]+)\}"
    r"|\$(MOLT_[A-Z0-9_]+)\b"
    r"|%(MOLT_[A-Z0-9_]+)%"
    r"|\$env:(MOLT_[A-Z0-9_]+)"
    r"|process\.env\.(MOLT_[A-Z0-9_]+)"
    r"|process\.env\[\"(MOLT_[A-Z0-9_]+)\"\]"
)


def _classify_text_line(line: str) -> dict[str, tuple[str, str]]:
    """Map each ``MOLT_*`` token on the line to ``(access, form)``."""

    result: dict[str, tuple[str, str]] = {}
    stripped = line.lstrip()
    if stripped.startswith("#") or stripped.startswith("//"):
        shaped = {
            group
            for match in ENV_SHAPED.finditer(line)
            for group in match.groups()
            if group
        }
        for name in TOKEN.findall(line):
            result[name] = (
                ACCESS_MENTION,
                FORM_ENV_MENTION if name in shaped else "comment",
            )
        return result
    for match in _STEM_FLAG.finditer(line):
        result[match.group(1)] = (ACCESS_STEM, "prefix-flag")
    for match in _EXPANSION.finditer(line):
        name = next(group for group in match.groups() if group)
        result.setdefault(name, (ACCESS_READ, "expansion"))
    key = _YAML_ENV_KEY.match(line)
    if key:
        result.setdefault(key.group(1), (ACCESS_SET, "env-map"))
    for match in _SHELL_ASSIGN.finditer(line):
        result.setdefault(match.group(1), (ACCESS_SET, "assignment"))
    for name in TOKEN.findall(line):
        if name.endswith("_"):
            result.setdefault(name, (ACCESS_PREFIX, "text"))
        else:
            result.setdefault(name, (ACCESS_REFERENCE, "text"))
    return result


def scan_text(path: Path, *, enforced: bool, root: Path = ROOT) -> list[Site]:
    sites: list[Site] = []
    rel = _rel(path, root)
    try:
        text = path.read_text(encoding="utf-8")
    except UnicodeDecodeError:
        return sites
    if "MOLT_" not in text:
        return sites
    for number, line in enumerate(text.splitlines(), start=1):
        if "MOLT_" not in line:
            continue
        for name, (access, form) in _classify_text_line(line).items():
            sites.append(Site(rel, number, name, access, form, "text", enforced))
    return sites


def _text_paths(root: Path = ROOT) -> Iterator[tuple[Path, bool]]:
    for path in _iter_files(ENFORCED_TEXT_ROOTS, TEXT_SUFFIXES, root):
        yield path, True
    for name in ENFORCED_TEXT_FILES:
        path = root / name
        if path.is_file():
            yield path, True
    for path in _iter_files(EVIDENCE_TEXT_ROOTS, TEXT_SUFFIXES, root):
        yield path, False


def scan_docs(root: Path = ROOT) -> list[Site]:
    """Top-level docs are enforced for env-shaped mentions; history for retired names."""

    sites: list[Site] = []
    enforced_paths = {_rel(p, root) for p in sorted(root.glob(DOC_ENFORCED_GLOB))}
    enforced_paths.update(name for name in DOC_FILES if (root / name).is_file())
    paths = list(_iter_files(DOC_HISTORY_ROOTS, frozenset({".md"}), root))
    paths.extend(root / name for name in DOC_FILES if (root / name).is_file())
    for path in paths:
        rel = _rel(path, root)
        if rel in DOC_SKIP_FILES or rel.endswith(".generated.md"):
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            continue
        if "MOLT_" not in text:
            continue
        enforced = rel in enforced_paths
        for number, line in enumerate(text.splitlines(), start=1):
            if "MOLT_" in line:
                sites.extend(
                    mention_sites(
                        rel, number, line, "doc", enforced=enforced, form="doc"
                    )
                )
    return sites


# --------------------------------------------------------------------------
# Whole scan
# --------------------------------------------------------------------------


def _harness_modules(root: Path = ROOT) -> frozenset[str]:
    """Modules under ``tests/`` the proof plan declares as executable authorities.

    ``tools/proof_plan.toml`` ``authority_inputs`` is the repository's own list
    of executable authorities; the entries under ``tests/`` that are not
    ``test_*.py`` modules (``tests/molt_diff.py``, the conformance runners,
    the process guards) are harness code: first-party readers, enforced. The
    rest of ``tests/`` is reader evidence only.
    """

    plan_path = root / "tools" / "proof_plan.toml"
    if not plan_path.is_file():
        return frozenset()
    with plan_path.open("rb") as handle:
        plan = tomllib.load(handle)
    return frozenset(
        entry
        for entry in plan.get("authority_inputs", ())
        if entry.startswith("tests/")
        and not entry.rsplit("/", 1)[-1].startswith("test_")
    )


def _python_jobs(root: Path = ROOT) -> list[tuple[str, bool, str]]:
    jobs: list[tuple[str, bool, str]] = []
    for entry in ENFORCED_PYTHON_ROOTS:
        jobs.extend(
            (_rel(path, root), True, str(root))
            for path in _iter_files((entry,), frozenset({".py"}), root)
            if _rel(path, root) != REGISTRY_PROJECTION
        )
    harness = _harness_modules(root)
    for entry in EVIDENCE_PYTHON_ROOTS:
        for path in _iter_files((entry,), frozenset({".py"}), root):
            rel = _rel(path, root)
            jobs.append((rel, rel in harness, str(root)))
    return jobs


def scan_repository(*, workers: int | None = None, root: Path = ROOT) -> Scan:
    """Scan every root. Python and Rust files are parsed in a process pool."""

    jobs = _python_jobs(root)
    rust_paths = [
        (_rel(path, root), str(root))
        for path in _iter_files(ENFORCED_RUST_ROOTS, frozenset({".rs"}), root)
    ]
    worker_count = (
        workers if workers is not None else max(1, min(8, os.cpu_count() or 1))
    )
    results: list[_ModuleResult] = []
    if worker_count == 1:
        results.extend(r for r in map(_scan_python_file, jobs) if r is not None)
        results.extend(r for r in map(_scan_rust_file, rust_paths) if r is not None)
    else:
        with ProcessPoolExecutor(max_workers=worker_count) as pool:
            for result in pool.map(_scan_python_file, jobs, chunksize=16):
                if result is not None:
                    results.append(result)
            for result in pool.map(_scan_rust_file, rust_paths, chunksize=16):
                if result is not None:
                    results.append(result)
    sites, unresolved = _finish_results(results)
    for path, enforced in _text_paths(root):
        sites.extend(scan_text(path, enforced=enforced, root=root))
    sites.extend(scan_docs(root))
    sites.sort(key=lambda s: (s.path, s.line, s.name, s.access))
    return Scan(sites=tuple(sites), unresolved=tuple(unresolved))


# --------------------------------------------------------------------------
# Checks
# --------------------------------------------------------------------------


def _registered_prefix(registry: EnvironmentRegistry, prefix: str) -> bool:
    if any(family.prefix.startswith(prefix) for family in registry.prefix_families):
        return True
    return any(name.startswith(prefix) for name in registry.registered_names())


def check(
    registry: EnvironmentRegistry, scan: Scan, root: Path = ROOT
) -> list[Violation]:
    violations: list[Violation] = []
    by_name = scan.by_name()
    retired_suffixes = {row.suffix: row for row in registry.retired_suffixes}

    for site in scan.sites:
        name = site.name
        if site.access == ACCESS_SUFFIX:
            if name in retired_suffixes and site.language != "doc":
                row = retired_suffixes[name]
                violations.append(
                    Violation(
                        "retired-suffix",
                        name,
                        site.path,
                        site.line,
                        f"composes retired suffix {name}; use {row.replacement_suffix}",
                    )
                )
            continue
        if site.access == ACCESS_PREFIX:
            if site.enforced and not _registered_prefix(registry, name):
                violations.append(
                    Violation(
                        "unregistered-prefix",
                        name,
                        site.path,
                        site.line,
                        f"composes names from prefix {name} but no registered name starts with it",
                    )
                )
            continue
        retired = registry.lookup_retired(name)
        if retired is not None:
            if site.path in retired.rejected_by:
                continue
            violations.append(
                Violation(
                    "retired-name",
                    name,
                    site.path,
                    site.line,
                    f"{name} was retired on {retired.retired}; {retired.advice}",
                )
            )
            continue
        if not site.enforced:
            continue
        if name in registry.stem_by_name or registry.is_registered(name):
            continue
        if site.access in DEFINITE_ACCESSES or site.access == ACCESS_STEM:
            violations.append(
                Violation(
                    "unregistered",
                    name,
                    site.path,
                    site.line,
                    f"{name} is not in src/molt/environment_registry.toml ({site.access} via {site.form})",
                )
            )
        elif site.access == ACCESS_MENTION and site.form == FORM_ENV_MENTION:
            violations.append(
                Violation(
                    "unregistered-mention",
                    name,
                    site.path,
                    site.line,
                    f"{name} is written as an environment variable here but is not registered",
                )
            )

    def has_site(path: str, predicate) -> bool:
        return any(predicate(site) for site in scan.sites if site.path == path)

    for variable in registry.variables:
        sites = by_name.get(variable.name, [])
        has_reader = any(
            site.access in READER_ACCESSES and site.language != "doc" for site in sites
        )
        setters = sorted(
            {f"{s.path}:{s.line}" for s in sites if s.access == ACCESS_SET}
        )
        # An internal row is produced for a child, and a ci row may be exported
        # for inspection in later workflow steps; when the consumer is outside
        # the scanned tree (a user's --eval-command, a WASM guest, a CI log) the
        # set site is the evidence and the summary names the consumer.
        if not has_reader and not (variable.audience in {"internal", "ci"} and setters):
            unsetters = sorted(
                {f"{s.path}:{s.line}" for s in sites if s.access == ACCESS_UNSET}
            )
            detail = (
                f"; only set at {', '.join(setters)}"
                if setters
                else f"; only removed at {', '.join(unsetters)}"
                if unsetters
                else "; no site at all"
            )
            violations.append(
                Violation(
                    "no-reader",
                    variable.name,
                    variable.owner,
                    0,
                    f"{variable.name} has no reader{detail}",
                )
            )
        if not (root / variable.owner).exists():
            violations.append(
                Violation(
                    "owner-missing",
                    variable.name,
                    variable.owner,
                    0,
                    f"owner path {variable.owner} does not exist",
                )
            )
        elif not has_site(
            variable.owner,
            lambda s, n=variable.name: s.name == n and s.access != ACCESS_MENTION,
        ):
            violations.append(
                Violation(
                    "owner-mismatch",
                    variable.name,
                    variable.owner,
                    0,
                    f"owner {variable.owner} has no site for {variable.name}",
                )
            )

    for family in registry.families:
        suffix_sites = [
            s for s in by_name.get(family.suffix, []) if s.access == ACCESS_SUFFIX
        ]
        if not suffix_sites:
            violations.append(
                Violation(
                    "no-reader",
                    family.display_name,
                    family.owner,
                    0,
                    f"no composed reader for suffix {family.suffix}",
                )
            )
        if not (root / family.owner).exists():
            violations.append(
                Violation(
                    "owner-missing",
                    family.display_name,
                    family.owner,
                    0,
                    f"owner path {family.owner} does not exist",
                )
            )
        elif not any(s.path == family.owner for s in suffix_sites):
            violations.append(
                Violation(
                    "owner-mismatch",
                    family.display_name,
                    family.owner,
                    0,
                    f"owner {family.owner} does not compose {family.suffix}",
                )
            )

    for stem in registry.stems:
        sites = [
            s
            for s in by_name.get(stem.name, [])
            if s.access != ACCESS_MENTION and s.language != "doc"
        ]
        if not sites:
            violations.append(
                Violation(
                    "no-reader",
                    stem.name,
                    stem.owner,
                    0,
                    f"stem {stem.name} is never passed as a guard prefix",
                )
            )
        if not (root / stem.owner).exists():
            violations.append(
                Violation(
                    "owner-missing",
                    stem.name,
                    stem.owner,
                    0,
                    f"owner path {stem.owner} does not exist",
                )
            )
        elif not any(s.path == stem.owner for s in sites):
            violations.append(
                Violation(
                    "owner-mismatch",
                    stem.name,
                    stem.owner,
                    0,
                    f"owner {stem.owner} has no site for {stem.name}",
                )
            )

    for row in registry.retired:
        for path in row.rejected_by:
            if not (root / path).exists():
                violations.append(
                    Violation(
                        "owner-missing",
                        row.name,
                        path,
                        0,
                        f"rejected_by path {path} does not exist",
                    )
                )
            elif not has_site(
                path, lambda s, n=row.name: s.name == n and s.access != ACCESS_MENTION
            ):
                violations.append(
                    Violation(
                        "owner-mismatch",
                        row.name,
                        path,
                        0,
                        f"{path} no longer names {row.name}; drop it from rejected_by",
                    )
                )

    for family in registry.prefix_families:
        sites = [
            s
            for s in scan.sites
            if s.access == ACCESS_PREFIX and s.name == family.prefix
        ]
        if not sites:
            violations.append(
                Violation(
                    "no-reader",
                    family.display_name,
                    family.owner,
                    0,
                    f"no composed site for prefix {family.prefix}",
                )
            )
        if not (root / family.owner).exists():
            violations.append(
                Violation(
                    "owner-missing",
                    family.display_name,
                    family.owner,
                    0,
                    f"owner path {family.owner} does not exist",
                )
            )
        elif not any(s.path == family.owner for s in sites):
            violations.append(
                Violation(
                    "owner-mismatch",
                    family.display_name,
                    family.owner,
                    0,
                    f"owner {family.owner} does not compose {family.prefix}",
                )
            )

    violations.sort(key=lambda v: (v.rule, v.path, v.line, v.name))
    return violations


def unclassified(registry: EnvironmentRegistry, scan: Scan) -> dict[str, list[Site]]:
    """Unregistered names that appear only as references (never enforced)."""

    result: dict[str, list[Site]] = {}
    for name, sites in scan.by_name().items():
        if name.startswith("_") or name.endswith("_"):
            continue
        if (
            name in registry.stem_by_name
            or registry.is_registered(name)
            or registry.lookup_retired(name)
        ):
            continue
        references = [s for s in sites if s.enforced and s.access == ACCESS_REFERENCE]
        if references:
            result[name] = references
    return result


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------


def _print_violations(violations: list[Violation]) -> None:
    by_rule: dict[str, list[Violation]] = defaultdict(list)
    for violation in violations:
        by_rule[violation.rule].append(violation)
    for rule, rows in sorted(by_rule.items()):
        print(f"[{rule}] {len(rows)}")
        for row in rows:
            location = f"{row.path}:{row.line}" if row.line else row.path
            print(f"  {location}: {row.message}")


def _inventory(scan: Scan) -> dict[str, object]:
    by_name = scan.by_name()
    return {
        "names": {
            name: [asdict(site) for site in sites]
            for name, sites in sorted(by_name.items())
        },
        "unresolved": [asdict(site) for site in scan.unresolved],
    }


def _load_registry() -> EnvironmentRegistry | None:
    try:
        return _gen.build_registry()
    except (_gen.RegistryFormatError, EnvironmentRegistryError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return None


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Environment-registry gate.")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument(
        "--check", action="store_true", help="fail on any violation (default)"
    )
    mode.add_argument(
        "--inventory", action="store_true", help="print every site as JSON"
    )
    mode.add_argument(
        "--list", action="store_true", help="print names with site counts"
    )
    mode.add_argument(
        "--unclassified", action="store_true", help="unregistered reference-only names"
    )
    parser.add_argument(
        "--json", action="store_true", help="machine-readable --check output"
    )
    args = parser.parse_args(argv)

    scan = scan_repository()
    if args.inventory:
        print(json.dumps(_inventory(scan), indent=2, sort_keys=True))
        return 0
    if args.list:
        for name, sites in sorted(scan.by_name().items()):
            accesses = sorted({site.access for site in sites})
            print(f"{name}\t{len(sites)}\t{','.join(accesses)}")
        return 0

    registry = _load_registry()
    if registry is None:
        return 3
    if args.unclassified:
        for name, sites in sorted(unclassified(registry, scan).items()):
            where = ", ".join(sorted({f"{s.path}:{s.line}" for s in sites})[:4])
            print(f"{name}\t{len(sites)}\t{where}")
        return 0

    violations = check(registry, scan)
    if not _gen.projection_is_current():
        violations.insert(
            0,
            Violation(
                "projection-stale",
                "",
                "src/molt/_environment_registry.py",
                0,
                "generated projection or doc is stale; run python3 tools/gen_environment_registry.py",
            ),
        )
    if args.json:
        print(json.dumps({"violations": [asdict(v) for v in violations]}, indent=2))
    elif violations:
        _print_violations(violations)
    else:
        print(
            f"environment registry: clean ({len(registry.variables)} variables, "
            f"{len(registry.families)} families, {len(registry.stems)} stems, "
            f"{len(scan.sites)} sites)"
        )
    return 2 if violations else 0


if __name__ == "__main__":
    raise SystemExit(main())
