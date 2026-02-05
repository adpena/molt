from pathlib import Path

from tests.wasm_linked_runner import (
    build_wasm_linked,
    require_wasm_toolchain,
    run_wasm_linked,
)


def test_wasm_async_protocol_parity(tmp_path: Path) -> None:
    require_wasm_toolchain()

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "async_protocol.py"
    src.write_text(
        "import asyncio\n"
        "\n"
        "class Counter:\n"
        "    def __init__(self, n):\n"
        "        self.i = 1\n"
        "        self.n = n\n"
        "    def __aiter__(self):\n"
        "        return self\n"
        "    async def __anext__(self):\n"
        "        if self.i > self.n:\n"
        "            raise StopAsyncIteration\n"
        "        val = self.i\n"
        "        self.i += 1\n"
        "        await asyncio.sleep(0)\n"
        "        return val\n"
        "\n"
        "async def main():\n"
        "    async for item in Counter(3):\n"
        "        print(item)\n"
        "    async for item in [20, 30]:\n"
        "        print(item)\n"
        "    it = aiter(Counter(1))\n"
        "    print(await anext(it))\n"
        "    try:\n"
        "        await anext(it)\n"
        "    except StopAsyncIteration:\n"
        "        print('done')\n"
        "    it2 = aiter(Counter(0))\n"
        "    print(await anext(it2, 7))\n"
        "\n"
        "asyncio.run(main())\n"
    )

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)
    assert run.returncode == 0, run.stderr
    assert run.stdout.strip() == "\n".join(
        ["1", "2", "3", "20", "30", "1", "done", "7"]
    )
