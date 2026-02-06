import tempfile
from contextlib import contextmanager
from pathlib import Path

from tests.wasm_linked_runner import (
    build_wasm_linked,
    require_wasm_toolchain,
    run_wasm_linked,
)


@contextmanager
def _work_dir(tmp_path: Path):
    root = Path("/Volumes/APDataStore/Molt")
    if root.exists():
        base = root / "tmp"
        base.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=base, prefix="molt_wasm_channel_") as td:
            yield Path(td)
        return
    yield tmp_path


def test_wasm_channel_basic(tmp_path: Path) -> None:
    require_wasm_toolchain()

    root = Path(__file__).resolve().parents[1]
    with _work_dir(tmp_path) as work_dir:
        src = work_dir / "channel_basic.py"
        src.write_text(
            "from molt.concurrency import channel, _call_intrinsic\n"
            "\n"
            "ch = channel(1)\n"
            'res = _call_intrinsic("molt_chan_send", ch._handle, 41)\n'
            'print("send_res", res)\n'
            "ok, val = ch.try_recv()\n"
            'print("try_recv", ok, val)\n'
            "ok, val = ch.try_recv()\n"
            'print("try_recv_empty", ok, val)\n'
        )

        output_wasm = build_wasm_linked(root, src, work_dir)
        run = run_wasm_linked(root, output_wasm)
        assert run.returncode == 0, run.stderr
        expected = "\n".join(
            [
                "send_res 0",
                "try_recv True 41",
                "try_recv_empty False None",
            ]
        )
        assert run.stdout.strip() == expected


def test_wasm_channel_dynamic_intrinsic_require(tmp_path: Path) -> None:
    require_wasm_toolchain()

    root = Path(__file__).resolve().parents[1]
    with _work_dir(tmp_path) as work_dir:
        src = work_dir / "channel_dynamic_require.py"
        src.write_text(
            "from molt import intrinsics as _intrinsics\n"
            "from molt.concurrency import channel\n"
            "\n"
            "ch = channel(1)\n"
            "send = _intrinsics.require('molt_chan_send', globals())\n"
            "recv = _intrinsics.require('molt_chan_try_recv', globals())\n"
            "print('send_res', send(ch._handle, 41))\n"
            "print('try_recv', recv(ch._handle))\n"
        )

        output_wasm = build_wasm_linked(root, src, work_dir)
        run = run_wasm_linked(root, output_wasm)
        assert run.returncode == 0, run.stderr
        expected = "\n".join(
            [
                "send_res 0",
                "try_recv 41",
            ]
        )
        assert run.stdout.strip() == expected
