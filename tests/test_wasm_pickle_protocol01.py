from __future__ import annotations

import textwrap
from pathlib import Path

from tests.wasm_linked_runner import (
    build_wasm_linked,
    require_wasm_toolchain,
    run_wasm_linked,
)


def test_wasm_pickle_protocol01_roundtrip(tmp_path: Path) -> None:
    require_wasm_toolchain()

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "pickle_protocol01.py"
    src.write_text(
        textwrap.dedent(
            """\
            import pickle


            payload = {
                "s": slice(1, 9, 2),
                "t": (1, None, True),
                "l": [3, 4],
                "d": {"x": 5, "y": 6},
                "b": b"abc",
                "ba": bytearray(b"xy"),
            }
            for proto in (0, 1):
                blob = pickle.dumps(payload, protocol=proto)
                out = pickle.loads(blob)
                print(
                    proto,
                    out["s"].start,
                    out["s"].stop,
                    out["s"].step,
                    out["t"][2],
                    out["l"][1],
                    out["d"]["y"],
                    out["b"],
                    type(out["ba"]).__name__,
                )
            """
        )
    )

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)
    assert run.returncode == 0, run.stderr
    assert run.stdout.strip() == (
        "0 1 9 2 True 4 6 b'abc' bytearray\n1 1 9 2 True 4 6 b'abc' bytearray"
    )
