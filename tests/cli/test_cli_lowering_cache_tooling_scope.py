"""The frontend semantic fingerprint keys exactly the declared producer code.

``_FRONTEND_COMPILER_INPUTS`` in molt.cli.cache_fingerprints declares every
compiler source that a frontend producer imports, and why it is in or out of
the key (docs/design/frontend_semantic_fingerprint.md). These tests prove that:

* the declaration is closed: every import that producer code can execute names
  a declared module (a static walk over every scope), and every import that a
  real lowering executes names one too (a fresh process records each import
  statement);
* an edit to any non-keyed entry leaves the key unchanged, and an edit to any
  keyed entry changes it;
* the key covers what changes output: a producer edit that changes the IR of a
  fixed program changes the key, while infrastructure edits change neither;
* the build-environment values that excluded modules compute reach a lowering
  record only through its key.
"""

from __future__ import annotations

import ast
import importlib
import importlib.util
import json
import os
import shutil
import sys
from pathlib import Path

import pytest

from tests.process_guard_common import run_guarded_test_process

CF = importlib.import_module("molt.cli.cache_fingerprints")
MC = importlib.import_module("molt.cli.module_cache")
MS = importlib.import_module("molt.cli.module_source")

ROLE = CF._FrontendInputRole
INPUTS = CF._FRONTEND_COMPILER_INPUTS
# The loaded compiler's own ``src/molt``; the declaration names paths under it.
MOLT = Path(CF.__file__).resolve().parents[1]
ROOT = MOLT.parents[1]


def _module_files() -> dict[str, Path]:
    """Every importable ``molt`` module of the loaded compiler, by name."""
    modules: dict[str, Path] = {}
    for path in MOLT.rglob("*.py"):
        if "__pycache__" in path.parts:
            continue
        parts = path.relative_to(MOLT.parent).with_suffix("").parts
        if parts[-1] == "__init__":
            parts = parts[:-1]
        modules[".".join(parts)] = path
    return modules


def _declared_role(path: Path) -> ROLE | None:
    relative = path.relative_to(MOLT).as_posix()
    role = INPUTS.get(relative)
    if role is not None:
        return role
    for entry, entry_role in INPUTS.items():
        if (MOLT / entry).is_dir() and relative.startswith(f"{entry}/"):
            return entry_role
    return None


def _is_type_checking(test: ast.expr) -> bool:
    return (isinstance(test, ast.Name) and test.id == "TYPE_CHECKING") or (
        isinstance(test, ast.Attribute) and test.attr == "TYPE_CHECKING"
    )


_DYNAMIC_IMPORT_CALLS = frozenset(
    {
        "__import__",
        "import_module",
        "spec_from_file_location",
        "spec_from_loader",
        "run_module",
        "run_path",
    }
)
_IMPORT_MODULES = frozenset({"builtins", "importlib", "importlib.util", "runpy"})
_IMPORT_ANCHORS = frozenset({"__package__", "__spec__", "__path__", "__name__"})


def _is_dynamic_import_call(func: ast.expr) -> bool:
    """``__import__(...)``, ``importlib.import_module(...)`` and their kin.

    A bare name counts (``from importlib import import_module``); an attribute
    counts only on an import machinery module, not on any object's method.
    """
    if isinstance(func, ast.Name):
        return func.id in _DYNAMIC_IMPORT_CALLS
    return (
        isinstance(func, ast.Attribute)
        and func.attr in _DYNAMIC_IMPORT_CALLS
        and ast.unparse(func.value) in _IMPORT_MODULES
    )


def _static_imports(
    path: Path, module: str, modules: dict[str, Path]
) -> tuple[set[str], list[str]]:
    """Modules that ``path`` can import when it runs, and dynamic-import sites.

    Every scope counts: a deferred import runs when its function does. Only
    ``if TYPE_CHECKING:`` bodies are skipped, since they never execute. An
    import also runs each parent package, so parents count as imports.
    """
    tree = ast.parse(path.read_bytes(), filename=str(path))
    package = module if path.name == "__init__.py" else module.rpartition(".")[0]
    found: set[str] = set()
    dynamic: list[str] = []

    def add(name: str) -> None:
        parts = name.split(".")
        for length in range(1, len(parts) + 1):
            candidate = ".".join(parts[:length])
            if candidate in modules:
                found.add(candidate)

    def visit(node: ast.AST) -> None:
        if isinstance(node, ast.If) and _is_type_checking(node.test):
            for child in node.orelse:
                visit(child)
            return
        if isinstance(node, ast.Import):
            for alias in node.names:
                if alias.name == "molt" or alias.name.startswith("molt."):
                    add(alias.name)
        elif isinstance(node, ast.ImportFrom):
            base = (
                importlib.util.resolve_name(
                    "." * node.level + (node.module or ""), package
                )
                if node.level
                else node.module or ""
            )
            if base == "molt" or base.startswith("molt."):
                add(base)
                for alias in node.names:
                    if f"{base}.{alias.name}" in modules:
                        add(f"{base}.{alias.name}")
        elif isinstance(node, ast.Call) and _is_dynamic_import_call(node.func):
            dynamic.append(f"{path.name}:{node.lineno} {ast.unparse(node.func)}()")
        elif isinstance(node, (ast.Assign, ast.AnnAssign, ast.AugAssign)):
            targets = node.targets if isinstance(node, ast.Assign) else [node.target]
            for target in targets:
                if isinstance(target, ast.Name) and target.id in _IMPORT_ANCHORS:
                    dynamic.append(f"{path.name}:{node.lineno} {target.id} =")
        for child in ast.iter_child_nodes(node):
            visit(child)

    visit(tree)
    found.discard(module)
    return found, dynamic


def test_every_import_a_producer_can_run_is_declared() -> None:
    modules = _module_files()
    producers = {
        name: path
        for name, path in modules.items()
        if _declared_role(path) is ROLE.PRODUCER
    }
    assert "molt.frontend" in producers and "molt.cli.module_cache" in producers
    undeclared: dict[str, list[str]] = {}
    dynamic: list[str] = []
    imported: set[str] = set()
    for name, path in sorted(producers.items()):
        targets, sites = _static_imports(path, name, modules)
        dynamic.extend(sites)
        imported |= targets
        for target in sorted(targets):
            if _declared_role(modules[target]) is None:
                undeclared.setdefault(target, []).append(name)
    assert not undeclared, (
        "frontend producers import modules that _FRONTEND_COMPILER_INPUTS does "
        f"not classify (importer lists): {undeclared}"
    )
    # A computed module name or a rebound anchor escapes this walk; the
    # declaration is only closed while producers import by literal name.
    assert not dynamic, f"producers import by computed name: {dynamic}"
    missing = sorted(entry for entry in INPUTS if not (MOLT / entry).exists())
    assert not missing, f"declared compiler inputs do not exist: {missing}"
    imported_paths = {modules[name].relative_to(MOLT).as_posix() for name in imported}
    unused = sorted(
        entry
        for entry, role in INPUTS.items()
        if role is not ROLE.PRODUCER
        and role is not ROLE.REFERENCE
        and entry not in imported_paths
    )
    assert not unused, f"no producer imports these declared entries: {unused}"


def test_reference_sources_are_the_gpu_descriptor_references() -> None:
    from molt.frontend.lowering.gpu_kernel_descriptor import (
        GPU_PYTHON_BODY_REFERENCES,
    )

    assert set(CF._FRONTEND_REFERENCE_SOURCES) == {
        relative for relative, _ in GPU_PYTHON_BODY_REFERENCES.values()
    }


# Each import statement the lowering executes, recorded by the importing module.
_RUNTIME_PROBE = r"""
import builtins, importlib.util, json, os, sys

edges, reads = set(), set()
real_import = builtins.__import__


def molt_frame_name(frame):
    while frame is not None:
        name = frame.f_globals.get("__name__", "")
        if name == "molt" or name.startswith("molt."):
            return name
        frame = frame.f_back
    return None


def recording_import(name, globals=None, locals=None, fromlist=(), level=0):
    module = real_import(name, globals, locals, fromlist, level)
    importer = (globals or {}).get("__name__", "")
    if importer == "molt" or importer.startswith("molt."):
        package = globals.get("__package__") or importer.rpartition(".")[0]
        absolute = importlib.util.resolve_name("." * level + name, package) if level else name
        if absolute != "molt" and not absolute.startswith("molt."):
            return module
        parts = absolute.split(".")
        for length in range(1, len(parts) + 1):
            edges.add((importer, ".".join(parts[:length])))
        for item in fromlist or ():
            if f"{absolute}.{item}" in sys.modules:
                edges.add((importer, f"{absolute}.{item}"))
    return module


def audit(event, args):
    # Data a producer reads: an "open" for reading that is not module source.
    if event != "open" or not args or not isinstance(args[0], (str, os.PathLike)):
        return
    mode = args[1] if len(args) > 1 else None
    flags = args[2] if len(args) > 2 else 0
    if isinstance(mode, str):
        writes = any(char in mode for char in "wax+")
    else:
        writes = bool(flags & (os.O_WRONLY | os.O_RDWR))
    path = os.fspath(args[0])
    if writes or path.endswith((".py", ".pyc")):
        return
    reader = molt_frame_name(sys._getframe(1))
    if reader is not None:
        reads.add((reader, path))


builtins.__import__ = recording_import
sys.addaudithook(audit)
from molt.frontend import compile_to_tir

ir = compile_to_tir(sys.stdin.read())
print(json.dumps({
    "functions": len(ir["functions"]),
    "edges": sorted(edges),
    "reads": sorted(reads),
    "loaded": sorted(name for name in sys.modules if name.startswith("molt.")),
}))
"""

_PROGRAM = """import json
from dataclasses import dataclass


class Account:
    def __init__(self, owner, balance=0):
        self.__owner = owner
        self.balance = balance

    @property
    def owner(self):
        return self.__owner


@dataclass
class Point:
    x: int
    y: int = 0


def squares(limit):
    for item in range(limit):
        yield item * item


async def countdown(limit):
    while limit:
        yield limit
        limit -= 1


def main():
    account = Account("ada", 3)
    even = {index: value for index, value in enumerate(squares(4)) if value % 2 == 0}
    try:
        with open(__file__) as handle:
            first = handle.readline()
    except OSError as exc:
        first = str(exc)
    match account.balance:
        case 3:
            label = "three"
        case _:
            label = "other"
    total = sum(value for value in even.values())
    print(json.dumps({"owner": account.owner}), Point(total), f"{total:>4}", label, first)


main()
"""


def _compiler_process_env() -> dict[str, str]:
    env = dict(os.environ)
    env.pop("MOLT_SOURCE_ROOT", None)
    env.pop("PYTHONPATH", None)
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    return env


def _run_with_compiler(src_root: Path, script: str, stdin: str = "") -> dict:
    """Run ``script`` in a fresh interpreter that imports only ``src_root``.

    ``-I -S`` keeps the checkout's site hook from importing another ``molt``.
    ``-I`` also ignores PYTHONDONTWRITEBYTECODE, so the bootstrap turns bytecode
    writes off: the probe must never write into the compiler tree it reads.
    """
    bootstrap = (
        "import sys; sys.dont_write_bytecode = True; "
        f"sys.path.insert(0, {str(src_root)!r})\n" + script
    )
    completed = run_guarded_test_process(
        [sys.executable, "-I", "-S", "-c", bootstrap],
        input=stdin,
        env=_compiler_process_env(),
        timeout=300,
    )
    assert completed.returncode == 0, completed.stderr
    return json.loads(completed.stdout)


def test_every_import_the_lowering_runs_is_declared() -> None:
    result = _run_with_compiler(MOLT.parent, _RUNTIME_PROBE, _PROGRAM)
    modules = _module_files()
    # Non-vacuous: the probe lowered the program through non-frontend producers.
    assert result["functions"] >= 5
    importers = {importer for importer, _ in result["edges"]}
    assert {"molt.frontend", "molt.python_private_names"} <= set(result["loaded"])
    assert "molt.frontend" in importers
    undeclared = sorted(
        (importer, target)
        for importer, target in result["edges"]
        if _declared_role(modules[importer]) is ROLE.PRODUCER
        and _declared_role(modules[target]) is None
    )
    assert not undeclared, f"the lowering imports undeclared modules: {undeclared}"
    keyed_roots = [MOLT / entry for entry, role in INPUTS.items() if role.keyed]
    unkeyed_reads = sorted(
        (reader, path)
        for reader, path in result["reads"]
        if _declared_role(modules[reader]) is ROLE.PRODUCER
        and not any(
            Path(path).resolve() == root.resolve()
            or Path(path).resolve().is_relative_to(root.resolve())
            for root in keyed_roots
        )
    )
    assert not unkeyed_reads, f"producers read data outside the key: {unkeyed_reads}"


def _write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


@pytest.fixture
def declared_tree(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> Path:
    """A compiler root that holds one small file per declared entry."""
    molt = tmp_path / "src" / "molt"
    for entry in INPUTS:
        target = molt / entry
        if (MOLT / entry).is_dir():
            target = target / "__init__.py"
        _write(target, "MARKER = 1\n")
    _write(molt / "cli" / "link_pipeline.py", "MARKER = 1\n")
    monkeypatch.setattr(CF, "_compiler_root", lambda: tmp_path)
    CF._SOURCE_TREE_CONTENT_DIGEST_CACHE.clear()
    return molt


def _edit(path: Path) -> None:
    with path.open("a", encoding="utf-8") as handle:
        handle.write("MARKER = 2  # edited\n")


def test_unkeyed_edits_keep_the_key_and_keyed_edits_change_it(
    declared_tree: Path,
) -> None:
    def key() -> str:
        return CF._frontend_semantic_tooling_fingerprint()

    baseline = key()
    kept: list[str] = []
    for entry, role in INPUTS.items():
        if role.keyed:
            continue
        _edit(declared_tree / entry)
        if key() != baseline:
            kept.append(entry)
    # An undeclared module (here a link-stage driver) is not a lowering input.
    _edit(declared_tree / "cli" / "link_pipeline.py")
    assert key() == baseline
    assert not kept, f"non-keyed edits changed the frontend key: {kept}"
    previous = baseline
    for entry, role in INPUTS.items():
        if not role.keyed:
            continue
        target = declared_tree / entry
        _edit(target / "__init__.py" if target.is_dir() else target)
        current = key()
        assert current != previous, f"editing keyed input {entry} kept the key"
        previous = current
    # A new file inside a keyed package is new producer code.
    _write(declared_tree / "frontend" / "lowering" / "added.py", "VALUE = 1\n")
    assert key() != previous


def test_module_analysis_record_key_uses_the_declared_key(declared_tree: Path) -> None:
    module_path = declared_tree.parents[1] / "program.py"
    module_path.write_text("VALUE = 1\n", encoding="utf-8")

    def analysis_key() -> str:
        return MC._module_analysis_record_key(
            module_path,
            module_name="program",
            is_package=False,
            import_scan_mode="full",
            target_python=MC._DEFAULT_TARGET_PYTHON_VERSION,
        )

    before = analysis_key()
    _edit(declared_tree / "toolchain_identity.py")
    _edit(declared_tree / "cli" / "build_diagnostics.py")
    assert analysis_key() == before
    _edit(declared_tree / "cli" / "frontend_pipeline.py")
    assert analysis_key() != before


def test_semantic_fingerprint_differs_from_broad_on_real_tree() -> None:
    assert (
        CF._frontend_semantic_tooling_fingerprint() != CF._cache_tooling_fingerprint()
    )


_KEY_AND_IR_PROBE = r"""
import hashlib, json, sys
from molt.cli import cache_fingerprints
from molt.frontend import compile_to_tir

ir = compile_to_tir(sys.stdin.read())
print(json.dumps({
    "root": str(cache_fingerprints._compiler_root()),
    "key": cache_fingerprints._frontend_semantic_tooling_fingerprint(),
    "ir": hashlib.sha256(json.dumps(ir, sort_keys=True, default=str).encode()).hexdigest(),
}))
"""


def test_key_changes_whenever_a_compiler_edit_changes_lowered_ir(
    tmp_path: Path,
) -> None:
    """Independent oracle: lower one program with an edited copy of the compiler."""
    copy = tmp_path / "compiler"
    # The guest stdlib (7.5 MB) is not lowered here; keep only its reference.
    shutil.copytree(
        MOLT,
        copy / "src" / "molt",
        ignore=lambda directory, names: [
            name
            for name in names
            if name == "__pycache__" or (Path(directory) == MOLT and name == "stdlib")
        ],
    )
    (copy / "src/molt/stdlib").mkdir()
    shutil.copy2(MOLT / "stdlib" / "struct.py", copy / "src/molt/stdlib/struct.py")

    def lower() -> dict:
        result = _run_with_compiler(copy / "src", _KEY_AND_IR_PROBE, _PROGRAM)
        assert Path(result["root"]).resolve() == copy.resolve()
        return result

    original = lower()
    # The key is content-addressed: the copy keys exactly as the checkout does.
    assert original["key"] == CF._frontend_semantic_tooling_fingerprint()
    for entry in (
        "toolchain_identity.py",
        "file_publication.py",
        "input_report.py",
        "cli/build_diagnostics.py",
        "cli/external_native.py",
        "cli/cache_fingerprints.py",
    ):
        assert INPUTS[entry].keyed is False
        with (copy / "src/molt" / entry).open("a", encoding="utf-8") as handle:
            handle.write("\nEDITED_FOR_KEY_ORACLE = 1\n")
    infrastructure = lower()
    assert infrastructure == original
    # A producer outside frontend/: CPython's private-name mangling rule.
    mangling = copy / "src/molt/python_private_names.py"
    source = mangling.read_text(encoding="utf-8")
    rule = 'return f"_{private}{name}" if private else name'
    assert source.count(rule) == 1
    mangling.write_text(
        source.replace(rule, 'return f"_{private}_{name}" if private else name'),
        encoding="utf-8",
    )
    producer = lower()
    assert producer["ir"] != original["ir"], "the producer edit did not change IR"
    assert producer["key"] != original["key"], "IR changed but the key did not"


def _lowering_context(module_path: Path, **changes: object) -> str | None:
    options: dict[str, object] = {
        "source_lease": MS._ModuleSourceLease.path_backed(module_path),
        "logical_source_path": str(module_path),
        "module_execution_kind": "imported",
        "module_source_role": "user",
        "known_classes_snapshot": {},
        "type_hint_policy": "ignore",
        "type_facts": None,
        "enable_phi": True,
        "known_modules": {"main", "alpha"},
        "direct_call_modules": set(),
        "stdlib_allowlist": set(),
        "known_func_defaults": {},
        "known_func_kinds": {},
        "native_callable_exports": None,
        "module_deps": {"main": {"alpha"}, "alpha": set()},
        "module_is_namespace": False,
        "module_chunking": False,
        "module_chunk_max_ops": 0,
        "optimization_profile": "dev",
        "pgo_hot_function_names": (),
    }
    options.update(changes)
    return MC._module_lowering_context_digest_for_module("main", module_path, **options)


@pytest.mark.parametrize(
    "field, value",
    [
        # cli/external_native.py: import admission and direct calls.
        ("direct_call_modules", {"alpha"}),
        ("known_modules", {"main"}),
        # cli/module_stdlib_policy.py reads the stdlib matrix document.
        ("stdlib_allowlist", {"json"}),
        # cli/build_request.py request fields.
        ("type_hint_policy", "trust"),
        ("optimization_profile", "release"),
    ],
)
def test_build_environment_values_reach_lowering_only_through_its_key(
    tmp_path: Path, field: str, value: object
) -> None:
    module_path = tmp_path / "main.py"
    module_path.write_text("import alpha\nprint(alpha)\n", encoding="utf-8")
    baseline = _lowering_context(module_path)
    assert baseline is not None
    assert _lowering_context(module_path, **{field: value}) != baseline


def test_import_molt_cli_does_not_load_backend() -> None:
    """``import molt.cli`` must not pull the backend / native-link / cargo layer.

    The frontend imports ``molt.cli`` (its ``__init__``) as a parent package, so
    an eager backend import there would load the backend on every frontend
    import. Runs in a fresh subprocess so modules an earlier test imported do
    not hide the failure.
    """
    probe = (
        "import sys, molt.cli\n"
        "backend = sorted(\n"
        "    m for m in sys.modules\n"
        "    if m.startswith('molt.cli.backend_')\n"
        "    or m.startswith('molt.cli.cargo')\n"
        "    or m in {\n"
        "        'molt.cli.link_pipeline', 'molt.cli.wasm',\n"
        "        'molt.cli.mlir_backend', 'molt.cli.native_toolchain',\n"
        "        'molt.cli.native_binary', 'molt.cli.native_link_command',\n"
        "        'molt.cli.runtime_build', 'molt.cli.runtime_native_build',\n"
        "        'molt.cli.runtime_wasm_build', 'molt.cli.runtime_wasm_build_policy',\n"
        "        'molt.cli.runtime_wasm_build_spec', 'molt.cli.runtime_wasm_build_support',\n"
        "        'molt.cli.runtime_wasm_pair_build', 'molt.cli.setup_readiness',\n"
        "    }\n"
        ")\n"
        "assert not backend, 'backend loaded on import molt.cli: ' + repr(backend)\n"
        "before = set(sys.modules)\n"
        "assert '__getattr__' not in vars(molt.cli), 'molt.cli forwards attributes'\n"
        "assert not hasattr(molt.cli, 'backend_binary')\n"
        "assert set(sys.modules) == before, 'attribute lookup imported a module'\n"
        "from molt.cli import backend_binary\n"
        "assert molt.cli.backend_binary is backend_binary\n"
        "print('OK')\n"
    )
    env = {**os.environ, "PYTHONPATH": str(ROOT / "src")}
    result = run_guarded_test_process(
        [sys.executable, "-c", probe],
        capture_output=True,
        text=True,
        env=env,
    )
    assert result.returncode == 0, (
        f"import-smoke failed:\nstdout={result.stdout}\nstderr={result.stderr}"
    )


# Files that run only after a module is lowered (command dispatch, extension
# sealing, build locking, the backend daemon, debug tooling).
_COMMAND_LAYER_OUT = (
    "cli/__init__.py",
    "cli/build_locks.py",
    "source_build_environment.py",
    "cli/extension_seal.py",
    "cli/extension_scan.py",
    "cli/package_registry.py",
    "cli/profile_feedback.py",
    "cli/runtime_features.py",
    "cli/debug_helpers.py",
    "backend_daemon_custody.py",
    "debug/reduce.py",
    "debug/ir.py",
)


def test_installed_admission_contract_has_no_eager_producer_dependencies() -> None:
    from molt.cli.python_import_resolution import PythonImportPolicy
    from molt.cli.python_source_closure import local_python_import_closure

    contract = ROOT / "src/molt/cli/installed_runtime_contract.py"
    closure = local_python_import_closure(
        ROOT,
        (contract,),
        policy=PythonImportPolicy(
            module_level_only=True,
            include_parent_packages=False,
            fail_on_nonliteral_dynamic_import=True,
            allowed_prefix="molt",
            purpose="source_dependency",
            unknown_relative_sources="local_inventory",
        ),
        search_roots=(ROOT / "src",),
    )
    rels = {path.relative_to(ROOT / "src/molt").as_posix() for path in closure.paths}
    assert "cli/installed_runtime_contract.py" in rels
    assert "cli/runtime_identity_schema.py" in rels
    assert not set(_COMMAND_LAYER_OUT) & rels
    assert (
        not {
            "cli/installed_runtime.py",
            "cli/runtime_build_identity.py",
            "artifact_publication.py",
        }
        & rels
    )
