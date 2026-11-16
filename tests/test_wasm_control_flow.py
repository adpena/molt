from pathlib import Path

from molt.wasm_artifact import wasm_runtime_manifest_path
from tests.wasm_linked_runner import (
    _run_wasm_test_process,
    build_wasm_linked,
    require_node_binary,
    require_wasm_build_toolchain,
    require_wasm_toolchain,
    run_wasm_linked,
)


def test_wasm_control_flow_parity(tmp_path: Path) -> None:
    require_wasm_toolchain()

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "if_else.py"
    src.write_text(
        "x = 1\nif x < 2:\n    print(1)\nelse:\n    print(2)\n", encoding="utf-8"
    )

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)
    assert run.returncode == 0, run.stderr
    assert run.stdout.strip() == "1"


def test_wasm_module_try_exception_loop_parity(tmp_path: Path) -> None:
    """A module-level try loop and a generator-fed `for` match CPython."""
    require_wasm_build_toolchain()
    node = require_node_binary()

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
        "\n"
        "def numbers():\n"
        "    yield 1\n"
        "    yield 2\n"
        "    yield 3\n"
        "\n"
        "def consume_iterator(values):\n"
        "    total = 0\n"
        "    for value in values:\n"
        "        total += value\n"
        "    return total\n"
        "\n"
        "print(consume_iterator(numbers()))\n",
        encoding="utf-8",
    )

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = _run_wasm_test_process(
        [
            node,
            str(root / "wasm" / "run_wasm.js"),
            str(wasm_runtime_manifest_path(output_wasm)),
        ],
        role="guest-host",
        cwd=root,
    )
    assert run.returncode == 0, run.stderr
    assert run.stdout.strip() == "107\n6"
