"""Build presentation only; compiler phase and timing producers own the facts."""

from __future__ import annotations

from contextvars import ContextVar, Token
from contextlib import contextmanager
from collections.abc import Iterator
from functools import wraps
import os
import sys
from typing import Any, Callable, ParamSpec, TextIO, TypeVar


_ACTIVE: ContextVar[BuildProgress | None] = ContextVar(
    "molt_build_progress", default=None
)
_PHASE_LABELS = {
    "prepare": "Preparing build",
    "module_graph": "Resolving imports",
    "module_analysis": "Analyzing modules",
    "ir_lowering": "Preparing lowering",
    "frontend": "Compiling Python",
    "backend_pipeline": "Generating and linking output",
}
_P = ParamSpec("_P")
_R = TypeVar("_R")


class BuildProgress:
    """One scoped renderer: TTY spinner, line-oriented progress, or silence.

    There are no estimated percentages, background probes, or synthetic timers.
    Module durations come from the existing frontend timing callback.
    """

    def __init__(
        self,
        *,
        mode: str = "auto",
        headless: bool = False,
        quiet: bool = False,
        json_output: bool = False,
        stream: TextIO | None = None,
    ) -> None:
        if mode not in {"auto", "plain", "off"}:
            raise ValueError(f"Unknown progress mode: {mode}")
        self.stream = sys.stderr if stream is None else stream
        self.quiet = quiet
        self.json_output = json_output
        tty = bool(getattr(self.stream, "isatty", lambda: False)())
        terminal = tty and os.environ.get("TERM") != "dumb" and not os.environ.get("CI")
        self.mode = (
            "off"
            if quiet or json_output or mode == "off"
            else "plain"
            if headless or mode == "plain" or not terminal
            else "tty"
        )
        self._live: Any = None
        self._spinner: Any = None
        self._token: Token[BuildProgress | None] | None = None
        self._last_phase: str | None = None
        self._finished = False

    def __enter__(self) -> BuildProgress:
        self._token = _ACTIVE.set(self)
        try:
            if self.mode == "tty":
                from rich.console import Console
                from rich.live import Live
                from rich.spinner import Spinner
                from rich.text import Text

                self._spinner = Spinner("dots", text=Text(_PHASE_LABELS["prepare"]))
                self._live = Live(
                    self._spinner,
                    console=Console(file=self.stream, highlight=False),
                    transient=True,
                    refresh_per_second=6,
                    redirect_stdout=False,
                    redirect_stderr=False,
                )
                self._live.start()
            self.phase("prepare")
            return self
        except BaseException:
            self.__exit__(*sys.exc_info())
            raise

    def __exit__(self, *exc: object) -> None:
        try:
            self.finish()
        finally:
            if self._token is not None:
                _ACTIVE.reset(self._token)
                self._token = None

    def finish(self) -> None:
        self._finished = True
        if self._live is not None:
            live, self._live = self._live, None
            live.stop()

    def _update(self, label: str) -> None:
        if self._live is not None:
            from rich.text import Text

            self._spinner.update(text=Text(label))

    def phase(self, name: str) -> None:
        if self._finished or self.mode == "off" or name == self._last_phase:
            return
        self._last_phase = name
        label = _PHASE_LABELS.get(name, name)
        if self.mode == "plain":
            print(f"molt: {label}", file=self.stream, flush=True)
        else:
            self._update(label)

    def frontend_module(self, module: str, total_s: float, *, timed_out: bool) -> None:
        # Plain logs stay bounded by phases, not the number of stdlib modules.
        if self.mode != "tty" or self._finished:
            return
        label = (
            f"Frontend: {module} timed out"
            if timed_out
            else f"Frontend: finished {module} in {total_s:.2f}s"
        )
        self._update(label)


def phase(name: str) -> None:
    active = _ACTIVE.get()
    if active is not None:
        active.phase(name)


def frontend_module(module: str, total_s: float, *, timed_out: bool = False) -> None:
    active = _ACTIVE.get()
    if active is not None:
        active.frontend_module(module, total_s, timed_out=timed_out)


def finish() -> None:
    active = _ACTIVE.get()
    if active is not None:
        active.finish()


def success_is_visible() -> bool:
    active = _ACTIVE.get()
    return active is None or not (active.quiet or active.json_output)


@contextmanager
def subprocess_status(label: str | None) -> Iterator[str | None]:
    """Route existing subprocess labels through this renderer, once.

    Outside a CLI build, retain the guard's existing keepalive behavior.
    Inside one, do not run a competing plain keepalive behind quiet/JSON/status.
    """
    active = _ACTIVE.get()
    if active is None:
        yield label
        return
    previous = active._last_phase
    try:
        if label is not None:
            active.phase(label)
        yield None
    finally:
        if previous is not None:
            active.phase(previous)


def notice(message: str) -> None:
    active = _ACTIVE.get()
    if active is None:
        print(message, file=sys.stderr, flush=True)
    else:
        active.phase(message)


def finish_after(function: Callable[_P, _R]) -> Callable[_P, _R]:
    """End a wrapper build's status before its caller runs the guest program."""

    @wraps(function)
    def wrapped(*args: _P.args, **kwargs: _P.kwargs) -> _R:
        try:
            return function(*args, **kwargs)
        finally:
            finish()

    return wrapped


def present_build(
    function: Callable[..., int],
    *,
    mode: str = "auto",
    headless: bool = False,
    quiet: bool = False,
    json_output: bool = False,
) -> Callable[..., int]:
    @wraps(function)
    def build(*args: Any, **kwargs: Any) -> int:
        with BuildProgress(
            mode=mode, headless=headless, quiet=quiet, json_output=json_output
        ):
            return function(*args, **kwargs)

    return build
