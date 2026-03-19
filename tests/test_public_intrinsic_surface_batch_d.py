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


def _load_module(name, path_text):
    spec = importlib.util.spec_from_file_location(name, path_text)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


_error_handlers = {{"strict": lambda exc: (_ for _ in ()).throw(exc)}}


def _codecs_encode(obj, encoding, errors):
    if isinstance(obj, str):
        return obj.encode("utf-8")
    return bytes(obj)


def _codecs_decode(obj, encoding, errors):
    return bytes(obj).decode("utf-8")


def _register_error(name, fn):
    _error_handlers[name] = fn


def _lookup_error(name):
    return _error_handlers[name]


_signal_values = {{
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
    "molt_signal_sigchld": 17,
    "molt_signal_nsig": 32,
    "molt_signal_sig_block": 0,
    "molt_signal_sig_unblock": 1,
    "molt_signal_sig_setmask": 2,
    "molt_signal_sigbus": 7,
    "molt_signal_sigcont": 18,
    "molt_signal_sigstop": 19,
    "molt_signal_sigtstp": 20,
    "molt_signal_sigttin": 21,
    "molt_signal_sigttou": 22,
    "molt_signal_sigxcpu": 24,
    "molt_signal_sigxfsz": 25,
    "molt_signal_sigvtalrm": 26,
    "molt_signal_sigprof": 27,
    "molt_signal_sigwinch": 28,
    "molt_signal_sigsys": 31,
}}

_stat_constants = (0,) * 71

builtins._molt_intrinsics = {{
    "molt_codecs_decode": _codecs_decode,
    "molt_codecs_encode": _codecs_encode,
    "molt_codecs_normalize_encoding": lambda encoding: str(encoding).lower().replace("-", "_"),
    "molt_codecs_register_error": _register_error,
    "molt_codecs_lookup_error": _lookup_error,
    "molt_codecs_bom_utf8": lambda: b"\\xef\\xbb\\xbf",
    "molt_codecs_bom_utf16_le": lambda: b"\\xff\\xfe",
    "molt_codecs_bom_utf16_be": lambda: b"\\xfe\\xff",
    "molt_codecs_bom_utf32_le": lambda: b"\\xff\\xfe\\x00\\x00",
    "molt_codecs_bom_utf32_be": lambda: b"\\x00\\x00\\xfe\\xff",
    "molt_codecs_incremental_encoder_new": lambda encoding, errors: (encoding, errors),
    "molt_codecs_incremental_encoder_encode": lambda handle, input, final=False: _codecs_encode(input, handle[0], handle[1]),
    "molt_codecs_incremental_encoder_reset": lambda handle: None,
    "molt_codecs_incremental_encoder_drop": lambda handle: None,
    "molt_codecs_incremental_decoder_new": lambda encoding, errors: (encoding, errors),
    "molt_codecs_incremental_decoder_decode": lambda handle, input, final=False: _codecs_decode(input, handle[0], handle[1]),
    "molt_codecs_incremental_decoder_reset": lambda handle: None,
    "molt_codecs_incremental_decoder_drop": lambda handle: None,
    "molt_codecs_stream_reader_new": lambda stream, encoding, errors: (stream, encoding, errors),
    "molt_codecs_stream_reader_read": lambda handle, size=-1, chars=-1, firstline=False: handle[0].read(),
    "molt_codecs_stream_reader_readline": lambda handle, size=None, keepends=True: handle[0].readline(),
    "molt_codecs_stream_reader_drop": lambda handle: None,
    "molt_codecs_stream_writer_new": lambda stream, encoding, errors: (stream, encoding, errors),
    "molt_codecs_stream_writer_write": lambda handle, obj: handle[0].write(_codecs_encode(obj, handle[1], handle[2])),
    "molt_codecs_stream_writer_drop": lambda handle: None,
    "molt_codecs_charmap_build": lambda decode_table: {{ord(ch): i for i, ch in enumerate(decode_table)}},
    "molt_codecs_charmap_decode": lambda data, errors, mapping: (bytes(data).decode("latin-1"), len(bytes(data))),
    "molt_codecs_charmap_encode": lambda data, errors, mapping: (str(data).encode("latin-1"), len(str(data))),
    "molt_codecs_make_identity_dict": lambda chars: {{ord(ch): ord(ch) for ch in chars}},
    "molt_pickle_dumps_core": lambda obj, protocol, fix_imports, _persistent_id, buffer_callback, _dispatch_table: __import__('pickle').dumps(
        obj, protocol=protocol, fix_imports=fix_imports, buffer_callback=buffer_callback
    ),
    "molt_pickle_loads_core": lambda data, fix_imports, encoding, errors, _persistent_load, _buffers_iter, buffers: __import__('pickle').loads(
        data, fix_imports=fix_imports, encoding=encoding, errors=errors, buffers=buffers
    ),
    "molt_errno_constants": lambda: ({{"EPERM": 1, "ENOENT": 2}}, {{1: "EPERM", 2: "ENOENT"}}),
    "molt_stat_constants": lambda: _stat_constants,
    "molt_stat_ifmt": lambda mode: int(mode) & 0xF000,
    "molt_stat_imode": lambda mode: int(mode) & 0o7777,
    "molt_stat_isdir": lambda mode: bool(int(mode) & 0x4000),
    "molt_stat_isreg": lambda mode: bool(int(mode) & 0x8000),
    "molt_stat_ischr": lambda mode: False,
    "molt_stat_isblk": lambda mode: False,
    "molt_stat_isfifo": lambda mode: False,
    "molt_stat_islnk": lambda mode: False,
    "molt_stat_issock": lambda mode: False,
    "molt_stat_isdoor": lambda mode: False,
    "molt_stat_isport": lambda mode: False,
    "molt_stat_iswht": lambda mode: False,
    "molt_stat_filemode": lambda mode: "mode",
    "molt_stdlib_probe": lambda: None,
    "molt_signal_raise": lambda sig: None,
    "molt_capabilities_trusted": lambda: True,
    "molt_capabilities_require": lambda cap: None,
    "molt_signal_signal": lambda sig, handler: 0,
    "molt_signal_getsignal": lambda sig: 0,
    "molt_signal_raise_signal": lambda sig: None,
    "molt_signal_alarm": lambda seconds: int(seconds),
    "molt_signal_pause": lambda: None,
    "molt_signal_set_wakeup_fd": lambda fd, warn_on_full_buffer=True: int(fd),
    "molt_signal_valid_signals": lambda: {{2, 15}},
    "molt_signal_strsignal": lambda sig: f"SIG{{sig}}",
    "molt_signal_pthread_sigmask": lambda how, signals: set(signals),
    "molt_signal_pthread_kill": lambda thread_id, sig: None,
    "molt_signal_sigpending": lambda: {{2}},
    "molt_signal_sigwait": lambda signals: min(signals),
    "molt_signal_default_int_handler": lambda signum=None, frame=None: None,
}}
for _name, _value in _signal_values.items():
    builtins._molt_intrinsics[_name] = (lambda value: (lambda: value))(_value)

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


codecs_mod = _load_module("molt_test_codecs", {str(STDLIB_ROOT / "codecs.py")!r})
pickle_mod = _load_module("molt_test_pickle", {str(STDLIB_ROOT / "pickle.py")!r})
errno_mod = _load_module("molt_test_errno", {str(STDLIB_ROOT / "errno.py")!r})
stat_mod = _load_module("molt_test_stat", {str(STDLIB_ROOT / "stat.py")!r})
signal_mod = _load_module("molt_test_signal", {str(STDLIB_ROOT / "signal.py")!r})

checks = {{
    "codecs": (
        codecs_mod.encode("hi", "utf-8") == b"hi"
        and codecs_mod.decode(b"hi", "utf-8") == "hi"
        and codecs_mod.BOM_UTF8 == b"\\xef\\xbb\\xbf"
        and "molt_codecs_encode" not in codecs_mod.__dict__
    ),
    "pickle": (
        pickle_mod.loads(pickle_mod.dumps({{"answer": 42}})) == {{"answer": 42}}
        and "molt_pickle_dumps_core" not in pickle_mod.__dict__
        and "molt_stdlib_probe" not in pickle_mod.__dict__
    ),
    "errno": (
        errno_mod.EPERM == 1
        and errno_mod.errorcode[1] == "EPERM"
        and "molt_errno_constants" not in errno_mod.__dict__
    ),
    "signal": (
        signal_mod.Signals.SIGINT == signal_mod.SIGINT
        and signal_mod.strsignal(signal_mod.SIGINT) == "SIG2"
        and signal_mod.valid_signals() == {{2, 15}}
        and "molt_signal_sigint" not in signal_mod.__dict__
    ),
    "stat": (
        stat_mod.S_IFMT(0x41ED) == (0x41ED & 0xF000)
        and stat_mod.S_ISDIR(0x4000) is True
        and stat_mod.filemode(0) == "mode"
        and "molt_stat_constants" not in stat_mod.__dict__
    ),
}}

for key in sorted(checks):
    print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def test_public_intrinsic_surface_batch_d() -> None:
    proc = subprocess.run(
        [sys.executable, "-c", _PROBE],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=True,
    )
    checks: dict[str, str] = {}
    for line in proc.stdout.splitlines():
        prefix, *rest = line.split("|")
        if prefix == "CHECK":
            checks[rest[0]] = rest[1]
    assert checks == {
        "codecs": "True",
        "errno": "True",
        "pickle": "True",
        "signal": "True",
        "stat": "True",
    }
