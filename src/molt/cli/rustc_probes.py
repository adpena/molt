"""The questions the runtime Cargo plan asks rustc, answered by the toolchain facts.

Each question (the target metadata for a target and flag set, the host triple)
runs the wrappers and then rustc, as Cargo's ``workspace_process`` does, keyed
in `molt.toolchain_facts` by every executable's identity, the argument vector,
the working directory, the typed environment and the files rustc consults. An
unchanged toolchain answers from the host store without a process (HF-357).
"""

from __future__ import annotations

import os
import re
import subprocess
import tempfile
from collections.abc import Generator, Mapping
from contextlib import ExitStack, contextmanager
from dataclasses import dataclass
from pathlib import Path
from types import MappingProxyType

from molt import process_guard
from molt.cli.cargo_target_cfg import (
    RustcTargetMetadata,
    cargo_target_query_arguments,
    parse_rustc_target_metadata,
    rust_probe_arguments,
)
from molt.process_environment import ProcessEnvironment, ProcessEnvironmentBuilder
from molt.rust_toolchain import rust_flag_spans
from molt.toolchain_facts import (
    RUSTC_PROBE_ENVIRONMENT,
    FileInput,
    ProbeAnswer,
    ProbeKey,
    ProbeTool,
    completed_probe,
    directory_inputs,
    environment_inputs,
    file_input,
    host_toolchain_facts,
)
from molt.toolchain_identity import _capture_executable_content


@dataclass(frozen=True, slots=True)
class RustProbeCommand:
    """Cargo workspace_process order (wrappers, then rustc), each tool captured.

    ``tools`` key a question in the toolchain-fact store; `fenced` runs the
    question only while every captured generation is proven current.
    """

    command: tuple[str, ...]
    tools: tuple[ProbeTool, ...]
    labels: tuple[str, ...]

    @contextmanager
    def fenced(self) -> Generator[list[str]]:
        with ExitStack() as stack:
            for tool, label in zip(self.tools, self.labels, strict=True):
                stack.enter_context(tool.fenced(label=label))
            yield list(self.command)


def rust_probe_command(
    rustc: Path, wrappers: Mapping[str, Path], *, label: str
) -> RustProbeCommand:
    command: list[str] = []
    tools: list[ProbeTool] = []
    labels: list[str] = []
    for role, path in (
        *(
            (role, wrappers[role])
            for role in ("RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER")
            if role in wrappers
        ),
        ("rustc", rustc),
    ):
        entrypoint, identity, _header = _capture_executable_content(
            path, label=f"{label}/{role}"
        )
        command.append(os.fspath(entrypoint))
        tools.append(ProbeTool.from_identity(entrypoint, identity))
        labels.append(f"{label}/{role}")
    return RustProbeCommand(tuple(command), tuple(tools), tuple(labels))


# The scratch directory a probe is confined to differs per call; the key names
# it by one placeholder, so equal questions share one answer.
_PROBE_SCRATCH_PLACEHOLDER = Path("<molt-probe-scratch>")


def _local_path(value: str, root: Path) -> Path:
    path = Path(value)
    return path if path.is_absolute() else root / path


def _rust_target_spec_inputs(
    flags: tuple[str, ...],
    target: str | None,
    *,
    root: Path,
    env: Mapping[str, str],
    rustc_content: Path,
) -> tuple[FileInput, ...]:
    """The files rustc 1.99 consults for a question's target and backend.

    ``Target::search`` takes a built-in triple first, then ``<name>.json``
    under each ``RUST_TARGET_PATH`` entry, then
    ``<sysroot>/lib/rustlib/<name>/target.json``; a name ending in ``.json``
    is a path. Every candidate is keyed, present or absent: one that appears
    later changes the answer. ``cfg(target_feature)`` comes from the codegen
    backend, so an explicit backend library and the sysroot's
    ``codegen-backends`` directories are keyed too.
    """
    sysroot = rustc_content.parent.parent
    names: dict[str, None] = {}
    backends: list[Path] = []
    for span in rust_flag_spans(flags):
        if span.option == "--target" and span.value:
            names[span.value] = None
        elif span.option == "--sysroot" and span.value:
            sysroot = _local_path(span.value, root)
        backend = None
        if span.codegen is not None and span.codegen.startswith("codegen-backend="):
            backend = span.codegen[len("codegen-backend=") :]
        elif (
            span.option == "-Z"
            and span.value is not None
            and span.value.startswith("codegen-backend=")
        ):
            backend = span.value[len("codegen-backend=") :]
        if backend and any(separator in backend for separator in "/\\"):
            backends.append(_local_path(backend, root))
    if target is not None:
        names[target] = None
    search = [
        _local_path(entry, root)
        for entry in (ProcessEnvironment(env).get("RUST_TARGET_PATH") or "").split(
            os.pathsep
        )
        if entry
    ]
    inputs: list[FileInput] = []
    for name in names:
        if name.endswith(".json"):
            inputs.append(file_input(_local_path(name, root)))
            continue
        inputs.extend(file_input(directory / f"{name}.json") for directory in search)
        inputs.append(file_input(sysroot / "lib" / "rustlib" / name / "target.json"))
    inputs.extend(file_input(path) for path in backends)
    rustlib = sysroot / "lib" / "rustlib"
    try:
        with os.scandir(rustlib) as iterator:
            hosts = sorted(
                entry.name for entry in iterator if entry.is_dir(follow_symlinks=False)
            )
    except OSError:
        hosts = []
    for host in hosts:
        inputs.extend(directory_inputs(rustlib / host / "codegen-backends"))
    return tuple(inputs)


def _rustc_probe_key(
    probe: RustProbeCommand,
    argv: tuple[str, ...],
    *,
    root: Path,
    env: Mapping[str, str],
    kind: str,
    inputs: tuple[FileInput, ...] = (),
) -> ProbeKey:
    """Key a rustc question by what can change its answer.

    Cargo's ``rustc_fingerprint`` (``src/util/rustc.rs``) hashes the rustc
    path, size and mtime, the wrappers and the rustup selection. Molt keys
    each executable's stat token and digest, the compiler's driver libraries
    beside the launcher (``<sysroot>/lib``; the dynamic libraries beside the
    launcher on Windows), the argument vector, the working directory, the
    typed environment set and the target-spec candidates.
    """
    rustc_content = Path(probe.tools[-1].content_path)
    return ProbeKey(
        kind,
        probe.tools,
        argv,
        os.fspath(root),
        environment_inputs(env, RUSTC_PROBE_ENVIRONMENT),
        (
            *directory_inputs(rustc_content.parent.parent / "lib"),
            *directory_inputs(rustc_content.parent, dynamic_libraries_only=True),
            *inputs,
        ),
    )


def rust_target_metadata(
    rustc: Path,
    target: str | None,
    flags: tuple[str, ...],
    *,
    root: Path,
    env: Mapping[str, str],
    wrappers: Mapping[str, Path] = MappingProxyType({}),
    qualify: bool = False,
) -> RustcTargetMetadata:
    """rustc's target metadata for one question, through the toolchain facts.

    A first answer is provisional: it lives in this process until a caller
    qualifies it. The Cargo plan replays each question under its captured
    resource closure with ``qualify=True``: a replay of a provisional answer
    runs rustc again and persists the answer only when both agree; a
    persisted answer is served without a process.
    """
    root = Path(os.path.abspath(root))
    facts = host_toolchain_facts()
    probe = rust_probe_command(rustc, wrappers, label="runtime rustc target metadata")
    key = _rustc_probe_key(
        probe,
        cargo_target_query_arguments(
            target, rust_probe_arguments(flags, _PROBE_SCRATCH_PLACEHOLDER)
        ),
        root=root,
        env=env,
        kind="rustc-target-metadata",
        inputs=_rust_target_spec_inputs(
            flags,
            target,
            root=root,
            env=env,
            rustc_content=Path(probe.tools[-1].content_path),
        ),
    )

    def run() -> subprocess.CompletedProcess[str]:
        with (
            tempfile.TemporaryDirectory(prefix="molt-rust-cfg-") as scratch,
            probe.fenced() as command,
        ):
            scratch_root = Path(scratch)
            (scratch_root / "output").mkdir()
            safe_flags = rust_probe_arguments(flags, scratch_root)
            probe_environment = ProcessEnvironmentBuilder(env)
            probe_environment.pop("RUSTC_LOG", None)
            return process_guard.run_completed_command(
                [*command, *cargo_target_query_arguments(target, safe_flags)],
                cwd=root,
                env=ProcessEnvironment(probe_environment),
                input="",
                capture_output=True,
                text=True,
                encoding="utf-8",
                timeout=30,
                memory_guard_prefix=None,
                check=False,
            )

    qualifying = qualify and not facts.persisted(key)
    served = None if qualifying else facts.lookup(key)
    result = (
        subprocess.CompletedProcess(list(key.argv), 0, served.stdout, served.stderr)
        if served is not None
        else run()
    )
    if result.returncode != 0:
        raise ValueError(
            f"runtime rustc target metadata failed ({result.returncode}): {result.stderr.strip()}"
        )
    metadata = parse_rustc_target_metadata(result.stdout, result.stderr)
    if served is None:
        answer = ProbeAnswer(result.stdout, result.stderr)
        if not qualifying:
            facts.record(key, answer, persist=False)
        elif facts.lookup(key) in (None, answer):
            facts.record(key, answer, persist=True)
        else:
            facts.forget(key)
    return metadata


def rustc_host_target(
    rustc: Path,
    wrappers: Mapping[str, Path],
    *,
    root: Path,
    env: Mapping[str, str],
) -> str:
    """The host triple rustc reports (`-vV`), through the toolchain facts."""
    host_probe = rust_probe_command(rustc, wrappers, label="runtime rustc host")

    def run_host_probe() -> subprocess.CompletedProcess[str]:
        with host_probe.fenced() as command:
            return process_guard.run_completed_command(
                [*command, "-vV"],
                cwd=root,
                env=ProcessEnvironment(env),
                capture_output=True,
                text=True,
                encoding="utf-8",
                timeout=30,
                memory_guard_prefix=None,
                check=False,
            )

    result, _cached = completed_probe(
        host_toolchain_facts(),
        _rustc_probe_key(host_probe, ("-vV",), root=root, env=env, kind="rustc-host"),
        run_host_probe,
    )
    match = re.search(r"(?m)^host:\s*(\S+)\s*$", result.stdout)
    if result.returncode != 0 or match is None:
        raise ValueError("runtime rustc host selection could not be proven")
    return match.group(1)
