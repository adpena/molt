"""Actual Molt filesystem facades must preserve aggregate runtime refusals."""

from __future__ import annotations

import ast
import importlib.util
import os
import io
import logging
import threading
from pathlib import Path
import sys
from types import SimpleNamespace

import pytest
from tests.module_isolation import forget_module
from tests.process_guard_common import ModuleView
from tests.stdlib_intrinsic_registry import (
    intrinsic_registry,
    bind_struct_sequence_module,
    struct_sequence_intrinsics,
)

STDLIB = Path(__file__).resolve().parents[1] / "src/molt/stdlib"


def _unused(*args):
    pytest.fail(f"unexpected intrinsic call: {args!r}")


PATHLIB_HOOKS = (
    "molt_sys_version_info",
    "molt_capabilities_require",
    "molt_file_open_ex",
    "molt_os_lstat",
    "molt_os_rename",
    "molt_os_replace",
    "molt_os_stat",
    "molt_path_abspath",
    "molt_path_as_uri",
    "molt_path_chmod",
    "molt_path_dirname",
    "molt_path_exists",
    "molt_path_glob",
    "molt_path_isabs",
    "molt_path_isdir",
    "molt_path_isfile",
    "molt_path_islink",
    "molt_path_makedirs",
    "molt_path_match",
    "molt_path_mkdir",
    "molt_path_readlink",
    "molt_path_rmdir",
    "molt_path_symlink",
    "molt_path_unlink",
    "molt_pathlib_cwd",
    "molt_pathlib_group",
    "molt_pathlib_hardlink_to",
    "molt_pathlib_is_mount",
    "molt_pathlib_iterdir",
    "molt_pathlib_owner",
    "molt_pathlib_parts",
    "molt_pathlib_read_bytes",
    "molt_pathlib_resolve",
    "molt_pathlib_samefile",
    "molt_path_splitroot",
    "molt_pathlib_touch",
    "molt_pathlib_write_bytes",
)

ZIPFILE_HOOKS = (
    "molt_capabilities_require",
    "molt_deflate_raw",
    "molt_inflate_raw",
    "molt_zipfile_build_zip64_extra",
    "molt_zipfile_crc32",
    "molt_zipfile_normalize_member_path",
    "molt_zipfile_parse_central_directory",
    "molt_zipfile_path_implied_dirs",
    "molt_zipfile_path_is_child",
    "molt_zipfile_path_resolve_dir",
    "molt_zipfile_path_translate_glob",
)

LOGGING_HOOKS = (
    "molt_capabilities_require",
    "molt_logging_add_level_name",
    "molt_logging_filter_check",
    "molt_logging_get_level_name",
    "molt_logging_handler_close",
    "molt_logging_handler_drop",
    "molt_logging_handler_flush",
    "molt_logging_handler_new",
    "molt_logging_handler_set_level",
    "molt_logging_level_to_int",
    "molt_logging_logger_add_handler",
    "molt_logging_logger_drop",
    "molt_logging_logger_get_effective_level",
    "molt_logging_logger_is_enabled_for",
    "molt_logging_logger_new",
    "molt_logging_logger_remove_handler",
    "molt_logging_logger_set_level",
    "molt_logging_shutdown",
    "molt_rlock_acquire",
    "molt_rlock_drop",
    "molt_rlock_locked",
    "molt_rlock_new",
    "molt_rlock_release",
    "molt_thread_current_ident",
)


def _load(monkeypatch, name, relative, hooks, behavior):
    spec = importlib.util.spec_from_file_location(name, STDLIB / relative)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    monkeypatch.setitem(sys.modules, name, module)
    previous_children = {key for key in sys.modules if key.startswith(name + ".")}
    providers = dict.fromkeys(hooks, _unused)
    if "molt_sys_version_info" in providers:
        selected_version = tuple(sys.version_info)
        providers["molt_sys_version_info"] = lambda: selected_version
    selected = {**providers, **behavior}
    bind_struct_sequence_module(module, selected)
    with intrinsic_registry(selected):
        spec.loader.exec_module(module)
    for key in list(sys.modules):
        if key.startswith(name + ".") and key not in previous_children:
            child = sys.modules.pop(key)
            monkeypatch.setitem(sys.modules, key, child)
    return module


@pytest.mark.parametrize("parents", [False, True])
def test_pathlib_mkdir_preserves_selected_operation_denial(monkeypatch, parents):
    calls = []
    denial = PermissionError(
        "missing [fs.read, fs.write] capabilities"
        if parents
        else "missing [fs.write] capabilities"
    )

    def reject(*args):
        calls.append(args)
        raise denial

    module = _load(
        monkeypatch,
        "_molt_pathlib_capability",
        "pathlib/__init__.py",
        PATHLIB_HOOKS,
        {
            "molt_path_makedirs" if parents else "molt_path_mkdir": reject,
        },
    )
    # Native admission receives the actual facade before any path parsing.
    path = module.Path("/tmp/selected")
    with pytest.raises(PermissionError) as caught:
        path.mkdir(parents=parents, mode=0o700, exist_ok=True)
    assert caught.value is denial
    expected = (path, 0o700, True) if parents else (path, 0o700)
    assert calls == [expected]


@pytest.mark.parametrize("mode", ["r", "w"])
def test_zipfile_preserves_complete_selected_open_denial(monkeypatch, mode):
    module = _load(
        monkeypatch,
        "_molt_zipfile_capability",
        "zipfile/__init__.py",
        ZIPFILE_HOOKS,
        {},
    )
    denial = PermissionError(
        "missing [fs.read, fs.tmp.read] capabilities"
        if mode == "r"
        else "missing [fs.write, fs.tmp.write] capabilities"
    )
    calls = []

    def reject(*args, **kwargs):
        calls.append((args, kwargs))
        raise denial

    monkeypatch.setattr(module, "open", reject, raising=False)
    with pytest.raises(PermissionError) as caught:
        module.ZipFile("/tmp/selected.zip", mode)
    assert caught.value is denial
    assert calls == [(("/tmp/selected.zip", mode + "b"), {})]


def _logging_module(monkeypatch):
    def compatibility_error(outcome):
        assert outcome == 4
        return NotImplementedError("emit must be implemented by Handler subclasses")

    _load(
        monkeypatch,
        "_compatibility_errors",
        "_compatibility_errors.py",
        ("molt_compatibility_error",),
        {"molt_compatibility_error": compatibility_error},
    )
    # Import-time root/lastResort construction and teardown use these concrete
    # hooks. All other hooks fail if unexpectedly exercised.
    behavior = {
        "molt_logging_logger_new": lambda *args: 1,
        "molt_logging_logger_set_level": lambda *args: None,
        "molt_logging_level_to_int": lambda value: value,
        "molt_logging_get_level_name": logging.getLevelName,
        "molt_thread_current_ident": threading.get_ident,
        "molt_rlock_new": threading.RLock,
        "molt_rlock_acquire": lambda lock, blocking, timeout: lock.acquire(
            blocking, timeout
        ),
        "molt_rlock_release": lambda lock: lock.release(),
        "molt_logging_handler_close": lambda *args: None,
        "molt_logging_handler_new": lambda *args: 3,
        "molt_logging_handler_set_level": lambda *args: None,
        "molt_logging_handler_drop": lambda *args: None,
        "molt_logging_logger_drop": lambda *args: None,
        "molt_rlock_drop": lambda *args: None,
    }
    return _load(
        monkeypatch,
        "_molt_logging_capability",
        "logging/__init__.py",
        LOGGING_HOOKS,
        behavior,
    )


@pytest.mark.parametrize("mode", ["r", "a", "r+"])
def test_filehandler_preserves_complete_selected_open_denial(monkeypatch, mode):
    module = _logging_module(monkeypatch)
    handler = module.FileHandler("/tmp/selected.log", mode=mode, delay=True)
    previous_stream = handler.stream
    denial = PermissionError("complete selected operation refusal")
    calls = []

    def reject(*args, **kwargs):
        calls.append((args, kwargs))
        raise denial

    monkeypatch.setattr(handler, "_builtin_open", reject)
    with pytest.raises(PermissionError) as caught:
        handler._open()
    assert caught.value is denial
    assert calls == [
        (
            ("/tmp/selected.log", mode),
            {"encoding": io.text_encoding(None), "errors": None},
        )
    ]
    assert handler.stream is previous_stream


# Control flow oracle: pinned CPython 3.12 logging.StreamHandler/FileHandler.
# The file and StringIO operations below are real host I/O; only unrelated
# record/handler allocation and synchronization intrinsics are supplied.
@pytest.mark.parametrize("delay", [False, True])
def test_filehandler_readonly_emission_cannot_mutate(monkeypatch, tmp_path, delay):
    module = _logging_module(monkeypatch)
    path = tmp_path / "readonly.log"
    path.write_bytes(b"original\n")
    handler = module.FileHandler(path, mode="r", delay=delay)
    errors = []
    handler.format = lambda record: "forbidden"
    handler.handleError = lambda record: errors.append(sys.exc_info()[1])
    assert (handler.stream is None) is delay
    try:
        handler.emit(SimpleNamespace())
        assert len(errors) == 1
        assert isinstance(errors[0], io.UnsupportedOperation)
        assert path.read_bytes() == b"original\n"
    finally:
        handler.close()


@pytest.mark.parametrize("delay", [False, True])
def test_filehandler_persistent_stream_and_closed_write_mode(
    monkeypatch, tmp_path, delay
):
    module = _logging_module(monkeypatch)
    path = tmp_path / "output.log"
    handler = module.FileHandler(path, mode="w", delay=delay)
    assert path.exists() is (not delay)
    seen = []

    def format_record(record):
        seen.append(record.msg)
        return "custom:" + record.msg

    handler.format = format_record
    handler.terminator = "|"
    handler.emit(SimpleNamespace(msg="one"))
    stream = handler.stream
    handler.emit(SimpleNamespace(msg="two"))
    assert handler.stream is stream
    assert path.read_bytes() == b"custom:one|custom:two|"
    handler.close()
    assert stream.closed and handler.stream is None and handler._closed
    handler.emit(SimpleNamespace(msg="after close"))
    assert seen == ["one", "two"]
    assert path.read_bytes() == b"custom:one|custom:two|"


def test_delayed_filehandler_emit_preserves_open_denial_without_mutation(
    monkeypatch, tmp_path
):
    module = _logging_module(monkeypatch)
    path = tmp_path / "denied.log"
    path.write_bytes(b"keep")
    handler = module.FileHandler(path, mode="w", delay=True)
    denial = PermissionError("missing fs.write, mount.write")
    calls = []

    def reject(*args, **kwargs):
        calls.append((args, kwargs))
        raise denial

    handler.format = lambda record: pytest.fail("formatted before denied open")
    handler.handleError = lambda record: pytest.fail("open denial was swallowed")
    monkeypatch.setattr(handler, "_builtin_open", reject)
    with pytest.raises(PermissionError) as caught:
        handler.emit(SimpleNamespace())
    assert caught.value is denial
    assert calls == [
        ((str(path), "w"), {"encoding": io.text_encoding(None), "errors": None})
    ]
    assert handler.stream is None
    assert path.read_bytes() == b"keep"
    handler.close()


def test_streamhandler_uses_selected_stream_formatter_terminator_and_filter(
    monkeypatch,
):
    module = _logging_module(monkeypatch)
    stream = io.StringIO()
    handler = module.StreamHandler(stream)
    formatted = []

    def format_record(record):
        formatted.append(record.msg)
        return record.msg.upper()

    handler.formatter = SimpleNamespace(format=format_record)
    handler.terminator = "!"
    handler.addFilter(SimpleNamespace(filter=lambda record: record.msg != "blocked"))
    assert not handler.handle(SimpleNamespace(msg="blocked"))
    assert handler.handle(SimpleNamespace(msg="allowed"))
    assert formatted == ["allowed"]
    assert stream.getvalue() == "ALLOWED!"
    handler.close()
    assert not stream.closed


@pytest.mark.parametrize("stage", ["format", "write", "flush"])
@pytest.mark.parametrize("error_type", [ValueError, RecursionError])
def test_streamhandler_emission_error_protocol(monkeypatch, stage, error_type):
    module = _logging_module(monkeypatch)
    error = error_type(stage)
    seen = []

    def fail(*args):
        raise error

    class Stream(io.StringIO):
        def write(self, text):
            if stage == "write":
                fail()
            return super().write(text)

        def flush(self):
            if stage == "flush":
                fail()
            return super().flush()

    stream = Stream()
    handler = module.StreamHandler(stream)
    handler.format = fail if stage == "format" else lambda record: "message"
    handler.handleError = lambda record: seen.append(sys.exc_info()[1])
    record = SimpleNamespace()
    if error_type is RecursionError:
        with pytest.raises(RecursionError) as caught:
            handler.handle(record)
        assert caught.value is error
        assert seen == []
    else:
        assert handler.handle(record)
        assert seen == [error]
    # A different thread can acquire the actual reentrant lock after failure.
    acquired = []

    def acquire_from_thread():
        acquired.append(handler.lock.acquire(False))
        if acquired[-1]:
            handler.lock.release()

    thread = threading.Thread(target=acquire_from_thread)
    thread.start()
    thread.join()
    assert acquired == [True]
    handler.close()


def test_filehandler_close_failure_still_retires_handler(monkeypatch, tmp_path):
    module = _logging_module(monkeypatch)
    handler = module.FileHandler(tmp_path / "unused", delay=True)
    error = OSError("flush failed")

    class Stream(io.StringIO):
        def flush(self):
            raise error

    stream = Stream()
    handler.stream = stream
    with pytest.raises(OSError) as caught:
        handler.close()
    assert caught.value is error
    assert stream.closed and handler.stream is None and handler._closed
    assert handler not in module._handler_list
    handler.close()


def test_streamhandler_handleerror_respects_raiseexceptions(monkeypatch):
    module = _logging_module(monkeypatch)
    stream = io.StringIO()
    stream.close()
    handler = module.StreamHandler(stream)
    handler.format = lambda record: "message"
    monkeypatch.setattr(module, "raiseExceptions", False)
    monkeypatch.setattr(module, "_traceback", SimpleNamespace(print_exception=_unused))
    handler.emit(SimpleNamespace())
    handler.close()


def _logging_handlers_module(monkeypatch):
    module = _logging_module(monkeypatch)
    with monkeypatch.context() as patch:
        patch.setitem(sys.modules, "logging", module)
        forget_module(patch, "logging.handlers")
        handlers = importlib.import_module("logging.handlers")
    return module, handlers


def test_loading_molt_logging_handlers_leaves_sys_modules_as_found(
    monkeypatch,
) -> None:
    """Whether or not the host had imported logging's submodules, a test that
    loads Molt's leaves none of its modules behind."""
    for key in ("logging.handlers", "logging.config"):
        forget_module(monkeypatch, key)
    real_logging = sys.modules["logging"]
    with pytest.MonkeyPatch.context() as patch:
        _logging_handlers_module(patch)
    assert sys.modules["logging"] is real_logging
    assert "logging.handlers" not in sys.modules
    assert "logging.config" not in sys.modules


def test_logging_handlers_use_normal_package_identity(monkeypatch):
    module = _logging_module(monkeypatch)
    assert "handlers" not in vars(module)
    assert "handlers" not in module.__all__
    assert not any(
        name in vars(module)
        for name in (
            "BaseRotatingHandler",
            "TimedRotatingFileHandler",
            "QueueHandler",
            "QueueListener",
        )
    )
    with monkeypatch.context() as patch:
        patch.setitem(sys.modules, "logging", module)
        forget_module(patch, "logging.handlers")
        forget_module(patch, "logging.config")
        handlers = importlib.import_module("logging.handlers")
        with intrinsic_registry(
            {
                name: _unused
                for name in (
                    "molt_logging_config_dict",
                    "molt_logging_config_valid_ident",
                    "molt_logging_config_file_config",
                    "molt_logging_config_listen",
                    "molt_logging_config_stop_listening",
                )
            }
        ):
            config = importlib.import_module("logging.config")
        assert (
            handlers is module.handlers is importlib.import_module("logging.handlers")
        )
        assert handlers.__file__ == str(STDLIB / "logging/handlers.py")
        assert config.logging is module
        assert handlers.RotatingFileHandler.__module__ == "logging.handlers"


def test_filehandler_retains_path_open_and_encoding_errors(monkeypatch, tmp_path):
    module = _logging_module(monkeypatch)
    monkeypatch.chdir(tmp_path)
    handler = module.FileHandler(
        Path("relative.log"), encoding="ascii", errors="backslashreplace", delay=True
    )
    assert handler.baseFilename == str(tmp_path / "relative.log")
    assert handler.errors == "backslashreplace"
    captured_open = handler._builtin_open
    monkeypatch.setattr(
        module,
        "open",
        lambda *args, **kwargs: pytest.fail("replaced open used"),
        raising=False,
    )
    (tmp_path / "elsewhere").mkdir()
    monkeypatch.chdir(tmp_path / "elsewhere")
    handler.format = lambda record: "caf\N{LATIN SMALL LETTER E WITH ACUTE}"
    try:
        handler.emit(SimpleNamespace())
        assert handler._builtin_open is captured_open
        assert (tmp_path / "relative.log").read_bytes() == b"caf\\xe9\n"
    finally:
        handler.close()


def test_rotating_uses_position_without_seeking_and_skips_empty_format(
    monkeypatch, tmp_path
):
    _, handlers = _logging_handlers_module(monkeypatch)
    handler = handlers.RotatingFileHandler(
        tmp_path / "position", maxBytes=5, delay=True
    )
    events = []

    class Stream:
        position = 0

        def tell(self):
            events.append("tell")
            return self.position

        def seek(self, *args):
            pytest.fail("rollover must not seek to end")

        def flush(self):
            pass

        def close(self):
            pass

    stream = Stream()
    handler.stream = stream
    handler.format = lambda record: events.append("format") or "abcd"
    assert not handler.shouldRollover(SimpleNamespace())
    assert events == ["tell"]
    stream.position = 1
    assert handler.shouldRollover(SimpleNamespace())
    assert events == ["tell", "tell", "format"]
    handler.maxBytes = 0
    assert not handler.shouldRollover(SimpleNamespace())
    assert events == ["tell", "tell", "format"]
    handler.close()


@pytest.mark.parametrize("stage", ["open", "format", "namer", "rotator"])
def test_rotating_forwards_original_failure_once(monkeypatch, tmp_path, stage):
    _, handlers = _logging_handlers_module(monkeypatch)
    path = tmp_path / "failure.log"
    path.write_text("prior\n", encoding="utf-8")
    handler = handlers.RotatingFileHandler(path, maxBytes=1, backupCount=1, delay=True)
    error = OSError("rotation refusal")
    seen = []

    def reject(*args, **kwargs):
        raise error

    handler.format = lambda record: "next"
    handler.handleError = lambda record: seen.append(sys.exc_info()[1])
    if stage == "open":
        handler._builtin_open = reject
    elif stage == "format":
        handler.format = reject
    else:
        setattr(handler, stage, reject)
    handler.emit(SimpleNamespace())
    assert seen == [error]
    assert path.read_text(encoding="utf-8") == "prior\n"
    assert not (tmp_path / "failure.log.1").exists()
    handler.close()


def test_timed_constructor_uses_same_path_errors_and_delay_owner(monkeypatch, tmp_path):
    _, handlers = _logging_handlers_module(monkeypatch)

    class PathLike:
        def __fspath__(self):
            return str(tmp_path / "timed.log")

        def __str__(self):
            raise AssertionError("PathLike must use fspath")

    handler = handlers.TimedRotatingFileHandler(
        PathLike(), when="S", encoding="ascii", errors="backslashreplace", delay=True
    )
    assert handler.errors == "backslashreplace"
    assert handler.encoding == "ascii"
    assert handler.stream is None
    handler.doRollover()
    assert handler.stream is None
    assert not (tmp_path / "timed.log").exists()
    handler.close()


def test_rotating_refuses_nonregular_rollover_after_format(monkeypatch, tmp_path):
    _, handlers = _logging_handlers_module(monkeypatch)
    handler = handlers.RotatingFileHandler(tmp_path, maxBytes=1, delay=True)
    events = []
    handler.stream = SimpleNamespace(tell=lambda: 1)
    handler.format = lambda record: events.append("format") or "message"
    try:
        assert handler.shouldRollover(SimpleNamespace()) is False
        assert events == ["format"]
    finally:
        handler.stream = None
        handler.close()


def test_handler_default_formatter_is_shared_without_record_allocations(monkeypatch):
    module = _logging_module(monkeypatch)
    first, second = module.Handler(), module.Handler()
    record = SimpleNamespace(
        getMessage=lambda: "selected", exc_info=None, stack_info=None
    )
    default = module._defaultFormatter
    monkeypatch.setattr(module.Formatter, "__init__", _unused)
    try:
        assert first.format(record) == second.format(record) == "selected"
        assert module._defaultFormatter is default
        first.formatter = SimpleNamespace(format=lambda record: "explicit")
        assert first.format(record) == "explicit"
        assert second.format(record) == "selected"
    finally:
        first.close()
        second.close()


def test_logging_record_and_formatter_share_live_python_fields(monkeypatch):
    module = _logging_module(monkeypatch)
    for owner in (logging, module):
        record = owner.LogRecord("live", owner.INFO, "fixture", 1, "old", (), None)
        record.msg, record.args = "updated %s", ("value",)
        record.custom = "extra"
        for style, fmt in (
            ("%", "%(custom)s:%(message)s"),
            ("{", "{custom}:{message}"),
            ("$", "$custom:$message"),
        ):
            formatter = owner.Formatter(fmt, style=style)
            assert formatter.format(record) == "extra:updated value"
            assert record.message == "updated value"
        record.msg, record.args = "%(value)s", ({"value": "mapping"},)
        # A mapping passed as the sole constructor argument is unwrapped.
        mapped = owner.LogRecord(
            "live", owner.INFO, "fixture", 1, "%(value)s", ({"value": "mapping"},), None
        )
        assert mapped.getMessage() == "mapping"
        record.args = {"value": "edited mapping"}
        assert record.getMessage() == "edited mapping"
        record.msg, record.args = None, ()
        assert record.getMessage() == "None"
        with pytest.raises(ValueError, match="Formatting field not found in record"):
            owner.Formatter("%(missing)s").format(record)


def test_logging_record_defers_message_callbacks_and_preserves_failure(monkeypatch):
    module = _logging_module(monkeypatch)
    for owner in (logging, module):
        calls = []
        failure = ValueError("message callback")

        class Message:
            def __str__(self):
                calls.append("str")
                raise failure

        record = owner.LogRecord("live", owner.INFO, "fixture", 1, Message(), (), None)
        assert calls == []
        with pytest.raises(ValueError) as caught:
            owner.Formatter().format(record)
        assert caught.value is failure
        assert calls == ["str"]


def test_logging_record_owns_supplied_process_metadata_without_native_snapshot(
    monkeypatch,
):
    module = _logging_module(monkeypatch)
    calls = []

    def getpid():
        calls.append("getpid")
        return 73

    monkeypatch.setattr(module, "_os", ModuleView(os, getpid=getpid))
    for enabled, expected, count in ((True, 73, 1), (False, None, 1)):
        module.logProcesses = enabled
        record = module.LogRecord(
            "metadata", module.INFO, "fixture", 1, "msg", (), None
        )
        assert record.process == expected
        assert module.Formatter("%(process)s").format(record) == str(expected)
        assert len(calls) == count
        assert not hasattr(record, "_handle")


def test_logging_formatter_time_uses_current_fields_and_selected_converter(monkeypatch):
    module = _logging_module(monkeypatch)
    for owner in (logging, module):
        record = owner.LogRecord("time", owner.INFO, "fixture", 1, "msg", (), None)
        record.created, record.msecs = 0, 7
        formatter = owner.Formatter("%(asctime)s:%(message)s")
        calls = []

        def converter(value):
            calls.append(value)
            return (2001, 2, 3, 4, 5, 6, 5, 34, 0)

        formatter.converter = converter
        assert formatter.format(record) == "2001-02-03 04:05:06,007:msg"
        record.created = 91
        formatter.default_time_format = "%H:%M"
        formatter.default_msec_format = None
        assert formatter.format(record) == "04:05:msg"
        assert formatter.formatTime(record, "%Y") == "2001"
        assert calls == [0, 91, 91]


def test_pathlib_text_methods_use_selected_open_and_preserve_failure_order(
    monkeypatch, tmp_path
):
    calls = []

    def selected_open(
        path, mode, buffering, encoding, errors, newline, closefd, opener
    ):
        calls.append((mode, encoding, errors, newline))
        return open(
            path,
            mode,
            buffering=buffering,
            encoding=encoding,
            errors=errors,
            newline=newline,
            closefd=closefd,
            opener=opener,
        )

    module = _load(
        monkeypatch,
        "_molt_pathlib_text",
        "pathlib/__init__.py",
        PATHLIB_HOOKS,
        {
            "molt_file_open_ex": selected_open,
            "molt_path_splitroot": lambda value, posix, wide, pathlib: (
                Path(value).drive,
                Path(value).root,
                "unused tail",
            ),
            "molt_pathlib_parts": lambda value, posix, wide: Path(value).parts,
        },
    )
    # Exercise the real Molt provider helper, not the host io import. Native
    # class storage stays the existing independent host facade boundary.
    import _io as host_provider

    promotions = []
    provider = _load(
        monkeypatch,
        "_molt_pathlib_io_provider",
        "_io.py",
        (),
        {
            "molt_io_class": lambda name: getattr(host_provider, name),
            "molt_io_default_buffer_size": lambda: host_provider.DEFAULT_BUFFER_SIZE,
            # CPython's C rule, which the runtime intrinsic implements.
            "molt_io_text_encoding": host_provider.text_encoding,
            "molt_builtin_class_lookup": lambda name: getattr(host_provider, name),
            "molt_function_set_builtin": promotions.append,
        },
    )
    module._io = provider
    assert promotions == [provider.text_encoding]
    target = tmp_path / "text.txt"
    path = module.Path(str(target))
    assert path.write_text("caf\xe9\n", encoding="latin-1", newline="\r\n") == 5
    assert target.read_bytes() == b"caf\xe9\r\n"
    assert path.read_text(encoding="latin-1") == "caf\xe9\n"
    assert calls == [("w", "latin-1", None, "\r\n"), ("r", "latin-1", None, None)]
    with pytest.raises(TypeError, match="data must be str"):
        path.write_text(42, encoding="utf-8")
    assert len(calls) == 2
    assert target.read_bytes() == b"caf\xe9\r\n"
    with pytest.raises(LookupError):
        path.write_text("replacement", encoding="molt-invalid-codec")
    assert target.read_bytes() == b""
    missing = module.Path(str(tmp_path / "missing"))
    with pytest.raises(FileNotFoundError):
        missing.read_text(encoding="molt-invalid-codec")


def _os_module(monkeypatch, behavior):
    # Unexercised imports fail on invocation. Deriving only the import inventory
    # keeps this loader separate from the independent boundary assertions below.
    tree = ast.parse((STDLIB / "os.py").read_text(encoding="utf-8"))
    hooks = {
        node.args[0].value
        for node in ast.walk(tree)
        if isinstance(node, ast.Call)
        and isinstance(node.func, ast.Name)
        and node.func.id in {"_require_intrinsic", "_require_os_intrinsic"}
        and node.args
        and isinstance(node.args[0], ast.Constant)
        and isinstance(node.args[0].value, str)
    }
    constants = {
        "name": os.name,
        "sep": os.sep,
        "altsep": os.altsep,
        "curdir": os.curdir,
        "pardir": os.pardir,
        "extsep": os.extsep,
        "pathsep": os.pathsep,
        "linesep": os.linesep,
        "devnull": os.devnull,
    }
    defaults = {
        "molt_os_" + name: (lambda value=value: value)
        for name, value in constants.items()
    }
    defaults.update(
        {
            "molt_capabilities_require": lambda capability: None,
            "molt_capabilities_require_operation": lambda operation: None,
            "molt_env_mapping_init": lambda: None,
            "molt_env_mapping_keys": lambda: [],
            "molt_env_mapping_len": lambda: 0,
            "molt_os_open_flags": lambda: [0] * 9,
            "molt_os_sysconf_names": lambda: None,
            "molt_os_fspath": os.fspath,
        }
    )
    native_records = struct_sequence_intrinsics()
    return _load(
        monkeypatch,
        "_molt_os_paths",
        "os.py",
        hooks,
        {**native_records, **defaults, **behavior},
    )


@pytest.mark.parametrize(
    "operation,intrinsic,kwargs,arguments",
    [
        ("path.exists", "molt_path_exists", {}, ()),
        ("path.isdir", "molt_path_isdir", {}, ()),
        ("path.isfile", "molt_path_isfile", {}, ()),
        ("path.islink", "molt_path_islink", {}, ()),
        ("readlink", "molt_path_readlink", {}, ()),
        ("readlink", "molt_os_readlink_at", {"dir_fd": 7}, (7,)),
        ("stat", "molt_os_stat", {}, ()),
        ("stat", "molt_os_lstat", {"follow_symlinks": False}, ()),
        ("stat", "molt_os_stat_at", {"dir_fd": 7}, (7, True)),
        ("lstat", "molt_os_lstat", {}, ()),
        ("lstat", "molt_os_lstat_at", {"dir_fd": 7}, (7,)),
        ("fstat", "molt_os_fstat", {}, ()),
    ],
)
def test_os_metadata_uses_one_selected_runtime_admission(
    monkeypatch, operation, intrinsic, kwargs, arguments
):
    # The runtime selects the mount after fspath and reports the complete grant
    # set. A facade-level fs.read check would hide the mount-specific refusal.
    refusal = PermissionError(
        "missing [fs.read] capabilities"
        if operation == "fstat"
        else "missing [fs.read, fs.tmp.read] capabilities"
    )
    path = 7 if operation == "fstat" else b"/tmp/raw/\xff"
    seen = []

    def denied(*args):
        seen.append(args)
        raise refusal

    module = _os_module(
        monkeypatch,
        {intrinsic: denied, "molt_capabilities_require": _unused},
    )
    owner = module.path if operation.startswith("path.") else module
    with pytest.raises(PermissionError) as caught:
        getattr(owner, operation.split(".")[-1])(path, **kwargs)
    assert caught.value is refusal
    assert seen == [(path, *arguments)]


@pytest.mark.parametrize(
    "method,intrinsic",
    [
        ("exists", "molt_path_exists"),
        ("is_dir", "molt_path_isdir"),
        ("is_file", "molt_path_isfile"),
        ("is_symlink", "molt_path_islink"),
        ("readlink", "molt_path_readlink"),
        ("stat", "molt_os_stat"),
        ("lstat", "molt_os_lstat"),
    ],
)
def test_pathlib_metadata_preserves_selected_mount_denial(
    monkeypatch, method, intrinsic
):
    refusal = PermissionError("missing [fs.read, fs.bundle.read] capabilities")
    seen = []

    def denied(path):
        seen.append(path)
        raise refusal

    module = _load(
        monkeypatch,
        "_molt_pathlib_metadata",
        "pathlib/__init__.py",
        PATHLIB_HOOKS,
        {intrinsic: denied},
    )
    path = module.Path("/bundle/selected")
    with pytest.raises(PermissionError) as caught:
        getattr(path, method)()
    assert caught.value is refusal
    assert seen == [path]


@pytest.mark.parametrize(
    "operation,intrinsic,result",
    [
        ("path.getatime", "molt_os_path_getatime", 1.5),
        ("path.getctime", "molt_os_path_getctime", 1.5),
        ("path.getmtime", "molt_os_path_getmtime", 1.5),
        ("path.getsize", "molt_os_path_getsize", 3),
        ("path.realpath", "molt_os_path_realpath", b"/raw/\xff"),
        ("chdir", "molt_os_chdir", None),
        ("removedirs", "molt_os_removedirs", None),
        ("readlink", "molt_path_readlink", b"../\xff"),
    ],
)
def test_os_path_facades_preserve_the_runtime_path_protocol(
    monkeypatch, operation, intrinsic, result
):
    class PathLike:
        calls = 0

        def __fspath__(self):
            self.calls += 1
            return b"/raw/\xff"

        def __str__(self):
            pytest.fail("path protocol was replaced by str")

    path = PathLike()
    seen = []

    def runtime(value):
        seen.append(value)
        assert os.fspath(value) == b"/raw/\xff"
        return result

    module = _os_module(monkeypatch, {intrinsic: runtime})
    owner = module.path if operation.startswith("path.") else module
    assert getattr(owner, operation.split(".")[-1])(path) == result
    assert seen == [path]
    assert path.calls == 1


@pytest.mark.parametrize(
    "operation,intrinsic,extra",
    [
        ("access", "molt_os_access", (4,)),
        ("truncate", "molt_os_truncate", (2,)),
        ("link", "molt_os_link", (b"/other/\xfe",)),
        ("path.samefile", "molt_os_path_samefile", (b"/other/\xfe",)),
        ("readlink", "molt_os_readlink_at", ()),
        ("utime", "molt_os_utime", ()),
    ],
)
def test_os_path_facades_preserve_bytes_and_keyword_branch_arguments(
    monkeypatch, operation, intrinsic, extra
):
    seen = []
    path = b"/raw/\xff"
    result = b"../\xfe" if operation == "readlink" else True

    def runtime(*args):
        seen.append(args)
        return result

    module = _os_module(monkeypatch, {intrinsic: runtime})
    owner = module.path if operation.startswith("path.") else module
    kwargs = {"dir_fd": 7} if operation == "readlink" else {}
    value = getattr(owner, operation.split(".")[-1])(path, *extra, **kwargs)
    expected = (path, *extra)
    if operation == "readlink":
        expected = (path, 7)
        assert value is result
    elif operation == "utime":
        expected = (path, None, None)
    assert seen == [expected]


def test_os_owned_results_preserve_flavor_without_scans_or_copies(monkeypatch):
    class UnscannableList(list):
        def __iter__(self):
            pytest.fail("listdir must return its owned runtime list directly")

    path = b"/raw/\xff"
    names = UnscannableList([b"\xfe"])
    native_stat = os.stat_result(
        tuple(range(10)), {"st_atime": 70.25, "st_atime_ns": 70_250_000_001}
    )
    stat_calls = []

    def metadata(*args):
        stat_calls.append(args)
        return native_stat

    stat_providers = dict.fromkeys(
        (
            "molt_os_stat",
            "molt_os_lstat",
            "molt_os_fstat",
            "molt_os_stat_at",
            "molt_os_lstat_at",
        ),
        metadata,
    )
    module = _os_module(
        monkeypatch,
        {
            **stat_providers,
            "molt_os_listdir": lambda value: names,
            "molt_env_mapping_keys": lambda: [b"SELECTED"],
            "molt_env_mapping_len": lambda: 1,
            "molt_env_mapping_get": lambda key: b"value",
            "molt_env_mapping_delete": lambda key: None,
            "molt_os_path_commonpath": lambda paths, windows_utf16: path,
        },
    )
    assert module.listdir(path) is names
    assert module.environ.copy() == {"SELECTED": "value"}
    assert list(module.environ) == ["SELECTED"]
    assert module.environ.popitem() == ("SELECTED", "value")
    assert module.path.commonpath(iter([path, path + b"/child"])) is path
    # One acquisition returns the native record itself, including fields beyond
    # its visible tuple. Public type rebinding cannot re-enter construction.
    module.stat_result = object
    entry = module.DirEntry(b"entry", path, False, True, False)
    with intrinsic_registry(stat_providers):
        for query, expected in (
            (lambda: module.stat(path), (path,)),
            (lambda: module.stat(path, follow_symlinks=False), (path,)),
            (lambda: module.stat(path, dir_fd=7), (path, 7, True)),
            (
                lambda: module.stat(path, dir_fd=7, follow_symlinks=False),
                (path, 7, False),
            ),
            (lambda: module.lstat(path), (path,)),
            (lambda: module.lstat(path, dir_fd=7), (path, 7)),
            (lambda: module.fstat(7), (7,)),
            (entry.stat, (path,)),
            (lambda: entry.stat(follow_symlinks=False), (path,)),
        ):
            before = len(stat_calls)
            assert query() is native_stat
            assert stat_calls[before:] == [expected]
    assert native_stat[7] == 7 and native_stat.st_atime == 70.25
    assert native_stat.st_atime_ns == 70_250_000_001


@pytest.mark.parametrize("path", [b"/raw/\xff", "/raw/\udcff"])
def test_os_directory_entries_and_walk_keep_runtime_path_flavor(monkeypatch, path):
    name = b"\xfe" if isinstance(path, bytes) else "\udcfe"
    child = os.path.join(path, name)
    triplet = (path, [], [name])
    calls = []

    def scandir(value):
        calls.append(("scandir", value))
        return [(name, child, False, True, False)]

    def walk(value, topdown, followlinks):
        calls.append(("walk", value, topdown, followlinks))
        return [triplet]

    module = _os_module(
        monkeypatch,
        {
            "molt_os_scandir": scandir,
            "molt_os_walk": walk,
        },
    )
    (entry,) = module.scandir(path)
    assert entry.name is name
    assert entry.path is child
    assert os.fspath(entry) is child
    assert entry.is_file() and not entry.is_dir() and not entry.is_symlink()
    assert next(module.walk(path, topdown=False, followlinks=True)) is triplet
    assert calls == [("scandir", path), ("walk", path, False, True)]


@pytest.mark.parametrize(
    "operation,intrinsic",
    [
        ("listdir", "molt_os_listdir"),
        ("scandir", "molt_os_scandir"),
        ("walk", "molt_os_walk"),
        ("snapshot", "molt_env_mapping_keys"),
    ],
)
def test_os_facades_preserve_runtime_capability_refusals(
    monkeypatch, operation, intrinsic
):
    refusal = PermissionError("selected runtime capability denied")

    def denied(*args):
        raise refusal

    module = _os_module(monkeypatch, {intrinsic: denied})
    with pytest.raises(PermissionError) as caught:
        if operation == "snapshot":
            module.environ.copy()
        elif operation == "walk":
            next(module.walk(b"/raw/\xff"))
        else:
            getattr(module, operation)(b"/raw/\xff")
    assert caught.value is refusal


@pytest.mark.parametrize(
    "grants", [(), ("env.read",), ("env.write",), ("env.read", "env.write")]
)
@pytest.mark.parametrize("operation", ["pop", "popitem", "setdefault", "clear"])
def test_environ_compound_admission_reports_all_missing_before_callbacks(
    monkeypatch, grants, operation
):
    from molt._host_capabilities_generated import OperationId, OPERATION_CAPABILITIES

    storage = {b"A": b"one", b"Z": b"two", b"B": b"three"}
    calls = []

    def require(name):
        required = OPERATION_CAPABILITIES[OperationId(name)]
        missing = [str(cap) for cap in required if str(cap) not in grants]
        if missing:
            raise PermissionError(
                f"missing [{', '.join(missing)}] capabilities for '{name}'"
            )

    def get(key):
        calls.append(("get", key))
        return storage[key]

    def put(key, value):
        calls.append(("set", key))
        storage[key] = value

    def delete(key):
        calls.append(("delete", key))
        del storage[key]

    module = _os_module(
        monkeypatch,
        {
            "molt_capabilities_require_operation": require,
            "molt_env_mapping_get": get,
            "molt_env_mapping_set": put,
            "molt_env_mapping_delete": delete,
            "molt_env_mapping_keys": lambda: list(storage),
            "molt_env_mapping_len": lambda: len(storage),
        },
    )
    encode = module.environ.encodekey

    def counted(key):
        calls.append(("encode", key))
        return encode(key)

    module.environ.encodekey = counted
    invoke = {
        "pop": lambda: module.environ.pop("A"),
        "popitem": module.environ.popitem,
        "setdefault": lambda: module.environ.setdefault("A", None),
        "clear": module.environ.clear,
    }[operation]
    missing = [cap for cap in ("env.read", "env.write") if cap not in grants]
    if missing:
        with pytest.raises(PermissionError) as error:
            invoke()
        assert (
            str(error.value)
            == f"missing [{', '.join(missing)}] capabilities for 'env.{operation}'"
        )
        assert calls == []
        assert storage == {b"A": b"one", b"Z": b"two", b"B": b"three"}
    else:
        result = invoke()
        assert (
            result
            == {
                "pop": "one",
                "popitem": ("A", "one"),
                "setdefault": "one",
                "clear": None,
            }[operation]
        )


def test_environ_order_views_first_advance_and_bytes_share_one_boundary(monkeypatch):
    storage = {b"Z": b"one", b"A": b"two"}
    snapshots = []

    def keys():
        snapshots.append(tuple(storage))
        return list(storage)

    module = _os_module(
        monkeypatch,
        {
            "molt_env_mapping_get": storage.__getitem__,
            "molt_env_mapping_set": storage.__setitem__,
            "molt_env_mapping_delete": storage.__delitem__,
            "molt_env_mapping_keys": keys,
            "molt_env_mapping_len": storage.__len__,
            "molt_env_mapping_items_iter": lambda: iter(storage.items()),
        },
    )
    view = module.environ.items()
    iterator = iter(module.environ)
    assert snapshots == []
    module.environ["Z"] = "overwritten"
    module.environ["B"] = "three"
    decoded = []
    decode = module.environ.decodekey
    module.environ.decodekey = lambda key: (decoded.append(key), decode(key))[1]
    assert next(iterator) == "Z"
    assert decoded == [b"Z"]
    assert snapshots == [(b"Z", b"A", b"B")]
    del module.environ["A"]
    module.environ["C"] = "four"
    assert list(iterator) == ["A", "B"]
    assert decoded == [b"Z", b"A", b"B"]
    assert list(view) == [("Z", "overwritten"), ("B", "three"), ("C", "four")]
    if module.supports_bytes_environ:
        module.environb[b"RAW"] = b"\xff"
        assert module.environ["RAW"] == "\udcff"
        assert module.environb[b"Z"] == b"overwritten"
    assert module.environ.popitem() == ("Z", "overwritten")
    module.environ["Z"] = "reinserted"
    assert list(module.environ)[-1] == "Z"
    assert not hasattr(module.environ, "_data")
    assert not hasattr(module.environ.keys(), "mapping")


def test_environ_repr_preserves_live_iterator_callback_mutation_failure(monkeypatch):
    storage = {b"Z": b"one", b"A": b"two"}
    module = _os_module(
        monkeypatch,
        {
            "molt_env_mapping_items_iter": lambda: iter(storage.items()),
        },
    )
    decode = module.environ.decodekey

    def mutating(key):
        storage[b"B"] = b"three"
        return decode(key)

    module.environ.decodekey = mutating
    with pytest.raises(RuntimeError, match="dictionary changed size"):
        repr(module.environ)


@pytest.mark.parametrize(
    "proxies", [None, {}, {"http": "http://explicit.invalid:3128"}]
)
def test_urllib_proxy_constructor_captures_route_and_empty_handler_never_reads_environment(
    monkeypatch, proxies
):
    import urllib.request as oracle

    environment = {
        "http_proxy": "http://captured.invalid:3128",
        "no_proxy": "initial.invalid",
    }
    events = []

    def required(operation):
        events.append(("require", operation))

    module = _load(
        monkeypatch,
        "_molt_urllib_proxy",
        "urllib/request.py",
        (
            "molt_urllib_request_request_init",
            "molt_urllib_request_opener_init",
            "molt_urllib_request_add_handler",
            "molt_urllib_request_open",
            "molt_urllib_request_process_http_error",
            "molt_capabilities_require_operation",
        ),
        {
            "molt_capabilities_require_operation": required,
            "molt_urllib_request_open": lambda opener, request, scheme, proxy: (
                events.append(("open", scheme, proxy))
            ),
        },
    )
    monkeypatch.setattr(module, "os", SimpleNamespace(environ=environment))
    monkeypatch.setattr(oracle, "os", SimpleNamespace(environ=environment))
    expected = oracle.ProxyHandler(proxies)
    actual = module.ProxyHandler(proxies)
    assert actual.proxies == expected.proxies
    constructor_events = list(events)
    environment["http_proxy"] = "http://changed.invalid:3128"
    environment["no_proxy"] = "dynamic.invalid"
    if proxies == {}:
        assert constructor_events == []
        assert not hasattr(actual, "http_open")
    else:
        actual.parent = object()
        actual.http_open(SimpleNamespace(full_url="http://target.invalid/"))
        assert events[-1] == ("open", "http", expected.http_open.__defaults__[0])
        assert (
            module.proxy_bypass_environment("dynamic.invalid")
            == oracle.proxy_bypass_environment("dynamic.invalid")
            is True
        )
        assert (
            module.proxy_bypass_environment("initial.invalid")
            == oracle.proxy_bypass_environment("initial.invalid")
            is False
        )
    assert (
        ("require", "env.snapshot") in constructor_events
        if proxies is None
        else constructor_events == []
    )


@pytest.mark.parametrize(
    "host",
    [
        "example.com",
        "example.com:",
        "example.com:443",
        "[::1]:443",
        "x:abc",
        "x:\n80",
        "x:８０",
    ],
)
def test_urllib_private_splitport_uses_cpython_parser(monkeypatch, host):
    import urllib.parse as oracle

    tree = ast.parse((STDLIB / "urllib/parse.py").read_text(encoding="utf-8"))
    hooks = {
        node.args[0].value
        for node in ast.walk(tree)
        if isinstance(node, ast.Call)
        and isinstance(node.func, ast.Name)
        and node.func.id == "_require_intrinsic"
        and node.args
    }
    module = _load(monkeypatch, "_molt_urllib_parse", "urllib/parse.py", hooks, {})
    assert module._splitport(host) == oracle._splitport(host)


@pytest.mark.parametrize("command", ["selected", b"selected"])
@pytest.mark.parametrize("mode", [os.F_OK, os.F_OK | os.X_OK])
def test_shutil_which_selects_inputs_after_aggregate_admission(
    monkeypatch, command, mode
):
    events = []
    denial = PermissionError(
        "missing [env.read, fs.read] capabilities for 'shutil.which'"
    )
    tree = ast.parse((STDLIB / "shutil.py").read_text(encoding="utf-8"))
    hooks = {
        node.args[0].value
        for node in ast.walk(tree)
        if isinstance(node, ast.Call)
        and isinstance(node.func, ast.Name)
        and node.func.id == "_require_intrinsic"
        and node.args
    }

    def require(operation):
        events.append(("require", operation))
        raise denial

    module = _load(
        monkeypatch,
        "_molt_shutil_which",
        "shutil.py",
        hooks,
        {
            "molt_capabilities_require_operation": require,
            "molt_shutil_which": lambda *args: events.append(("kernel", args)),
        },
    )
    # This oracle describes POSIX selection on every test host; the sibling
    # below independently supplies the Windows policy and PATHEXT observations.
    module._os = SimpleNamespace(
        name="posix",
        pathsep=":",
        fspath=os.fspath,
        fsencode=os.fsencode,
        fsdecode=os.fsdecode,
        environ={},
    )
    with pytest.raises(PermissionError) as error:
        module.which(command, mode)
    assert error.value is denial
    assert events == [("require", "shutil.which")]
    events.clear()
    module._MOLT_REQUIRE_OPERATION = lambda operation: events.append(
        ("require", operation)
    )
    module.which(command, mode, path="/first:/second:/first")
    assert events[0] == ("require", "shutil.which.explicit_path")
    files, paths, selected_mode = events[1][1]
    assert files == [command]
    assert paths == (
        [b"/first", b"/second"] if isinstance(command, bytes) else ["/first", "/second"]
    )
    assert selected_mode == mode


@pytest.mark.parametrize(
    "mode,needed", [(os.F_OK, False), (os.X_OK, False), (os.X_OK, True)]
)
def test_windows_which_current_directory_query_is_selected_and_conditional(
    monkeypatch, mode, needed
):
    events = []
    tree = ast.parse((STDLIB / "shutil.py").read_text(encoding="utf-8"))
    hooks = {
        node.args[0].value
        for node in ast.walk(tree)
        if isinstance(node, ast.Call)
        and isinstance(node.func, ast.Name)
        and node.func.id == "_require_intrinsic"
        and node.args
    }

    def query(command):
        events.append(("query", command))
        return needed

    module = _load(
        monkeypatch,
        "_molt_windows_which",
        "shutil.py",
        hooks,
        {
            "molt_capabilities_require_operation": lambda operation: events.append(
                ("require", operation)
            ),
            "molt_os_need_current_directory_for_exe_path": query,
            "molt_shutil_which": lambda *args: events.append(("kernel", args)),
        },
    )
    import ntpath

    module._os = SimpleNamespace(
        _path_splitroot=lambda value, posix: ntpath.splitroot(value),
        name="nt",
        F_OK=os.F_OK,
        X_OK=os.X_OK,
        curdir=".",
        pathsep=";",
        fspath=os.fspath,
        fsencode=os.fsencode,
        fsdecode=os.fsdecode,
        environ={"PATHEXT": ".EXE;.CMD"},
    )
    module.which("selected", mode, path="first;second")
    assert events[0] == ("require", "shutil.which")
    assert [event for event in events if event[0] == "query"] == (
        [("query", "selected")] if mode & os.X_OK else []
    )
    files, directories, selected_mode = events[-1][1]
    assert directories == (
        [".", "first", "second"]
        if not mode & os.X_OK or needed
        else ["first", "second"]
    )
    assert selected_mode == mode
    assert files == (
        ["selected", "selected.EXE", "selected.CMD"]
        if not mode & os.X_OK
        else ["selected.EXE", "selected.CMD"]
    )


@pytest.mark.parametrize("flavor", ["posix", "nt"])
@pytest.mark.parametrize("raw", [False, True])
def test_os_path_expansion_matches_cpython_protocols(monkeypatch, flavor, raw):
    import ntpath
    import posixpath

    oracle = ntpath if flavor == "nt" else posixpath
    events = []

    class Mapping(dict):
        def __contains__(self, key):
            events.append(("contains", key))
            return super().__contains__(key)

        def __getitem__(self, key):
            events.append(("get", key))
            return super().__getitem__(key)

    class Text(str):
        def __contains__(self, key):
            events.append(("path.contains", key))
            return super().__contains__(key)

        def startswith(self, prefix, *args):
            events.append(("path.startswith", prefix))
            return super().startswith(prefix, *args)

        def find(self, sub, *args):
            events.append(("path.find", sub))
            return super().find(sub, *args)

        def __getitem__(self, key):
            events.append(("path.getitem", repr(key)))
            return super().__getitem__(key)

    class Home(str):
        def rstrip(self, chars=None):
            events.append(("home.rstrip", chars))
            return super().rstrip(chars)

        def __add__(self, other):
            events.append(("home.add", other))
            return super().__add__(other)

    mapping = Mapping(
        A="alpha",
        HOME=Home("/home/test/"),
        USERPROFILE=Home("C:\\Users\\test"),
        USERNAME="test",
    )
    mapping["é"] = "unicode-name"
    mapping["A-B"] = "hyphen-name"
    mapping[""] = "empty-name"
    bytes_mapping = Mapping(
        {b"A": b"\xff", "é".encode(): b"unicode-name", b"": b"empty-name"}
    )
    module = _os_module(monkeypatch, {})
    module.environ = mapping
    if raw and flavor == "posix":
        module.environb = bytes_mapping
    else:
        monkeypatch.delattr(module, "environb", raising=False)
    fake_os = SimpleNamespace(
        fspath=os.fspath,
        fsencode=os.fsencode,
        fsdecode=os.fsdecode,
        environ=mapping,
        getuid=lambda: 7,
    )
    if raw and flavor == "posix":
        fake_os.environb = bytes_mapping
    monkeypatch.setattr(oracle, "os", fake_os)
    cases = [
        "plain",
        "$A/${MISSING}/$A",
        "$$A/${A}/${A",
        "$é/$A",
        "é😀/$A/尾/${A}/$é/${é}",
        "$A-B/${A-B}/$0/${}/$A_$A",
        "${unfinished/$A/%A%",
        "${outer${A}}/$A",
        "$/$!/$$/$$$A",
        "'%A%'/%A%/%%/$$",
        "'$A'/$A/'unterminated $A/%A%",
        "%A%%A%/%%%A%/%unfinished/$A",
        "\ud800/$A/\udcff/${A}",
        "~",
        "~/child",
        "~test/child",
    ]
    for operation in ("expandvars", "expanduser"):
        actual = getattr(module, f"_{flavor}_{operation}")
        expected = getattr(oracle, operation)
        for value in cases:
            if raw and "\ud800" in value:
                continue
            value = os.fsencode(value) if raw else Text(value)

            def capture(function):
                events.clear()
                try:
                    result = function(value)
                    outcome = (type(result).__name__, result)
                except Exception as error:
                    outcome = (type(error).__name__,)
                return outcome, events.copy()

            assert capture(actual) == capture(expected), (flavor, raw, operation, value)


@pytest.mark.parametrize("flavor", ["posix", "nt"])
@pytest.mark.parametrize("raw", [False, True])
def test_os_expandvars_preserves_mapping_callbacks(monkeypatch, flavor, raw):
    import ntpath
    import posixpath

    module = _os_module(monkeypatch, {})
    oracle = ntpath if flavor == "nt" else posixpath
    failure = RuntimeError("mapping lookup failed")

    def capture(use_oracle, scenario):
        events = []

        def value(text):
            return os.fsencode(text) if raw else text

        class Mapping(dict):
            def __getitem__(self, key):
                events.append(key)
                if key == value("A"):
                    self[value("B")] = value("changed")
                    if scenario == "rebind":
                        replacement = {value("B"): value("replacement")}
                        if raw:
                            module.environb = replacement
                            fake_os.environb = replacement
                        else:
                            module.environ = replacement
                            fake_os.environ = replacement
                    if scenario == "none":
                        return None
                    if scenario == "bad":
                        return object()
                if key == value("B") and scenario == "raise":
                    raise failure
                return super().__getitem__(key)

        mapping = Mapping({value("A"): value("$B"), value("B"): value("old")})
        fake_os = SimpleNamespace(
            fspath=os.fspath,
            fsencode=os.fsencode,
            fsdecode=os.fsdecode,
            environ=mapping,
        )
        if raw:
            module.environb = mapping
            fake_os.environb = mapping
        else:
            module.environ = mapping
        monkeypatch.setattr(oracle, "os", fake_os)
        function = (
            oracle.expandvars
            if use_oracle
            else getattr(module, f"_{flavor}_expandvars")
        )
        try:
            result = function(value("'literal'/$A/$B/${MISSING}"))
            outcome = (type(result).__name__, result)
        except Exception as error:
            outcome = (type(error).__name__, str(error), error is failure)
        return outcome, events

    for scenario in ("mutate", "rebind", "none", "bad", "raise"):
        assert capture(False, scenario) == capture(True, scenario), scenario


@pytest.mark.parametrize("flavor", ["posix", "nt"])
def test_os_expandvars_bytes_without_environb_uses_live_text_mapping(
    monkeypatch, flavor
):
    import ntpath
    import posixpath

    module = _os_module(monkeypatch, {})
    oracle = ntpath if flavor == "nt" else posixpath
    monkeypatch.delattr(module, "environb", raising=False)

    def capture(use_oracle):
        events = []
        fake_os = SimpleNamespace(
            fspath=os.fspath, fsencode=os.fsencode, fsdecode=os.fsdecode
        )

        class Mapping(dict):
            def __getitem__(self, key):
                events.append(key)
                module.environ = fake_os.environ = {"B": "rebound"}
                return super().__getitem__(key)

        module.environ = fake_os.environ = Mapping(A="first", B="old")
        monkeypatch.setattr(oracle, "os", fake_os)
        function = (
            oracle.expandvars
            if use_oracle
            else getattr(module, f"_{flavor}_expandvars")
        )
        return function(b"$A/$B"), events

    assert capture(False) == capture(True) == (b"first/rebound", ["A"])


@pytest.mark.parametrize("flavor", ["posix", "nt"])
@pytest.mark.parametrize("raw", [False, True])
def test_os_expandvars_reads_subclass_storage_without_hooks(monkeypatch, flavor, raw):
    module = _os_module(monkeypatch, {})
    module.environ = {"A": "value"}
    module.environb = {b"A": b"value"}
    base = bytes if raw else str

    class Path(base):
        def __getitem__(self, key):
            raise AssertionError("path slicing hook")

        def __iter__(self):
            raise AssertionError("path iteration hook")

        def encode(self, *args):
            raise AssertionError("path encoding hook")

        def __bytes__(self):
            raise AssertionError("path bytes hook")

    path = Path(b"$A/tail" if raw else "$A/tail")
    result = getattr(module, f"_{flavor}_expandvars")(path)
    assert type(result) is base
    assert result == (b"value/tail" if raw else "value/tail")


def test_os_path_expansion_absence_errors_and_admission(monkeypatch):
    module = _os_module(monkeypatch, {})
    calls = []

    class Mapping(dict):
        def __contains__(self, key):
            calls.append(("contains", key))
            return super().__contains__(key)

        def __getitem__(self, key):
            calls.append(("get", key))
            return super().__getitem__(key)

    module.environ = Mapping(A=None)
    module.environb = Mapping({b"A": None})
    for function, value in [
        (module._posix_expandvars, "$A"),
        (module._nt_expandvars, "%A%"),
    ]:
        # re.sub treats a None replacement as empty; it never coerces through str.
        assert function(value) == ""
    module.environ = Mapping(HOME=None, USERPROFILE=None)
    for function in (module._posix_expanduser, module._nt_expanduser):
        with pytest.raises((AttributeError, TypeError)):
            function("~")
    calls.clear()
    denial = PermissionError("missing [env.read] capability")

    def refuse(operation):
        raise denial

    module._MOLT_REQUIRE_OPERATION = refuse
    for function, value in [
        (module._posix_expandvars, "$A"),
        (module._nt_expandvars, "%A%"),
        (module._posix_expandvars, b"$A"),
        (module._nt_expandvars, b"%A%"),
        (module._posix_expanduser, "~"),
        (module._nt_expanduser, "~"),
    ]:
        with pytest.raises(PermissionError) as raised:
            function(value)
        assert raised.value is denial
    assert calls == []
    for function in (
        module._posix_expandvars,
        module._nt_expandvars,
        module._posix_expanduser,
        module._nt_expanduser,
    ):
        for value in ("plain", b"plain"):
            assert function(value) is value


def test_os_expanduser_uses_pwd_only_on_the_cpython_absent_route(monkeypatch):
    module = _os_module(monkeypatch, {"molt_os_getuid": lambda: 7})
    calls = []
    module.environ = {}
    monkeypatch.setitem(
        sys.modules,
        "pwd",
        SimpleNamespace(
            getpwuid=lambda uid: (
                calls.append(("uid", uid)) or SimpleNamespace(pw_dir="/home/fallback")
            ),
            getpwnam=lambda name: (
                calls.append(("name", name)) or SimpleNamespace(pw_dir="/home/named")
            ),
        ),
    )
    assert module._posix_expanduser("~/x") == "/home/fallback/x"
    assert module._posix_expanduser(b"~someone/x") == b"/home/named/x"
    assert calls == [("uid", 7), ("name", "someone")]
    module.environ = {"HOME": "/home/explicit"}
    assert module._posix_expanduser("~") == "/home/explicit"
    assert len(calls) == 2


def test_os_windows_expanduser_selected_mapping_and_path_callbacks(monkeypatch):
    import ntpath

    events = []

    class Mapping(dict):
        def __contains__(self, key):
            events.append(("contains", key))
            return super().__contains__(key)

        def __getitem__(self, key):
            events.append(("get", key))
            return super().__getitem__(key)

        def get(self, key, default=None):
            events.append(("mapping.get", key))
            return super().get(key, default)

    class Home(str):
        def replace(self, old, new, *args):
            events.append(("replace", old, new))
            return super().replace(old, new, *args)

        def __getitem__(self, key):
            events.append(("slice", repr(key)))
            return super().__getitem__(key)

        def __add__(self, other):
            events.append(("add", other))
            return super().__add__(other)

    def splitroot(value, posix, wide, pathlib):
        # This fixture selects Windows raw roots. Legacy subclasses continue
        # through the real Python owner; exact values now use its native edge.
        assert posix is False and pathlib is False
        assert wide is module._WINDOWS_UTF16_ROOT
        if not wide:
            assert type(value) in (str, bytes)
        return ntpath.splitroot(value)

    module = _os_module(
        monkeypatch,
        {
            # Exercise the real published continuation boundary, not a raw string
            # coercion that would erase callbacks before the Python path owner.
            "molt_path_join_many": lambda first, rest, posix: (0, None, None),
            "molt_path_basename": ntpath.basename,
            "molt_path_dirname": ntpath.dirname,
            "molt_path_splitroot": splitroot,
        },
    )
    module.name, module.sep = "nt", "\\"
    fake_os = SimpleNamespace(
        fspath=os.fspath, fsencode=os.fsencode, fsdecode=os.fsdecode
    )
    monkeypatch.setattr(ntpath, "os", fake_os)
    for entries in [
        {},
        {"HOME": "ignored"},
        {"HOMEPATH": Home("\\Users\\test"), "USERNAME": "test"},
        {"HOMEDRIVE": "C:", "HOMEPATH": Home("\\Users\\test"), "USERNAME": "test"},
        {"USERPROFILE": Home("C:\\Users\\test"), "USERNAME": "test"},
        {"USERPROFILE": Home("C:\\unrelated"), "USERNAME": "test"},
        {"USERPROFILE": None},
    ]:
        module.environ = fake_os.environ = Mapping(entries)
        for value in ("~", "~other/child", b"~other/child"):

            def capture(function):
                events.clear()
                try:
                    result = function(value)
                    outcome = (type(result).__name__, result)
                except Exception as error:
                    outcome = (type(error).__name__,)
                return outcome, events.copy()

            assert capture(module._nt_expanduser) == capture(ntpath.expanduser)


@pytest.mark.parametrize("method", ["glob", "rglob"])
@pytest.mark.parametrize("denied", [False, True])
@pytest.mark.parametrize("pathlike", [False, True])
def test_pathlib_glob_original_pattern_transport_and_denial(
    monkeypatch, method, denied, pathlike
):
    events = []
    denial = PermissionError("fs.read denied")
    admitted = RuntimeError("actual five-argument intrinsic reached")

    class Pattern(str):
        def __getitem__(self, key):
            if key == -1:
                events.append("tail")
                raise AssertionError("tail callback ran before native anchor admission")
            return super().__getitem__(key)

    class PathPattern:
        def __fspath__(self):
            events.append("fspath")
            return "/anchor"

        def replace(self, old, new):
            events.append("replace")
            return "/anchor".replace(old, new)

        def __getitem__(self, key):
            raise AssertionError(
                "original PathLike tail ran before native anchor admission"
            )

    pattern = PathPattern() if pathlike else Pattern("/anchor")

    def require(capability):
        events.append(("capability", capability))
        if denied:
            raise denial

    def intrinsic(path, parsed, recursive, original, legacy_parsed):
        events.append(("intrinsic", parsed, recursive))
        if sys.version_info < (3, 13) or (method == "glob" and not pathlike):
            assert original is pattern
        else:
            assert original == "/anchor"
        raise admitted

    module = _load(
        monkeypatch,
        "_molt_glob_transport",
        "pathlib/__init__.py",
        PATHLIB_HOOKS,
        {"molt_capabilities_require": require, "molt_path_glob": intrinsic},
    )
    path = module.Path("/missing")
    module._os = _os_module(
        monkeypatch,
        {
            "molt_path_join_many": lambda first, rest, posix: (0, None, None),
        },
    )
    with pytest.raises(PermissionError if denied else RuntimeError) as caught:
        list(getattr(path, method)(pattern))
    assert caught.value is (denial if denied else admitted)
    assert "tail" not in events
    assert events[0] == ("capability", "fs.read")
    if denied:
        assert events == [("capability", "fs.read")]
    else:
        assert events[-1] == (
            "intrinsic",
            "/anchor",
            method == "rglob" and sys.version_info < (3, 13),
        )


@pytest.mark.parametrize("windows_utf16", [False, True])
def test_pathlib_root_and_parts_use_explicit_selected_coordinate(
    monkeypatch, windows_utf16
):
    calls = []
    parts_result = ("C:\\", "leaf")

    def splitroot(path, posix, wide, pathlib):
        calls.append(("root", path, posix, wide, pathlib))
        return ("C:", "\\", "leaf")

    def parts(path, posix, wide):
        calls.append(("parts", path, posix, wide))
        return parts_result

    # Selected-coordinate transport, not a simulated Windows execution claim.
    with monkeypatch.context() as selected:
        selected.setitem(sys.modules, "os", ModuleView(os, name="nt"))
        module = _load(
            selected,
            "_molt_path_coordinates",
            "pathlib/__init__.py",
            PATHLIB_HOOKS,
            {
                "molt_path_splitroot": splitroot,
                "molt_pathlib_parts": parts,
                "molt_sys_version_info": lambda: (
                    3,
                    13 if windows_utf16 else 12,
                    0,
                    "final",
                    0,
                ),
            },
        )
    path = module.PureWindowsPath("C:/leaf")
    assert path.anchor == "C:\\"
    assert path.parts == ("C:\\", "leaf")
    assert calls == [
        ("root", "C:\\leaf", False, windows_utf16, True),
        ("parts", "C:\\leaf", False, windows_utf16),
    ]


def test_os_modern_splitroot_uses_native_projected_bytes_without_subclass_callbacks(
    monkeypatch,
):
    events = []
    projected = (b"canonical:", b"\\", b"tail")

    class BytePath(bytes):
        def replace(self, *_args):
            raise AssertionError("modern splitroot used legacy subclass replacement")

    class PathLike:
        def __fspath__(self):
            events.append("fspath")
            return BytePath(b"alias:/tail")

    def splitroot(value, posix, wide, pathlib):
        assert type(value) is BytePath
        events.append(("splitroot", posix, wide, pathlib))
        return projected

    module = _os_module(
        monkeypatch,
        {
            "molt_path_splitroot": splitroot,
            "molt_os_name": lambda: "nt",
            "molt_sys_version_info": lambda: (3, 13, 0, "final", 0),
        },
    )
    assert module._path_splitroot(PathLike(), False) is projected
    assert events == ["fspath", ("splitroot", False, True, False)]


@pytest.mark.parametrize("minor", [12, 13, 14])
@pytest.mark.parametrize("denied", [False, True])
def test_pathlib_glob_bytes_argument_phase_and_denial(monkeypatch, minor, denied):
    events = []
    denial = PermissionError("fs.read denied")
    reached = RuntimeError("bytes reached the owned native error classifier")

    class Pattern:
        def __fspath__(self):
            events.append("fspath")
            return b"/"

        def __getitem__(self, key):
            raise AssertionError("bytes reached original tail")

    def require(capability):
        events.append("admission")
        if denied:
            raise denial

    def intrinsic(path, parsed, recursive, original, legacy_parsed):
        assert parsed == b"/" and not recursive
        events.append("intrinsic")
        raise reached

    module = _load(
        monkeypatch,
        "_molt_glob_bytes_phase",
        "pathlib/__init__.py",
        PATHLIB_HOOKS,
        {
            "molt_capabilities_require": require,
            "molt_path_glob": intrinsic,
            "molt_sys_version_info": lambda: (3, minor, 0, "final", 0),
        },
    )
    path = module.Path()
    module._os = SimpleNamespace(name="posix", fspath=os.fspath)
    with pytest.raises(
        PermissionError if denied else TypeError if minor == 13 else RuntimeError
    ) as caught:
        list(path.glob(Pattern()))
    if denied:
        assert caught.value is denial and events == ["admission"]
    elif minor == 13:
        assert events == ["admission", "fspath"]
    else:
        assert caught.value is reached and events == [
            "admission",
            "fspath",
            "intrinsic",
        ]


def test_pathlib_glob_selected_313_truth_precedes_native_root(monkeypatch):
    events = []

    class Pattern(str):
        def __bool__(self):
            events.append("truth")
            raise LookupError("pattern truth")

    def intrinsic(*args):
        raise AssertionError("native root preceded pattern truth")

    module = _load(
        monkeypatch,
        "_molt_glob_truth_phase",
        "pathlib/__init__.py",
        PATHLIB_HOOKS,
        {
            "molt_sys_version_info": lambda: (3, 13, 0, "final", 0),
            "molt_capabilities_require": lambda cap: events.append("admission"),
            "molt_path_glob": intrinsic,
        },
    )
    path = module.Path()
    module._os = SimpleNamespace(name="posix", fspath=os.fspath)
    with pytest.raises(LookupError, match="pattern truth"):
        list(path.glob(Pattern("/")))
    assert events == ["admission", "truth"]


@pytest.mark.parametrize("posix", [True, False])
def test_os_legacy_splitroot_preserves_subclass_results(monkeypatch, posix):
    module = _os_module(
        monkeypatch,
        {
            "molt_sys_version_info": lambda: (3, 12, 0, "final", 0),
        },
    )
    events = []

    class PathText(str):
        def replace(self, old, new):
            events.append("replace")
            return self

        def __getitem__(self, key):
            events.append((key.start, key.stop))
            if key == slice(None, 1):
                raise LookupError("observed slice")
            return super().__getitem__(key)

    with pytest.raises(LookupError, match="observed slice"):
        module._path_splitroot(PathText("/leaf"), posix)
    assert events == ([(None, 1)] if posix else ["replace", (None, 1)])


def test_pathlib_legacy_glob_transports_actual_normalized_parts(monkeypatch):
    events = []

    class Part:
        def __str__(self):
            events.append("part-str")
            return "selected.txt"

    class Pattern(str):
        def split(self, separator):
            events.append(("split", separator))
            return [Part()]

        def __getitem__(self, key):
            if key == -1:
                raise AssertionError("native tail must follow parsed anchor admission")
            return super().__getitem__(key)

    pattern = Pattern("ignored.txt")

    def intrinsic(path, raw, recursive, original, parsed):
        assert raw is pattern and original is pattern and not recursive
        assert parsed == ("", "", ["selected.txt"])
        events.append("native")
        return []

    module = _load(
        monkeypatch,
        "_molt_glob_legacy_parts",
        "pathlib/__init__.py",
        PATHLIB_HOOKS,
        {
            "molt_sys_version_info": lambda: (3, 12, 0, "final", 0),
            "molt_capabilities_require": lambda cap: events.append("admission"),
            "molt_path_glob": intrinsic,
        },
    )
    path = module.Path()
    module._os = _os_module(
        monkeypatch,
        {
            "molt_sys_version_info": lambda: (3, 12, 0, "final", 0),
        },
    )
    module._os.name = "posix"
    assert list(path.glob(pattern)) == []
    assert events == ["admission", ("split", "/"), "part-str", "native"]


@pytest.mark.parametrize("raw", ["/anchor", b"/anchor"])
def test_pathlib_313_windows_replace_result_uses_fspath(monkeypatch, raw):
    events = []
    reached = RuntimeError("selected native pattern classifier")

    class Replaced:
        def __fspath__(self):
            events.append("fspath")
            return raw

    class Pattern(str):
        def replace(self, old, new):
            assert (old, new) == ("/", "\\")
            events.append("replace")
            return Replaced()

    pattern = Pattern("/anchor")

    def intrinsic(path, value, recursive, original, legacy_parsed):
        assert value is raw and original is pattern and not recursive
        assert legacy_parsed is None
        events.append("native")
        raise reached

    module = _load(
        monkeypatch,
        "_molt_glob_313_replace",
        "pathlib/__init__.py",
        PATHLIB_HOOKS,
        {
            "molt_sys_version_info": lambda: (3, 13, 0, "final", 0),
            "molt_capabilities_require": lambda cap: events.append("admission"),
            "molt_path_glob": intrinsic,
        },
    )
    path = module.Path()
    module._os = SimpleNamespace(name="nt", fspath=os.fspath)
    with pytest.raises(RuntimeError) as error:
        list(path.glob(pattern))
    assert error.value is reached
    assert events == ["admission", "replace", "fspath", "native"]
