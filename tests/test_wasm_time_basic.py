import tempfile
from contextlib import contextmanager
from pathlib import Path

from tests.wasm_linked_runner import (
    build_wasm_linked,
    require_wasm_toolchain,
    run_wasm_linked,
)


@contextmanager
def _work_dir(root: Path):
    ext_root = Path("/Volumes/APDataStore/Molt")
    if ext_root.exists():
        base = ext_root / "tmp"
        base.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=base, prefix="molt_wasm_time_") as td:
            yield Path(td)
        return
    fallback = root / "build" / "wasm"
    fallback.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=fallback, prefix="molt_wasm_time_") as td:
        yield Path(td)


def test_wasm_time_intrinsics_basic() -> None:
    require_wasm_toolchain()

    root = Path(__file__).resolve().parents[1]
    with _work_dir(root) as work_dir:
        src = work_dir / "time_intrinsics_basic.py"
        src.write_text(
            "import time\n"
            "g0 = time.gmtime(0)\n"
            "print('monotonic_type', isinstance(time.monotonic(), float))\n"
            "print('monotonic_ns_type', isinstance(time.monotonic_ns(), int))\n"
            "print('perf_counter_type', isinstance(time.perf_counter(), float))\n"
            "try:\n"
            "    _pt = time.process_time_ns()\n"
            "except OSError as exc:\n"
            "    print('process_time_ns_unavailable', 'process_time unavailable' in str(exc))\n"
            "else:\n"
            "    print('process_time_ns_type', isinstance(_pt, int))\n"
            "print('gmtime_year', g0.tm_year)\n"
            "print('strftime', time.strftime('%Y', g0))\n"
            "print('timezone_type', isinstance(time.timezone, int))\n"
            "print('tzname_len', len(time.tzname))\n"
        )

        output_wasm = build_wasm_linked(root, src, work_dir)
        run = run_wasm_linked(root, output_wasm)
        assert run.returncode == 0, run.stderr
        expected = "\n".join(
            [
                "monotonic_type True",
                "monotonic_ns_type True",
                "perf_counter_type True",
                "process_time_ns_unavailable True",
                "gmtime_year 1970",
                "strftime 1970",
                "timezone_type True",
                "tzname_len 2",
            ]
        )
        assert run.stdout.strip() == expected


def test_wasm_time_dynamic_intrinsic_require() -> None:
    require_wasm_toolchain()

    root = Path(__file__).resolve().parents[1]
    with _work_dir(root) as work_dir:
        src = work_dir / "time_dynamic_require.py"
        src.write_text(
            "from molt import intrinsics as _intrinsics\n"
            "mono = _intrinsics.require('molt_time_monotonic', globals())\n"
            "print('mono_is_float', isinstance(mono(), float))\n"
            "try:\n"
            "    _intrinsics.require('molt_time_not_real', globals())\n"
            "except RuntimeError as exc:\n"
            "    print('missing_intrinsic_raises', 'intrinsic unavailable:' in str(exc))\n"
        )

        output_wasm = build_wasm_linked(root, src, work_dir)
        run = run_wasm_linked(root, output_wasm)
        assert run.returncode == 0, run.stderr
        expected = "\n".join(
            [
                "mono_is_float True",
                "missing_intrinsic_raises True",
            ]
        )
        assert run.stdout.strip() == expected
