from pathlib import Path

from tests.wasm_linked_runner import (
    build_wasm_linked,
    require_wasm_toolchain,
    run_wasm_linked,
)


def test_wasm_control_flow_parity(tmp_path: Path) -> None:
    require_wasm_toolchain()

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "if_else.py"
    src.write_text("x = 1\nif x < 2:\n    print(1)\nelse:\n    print(2)\n")

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)
    assert run.returncode == 0, run.stderr
    assert run.stdout.strip() == "1"


def test_wasm_module_try_exception_loop_parity(tmp_path: Path) -> None:
    require_wasm_toolchain()

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "exception_loop.py"
    src.write_text(
        "i = 0\n"
        "total = 0\n"
        "while i < 5:\n"
        "    try:\n"
        "        if i == 3:\n"
        "            raise RuntimeError('boom')\n"
        "        total = total + i\n"
        "    except RuntimeError:\n"
        "        total = total + 100\n"
        "    i = i + 1\n"
        "print(total)\n"
    )

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)
    assert run.returncode == 0, run.stderr
    assert run.stdout.strip() == "107"
