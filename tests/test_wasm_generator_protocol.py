import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

from tests.wasm_harness import write_wasm_runner


def test_wasm_generator_protocol_parity(tmp_path: Path) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for wasm parity test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for wasm parity test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "generator_protocol.py"
    src.write_text(
        "def gen_send():\n"
        "    x = yield 1\n"
        "    yield x\n"
        "\n"
        "g = gen_send()\n"
        "print(g.send(None))\n"
        "print(g.send(10))\n"
        "try:\n"
        "    g.send(11)\n"
        "except StopIteration:\n"
        "    print(0)\n"
        "\n"
        "def gen_throw():\n"
        "    try:\n"
        "        yield 1\n"
        "    except ValueError:\n"
        "        print(7)\n"
        "        yield 2\n"
        "\n"
        "g2 = gen_throw()\n"
        "print(g2.send(None))\n"
        "print(g2.throw(ValueError('x')))\n"
        "try:\n"
        "    g2.send(None)\n"
        "except StopIteration:\n"
        "    print(3)\n"
        "\n"
        "def gen_close():\n"
        "    try:\n"
        "        yield 1\n"
        "    finally:\n"
        "        print(4)\n"
        "\n"
        "g3 = gen_close()\n"
        "print(g3.send(None))\n"
        "g3.close()\n"
        "\n"
        "def sub_send():\n"
        "    x = yield 101\n"
        "    yield x\n"
        "\n"
        "def gen_yf():\n"
        "    yield from sub_send()\n"
        "\n"
        "g4 = gen_yf()\n"
        "print(g4.send(None))\n"
        "print(g4.send(202))\n"
        "try:\n"
        "    g4.send(None)\n"
        "except StopIteration:\n"
        "    print(203)\n"
        "\n"
        "def gen_close_raise():\n"
        "    try:\n"
        "        yield 111\n"
        "    finally:\n"
        "        print(112)\n"
        "        raise RuntimeError('boom')\n"
        "\n"
        "g5 = gen_close_raise()\n"
        "print(g5.send(None))\n"
        "try:\n"
        "    g5.close()\n"
        "except RuntimeError:\n"
        "    print(113)\n"
        "\n"
        "def gen_throw_finally():\n"
        "    try:\n"
        "        yield 121\n"
        "    finally:\n"
        "        print(122)\n"
        "        raise RuntimeError('boom')\n"
        "\n"
        "g6 = gen_throw_finally()\n"
        "print(g6.send(None))\n"
        "try:\n"
        "    g6.throw(ValueError('x'))\n"
        "except RuntimeError:\n"
        "    print(123)\n"
        "\n"
        "def gen_ctx():\n"
        "    raise RuntimeError('inner')\n"
        "    yield 1\n"
        "\n"
        "g7 = gen_ctx()\n"
        "try:\n"
        "    raise ValueError('outer')\n"
        "except ValueError as outer:\n"
        "    try:\n"
        "        g7.send(None)\n"
        "    except RuntimeError as exc:\n"
        "        print(exc.__context__ is outer)\n"
        "\n"
        "def gen_raise_from():\n"
        "    try:\n"
        "        raise ValueError('inner')\n"
        "    except ValueError as err:\n"
        "        raise RuntimeError('outer') from err\n"
        "    yield 1\n"
        "\n"
        "g9 = gen_raise_from()\n"
        "try:\n"
        "    g9.send(None)\n"
        "except RuntimeError as exc:\n"
        "    print(exc.__cause__ is None)\n"
        "    print(exc.__context__ is exc.__cause__)\n"
        "    print(exc.__suppress_context__)\n"
        "\n"
        "def gen_raise_from_none():\n"
        "    try:\n"
        "        raise ValueError('inner')\n"
        "    except ValueError:\n"
        "        raise RuntimeError('outer') from None\n"
        "    yield 1\n"
        "\n"
        "g10 = gen_raise_from_none()\n"
        "try:\n"
        "    g10.send(None)\n"
        "except RuntimeError as exc:\n"
        "    print(exc.__cause__ is None)\n"
        "    print(exc.__context__ is None)\n"
        "    print(exc.__suppress_context__)\n"
    )

    output_wasm = tmp_path / "output.wasm"

    runner = write_wasm_runner(tmp_path, "run_wasm_generator_protocol.js")

    env = os.environ.copy()
    env["PYTHONPATH"] = str(root / "src")
    build = subprocess.run(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            str(src),
            "--target",
            "wasm",
            "--out-dir",
            str(tmp_path),
        ],
        cwd=root,
        env=env,
        capture_output=True,
        text=True,
    )
    assert build.returncode == 0, build.stderr

    run = subprocess.run(
        ["node", str(runner), str(output_wasm)],
        cwd=root,
        capture_output=True,
        text=True,
    )
    assert run.returncode == 0, run.stderr
    assert run.stdout.strip() == "\n".join(
        [
            "1",
            "10",
            "0",
            "1",
            "7",
            "2",
            "3",
            "1",
            "4",
            "101",
            "202",
            "203",
            "111",
            "112",
            "113",
            "121",
            "122",
            "123",
            "True",
            "False",
            "True",
            "True",
            "True",
            "False",
            "True",
        ]
    )
