from pathlib import Path

from tests.wasm_linked_runner import (
    build_wasm_linked,
    require_wasm_toolchain,
    run_wasm_linked,
)


def test_wasm_channel_async_parity(tmp_path: Path) -> None:
    require_wasm_toolchain()

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "channel_async.py"
    src.write_text(
        "import asyncio\n"
        "\n"
        "async def main():\n"
        "    chan = molt_chan_new(1)\n"
        "    molt_chan_send(chan, 41)\n"
        "    print(molt_chan_recv(chan))\n"
        "    molt_chan_send(chan, 1)\n"
        "    print(molt_chan_recv(chan))\n"
        "\n"
        "asyncio.run(main())\n"
    )

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)
    assert run.returncode == 0, run.stderr
    assert run.stdout.strip() == "\n".join(["41", "1"])
