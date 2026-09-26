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
        "from moltlib.concurrency import channel\n"
        "\n"
        "async def main():\n"
        "    chan = channel(1)\n"
        "    await chan.send_async(41)\n"
        "    sender = asyncio.create_task(chan.send_async(1))\n"
        "    await asyncio.sleep(0)\n"
        "    assert not sender.done(), 'send bypassed channel capacity'\n"
        "    print(await chan.recv_async())\n"
        "    await sender\n"
        "    print(await chan.recv_async())\n"
        "    chan.close()\n"
        "\n"
        "asyncio.run(main())\n"
    )

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)
    assert run.returncode == 0, run.stderr
    assert run.stdout.strip() == "\n".join(["41", "1"])
