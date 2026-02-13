from __future__ import annotations

import textwrap
from pathlib import Path

from tests.wasm_linked_runner import (
    build_wasm_linked,
    require_wasm_toolchain,
    run_wasm_linked,
)


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
        )
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
