from __future__ import annotations

from pathlib import Path

from tests.wasm_linked_runner import (
    build_wasm_linked,
    require_wasm_toolchain,
    run_wasm_linked,
)


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
        "y = True\n"
        "print(y)\n"
        "if y:\n"
        "    print('a')\n"
        "else:\n"
        "    print('b')\n",
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
