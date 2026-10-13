"""Actual Molt filesystem facades must preserve aggregate runtime refusals."""

from __future__ import annotations

import importlib.util
import io
import threading
from pathlib import Path
import sys
from types import SimpleNamespace

import pytest
from tests.stdlib_intrinsic_registry import intrinsic_registry

STDLIB = Path(__file__).resolve().parents[1] / "src/molt/stdlib"


def _unused(*args):
    pytest.fail(f"unexpected intrinsic call: {args!r}")


PATHLIB_HOOKS = (
    "molt_capabilities_require",
    "molt_file_open_ex",
    "molt_os_lstat",
    "molt_os_rename",
    "molt_os_replace",
    "molt_os_stat",
    "molt_path_abspath",
    "molt_path_as_uri",
    "molt_path_chmod",
    "molt_path_compare",
    "molt_path_dirname",
    "molt_path_exists",
    "molt_path_glob",
    "molt_path_is_relative_to",
    "molt_path_isabs",
    "molt_path_isdir",
    "molt_path_isfile",
    "molt_path_islink",
    "molt_path_makedirs",
    "molt_path_match",
    "molt_path_mkdir",
    "molt_path_name",
    "molt_path_parents",
    "molt_path_readlink",
    "molt_path_relative_to",
    "molt_path_relative_to_many",
    "molt_path_rmdir",
    "molt_path_stem",
    "molt_path_suffix",
    "molt_path_suffixes",
    "molt_path_symlink",
    "molt_path_unlink",
    "molt_path_with_name",
    "molt_path_with_stem",
    "molt_path_with_suffix",
    "molt_pathlib_as_posix",
    "molt_pathlib_cwd",
    "molt_pathlib_eq",
    "molt_pathlib_expanduser",
    "molt_pathlib_group",
    "molt_pathlib_hardlink_to",
    "molt_pathlib_hash",
    "molt_pathlib_home",
    "molt_pathlib_is_mount",
    "molt_pathlib_iterdir",
    "molt_pathlib_lt",
    "molt_pathlib_owner",
    "molt_pathlib_parts",
    "molt_pathlib_read_bytes",
    "molt_pathlib_read_text",
    "molt_pathlib_resolve",
    "molt_pathlib_rglob",
    "molt_pathlib_samefile",
    "molt_pathlib_splitroot",
    "molt_pathlib_str",
    "molt_pathlib_touch",
    "molt_pathlib_write_bytes",
    "molt_pathlib_write_text",
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
    "molt_logging_formatter_drop",
    "molt_logging_formatter_format",
    "molt_logging_formatter_format_time",
    "molt_logging_formatter_new",
    "molt_logging_get_level_name",
    "molt_logging_handler_close",
    "molt_logging_handler_drop",
    "molt_logging_handler_emit",
    "molt_logging_handler_flush",
    "molt_logging_handler_new",
    "molt_logging_handler_set_formatter",
    "molt_logging_handler_set_level",
    "molt_logging_level_to_int",
    "molt_logging_logger_add_handler",
    "molt_logging_logger_drop",
    "molt_logging_logger_get_effective_level",
    "molt_logging_logger_is_enabled_for",
    "molt_logging_logger_new",
    "molt_logging_logger_remove_handler",
    "molt_logging_logger_set_level",
    "molt_logging_percent_style_format",
    "molt_logging_record_drop",
    "molt_logging_record_get_message",
    "molt_logging_record_new",
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
    with intrinsic_registry({**dict.fromkeys(hooks, _unused), **behavior}):
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
    # Construct the actual facade without exercising the unrelated path parser.
    path = module.Path()
    path._path = "/tmp/selected"
    with pytest.raises(PermissionError) as caught:
        path.mkdir(parents=parents, mode=0o700, exist_ok=True)
    assert caught.value is denial
    expected = ("/tmp/selected", 0o700, True) if parents else ("/tmp/selected", 0o700)
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
    # Import-time root/lastResort construction and teardown use these concrete
    # hooks. All other hooks fail if unexpectedly exercised.
    behavior = {
        "molt_logging_logger_new": lambda *args: 1,
        "molt_logging_logger_set_level": lambda *args: None,
        "molt_logging_level_to_int": lambda value: value,
        "molt_rlock_new": threading.RLock,
        "molt_rlock_acquire": lambda lock, blocking, timeout: lock.acquire(blocking, timeout),
        "molt_rlock_release": lambda lock: lock.release(),
        "molt_logging_handler_close": lambda *args: None,
        "molt_logging_handler_new": lambda *args: 3,
        "molt_logging_formatter_new": lambda *args: 5,
        "molt_logging_handler_set_level": lambda *args: None,
        "molt_logging_handler_drop": lambda *args: None,
        "molt_logging_logger_drop": lambda *args: None,
        "molt_logging_formatter_drop": lambda *args: None,
        "molt_rlock_drop": lambda *args: None,
    }
    monkeypatch.setitem(
        sys.modules, "logging.handlers", sys.modules.get("logging.handlers")
    )
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

    monkeypatch.setattr(module, "_io", SimpleNamespace(open=reject))
    with pytest.raises(PermissionError) as caught:
        handler._open()
    assert caught.value is denial
    assert calls == [(("/tmp/selected.log", mode), {"encoding": None})]
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
def test_filehandler_persistent_stream_and_closed_write_mode(monkeypatch, tmp_path, delay):
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


def test_delayed_filehandler_emit_preserves_open_denial_without_mutation(monkeypatch, tmp_path):
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
    monkeypatch.setattr(module, "_io", SimpleNamespace(open=reject))
    with pytest.raises(PermissionError) as caught:
        handler.emit(SimpleNamespace())
    assert caught.value is denial
    assert calls == [((str(path), "w"), {"encoding": None})]
    assert handler.stream is None
    assert path.read_bytes() == b"keep"
    handler.close()


def test_streamhandler_uses_selected_stream_formatter_terminator_and_filter(monkeypatch):
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
