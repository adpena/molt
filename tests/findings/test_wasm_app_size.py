"""HF-859: an LLVM wasm32 split app stays within its 1 MiB size budget.

Oracle: the budget the retired WASM backend met for the same program
(841,250 bytes). A split build's app.wasm holds the program and the stdlib
functions it reaches; the runtime ships separately.
"""

from __future__ import annotations

from pathlib import Path

from tests.findings import finding
from tests.test_wasm_split_runtime import PROGRAM_A, _build_split

APP_BUDGET_BYTES = 1024 * 1024


@finding("HF-859")
def test_split_app_wasm_fits_its_size_budget(tmp_path: Path) -> None:
    source = tmp_path / "prog_a.py"
    source.write_text(PROGRAM_A, encoding="utf-8")
    out_dir = tmp_path / "out"
    out_dir.mkdir()
    build = _build_split(source, out_dir)
    if build.returncode != 0:
        raise RuntimeError(f"split build failed:\n{build.stderr[-2000:]}")
    size = (out_dir / "app.wasm").stat().st_size
    assert size < APP_BUDGET_BYTES, (
        f"app.wasm is {size:,} bytes; budget {APP_BUDGET_BYTES:,} (HF-859)"
    )
