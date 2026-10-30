from pathlib import Path

from tests.wasm_linked_runner import (
    build_wasm_linked,
    require_wasm_toolchain,
    run_cpython_reference,
    run_wasm_linked,
)

# A list has no __aiter__, so `async for` over it raises TypeError
# (Python/bytecodes.c inst(GET_AITER)), as does aiter() on it
# (Objects/abstract.c:PyObject_GetAIter). The program prints only the
# exception type for those two probes: Molt raises the right type with one
# message for both, where CPython words the statement and the builtin
# differently; that message gap is tracked separately. CPython is the oracle.
PROGRAM = """\
import asyncio

class Counter:
    def __init__(self, n):
        self.i = 1
        self.n = n
    def __aiter__(self):
        return self
    async def __anext__(self):
        if self.i > self.n:
            raise StopAsyncIteration
        val = self.i
        self.i += 1
        await asyncio.sleep(0)
        return val

async def main():
    async for item in Counter(3):
        print(item)
    try:
        async for item in [20, 30]:
            print(item)
    except TypeError as exc:
        print(type(exc).__name__)
    try:
        aiter([20, 30])
    except TypeError as exc:
        print(type(exc).__name__)
    it = aiter(Counter(1))
    print(await anext(it))
    try:
        await anext(it)
    except StopAsyncIteration:
        print('done')
    it2 = aiter(Counter(0))
    print(await anext(it2, 7))

asyncio.run(main())
"""


def test_wasm_async_protocol_parity(tmp_path: Path) -> None:
    require_wasm_toolchain()

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "async_protocol.py"
    src.write_text(PROGRAM, encoding="utf-8")

    expected = run_cpython_reference(src)
    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)
    assert run.returncode == 0, run.stderr
    assert expected.stdout.strip()
    assert run.stdout == expected.stdout
