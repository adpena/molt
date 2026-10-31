#!/usr/bin/env python3
"""Landing test selection from the repository's dependency facts.

A changed file selects its direct test readers for presubmit and every
dependent test for postsubmit. The dependency relation is derived from the
tracked sources, never restated by hand:

* Python imports. Every ``import`` and ``from`` statement of a tracked module,
  at any scope except an ``if TYPE_CHECKING:`` body, plus literal
  ``importlib.import_module``/``__import__`` names and dotted string literals
  that name a local module (monkeypatch targets, import helpers). Importing a
  module executes its parent packages, so their ``__init__`` files count too.
* Repository paths. String literals and constant path joins (``ROOT / "a" /
  "b"``, ``os.path.join``, ``joinpath``, ``Path(...)``, f-string prefixes) that
  name a tracked file or a tracked directory below the top level, read from the
  repository root or from the module's directory or one of its ancestors. A
  named ``.py`` file executes (``import_file``, ``python tools/x.py``); any
  other named file or directory is data, and a directory stands for every file
  under it.
* The pytest harness. A test depends on the ``conftest.py`` files above it: on
  their module-level code, their hooks, their autouse fixtures and the fixtures
  the test requests by name, closed over fixture parameters and the module
  functions those bodies reference. It also depends on ``pyproject.toml``,
  whose pytest configuration every run reads, and on the plugins it loads
  with ``-p``.
* Generator provenance (``tools/generator_manifest.toml``). A generator's
  ``sync_test`` executes the generator. The always gate ``tools/generators.py
  check`` re-renders every reproducible check-mode generator, so a committed
  output that its sources would change is stale and fails there; readers of a
  fresh output are selected by the output's own edit. A generator outside that
  gate (no check mode, discovery only, or host input) gives each fixed output an
  execution edge to its generator, so its sources select the output's readers.

Prior art: Pants dependency inference (imports, string "assets", conftest
files), Bazel ``rdeps(universe, set)``, Buck2's target determinator and
pytest-testmon's dependency-keyed selection. Facts are cached by Git blob id,
the content address Bazel and Buck2 use for action inputs, so a landing parses
only the blobs it has not seen.
"""

from __future__ import annotations

import argparse
import ast
import hashlib
import json
import re
import sys
import time
import tomllib
from collections.abc import Callable, Iterable, Mapping
from dataclasses import dataclass, field
from pathlib import Path, PurePosixPath
from typing import assert_never

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

ROOT = bind_repository_imports(__file__)

from tools.command_execution import CommandExecutor  # noqa: E402
from tools.molt_dev_policy import LandingMode  # noqa: E402

_COMMANDS = CommandExecutor.for_file(__file__, stem="MOLT_DEV")

# Bump when the extracted facts change shape or meaning; cached facts of an
# older scanner are never reused.
SCANNER_VERSION = 5
CACHE_SCHEMA = f"molt.test-dependency-facts.v{SCANNER_VERSION}"
GENERATOR_MANIFEST = "tools/generator_manifest.toml"
PYPROJECT = "pyproject.toml"
# pyproject's pytest ``pythonpath`` order: ``src`` first, then the root.
_IMPORT_ROOTS = ("src", "")
# Programs compiled by Molt, not modules any host process imports. Tests name
# them by path, which the data edges cover.
_UNSCANNED_PREFIXES = ("tests/differential/",)
_TEST_EXCLUDED_PREFIXES = ("tests/differential/", "tests/compliance/")
_MAX_LITERAL = 300
_DOTTED = re.compile(r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)+")
_IDENTIFIER = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
_GLOB_OR_FORMAT = re.compile(r"[*?\[{]")
_PATH_CALLS = frozenset({"Path", "PurePath", "PurePosixPath", "PureWindowsPath"})
_IMPORT_CALLS = frozenset({"import_module", "__import__", "find_spec"})


# --------------------------------------------------------------------------
# Repository snapshot
# --------------------------------------------------------------------------


def _git_blob_id(content: bytes) -> str:
    return hashlib.sha1(b"blob %d\0" % len(content) + content).hexdigest()


@dataclass(frozen=True)
class RepositoryTree:
    """The tracked files of one source state and their Git blob ids.

    ``commit`` names an immutable tree, read through Git objects. Without it
    the tree is the working copy: the index plus modified and untracked files,
    read from disk, so a local selection sees uncommitted edits.
    """

    root: Path
    blobs: Mapping[str, str]
    commit: str | None = None

    @staticmethod
    def from_commit(root: Path, commit: str) -> RepositoryTree:
        listing = _git(root, ["ls-tree", "-r", "-z", "--full-tree", commit])
        blobs: dict[str, str] = {}
        for record in listing.split("\0"):
            if not record:
                continue
            meta, _, path = record.partition("\t")
            _mode, kind, oid = meta.split(" ")
            if kind == "blob":
                blobs[path] = oid
        return RepositoryTree(root, blobs, commit)

    @staticmethod
    def from_worktree(root: Path) -> RepositoryTree:
        blobs: dict[str, str] = {}
        for record in _git(root, ["ls-files", "-s", "-z"]).split("\0"):
            if record:
                meta, _, path = record.partition("\t")
                blobs[path] = meta.split(" ")[1]
        for path in _git(root, ["ls-files", "-d", "-z"]).split("\0"):
            blobs.pop(path, None)
        changed = [
            path
            for path in (
                *_git(root, ["ls-files", "-m", "-z"]).split("\0"),
                *_git(root, ["ls-files", "-o", "--exclude-standard", "-z"]).split("\0"),
            )
            if path and (root / path).is_file()
        ]
        for path in changed:
            blobs[path] = _git_blob_id((root / path).read_bytes())
        return RepositoryTree(root, blobs)

    def read(self, paths: Iterable[str]) -> dict[str, bytes]:
        paths = list(paths)
        if not paths:
            return {}
        if self.commit is None:
            return {path: (self.root / path).read_bytes() for path in paths}
        request = "".join(f"{self.blobs[path]}\n" for path in paths).encode()
        result = _COMMANDS.run(
            ["git", "-C", str(self.root), "cat-file", "--batch"],
            input=request,
            capture_output=True,
            check=True,
        )
        data: bytes = result.stdout
        contents: dict[str, bytes] = {}
        offset = 0
        for path in paths:
            header_end = data.index(b"\n", offset)
            oid, kind, size = data[offset:header_end].split(b" ")
            if kind != b"blob" or oid.decode() != self.blobs[path]:
                raise RuntimeError(f"git cat-file returned {oid!r} for {path}")
            start = header_end + 1
            end = start + int(size)
            contents[path] = data[start:end]
            offset = end + 1
        return contents


def _git(root: Path, arguments: list[str]) -> str:
    result = _COMMANDS.run(
        ["git", "-C", str(root), *arguments],
        capture_output=True,
        check=True,
        text=True,
        encoding="utf-8",
    )
    return result.stdout


# --------------------------------------------------------------------------
# Source facts: what one blob says, independent of where it sits
# --------------------------------------------------------------------------

# Facts of one scope: imports (level, module, names), path-like literals and
# dotted names, requested fixture names and referenced module functions.
_Scope = dict[str, list]


def _empty_scope() -> _Scope:
    return {"imports": [], "literals": [], "names": [], "refs": []}


# A folded path is (anchor, suffix). The anchor is ``FILE`` for the module's
# own file, ``k >= 0`` for its k-th ancestor directory (0: its directory), or
# ``UNKNOWN`` for a root the source does not show (a parameter, ``tmp_path``,
# an imported constant). Resolution tries an unknown root at the repository
# root and at each of the module's ancestors.
FILE = "file"
UNKNOWN = "?"
# A join onto an unknown root (``tmp_path / "src/molt"``). Tests build fake
# trees that mirror the repository's layout, so such a join names a tracked
# file only; it never stands for a whole tracked directory.
JOINED = "~"
_Folded = tuple[str | int, str]
_PASSTHROUGH = frozenset({"resolve", "absolute", "expanduser", "abspath", "realpath"})


def _join(left: _Folded, right: str) -> _Folded:
    anchor, suffix = left
    if anchor == FILE:
        # ``Path(__file__) / "x"`` is not a path; treat the root as unknown.
        anchor, suffix = UNKNOWN, ""
    return anchor, f"{suffix}/{right}" if suffix else right


def _up(folded: _Folded, levels: int) -> _Folded | None:
    anchor, suffix = folded
    if anchor == FILE and not suffix:
        return 0 + levels - 1, ""
    if isinstance(anchor, int) and not suffix:
        return anchor + levels, ""
    return None


def _fold_path(
    node: ast.AST, env: Mapping[str, ast.expr], seen: frozenset[str] = frozenset()
) -> _Folded | None:
    """Fold a path expression to its anchor and constant suffix, if it has one."""
    if isinstance(node, ast.Constant):
        return (UNKNOWN, node.value) if isinstance(node.value, str) else None
    if isinstance(node, ast.Name):
        if node.id == "__file__":
            return FILE, ""
        if node.id in env and node.id not in seen:
            return _fold_path(env[node.id], env, seen | {node.id})
        return UNKNOWN, ""
    if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Div):
        # Any root the source does not show (``self.root``, ``root()``) is
        # an unknown root, not a reason to drop the constant suffix.
        left = _fold_path(node.left, env, seen) or (UNKNOWN, "")
        right = _fold_path(node.right, env, seen)
        if right is None or right[0] != UNKNOWN or not right[1]:
            return None
        return _join(left, right[1])
    if isinstance(node, ast.Attribute):
        base = _fold_path(node.value, env, seen)
        if base is not None and node.attr == "parent":
            return _up(base, 1)
        return None
    if (
        isinstance(node, ast.Subscript)
        and isinstance(node.value, ast.Attribute)
        and node.value.attr == "parents"
        and isinstance(node.slice, ast.Constant)
        and isinstance(node.slice.value, int)
    ):
        base = _fold_path(node.value.value, env, seen)
        return _up(base, node.slice.value + 1) if base is not None else None
    if isinstance(node, ast.JoinedStr):
        prefix = ""
        for value in node.values:
            if isinstance(value, ast.Constant) and isinstance(value.value, str):
                prefix += value.value
            else:
                break
        # Only the directory part before the first formatted value is known.
        directory = prefix.rpartition("/")[0]
        return (UNKNOWN, directory) if directory else None
    if isinstance(node, ast.Call):
        func = node.func
        name = (
            func.id
            if isinstance(func, ast.Name)
            else func.attr
            if isinstance(func, ast.Attribute)
            else ""
        )
        if name in _PASSTHROUGH:
            target = (
                node.args[0]
                if node.args
                else func.value
                if isinstance(func, ast.Attribute)
                else None
            )
            return _fold_path(target, env, seen) if target is not None else None
        if name == "dirname" and len(node.args) == 1:
            base = _fold_path(node.args[0], env, seen)
            return _up(base, 1) if base is not None else None
        if name in _PATH_CALLS or name in {"join", "joinpath"}:
            folded: _Folded | None = None
            if name == "joinpath" and isinstance(func, ast.Attribute):
                folded = _fold_path(func.value, env, seen)
            for argument in node.args:
                part = _fold_path(argument, env, seen)
                if part is None:
                    if folded is not None:
                        return None
                    part = (UNKNOWN, "")
                if folded is None:
                    folded = part
                elif part[0] == UNKNOWN and part[1]:
                    folded = _join(folded, part[1])
                elif part[1] or part[0] != UNKNOWN:
                    return None
            return folded
    return None


def _is_type_checking(test: ast.expr) -> bool:
    return (isinstance(test, ast.Name) and test.id == "TYPE_CHECKING") or (
        isinstance(test, ast.Attribute) and test.attr == "TYPE_CHECKING"
    )


def _fixture_decorator(decorator: ast.expr) -> tuple[bool, str | None] | None:
    """``(autouse, name)`` when ``decorator`` declares a pytest fixture."""
    call = decorator if isinstance(decorator, ast.Call) else None
    target = call.func if call is not None else decorator
    name = (
        target.attr
        if isinstance(target, ast.Attribute)
        else target.id
        if isinstance(target, ast.Name)
        else ""
    )
    if name != "fixture":
        return None
    autouse = False
    alias = None
    for keyword in call.keywords if call is not None else ():
        if keyword.arg == "autouse" and isinstance(keyword.value, ast.Constant):
            autouse = bool(keyword.value.value)
        if keyword.arg == "name" and isinstance(keyword.value, ast.Constant):
            alias = str(keyword.value.value)
    return autouse, alias


class _FactCollector(ast.NodeVisitor):
    def __init__(self) -> None:
        self.scopes: dict[str, _Scope] = {"": _empty_scope()}
        self.fixtures: dict[str, dict] = {}
        self.functions: set[str] = set()
        # Module-level single-name assignments, for ``ROOT / "x"`` folding.
        self.env: dict[str, ast.expr] = {}
        # Path constants a module defines; each use site records the read.
        self._definitions: set[int] = set()
        self._scope = ""

    def _current(self) -> _Scope:
        return self.scopes.setdefault(self._scope, _empty_scope())

    def visit_Module(self, node: ast.Module) -> None:
        for statement in node.body:
            if isinstance(statement, (ast.FunctionDef, ast.AsyncFunctionDef)):
                self.functions.add(statement.name)
            elif (
                isinstance(statement, ast.Assign)
                and len(statement.targets) == 1
                and isinstance(statement.targets[0], ast.Name)
            ):
                self._define(statement.targets[0].id, statement.value)
            elif (
                isinstance(statement, ast.AnnAssign)
                and isinstance(statement.target, ast.Name)
                and statement.value is not None
            ):
                self._define(statement.target.id, statement.value)
        for statement in node.body:
            if isinstance(statement, (ast.FunctionDef, ast.AsyncFunctionDef)):
                self._scope = statement.name
                for decorator in statement.decorator_list:
                    declared = _fixture_decorator(decorator)
                    if declared is not None:
                        autouse, alias = declared
                        self.fixtures[alias or statement.name] = {
                            "autouse": autouse,
                            "scope": statement.name,
                        }
                self.visit(statement)
                self._scope = ""
            else:
                self.visit(statement)

    def _define(self, name: str, value: ast.expr) -> None:
        if name in self.env:
            return
        self.env[name] = value
        if (
            isinstance(value, (ast.BinOp, ast.Call))
            and _fold_path(value, {}) is not None
        ):
            # ``ROOT = Path(__file__).parents[1]`` or ``DATA = ROOT / "x"``:
            # a root to join onto, read where the module uses it.
            self._definitions.add(id(value))

    def _is_path_name(self, node: ast.expr) -> bool:
        return isinstance(node, ast.Name) and node.id in self.env

    def visit_Name(self, node: ast.Name) -> None:
        if node.id in self.functions:
            self._current()["refs"].append(node.id)
        if isinstance(node.ctx, ast.Load) and node.id in self.env:
            value = self.env[node.id]
            if id(value) in self._definitions:
                self._record(_fold_path(value, self.env))

    def visit_If(self, node: ast.If) -> None:
        if _is_type_checking(node.test):
            for statement in node.orelse:
                self.visit(statement)
            return
        self.generic_visit(node)

    def visit_Import(self, node: ast.Import) -> None:
        for alias in node.names:
            self._current()["imports"].append([0, alias.name, []])

    def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
        self._current()["imports"].append(
            [node.level, node.module or "", [alias.name for alias in node.names]]
        )

    def _arguments(self, node: ast.arguments) -> None:
        names = self._current()["names"]
        for argument in (*node.posonlyargs, *node.args, *node.kwonlyargs):
            names.append(argument.arg)

    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
        self._arguments(node.args)
        self.generic_visit(node)

    def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
        self._arguments(node.args)
        self.generic_visit(node)

    def visit_Lambda(self, node: ast.Lambda) -> None:
        self._arguments(node.args)
        self.generic_visit(node)

    def visit_Constant(self, node: ast.Constant) -> None:
        value = node.value
        if not isinstance(value, str) or not value or len(value) > _MAX_LITERAL:
            return
        if any(character.isspace() for character in value):
            return
        scope = self._current()
        if _IDENTIFIER.fullmatch(value):
            scope["names"].append(value)
        elif "/" in value or "." in value:
            scope["literals"].append(f"{UNKNOWN}:{value}")

    def _fold(self, node: ast.AST) -> None:
        if id(node) in self._definitions:
            return
        self._record(
            _fold_path(node, self.env), joined=not isinstance(node, ast.JoinedStr)
        )

    def _record(self, folded: _Folded | None, *, joined: bool = True) -> None:
        if folded is None:
            return
        anchor, suffix = folded
        # A bare constant is recorded by visit_Constant. An anchor alone (a
        # module's directory or an ancestor) is a root to join, not a read.
        if (
            not suffix
            or anchor == FILE
            or (anchor == UNKNOWN and "/" not in suffix)
            or len(suffix) > _MAX_LITERAL
        ):
            return
        if anchor == UNKNOWN and joined:
            anchor = JOINED
        self._current()["literals"].append(f"{anchor}:{suffix}")

    def visit_BinOp(self, node: ast.BinOp) -> None:
        if isinstance(node.op, ast.Div):
            self._fold(node)
            # The chain's inner joins, its constant parts and its root name
            # are parts of the one path the fold recorded, not reads.
            parts: list[ast.expr] = [node.right]
            left: ast.expr = node.left
            while isinstance(left, ast.BinOp) and isinstance(left.op, ast.Div):
                parts.append(left.right)
                left = left.left
            parts.append(left)
            folded = _fold_path(node, self.env) is not None
            for part in parts:
                if not folded or (
                    not isinstance(part, ast.Constant) and not self._is_path_name(part)
                ):
                    self.visit(part)
            return
        self.generic_visit(node)

    def visit_Call(self, node: ast.Call) -> None:
        func = node.func
        name = (
            func.id
            if isinstance(func, ast.Name)
            else func.attr
            if isinstance(func, ast.Attribute)
            else ""
        )
        if (
            name in _IMPORT_CALLS
            and node.args
            and isinstance(node.args[0], ast.Constant)
            and isinstance(node.args[0].value, str)
        ):
            self._current()["imports"].append([0, node.args[0].value, []])
        self._fold(node)
        if (name in _PATH_CALLS or name in {"join", "joinpath"}) and _fold_path(
            node, self.env
        ) is not None:
            # As for a ``/`` chain: the parts form the one recorded path.
            if isinstance(func, ast.Attribute) and not self._is_path_name(func.value):
                self.visit(func.value)
            for argument in (*node.args, *(keyword.value for keyword in node.keywords)):
                if not isinstance(argument, ast.Constant) and not self._is_path_name(
                    argument
                ):
                    self.visit(argument)
            return
        self.generic_visit(node)

    def visit_JoinedStr(self, node: ast.JoinedStr) -> None:
        self._fold(node)
        self.generic_visit(node)


def scan_source(content: bytes) -> dict:
    """The dependency facts one Python blob declares, keyed by scope.

    Scope ``""`` is module-level code (classes included); a top-level function
    is its own scope, so a conftest fixture's imports bind only its users.
    A blob that does not parse yields no facts and a recorded error.
    """
    try:
        tree = ast.parse(content)
    except (SyntaxError, ValueError) as exc:
        return {"scopes": {}, "fixtures": {}, "error": f"{type(exc).__name__}: {exc}"}
    collector = _FactCollector()
    collector.visit(tree)
    scopes = {
        name: {key: sorted(set(map(_freeze, values))) for key, values in scope.items()}
        for name, scope in collector.scopes.items()
    }
    for scope in scopes.values():
        scope["imports"] = [list(item) for item in scope["imports"]]
    return {"scopes": scopes, "fixtures": collector.fixtures}


def _freeze(value: object) -> object:
    if isinstance(value, list):
        return (value[0], value[1], tuple(value[2]))
    return value


# --------------------------------------------------------------------------
# Fact cache: content-addressed by Git blob id
# --------------------------------------------------------------------------


def default_cache_path() -> Path:
    from molt.default_paths import _default_molt_cache

    return _default_molt_cache() / "test_dependency_graph" / f"{CACHE_SCHEMA}.json"


def _load_cache(path: Path | None) -> dict[str, dict]:
    if path is None:
        return {}
    try:
        payload = json.loads(path.read_bytes())
    except (OSError, ValueError):
        return {}
    if not isinstance(payload, dict) or payload.get("schema") != CACHE_SCHEMA:
        return {}
    facts = payload.get("facts")
    return facts if isinstance(facts, dict) else {}


def _store_cache(path: Path | None, facts: Mapping[str, dict]) -> None:
    if path is None:
        return
    from molt.file_publication import atomic_write_bytes

    path.parent.mkdir(parents=True, exist_ok=True)
    payload = json.dumps(
        {"schema": CACHE_SCHEMA, "facts": facts}, separators=(",", ":")
    )
    atomic_write_bytes(path, payload.encode("utf-8"))


def source_facts(
    tree: RepositoryTree, paths: Iterable[str], *, cache: Path | None
) -> tuple[dict[str, dict], dict[str, float | int]]:
    """Facts for ``paths``, parsing only blobs the cache has not seen.

    The cache keeps exactly the blobs of the latest tree, so it stays bounded
    by one snapshot; another checkout at another commit re-parses only the
    blobs that differ.
    """
    paths = sorted(paths)
    started = time.perf_counter()
    cached = _load_cache(cache)
    loaded = time.perf_counter()
    misses = sorted({tree.blobs[path] for path in paths} - cached.keys())
    by_oid = {tree.blobs[path]: path for path in paths}
    contents = tree.read(by_oid[oid] for oid in misses)
    fresh = {oid: scan_source(contents[by_oid[oid]]) for oid in misses}
    parsed = time.perf_counter()
    facts = {oid: cached.get(oid) or fresh[oid] for oid in by_oid}
    if fresh or len(cached) != len(facts):
        _store_cache(cache, facts)
    stats: dict[str, float | int] = {
        "python_files": len(paths),
        "parsed_blobs": len(misses),
        "cache_load_seconds": round(loaded - started, 4),
        "parse_seconds": round(parsed - loaded, 4),
    }
    return {path: facts[tree.blobs[path]] for path in paths}, stats


# --------------------------------------------------------------------------
# The graph
# --------------------------------------------------------------------------


_INIT = "#init"


def init_node(path: str) -> str:
    """The node for a module's module-level code alone (a package's init)."""
    return f"{path}{_INIT}"


def function_node(path: str, function: str) -> str:
    """The node for one top-level function and what its body reaches."""
    return f"{path}::{function}"


def node_path(node: str) -> str:
    return node.removesuffix(_INIT).partition("::")[0]


@dataclass(frozen=True)
class Dependencies:
    """What one node reads: executed sources and data files or directories."""

    execs: frozenset[str] = frozenset()
    files: frozenset[str] = frozenset()
    directories: frozenset[str] = frozenset()


@dataclass
class _Resolver:
    files: frozenset[str]
    directories: frozenset[str]
    modules: dict[str, str]
    # Each module's top-level functions, for name-level imports.
    functions: Mapping[str, frozenset[str]] = field(default_factory=dict)

    @staticmethod
    def build(
        paths: Iterable[str], functions: Mapping[str, frozenset[str]]
    ) -> _Resolver:
        files = frozenset(paths)
        directories = frozenset(
            str(parent)
            for path in files
            for parent in PurePosixPath(path).parents
            if str(parent) != "."
        )
        modules: dict[str, str] = {}
        for import_root in _IMPORT_ROOTS:
            prefix = f"{import_root}/" if import_root else ""
            for path in sorted(files):
                if not path.endswith(".py") or not path.startswith(prefix):
                    continue
                parts = path[len(prefix) : -3].split("/")
                if parts[-1] == "__init__":
                    parts = parts[:-1]
                if parts and all(_IDENTIFIER.fullmatch(part) for part in parts):
                    modules.setdefault(".".join(parts), path)
        return _Resolver(files, directories, modules, functions)

    def module_name(self, path: str) -> str | None:
        for import_root in _IMPORT_ROOTS:
            prefix = f"{import_root}/" if import_root else ""
            if path.startswith(prefix) and path.endswith(".py"):
                parts = path[len(prefix) : -3].split("/")
                if parts[-1] == "__init__":
                    parts = parts[:-1]
                name = ".".join(parts)
                if self.modules.get(name) == path:
                    return name
        return None

    def module_targets(self, name: str) -> set[str]:
        """The nodes importing ``name`` executes.

        The module runs whole, since its importer may call any of its
        functions; each parent package runs only its module-level code.
        """
        parts = name.split(".")
        found = set()
        for index in range(1, len(parts) + 1):
            path = self.modules.get(".".join(parts[:index]))
            if path is not None:
                found.add(path if index == len(parts) else init_node(path))
        return found

    def imports(self, path: str, imports: Iterable[list]) -> set[str]:
        name = self.module_name(path)
        package = (
            name
            if name is not None and path.endswith("/__init__.py")
            else name.rpartition(".")[0]
            if name is not None
            else ""
        )
        resolved: set[str] = set()
        for level, module, names in imports:
            if level:
                base = package.split(".") if package else []
                if level > 1:
                    base = base[: len(base) - (level - 1)]
                module = ".".join([*base, *([module] if module else [])])
            if not module:
                continue
            submodules = [
                f"{module}.{member}"
                for member in names
                if f"{module}.{member}" in self.modules
            ]
            source = self.modules.get(module)
            if source is not None and names and "*" not in names:
                # ``from module import name`` runs the module's module-level
                # code (which defines its classes and constants) and only the
                # named top-level functions the importer may call.
                functions = self.functions.get(source, frozenset())
                resolved.update(
                    init_node(target) if target == source else target
                    for target in self.module_targets(module)
                )
                resolved.update(
                    function_node(source, member)
                    for member in names
                    if member in functions
                )
            else:
                resolved.update(self.module_targets(module))
            for submodule in submodules:
                resolved.update(self.module_targets(submodule))
        resolved.discard(path)
        resolved.discard(init_node(path))
        return resolved

    def literal(self, path: str, record: str) -> tuple[set[str], set[str], set[str]]:
        """(executed sources, data files, data directories) a literal names."""
        execs: set[str] = set()
        files: set[str] = set()
        directories: set[str] = set()
        anchor, _, value = record.partition(":")
        # A test names the modules it launches (``-m``), patches or loads; a
        # dotted name elsewhere is a registry key or a process pattern. Import
        # calls with a literal name are imports wherever they appear.
        launches = is_test_file(path)
        if launches and anchor == UNKNOWN and _DOTTED.fullmatch(value):
            parts = value.split(".")
            for end in range(len(parts), 1, -1):
                candidate = ".".join(parts[:end])
                if candidate in self.modules:
                    execs.update(self.module_targets(candidate))
                    break
        text = value.removeprefix("./").rstrip("/")
        glob = _GLOB_OR_FORMAT.search(text)
        if glob is not None:
            text = text[: glob.start()].rpartition("/")[0]
        if not text or text.startswith("/") or ".." in text.split("/"):
            return execs, files, directories
        ancestors = [str(parent) for parent in PurePosixPath(path).parents]
        if anchor in (UNKNOWN, JOINED):
            bases = ["", *(base for base in ancestors if base != ".")]
        else:
            level = int(anchor)
            if level >= len(ancestors):
                return execs, files, directories
            bases = ["" if ancestors[level] == "." else ancestors[level]]
        # A test launches a named Python source (``import_file``, a tool's
        # argv); a tool, library or helper names one as data (a file list).
        for base in bases:
            candidate = f"{base}/{text}" if base else text
            if is_test_file(candidate):
                # A test file is code pytest runs, never another test's data:
                # naming one (a selection oracle, an ownership list) reads the
                # plan, not the file. Tests that import it still depend on it.
                continue
            if candidate in self.files:
                if launches and candidate.endswith(".py"):
                    execs.add(candidate)
                else:
                    files.add(candidate)
            elif candidate in self.directories and "/" in candidate:
                # A top-level directory names a whole subsystem, not an input.
                if anchor != JOINED:
                    directories.add(candidate)
        return execs, files, directories


def _scope_dependencies(
    resolver: _Resolver, path: str, scopes: Iterable[_Scope]
) -> Dependencies:
    execs: set[str] = set()
    files: set[str] = set()
    directories: set[str] = set()
    for scope in scopes:
        execs |= resolver.imports(path, scope.get("imports", ()))
        for value in scope.get("literals", ()):
            more_execs, more_files, more_directories = resolver.literal(path, value)
            execs |= more_execs
            files |= more_files
            directories |= more_directories
    execs.discard(path)
    return Dependencies(frozenset(execs), frozenset(files), frozenset(directories))


def is_test_file(path: str) -> bool:
    name = PurePosixPath(path).name
    return (
        path.startswith("tests/")
        and name.startswith("test_")
        and name.endswith(".py")
        and not path.startswith(_TEST_EXCLUDED_PREFIXES)
        and "/fixtures/" not in path
    )


def _pytest_plugins(pyproject: bytes | None) -> tuple[str, ...]:
    if pyproject is None:
        return ()
    options = (
        tomllib.loads(pyproject.decode("utf-8"))
        .get("tool", {})
        .get("pytest", {})
        .get("ini_options", {})
        .get("addopts", [])
    )
    if isinstance(options, str):
        options = options.split()
    return tuple(
        options[index + 1]
        for index, option in enumerate(options[:-1])
        if option == "-p" and not options[index + 1].startswith("no:")
    )


def _generator_edges(
    manifest: bytes | None, files: frozenset[str]
) -> tuple[dict[str, set[str]], dict[str, set[str]]]:
    """(sync-test edges, unchecked-output edges) of generator provenance."""
    sync_edges: dict[str, set[str]] = {}
    output_edges: dict[str, set[str]] = {}
    if manifest is None:
        return sync_edges, output_edges
    for row in tomllib.loads(manifest.decode("utf-8")).get("generator", ()):
        tool = row.get("tool")
        if not isinstance(tool, str) or tool not in files:
            continue
        sync_test = row.get("sync_test")
        if isinstance(sync_test, str) and sync_test in files:
            sync_edges.setdefault(sync_test, set()).add(tool)
        # tools/generators.py check re-renders exactly these (manifest_generators).
        gated = (
            row.get("check_mode")
            and not row.get("discovery_only")
            and row.get("ci_checkable", True)
        )
        if not gated:
            for output in row.get("outputs", ()):
                if output in files:
                    output_edges.setdefault(output, set()).add(tool)
    return sync_edges, output_edges


@dataclass(frozen=True)
class TestInputs:
    """What one test file reads beyond its own module's import closure.

    ``roots`` are the sources the test executes: itself, its package inits,
    the pytest plugins and the conftest scopes it uses. ``files`` and
    ``directories`` are data it reads: the paths its own module, its
    ``tests/`` helpers, the modules those import directly and the fixtures it
    requests name. A module deeper in the import graph that names a path is
    an implementation detail of a call; binding its reads to every importer
    would make a broad path constant select the whole suite.
    """

    roots: frozenset[str]
    files: frozenset[str]
    directories: frozenset[str]


@dataclass
class DependencyGraph:
    """The repository's file-level dependency graph over one tree."""

    tree: RepositoryTree
    nodes: dict[str, Dependencies]
    tests: tuple[str, ...]
    inputs: dict[str, TestInputs]
    conftests: dict[str, tuple[str, ...]]
    # Generated outputs whose generator no freshness gate re-renders.
    unchecked_outputs: frozenset[str] = frozenset()
    stats: dict[str, float | int] = field(default_factory=dict)
    _reverse: dict[str, frozenset[str]] | None = None
    _by_path: dict[str, list[str]] | None = None

    def _reverse_execs(self) -> dict[str, frozenset[str]]:
        if self._reverse is None:
            reverse: dict[str, set[str]] = {}
            for node, dependencies in self.nodes.items():
                for target in dependencies.execs:
                    reverse.setdefault(target, set()).add(node)
            self._reverse = {key: frozenset(value) for key, value in reverse.items()}
        return self._reverse

    def _nodes_of(self, path: str) -> list[str]:
        if self._by_path is None:
            by_path: dict[str, list[str]] = {}
            for node in self.nodes:
                by_path.setdefault(node_path(node), []).append(node)
            self._by_path = by_path
        return self._by_path.get(path, [path])

    def affected(self, changed: Iterable[str]) -> frozenset[str]:
        """Every source whose execution a change to ``changed`` reaches."""
        reverse = self._reverse_execs()
        affected: set[str] = set()
        pending = [node for path in changed for node in self._nodes_of(path)]
        while pending:
            node = pending.pop()
            if node in affected:
                continue
            affected.add(node)
            pending.extend(reverse.get(node, ()))
        return frozenset(affected)

    def select(self, changed: Iterable[str]) -> tuple[str, ...]:
        """The test files that depend on any changed path, sorted."""
        changed = frozenset(changed)
        affected = self.affected(changed)
        # A stale unchecked output changes what its readers read. A test file
        # is no other test's data (see _Resolver.literal).
        read = {path for path in changed if not is_test_file(path)} | (
            {node_path(node) for node in affected} & self.unchecked_outputs
        )
        read_directories = {
            str(parent) for path in read for parent in PurePosixPath(path).parents
        }
        selected = []
        for test in self.tests:
            inputs = self.inputs[test]
            if (
                inputs.roots & affected
                or any(conftest in changed for conftest in self.conftests[test])
                or inputs.files & read
                or inputs.directories & read_directories
            ):
                selected.append(test)
        return tuple(selected)

    def direct_readers(self, changed: Iterable[str]) -> tuple[str, ...]:
        """Tests at the first hop, without walking their import closure.

        The existing scanner already records each test's direct data reads,
        harness inputs and executed sources. Only the test's own outgoing
        edges are added here; an importer's importer belongs to postsubmit.
        """
        changed = frozenset(changed)
        read = {path for path in changed if not is_test_file(path)}
        directories = {
            str(parent) for path in read for parent in PurePosixPath(path).parents
        }
        selected = []
        for test in self.tests:
            inputs = self.inputs[test]
            sources = {
                node_path(node)
                for node in inputs.roots | self.nodes.get(test, Dependencies()).execs
            }
            if (
                test in changed
                or sources & changed
                or any(conftest in changed for conftest in self.conftests[test])
                or inputs.files & read
                or inputs.directories & directories
            ):
                selected.append(test)
        return tuple(selected)

    def dependencies(self, test: str) -> frozenset[str]:
        """Every tracked file whose change selects ``test`` (for inspection)."""
        reached: set[str] = set()
        pending = [*self.inputs[test].roots, *self.conftests[test]]
        while pending:
            node = pending.pop()
            if node not in reached:
                reached.add(node)
                pending.extend(self.nodes.get(node, Dependencies()).execs)
        directories = self.inputs[test].directories
        return frozenset(
            {node_path(node) for node in reached}
            | self.inputs[test].files
            | {
                path
                for path in self.tree.blobs
                if any(
                    str(parent) in directories for parent in PurePosixPath(path).parents
                )
            }
        )


def _conftest_scopes(
    requested: set[str], chain: tuple[str, ...], facts: Mapping[str, Mapping]
) -> tuple[dict[str, set[str]], dict[str, set[str]]]:
    """(setup scopes, requested-fixture scopes) of each conftest in ``chain``.

    Setup scopes run for every test below the conftest: module-level code,
    hooks and autouse fixtures. A fixture the test names, a fixture those
    request and the module functions their bodies reference run only for it.
    """
    setup: dict[str, set[str]] = {}
    used: dict[str, set[str]] = {conftest: set() for conftest in chain}
    for conftest in chain:
        record = facts[conftest]
        chosen = {""} | {
            name for name in record.get("scopes", {}) if name.startswith("pytest_")
        }
        chosen |= {
            fixture["scope"]
            for fixture in record.get("fixtures", {}).values()
            if fixture["autouse"]
        }
        setup[conftest] = chosen
    requested = set(requested)
    while True:
        grown = False
        for conftest in chain:
            record = facts[conftest]
            scopes = record.get("scopes", {})
            for name in requested:
                fixture = record.get("fixtures", {}).get(name)
                if fixture is not None and fixture["scope"] not in (
                    setup[conftest] | used[conftest]
                ):
                    used[conftest].add(fixture["scope"])
                    grown = True
            for bucket in (setup[conftest], used[conftest]):
                for scope_name in list(bucket):
                    scope = scopes.get(scope_name, {})
                    for reference in scope.get("refs", ()):
                        if reference in scopes and reference not in bucket:
                            bucket.add(reference)
                            grown = True
                    new_names = set(scope.get("names", ())) - requested
                    if new_names:
                        requested |= new_names
                        grown = True
        if not grown:
            return setup, used


def _test_inputs(
    resolver: _Resolver,
    test: str,
    nodes: Mapping[str, Dependencies],
    chain: tuple[str, ...],
    facts: Mapping[str, Mapping],
    plugins: tuple[str, ...],
    configuration: frozenset[str],
) -> TestInputs:
    # The test module and the tests/ helpers it executes form its own side.
    side: set[str] = set()
    pending = [test]
    while pending:
        node = pending.pop()
        if node in side:
            continue
        side.add(node)
        pending.extend(
            target
            for target in nodes.get(node, Dependencies()).execs
            if target.startswith("tests/")
            and PurePosixPath(target).name != "conftest.py"
        )
    # A directly imported module's reads are the test's inputs: close each
    # imported node over its own module's init and the functions it calls.
    readers: set[str] = set()
    pending = list(side) + [
        target for node in side for target in nodes.get(node, Dependencies()).execs
    ]
    while pending:
        node = pending.pop()
        if node in readers:
            continue
        readers.add(node)
        pending.extend(
            target
            for target in nodes.get(node, Dependencies()).execs
            if node_path(target) == node_path(node)
        )
    files = {path for node in readers for path in nodes.get(node, Dependencies()).files}
    directories = {
        path for node in readers for path in nodes.get(node, Dependencies()).directories
    }
    requested = {
        name
        for scope in facts[test].get("scopes", {}).values()
        for name in scope.get("names", ())
    }
    setup, used = _conftest_scopes(requested, chain, facts)
    roots = {test}
    for conftest in chain:
        scopes = facts[conftest].get("scopes", {})
        harness = _scope_dependencies(
            resolver,
            conftest,
            (scopes[name] for name in setup[conftest] if name in scopes),
        )
        # Setup code names isolation roots (scratch, caches, forbidden import
        # roots), not inputs; only its executed sources bind every test.
        roots |= harness.execs
        requested_inputs = _scope_dependencies(
            resolver,
            conftest,
            (scopes[name] for name in used[conftest] if name in scopes),
        )
        roots |= requested_inputs.execs
        files |= requested_inputs.files
        directories |= requested_inputs.directories
    # pytest reads its configuration (addopts, plugins, markers, import
    # mode) for every test it runs.
    files |= configuration
    for plugin in plugins:
        roots.update(resolver.module_targets(plugin))
    # The test module itself runs as a member of its packages.
    name = resolver.module_name(test)
    if name is not None:
        roots.update(resolver.module_targets(name))
    return TestInputs(frozenset(roots), frozenset(files), frozenset(directories))


def build_graph(tree: RepositoryTree, *, cache: Path | None) -> DependencyGraph:
    started = time.perf_counter()
    python = [
        path
        for path in tree.blobs
        if path.endswith(".py") and not path.startswith(_UNSCANNED_PREFIXES)
    ]
    facts, stats = source_facts(tree, python, cache=cache)
    functions = {
        path: frozenset(name for name in facts[path].get("scopes", {}) if name)
        for path in python
    }
    resolver = _Resolver.build(tree.blobs, functions)
    nodes: dict[str, Dependencies] = {}
    for path in python:
        if PurePosixPath(path).name == "conftest.py":
            # A conftest binds tests through the harness, scope by scope.
            nodes[path] = Dependencies()
            continue
        scopes = facts[path].get("scopes", {})
        whole = _scope_dependencies(resolver, path, scopes.values())
        nodes[path] = Dependencies(
            whole.execs | {init_node(path)}, whole.files, whole.directories
        )
        for name, scope in scopes.items():
            reached = _scope_dependencies(resolver, path, [scope])
            calls = {
                function_node(path, reference)
                for reference in scope.get("refs", ())
                if reference in scopes and reference != name
            }
            node = function_node(path, name) if name else init_node(path)
            if name:
                calls.add(init_node(path))
            nodes[node] = Dependencies(
                reached.execs | calls, reached.files, reached.directories
            )
    configuration = tree.read(
        path for path in (PYPROJECT, GENERATOR_MANIFEST) if path in tree.blobs
    )
    sync_edges, output_edges = _generator_edges(
        configuration.get(GENERATOR_MANIFEST), resolver.files
    )
    for node, tools in (*sync_edges.items(), *output_edges.items()):
        current = nodes.get(node, Dependencies())
        nodes[node] = Dependencies(
            current.execs | frozenset(tools), current.files, current.directories
        )
    plugins = _pytest_plugins(configuration.get(PYPROJECT))
    pytest_configuration = frozenset(
        path for path in (PYPROJECT,) if path in configuration
    )
    tests = tuple(sorted(path for path in python if is_test_file(path)))
    conftests: dict[str, tuple[str, ...]] = {}
    inputs: dict[str, TestInputs] = {}
    for test in tests:
        chain = tuple(
            candidate
            for parent in reversed(PurePosixPath(test).parents)
            if (candidate := f"{parent}/conftest.py".removeprefix("./")) in facts
        )
        conftests[test] = chain
        inputs[test] = _test_inputs(
            resolver, test, nodes, chain, facts, plugins, pytest_configuration
        )
    stats["graph_seconds"] = round(time.perf_counter() - started, 4)
    stats["tests"] = len(tests)
    return DependencyGraph(
        tree, nodes, tests, inputs, conftests, frozenset(output_edges), stats
    )


# --------------------------------------------------------------------------
# Landing envelopes: how the landing runs a selected test
# --------------------------------------------------------------------------

# Presubmit and postsubmit also admit main-owned direct consumers.
LANDING_TIER = "pre-push"
# Commands of this class need compiler builds that a landing's pytest
# consumers do not provision; their own proof cells run their tests.
BUILD_RESOURCE_CLASS = "compiler-build-resource"


@dataclass(frozen=True)
class LandingEnvelope:
    """The targets and tools under which the landing runs one test file.

    Pre-push and main plan commands supply the file's declared prerequisites.
    Presubmit runs its direct inputs unfiltered. Postsubmit also admits the
    existing slow cell; complementary selectors run the file unfiltered.
    Cells that require a compiler build retain their separate proof owner.
    """

    targets: tuple[str, ...]
    toolchains: tuple[str, ...]
    owners: tuple[str, ...]
    select: tuple[str, ...] = ()


@dataclass(frozen=True)
class GraphSelection:
    """The graph's test selection and where each selected test runs."""

    selected: tuple[str, ...]
    run: Mapping[str, LandingEnvelope]
    # Selected tests whose proof cells need builds, or that no cell owns.
    deferred: Mapping[str, tuple[str, ...]]


_SELECTOR_OPTIONS = frozenset({"-k", "-m"})


def cell_selector(command: Mapping) -> tuple[str, ...]:
    """The ``-k``/``-m`` options a plan command passes to pytest itself."""
    argv = [str(part) for part in command.get("argv", ())]
    if "pytest" not in argv:
        return ()
    options = argv[argv.index("pytest") + 1 :]
    selector: list[str] = []
    for index, option in enumerate(options[:-1]):
        if option in _SELECTOR_OPTIONS:
            selector.extend((option, options[index + 1]))
    return tuple(selector)


def landing_envelopes(
    commands: Iterable[Mapping],
    *,
    mode: LandingMode = LandingMode.PRESUBMIT,
    foreign_cells: frozenset[str] = frozenset(),
) -> tuple[dict[str, LandingEnvelope], dict[str, tuple[str, ...]]]:
    """(landing envelopes, other owners) of every test file the plan names.

    A command of a ``foreign_cells`` cell verifies another platform (its hosted
    toolchains are absent here), so its own cell owns its tests.
    """
    whole: dict[str, list[Mapping]] = {}
    nodes: dict[str, list[tuple[str, Mapping]]] = {}
    slow_whole: dict[str, list[Mapping]] = {}
    slow_nodes: dict[str, list[tuple[str, Mapping]]] = {}
    others: dict[str, list[str]] = {}
    for command in commands:
        targets = command.get("pytest") or ()
        tiers = command.get("tiers") or ()
        match mode:
            case LandingMode.PRESUBMIT:
                selected_tier = LANDING_TIER in tiers or "main" in tiers
            case LandingMode.POSTSUBMIT:
                selected_tier = (
                    LANDING_TIER in tiers
                    or "main" in tiers
                    or cell_selector(command) == ("-m", "slow")
                )
            case _:
                assert_never(mode)
        landing = (
            selected_tier
            and command.get("resource_class") != BUILD_RESOURCE_CLASS
            and command.get("cell") not in foreign_cells
        )
        for target in targets:
            path, separator, _node = target.partition("::")
            if not landing:
                others.setdefault(path, []).append(str(command["id"]))
                if (
                    mode is LandingMode.PRESUBMIT
                    and cell_selector(command) == ("-m", "slow")
                    and command.get("resource_class") != BUILD_RESOURCE_CLASS
                ):
                    if separator:
                        slow_nodes.setdefault(path, []).append((target, command))
                    else:
                        slow_whole.setdefault(path, []).append(command)
            elif separator:
                nodes.setdefault(path, []).append((target, command))
            else:
                whole.setdefault(path, []).append(command)
    # A direct changed input may be owned only by the scheduled slow cell.
    # Use that declared non-build envelope without enlarging ordinary owners.
    ordinary = {path: None for path in {*whole, *nodes}}
    for path in {*slow_whole, *slow_nodes}:
        if _owned_by(path, ordinary) is None:
            if path in slow_whole:
                whole[path] = slow_whole[path]
            else:
                nodes[path] = slow_nodes[path]
    envelopes: dict[str, LandingEnvelope] = {}
    for path in {*whole, *nodes}:
        if path in whole:
            owners = whole[path]
            targets: tuple[str, ...] = (path,)
        else:
            owners = [command for _target, command in nodes[path]]
            targets = tuple(dict.fromkeys(target for target, _ in nodes[path]))
        selectors = {cell_selector(owner) for owner in owners}
        envelopes[path] = LandingEnvelope(
            targets,
            tuple(
                dict.fromkeys(tool for owner in owners for tool in owner["toolchains"])
            ),
            tuple(dict.fromkeys(str(owner["id"]) for owner in owners)),
            next(iter(selectors)) if len(selectors) == 1 else (),
        )
    return envelopes, {path: tuple(dict.fromkeys(ids)) for path, ids in others.items()}


def _owned_by(path: str, table: Mapping[str, object]) -> str | None:
    """The declared target (the file or a directory above it) that owns ``path``."""
    for candidate in (path, *(str(parent) for parent in PurePosixPath(path).parents)):
        if candidate in table:
            return candidate
    return None


def graph_selection(
    graph: DependencyGraph,
    changed: Iterable[str],
    commands: Iterable[Mapping],
    *,
    mode: LandingMode = LandingMode.PRESUBMIT,
    recent_failures: Iterable[str] = (),
    foreign_cells: frozenset[str] = frozenset(),
) -> GraphSelection:
    envelopes, others = landing_envelopes(
        commands, mode=mode, foreign_cells=foreign_cells
    )
    run: dict[str, LandingEnvelope] = {}
    deferred: dict[str, tuple[str, ...]] = {}
    changed = tuple(changed)
    match mode:
        case LandingMode.PRESUBMIT:
            recent = (
                target.partition("::")[0]
                for target in recent_failures
                if target.partition("::")[0] in graph.tests
            )
            selected = tuple(
                dict.fromkeys(
                    (
                        *recent,
                        *(path for path in changed if path in graph.tests),
                        *graph.direct_readers(changed),
                    )
                )
            )
        case LandingMode.POSTSUBMIT:
            selected = graph.select(changed)
        case _:
            assert_never(mode)
    for test in selected:
        owner = _owned_by(test, envelopes)
        if owner is not None:
            envelope = envelopes[owner]
            # Every presubmit graph target is a changed test, direct reader or
            # recent failure. Exercise slow probes too at this direct boundary.
            selector = () if mode is LandingMode.PRESUBMIT else envelope.select
            run[test] = LandingEnvelope(
                (test,) if owner != test else envelope.targets,
                envelope.toolchains,
                envelope.owners,
                selector,
            )
        else:
            other = _owned_by(test, others)
            deferred[test] = others[other] if other is not None else ()
    return GraphSelection(selected, run, deferred)


_GRAPHS: dict[tuple[Path, str | None], DependencyGraph] = {}


def cached_graph(root: Path, *, commit: str | None = None) -> DependencyGraph:
    """One graph per source state and process; a worktree graph is a snapshot."""
    key = (root.resolve(), commit)
    if key not in _GRAPHS:
        _GRAPHS[key] = load_graph(key[0], commit=commit)
    return _GRAPHS[key]


def load_graph(
    root: Path,
    *,
    commit: str | None = None,
    cache: Path | None | Callable[[], Path] = default_cache_path,
) -> DependencyGraph:
    tree = (
        RepositoryTree.from_commit(root, commit)
        if commit is not None
        else RepositoryTree.from_worktree(root)
    )
    return build_graph(tree, cache=cache() if callable(cache) else cache)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n", 1)[0])
    parser.add_argument("paths", nargs="*", help="changed repository paths")
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument("--commit", help="read this commit's tree, not the worktree")
    parser.add_argument(
        "--dependencies-of", metavar="TEST", help="list what selects one test file"
    )
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)
    started = time.perf_counter()
    graph = load_graph(args.root.resolve(), commit=args.commit)
    if args.dependencies_of:
        rows = sorted(graph.dependencies(args.dependencies_of))
        print("\n".join(rows))
        return 0
    selected = graph.select(args.paths)
    elapsed = round(time.perf_counter() - started, 3)
    if args.json:
        print(
            json.dumps(
                {"selected": selected, "stats": {**graph.stats, "seconds": elapsed}},
                indent=1,
            )
        )
    else:
        print("\n".join(selected))
        print(
            f"test-dependency-graph: {len(selected)} of {len(graph.tests)} test "
            f"files in {elapsed}s ({graph.stats})",
            file=sys.stderr,
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
