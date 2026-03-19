from __future__ import annotations

from pathlib import Path

from tests.wasm_linked_runner import (
    build_wasm_linked,
    require_wasm_toolchain,
    run_wasm_linked,
)


def test_wasm_importlib_machinery_imports(tmp_path: Path) -> None:
    require_wasm_toolchain()
    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "importlib_machinery_probe.py"
    src.write_text(
        "\n".join(
            [
                "import importlib.machinery",
                "print(importlib.machinery.__name__)",
                "print(hasattr(importlib.machinery, 'ModuleSpec'))",
            ]
        )
        + "\n",
        encoding="utf-8",
    )

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)

    assert run.returncode == 0, run.stderr
    assert run.stdout.splitlines() == ["importlib.machinery", "True"]
