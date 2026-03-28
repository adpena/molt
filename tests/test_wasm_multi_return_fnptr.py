from __future__ import annotations

import textwrap
from pathlib import Path

from tests.wasm_linked_runner import (
    build_wasm_linked,
    require_wasm_toolchain,
    run_wasm_linked,
)


MULTI_RETURN_FN_PTR_SRC = textwrap.dedent(
    """\
    def pair(x):
        return x, x + 1

    a, b = pair(5)
    print(a + b)

    fn = pair
    result = fn(10)
    print(result[0])
    print(result[1])
    """
)

UNPACK_SEQUENCE_DIRECT_SRC = textwrap.dedent(
    """\
    def pair(x):
        return x, x + 1

    r = pair(5)
    print(r[0])
    print(r[1])

    a, b = pair(5)
    print(a)
    print(b)
    print(a + b)
    """
)


def test_wasm_unpack_sequence_direct_call(tmp_path: Path) -> None:
    require_wasm_toolchain()

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "unpack_sequence_direct.py"
    src.write_text(UNPACK_SEQUENCE_DIRECT_SRC)

    output_wasm = build_wasm_linked(
        root,
        src,
        tmp_path,
        extra_args=["--codec", "json"],
    )
    run = run_wasm_linked(root, output_wasm)

    assert run.returncode == 0, run.stderr
    assert run.stdout.splitlines() == ["5", "6", "5", "6", "11"]


def test_wasm_unpack_sequence_function_local(tmp_path: Path) -> None:
    require_wasm_toolchain()

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "unpack_sequence_function_local.py"
    src.write_text(
        textwrap.dedent(
            """\
            def pair(x):
                return x, x + 1

            def first_sum(r):
                a, b = r
                print(a)
                print(b)
                return a + b

            print(first_sum(pair(5)))
            """
        )
    )

    output_wasm = build_wasm_linked(
        root,
        src,
        tmp_path,
        extra_args=["--codec", "json"],
    )
    run = run_wasm_linked(root, output_wasm)

    assert run.returncode == 0, run.stderr
    assert run.stdout.splitlines() == ["5", "6", "11"]


def test_wasm_multi_return_function_object_call(tmp_path: Path) -> None:
    require_wasm_toolchain()

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "multi_return_fnptr.py"
    src.write_text(MULTI_RETURN_FN_PTR_SRC)

    output_wasm = build_wasm_linked(
        root,
        src,
        tmp_path,
        extra_args=["--codec", "json"],
    )
    run = run_wasm_linked(root, output_wasm)

    assert run.returncode == 0, run.stderr
    assert run.stdout.splitlines() == ["11", "10", "11"]
