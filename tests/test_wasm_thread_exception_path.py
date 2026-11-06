from __future__ import annotations

import pytest

import textwrap
from pathlib import Path

from tests.wasm_linked_runner import (
    build_wasm_linked,
    require_wasm_toolchain,
    run_wasm_linked,
)


# A cold linked WASM build of this program took up to 323 s under landing
# load on 2026-11-05 (Molt compile latency, HF-791): presubmit skips it for
# harness-only readers; WASM changes and postsubmit run it.
@pytest.mark.slow
def test_wasm_thread_start_notimplemented_has_clean_exception_path(
    tmp_path: Path,
) -> None:
    require_wasm_toolchain()

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "wasm_thread_notimplemented.py"
    src.write_text(
        textwrap.dedent(
            """\
            import threading


            def _worker():
                return None


            try:
                t = threading.Thread(target=_worker)
                t.start()
                t.join()
                print("unexpected-thread-success")
            except Exception as exc:
                print(type(exc).__name__)
                print(str(exc))
                print("caught")
            """
        ),
        encoding="utf-8",
    )

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)
    assert run.returncode == 0, run.stderr
    assert (
        run.stdout.strip()
        == "NotImplementedError\nthreads are unavailable in wasm\ncaught"
    )
    assert "TypeError: list indices must be integers or slices" not in run.stdout
    assert "TypeError: list indices must be integers or slices" not in run.stderr
