from pathlib import Path

from tests.wasm_linked_runner import (
    build_wasm_linked,
    require_wasm_toolchain,
    run_wasm_linked,
)


def test_wasm_db_host_poll_parity(tmp_path: Path) -> None:
    require_wasm_toolchain()

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "db_host_poll.py"
    src.write_text(
        "def main():\n"
        "    stream = molt_stream_new(0)\n"
        "    data = molt_stream_recv(stream)\n"
        "    if isinstance(data, (bytes, bytearray)):\n"
        "        print(data)\n"
        "        print(len(data))\n"
        "    else:\n"
        "        print('pending')\n"
        "\n"
        "if __name__ == '__main__':\n"
        "    main()\n"
    )

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)
    assert run.returncode == 0, run.stderr
    assert run.stdout.strip() == "\n".join(["b'poll'", "4"])
