"""Two-direction gate test for tools/witness_iter.py (FAST-WITNESS-ITER).

Host-independent: exercises the runner's PARSE + EVALUATE logic against synthetic
native-engine output, so the PASS/RED gate is proven correct without needing a
Unix host or a built harness. The #39 two-direction pattern:

  * a clean, all-fixes-landed drive (reaches numpy.exceptions, GAP=5, no datetime
    silent-failure) -> PASS;
  * each reverted-landed-fix signature (datetime CAPI silent failure; a widened
    symbol GAP; an engine panic) -> RED.

A runner that could not distinguish these would be theater (M05); this test is
the mask-proof that it can.
"""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

_RUNNER = Path(__file__).resolve().parents[2] / "tools" / "witness_iter.py"


def _load_runner():
    spec = importlib.util.spec_from_file_location("witness_iter_mod", _RUNNER)
    assert spec and spec.loader
    mod = importlib.util.module_from_spec(spec)
    # Register before exec so @dataclass can resolve cls.__module__ during import.
    sys.modules[spec.name] = mod
    spec.loader.exec_module(mod)
    return mod


WI = _load_runner()
BASELINE = WI.DEFAULT_BASELINES["_multiarray_umath"]


# ── Synthetic engine outputs ──────────────────────────────────────────────────
KNOWN_GOOD = """\
== static symbol-gap check ...
   numpy needs 301 Py* symbols; harness exports 581; GAP=20
   --- MISSING Py* symbols (symbol-gap frontiers) ---
     _PyArg_ParseTuple_SizeT
     _PyArg_ParseTupleAndKeywords_SizeT
== driving PyInit__multiarray_umath against molt ABI ...
===MOLT_DISCOVERY: real cpython-abi hooks registered (single static pool)
===MOLT_DISCOVERY: driving PyInit__multiarray_umath from /w/_multiarray_umath.so
[MOLT_TRACE_CAPI] call PyCapsule_Import(datetime.datetime_CAPI)
[MOLT_TRACE_CAPI] call PyImport_ImportModule(numpy.exceptions)
===MOLT_DISCOVERY_FRONTIER (LoadError): InitReturnedNull { name: "_multiarray_umath" }
===MOLT_DISCOVERY_FRONTIER_DISPLAY: numpy _multiarray_umath init returned NULL
===MOLT_DISCOVERY_EXC: "import of 'numpy.exceptions' failed (runtime import error pending)"
== driver exit code: 10
"""

# Regression 1: datetime CAPI capsule reverted (09c8d2337) -> PyCapsule_Import
# silently fails, numpy stops BEFORE the numpy.exceptions import. (Mirrors the
# real x86_64-Linux drive captured 2026-07-10.)
REGRESSION_DATETIME = """\
== static symbol-gap check ...
   numpy needs 301 Py* symbols; harness exports 581; GAP=20
== driving PyInit__multiarray_umath against molt ABI ...
[MOLT_TRACE_CAPI] call PyCapsule_Import(datetime.datetime_CAPI)
[MOLT_TRACE_CAPI] call PyImport_ImportModule(datetime)
[MOLT_TRACE_CAPI] silent-failure PyCapsule_Import(datetime.datetime_CAPI)
===MOLT_DISCOVERY_FRONTIER (LoadError): InitReturnedNull { name: "_multiarray_umath" }
===MOLT_DISCOVERY_EXC: pending exception value = "PyCapsule_Import could not import module capsule \\"datetime.datetime_CAPI\\""
== driver exit code: 10
"""

# Regression 2: allocator/private-symbol batch reverted (61093cb4a) -> the
# static Py* symbol wall widens above the known-good ceiling.
REGRESSION_SYMBOLS = """\
== static symbol-gap check ...
   numpy needs 301 Py* symbols; harness exports 578; GAP=25
     PyObject_Malloc
     PyObject_Calloc
     PyObject_Realloc
== driving PyInit__multiarray_umath against molt ABI ...
[MOLT_TRACE_CAPI] call PyImport_ImportModule(numpy.exceptions)
===MOLT_DISCOVERY_EXC: "import of 'numpy.exceptions' failed"
== driver exit code: 10
"""

# Regression 3: a hard panic mid-drive is never a valid frontier.
REGRESSION_PANIC = """\
   numpy needs 301 Py* symbols; harness exports 581; GAP=20
== driving PyInit__multiarray_umath against molt ABI ...
===MOLT_FRONTIER_PANIC===
panic: called `Option::unwrap()` on a `None` value
backtrace:
   0: ...
===END_MOLT_FRONTIER_PANIC===
== driver exit code: 134
"""


def _verdict(sample: str):
    fp = WI.parse_drive_output("_multiarray_umath", sample)
    return fp, WI.evaluate(fp, BASELINE)


def test_known_good_reaches_frontier_and_passes():
    fp, v = _verdict(KNOWN_GOOD)
    assert fp.symbol_gap == 20
    assert not fp.pyinit_ok  # AOT wall: PyInit returns NULL here, by design
    assert "numpy.exceptions" in fp.haystack()
    assert v.passed, v.reasons
    assert any("reached known-good frontier" in r for r in v.reasons)


def test_datetime_revert_turns_red():
    fp, v = _verdict(REGRESSION_DATETIME)
    assert fp.silent_failures  # the reverted-fix signature was captured
    assert not v.passed
    # Both sides of the gate fire: forbidden present AND required absent.
    joined = " ".join(v.reasons)
    assert "regression marker PRESENT" in joined
    assert "required known-good marker ABSENT" in joined


def test_symbol_gap_widening_turns_red():
    fp, v = _verdict(REGRESSION_SYMBOLS)
    assert fp.symbol_gap == 25
    assert not v.passed
    assert any("symbol GAP 25 > known-good ceiling 20" in r for r in v.reasons)


def test_shrinking_symbol_gap_is_not_a_regression():
    # Better aliasing on a platform (fewer missing symbols) must NOT false-RED.
    sample = KNOWN_GOOD.replace("GAP=20", "GAP=2")
    fp, v = _verdict(sample)
    assert fp.symbol_gap == 2
    assert v.passed, v.reasons


def test_panic_turns_red():
    fp, v = _verdict(REGRESSION_PANIC)
    assert fp.panic
    assert not v.passed
    assert any("PANIC" in r for r in v.reasons)


def test_gate_is_two_sided_not_one_sided():
    # A gate that always-passes or always-fails is theater. Prove it does both.
    _, good = _verdict(KNOWN_GOOD)
    _, bad = _verdict(REGRESSION_DATETIME)
    assert good.passed and not bad.passed


def test_wasm_confirmation_accepts_only_typed_argv_after_separator(monkeypatch):
    captured: list[str] = []
    monkeypatch.setattr(WI, "maybe_dispatch_to_wsl", lambda _argv: None)
    monkeypatch.setattr(WI.platform, "system", lambda: "Linux")

    def fake_confirm(command: list[str]) -> int:
        captured.extend(command)
        return 17

    monkeypatch.setattr(WI, "run_wasm_confirm", fake_confirm)

    assert (
        WI._main(["--wasm-confirm", "--", sys.executable, "-c", "print('typed argv')"])
        == 17
    )
    assert captured == [sys.executable, "-c", "print('typed argv')"]


def test_wasm_confirmation_refuses_missing_argv(monkeypatch):
    monkeypatch.setattr(WI, "maybe_dispatch_to_wsl", lambda _argv: None)
    monkeypatch.setattr(WI.platform, "system", lambda: "Linux")

    with pytest.raises(SystemExit) as exc:
        WI._main(["--wasm-confirm"])

    assert exc.value.code == 2


def test_wsl_boundary_keeps_dynamic_values_out_of_shell_program(monkeypatch):
    calls: list[tuple[list[str], dict[str, object]]] = []
    marker = "; touch /tmp/not-executed"
    monkeypatch.setattr(WI.platform, "system", lambda: "Windows")
    monkeypatch.setattr(WI.shutil, "which", lambda _name: "wsl.exe")
    monkeypatch.delenv("MOLT_WITNESS_WSL_REPO", raising=False)

    def fake_run(command: list[str], **kwargs: object):
        calls.append((command, kwargs))
        if "wslpath" in command:
            return SimpleNamespace(returncode=0, stdout="/mnt/c/Molt/molt-src\n")
        return SimpleNamespace(returncode=23, stdout="")

    monkeypatch.setattr(WI, "_run_child", fake_run)

    assert WI.maybe_dispatch_to_wsl(["--wasm-confirm", "--", marker]) == 23
    launch = calls[1][0]
    shell_program = launch[launch.index("-lc") + 1]
    assert marker not in shell_program
    assert launch[-1] == marker
    assert launch.index("--in-wsl") < launch.index("--")
