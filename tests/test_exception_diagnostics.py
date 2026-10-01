from __future__ import annotations

import importlib
from pathlib import Path
import sys

import pytest


@pytest.fixture(params=["molt.exception_diagnostics", "tools.exception_diagnostics"])
def diagnostics(request):
    return importlib.import_module(request.param)


def _storage(error):
    return BaseException.__dict__["__dict__"].__get__(error, BaseException)


def test_normal_notes_keep_order_and_existing_storage(diagnostics):
    error = RuntimeError("primary")
    BaseException.add_note(error, "existing")
    notes = _storage(error)["__notes__"]
    assert diagnostics.add_exception_note(error, "secondary")
    assert _storage(error)["__notes__"] is notes
    assert notes == ["existing", "secondary"]


def test_hostile_descriptors_and_formatters_are_never_called(diagnostics):
    called = []

    class Meta(type):
        @property
        def __name__(cls):
            called.append("name")
            raise RuntimeError("name callback")

    class Hostile(BaseException, metaclass=Meta):
        @property
        def __notes__(self):
            called.append("notes")
            raise RuntimeError("notes callback")

        def __getattribute__(self, name):
            called.append("attribute:" + name)
            raise RuntimeError("attribute callback")

        def __str__(self):
            called.append("str")
            raise RuntimeError("str callback")

    error = Hostile("primary", object())
    _storage(error)["__notes__"] = []
    assert diagnostics.exception_type_name(error) == "Hostile"
    assert diagnostics.exception_diagnostic(error) == "Hostile: primary"
    assert diagnostics.add_exception_note(error, "secondary")
    assert _storage(error)["__notes__"] == ["secondary"]
    assert called == []
    with pytest.raises(Hostile) as caught:
        raise error
    assert caught.value is error


def test_builtin_add_note_counterexample(diagnostics):
    called = []

    class Hostile(BaseException):
        @property
        def __notes__(self):
            called.append("notes")
            raise RuntimeError("secondary masks primary")

    error = Hostile()
    with pytest.raises(RuntimeError, match="secondary masks primary"):
        BaseException.add_note(error, "diagnostic")
    assert called == ["notes"]
    called.clear()
    assert not diagnostics.add_exception_note(error, "diagnostic")
    assert list(dict.items(_storage(error))) == []
    assert called == []


def test_collision_key_is_rejected_before_dictionary_lookup(diagnostics):
    called = []

    class Key:
        def __hash__(self):
            return hash("__notes__")

        def __eq__(self, other):
            called.append("equality")
            raise RuntimeError("collision callback")

    error = RuntimeError()
    _storage(error)[Key()] = "sentinel"
    assert not diagnostics.add_exception_note(error, "diagnostic")
    assert called == []


def test_list_subclass_and_string_subclass_are_not_invoked(diagnostics):
    called = []

    class Notes(list):
        def append(self, value):
            called.append("append")
            raise RuntimeError("append callback")

    class Note(str):
        def __str__(self):
            called.append("str")
            raise RuntimeError("string callback")

    error = RuntimeError()
    _storage(error)["__notes__"] = Notes()
    assert not diagnostics.add_exception_note(error, "diagnostic")
    assert not diagnostics.add_exception_note(RuntimeError(), Note("diagnostic"))
    assert called == []


@pytest.mark.parametrize("stored", [None, (), "bad"])
def test_invalid_note_storage_is_reported_without_masking(diagnostics, stored):
    error = RuntimeError()
    _storage(error)["__notes__"] = stored
    assert not diagnostics.add_exception_note(error, "diagnostic")
    assert _storage(error)["__notes__"] is stored


def test_missing_notes_decline_without_changing_user_storage(diagnostics):
    error = RuntimeError("primary")
    storage = _storage(error)
    storage["user"] = object()
    before = list(dict.items(storage))
    assert not diagnostics.add_exception_note(error, "secondary")
    assert list(dict.items(storage)) == before


@pytest.mark.parametrize("existing", [False, True])
def test_trace_injected_collision_never_dispatches_equality(diagnostics, existing):
    called = []

    class Key:
        def __hash__(self):
            return hash("__notes__")

        def __eq__(self, other):
            called.append("equality")
            return False

    error = RuntimeError("primary")
    storage = _storage(error)
    notes = []
    if existing:
        storage["__notes__"] = notes
    injected = False

    def inject(frame, event, arg):
        nonlocal injected
        if frame.f_code is diagnostics.add_exception_note.__code__ and event == "line":
            if not injected and "storage" in frame.f_locals:
                injected = True
                storage[Key()] = "sentinel"
                # Inserting a colliding key can itself compare with an existing
                # notes key. Count only dispatch after the trace mutation.
                called.clear()
        return inject

    prior = sys.gettrace()
    try:
        sys.settrace(inject)
        result = diagnostics.add_exception_note(error, "secondary")
    finally:
        sys.settrace(prior)
    assert injected
    assert called == []
    assert result is existing
    assert notes == (["secondary"] if existing else [])


def test_diagnostic_allocation_failure_returns_fixed_fallback(diagnostics, monkeypatch):
    def fail_name(error):
        raise MemoryError("injected allocation failure")

    monkeypatch.setattr(diagnostics, "exception_type_name", fail_name)
    assert (
        diagnostics.exception_diagnostic(RuntimeError())
        == "exception diagnostic unavailable"
    )


def test_assigned_class_name_subclass_is_normalized_without_callbacks(diagnostics):
    called = []

    class Name(str):
        def __add__(self, other):
            called.append("add")
            raise RuntimeError("name addition")

        def __str__(self):
            called.append("str")
            raise RuntimeError("name stringification")

        def __format__(self, spec):
            called.append("format")
            raise RuntimeError("name formatting")

    class Error(Exception):
        pass

    Error.__name__ = Name("Error")
    error = Error("detail")
    assert type(diagnostics.exception_type_name(error)) is str
    assert diagnostics.exception_diagnostic(error) == "Error: detail"
    assert called == []


def test_builtin_args_setter_normalizes_subclass_before_diagnostics(diagnostics):
    called = []

    class Args(tuple):
        def __iter__(self):
            called.append("iterate")
            return tuple.__iter__(self)

    error = RuntimeError()
    BaseException.__dict__["args"].__set__(error, Args(("detail",)))
    actual = BaseException.__dict__["args"].__get__(error, BaseException)
    assert type(actual) is tuple and called == ["iterate"]
    called.clear()
    result = diagnostics.exception_diagnostic(error)
    assert result == (
        "RuntimeError: detail" if type(actual) is tuple else "RuntimeError"
    )
    assert called == []


def test_name_normalization_allocation_failure_is_nonthrowing(diagnostics, monkeypatch):
    class FailingString:
        @staticmethod
        def __str__(name):
            raise MemoryError("injected normalization allocation failure")

    monkeypatch.setattr(diagnostics, "str", FailingString, raising=False)
    assert diagnostics.exception_type_name(RuntimeError()) == "unknown exception type"


def test_detached_observed_list_reports_append_not_publication(diagnostics):
    error = RuntimeError("primary")
    storage = _storage(error)
    notes = []
    storage["__notes__"] = notes
    detached = False

    def detach(frame, event, arg):
        nonlocal detached
        if frame.f_code is diagnostics.add_exception_note.__code__ and event == "line":
            if not detached and "notes" in frame.f_locals:
                detached = True
                storage.clear()
        return detach

    prior = sys.gettrace()
    try:
        sys.settrace(detach)
        result = diagnostics.add_exception_note(error, "secondary")
    finally:
        sys.settrace(prior)
    assert detached and result
    assert notes == ["secondary"]
    assert list(dict.items(storage)) == []


def test_iteration_resize_declines_without_masking_primary(diagnostics):
    error = RuntimeError("primary")
    storage = _storage(error)
    storage["user"] = "value"
    resized = False

    def resize(frame, event, arg):
        nonlocal resized
        if frame.f_code is diagnostics.add_exception_note.__code__ and event == "line":
            if not resized and "key" in frame.f_locals:
                resized = True
                storage["new"] = "value"
        return resize

    prior = sys.gettrace()
    try:
        sys.settrace(resize)
        result = diagnostics.add_exception_note(error, "secondary")
    finally:
        sys.settrace(prior)
    assert resized and not result
    assert list(dict.items(storage)) == [("user", "value"), ("new", "value")]


def test_packaged_and_standalone_diagnostic_authorities_do_not_drift():
    packaged = importlib.import_module("molt.exception_diagnostics")
    standalone = importlib.import_module("tools.exception_diagnostics")
    assert (
        Path(packaged.__file__).read_bytes() == Path(standalone.__file__).read_bytes()
    )


def test_exception_diagnostics_are_mandatory_harness_and_portability_proofs():
    import tomllib
    from pathlib import Path

    plan = tomllib.loads(
        (Path(__file__).resolve().parents[1] / "tools/proof_plan.toml").read_text()
    )
    commands = {command["id"]: command for command in plan["command"]}
    for identifier in (
        "python.unit.harness",
        "portability.cargo-custody.linux",
        "portability.cargo-custody.macos",
        "portability.cargo-custody.windows",
    ):
        command = commands[identifier]
        assert "tests/test_exception_diagnostics.py" in command["argv"]
        assert {"pr", "main"} <= set(command["tiers"])
    assert {
        "src/molt/exception_diagnostics.py",
        "tools/exception_diagnostics.py",
        "tests/test_exception_diagnostics.py",
    } <= set(plan["authority_inputs"])
