"""The intrinsic-first stdlib stubs match their generator and behave as gaps.

``tools/gen_stdlib_stubs.py`` owns every stub. These tests hold the committed
stubs to the generator byte for byte, then load every stub in one child
interpreter and prove the contract a stub promises: the import requires the
capability intrinsic, the module binds nothing, any attribute raises the
canonical gap error, a package lets the import system load its submodules, and
a platform-only module keeps CPython's import outcome off its platform.
"""

from __future__ import annotations

import json
from pathlib import Path
import runpy
import sys

import pytest

from tests.surface_process_guard import run_surface_test_process
from tools import gen_stdlib_stubs
from tools.generator_io import stale_outputs

ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = ROOT / "src" / "molt" / "stdlib"

# The namespace of a module loaded from a file, before its body binds names;
# a package also has ``__path__``.
MODULE_DUNDERS = frozenset(
    {
        "__builtins__",
        "__cached__",
        "__doc__",
        "__file__",
        "__loader__",
        "__name__",
        "__package__",
        "__spec__",
    }
)

# Runs in a fresh interpreter: the probe installs a fake ``_intrinsics`` module
# and patches ``sys.platform``, which must not leak into the test process.
_PROBE = r"""
import importlib.util
import json
import sys
import types


def install_intrinsics(available):
    calls = []

    def require_intrinsic(name, namespace=None):
        calls.append(name)
        if name not in available:
            raise RuntimeError(f"intrinsic unavailable: {name}")
        return available[name]

    module = types.ModuleType("_intrinsics")
    module.require_intrinsic = require_intrinsic
    sys.modules["_intrinsics"] = module
    return calls


def outcome(read):
    try:
        read()
    except Exception as exc:
        return [type(exc).__name__, str(exc), getattr(exc, "name", None)]
    return None


def load(name, path, platform, available, children):
    calls = install_intrinsics(available)
    real_platform = sys.platform
    sys.platform = platform
    try:
        spec = importlib.util.spec_from_file_location(name, path)
        module = importlib.util.module_from_spec(spec)
        try:
            spec.loader.exec_module(module)
        except Exception as exc:
            return {
                "raised": [type(exc).__name__, str(exc), getattr(exc, "name", None)],
                "calls": calls,
            }
    finally:
        sys.platform = real_platform
    return {
        "namespace": sorted(vars(module)),
        "calls": calls,
        "attribute": outcome(lambda: module.any_attribute),
        "submodules": {
            child: outcome(lambda: getattr(module, child)) for child in children
        },
    }


capability = {"molt_capabilities_has": lambda name=None: True}
report = {}
for name, path, home, foreign, children in json.load(sys.stdin):
    report[name] = {
        "with_intrinsic": load(name, path, home, capability, children),
        "without_intrinsic": load(name, path, home, {}, children),
        "foreign_platform": (
            None if foreign is None else load(name, path, foreign, capability, [])
        ),
    }
print(json.dumps(report))
"""


def _module_name(path: Path) -> str:
    parts = path.relative_to(STDLIB_ROOT).with_suffix("").parts
    return ".".join(parts[:-1] if parts[-1] == "__init__" else parts)


def _union_submodules() -> dict[str, list[str]]:
    union = runpy.run_path(str(ROOT / "tools" / "stdlib_module_union.py"))
    children: dict[str, list[str]] = {}
    for table in ("STDLIB_MODULE_UNION", "STDLIB_PY_SUBMODULE_UNION"):
        for module in union[table]:
            parent, dot, leaf = module.rpartition(".")
            if dot:
                children.setdefault(parent, []).append(leaf)
    return {parent: sorted(leaves) for parent, leaves in children.items()}


def test_committed_stubs_match_the_generator() -> None:
    stale = stale_outputs(gen_stdlib_stubs.generated_outputs())
    assert [path.relative_to(ROOT).as_posix() for path in stale] == []


def test_every_stub_imports_as_an_intrinsic_backed_gap() -> None:
    stubs = sorted(gen_stdlib_stubs.generated_outputs())
    assert stubs, "the generator owns no stub"
    submodules = _union_submodules()
    rows = []
    for path in stubs:
        name = _module_name(path)
        home = gen_stdlib_stubs.PLATFORM_ONLY.get(name, sys.platform)
        foreign = (
            None
            if name not in gen_stdlib_stubs.PLATFORM_ONLY
            else ("linux" if home != "linux" else "darwin")
        )
        children = submodules.get(name, []) if path.name == "__init__.py" else []
        rows.append((name, str(path), home, foreign, children))
    proc = run_surface_test_process(
        [sys.executable, "-c", _PROBE],
        cwd=ROOT,
        input=json.dumps(rows),
        check=True,
    )
    report = json.loads(proc.stdout)
    assert sorted(report) == sorted(name for name, *_ in rows)

    failures = []
    for path, (name, _, _, foreign, children) in zip(stubs, rows):
        package = path.name == "__init__.py"
        kind = "package" if package else "module"
        namespace = (
            MODULE_DUNDERS | {"__getattr__"} | ({"__path__"} if package else set())
        )
        gap = (
            f'stdlib {kind} "{name}" is not fully lowered yet; '
            "only an intrinsic-first stub is available."
        )
        expected = {
            "with_intrinsic": {
                "namespace": sorted(namespace),
                "calls": ["molt_capabilities_has"],
                "attribute": ["RuntimeError", gap, None],
                "submodules": {
                    child: [
                        "AttributeError",
                        f"module '{name}' has no attribute '{child}'",
                        child,
                    ]
                    for child in children
                },
            },
            "without_intrinsic": {
                "raised": [
                    "RuntimeError",
                    "intrinsic unavailable: molt_capabilities_has",
                    None,
                ],
                "calls": ["molt_capabilities_has"],
            },
            "foreign_platform": (
                None
                if foreign is None
                else {
                    "raised": [
                        "ModuleNotFoundError",
                        f"No module named '{name}'",
                        name,
                    ],
                    "calls": [],
                }
            ),
        }
        failures.extend(
            f"{name} {scenario}: got {report[name][scenario]!r}, want {want!r}"
            for scenario, want in expected.items()
            if report[name][scenario] != want
        )
    assert failures == []


def _write_tree(root: Path, union: str, stubs: dict[str, str]) -> None:
    (root / "tools").mkdir(parents=True)
    (root / "tools" / "stdlib_module_union.py").write_text(union, encoding="utf-8")
    for relative, text in stubs.items():
        path = root / "src" / "molt" / "stdlib" / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")


@pytest.fixture
def stub_tree(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    monkeypatch.setattr(gen_stdlib_stubs, "ROOT", tmp_path)
    monkeypatch.setattr(gen_stdlib_stubs, "STDLIB_ROOT", tmp_path / "src/molt/stdlib")
    monkeypatch.setattr(
        gen_stdlib_stubs, "UNION", tmp_path / "tools" / "stdlib_module_union.py"
    )
    return tmp_path


_UNION = (
    'STDLIB_MODULE_UNION = ("alpha", "beta", "pkg")\n'
    'STDLIB_PACKAGE_UNION = ("pkg",)\n'
    'STDLIB_PY_SUBMODULE_UNION = ("pkg.sub", "pkg.inner.leaf")\n'
    'STDLIB_PY_SUBPACKAGE_UNION = ("pkg.inner",)\n'
)


def test_generator_writes_missing_union_modules_with_their_packages(
    stub_tree: Path,
) -> None:
    _write_tree(stub_tree, _UNION, {"alpha.py": "VALUE = 1\n"})

    outputs = gen_stdlib_stubs.generated_outputs()

    stdlib = stub_tree / "src" / "molt" / "stdlib"
    assert sorted(path.relative_to(stdlib).as_posix() for path in outputs) == [
        "beta.py",
        "pkg/__init__.py",
        "pkg/inner/__init__.py",
        "pkg/inner/leaf.py",
        "pkg/sub.py",
    ]
    assert 'stdlib package "pkg.inner"' in outputs[stdlib / "pkg/inner/__init__.py"]


def test_generator_refuses_a_stub_of_the_wrong_kind(stub_tree: Path) -> None:
    stub = gen_stdlib_stubs.stub_source("pkg", package=False)
    _write_tree(stub_tree, _UNION, {"pkg.py": stub})

    with pytest.raises(SystemExit) as excinfo:
        gen_stdlib_stubs.generated_outputs()

    assert (
        "src/molt/stdlib/pkg.py: CPython ships `pkg` as a package; "
        "git mv it to src/molt/stdlib/pkg/__init__.py"
    ) in str(excinfo.value)


def test_generator_refuses_a_stub_outside_the_union(stub_tree: Path) -> None:
    stub = gen_stdlib_stubs.stub_source("gamma", package=False)
    _write_tree(stub_tree, _UNION, {"gamma.py": stub})

    with pytest.raises(SystemExit) as excinfo:
        gen_stdlib_stubs.generated_outputs()

    assert "src/molt/stdlib/gamma.py: CPython ships no `gamma`; delete the stub" in (
        str(excinfo.value)
    )
