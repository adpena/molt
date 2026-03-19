from __future__ import annotations

import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"

_PROBE = f"""
import builtins
import importlib.util
import sys
import types

_consts = {{
    "molt_signal_sig_dfl": 0,
    "molt_signal_sig_ign": 1,
    "molt_signal_sigint": 2,
    "molt_signal_sigterm": 15,
    "molt_signal_sighup": 1,
    "molt_signal_sigquit": 3,
    "molt_signal_sigabrt": 6,
    "molt_signal_sigfpe": 8,
    "molt_signal_sigill": 4,
    "molt_signal_sigsegv": 11,
    "molt_signal_sigpipe": 13,
    "molt_signal_sigalrm": 14,
    "molt_signal_sigusr1": 10,
    "molt_signal_sigusr2": 12,
    "molt_signal_sigchld": 20,
    "molt_signal_nsig": 32,
    "molt_signal_sig_block": 0,
    "molt_signal_sig_unblock": 1,
    "molt_signal_sig_setmask": 2,
    "molt_signal_sigbus": 7,
    "molt_signal_sigcont": 19,
    "molt_signal_sigstop": 17,
    "molt_signal_sigtstp": 18,
    "molt_signal_sigttin": 21,
    "molt_signal_sigttou": 22,
    "molt_signal_sigxcpu": 24,
    "molt_signal_sigxfsz": 25,
    "molt_signal_sigvtalrm": 26,
    "molt_signal_sigprof": 27,
    "molt_signal_sigwinch": 28,
    "molt_signal_sigsys": 31,
}}

_handlers = {{}}


def _const(name):
    return lambda: _consts[name]


builtins._molt_intrinsics = {{
    "molt_stdlib_probe": lambda: None,
    **{{name: _const(name) for name in _consts}},
    "molt_signal_signal": lambda sig, handler: _handlers.setdefault(sig, _consts["molt_signal_sig_dfl"]),
    "molt_signal_getsignal": lambda sig: _handlers.get(sig, _consts["molt_signal_sig_dfl"]),
    "molt_signal_raise_signal": lambda sig: None,
    "molt_signal_alarm": lambda seconds: int(seconds),
    "molt_signal_pause": lambda: None,
    "molt_signal_set_wakeup_fd": lambda fd: int(fd),
    "molt_signal_valid_signals": lambda: [2, 15],
    "molt_signal_strsignal": lambda sig: f"sig{{sig}}",
    "molt_signal_pthread_sigmask": lambda how, mask: list(mask),
    "molt_signal_pthread_kill": lambda tid, sig: None,
    "molt_signal_sigpending": lambda: [2],
    "molt_signal_sigwait": lambda sigset: list(sigset)[0],
    "molt_signal_default_int_handler": lambda *args: "default-int-handler",
}}

_intrinsics_mod = types.ModuleType("_intrinsics")


def _require_intrinsic(name, namespace=None):
    intrinsics = getattr(builtins, "_molt_intrinsics", {{}})
    if name in intrinsics:
        value = intrinsics[name]
        if namespace is not None:
            namespace[name] = value
        return value
    raise RuntimeError(f"intrinsic unavailable: {{name}}")


_intrinsics_mod.require_intrinsic = _require_intrinsic
sys.modules["_intrinsics"] = _intrinsics_mod


def _load_module(name, path_text):
    spec = importlib.util.spec_from_file_location(name, path_text)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


_private = _load_module("_molt_private_signal", {str(STDLIB_ROOT / "_signal.py")!r})

rows = [
    (name, type(getattr(_private, name)).__name__, bool(callable(getattr(_private, name))))
    for name in sorted(dir(_private))
    if not name.startswith("_") and name != "annotations"
]
for name, type_name, is_callable in rows:
    print(f"ROW|{{name}}|{{type_name}}|{{is_callable}}")

checks = {{
    "behavior": (
        _private.SIGINT == 2
        and _private.NSIG == 32
        and _private.alarm(5) == 5
        and _private.set_wakeup_fd(9) == 9
        and _private.valid_signals() == {{2, 15}}
        and _private.strsignal(2) == "sig2"
        and _private.pthread_sigmask(_private.SIG_BLOCK, [2, 15]) == {{2, 15}}
        and _private.sigpending() == {{2}}
        and _private.sigwait([15, 2]) == 15
    ),
}}
for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def _run_probe() -> tuple[list[tuple[str, str, str]], dict[str, str]]:
    proc = subprocess.run(
        [sys.executable, "-c", _PROBE],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=True,
    )
    rows: list[tuple[str, str, str]] = []
    checks: dict[str, str] = {}
    for line in proc.stdout.splitlines():
        prefix, *rest = line.split("|")
        if prefix == "ROW":
            rows.append((rest[0], rest[1], rest[2]))
        elif prefix == "CHECK":
            checks[rest[0]] = rest[1]
    return rows, checks


def test__signal_public_surface_matches_expected_shape() -> None:
    rows, checks = _run_probe()
    names = [name for name, _, _ in rows]
    assert "molt_signal_sigint" not in names
    assert "SIGINT" in names
    assert "SIGTERM" in names
    assert "signal" in names
    assert "getsignal" in names
    assert "raise_signal" in names
    assert checks == {"behavior": "True"}
