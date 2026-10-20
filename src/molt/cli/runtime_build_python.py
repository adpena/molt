"""Borrow the enclosing compiler operation's canonical Python runtime owner."""

from __future__ import annotations

from molt.process_environment import ProcessEnvironment, ProcessEnvironmentBuilder

from contextlib import contextmanager
from dataclasses import replace
from pathlib import Path
from typing import Generator, Mapping, TYPE_CHECKING, cast

if TYPE_CHECKING:
    from molt.cli.models import _RuntimeArtifactState
    from molt.cli.runtime_cargo_plan import RuntimeCargoPlan
    from molt.cli.runtime_identity_schema import RuntimeBuildIdentity
    from molt.python_runtime_admission import BuildPythonAdmission


BUILD_PYTHON_SIGNATURE = "MOLT_BUILD_PYTHON_SIGNATURE"


def pin_runtime_build_python_environment(
    environment: Mapping[str, str], *, cwd: Path
) -> ProcessEnvironment:
    """Pin effective generator selection before its first identity capture.

    Cargo launches build scripts from their crate directories. An absolute
    lexical entrypoint keeps both explicit selectors and relative PATH roots
    attached to the operation directory, preserving virtualenv aliases.
    """
    from molt.python_runtime_admission import resolve_build_python

    selected_environment = ProcessEnvironmentBuilder(environment)
    entrypoint = resolve_build_python(selected_environment, cwd=cwd)
    selected = next(
        (
            name
            for name in ("MOLT_BUILD_PYTHON", "PYTHON")
            if selected_environment.get(name, "").strip()
        ),
        "MOLT_BUILD_PYTHON",
    )
    selected_environment[selected] = str(entrypoint)
    return ProcessEnvironment(selected_environment)


def bind_runtime_build_python(
    plan: RuntimeCargoPlan, identity: RuntimeBuildIdentity
) -> RuntimeCargoPlan:
    """Carry already captured generator content into Cargo's rerun key.

    This is a projection of the validated build identity, not another Python
    admission. Later identity fences still capture and verify the live owner.
    """
    family = cast(Mapping[str, object], identity.payload["family"])
    compilation = cast(Mapping[str, object], family["compile"])
    configuration = cast(Mapping[str, object], compilation["common_config"])
    scripts = cast(Mapping[str, object], configuration["build_script_environment"])
    python = cast(Mapping[str, object], scripts["build_python"])
    signature = cast(str, python["content_digest"])
    previous = plan.environment.get(BUILD_PYTHON_SIGNATURE)
    if previous == signature:
        return plan
    if previous is not None:
        raise ValueError("runtime build Python signature differs from captured content")
    return replace(
        plan, environment=plan.environment.updated({BUILD_PYTHON_SIGNATURE: signature})
    )


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
