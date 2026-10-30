from __future__ import annotations

from pathlib import Path
import shutil

import pytest

from tests.wasm_linked_runner import (
    ModuleEntry,
    _run_wasm_test_process,
    build_wasm_linked,
    require_node_binary,
    require_wasm_build_toolchain,
    require_wasm_toolchain,
    run_cpython_reference,
    run_wasm_linked,
)
from molt.wasm_artifact import wasm_runtime_manifest_path


def test_wasm_runtime_resolved_intrinsics_return_boxed_none(tmp_path: Path) -> None:
    require_wasm_toolchain()
    root = Path(__file__).resolve().parents[1]
    src = root / "tests/fixtures/intrinsic_boxed_none.py"
    output = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output)
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.splitlines() == ["boxed-none-ok"]


def test_wasm_importlib_import_forms_and_os_sys_bootstrap_smoke(
    tmp_path: Path,
) -> None:
    require_wasm_toolchain()
    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "bootstrap_probe.py"
    src.write_text(
        "import importlib\n"
        "from importlib import machinery\n"
        "import os\n"
        "import sys\n\n"
        "print(importlib.__name__)\n"
        "print(machinery.__name__)\n"
        'print(sys.modules["os"] is os)\n'
        'print(sys.modules["sys"] is sys)\n',
        encoding="utf-8",
    )

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)

    assert run.returncode == 0, run.stderr
    assert run.stdout.splitlines() == [
        "importlib",
        "importlib.machinery",
        "True",
        "True",
    ]


def _write_package_main_probe(tmp_path: Path) -> Path:
    pkg = tmp_path / "probe_pkg"
    (pkg / "subpkg").mkdir(parents=True)
    (pkg / "__init__.py").write_text("", encoding="utf-8")
    (pkg / "sibling.py").write_text("SIBLING = 'sibling-ok'\n", encoding="utf-8")
    (pkg / "helper.py").write_text(
        "import os\n"
        "from .sibling import SIBLING\n\n"
        "class Helper:\n"
        "    def describe(self):\n"
        "        return 'helper-ok'\n\n"
        "def ping():\n"
        "    return SIBLING\n",
        encoding="utf-8",
    )
    (pkg / "subpkg" / "__init__.py").write_text("", encoding="utf-8")
    (pkg / "subpkg" / "leaf.py").write_text(
        "import os\n"
        "import sys\n\n"
        "def describe_leaf():\n"
        "    return (__name__, __package__, sys.__name__, os.__name__)\n",
        encoding="utf-8",
    )
    src = pkg / "__main__.py"
    src.write_text(
        "import os as os_alias\n"
        "import sys as sys_alias\n"
        "import probe_pkg.helper as helper_alias\n"
        "from .helper import Helper as HelperAlias\n"
        "from .subpkg.leaf import describe_leaf as describe_leaf_alias\n\n"
        "print(__name__)\n"
        "print(__package__)\n"
        'print(sys_alias.modules["__main__"] is sys_alias.modules[__name__])\n'
        'print(sys_alias is sys_alias.modules["sys"])\n'
        'print(os_alias is sys_alias.modules["os"])\n'
        'print(helper_alias is sys_alias.modules["probe_pkg.helper"])\n'
        "print(HelperAlias is helper_alias.Helper)\n"
        "print(describe_leaf_alias())\n"
        "print(helper_alias.ping())\n"
        "print(sys_alias.__name__)\n"
        "print(os_alias.__name__)\n",
        encoding="utf-8",
    )
    return src


def test_wasm_importlib_package_main_alias_identity_smoke(
    tmp_path: Path,
) -> None:
    """``python -m probe_pkg``: the package is the program.

    Lib/runpy.py:_run_module_as_main imports the package, runs
    ``probe_pkg.__main__`` as ``__main__`` with ``__package__ == "probe_pkg"``,
    and the entry's submodules import their siblings (helper imports
    .sibling). The package root is an explicit module root, so the build must
    still compile the package's whole closure. CPython is the oracle.
    """
    require_wasm_toolchain()
    root = Path(__file__).resolve().parents[1]
    _write_package_main_probe(tmp_path / "src")
    entry = ModuleEntry("probe_pkg", tmp_path / "src")

    expected = run_cpython_reference(entry)
    assert expected.stdout.splitlines() == [
        "__main__",
        "probe_pkg",
        "True",
        "True",
        "True",
        "True",
        "True",
        "('probe_pkg.subpkg.leaf', 'probe_pkg.subpkg', 'sys', 'os')",
        "sibling-ok",
        "sys",
        "os",
    ]
    output_wasm = build_wasm_linked(root, entry, tmp_path / "out")
    run = run_wasm_linked(root, output_wasm)

    assert run.returncode == 0, run.stderr
    assert run.stdout == expected.stdout


def test_wasm_importlib_package_main_as_script_rejects_relative_import(
    tmp_path: Path,
) -> None:
    """``python probe_pkg/__main__.py`` runs a script, not the package.

    A script has no parent package, so its first relative import raises
    ImportError (Python/import.c:resolve_name). Molt builds a file entry as a
    script and must fail the same way. CPython is the oracle.
    """
    require_wasm_toolchain()
    root = Path(__file__).resolve().parents[1]
    pkg = tmp_path / "src" / "probe_pkg"
    pkg.mkdir(parents=True)
    (pkg / "__init__.py").write_text("", encoding="utf-8")
    (pkg / "helper.py").write_text("VALUE = 'helper-ok'\n", encoding="utf-8")
    src = pkg / "__main__.py"
    src.write_text(
        "print('script-start')\nfrom . import helper\nprint(helper.VALUE)\n",
        encoding="utf-8",
    )

    expected = run_cpython_reference(src, check=False)
    assert expected.stdout == "script-start\n"
    assert expected.returncode == 1
    assert expected.stderr.splitlines()[-1] == (
        "ImportError: attempted relative import with no known parent package"
    )
    output_wasm = build_wasm_linked(root, src, tmp_path / "out")
    run = run_wasm_linked(root, output_wasm)

    assert run.returncode == expected.returncode, run.stdout + run.stderr
    assert run.stdout == expected.stdout
    assert run.stderr.splitlines()[-1] == expected.stderr.splitlines()[-1]


@pytest.mark.parametrize("module", [True, False], ids=["module", "package-script"])
def test_wasm_entry_search_path_matches_cpython(tmp_path: Path, module: bool) -> None:
    # Select the existing Node host explicitly; this cell does not require
    # a prebuilt Rust host or infer support from a failed host launch.
    require_wasm_build_toolchain()
    node = require_node_binary()
    root = Path(__file__).resolve().parents[1]
    source_root = tmp_path / "program"
    shutil.copytree(root / "tests/fixtures/entry_search_path", source_root)
    # Make project discovery see the enclosing package's parent as well.
    (source_root / "pyproject.toml").write_text("", encoding="utf-8")
    entry = (
        ModuleEntry("probe_pkg", source_root)
        if module
        else source_root / "probe_pkg/script.py"
    )
    expected = run_cpython_reference(entry, cwd=source_root, check=False)
    assert expected.returncode == 1
    assert expected.stdout.splitlines() == (
        ["self-import-ok self-import-ok", "before-missing-helper"]
        if module
        else ["self-import-ok", "before-missing-package"]
    )
    missing = "helper" if module else "probe_pkg"
    assert f"ModuleNotFoundError: No module named '{missing}'" in expected.stderr

    output = build_wasm_linked(root, entry, tmp_path / "out", cwd=source_root)
    run = _run_wasm_test_process(
        [node, str(root / "wasm/run_wasm.js"), str(wasm_runtime_manifest_path(output))],
        role="guest-host",
        cwd=source_root,
    )
    # HF-595 may change the predicate error during the unresolved runtime
    # import. Prove the import failed after the successful imports above.
    assert run.returncode == expected.returncode, run.stdout + run.stderr
    assert run.stdout == expected.stdout


def test_wasm_importlib_top_level_import_runs_module_body(tmp_path: Path) -> None:
    require_wasm_toolchain()
    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "probe_import.py"
    src.write_text('import importlib\nprint("hi")\n', encoding="utf-8")

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)

    assert run.returncode == 0, run.stderr
    assert run.stdout.splitlines() == ["hi"]


def test_wasm_importlib_machinery_direct_import_runs_module_body(
    tmp_path: Path,
) -> None:
    require_wasm_toolchain()
    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "probe_machinery.py"
    src.write_text('import importlib.machinery\nprint("hi")\n', encoding="utf-8")

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)

    assert run.returncode == 0, run.stderr
    assert run.stdout.splitlines() == ["hi"]


def test_wasm_linked_loader_style_bootstrap_runs_class_init_and_type_name(
    tmp_path: Path,
) -> None:
    require_wasm_toolchain()
    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "loader_probe.py"
    src.write_text(
        "from __future__ import annotations\n\n"
        "class _MoltLoader:\n"
        '    def create_module(self, _spec: "ModuleSpec"):\n'
        "        return None\n\n"
        "class BuiltinImporter(_MoltLoader):\n"
        "    pass\n\n"
        "_MOLT_LOADER = BuiltinImporter()\n"
        "print(type(_MOLT_LOADER).__name__)\n",
        encoding="utf-8",
    )

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)

    assert run.returncode == 0, run.stderr
    assert run.stdout.splitlines() == ["BuiltinImporter"]


def test_wasm_linked_future_annotation_return_none_runs_module_body(
    tmp_path: Path,
) -> None:
    require_wasm_toolchain()
    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "annotate_probe.py"
    src.write_text(
        "from __future__ import annotations\n\n"
        "class A:\n"
        "    def f(self) -> None:\n"
        "        return None\n\n"
        'print("hi")\n',
        encoding="utf-8",
    )

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)

    assert run.returncode == 0, run.stderr
    assert run.stdout.splitlines() == ["hi"]


def test_wasm_linked_loader_annotation_shape_runs_module_body(
    tmp_path: Path,
) -> None:
    require_wasm_toolchain()
    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "loader_shape_probe.py"
    src.write_text(
        "from __future__ import annotations\n\n"
        "class _MoltLoader:\n"
        '    def create_module(self, _spec: "ModuleSpec"):\n'
        "        return None\n\n"
        "    def exec_module(self, module) -> None:\n"
        "        return None\n\n"
        'print("hi")\n',
        encoding="utf-8",
    )

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)

    assert run.returncode == 0, run.stderr
    assert run.stdout.splitlines() == ["hi"]


def test_wasm_linked_abcmeta_import_preserves_class_identity(
    tmp_path: Path,
) -> None:
    require_wasm_toolchain()
    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "abcmeta_probe.py"
    src.write_text(
        "import abc as _abc\n"
        "print(type(_abc.ABCMeta).__name__)\n"
        "print(isinstance(_abc.ABCMeta, type))\n"
        "print(issubclass(_abc.ABCMeta, type))\n",
        encoding="utf-8",
    )

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)

    assert run.returncode == 0, run.stderr
    assert run.stdout.splitlines() == ["type", "True", "True"]


def test_wasm_linked_bool_truthiness_controls_if_branch(tmp_path: Path) -> None:
    require_wasm_toolchain()
    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "bool_truthiness_probe.py"
    src.write_text(
        "y = True\nprint(y)\nif y:\n    print('a')\nelse:\n    print('b')\n",
        encoding="utf-8",
    )

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)

    assert run.returncode == 0, run.stderr
    assert run.stdout.splitlines() == ["True", "a"]


def test_wasm_linked_caught_missing_intrinsic_does_not_poison_module_init(
    tmp_path: Path,
) -> None:
    require_wasm_toolchain()
    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "missing_intrinsic_probe.py"
    src.write_text(
        "from _intrinsics import require_intrinsic\n\n"
        "try:\n"
        "    require_intrinsic('molt_definitely_missing_probe')\n"
        "except RuntimeError:\n"
        "    print('caught')\n\n"
        "print('ok')\n",
        encoding="utf-8",
    )

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)

    assert run.returncode == 0, run.stderr
    assert run.stdout.splitlines() == ["caught", "ok"]


def test_wasm_linked_gpu_intrinsics_are_available_to_stdlib_wrapper(
    tmp_path: Path,
) -> None:
    require_wasm_toolchain()
    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "gpu_intrinsics_probe.py"
    src.write_text(
        "import _intrinsics\n"
        "from molt.gpu.tensor import Tensor\n\n"
        "for name in (\n"
        "    'molt_gpu_matmul_contiguous',\n"
        "    'molt_gpu_tensor__zeros',\n"
        "):\n"
        "    print(_intrinsics.load_intrinsic(name) is not None)\n",
        encoding="utf-8",
    )

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)

    assert run.returncode == 0, run.stderr
    assert run.stdout.splitlines() == ["True", "True"]


def test_wasm_linked_import_typing_runs_module_body(tmp_path: Path) -> None:
    require_wasm_toolchain()
    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "typing_probe.py"
    src.write_text('import typing\nprint("ok")\n', encoding="utf-8")

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)

    assert run.returncode == 0, run.stderr
    assert run.stdout.splitlines() == ["ok"]
