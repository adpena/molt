from pathlib import Path

from tests.wasm_linked_runner import (
    build_wasm_linked,
    require_wasm_toolchain,
    run_wasm_linked,
)


def test_wasm_bytes_ops_parity(tmp_path: Path) -> None:
    require_wasm_toolchain()

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "bytes_ops.py"
    src.write_text(
        "b = b'one,two'\n"
        "print(len(b))\n"
        "print((b + b'!').find(b'two'))\n"
        "print((b + b'!').find(b'two', 2))\n"
        "parts = b.split(b',')\n"
        "print(len(parts))\n"
        "print(len(parts[0]))\n"
        "print(len(parts[1]))\n"
        "print(b.replace(b'one', b'uno').find(b'uno'))\n"
        "print(b.startswith(b'one'))\n"
        "print(b.startswith(b'one', 0, 3))\n"
        "print(b.endswith(b'two'))\n"
        "print(b.endswith(b'two', 0, len(b)))\n"
        "print(b.count(b'o'))\n"
        "print(b[1])\n"
        "print(b.find(b'ne'))\n"
        "print(44 in b)\n"
        "ba = bytearray(b'one,two')\n"
        "print(len(ba))\n"
        "print(ba.find(b'two'))\n"
        "print(ba.find(b'two', 2))\n"
        "parts2 = ba.split(b',')\n"
        "print(len(parts2))\n"
        "print(len(parts2[0]))\n"
        "print(len(parts2[1]))\n"
        "print(ba.replace(b'two', b'dos').find(b'dos'))\n"
        "print((ba + bytearray(b'!')).find(b'!'))\n"
        "print(ba.startswith(b'one'))\n"
        "print(ba.startswith(b'one', 0, 3))\n"
        "print(ba.endswith(b'two'))\n"
        "print(ba.endswith(b'two', 0, len(ba)))\n"
        "print(ba.count(b'o'))\n"
        "print(ba[1])\n"
        "print(ba.find(b'ne'))\n"
        "print(44 in ba)\n"
    )

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)
    assert run.returncode == 0, run.stderr
    assert run.stdout.strip() == (
        "7\n4\n4\n2\n3\n3\n0\nTrue\nTrue\nTrue\nTrue\n2\n110\n1\nTrue\n7\n4\n"
        "4\n2\n3\n3\n4\n7\nTrue\nTrue\nTrue\nTrue\n2\n110\n1\nTrue"
    )
