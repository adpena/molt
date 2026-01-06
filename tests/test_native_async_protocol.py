import os
import platform
import shutil
import subprocess
import sys
from pathlib import Path

import pytest


def test_native_async_protocol(tmp_path: Path) -> None:
    root = Path(__file__).resolve().parents[1]
    runtime_lib = root / "target" / "release" / "libmolt_runtime.a"
    if not runtime_lib.exists():
        pytest.skip("molt-runtime release library not built")
    if shutil.which("clang") is None:
        pytest.skip("clang not available")
    if shutil.which("cargo") is None:
        pytest.skip("cargo not available")
    if sys.platform == "darwin" and shutil.which("lipo") is not None:
        info = subprocess.run(
            ["lipo", "-info", str(runtime_lib)],
            capture_output=True,
            text=True,
        )
        arch = platform.machine()
        if info.returncode == 0 and arch not in info.stdout:
            pytest.skip("runtime lib architecture mismatch")

    src = tmp_path / "async_protocol.py"
    src.write_text(
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

    output_binary = root / "hello_molt"
    artifacts = [root / "output.o", root / "main_stub.c", output_binary]
    existed = {path: path.exists() for path in artifacts}

    env = os.environ.copy()
    env["PYTHONPATH"] = str(root / "src")

    try:
        build = subprocess.run(
            [sys.executable, "-m", "molt.cli", "build", str(src)],
            cwd=root,
            env=env,
            capture_output=True,
            text=True,
        )
        assert build.returncode == 0, build.stderr

        run = subprocess.run(
            [str(output_binary)],
            cwd=root,
            capture_output=True,
            text=True,
        )
        assert run.returncode == 0, run.stderr
        assert run.stdout.strip().splitlines() == [
            "1",
            "2",
            "3",
            "20",
            "30",
            "1",
            "done",
            "7",
        ]
    finally:
        for path in artifacts:
            if not existed[path] and path.exists():
                path.unlink()
