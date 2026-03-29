from __future__ import annotations

from pathlib import Path

from tests.wasm_linked_runner import (
    build_wasm_linked,
    require_wasm_toolchain,
    run_wasm_linked,
)


def test_wasm_linked_free_function_alias_preserves_args(tmp_path: Path) -> None:
    require_wasm_toolchain()
    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "free_fn_alias_probe.py"
    src.write_text(
        "class A:\n"
        "    pass\n\n"
        "def f(cls, values):\n"
        "    print(values)\n\n"
        "g = f\n"
        "g(A, (1, 2))\n",
        encoding="utf-8",
    )

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)

    assert run.returncode == 0, run.stderr
    assert run.stdout.splitlines() == ["(1, 2)"]
