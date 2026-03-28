from __future__ import annotations

from pathlib import Path

from tests.wasm_linked_runner import (
    build_wasm_linked,
    require_wasm_toolchain,
    run_wasm_linked,
)


def test_wasm_linked_class_instantiation_preserves_type_name(tmp_path: Path) -> None:
    require_wasm_toolchain()
    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "class_probe.py"
    src.write_text(
        "class A:\n"
        "    pass\n\n"
        "print(type(A()).__name__)\n",
        encoding="utf-8",
    )

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)

    assert run.returncode == 0, run.stderr
    assert run.stdout.splitlines() == ["A"]


def test_wasm_linked_class_instantiation_runs_module_body(tmp_path: Path) -> None:
    require_wasm_toolchain()
    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "class_inst_probe.py"
    src.write_text(
        "class A:\n"
        "    pass\n\n"
        "A()\n"
        'print("hi")\n',
        encoding="utf-8",
    )

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)

    assert run.returncode == 0, run.stderr
    assert run.stdout.splitlines() == ["hi"]


def test_wasm_linked_plain_class_definition_runs_module_body(tmp_path: Path) -> None:
    require_wasm_toolchain()
    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "class_plain_probe.py"
    src.write_text(
        "class A:\n"
        "    pass\n\n"
        'print("hi")\n',
        encoding="utf-8",
    )

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)

    assert run.returncode == 0, run.stderr
    assert run.stdout.splitlines() == ["hi"]


def test_wasm_linked_class_method_definition_does_not_corrupt_stdout(
    tmp_path: Path,
) -> None:
    require_wasm_toolchain()
    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "class_method_probe.py"
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


def test_wasm_linked_class_function_attribute_definition_does_not_corrupt_stdout(
    tmp_path: Path,
) -> None:
    require_wasm_toolchain()
    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "class_func_attr_probe.py"
    src.write_text(
        "def f(self):\n"
        "    return None\n\n"
        "class A:\n"
        "    f = f\n\n"
        'print("hi")\n',
        encoding="utf-8",
    )

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)

    assert run.returncode == 0, run.stderr
    assert run.stdout.splitlines() == ["hi"]
