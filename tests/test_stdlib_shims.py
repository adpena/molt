from __future__ import annotations

import importlib

import pytest

from molt.stdlib import copy as molt_copy
from molt.stdlib import fnmatch as molt_fnmatch
from molt.stdlib import inspect as molt_inspect
from molt.stdlib import os as molt_os
from molt.stdlib import sys as molt_sys
from molt.stdlib import traceback as molt_traceback
from molt.stdlib import warnings as molt_warnings


def test_warnings_catch_and_filter(capsys) -> None:
    molt_warnings.resetwarnings()
    with molt_warnings.catch_warnings(record=True) as rec:
        molt_warnings.simplefilter("always")
        molt_warnings.warn("hello")
        assert len(rec) == 1

    molt_warnings.resetwarnings()
    molt_warnings.simplefilter("ignore")
    molt_warnings.warn("ignored")
    captured = capsys.readouterr()
    assert captured.out == ""

    molt_warnings.resetwarnings()
    molt_warnings.simplefilter("error")
    try:
        molt_warnings.warn("boom")
    except Exception as exc:  # noqa: BLE001
        assert exc.__class__.__name__ == "UserWarning"


def test_traceback_format_exc() -> None:
    try:
        raise ValueError("boom")
    except ValueError:
        text = molt_traceback.format_exc()
    assert "ValueError: boom" in text


def test_traceback_format_tb_symbol() -> None:
    lines = molt_traceback.format_tb((("demo.py", 7, "main"),))
    assert 'File "demo.py", line 7, in main' in lines[0]


def test_inspect_signature() -> None:
    def foo(a, b=1):
        return a + b

    sig = molt_inspect.signature(foo)
    assert str(sig) == "(a, b=1)"

    class Dummy:
        __molt_arg_names__ = ("x", "y")
        __defaults__ = (2,)

    dummy_sig = molt_inspect.signature(Dummy)
    assert str(dummy_sig) == "(x, y=2)"


def test_fnmatch_basic() -> None:
    assert molt_fnmatch.fnmatchcase("foo.txt", "*.txt")
    assert molt_fnmatch.fnmatchcase("a1", "a[0-9]")
    assert not molt_fnmatch.fnmatchcase("aZ", "a[0-9]")


def test_copy_hooks() -> None:
    class Widget:
        def __copy__(self):
            return "copied"

        def __deepcopy__(self, memo):
            return "deepcopied"

    widget = Widget()
    assert molt_copy.copy(widget) == "copied"
    assert molt_copy.deepcopy(widget) == "deepcopied"


def test_os_env_gating(monkeypatch) -> None:
    monkeypatch.delenv("MOLT_CAPABILITIES", raising=False)
    importlib.reload(molt_os)
    with pytest.raises(PermissionError):
        molt_os.getenv("MOLT_CAPABILITIES")

    monkeypatch.setenv("MOLT_CAPABILITIES", "env.read")
    importlib.reload(molt_os)
    assert molt_os.getenv("MOLT_CAPABILITIES") == "env.read"


def test_sys_defaults_without_env(monkeypatch) -> None:
    monkeypatch.delenv("MOLT_CAPABILITIES", raising=False)
    importlib.reload(molt_sys)
    assert molt_sys.argv == []
    assert molt_sys.platform == "molt"
