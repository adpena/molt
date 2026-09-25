from __future__ import annotations

import io
import json
import sys

import pytest

from molt.cli import output, progress
from molt.cli.entrypoint_parser import _build_entrypoint_parser


class Terminal(io.StringIO):
    def isatty(self):
        return True


@pytest.fixture
def live_calls(monkeypatch):
    import rich.live

    calls = []

    class Live:
        def __init__(self, renderable, **kwargs):
            calls.append(("create", renderable, kwargs))

        def start(self):
            calls.append(("start",))

        def stop(self):
            calls.append(("stop",))

    monkeypatch.setattr(rich.live, "Live", Live)
    monkeypatch.delenv("CI", raising=False)
    monkeypatch.setenv("TERM", "xterm")
    return calls


def test_auto_terminal_uses_one_transient_stderr_renderer(live_calls):
    stream = Terminal()
    with progress.BuildProgress(stream=stream):
        progress.phase("module_graph")
        progress.frontend_module("pkg.[literal]", 1.25)
    created = live_calls[0]
    assert created[0] == "create"
    assert created[2]["console"].file is stream
    assert created[2]["transient"] is True
    assert created[2]["redirect_stdout"] is False
    assert created[2]["redirect_stderr"] is False
    assert created[1].text.plain == "Frontend: finished pkg.[literal] in 1.25s"
    assert [item[0] for item in live_calls] == ["create", "start", "stop"]


@pytest.mark.parametrize("kwargs", [{}, {"mode": "plain"}, {"headless": True}])
def test_nonterminal_progress_is_bounded_plain_lines(kwargs, capsys):
    stream = io.StringIO()
    with progress.BuildProgress(stream=stream, **kwargs):
        progress.phase("module_graph")
        progress.phase("module_graph")
        for index in range(100):
            progress.frontend_module(f"module_{index}", 0.01)
        progress.phase("frontend")
    assert stream.getvalue().splitlines() == [
        "molt: Preparing build",
        "molt: Resolving imports",
        "molt: Compiling Python",
    ]
    assert "\x1b" not in stream.getvalue()
    assert capsys.readouterr().out == ""


@pytest.mark.parametrize("kwargs", [{"headless": True}, {"mode": "plain"}])
def test_plain_and_headless_never_construct_terminal_renderer(kwargs, live_calls):
    stream = Terminal()
    with progress.BuildProgress(stream=stream, **kwargs):
        progress.phase("frontend")
    assert live_calls == []
    assert "\x1b" not in stream.getvalue()


@pytest.mark.parametrize(
    "kwargs", [{"quiet": True}, {"json_output": True}, {"mode": "off"}]
)
def test_disabled_progress_never_writes_status(kwargs, live_calls):
    stream = Terminal()
    with progress.BuildProgress(stream=stream, **kwargs):
        progress.phase("frontend")
        progress.frontend_module("main", 2.0)
        with progress.subprocess_status("Backend compilation") as label:
            assert label is None
    assert stream.getvalue() == ""
    assert live_calls == []


def test_quiet_retains_errors_and_does_not_leak_to_next_build(capsys):
    with progress.BuildProgress(quiet=True):
        output.success("hidden success", file=sys.stderr)
        assert output.fail("actual failure", False, code=7, command="build") == 7
    captured = capsys.readouterr()
    assert captured.out == ""
    assert captured.err == "actual failure\n"
    output.success("visible again")
    assert capsys.readouterr().out == "visible again\n"


def test_json_is_the_only_stdout_and_has_no_status(capsys, live_calls):
    with progress.BuildProgress(json_output=True, stream=Terminal()):
        progress.phase("frontend")
        output.success("not JSON")
        assert output.fail("bad input", True, code=2, command="build") == 2
    captured = capsys.readouterr()
    payload = json.loads(captured.out)
    assert payload["status"] == "error"
    assert payload["errors"] == ["bad input"]
    assert captured.err == ""
    assert live_calls == []


def test_renderer_stops_before_success_and_on_exception(live_calls, capsys):
    with progress.BuildProgress(stream=Terminal()):
        output.success("built", file=sys.stderr)
        assert live_calls[-1] == ("stop",)
    assert [call[0] for call in live_calls].count("stop") == 1
    assert capsys.readouterr().err == "built\n"
    with pytest.raises(RuntimeError, match="compile failed"):
        with progress.BuildProgress(stream=Terminal()):
            raise RuntimeError("compile failed")
    assert live_calls[-1] == ("stop",)
    assert progress.success_is_visible()


def test_existing_subprocess_keepalive_is_replaced_only_inside_build():
    with progress.subprocess_status("Backend compilation") as label:
        assert label == "Backend compilation"
    stream = io.StringIO()
    with progress.BuildProgress(stream=stream):
        progress.phase("backend_pipeline")
        with progress.subprocess_status("Backend compilation") as label:
            assert label is None
    assert stream.getvalue().endswith(
        "molt: Backend compilation\nmolt: Generating and linking output\n"
    )
    with progress.subprocess_status("Backend compilation") as label:
        assert label == "Backend compilation"


def test_wrapper_build_stops_status_before_guest_execution(live_calls, capsys):
    @progress.finish_after
    def wrapper_build():
        progress.notice("Compiling app.py...")
        return "artifact", 1.0, None

    with progress.BuildProgress(stream=Terminal()):
        assert wrapper_build() == ("artifact", 1.0, None)
        assert live_calls[-1] == ("stop",)
        print("guest stdout")
    assert capsys.readouterr().out == "guest stdout\n"


def test_quiet_wrapper_notice_does_not_hide_guest_output(capsys):
    with progress.BuildProgress(quiet=True):
        progress.notice("Compiling app.py...")
        progress.finish()
        print("guest stdout")
    captured = capsys.readouterr()
    assert captured.out == "guest stdout\n"
    assert captured.err == ""


def test_build_wrapper_preserves_arguments_result_and_exception(capsys):
    seen = []

    def build(*args, **kwargs):
        seen.append((args, kwargs))
        progress.phase("frontend")
        output.success("quiet success")
        return 9

    wrapped = progress.present_build(build, quiet=True)
    assert wrapped("entry.py", target="wasm") == 9
    assert seen == [(("entry.py",), {"target": "wasm"})]
    assert capsys.readouterr() == ("", "")
    assert progress.success_is_visible()


@pytest.mark.parametrize("command", ["build", "run"])
def test_build_and_run_expose_explicit_presentation_modes(command):
    args = _build_entrypoint_parser().parse_args(
        [command, "--headless", "--quiet", "--progress", "off", "app.py"]
    )
    assert args.headless and args.quiet and args.progress == "off"


@pytest.mark.parametrize("name", ["CI", "TERM"])
def test_ci_and_dumb_terminals_do_not_animate(name, monkeypatch, live_calls):
    monkeypatch.setenv(name, "1" if name == "CI" else "dumb")
    stream = Terminal()
    with progress.BuildProgress(stream=stream):
        pass
    assert live_calls == []
    assert "\x1b" not in stream.getvalue()
