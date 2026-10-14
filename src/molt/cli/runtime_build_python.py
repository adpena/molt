"""Borrow the enclosing compiler operation's canonical Python runtime owner."""

from __future__ import annotations

from contextlib import contextmanager
from typing import Generator, TYPE_CHECKING

if TYPE_CHECKING:
    from molt.cli.models import _RuntimeArtifactState
    from molt.python_runtime_admission import BuildPythonAdmission


@contextmanager
def build_python_scope(
    state: _RuntimeArtifactState | None,
) -> Generator[BuildPythonAdmission]:
    """Borrow the enclosing build owner, or close one standalone producer scope."""
    if state is not None and state.build_python_admission is not None:
        yield state.build_python_admission
        return
    from molt.python_runtime_admission import BuildPythonAdmission

    admission = BuildPythonAdmission()
    if state is not None:
        state.build_python_admission = admission
    try:
        with admission:
            yield admission
    finally:
        if state is not None and admission.terminal:
            state.build_python_admission = None
