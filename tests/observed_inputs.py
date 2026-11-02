"""Postsubmit input observation; loaded explicitly by the landing controller."""

from enum import Enum, auto
from pathlib import Path

import pytest

from tools.test_dependency_graph import OBSERVED_CONTEXT_OPTION, TestInputRecorder

_RECORDER = pytest.StashKey[TestInputRecorder]()
_FIXTURES = pytest.StashKey[dict[pytest.FixtureDef, dict]]()
_SUBJECTS = pytest.StashKey[frozenset[str]]()


class _Protocol(Enum):
    SETUP = auto()
    CALL = auto()
    TEARDOWN = auto()
    COMPLETE = auto()
    INCOMPLETE = auto()


_PROTOCOL = pytest.StashKey[_Protocol]()


def pytest_addoption(parser: pytest.Parser) -> None:
    parser.addoption(
        OBSERVED_CONTEXT_OPTION,
        default=None,
        help="Observed-input context issued by the landing controller",
    )
    parser.addoption(
        "--molt-observed-target",
        action="append",
        default=[],
        help="Qualified whole-file observation subject",
    )


def pytest_configure(config: pytest.Config) -> None:
    context = config.getoption(OBSERVED_CONTEXT_OPTION)
    if context is not None and not config.option.collectonly:
        config.stash[_RECORDER] = TestInputRecorder(config.rootpath, context)
        config.stash[_FIXTURES] = {}
        config.stash[_SUBJECTS] = frozenset(config.getoption("--molt-observed-target"))


def _subject(config: pytest.Config, nodeid: str) -> bool:
    subjects = config.stash.get(_SUBJECTS, frozenset())
    return not subjects or nodeid.partition("::")[0] in subjects


@pytest.hookimpl(hookwrapper=True)
def pytest_make_collect_report(collector):
    recorder = collector.config.stash.get(_RECORDER, None)
    subject = (
        isinstance(collector, pytest.Module)
        and recorder is not None
        and _subject(collector.config, collector.nodeid)
    )
    if subject:
        recorder.start("collection:" + collector.nodeid)
    completed = False
    try:
        outcome = yield
        if subject:
            completed = outcome.get_result().passed
    finally:
        if subject:
            recorder.complete = recorder.complete and completed
            recorder.finish()


@pytest.hookimpl(hookwrapper=True)
def pytest_fixture_setup(fixturedef, request):
    recorder = request.config.stash.get(_RECORDER, None)
    if recorder is None or recorder.node is None:
        yield
        return
    with recorder.fixture_reads() as captured:
        yield
    # FixtureDef owns the current cached instance. A new parameter/setup
    # replaces it; an unrelated invocation with the same name cannot donate
    # reads or opacity. Values are never retained by this recorder.
    request.config.stash[_FIXTURES][fixturedef] = captured


@pytest.hookimpl(hookwrapper=True, tryfirst=True)
def pytest_runtest_teardown(item: pytest.Item):
    yield
    recorder = item.config.stash.get(_RECORDER, None)
    if (
        recorder is not None
        and recorder.node is not None
        and isinstance(item, pytest.Function)
    ):
        fixtures = item.config.stash[_FIXTURES]
        # Pytest clears the request after teardown hooks return. Its active
        # definitions include cached fixtures requested dynamically in the
        # body/finalizers, which fixturenames alone cannot describe.
        for definition in item._request._fixture_defs.values():
            if isinstance(definition, pytest.FixtureDef) and definition in fixtures:
                recorder.add_reads(fixtures[definition])


@pytest.hookimpl(hookwrapper=True, tryfirst=True)
def pytest_collection_modifyitems(config: pytest.Config, items: list[pytest.Item]):
    recorder = config.stash.get(_RECORDER, None)
    if recorder is not None:
        # Capture every parameter before markers and --deselect remove cases.
        # A node-only collection cannot claim a whole-file inventory.
        whole_files = set()
        for argument in config.args:
            path, separator, _ = str(argument).partition("::")
            if separator:
                continue
            absolute = Path(path).absolute()
            for item in items:
                if (
                    item.path == absolute or absolute in item.path.parents
                ) and _subject(config, item.nodeid):
                    whole_files.add(item.nodeid.partition("::")[0])
        recorder.inventory((item.nodeid for item in items), whole_files)
    yield


@pytest.hookimpl(hookwrapper=True)
def pytest_runtest_makereport(item: pytest.Item, call):
    outcome = yield
    report = outcome.get_result()
    recorder = item.config.stash.get(_RECORDER, None)
    if recorder is not None and recorder.node is not None:
        state = item.stash.get(_PROTOCOL, _Protocol.INCOMPLETE)
        match state, report.when, report.passed:
            case _Protocol.SETUP, "setup", True:
                state = _Protocol.CALL
            case _Protocol.CALL, "call", True:
                state = _Protocol.TEARDOWN
            case _Protocol.TEARDOWN, "teardown", True:
                state = _Protocol.COMPLETE
            case _:
                state = _Protocol.INCOMPLETE
        item.stash[_PROTOCOL] = state


@pytest.hookimpl(hookwrapper=True, trylast=True)
def pytest_runtest_protocol(item: pytest.Item, nextitem):
    # Inside conftest's checkout-leak observer: its before/after root listings
    # are harness inventory, not data read by every test.
    recorder = item.config.stash.get(_RECORDER, None)
    if recorder is not None and _subject(item.config, item.nodeid):
        item.stash[_PROTOCOL] = _Protocol.SETUP
        recorder.start(item.nodeid)
    completed = False
    try:
        outcome = yield
        completed = outcome.excinfo is None
    finally:
        if recorder is not None and recorder.node is not None:
            recorder.complete = (
                recorder.complete
                and completed
                and item.stash.get(_PROTOCOL, _Protocol.INCOMPLETE)
                is _Protocol.COMPLETE
            )
            recorder.finish()
