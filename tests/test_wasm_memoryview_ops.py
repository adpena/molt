import textwrap
from pathlib import Path

from tests.wasm_linked_runner import (
    build_wasm_linked,
    require_wasm_toolchain,
    run_wasm_linked,
)


def test_wasm_memoryview_ops_parity(tmp_path: Path) -> None:
    require_wasm_toolchain()

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "memoryview_ops.py"
    src.write_text(
        textwrap.dedent(
            """\
            b = b'one,two'
            mv = memoryview(b)
            print(len(mv))
            print(mv[1])
            data = mv.tobytes()
            print(data[0])
            print(data[-1])
            ba = bytearray(b'one,two')
            mv2 = memoryview(ba)
            print(len(mv2))
            print(mv2[1])
            data2 = mv2.tobytes()
            print(data2[0])
            print(data2[-1])
            ba2 = bytearray([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11])
            mv3 = memoryview(ba2).cast('B', shape=[3, 4])
            print(mv3.shape[0])
            print(mv3.shape[1])
            print(mv3.strides[0])
            print(mv3.strides[1])
            print(mv3[1, 2])
            print(mv3[-1, -1])
            ba0 = bytearray(b'a')
            mv0 = memoryview(ba0).cast('B', shape=[])
            print(mv0[()])
            mvc = memoryview(bytearray(b'abc')).cast('c')
            print(mvc[0][0])
            mvc[0] = b'z'
            print(mvc[0][0])
            """
        )
    )

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)
    assert run.returncode == 0, run.stderr
    assert (
        run.stdout.strip()
        == "7\n110\n111\n111\n7\n110\n111\n111\n3\n4\n4\n1\n6\n11\n97\n97\n122"
    )
