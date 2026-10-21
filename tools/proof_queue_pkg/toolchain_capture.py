"""Compact one-capture toolchain custody and frozen-manifest verification."""

from __future__ import annotations

from molt.flag_strings import FlagStringRule
from molt.process_environment import ProcessEnvironment, ProcessEnvironmentBuilder

from concurrent.futures import ThreadPoolExecutor
from itertools import islice
from dataclasses import dataclass
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
import tempfile
import time
from typing import TYPE_CHECKING, Any, Iterable, Mapping, Sequence, cast

from molt.default_paths import executable_environment_value
from molt.exact_json import canonical_json_sha256
from molt.toolchain_identity import (
    find_executable,
    stable_regular_file_identity,
    verify_stable_regular_file_identity,
)
from molt.wasi_sdk_identity import (
    SDK_TOOL_NAMES,
    capture_wasi_sdk_tool_files,
    validate_wasi_sdk_selection,
)
from molt.rust_toolchain import (
    RustToolSearch,
    RustupProxyUnavailable,
    cargo_config_arguments,
    cargo_external_subcommand_path,
    rust_component_executables,
    rust_component_version_arguments,
    rust_component_version_matches,
    rust_toolchain_library_environment,
    resolve_rustup_proxy,
    rustup_proxy_selection,
    relative_rustc_tool_paths,
    rustc_host,
    rustc_printed_sysroot,
)
from tools.command_execution import CommandExecutor
from tools.proof_queue_pkg import custody_cas
from tools.proof_queue_pkg.process_image_capture import (
    PROCESS_IMAGE_SCHEMA,
    _image_path_key,
    custody_file,
    custody_path,
    require_custody_coordinate,
    canonical_images,
    capture_image,
    revalidate_images,
)


if TYPE_CHECKING:
    from molt.wasi_sdk_identity import WasiCAbiProjection
    from tools.proof_queue_pkg import command_admission


CAPTURE_SCHEMA = "molt.proof-toolchain-capture.v1"
VERIFICATION_SCHEMA = "molt.proof-toolchain-verification.v1"
# std::process::Command's Debug form, which rustc uses to print link commands:
# an optional `cd "dir" && `, then `env -i ` (cleared) or `env -u NAME ...`
# (removed variables; rustc strips Apple deployment targets this way), then
# `NAME="value"` assignments, an optional `["program"] ` when the executable
# differs from argv[0], and the quoted argv.
_COMMAND_CWD_PREFIX = re.compile(r'cd (?=")')
_COMMAND_ENVIRONMENT_EDIT = re.compile(r'env(?: -i| -u [^\s"]+)+ ')
_COMMAND_ENVIRONMENT_ASSIGNMENT = re.compile(r'[A-Za-z_][A-Za-z0-9_]*=(?=")')
_COMMAND_PROGRAM_OVERRIDE = re.compile(r'\[(?=")')
# gcc collect2.cc under -debug: fprintf (stderr, "ld_file_name        = %s\n").
_COLLECT2_LINKER_REPORT = re.compile(r"ld_file_name +=\x20(.*)")
_COMMANDS = CommandExecutor.for_file(__file__)


def _image_membership_error(
    message: str,
    *,
    unit: object = None,
    role: object = None,
    selected: object = None,
    content: object = None,
    images: Iterable[tuple[str, str]] = (),
) -> ValueError:
    """Retain the rejected edge without dumping environments or transcripts."""

    def bounded(value: object) -> str | None:
        return value[:512] if isinstance(value, str) else None

    captured = list(islice(images, 9))
    detail = {
        "unit": bounded(unit),
        "role": bounded(role),
        "selected_path": bounded(selected),
        "content_path": bounded(content),
        "captured_images": [
            {"role": bounded(image_role), "path": bounded(path)}
            for image_role, path in captured[:8]
        ],
        "captured_images_truncated": len(captured) > 8,
    }
    return ValueError(message + "; image_edge=" + json.dumps(detail, sort_keys=True))


class RustLinkCaptureError(ValueError):
    """Child failure details with full probe transcripts for the existing CAS."""

    def __init__(self, message: str, *, unit: str, probes: list[dict[str, object]]):
        unit_probes = [probe for probe in probes if probe.get("unit") == unit]
        phase = str(unit_probes[-1]["phase"]) if unit_probes else "configuration"
        last_probe = unit_probes[-1] if unit_probes else {}
        self.returncode = last_probe.get("returncode")
        self.stderr = str(last_probe.get("stderr", ""))
        detail = f"Rust linker capture {unit}/{phase}: {message}"
        if self.returncode not in (None, 0):
            detail += f" (exit status {self.returncode})\nstderr:\n"
            detail += self.stderr or "(empty)"
        super().__init__(detail)
        self.diagnostic = {
            "schema": "molt.proof-rust-link-capture-failure.v1",
            "unit": unit,
            "phase": phase,
            "reason": message,
            "probes": list(probes),
        }


def _run_rust_link_probe(
    command: Sequence[str],
    *,
    phase: str,
    unit: str,
    cwd: Path,
    compiler_cwd: Path,
    env: ProcessEnvironment,
    timeout: float,
    probes: list[dict[str, object]],
    input: str | None = None,
) -> subprocess.CompletedProcess[str]:
    """Retain command results, without logging environment values or retrying."""
    require_custody_coordinate(Path(command[0]))
    record: dict[str, object] = {
        "phase": phase,
        "unit": unit,
        "argv": list(command),
        "cwd": str(cwd),
        "compiler_cwd": str(compiler_cwd),
        "returncode": None,
        "stdout": "",
        "stderr": "",
    }
    probes.append(record)
    try:
        result = _COMMANDS.run(
            command,
            cwd=cwd,
            env=env,
            check=False,
            capture_output=True,
            text=True,
            timeout=timeout,
            encoding="utf-8",
            **({"input": input} if input is not None else {}),
        )
    except Exception as exc:
        for stream in ("stdout", "stderr"):
            value = getattr(exc, stream, "") or ""
            record[stream] = (
                value.decode("utf-8", errors="replace")
                if isinstance(value, bytes)
                else str(value)
            )
        record["exception_type"] = type(exc).__name__
        record["exception_message"] = str(exc)
        raise RustLinkCaptureError(
            f"probe execution raised {type(exc).__name__}",
            unit=unit,
            probes=probes,
        ) from exc
    record.update(
        returncode=result.returncode, stdout=result.stdout, stderr=result.stderr
    )
    return result


def command_requires_clippy(envelope: Mapping[str, object]) -> bool:
    from tools.proof_queue_pkg import command_admission

    if "cargo" not in envelope.get("toolchains", []):
        return False
    invocation = command_admission.cargo_invocation_for_envelope(envelope)
    return invocation is not None and invocation.subcommand == "clippy"


def select_clippy_component_paths(
    *, cwd: Path, env: Mapping[str, str]
) -> dict[str, str]:
    """Resolve exactly the Cargo plugin and the driver its current_exe selects."""
    plugin, driver = rust_component_executables("clippy")
    launcher = custody_path(cargo_external_subcommand_path("clippy", cwd=cwd, env=env))
    component = custody_path(
        resolve_rustup_proxy(launcher, role=plugin, root=cwd, env=env)
    )
    content = component.resolve(strict=True)
    driver_path = component.with_name(driver + (".exe" if os.name == "nt" else ""))
    for path in (content, driver_path):
        require_custody_coordinate(path)
        if (
            not path.is_file()
            or not path.stat().st_size
            or not os.access(path, os.X_OK)
        ):
            raise ValueError(
                f"Clippy requires an available component executable: {path}"
            )
    content_driver = content.with_name(driver + (".exe" if os.name == "nt" else ""))
    if not content_driver.is_file() or content_driver.resolve(
        strict=True
    ) != driver_path.resolve(strict=True):
        raise ValueError(
            "Clippy invoked and content paths select different sibling drivers"
        )
    paths = {
        "clippy-launcher": str(launcher),
        "cargo-clippy": str(component),
        "clippy-driver": str(driver_path),
    }
    resolver = rustup_proxy_selection(launcher)
    if resolver is not None:
        paths["clippy-rustup"] = str(resolver.entrypoint)
    return paths


def clippy_component_images(paths: Mapping[str, str]) -> list[dict[str, object]]:
    return canonical_images(
        [
            capture_image(role, Path(path), preserve_path=True)
            for role, path in paths.items()
        ]
        + [capture_image(role, Path(path)) for role, path in paths.items()]
    )


def capture_clippy_selection(
    paths: Mapping[str, str],
    *,
    compiler_version: str,
    cwd: Path,
    env: Mapping[str, str],
) -> dict[str, object]:
    fields = dict(
        line.split(": ", 1) for line in compiler_version.splitlines() if ": " in line
    )
    selected_env = ProcessEnvironment(env).updated(
        rust_toolchain_library_environment(Path(paths["cargo-clippy"]), env)
    )
    versions: dict[str, str] = {}
    probes: list[dict[str, object]] = []
    for role in rust_component_executables("clippy"):
        arguments = [paths[role], *rust_component_version_arguments(role)]
        component = Path(paths["cargo-clippy"]).resolve(strict=True)
        resolver = paths.get("clippy-rustup")
        if resolver is not None:
            # A content-proven Rustup proxy selected this component. Preserve
            # its loader setup for both wrapper and independent driver probes.
            arguments = [resolver, "run", str(component.parent.parent), *arguments]
        result = _run_rust_link_probe(
            arguments,
            phase="component-version",
            unit="clippy",
            cwd=cwd,
            compiler_cwd=cwd,
            env=selected_env,
            timeout=30.0,
            probes=probes,
        )
        if result.returncode or not rust_component_version_matches(
            role, result.stdout, fields
        ):
            raise ValueError(
                f"selected {role} version probe conflicts with selected compiler"
            )
        versions[role] = result.stdout.strip()
    return {
        "schema": "molt.proof-clippy-selection.v1",
        "paths": dict(paths),
        "versions": versions,
    }


def validate_clippy_selection(
    identity: Mapping[str, object],
    *,
    required: bool | None = None,
    expected_paths: Mapping[str, str] | None = None,
) -> None:
    selection = identity.get("clippy_selection")
    if selection is None:
        if required:
            raise ValueError("Clippy command is missing its component closure")
        return
    if required is False:
        raise ValueError("non-Clippy command has unexpected Clippy component closure")
    if (
        not isinstance(selection, Mapping)
        or set(selection) != {"schema", "paths", "versions"}
        or selection.get("schema") != "molt.proof-clippy-selection.v1"
    ):
        raise ValueError("Clippy component selection is malformed")
    paths, versions = selection["paths"], selection["versions"]
    roles = rust_component_executables("clippy")
    if (
        not isinstance(paths, Mapping)
        or set(paths)
        not in (
            {"clippy-launcher", *roles},
            {"clippy-launcher", "clippy-rustup", *roles},
        )
        or not isinstance(versions, Mapping)
        or set(versions) != set(roles)
    ):
        raise ValueError("Clippy component family is incomplete")
    if expected_paths is not None and dict(paths) != dict(expected_paths):
        raise ValueError("Clippy component selection changed")
    images = identity.get("process_images")
    if not isinstance(images, list):
        raise ValueError("Clippy components have no process images")
    captured = {(row["role"], row["path"]) for row in canonical_images(images)}
    for role, raw in paths.items():
        if not isinstance(raw, str) or not Path(raw).is_absolute():
            raise ValueError("Clippy component path is malformed")
        path = Path(raw)
        require_custody_coordinate(path)
        # capture_image records the resolved image under custody spelling too,
        # which on Windows lowercases the drive anchor that resolve() restores.
        if (role, str(custody_path(path))) not in captured or (
            role,
            str(custody_path(path.resolve(strict=True))),
        ) not in captured:
            raise ValueError(f"Clippy component image closure is incomplete: {role}")
    launcher = Path(str(paths["clippy-launcher"]))
    content = Path(str(paths["cargo-clippy"]))
    if (launcher.resolve(strict=True) != content.resolve(strict=True)) != (
        "clippy-rustup" in paths
    ):
        raise ValueError("Clippy proxy resolver closure is incomplete")
    if "clippy-rustup" in paths:
        resolver = Path(str(paths["clippy-rustup"]))
        if resolver.name != ("rustup.exe" if os.name == "nt" else "rustup"):
            raise ValueError("Clippy resolver must preserve the Rustup CLI entrypoint")
        by_edge = {(row["role"], row["path"]): row["sha256"] for row in images}
        if (
            by_edge[("clippy-rustup", str(resolver))]
            != by_edge[("clippy-launcher", str(launcher))]
        ):
            raise ValueError("Clippy Rustup resolver differs from selected proxy")
    driver = content.with_name("clippy-driver" + (".exe" if os.name == "nt" else ""))
    content_driver = content.resolve(strict=True).with_name(driver.name)
    if str(driver) != paths["clippy-driver"] or content_driver.resolve(
        strict=True
    ) != driver.resolve(strict=True):
        raise ValueError("Clippy driver is not the selected plugin sibling")
    compiler_version = identity.get("version")
    if not isinstance(compiler_version, str):
        raise ValueError("Clippy selection has no compiler version")
    fields = dict(
        line.split(": ", 1) for line in compiler_version.splitlines() if ": " in line
    )
    for role in roles:
        version = versions[role]
        if not isinstance(version, str) or not rust_component_version_matches(
            role, version, fields
        ):
            raise ValueError(f"Clippy {role} identity conflicts with selected compiler")


def select_cargo_build_tool_environment(
    *, cwd: Path, env: Mapping[str, str], rustdoc_required: bool = False
) -> tuple[dict[str, str], dict[str, object]]:
    """Bind Cargo's header, formatting and documentation process selections.

    A Rust linker is not the bindgen compiler authority. The queue selects an
    optional host driver once and publishes the upstream explicit CLANG_PATH
    hook, including for cross-target headers. This is a pinned-driver policy,
    not a replay of every build script's target-prefixed ambient discovery.
    Existing executable-environment custody hashes, watches and admits it;
    no installation directory or ambient candidate set is admitted. Rustup
    proxies become their selected component before the same executable-input
    capture; Cargo documentation and doctests do not get a separate allowlist.
    """
    env = ProcessEnvironment(env)
    updates: dict[str, str] = {}
    probes: list[dict[str, object]] = []
    search_diagnostics: list[dict[str, object]] = []
    source = "unavailable"
    selected: Path | None = None

    def explicit_path(name: str, value: str) -> Path:
        path = Path(value)
        if not path.is_absolute():
            raise ValueError(
                f"Cargo build tool {name} requires an absolute executable path; "
                "build-script working directories differ from Cargo's invocation directory"
            )
        selected = custody_file(path)
        if selected is None or not os.access(selected, os.X_OK):
            raise ValueError(f"Cargo build tool {name} must name one executable file")
        return selected

    def query(path: Path, arguments: Sequence[str], *, required: bool) -> str | None:
        try:
            completed = _run_rust_link_probe(
                [str(path), *arguments],
                phase="header-driver-selection",
                unit="cargo-bindgen",
                cwd=cwd,
                compiler_cwd=cwd,
                env=env,
                timeout=30.0,
                probes=probes,
            )
        except RustLinkCaptureError:
            if required:
                raise
            return None  # The probe transcript retains the unavailable capability.
        if completed.returncode != 0:
            if not required:
                return None
            raise RustLinkCaptureError(
                "header-driver selector command failed",
                unit="cargo-bindgen",
                probes=probes,
            )
        lines = completed.stdout.splitlines()
        if len(lines) != 1 or not lines[0].strip():
            probes[-1]["selection_error"] = "selector did not print exactly one path"
            if not required:
                return None
            raise RustLinkCaptureError(
                "header-driver selector must print exactly one path",
                unit="cargo-bindgen",
                probes=probes,
            )
        return lines[0].strip()

    clang_value = executable_environment_value(env, "CLANG_PATH")
    config_value = executable_environment_value(env, "LLVM_CONFIG_PATH")
    config = None
    if config_value:
        config = explicit_path("LLVM_CONFIG_PATH", config_value)
    else:
        # Library discovery invokes llvm-config independently of Clang driver
        # discovery, even when the caller supplies an explicit CLANG_PATH.
        config = find_executable("llvm-config", environment=env, cwd=cwd)
    if config is not None:
        updates["LLVM_CONFIG_PATH"] = str(config)
    if clang_value:
        selected = explicit_path("CLANG_PATH", clang_value)
        source = "CLANG_PATH"
    else:
        search: list[tuple[Path, str]] = []
        if config is not None:
            output = query(config, ["--bindir"], required=bool(config_value))
            if output is not None:
                raw = Path(output)
                search.append(
                    (raw if raw.is_absolute() else cwd / raw, "llvm-config --bindir")
                )
        path_value = executable_environment_value(env, "PATH")
        if path_value:
            for value in path_value.split(os.pathsep):
                path = Path(value.strip('"') if os.name == "nt" else value)
                search.append((path if path.is_absolute() else cwd / path, "PATH"))
        suffix = ".exe" if os.name == "nt" else ""
        for directory, authority in search:
            try:
                # Do not enumerate a directory when its ordinary entrypoint
                # already suffices. A PATH entry can permit file execution but
                # deny directory enumeration, especially on Windows.
                ordinary = directory / f"clang{suffix}"
                if ordinary.is_file() and os.access(ordinary, os.X_OK):
                    selected = ordinary.absolute()
                else:
                    selected = next(
                        (
                            path.absolute()
                            for path in sorted(directory.glob(f"clang-[0-9]*{suffix}"))
                            if path.is_file() and os.access(path, os.X_OK)
                        ),
                        None,
                    )
            except OSError as exc:
                # An implicit search miss is not authority to change access
                # controls or fail an otherwise usable later PATH selection.
                search_diagnostics.append(
                    {
                        "source": authority,
                        "directory": str(directory),
                        "exception_type": type(exc).__name__,
                        "errno": exc.errno,
                        "winerror": getattr(exc, "winerror", None),
                    }
                )
                continue
            if selected is not None:
                source = authority
                break
        if selected is None and sys.platform == "darwin":
            xcrun = find_executable("xcrun", environment=env, cwd=cwd)
            if xcrun is not None:
                output = query(xcrun, ["--find", "clang"], required=False)
                if output is not None:
                    selected = explicit_path("xcrun-selected Clang", output)
                    source = "xcrun --find clang"
    if selected is not None:
        updates["CLANG_PATH"] = str(selected)
    rust_components: dict[str, bool] = {}
    for role, names, required in (
        ("rustfmt", ("RUSTFMT",), False),
        ("rustdoc", ("RUSTDOC", "CARGO_BUILD_RUSTDOC"), rustdoc_required),
    ):
        explicit = next(
            (
                (name, value)
                for name in names
                if (value := executable_environment_value(env, name))
            ),
            None,
        )
        component = (
            explicit_path(*explicit)
            if explicit is not None
            else find_executable(role, environment=env, cwd=cwd)
        )
        if component is not None:
            try:
                component = resolve_rustup_proxy(
                    component, role=role, root=cwd, env=env
                )
            except RustupProxyUnavailable as exc:
                if explicit is not None or required:
                    raise  # Never replace an explicit or required component.
                probes.append(exc.diagnostic)
                component = None
        if component is None and required:
            raise ValueError(
                f"Cargo requires an available {role} executable before capture"
            )
        if component is not None:
            updates[names[0]] = str(component)
            # RUSTDOC precedes Cargo's build.rustdoc hook. Collapse any supplied
            # lower-priority hook to that selection rather than admit a shadowed
            # executable as a second process authority.
            for name in names[1:]:
                if executable_environment_value(env, name):
                    updates[name] = str(component)
        rust_components[role] = component is not None
    return updates, {
        "schema": "molt.proof-cargo-build-tool-selection.v1",
        "available": selected is not None,
        "source": source,
        "formatter_available": rust_components["rustfmt"],
        "rustdoc_available": rust_components["rustdoc"],
        "rustdoc_required": rustdoc_required,
        "bound_names": sorted(updates),
        "probes": probes,
        "search_diagnostics": search_diagnostics,
    }


@dataclass(frozen=True)
class FrozenFile:
    path: str
    sha256: str
    size: int | None

    def as_dict(self) -> dict[str, object]:
        return {"path": self.path, "sha256": self.sha256, "size": self.size}


def _command_tokens(line: str) -> list[str]:
    """Decode rustc's quoted command-debug output without platform guessing."""
    tokens: list[str] = []
    decoder = json.JSONDecoder()
    index = 0
    if cwd := _COMMAND_CWD_PREFIX.match(line):
        _directory, end = decoder.raw_decode(line, cwd.end())
        if not line.startswith(" && ", end):
            raise ValueError("rust linker command has a malformed working directory")
        index = end + len(" && ")
    if edit := _COMMAND_ENVIRONMENT_EDIT.match(line, index):
        index = edit.end()
    while assignment := _COMMAND_ENVIRONMENT_ASSIGNMENT.match(line, index):
        value, end = decoder.raw_decode(line, assignment.end())
        if not isinstance(value, str) or end >= len(line) or not line[end].isspace():
            raise ValueError(
                "rust linker command contains a malformed environment assignment"
            )
        index = end
        while index < len(line) and line[index].isspace():
            index += 1
    program: str | None = None
    if override := _COMMAND_PROGRAM_OVERRIDE.match(line, index):
        program, end = decoder.raw_decode(line, override.end())
        if not isinstance(program, str) or not line.startswith("] ", end):
            raise ValueError("rust linker command has a malformed program override")
        index = end + len("] ")
    if index and (index >= len(line) or line[index] != '"'):
        raise ValueError("rust linker environment assignments have no quoted command")
    command_start = index
    while index < len(line):
        while index < len(line) and line[index].isspace():
            index += 1
        if index >= len(line):
            break
        if line[index] != '"':
            return shlex.split(line[command_start:], posix=os.name != "nt")
        value, end = decoder.raw_decode(line, index)
        if not isinstance(value, str):
            raise ValueError("rust linker command contains a non-string argument")
        tokens.append(value)
        index = end
    if program is not None and tokens:
        # argv[0] is display-only once the executable is named separately.
        tokens[0] = program
    return tokens


def _selected_command_lines(output: str) -> list[list[str]]:
    commands: list[list[str]] = []
    for line in output.splitlines():
        stripped = line.strip()
        if not stripped.startswith('"') and not any(
            prefix.match(stripped)
            for prefix in (
                _COMMAND_CWD_PREFIX,
                _COMMAND_ENVIRONMENT_EDIT,
                _COMMAND_ENVIRONMENT_ASSIGNMENT,
                _COMMAND_PROGRAM_OVERRIDE,
            )
        ):
            continue
        try:
            tokens = _command_tokens(stripped)
        except (ValueError, json.JSONDecodeError):
            continue
        if tokens:
            commands.append(tokens)
    return commands


def _driver_command_tokens(line: str) -> list[str]:
    """Decode one `-###` command line as gcc and clang print it.

    Both drivers print every argument after a space. gcc quotes an argument
    unless it is plainly safe, clang always quotes, and inside quotes only a
    double quote, a backslash or a dollar sign is backslash-escaped.
    """
    tokens: list[str] = []
    index = 0
    while index < len(line):
        if line[index] == " ":
            index += 1
            continue
        if line[index] != '"':
            end = line.find(" ", index)
            end = len(line) if end < 0 else end
            if '"' in line[index:end] or "\\" in line[index:end]:
                raise ValueError("driver command has a malformed bare argument")
            tokens.append(line[index:end])
            index = end
            continue
        value: list[str] = []
        index += 1
        while True:
            if index >= len(line):
                raise ValueError("driver command has an unterminated quote")
            character = line[index]
            if character == "\\":
                if index + 1 >= len(line) or line[index + 1] not in '"\\$':
                    raise ValueError("driver command has an unknown escape")
                value.append(line[index + 1])
                index += 2
            elif character == '"':
                index += 1
                break
            else:
                value.append(character)
                index += 1
        if index < len(line) and line[index] != " ":
            raise ValueError("driver command argument runs into the next one")
        tokens.append("".join(value))
    return tokens


def _driver_command_lines(output: str) -> list[list[str]]:
    """Select the helper commands a compiler driver reports under `-###`.

    Command lines are indented by exactly one space; banner lines (`Target:`,
    `COLLECT_GCC_OPTIONS=`, ...) start at column zero. Clang also prints an
    unquoted `(in-process)` annotation before an integrated frontend command.
    """
    commands: list[list[str]] = []
    for line in output.splitlines():
        if line == " (in-process)":
            continue
        if not line.startswith(" ") or line.startswith("  "):
            continue
        try:
            tokens = _driver_command_tokens(line.rstrip("\r"))
        except ValueError:
            continue
        if tokens:
            commands.append(tokens)
    return commands


def _is_gcc_collect2(path: Path) -> bool:
    return path.name.casefold() in {"collect2", "collect2.exe"}


def _collect2_selected_linker(
    driver: Path,
    link_argv: Sequence[str],
    *,
    unit: str,
    cwd: Path,
    env: Mapping[str, str],
    probes: list[dict[str, object]],
) -> tuple[Path, dict[str, object]]:
    """Ask GCC's collect2 which linker it executes for this exact link.

    `-###` stops at collect2, which picks the real linker itself (gcc
    collect2.cc main: real-ld, collect-ld, then the `-fuse-ld` name in the
    driver's COMPILER_PATH, then PATH). Its `-debug` report prints that
    `ld_file_name` before the exec, so relink the synthetic inputs through the
    same driver argv instead of re-deriving GCC's search. The driver's
    `-print-prog-name=ld` follows a different search and can name another
    file than the one collect2 runs.
    """
    env = ProcessEnvironment(env)
    report = _run_rust_link_probe(
        [str(driver), *link_argv, "-Wl,-debug"],
        phase="collect2-linker",
        unit=unit,
        cwd=cwd,
        compiler_cwd=cwd,
        env=env,
        timeout=120.0,
        probes=probes,
    )
    reported = [
        match.group(1)
        for line in report.stderr.splitlines()
        if (match := _COLLECT2_LINKER_REPORT.fullmatch(line.rstrip("\r")))
    ]
    if len(reported) != 1:
        raise ValueError(f"GCC collect2 reported {len(reported)} linker selections")
    if reported[0] == "not found":
        raise ValueError(
            "GCC collect2 found no linker for the selected link arguments; install "
            "the linker that -fuse-ld/-B select or put it on the driver's PATH"
        )
    if report.returncode != 0:
        raise ValueError("GCC collect2 linker report link failed")
    selected = find_executable(reported[0], cwd=cwd, environment=env)
    if selected is None:
        raise ValueError(
            f"GCC collect2 selected linker is unavailable: {reported[0]!r}"
        )
    path = Path(selected).absolute()
    return path, {
        "requested": reported[0],
        "path": str(path),
        "content_path": str(path.resolve(strict=True)),
        "origin": "collect2-report",
    }


def _selected_rust_link_command(stdout: str, stderr: str) -> list[str]:
    """Select exactly one rustc link command from its complete output stream."""
    commands = _selected_command_lines(stdout + "\n" + stderr)
    if len(commands) != 1:
        raise ValueError(
            f"synthetic Rust linker selection returned {len(commands)} commands"
        )
    return commands[0]


def _cargo_context_request(command: Sequence[str], *, cwd: Path):
    """One selector projection for Cargo context queries and their receivers."""
    context_args = cargo_config_arguments(command, cwd=cwd)
    packages: list[str] = []
    selectors: list[tuple[str, str | None]] = []
    root_override = None
    index = 1
    while index < len(command) and command[index] != "--":
        value = command[index]
        name, equal, inline = value.partition("=")
        if name in {
            "--manifest-path",
            "--package",
            "-p",
            "--bin",
            "--example",
            "--test",
            "--bench",
            "-Z",
        }:
            if equal:
                argument = inline
            else:
                index += 1
                if index >= len(command):
                    raise ValueError(f"Cargo {name} requires a value")
                argument = command[index]
            if name == "--manifest-path":
                path = Path(argument)
                context_args.extend(
                    (name, str(path if path.is_absolute() else cwd / path))
                )
            elif name in {"--package", "-p"}:
                packages.append(argument)
            elif name == "-Z":
                if argument.startswith("root-dir="):
                    root_override = cwd / argument.split("=", 1)[1]
            else:
                selectors.append((name[2:], argument))
        elif value == "--lib":
            selectors.append(("lib", None))
        elif value.startswith("-p") and len(value) > 2:
            packages.append(value[2:])
        elif value.startswith("-Zroot-dir="):
            root_override = cwd / value.split("=", 1)[1]
        index += 1

    return context_args, packages, selectors, root_override


def _cargo_context_from_metadata(
    metadata: object,
    command: Sequence[str],
    *,
    cwd: Path,
    package_ids: Mapping[str, str],
    require_crate_types: bool,
) -> dict[str, object]:
    """Derive selected source/target facts from Cargo's retained metadata."""
    if not isinstance(metadata, dict):
        raise ValueError("Cargo compiler-cwd metadata is not an object")
    _args, packages, selectors, root_override = _cargo_context_request(command, cwd=cwd)
    if (
        not isinstance(package_ids, Mapping)
        or set(package_ids) != set(packages)
        or any(
            not isinstance(value, str) or not value for value in package_ids.values()
        )
    ):
        raise ValueError("Cargo selected package facts differ from command selectors")
    selected_ids = (
        list(package_ids.values())
        if packages
        else metadata.get("workspace_default_members", [])
    )
    workspace = Path(str(metadata["workspace_root"]))
    root = workspace if root_override is None else root_override.absolute()
    contexts = []
    for package in metadata.get("packages", []):
        if package.get("id") not in selected_ids:
            continue
        package_root = Path(package["manifest_path"]).parent
        for target in package.get("targets", []):
            kinds = target.get("kind", [])
            if "custom-build" in kinds:
                continue  # Forwarded cargo-rustc arguments never apply here.
            if not selectors and all(
                kind in {"example", "test", "bench"} for kind in kinds
            ):
                continue
            if selectors and not any(
                (
                    kind in kinds
                    or kind == "lib"
                    and any(
                        k in kinds
                        for k in (
                            "lib",
                            "rlib",
                            "dylib",
                            "cdylib",
                            "staticlib",
                            "proc-macro",
                        )
                    )
                )
                and (name is None or name == target.get("name"))
                for kind, name in selectors
            ):
                continue
            source = Path(target["src_path"])
            compiler_cwd = (
                root
                if package.get("source") is None and source.is_relative_to(root)
                else package_root
            )
            selected_context = {
                "package_id": package["id"],
                "source": str(source),
                "compiler_cwd": str(compiler_cwd),
            }
            if require_crate_types:
                kinds = target.get("crate_types")
                if (
                    not isinstance(kinds, list)
                    or not kinds
                    or any(not isinstance(kind, str) for kind in kinds)
                ):
                    raise ValueError(
                        "Cargo metadata has no selected target crate types"
                    )
                selected_context.update(
                    crate_types=kinds, manifest_path=package["manifest_path"]
                )
            contexts.append(selected_context)
    directories = {row["compiler_cwd"] for row in contexts}
    if len(directories) != 1 or not Path(next(iter(directories))).is_absolute():
        raise ValueError(
            "Cargo forwarded relative tool path has no unique compiler-cwd provenance: "
            + json.dumps(contexts, sort_keys=True)
        )
    if require_crate_types and len(contexts) != 1:
        raise ValueError("forwarded crate types require one selected Cargo target")
    return {
        "compiler_cwd": next(iter(directories)),
        "workspace_root": str(workspace),
        "sources": contexts,
        "metadata_sha256": canonical_json_sha256(metadata),
    }


def _cargo_forwarded_compiler_context(
    cargo: Path,
    command: Sequence[str],
    *,
    cwd: Path,
    env: Mapping[str, str],
    probes: list[dict[str, object]] | None = None,
    unit: str = "target",
    require_crate_types: bool = False,
) -> dict[str, object]:
    """Resolve the compiler cwd from Cargo-owned workspace/package provenance.

    Cargo 1.99 src/util/workspace.rs::path_args uses the workspace root for path
    sources below that root, otherwise the package root. The optional root-dir
    flag changes the first root, not the fallback. Never infer this from cwd or
    a hand-parsed Cargo.toml, and never compile an original package here.
    """
    env = ProcessEnvironment(env)
    context_args, packages, _selectors, _root_override = _cargo_context_request(
        command, cwd=cwd
    )
    probe_records = probes if probes is not None else []

    def query(arguments: list[str]) -> object:
        result = _run_rust_link_probe(
            [str(cargo), *arguments, *context_args],
            phase="cargo-context-" + arguments[0],
            unit=unit,
            cwd=cwd,
            compiler_cwd=cwd,
            env=env,
            timeout=30.0,
            probes=probe_records,
        )
        if result.returncode != 0:
            raise ValueError(
                "Cargo compiler-cwd provenance unavailable: metadata command failed"
            )
        return result.stdout

    metadata_arguments = [
        "metadata",
        "--offline",
        "--locked",
        "--no-deps",
        "--format-version",
        "1",
    ]
    discovery = json.loads(str(query(metadata_arguments)))
    package_ids = {
        package: str(
            query(["pkgid", "--offline", "--locked", "--package", package])
        ).strip()
        for package in packages
    }
    context = _cargo_context_from_metadata(
        discovery,
        command,
        cwd=cwd,
        package_ids=package_ids,
        require_crate_types=require_crate_types,
    )
    if not require_crate_types:
        return context

    # Cargo owns target discovery. Only ambiguous forwarded kinds need this
    # second query: discovery locates finite inputs before their generation
    # fence; the authoritative metadata query runs inside that fence.
    inputs = {Path(context["workspace_root"]) / "Cargo.toml"}
    for source in context["sources"]:
        inputs.update((Path(source["manifest_path"]), Path(source["source"])))
    captured = [
        stable_regular_file_identity(path, label="Cargo artifact input")
        for path in sorted(inputs)
    ]
    raw = str(query(metadata_arguments))
    final_probe = dict(probe_records[-1])
    final_packages = {}
    package_probes = []
    for package in packages:
        final_packages[package] = str(
            query(["pkgid", "--offline", "--locked", "--package", package])
        ).strip()
        package_probes.append(dict(probe_records[-1]))
    for identity in captured:
        verify_stable_regular_file_identity(identity, label="Cargo artifact input")
    if final_packages != package_ids:
        raise ValueError("Cargo package selection changed during metadata capture")
    selected = _cargo_context_from_metadata(
        json.loads(raw),
        command,
        cwd=cwd,
        package_ids=final_packages,
        require_crate_types=True,
    )

    def locations(value):
        return (
            value["workspace_root"],
            value["compiler_cwd"],
            [
                (
                    row["package_id"],
                    row["source"],
                    row["manifest_path"],
                    row["compiler_cwd"],
                )
                for row in value["sources"]
            ],
        )

    if locations(selected) != locations(context):
        raise ValueError(
            "Cargo artifact input selection changed during metadata capture"
        )
    identities = {
        str(value.path): {
            "path": str(value.path),
            "sha256": value.sha256,
            "size": value.size,
        }
        for value in captured
    }
    for source in selected["sources"]:
        source["manifest"] = identities[source["manifest_path"]]
    selected.update(
        metadata_probe=final_probe,
        metadata_sha256=hashlib.sha256(raw.encode()).hexdigest(),
        package_probes=package_probes,
        invocation_cwd=str(cwd),
        input_files=list(identities.values()),
    )
    return selected


def _cargo_artifact_crate_types(
    context: object, command: Sequence[str]
) -> tuple[str, ...]:
    """Re-derive the selected target from the same retained Cargo transcript."""
    if not isinstance(context, Mapping) or not isinstance(
        context.get("metadata_probe"), Mapping
    ):
        raise ValueError("Cargo artifact context has no captured metadata")
    probe = context["metadata_probe"]
    cwd = context.get("invocation_cwd")
    raw = probe.get("stdout")
    if (
        not isinstance(cwd, str)
        or not Path(cwd).is_absolute()
        or probe.get("cwd") != cwd
        or probe.get("compiler_cwd") != cwd
        or probe.get("phase") != "cargo-context-metadata"
        or probe.get("unit") != "target"
        or probe.get("returncode") != 0
        or not isinstance(raw, str)
        or context.get("metadata_sha256") != hashlib.sha256(raw.encode()).hexdigest()
    ):
        raise ValueError("Cargo artifact metadata generation is malformed")
    args, packages, _selectors, _override = _cargo_context_request(
        command, cwd=Path(cwd)
    )
    argv = probe.get("argv")
    expected_args = [
        "metadata",
        "--offline",
        "--locked",
        "--no-deps",
        "--format-version",
        "1",
        *args,
    ]
    if (
        not isinstance(argv, list)
        or len(argv) != 1 + len(expected_args)
        or argv[1:] != expected_args
    ):
        raise ValueError("Cargo artifact metadata query differs from command selection")
    package_probes = context.get("package_probes")
    if not isinstance(package_probes, list) or len(package_probes) != len(packages):
        raise ValueError("Cargo selected package query custody is missing")
    package_ids = {}
    for package, package_probe in zip(packages, package_probes, strict=True):
        if (
            not isinstance(package_probe, Mapping)
            or package_probe.get("argv")
            != [argv[0], "pkgid", "--offline", "--locked", "--package", package, *args]
            or package_probe.get("phase") != "cargo-context-pkgid"
            or package_probe.get("unit") != "target"
            or package_probe.get("returncode") != 0
            or package_probe.get("cwd") != cwd
            or package_probe.get("compiler_cwd") != cwd
            or not isinstance(package_probe.get("stdout"), str)
        ):
            raise ValueError(
                "Cargo selected package query differs from command selection"
            )
        package_ids[package] = package_probe["stdout"].strip()
    try:
        expected = _cargo_context_from_metadata(
            json.loads(raw),
            command,
            cwd=Path(cwd),
            package_ids=package_ids,
            require_crate_types=True,
        )
    except (KeyError, TypeError) as exc:
        raise ValueError("Cargo artifact metadata is malformed") from exc
    files = context.get("input_files")
    if not isinstance(files, list):
        raise ValueError("Cargo artifact manifest/source custody is missing")
    captured = {}
    for row in files:
        if (
            not isinstance(row, Mapping)
            or set(row) != {"path", "sha256", "size"}
            or not isinstance(row["path"], str)
            or not Path(row["path"]).is_absolute()
            or type(row["size"]) is not int
            or row["size"] < 0
            or not isinstance(row["sha256"], str)
            or re.fullmatch(r"[0-9a-f]{64}", row["sha256"]) is None
            or row["path"] in captured
        ):
            raise ValueError("Cargo artifact manifest/source identity is malformed")
        captured[row["path"]] = row
    paths = {str(Path(expected["workspace_root"]) / "Cargo.toml")}
    for source in expected["sources"]:
        paths.update((source["manifest_path"], source["source"]))
        if source["manifest_path"] not in captured:
            raise ValueError("Cargo artifact context has no captured manifest")
        source["manifest"] = captured[source["manifest_path"]]
    if set(captured) != paths:
        raise ValueError(
            "Cargo artifact manifest/source membership differs from selection"
        )
    sources = context.get("sources")
    if (
        not isinstance(sources, list)
        or len(sources) != 1
        or not isinstance(sources[0], Mapping)
        or "manifest" not in sources[0]
    ):
        raise ValueError("Cargo artifact context has no captured manifest")
    if any(
        context.get(key) != expected[key]
        for key in ("sources", "compiler_cwd", "workspace_root")
    ):
        raise ValueError(
            "Cargo artifact selected target differs from retained metadata"
        )
    from tools.proof_queue_pkg.command_admission import _rust_crate_types

    return _rust_crate_types(",".join(expected["sources"][0]["crate_types"]))


def revalidate_rust_selection_sources(
    selection: Mapping[str, object], *, _verified=None
) -> None:
    """Reuse checks finite inputs; armed custody checks these frozen files."""
    verified = {} if _verified is None else _verified

    def unchanged(path, digest, size, label):
        key = _image_path_key(Path(path))
        if key not in verified:
            actual = stable_regular_file_identity(Path(path), label=label)
            verified[key] = (actual.sha256, actual.size)
        return verified[key] == (digest, size)

    if selection.get("schema") == RUNTIME_GATE_RUST_SELECTION_SCHEMA:
        for operation in cast(Sequence[Mapping[str, Any]], selection["operations"]):
            revalidate_rust_selection_sources(
                operation["selection"], _verified=verified
            )
            projection = operation["projection"]
            if projection is not None:
                receipt = projection["sdk"]["receipt"]
                if not unchanged(
                    receipt["path"],
                    receipt["sha256"],
                    receipt["size_bytes"],
                    "managed WASI receipt",
                ):
                    raise ValueError("managed WASI SDK changed since Cargo selection")
        return
    for unit in selection["units"]:
        startup = unit.get("startup")
        if startup is not None:
            member = startup["member"]
            if not unchanged(
                member["path"], member["sha256"], member["size"], "WASI command CRT"
            ):
                raise ValueError("WASI command startup changed since selection")
        flags = unit.get("flag_context")
        contexts = (
            []
            if flags is None
            else [flags, *([flags["documenter"]] if "documenter" in flags else [])]
        )
        for flag_context in contexts:
            for source in flag_context["sources"]:
                if not unchanged(
                    source["path"],
                    source["sha256"],
                    source["size"],
                    "Cargo flag source " + source["label"],
                ):
                    raise ValueError(
                        f"Cargo flag source changed: {source['label']} ({source['path']})"
                    )
        context = unit.get("artifact_context")
        if context is None:
            continue
        _cargo_artifact_crate_types(context, selection["producer_command"])
        for expected in context["input_files"]:
            if not unchanged(
                expected["path"],
                expected["sha256"],
                expected["size"],
                "Cargo artifact input",
            ):
                raise ValueError(
                    "Cargo artifact manifest/source changed since selection"
                )


def _managed_wasi_c_command(command, abi):
    """Expose cc-rs's pinned Clang target/sysroot before querying its phases."""
    from molt.cli.compiler_target import (
        compiler_argument_spans,
        validate_compiler_target,
    )
    from molt.wasi_sdk_identity import WASI_C_ABI_TARGET

    # cc 1.6.0 adds these defaults before CFLAGS (lib.rs2599,2777). Other
    # optimization/default flags cannot add compiler images or resource roots.
    validate_compiler_target(command, WASI_C_ABI_TARGET)
    for span in compiler_argument_spans(command[1:]):
        if span.context not in {"driver", "cc1"}:
            continue
        sysroot = None
        if span.option in {"--sysroot", "-isysroot"}:
            sysroot = span.arguments[1]
        elif span.option.startswith("--sysroot="):
            sysroot = span.option.partition("=")[2]
        if sysroot is not None and sysroot != str(abi.sysroot):
            raise ValueError("managed C flags select another WASI sysroot")
    return [
        command[0],
        f"--sysroot={abi.sysroot}",
        f"--target={WASI_C_ABI_TARGET}",
        *command[1:],
    ]


def select_cargo_native_units(
    *,
    required: Mapping[str, Sequence[str]] | None,
    target: str | None,
    host: str,
    cwd: Path,
    env: Mapping[str, str],
    managed_wasi_sdk: Mapping[str, object] | None = None,
) -> list[dict[str, object]]:
    """Resolve declared role/language pairs without compiler probes or hashing."""
    from tools.proof_plan import cargo_native_units
    from molt.cli.runtime_cargo_plan import (
        _c_tool_environment_names,
        _resolve_c_build_resources,
        runtime_c_tool_selection,
    )

    requirements = cargo_native_units(
        {
            "cargo_native_units": {} if required is None else required,
            "toolchains": ["cargo", "rustc"],
        }
    )
    by_target: dict[str, dict[str, list[str]]] = {}
    for unit, languages in requirements.items():
        triple = (target or host) if unit == "target" else host
        if triple.startswith("wasm") and managed_wasi_sdk is None:
            raise ValueError("managed WASI tools are not native C/C++ build units")
        by_target.setdefault(triple, {})[unit] = languages

    selected_env = ProcessEnvironmentBuilder(env)
    result: list[dict[str, object]] = []
    for triple, units in by_target.items():
        sdk_root = None
        expected_abi = None
        sdk = None
        if triple.startswith("wasm"):
            from molt.wasi_sdk_identity import (
                WASI_C_ABI_TARGET,
                WasiCAbiProjection,
                wasi_c_abi_projection,
            )

            sdk = validate_wasi_sdk_selection(managed_wasi_sdk)
            generation = sdk["generation"]
            sdk_root = Path(sdk["sdk"])
            expected_abi = WasiCAbiProjection.from_facts(
                sdk_root,
                sdk_version=generation["asset"]["sdk_version"],
                llvm_version=generation["asset"]["llvm_version"],
                tree_sha256=generation["tree"]["sha256"],
                facts=generation["facts"],
            )
            if (
                triple != WASI_C_ABI_TARGET
                or wasi_c_abi_projection(env) != expected_abi
            ):
                raise ValueError("managed C units differ from the admitted WASI C ABI")
        languages = [
            language
            for language in ("c", "c++")
            if any(language in values for values in units.values())
        ]
        tools = {}
        for role in [
            *("cc" if language == "c" else "cxx" for language in languages),
            "ar",
        ]:
            names = _c_tool_environment_names(role, target=triple, host_target=host)
            selected = next(
                (selected_env[name] for name in names if selected_env.get(name)), None
            )
            if (
                selected is not None
                and not Path(selected).is_absolute()
                and any(sep in selected for sep in ("/", "\\"))
            ):
                raise ValueError(
                    "native compiler selector has unresolved build-script cwd"
                )
            path = runtime_c_tool_selection(
                role, root=cwd, env=selected_env, target=triple, host_target=host
            )
            if path is None:
                raise ValueError(
                    f"native build unit {triple} requires an explicit {role}"
                )
            tools[role] = str(path)
            if sdk_root is not None and sdk is not None:
                sdk_role = {"cc": "clang", "cxx": "clang++", "ar": "llvm-ar"}[role]
                expected_path = (
                    sdk_root / sdk["generation"]["facts"]["tools"][sdk_role]["path"]
                )
                if path != expected_path:
                    raise ValueError(
                        "managed WASI C unit selects a foreign compiler tool"
                    )
        # The canonical Cargo resource grammar owns flags and search inputs for
        # both languages; shared roots have one armed inventory per target.
        resources = _resolve_c_build_resources(
            selected_env, target=triple, host_target=host
        )
        c_flag_rule = FlagStringRule.cc(selected_env)
        compilers = {}
        for language in languages:
            role, flag_role = (
                ("cc", "cflags") if language == "c" else ("cxx", "cxxflags")
            )
            flags = [
                token
                for name in reversed(
                    _c_tool_environment_names(
                        flag_role, target=triple, host_target=host
                    )
                )
                for token in c_flag_rule.decode(selected_env.get(name, ""))
            ]
            command = [tools[role], *flags]
            compilers[language] = (
                command
                if expected_abi is None
                else _managed_wasi_c_command(command, expected_abi)
            )
        if selected_env.get("CC_KNOWN_WRAPPER_CUSTOM") or Path(
            selected_env.get("RUSTC_WRAPPER", "")
        ).stem in {
            "sccache",
            "ccache",
            "cachepot",
            "buildcache",
            "kache",
            "distcc",
            "icecc",
        }:
            raise ValueError(
                "native compiler wrapper requires executable child custody"
            )
        result.append(
            {
                "units": units,
                "target": triple,
                "compilers": compilers,
                "archiver": tools["ar"],
                "resource_roots": sorted(
                    {
                        str(root.path)
                        for root in resources.roots
                        if sdk_root is None or not root.path.is_relative_to(sdk_root)
                    }
                ),
            }
        )
    return result


def native_build_environment(selection: Mapping[str, object]) -> dict[str, str]:
    """Publish captured drivers for each language without reselecting tools."""
    updates: dict[str, str] = {}
    for row in selection.get("native_build", []):
        selected = row["selection"]
        triple = selected["target"]
        tools = {"AR": selected["archiver"]}
        for language, command in selected["compilers"].items():
            tools["CC" if language == "c" else "CXX"] = command[0]
        for role, value in tools.items():
            for form in (triple, triple.replace("-", "_").replace(".", "_")):
                updates[f"{role}_{form}"] = value
    return updates


def rust_native_selection_is_current(selection, *, cwd, env):
    """Reselect ordinary C units; managed units retain their admitted SDK plan."""
    if selection.get("schema") != RUNTIME_GATE_RUST_SELECTION_SCHEMA:
        current = select_cargo_native_units(
            required=selection["native_required"],
            target=selection["target"],
            host=selection["compiler_host"],
            cwd=cwd,
            env=env,
        )
    else:
        current = []
        managed_targets = set()
        for operation in selection["operations"]:
            child = operation["selection"]
            if operation["c_units"] == "managed-wasi":
                managed_targets.add(child["target"])
            elif operation["c_units"] == "native":
                current.extend(
                    select_cargo_native_units(
                        required=operation["native_required"],
                        target=child["target"],
                        host=selection["compiler_host"],
                        cwd=cwd,
                        env=env,
                    )
                )
        current.extend(
            row["selection"]
            for row in selection["native_build"]
            if row["selection"]["target"] in managed_targets
        )
    expected = [row["selection"] for row in selection["native_build"]]
    return sorted(current, key=canonical_json_sha256) == sorted(
        expected, key=canonical_json_sha256
    )


def _native_compiler_role(language: str, target: str) -> str:
    return "rust-build-native-" + ("c" if language == "c" else "cxx") + "-" + target


def _compiler_phase_languages(language: str, target: str) -> tuple[str, ...]:
    # WebAssembly has no GNU assembly translation unit. Its native LLVM
    # provider still exposes the actual C/C++ frontend helper phase.
    return (
        (language,) if target.startswith("wasm") else (language, "assembler-with-cpp")
    )


def llvm_sdk_tool_images(selection: Mapping[str, object]) -> list[dict[str, object]]:
    images = []
    for role, value in {**selection["tools"], **selection["selectors"]}.items():
        path = Path(value)
        images.append(capture_image("llvm-sdk-" + role, path, preserve_path=True))
        if custody_path(path.resolve(strict=True)) != custody_path(path):
            images.append(capture_image("llvm-sdk-" + role, path))
    return canonical_images(images)


def _llvm_sdk_link_commands(
    selection: Mapping[str, object], scratch: Path, closure: tuple[str, ...]
):
    from molt.llvm_toolchain import llvm_sdk_probe_command
    from molt.llvm_linker_roles import host_llvm_linker_role
    import platform

    prefix = Path(selection["prefix"])
    tools = {role: Path(value) for role, value in selection["tools"].items()}
    return (
        (
            "sdk-cpp-link",
            llvm_sdk_probe_command(
                prefix,
                tools["clang++"],
                tools[host_llvm_linker_role(platform.system())],
                closure,
                scratch / "sdk_probe.cpp",
                scratch / "sdk_probe",
            ),
        ),
        (
            "c-link",
            [
                str(tools["clang"]),
                "-std=c11",
                str(scratch / "main.c"),
                "-o",
                str(scratch / "main"),
            ],
        ),
    )


def capture_llvm_sdk_selection(
    selection: Mapping[str, object], *, cwd: Path, env: Mapping[str, str]
) -> tuple[list[dict[str, object]], dict[str, object]]:
    """Trace SDK verification and ordinary C final-link helpers without compiling."""
    env = ProcessEnvironment(env)
    from molt.llvm_toolchain import (
        _llvm_link_closure,
    )

    prefix = Path(selection["prefix"])
    tools = {role: Path(value) for role, value in selection["tools"].items()}
    closure, _libraries = _llvm_link_closure(prefix, tools["llvm-config"])
    probe_env = env.updated(cast(Mapping[str, str], selection["probe_environment"]))
    images = llvm_sdk_tool_images(selection)
    phases = []
    probes = []
    with tempfile.TemporaryDirectory(prefix="molt-proof-llvm-sdk-") as temporary:
        scratch = Path(temporary)
        cpp, source = scratch / "sdk_probe.cpp", scratch / "main.c"
        cpp.write_text("int main() { return 0; }\n", encoding="utf-8")
        source.write_text("int main(void) { return 0; }\n", encoding="utf-8")
        commands = _llvm_sdk_link_commands(selection, scratch, closure)
        for phase, command in commands:
            phase_env = probe_env if phase == "sdk-cpp-link" else env
            completed = _run_rust_link_probe(
                [command[0], "-###", *command[1:]],
                phase=phase,
                unit="llvm-sdk",
                cwd=cwd,
                compiler_cwd=cwd,
                env=phase_env,
                timeout=30.0,
                probes=probes,
            )
            reported = _driver_command_lines(completed.stdout + "\n" + completed.stderr)
            if completed.returncode != 0 or not reported:
                raise RustLinkCaptureError(
                    "LLVM SDK driver did not expose final-link helpers",
                    unit="llvm-sdk",
                    probes=probes,
                )
            helpers = []
            for argv in reported:
                path = Path(argv[0])
                if not path.is_absolute():
                    path = (
                        cwd / path
                        if any(sep in argv[0] for sep in ("/", "\\"))
                        else find_executable(argv[0], environment=phase_env)
                    )
                if path is None:
                    raise ValueError(f"LLVM SDK helper is unavailable: {argv[0]}")
                image = capture_image("llvm-sdk-" + phase, path, preserve_path=True)
                images.append(image)
                if custody_path(path.resolve(strict=True)) != custody_path(path):
                    images.append(capture_image("llvm-sdk-" + phase, path))
                helpers.append(
                    {"command": argv, "path": image["path"], "sha256": image["sha256"]}
                )
            phases.append({"phase": phase, "command": command, "helpers": helpers})
    return canonical_images(images), {
        **selection,
        "scratch": str(scratch),
        "link_closure": list(closure),
        "phases": phases,
        "probes": probes,
    }


def validate_llvm_sdk_selection(
    identity: Mapping[str, object],
    *,
    required: bool,
    expected: Mapping[str, object] | None = None,
) -> None:
    """Validate exact selected roles and transcript-bound helpers at every receiver."""
    from molt.llvm_toolchain import llvm_verification_tool_paths

    record = identity.get("llvm_sdk")
    if not required:
        if record is not None:
            raise ValueError("non-verifier toolchain has unexpected LLVM SDK closure")
        return
    if not isinstance(record, Mapping) or set(record) != {
        "prefix",
        "tools",
        "phases",
        "probes",
        "scratch",
        "link_closure",
        "selectors",
        "probe_environment",
    }:
        raise ValueError("LLVM SDK verification closure is missing or malformed")
    tools = record["tools"]
    if (
        not isinstance(tools, Mapping)
        or not isinstance(record["prefix"], str)
        or not Path(record["prefix"]).is_absolute()
    ):
        raise ValueError("LLVM SDK selection is malformed")
    if any(
        not isinstance(value, str) or not Path(value).is_absolute()
        for value in tools.values()
    ) or not isinstance(tools.get("llvm-config"), str):
        raise ValueError("LLVM SDK tool paths are malformed")
    canonical = {
        role: str(path)
        for role, path in llvm_verification_tool_paths(
            Path(record["prefix"]), Path(tools["llvm-config"])
        )
    }
    selected_path = identity.get("path")
    if (
        dict(tools) != canonical
        or not isinstance(selected_path, str)
        # The selection reports the OS spelling; the identity path carries
        # custody's, which lowercases a Windows drive anchor.
        or custody_path(Path(tools["llvm-config"])) != custody_path(Path(selected_path))
    ):
        raise ValueError("LLVM SDK tools differ from the verification selection")
    if expected is not None and {
        key: record[key]
        for key in ("prefix", "tools", "selectors", "probe_environment")
    } != dict(expected):
        raise ValueError("LLVM SDK selection has changed")
    selectors, overlay = record["selectors"], record["probe_environment"]
    if (
        not isinstance(selectors, Mapping)
        or not isinstance(overlay, Mapping)
        or (
            set(selectors) not in ({"xcrun"}, {"xcode-select", "xcrun"})
            if sys.platform == "darwin"
            else bool(selectors)
        )
        or set(overlay)
        != (
            {"DEVELOPER_DIR", "SDKROOT", "MACOSX_DEPLOYMENT_TARGET"}
            if sys.platform == "darwin"
            else set()
        )
    ):
        raise ValueError("LLVM SDK platform selection is malformed")
    if any(
        not isinstance(value, str) or not Path(value).is_absolute()
        for value in selectors.values()
    ) or any(not isinstance(value, str) or not value for value in overlay.values()):
        raise ValueError("LLVM SDK platform selection values are malformed")
    raw_images = identity.get("process_images")
    if not isinstance(raw_images, list):
        raise ValueError("LLVM SDK has no process images")
    images = canonical_images(raw_images)
    observed = {row["path"]: row for row in images}
    expected_images = set()
    for path in (*tools.values(), *selectors.values()):
        lexical, physical = (
            str(custody_path(Path(path))),
            str(custody_path(Path(path).resolve(strict=True))),
        )
        expected_images.update((lexical, physical))
        if (
            lexical not in observed
            or physical not in observed
            or observed[lexical]["sha256"] != observed[physical]["sha256"]
        ):
            raise ValueError("LLVM SDK selected tool image is missing or substituted")
    phases, probes = record["phases"], record["probes"]
    if (
        not isinstance(phases, list)
        or [row.get("phase") for row in phases if isinstance(row, Mapping)]
        != ["sdk-cpp-link", "c-link"]
        or not isinstance(probes, list)
        or len(probes) != 2
    ):
        raise ValueError("LLVM SDK final-link phases are incomplete")
    if (
        not isinstance(record["scratch"], str)
        or not Path(record["scratch"]).is_absolute()
        or not isinstance(record["link_closure"], list)
        or any(not isinstance(token, str) for token in record["link_closure"])
    ):
        raise ValueError("LLVM SDK probe inputs are malformed")
    commands = dict(
        _llvm_sdk_link_commands(
            record, Path(record["scratch"]), tuple(record["link_closure"])
        )
    )
    for phase, probe, driver in zip(phases, probes, ("clang++", "clang"), strict=True):
        command, helpers = phase.get("command"), phase.get("helpers")
        if (
            not isinstance(command, list)
            or not command
            or command != commands[phase["phase"]]
            or not isinstance(helpers, list)
            or not helpers
            or not isinstance(probe, Mapping)
        ):
            raise ValueError("LLVM SDK driver phase is malformed")
        if (
            probe.get("returncode") != 0
            or probe.get("phase") != phase["phase"]
            or probe.get("argv") != [command[0], "-###", *command[1:]]
        ):
            raise ValueError("LLVM SDK driver transcript differs from selection")
        reported = _driver_command_lines(
            str(probe.get("stdout", "")) + "\n" + str(probe.get("stderr", ""))
        )
        if [
            row.get("command") for row in helpers if isinstance(row, Mapping)
        ] != reported:
            raise ValueError("LLVM SDK helper closure differs from its transcript")
        for helper in helpers:
            path = helper.get("path")
            image = observed.get(path)
            if image is None or image["sha256"] != helper.get("sha256"):
                raise ValueError("LLVM SDK helper image is missing or substituted")
            invoked = Path(helper["command"][0])
            if invoked.is_absolute() and str(custody_path(invoked)) != path:
                raise ValueError("LLVM SDK helper differs from driver-selected path")
            physical_path = str(custody_path(Path(path).resolve(strict=True)))
            expected_images.update((path, physical_path))
            physical = observed.get(physical_path)
            if physical is None or physical["sha256"] != image["sha256"]:
                raise ValueError("LLVM SDK helper physical content is missing")

    if set(observed) != expected_images:
        raise ValueError("LLVM SDK closure contains undeclared executable images")
    material = dict(identity)
    digest = material.pop("identity_sha256", None)
    if digest != canonical_json_sha256(material):
        raise ValueError("LLVM SDK identity digest is invalid")


def capture_native_compiler_process_images(
    command: Sequence[str],
    *,
    role: str,
    language: str,
    target: str,
    cwd: Path,
    env: Mapping[str, str],
    captured_images: Sequence[Mapping[str, object]] = (),
) -> tuple[list[dict[str, object]], dict[str, object]]:
    """Capture actual source-to-object driver phases in operation-owned storage."""
    env = ProcessEnvironment(env)
    from molt.cli.native_link_plan import is_zig_compiler_command
    from molt.llvm_linker_roles import executable_entrypoint_name

    if not command or not Path(command[0]).is_absolute():
        raise ValueError("native compiler phase requires an absolute selected driver")
    name = executable_entrypoint_name(Path(command[0]))
    if name in {
        "ccache",
        "distcc",
        "sccache",
        "icecc",
        "cachepot",
        "buildcache",
        "kache",
    } or is_zig_compiler_command(command):
        raise ValueError("native compiler launcher requires executable child custody")
    captured: dict[str, dict[str, object]] = {}

    def image(path: Path) -> dict[str, object]:
        value = _image_path_key(path)
        if value not in captured:
            prior = next(
                (
                    row
                    for row in captured_images
                    if _image_path_key(Path(row["path"])) == value
                ),
                None,
            )
            captured[value] = (
                {**prior, "role": role, "path_kind": "selection"}
                if prior is not None
                else capture_image(role, path, preserve_path=True)
            )
        return captured[value]

    image(Path(command[0]))
    phases: list[dict[str, object]] = []
    probes: list[dict[str, object]] = []
    # MSVC cl's C frontend is in-process. Preserve the existing cl/lib policy;
    # GNU assembly driver tracing does not describe this compiler's grammar.
    if name in {"cl", "cl.exe"}:
        return list(captured.values()), {
            "command": list(command),
            "target": target,
            "language": language,
            "mode": "msvc-in-process",
            "phases": [],
            "probes": [],
        }
    with tempfile.TemporaryDirectory(prefix="molt-proof-native-compile-") as directory:
        root = Path(directory)
        for phase, suffix, source in (
            (language, ".c" if language == "c" else ".cc", "int molt_proof_unit;\n"),
            ("assembler-with-cpp", ".S", "/* empty assembler translation unit */\n"),
        ):
            if phase not in _compiler_phase_languages(language, target):
                continue
            path = root / ("unit" + suffix)
            path.write_text(source, encoding="utf-8")
            from molt.cli.compiler_target import source_extension_compiler_dialect

            dialect = source_extension_compiler_dialect(command)
            arguments = [
                dialect.forward("-x"),
                dialect.forward(phase),
                *(
                    ["/c", str(path), f"/Fo{root / 'unit.o'}"]
                    if dialect.value == "clang-cl"
                    else ["-c", str(path), "-o", str(root / "unit.o")]
                ),
            ]
            completed = _run_rust_link_probe(
                [*command, "-###", *arguments],
                phase="native-compile-" + phase,
                unit=role,
                cwd=root,
                compiler_cwd=root,
                env=env,
                timeout=30.0,
                probes=probes,
            )
            commands = _driver_command_lines(completed.stdout + "\n" + completed.stderr)
            if completed.returncode != 0 or not commands:
                raise RustLinkCaptureError(
                    "native compiler did not expose source-to-object helper commands",
                    unit=role,
                    probes=probes,
                )
            helpers = []
            for helper in commands:
                value = helper[0]
                selected = Path(value)
                if not selected.is_absolute():
                    if any(sep in value for sep in ("/", "\\")):
                        selected = root / selected
                    else:
                        selected = find_executable(value, environment=env)
                        if selected is None:
                            raise RustLinkCaptureError(
                                f"native compiler helper is unavailable: {value}",
                                unit=role,
                                probes=probes,
                            )
                try:
                    captured_image = image(selected)
                except (OSError, ValueError) as exc:
                    raise RustLinkCaptureError(
                        str(exc), unit=role, probes=probes
                    ) from exc
                helpers.append(
                    {
                        "command": helper,
                        "path": captured_image["path"],
                        "sha256": captured_image["sha256"],
                    }
                )
            phases.append({"language": phase, "helpers": helpers})
    return canonical_images(list(captured.values())), {
        "command": list(command),
        "target": target,
        "language": language,
        "mode": "driver-phases",
        "phases": phases,
        "probes": probes,
    }


def validate_native_compiler_capture(
    record: object,
    images: Sequence[Mapping[str, object]],
    *,
    command: Sequence[str],
    language: str,
    target: str,
    role: str,
) -> None:
    """Validate exact recorded phase/image membership without executing a driver."""
    if (
        not isinstance(command, (list, tuple))
        or not command
        or any(not isinstance(value, str) or not value for value in command)
    ):
        raise ValueError("native compiler command is malformed")
    if not isinstance(record, Mapping) or set(record) != {
        "command",
        "target",
        "language",
        "mode",
        "phases",
        "probes",
    }:
        raise ValueError("native compiler phase capture is malformed")
    if (
        record["command"] != list(command)
        or record["target"] != target
        or record["language"] != language
    ):
        raise ValueError("native compiler phase selection differs from its operation")
    phases = record["phases"]
    from molt.llvm_linker_roles import executable_entrypoint_name

    msvc = executable_entrypoint_name(Path(command[0])) in {"cl", "cl.exe"}
    if record["mode"] != (
        "msvc-in-process" if msvc else "driver-phases"
    ) or not isinstance(phases, list):
        raise ValueError("native compiler phase mode differs from its driver")
    if [row.get("language") for row in phases if isinstance(row, Mapping)] != (
        [] if msvc else list(_compiler_phase_languages(language, target))
    ):
        raise ValueError("native compiler phase capture is incomplete")
    probes = record["probes"]
    if (
        not isinstance(probes, list)
        or len(probes) != len(phases)
        or any(
            not isinstance(probe, Mapping) or probe.get("returncode") != 0
            for probe in probes
        )
    ):
        raise ValueError("native compiler phase transcripts are incomplete")
    if not Path(command[0]).is_absolute():
        raise ValueError("native compiler command is not absolute")
    expected = {_image_path_key(Path(command[0]))}
    observed = {
        _image_path_key(Path(row["path"])): row for row in images if row["role"] == role
    }
    for phase, probe in zip(phases, probes, strict=True):
        helpers = phase.get("helpers")
        if not isinstance(helpers, list) or not helpers:
            raise ValueError("native compiler phase has no helper custody")
        reported = _driver_command_lines(
            str(probe.get("stdout", "")) + "\n" + str(probe.get("stderr", ""))
        )
        if (
            probe.get("phase") != "native-compile-" + phase["language"]
            or not isinstance(probe.get("argv"), list)
            or probe["argv"][: len(command)] != list(command)
            or [
                helper.get("command")
                for helper in helpers
                if isinstance(helper, Mapping)
            ]
            != reported
        ):
            raise ValueError(
                "native compiler helpers differ from driver phase transcript"
            )
        for helper in helpers:
            if (
                not isinstance(helper, Mapping)
                or set(helper) != {"command", "path", "sha256"}
                or not isinstance(helper["command"], list)
                or not helper["command"]
                or any(
                    not isinstance(value, str) or not value
                    for value in helper["command"]
                )
                or not isinstance(helper["path"], str)
                or not Path(helper["path"]).is_absolute()
                or _image_path_key(Path(helper["path"])) not in observed
                or observed[_image_path_key(Path(helper["path"]))]["sha256"]
                != helper["sha256"]
            ):
                raise _image_membership_error(
                    "native compiler phase helper differs from captured images",
                    unit=phase.get("language"),
                    role=role,
                    selected=helper.get("path")
                    if isinstance(helper, Mapping)
                    else None,
                    images=((image["role"], image["path"]) for image in images),
                )
            invoked = helper["command"][0]
            if Path(invoked).is_absolute() and _image_path_key(
                Path(invoked)
            ) != _image_path_key(Path(helper["path"])):
                raise _image_membership_error(
                    "native compiler helper binding differs from its exact command",
                    unit=phase.get("language"),
                    role=role,
                    selected=invoked,
                    content=helper["path"],
                    images=((image["role"], image["path"]) for image in images),
                )
            expected.add(_image_path_key(Path(helper["path"])))
    if set(observed) != expected:
        raise _image_membership_error(
            "native compiler phase image membership is incomplete",
            unit=language,
            role=role,
            selected=command[0],
            images=((image["role"], image["path"]) for image in images),
        )


def native_compiler_selection_is_current(
    record: Mapping[str, object], *, env: Mapping[str, str]
) -> bool:
    for phase in record["phases"]:
        for helper in phase["helpers"]:
            value = helper["command"][0]
            if not Path(value).is_absolute():
                selected = find_executable(value, environment=env)
                if selected is None or _image_path_key(selected) != _image_path_key(
                    Path(helper["path"])
                ):
                    return False
    return True


def _proof_rust_probe_arguments(
    arguments: tuple[str, ...], root: Path, *, documenter: bool = False
) -> tuple[str, ...]:
    """Apply development-proof capability policy before shared output confinement."""
    from molt.rust_toolchain import rust_flag_spans
    from molt.cli.cargo_target_cfg import rust_probe_arguments

    if any(
        span.option == "-Z"
        for span in rust_flag_spans(arguments, documenter=documenter)
    ):
        raise ValueError(
            "unstable Rust probe selectors require an explicit capability authority"
        )
    return rust_probe_arguments(arguments, root, documenter=documenter)


def _cargo_flag_context(
    *,
    command: Sequence[str],
    cwd: Path,
    env: Mapping[str, str],
    target: str,
    host_unit: bool,
    rustc: Path,
    probes: list[dict[str, object]],
    flag_kind: str = "rustflags",
    compiler_context: Mapping[str, object] | None = None,
    capture_inputs=None,
) -> tuple[dict[str, object], ProcessEnvironment]:
    from molt.cli.runtime_cargo_plan import (
        capture_cargo_configuration_inputs,
        cargo_selected_value,
        _tool_path,
    )
    from molt.cli.rustc_probes import rust_probe_command
    from molt.cli.cargo_target_cfg import (
        cargo_target_query_arguments,
        parse_rustc_target_metadata,
    )

    environment = ProcessEnvironment(env)
    key = (
        str(cwd),
        tuple(command),
        environment.rule,
        tuple(sorted(environment.items())),
    )
    cached = None if capture_inputs is None else capture_inputs.get(key)
    if cached is None:
        (
            _root,
            configured_env,
            paths,
            inputs,
            configuration,
            profiles,
            command_target,
            cli,
            _forced,
            _resources,
        ) = capture_cargo_configuration_inputs(
            cwd, env=environment, cargo_command=command
        )
        sources = []
        for item in inputs:
            raw = item.identity.path.read_bytes()
            verify_stable_regular_file_identity(item.identity, label=item.label)
            if hashlib.sha256(raw).hexdigest() != item.identity.sha256:
                raise ValueError("Cargo flag source changed during capture")
            sources.append(
                {
                    "label": item.label,
                    "path": str(item.identity.path),
                    "text": raw.decode("utf-8"),
                    "sha256": item.identity.sha256,
                    "size": item.identity.size,
                }
            )
        cached = (
            (
                _root,
                configured_env,
                paths,
                inputs,
                configuration,
                profiles,
                command_target,
                cli,
                _forced,
                _resources,
            ),
            sources,
        )
        if capture_inputs is not None:
            capture_inputs[key] = cached
    else:
        for item in cached[0][3]:
            verify_stable_regular_file_identity(item.identity, label=item.label)
    (
        (
            _root,
            configured_env,
            paths,
            inputs,
            configuration,
            profiles,
            command_target,
            cli,
            _forced,
            _resources,
        ),
        sources,
    ) = cached
    wrappers = {}
    for variable, key in (
        ("RUSTC_WRAPPER", "rustc-wrapper"),
        ("RUSTC_WORKSPACE_WRAPPER", "rustc-workspace-wrapper"),
    ):
        value = cargo_selected_value(
            configuration,
            cli,
            configured_env,
            ("build", key),
            (variable, "CARGO_BUILD_" + variable),
            "",
        )
        if value:
            wrappers[variable] = _tool_path(
                value, root=cwd, env=configured_env, role=variable
            )
    context = {
        "schema": "molt.proof-cargo-flag-inputs.v1",
        "command": list(command),
        "cwd": str(cwd),
        "target": target,
        "rustc": str(rustc),
        "host_unit": host_unit,
        "flag_kind": flag_kind,
        "configuration_paths": [str(path) for path in paths],
        "sources": sources,
        "environment": {
            name: value
            for name, value in (
                (environment.rule.key(name), value)
                for name, value in environment.items()
            )
            if name.endswith(("RUSTFLAGS", "RUSTDOCFLAGS"))
            or name.startswith(("CARGO_PROFILE_", "CARGO_UNSTABLE_"))
            or name
            in {
                "CARGO_BUILD_TARGET",
                "RUSTC_BOOTSTRAP",
                "__CARGO_TEST_CHANNEL_OVERRIDE_DO_NOT_USE_THIS",
            }
        },
        "cfg_probes": [],
        "forced_environment": list(_forced),
    }

    reused_probes = iter(
        () if compiler_context is None else compiler_context["cfg_probes"]
    )
    if compiler_context is not None and any(
        context[key] != compiler_context[key]
        for key in (
            "command",
            "cwd",
            "target",
            "rustc",
            "sources",
            "environment",
            "configuration_paths",
        )
    ):
        raise ValueError("documenter cfg source differs from compiler custody")

    def probe(flags: tuple[str, ...]):
        if compiler_context is not None:
            row = next(reused_probes, None)
            if row is None or row["flags"] != list(flags):
                raise ValueError("documenter cfg differs from compiler witness")
            context["cfg_probes"].append(dict(row))
            return parse_rustc_target_metadata(row["stdout"], row["stderr"]).cfg
        probe = rust_probe_command(rustc, wrappers, label="proof Cargo cfg")
        with (
            tempfile.TemporaryDirectory(prefix="molt-rust-cfg-") as scratch,
            probe.fenced() as compiler_command,
        ):
            root = Path(scratch)
            (root / "output").mkdir()
            safe = _proof_rust_probe_arguments(flags, root)
            argv = [*compiler_command, *cargo_target_query_arguments(target, safe)]
            query_env = ProcessEnvironment(configured_env).updated(
                {
                    "CARGO_ENCODED_RUSTFLAGS": FlagStringRule.CARGO_ENCODED.encode(
                        safe, variable="CARGO_ENCODED_RUSTFLAGS"
                    )
                }
            )
            result = _run_rust_link_probe(
                argv,
                phase="flag-cfg",
                unit="host-proc-macro" if host_unit else "target",
                cwd=cwd,
                compiler_cwd=cwd,
                env=query_env,
                timeout=30.0,
                probes=probes,
                input="",
            )
            if result.returncode:
                raise ValueError("Cargo flag cfg metadata probe failed")
            context["cfg_probes"].append(
                {"flags": list(flags), "stdout": result.stdout, "stderr": result.stderr}
            )
            return parse_rustc_target_metadata(result.stdout, result.stderr).cfg

    selected = _resolve_cargo_flag_context(context, probe=probe)
    with tempfile.TemporaryDirectory(prefix="molt-rust-flag-check-") as scratch:
        _proof_rust_probe_arguments(
            selected, Path(scratch), documenter=flag_kind == "rustdocflags"
        )
    context["selected_flags"] = list(selected)
    return context, ProcessEnvironment(configured_env, rule=environment.rule)


def _resolve_cargo_flag_context(
    context: Mapping[str, object], *, probe=None, selected_target=None
) -> tuple[str, ...]:
    """Replay the shared Cargo input authority from source bytes, never live files."""
    from molt.cli.runtime_cargo_plan import (
        CargoConfigurationSource,
        capture_cargo_configuration_inputs,
        cargo_configuration_document,
        cargo_flag_inputs,
        cargo_cfg_flags,
        cargo_requested_target,
    )
    from molt.cli.cargo_target_cfg import (
        parse_rustc_target_metadata,
        select_cargo_target_flags,
    )

    if context.get("schema") != "molt.proof-cargo-flag-inputs.v1":
        raise ValueError("Cargo flag input schema mismatch")
    required = {
        "schema",
        "command",
        "cwd",
        "target",
        "rustc",
        "host_unit",
        "flag_kind",
        "configuration_paths",
        "sources",
        "environment",
        "cfg_probes",
        "forced_environment",
    }
    if probe is None:
        required.add("selected_flags")
    if not required <= set(context) or set(context) - required - {
        "selected_flags",
        "documenter",
    }:
        raise ValueError("Cargo flag input fields differ from their schema")
    if (
        type(context["host_unit"]) is not bool
        or context["flag_kind"] not in {"rustflags", "rustdocflags"}
        or not isinstance(context["command"], list)
        or not context["command"]
        or any(not isinstance(token, str) or not token for token in context["command"])
        or not isinstance(context["environment"], Mapping)
        or any(
            not isinstance(key, str) or not isinstance(value, str)
            for key, value in context["environment"].items()
        )
        or any(
            not isinstance(context[key], list)
            for key in (
                "sources",
                "configuration_paths",
                "cfg_probes",
                "forced_environment",
            )
        )
        or any(
            not isinstance(context[key], str) or not context[key]
            for key in ("cwd", "target", "rustc")
        )
        or not Path(context["rustc"]).is_absolute()
    ):
        raise ValueError("Cargo flag input coordinates are malformed")
    sources = {}
    for row in context["sources"]:
        if (
            not isinstance(row, Mapping)
            or set(row) != {"text", "size", "sha256", "path", "label"}
            or any(
                not isinstance(row[key], str)
                for key in ("text", "sha256", "path", "label")
            )
            or type(row["size"]) is not int
        ):
            raise ValueError("Cargo flag source custody is malformed")
        raw = row["text"].encode("utf-8")
        if len(raw) != row["size"] or hashlib.sha256(raw).hexdigest() != row["sha256"]:
            raise ValueError("Cargo flag source bytes differ from their custody")
        path = Path(row["path"])
        if not path.is_absolute() or row["label"] in sources:
            raise ValueError("Cargo flag source path is not unique and absolute")
        sources[row["label"]] = CargoConfigurationSource(
            row["label"], path, cargo_configuration_document(raw, path)
        )
    cwd = Path(context["cwd"])
    if not cwd.is_absolute():
        raise ValueError("Cargo flag input cwd is not absolute")
    if any(
        not isinstance(path, str) or not Path(path).is_absolute()
        for path in context["configuration_paths"]
    ):
        raise ValueError("Cargo flag configuration paths are malformed")
    for row in context["cfg_probes"]:
        if (
            not isinstance(row, Mapping)
            or set(row) != {"flags", "stdout", "stderr"}
            or not isinstance(row["flags"], list)
            or any(not isinstance(flag, str) for flag in row["flags"])
            or any(not isinstance(row[key], str) for key in ("stdout", "stderr"))
        ):
            raise ValueError("Cargo flag cfg witness is malformed")
    (
        _root,
        env,
        _paths,
        used,
        configuration,
        profiles,
        command_target,
        cli,
        _forced,
        _resources,
    ) = capture_cargo_configuration_inputs(
        cwd,
        env=context["environment"],
        cargo_command=context["command"],
        captured_sources=sources,
        captured_paths=[Path(path) for path in context["configuration_paths"]],
    )
    if {item.label for item in used} != set(sources):
        raise ValueError("Cargo flag context contains unused source custody")
    if context.get("forced_environment") != list(_forced):
        raise ValueError("Cargo forced environment differs from source inputs")

    def unstable_profile(value):
        return isinstance(value, Mapping) and any(
            key in {"rustflags", "codegen-backend"} or unstable_profile(child)
            for key, child in value.items()
        )

    before = (
        context["command"][: context["command"].index("--")]
        if "--" in context["command"]
        else context["command"]
    )
    if (
        any(token == "-Z" or token.startswith("-Z") for token in before)
        or unstable_profile(profiles.get("profile", {}))
        or unstable_profile(cli.get("profile", {}))
    ):
        raise ValueError(
            "unstable Cargo compiler/profile selectors require an explicit capability authority"
        )
    for document in (configuration, cli):
        unstable = document.get("unstable", {})
        if any(
            unstable.get(key)
            for key in (
                "profile-rustflags",
                "host-config",
                "target-applies-to-host",
                "build-std",
                "per-package-target",
            )
        ):
            raise ValueError(
                "unstable Cargo flag selection requires an explicit capability authority"
            )
    if any(
        name.startswith("CARGO_UNSTABLE_") and value not in {"", "0", "false"}
        for name, value in env.items()
    ):
        raise ValueError(
            "unstable Cargo environment selection requires an explicit capability authority"
        )
    _require_stable_cargo_environment(env)
    if selected_target is not None:
        selected_target.append(
            cargo_requested_target(configuration, cli, env, command_target)
        )
    explicit_target = (
        command_target is not None
        or configuration.get("build", {}).get("target") is not None
        or cli.get("build", {}).get("target") is not None
        or "CARGO_BUILD_TARGET" in env
    )
    if context["host_unit"] and explicit_target:
        selected = ()
        if context["cfg_probes"]:
            raise ValueError(
                "cross Cargo host flags must not inherit target cfg probes"
            )
    else:
        tables, target_flags, build_flags, environment_flags = cargo_flag_inputs(
            configuration, cli, env, context["target"], flag_kind=context["flag_kind"]
        )
        records = iter(context["cfg_probes"])

        def replay(flags):
            _proof_rust_probe_arguments(flags, Path(context["cwd"]))
            row = next(records, None)
            if row is None or row["flags"] != list(flags):
                raise ValueError(
                    "Cargo cfg probe differs from its selected flag inputs"
                )
            return parse_rustc_target_metadata(row["stdout"], row["stderr"]).cfg

        selected_probe = probe if probe is not None else replay
        cfg_facts = None
        if context["flag_kind"] == "rustdocflags" and tables:
            # Cargo TargetInfo selects rustdocflags once from the final rustc
            # cfg. Rustdoc-only options must never reach the compiler query.
            rust_tables, rust_target, rust_build, rust_environment = cargo_flag_inputs(
                configuration, cli, env, context["target"]
            )

            def compiler_probe(flags):
                nonlocal cfg_facts
                cfg_facts = selected_probe(flags)
                return cfg_facts

            select_cargo_target_flags(
                rust_tables,
                target_flags=rust_target,
                build_flags=rust_build,
                environment_flags=rust_environment,
                flags=cargo_cfg_flags,
                probe=compiler_probe,
            )
        selected, _matched = select_cargo_target_flags(
            tables,
            target_flags=target_flags,
            build_flags=build_flags,
            environment_flags=environment_flags,
            flags=cargo_cfg_flags,
            probe=selected_probe,
            flag_kind=context["flag_kind"],
            cfg_facts=cfg_facts,
        )
        if probe is None and next(records, None) is not None:
            raise ValueError("Cargo flag context has unused cfg probes")
    _proof_rust_probe_arguments(
        selected,
        Path(context["cwd"]),
        documenter=context["flag_kind"] == "rustdocflags",
    )
    if "selected_flags" in context and context["selected_flags"] != list(selected):
        raise ValueError("Cargo selected flags differ from source inputs")
    return selected


def _require_stable_cargo_environment(env: Mapping[str, str]) -> None:
    for name in ("RUSTC_BOOTSTRAP", "__CARGO_TEST_CHANNEL_OVERRIDE_DO_NOT_USE_THIS"):
        if env.get(name) not in {None, "", "0"}:
            raise ValueError(
                f"{name} requires an explicit unstable Cargo capability authority"
            )


def _require_stable_cargo_version(version: object) -> None:
    if (
        not isinstance(version, str)
        or re.search(r"(?m)^release: [0-9]+\.[0-9]+\.[0-9]+$", version) is None
    ):
        raise ValueError(
            "Rust operation projection requires a captured stable Cargo release; nightly/compiler-profile selectors require an explicit capability authority"
        )


RUNTIME_GATE_RUST_SELECTION_SCHEMA = "molt.proof-runtime-gate-rust-selection.v2"


def toolchain_capture_environment(
    policy,
    *,
    environment: Mapping[str, str],
    cwd: Path,
    cargo_command: Sequence[str] = ("cargo",),
) -> ProcessEnvironment:
    """Build generators observe Cargo's effective environment, including force."""
    environment = ProcessEnvironment(environment)
    if policy.data.get("compiler_tool_selector") != "build-python":
        return environment
    from molt.cli.runtime_cargo_plan import capture_cargo_configuration_inputs

    configured = capture_cargo_configuration_inputs(
        cwd, env=environment, cargo_command=cargo_command
    )[1]
    return ProcessEnvironment(configured, rule=environment.rule)


def bind_build_python_environment(identity, *, policy, env, cwd, cargo_command):
    """Project the isolated generator already held by queue file/image custody."""
    from molt.python_runtime_admission import (
        build_python_identity,
        resolve_build_python,
    )
    from tools import proof_plan

    selected = identity["python_runtime"]
    proof_plan.validate_selected_python_runtime(policy, selected)
    entrypoint = Path(selected["launch_argv"][0])
    images = [
        image
        for image in identity["process_images"]
        if image["role"] == policy.name + "-selected-interpreter"
    ]
    if len(images) != 1 or images[0]["path"] != str(entrypoint):
        raise ValueError("build Python signature lacks its admitted executable image")
    image = images[0]
    material = build_python_identity(
        entrypoint=entrypoint,
        content_path=Path(
            selected["startup_selection"]["selected_executable"]["resolved"]
        ),
        size=image["size_bytes"],
        sha256=image["sha256"],
        runtime=selected["runtime"],
    )
    signature = canonical_json_sha256(material)
    previous = env.get("MOLT_BUILD_PYTHON_SIGNATURE")
    if previous is not None and previous != signature:
        raise ValueError("build Python signature differs from its isolated capture")
    bound = ProcessEnvironment(env).updated(
        {
            "MOLT_BUILD_PYTHON": str(entrypoint),
            "MOLT_BUILD_PYTHON_SIGNATURE": signature,
        }
    )
    effective = toolchain_capture_environment(
        policy,
        environment=bound,
        cwd=cwd,
        cargo_command=cargo_command,
    )
    if (
        resolve_build_python(effective, cwd=cwd) != entrypoint
        or effective.get("MOLT_BUILD_PYTHON_SIGNATURE") != signature
    ):
        raise ValueError("Cargo configuration overrides the admitted build Python")
    return bound


def resolve_runtime_gate_cargo_operation(operation, *, cwd, env, compiler_host):
    """Resolve the one declared runtime-CABI operation through its existing owner."""
    if operation.environment != "runtime-wasi":
        return list(operation.command), ProcessEnvironment(env), None
    from molt.cli.runtime_cargo_plan import resolve_runtime_cargo_plan
    from tools.runtime_test_gate_plan import WASI_TARGET

    plan = resolve_runtime_cargo_plan(
        cwd,
        env=env,
        cargo_command=operation.command,
        requested_target=WASI_TARGET,
        host_target=compiler_host,
        rustflags_transform=lambda flags: (*flags, "-C", "link-arg=--export-table"),
    )
    plan.verify()
    return list(plan.command), plan.environment, plan


def rust_link_selections(
    selection: Mapping[str, object],
) -> tuple[Mapping[str, Any], ...]:
    """Iterate the canonical scalar selections in a direct or planned operation."""
    if selection.get("schema") == RUNTIME_GATE_RUST_SELECTION_SCHEMA:
        return tuple(
            row["selection"]
            for row in cast(Sequence[Mapping[str, Any]], selection["operations"])
        )
    if selection.get("schema") != "molt.proof-rust-link-selection-telemetry.v5":
        raise ValueError("unknown Rust compilation selection schema")
    return (selection,)


def _runtime_gate_projection(plan, *, cwd, env):
    if plan is None:
        return None
    from molt.llvm_toolchain import capture_wasi_sdk_selection
    from molt.source_root import compiler_source_root

    sdk = capture_wasi_sdk_selection(root=compiler_source_root(), cwd=cwd, env=env)
    if plan.wasi_sdk is None or str(plan.wasi_sdk.sdk) != sdk["sdk"]:
        raise ValueError("runtime Cargo plan selected another managed SDK")
    return {
        "sdk": sdk,
        "c_abi": plan.wasi_c_abi.encode(),
        "c_environment": {
            key: list(value) for key, value in plan.c_environment.items()
        },
        "rustflags": list(plan.rustflags),
        "tools": {key: str(value) for key, value in plan.tools.items()},
        "wrappers": {key: str(value) for key, value in plan.wrappers.items()},
        "forced_environment": list(plan.forced_environment),
        "build_directory_base": str(plan.build_directory_base),
        "build_directory": str(plan.build_directory),
        "target_directory": (
            None if plan.target_directory is None else str(plan.target_directory)
        ),
    }


def _capture_runtime_gate_rust_operations(
    *,
    rustc,
    cargo,
    cwd,
    env,
    target,
    command_argv,
    admitted_command,
    native_units,
    linker_process_helpers,
    linker_build_tools,
    rustc_version,
):
    env = ProcessEnvironment(env)
    from tools.proof_queue_pkg import command_admission
    from tools.proof_queue_pkg.command_identity import _rust_target
    from tools.runtime_test_gate_plan import cargo_operations

    envelope = command_admission.envelope_for_command(admitted_command)
    args = command_admission.runtime_gate_arguments(envelope)
    if args is None or cargo is None:
        raise ValueError("runtime Cargo operations require their registered driver")
    # RuntimeCargoPlan pins the physical Cargo selected by rustup. Bind that
    # same component before constructing child operations, as for rustc.
    cargo = resolve_rustup_proxy(cargo, role="cargo", root=cwd, env=env)
    if rustc_version is None:
        result = _run_rust_link_probe(
            [str(rustc), "-vV"],
            phase="compiler-metadata",
            unit="compiler",
            cwd=cwd,
            compiler_cwd=cwd,
            env=env,
            timeout=30.0,
            probes=[],
        )
        if result.returncode:
            raise ValueError("runtime Cargo compiler metadata failed")
        rustc_version = result.stdout
    host = rustc_host(rustc_version)
    from tools.proof_plan import cargo_native_units

    declared = cargo_native_units(envelope)
    if dict(native_units or {}) != declared:
        raise ValueError("runtime native requirements differ from admission")
    operations, images, native_selections = [], [], []
    capture_cache = {}
    for operation in cargo_operations(
        args.policy, args.profile, cargo=str(cargo), compiler_host=host
    ):
        if not operation.admits_native_requirements(declared):
            raise ValueError(
                "runtime Cargo operation C declaration differs from its source plan"
            )
        command, operation_env, plan = resolve_runtime_gate_cargo_operation(
            operation, cwd=cwd, env=env, compiler_host=host
        )
        projection = _runtime_gate_projection(plan, cwd=cwd, env=operation_env)
        if projection is not None and not {"wasi-clang", "wasm-ld"} <= set(
            cast(Sequence[str], envelope["toolchains"])
        ):
            raise ValueError(
                "managed WASI C operation lacks its SDK toolchain declaration"
            )
        child_admitted = ["cargo", *command[1:]]
        child_envelope = command_admission.envelope_for_command(child_admitted)
        child_target = _rust_target(child_envelope, operation_env, cwd=cwd)
        child_images, selection = _capture_rust_compilation(
            rustc=rustc,
            cargo=cargo,
            cwd=cwd,
            env=operation_env,
            target=child_target,
            command_argv=command,
            admitted=child_admitted,
            declared={},
            cargo_invocation=command_admission.parse_cargo_invocation(child_admitted),
            native_units={},
            linker_process_helpers=linker_process_helpers,
            linker_build_tools=linker_build_tools,
            rustc_version=rustc_version,
            _capture_cache=capture_cache,
            _captured_images=images,
            wasi_command_abi=None if plan is None else plan.wasi_c_abi,
        )
        images.extend(child_images)
        requirements = declared
        selected_native = select_cargo_native_units(
            required=requirements,
            target=child_target,
            host=host,
            cwd=cwd,
            env=operation_env,
            managed_wasi_sdk=None if projection is None else projection["sdk"],
        )
        native_selections.extend((row, operation_env) for row in selected_native)
        operations.append(
            {
                "role": operation.role,
                "source_command": list(operation.command),
                "environment": operation.environment,
                "c_units": operation.c_units,
                "native_required": requirements,
                "projection": projection,
                "selection": selection,
                "process_image_refs": [
                    {"role": image["role"], "path": image["path"]}
                    for image in child_images
                ],
            }
        )
    native_build = []
    seen_native = set()
    for native, operation_env in native_selections:
        key = canonical_json_sha256(native)
        if key not in seen_native:
            native_build.extend(
                _capture_rust_native_build([native], images, cwd=cwd, env=operation_env)
            )
            seen_native.add(key)
    images = canonical_images(images)
    return images, {
        "schema": RUNTIME_GATE_RUST_SELECTION_SCHEMA,
        "producer_command": list(command_argv),
        "admitted_command": list(admitted_command),
        "command_semantics_sha256": canonical_json_sha256(list(command_argv)),
        "compiler_host": host,
        "cargo": str(cargo),
        "target": target,
        "native_required": declared,
        "native_build": native_build,
        "operations": operations,
        "selected_process_count": len(images),
    }


def _capture_rust_native_build(
    native_selections: Sequence[Mapping[str, object]],
    images: list[dict[str, object]],
    *,
    cwd: Path,
    env: Mapping[str, str],
) -> list[dict[str, object]]:
    native_build: list[dict[str, object]] = []
    for selection in native_selections:
        compilers = {}
        for language, command in selection["compilers"].items():
            role = _native_compiler_role(language, selection["target"])
            selected_images, compiler = capture_native_compiler_process_images(
                command,
                role=role,
                language=language,
                target=selection["target"],
                cwd=cwd,
                env=env,
                captured_images=images,
            )
            images.extend(selected_images)
            compilers[language] = compiler
        archiver = capture_image(
            "rust-build-native-archiver-" + selection["target"],
            Path(selection["archiver"]),
            preserve_path=True,
        )
        images.append(archiver)
        native_build.append(
            {"selection": selection, "compilers": compilers, "resources": None}
        )
    return native_build


def capture_rust_link_process_images(
    *,
    rustc: Path,
    cargo: Path | None,
    cwd: Path,
    env: Mapping[str, str],
    target: str | None,
    command_argv: Sequence[str],
    linker_process_helpers: Mapping[str, Sequence[str]] | None = None,
    linker_build_tools: Mapping[str, Mapping[str, str]] | None = None,
    rustc_version: str | None = None,
    native_units: Mapping[str, Sequence[str]] | None = None,
    admitted_command: Sequence[str] | None = None,
    _capture_cache=None,
    _captured_images: Sequence[Mapping[str, object]] = (),
) -> tuple[list[dict[str, object]], dict[str, object]]:
    """Capture both Cargo target and host-unit linker families exactly once."""
    if not command_argv:
        raise ValueError("Rust link capture requires its actual command")
    from tools.proof_queue_pkg import command_admission

    admitted = list(command_argv if admitted_command is None else admitted_command)
    admitted_envelope = command_admission.envelope_for_command(admitted)
    if command_admission.runtime_gate_arguments(admitted_envelope) is not None:
        return _capture_runtime_gate_rust_operations(
            rustc=rustc,
            cargo=cargo,
            cwd=cwd,
            env=env,
            target=target,
            command_argv=command_argv,
            admitted_command=admitted,
            native_units=native_units,
            linker_process_helpers=linker_process_helpers,
            linker_build_tools=linker_build_tools,
            rustc_version=rustc_version,
        )
    declared = admitted_envelope["cargo_native_units"]
    if ({} if native_units is None else native_units) != declared:
        raise ValueError("native build requirements differ from admitted command")
    cargo_invocation = (
        command_admission.cargo_invocation_for_envelope(admitted_envelope)
        if "cargo" in admitted_envelope["toolchains"]
        else None
    )
    return _capture_rust_compilation(
        rustc=rustc,
        cargo=cargo,
        cwd=cwd,
        env=env,
        target=target,
        command_argv=command_argv,
        admitted=admitted,
        declared=declared,
        cargo_invocation=cargo_invocation,
        native_units=native_units,
        linker_process_helpers=linker_process_helpers,
        linker_build_tools=linker_build_tools,
        rustc_version=rustc_version,
        _capture_cache=_capture_cache,
        _captured_images=_captured_images,
    )


def _capture_rust_compilation(
    *,
    rustc,
    cargo,
    cwd,
    env,
    target,
    command_argv,
    admitted,
    declared,
    cargo_invocation,
    native_units,
    linker_process_helpers,
    linker_build_tools,
    rustc_version,
    _capture_cache=None,
    _captured_images=(),
    wasi_command_abi: WasiCAbiProjection | None = None,
):
    """One actual Rust/Cargo operation; its caller owns source admission."""
    env = ProcessEnvironment(env)
    from tools.proof_queue_pkg import command_admission

    compiler_probes: list[dict[str, object]] = []
    capture_cache = {} if _capture_cache is None else _capture_cache
    metadata_cache = capture_cache.setdefault("metadata", {})
    configuration_cache = capture_cache.setdefault("configuration", {})

    def metadata(
        arguments: list[str], *, executable=rustc, phase="compiler-metadata"
    ) -> str:
        key = (str(executable), tuple(arguments), str(cwd), tuple(sorted(env.items())))
        if key in metadata_cache:
            stdout, records = metadata_cache[key]
            compiler_probes.extend(dict(record) for record in records)
            return stdout
        before = len(compiler_probes)
        result = _run_rust_link_probe(
            [str(executable), *arguments],
            phase=phase,
            unit="compiler",
            cwd=cwd,
            compiler_cwd=cwd,
            env=env,
            timeout=30.0,
            probes=compiler_probes,
        )
        if result.returncode != 0:
            raise RustLinkCaptureError(
                "selected Rust toolchain metadata command failed",
                unit="compiler",
                probes=compiler_probes,
            )
        metadata_cache[key] = (result.stdout, compiler_probes[before:])
        return result.stdout

    try:
        host = rustc_host(
            rustc_version if rustc_version is not None else metadata(["-vV"])
        )
        compiler_sysroot = rustc_printed_sysroot(
            metadata(["--print", "sysroot"]), cwd=cwd
        )
        cargo_version = None
        if cargo is not None and not (
            cargo_invocation and cargo_invocation.is_cargo_query
        ):
            _require_stable_cargo_environment(env)
            cargo_version = metadata(
                ["--version", "--verbose"], executable=cargo, phase="cargo-metadata"
            ).strip()
            _require_stable_cargo_version(cargo_version)
    except RustLinkCaptureError:
        raise
    except Exception as exc:
        raise RustLinkCaptureError(
            str(exc), unit="compiler", probes=compiler_probes
        ) from exc
    native_selections = select_cargo_native_units(
        required=native_units, target=target, host=host, cwd=cwd, env=env
    )
    if any(row["target"] != host for row in native_selections):
        raise ValueError(
            "native C/C++ cross-target proof requires an effective cc-rs compiler command; driver/archiver paths alone do not attest target flags"
        )
    units = ["target", "host-proc-macro"] if cargo is not None else ["target"]
    images: list[dict[str, object]] = []
    selections: list[dict[str, object]] = []
    for unit in units:
        probes = list(compiler_probes)
        try:
            selected_images, selection = _capture_rust_link_unit(
                rustc=rustc,
                cargo=cargo,
                cwd=cwd,
                env=env,
                target=target,
                command_argv=command_argv,
                cargo_invocation=cargo_invocation,
                linker_process_helpers=linker_process_helpers,
                linker_build_tools=linker_build_tools,
                unit=unit,
                host=host,
                compiler_sysroot=compiler_sysroot,
                captured_images=(*_captured_images, *images),
                capture_inputs=configuration_cache,
                probes=probes,
                wasi_command_abi=wasi_command_abi,
                compiler_flag_context=(
                    selections[0]["flag_context"]
                    if unit == "target-documenter"
                    else None
                ),
            )
        except RustLinkCaptureError:
            raise
        except Exception as exc:
            raise RustLinkCaptureError(str(exc), unit=unit, probes=probes) from exc
        images.extend(selected_images)
        selections.append(selection)
        if unit == "target" and cargo_invocation is not None:
            context = selection.get("flag_context")
            doc_flags = (
                ()
                if context is None or "documenter" not in context
                else context["documenter"]["selected_flags"]
            )
            if command_admission.rust_documenter_required(cargo_invocation, doc_flags):
                units.append("target-documenter")
    native_build = _capture_rust_native_build(
        native_selections, images, cwd=cwd, env=env
    )
    images = canonical_images(images)
    return images, {
        "schema": "molt.proof-rust-link-selection-telemetry.v5",
        "producer_command": list(command_argv),
        "admitted_command": admitted,
        "native_required": declared,
        "native_build": native_build,
        "target": target,
        "compiler_host": host,
        "cargo_version": cargo_version,
        "selection_probe_count": sum(
            row["selection_probe_count"] for row in selections
        ),
        "selected_process_count": len(images),
        "units": selections,
        "command_semantics_sha256": canonical_json_sha256(list(command_argv)),
    }


def _rust_probe_startup(
    abi: WasiCAbiProjection | None,
    *,
    unit: str,
    target: str | None,
    artifact: Mapping[str, object],
) -> tuple[str, dict[str, object] | None]:
    """Project the runtime's admitted test-only CRT, never author an entry point."""
    if abi is None or unit != "target":
        return "", None
    from molt.wasi_sdk_identity import WASI_C_ABI_TARGET

    if (
        target != WASI_C_ABI_TARGET
        or not artifact["test_harness"]
        or not artifact["link_required"]
    ):
        raise ValueError("managed WASI command startup requires its target libtest")
    _role, path, size, digest = next(
        row for row in abi.files if row[0] == "crt_command"
    )
    # Match build.rs's literal declaration consumed only by cfg(test). The
    # already admitted SDK search paths select this exact object in place.
    name = json.dumps(":" + path.name, ensure_ascii=False)
    declaration = (
        f'#[link(name = {name}, kind = "static", modifiers = "-bundle")]\n'
        'unsafe extern "C" {}\n'
    )
    return declaration, {
        "kind": "wasi-command",
        "member": {"path": str(path), "size": size, "sha256": digest},
        "declaration_sha256": hashlib.sha256(declaration.encode("utf-8")).hexdigest(),
    }


def _capture_rust_link_unit(
    *,
    rustc: Path,
    cargo: Path | None,
    cwd: Path,
    env: Mapping[str, str],
    target: str | None,
    command_argv: Sequence[str],
    cargo_invocation: command_admission.CargoInvocation | None,
    linker_process_helpers: Mapping[str, Sequence[str]] | None,
    linker_build_tools: Mapping[str, Mapping[str, str]] | None,
    unit: str,
    host: str,
    compiler_sysroot: Path,
    captured_images: Sequence[Mapping[str, object]],
    capture_inputs,
    probes: list[dict[str, object]],
    wasi_command_abi: WasiCAbiProjection | None = None,
    compiler_flag_context: Mapping[str, object] | None = None,
) -> tuple[list[dict[str, object]], dict[str, object]]:
    """Ask the selected Rust toolchain to link a zero-dependency synthetic crate.

    The probe runs outside the repository source/target and uses the exact captured
    environment and Cargo configuration. It executes no repository build script.
    `--print link-args` supplies the actual selected driver argv; compiler-driver
    dry-run output then exposes internally selected linker helpers such as mold.
    """
    from tools.proof_queue_pkg import command_admission

    started = time.perf_counter()
    captured_by_path = {
        _image_path_key(Path(str(row["path"]))): row for row in captured_images
    }

    def image(role, path, root_exit_disposition="require-exit", *, preserve_path=False):
        selected = custody_path(path)
        resolved = (
            selected if preserve_path else custody_path(selected.resolve(strict=True))
        )
        prior = captured_by_path.get(_image_path_key(resolved))
        if prior is None:
            prior = capture_image(
                role, path, root_exit_disposition, preserve_path=preserve_path
            )
            captured_by_path[_image_path_key(resolved)] = prior
        result = {
            key: value
            for key, value in prior.items()
            if key not in {"role", "root_exit_disposition", "path_kind"}
        }
        result["role"] = role
        if root_exit_disposition != "require-exit":
            result["root_exit_disposition"] = root_exit_disposition
        if preserve_path:
            result["path_kind"] = "selection"
        return result

    probe_env = ProcessEnvironment(env)
    # Cargo's synthetic and forwarded-context probes must use the same retained
    # compiler as metadata and payload, never select another Rustup component.
    if cargo is not None:
        from molt.rust_toolchain import rust_toolchain_library_environment

        probe_env = probe_env.updated(
            {
                "RUSTC": str(rustc),
                **rust_toolchain_library_environment(rustc, probe_env),
            }
        )

    mode = command_admission.rust_compilation_mode(
        command_argv,
        cargo=cargo is not None,
        cargo_invocation=cargo_invocation,
        unit=unit,
    )
    flag_context = None
    configured_flags: tuple[str, ...] = ()
    documenter_flags: tuple[str, ...] = ()
    if cargo is not None and mode != "query":
        flag_context, probe_env = _cargo_flag_context(
            command=command_argv,
            cwd=cwd,
            env=probe_env,
            target=target or host,
            host_unit=unit == "host-proc-macro",
            rustc=rustc,
            probes=probes,
            flag_kind="rustdocflags" if unit == "target-documenter" else "rustflags",
            compiler_context=compiler_flag_context,
            capture_inputs=capture_inputs,
        )
        configured_flags = tuple(flag_context["selected_flags"])
        if (
            unit == "target"
            and cargo_invocation is not None
            and cargo_invocation.requires_documenter
        ):
            documenter_context, _documenter_env = _cargo_flag_context(
                command=command_argv,
                cwd=cwd,
                env=env,
                target=target or host,
                host_unit=False,
                rustc=rustc,
                probes=probes,
                flag_kind="rustdocflags",
                compiler_context=flag_context,
                capture_inputs=capture_inputs,
            )
            flag_context["documenter"] = documenter_context
            documenter_flags = tuple(documenter_context["selected_flags"])
        mode = command_admission.rust_compilation_mode(
            command_argv,
            cargo=True,
            cargo_invocation=cargo_invocation,
            unit=unit,
            configured_flags=configured_flags,
        )
    original_arguments = (
        cargo_invocation.compiler_arguments(
            documenter=cargo_invocation.subcommand == "rustdoc"
        )
        if cargo_invocation is not None
        else ()
        if cargo is not None
        else command_argv[1:]
    )
    with tempfile.TemporaryDirectory(prefix="molt-rust-argument-check-") as scratch:
        _proof_rust_probe_arguments(
            tuple(original_arguments),
            Path(scratch),
            documenter=cargo_invocation is not None
            and cargo_invocation.subcommand == "rustdoc",
        )
    artifact_context = None
    base = None
    if cargo is not None and unit == "target" and cargo_invocation is not None:
        invocation = cargo_invocation
        if invocation.crate_types is None and invocation.forwarded_crate_types:
            artifact_context = _cargo_forwarded_compiler_context(
                cargo,
                command_argv,
                cwd=cwd,
                env=probe_env,
                probes=probes,
                unit=unit,
                require_crate_types=True,
            )
            base = _cargo_artifact_crate_types(artifact_context, command_argv)
    artifact_selection = command_admission.rust_link_artifact_selection(
        command_argv,
        cargo=cargo is not None,
        cargo_invocation=cargo_invocation,
        unit=unit,
        manifest_crate_types=base,
        configured_flags=configured_flags,
    )
    cargo_crate_types = artifact_selection["cargo_crate_types"]
    explicit_library = bool(cargo_crate_types or base) and "bin" not in (
        cargo_crate_types or base
    )
    test_harness = artifact_selection["test_harness"]
    startup_source, startup = _rust_probe_startup(
        wasi_command_abi, unit=unit, target=target, artifact=artifact_selection
    )
    with tempfile.TemporaryDirectory(prefix="molt-rust-link-capture-") as raw_root:
        root = Path(raw_root).resolve()
        (root / "output").mkdir()
        safe_flags = _proof_rust_probe_arguments(
            configured_flags, root, documenter=unit == "target-documenter"
        )
        if unit == "target-documenter":
            safe_flags = command_admission.rust_link_arguments(
                safe_flags, documenter=True
            )
        _proof_rust_probe_arguments(documenter_flags, root, documenter=True)
        if cargo is not None:
            probe_env = probe_env.updated(
                {
                    "CARGO_ENCODED_RUSTFLAGS": FlagStringRule.CARGO_ENCODED.encode(
                        safe_flags, variable="CARGO_ENCODED_RUSTFLAGS"
                    )
                }
            )
        compiler_cwd = root if cargo is not None else cwd
        forwarded_context: dict[str, object] | None = None
        path_projections: list[dict[str, object]] = []
        config_files: list[dict[str, object]] = []
        source = root / ("host.rs" if unit == "host-proc-macro" else "main.rs")
        source.write_text(
            "extern crate proc_macro;\n"
            if unit == "host-proc-macro"
            else startup_source + "fn main() {}\n#[test]\nfn proof_link_test() {}\n",
            encoding="utf-8",
        )
        output = root / ("probe.exe" if os.name == "nt" else "probe")
        if cargo is not None:
            manifest = root / "Cargo.toml"
            feature_names: set[str] = set()
            cargo_profile_args: list[str] = []
            rustc_link_args: list[str] = []
            command = [str(value) for value in command_argv]
            config_args = cargo_config_arguments(command, cwd=cwd)
            for argument in config_args[1::2]:
                if "=" not in argument:
                    path = Path(argument)
                    config_files.append(
                        {
                            "path": str(path),
                            "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
                            "size": path.stat().st_size,
                        }
                    )
            try:
                separator = command.index("--")
            except ValueError:
                separator = len(command)
            cargo_args = command[1:separator]
            index = 0
            while index < len(cargo_args):
                value = cargo_args[index]
                if value in {"--all-features", "--no-default-features"}:
                    cargo_profile_args.append(value)
                    index += 1
                    continue
                if value in {"--profile", "--features", "-F", "--config"}:
                    if index + 1 >= len(cargo_args):
                        raise ValueError(f"Cargo {value} requires a value")
                    argument = cargo_args[index + 1]
                    if value == "--config":
                        pass  # Invocation-relative files were bound above.
                    elif value == "--profile":
                        pass  # Capability witnesses do not recreate package profiles.
                    else:
                        selected_features: list[str] = []
                        for feature in argument.replace(",", " ").split():
                            local = feature.rsplit("/", 1)[-1]
                            if local and all(
                                character.isalnum() or character in "_-"
                                for character in local
                            ):
                                feature_names.add(local)
                                selected_features.append(local)
                        cargo_profile_args.extend((value, ",".join(selected_features)))
                    index += 2
                    continue
                if value.startswith("--config="):
                    pass
                elif value.startswith("--profile="):
                    pass
                elif value.startswith("--features="):
                    raw_features = value.split("=", 1)[1]
                    selected_features = []
                    for feature in raw_features.replace(",", " ").split():
                        local = feature.rsplit("/", 1)[-1]
                        if local and all(
                            character.isalnum() or character in "_-"
                            for character in local
                        ):
                            feature_names.add(local)
                            selected_features.append(local)
                    cargo_profile_args.append(
                        "--features=" + ",".join(selected_features)
                    )
                index += 1
            rustc_link_args = list(
                command_admission.rust_link_arguments(
                    _proof_rust_probe_arguments(
                        cargo_invocation.compiler_arguments(
                            documenter=unit == "target-documenter"
                        )
                        if cargo_invocation is not None
                        else (),
                        root,
                        documenter=unit == "target-documenter",
                    ),
                    documenter=unit == "target-documenter",
                )
            )
            relative_paths = (
                relative_rustc_tool_paths(rustc_link_args)
                if unit != "host-proc-macro"
                else []
            )
            if relative_paths:
                forwarded_context = (
                    artifact_context
                    or _cargo_forwarded_compiler_context(
                        cargo,
                        command_argv,
                        cwd=cwd,
                        env=probe_env,
                        probes=probes,
                        unit=unit,
                    )
                )
                original_cwd = Path(str(forwarded_context["compiler_cwd"]))
                for position, prefix, original in relative_paths:
                    projected = str((original_cwd / original).absolute())
                    rustc_link_args[position] = prefix + projected
                    path_projections.append(
                        {
                            "argument_index": position,
                            "original": original,
                            "projected": projected,
                            "compiler_cwd": str(original_cwd),
                        }
                    )
            feature_table = "".join(
                f"{json.dumps(name)}=[]\n" for name in sorted(feature_names)
            )
            target_table = (
                '[lib]\nproc-macro=true\npath="host.rs"\n'
                if unit == "host-proc-macro"
                else '[lib]\npath="main.rs"\n'
                if explicit_library or test_harness
                else '[[bin]]\nname="molt_link_capture"\npath="main.rs"\n'
            )
            if unit == "target" and base is not None and explicit_library:
                target_table += (
                    "proc-macro=true\n"
                    if base == ("proc-macro",)
                    else "crate-type=" + json.dumps(base) + "\n"
                )
            manifest.write_text(
                '[package]\nname="molt_link_capture"\nversion="0.0.0"\n'
                'edition="2024"\npublish=false\n\n'
                # Owner-selected TMPDIR can be below another Cargo workspace.
                # Each probe owns its workspace rather than becoming an
                # unlisted member of the enclosing project's workspace.
                + "[workspace]\n\n"
                + target_table
                + "\n[features]\n"
                + feature_table,
                encoding="utf-8",
            )
            probe_env = probe_env.updated(
                {"CARGO_TARGET_DIR": str(root / "target"), "CARGO_INCREMENTAL": "0"}
            )
            command = [
                str(cargo),
                "rustc",
                "--quiet",
                "--offline",
                "--manifest-path",
                str(manifest),
            ]
            if target:
                command.extend(("--target", target))
            command.extend(cargo_profile_args)
            if mode == "metadata" and unit != "host-proc-macro":
                command.extend(("--profile", "check"))
            command.extend(config_args)
            # Cargo's build directory is independent of CARGO_TARGET_DIR. Its
            # final CLI scalar override confines intermediate files as well.
            command.extend(
                ("--config", "build.build-dir=" + json.dumps(str(root / "build")))
            )
            from molt.cli import cargo_command as cargo_argv

            command.extend(
                token
                for override in cargo_argv.cargo_environment_force_overrides(
                    () if flag_context is None else flag_context["forced_environment"]
                )
                for token in ("--config", override)
            )
            command.extend(
                ("--lib",)
                if unit == "host-proc-macro" or explicit_library or test_harness
                else ("--bin", "molt_link_capture")
            )
            if unit == "target" and cargo_crate_types is not None:
                command.extend(("--crate-type", ",".join(cargo_crate_types)))
            command.extend(
                (
                    "--",
                    *(rustc_link_args if unit != "host-proc-macro" else ()),
                    *(
                        ("--test",)
                        if test_harness
                        and not any(
                            arguments[span.start] == "--test"
                            for arguments in (safe_flags, rustc_link_args)
                            for span in command_admission.rust_flag_spans(arguments)
                        )
                        else ()
                    ),
                )
            )
        else:
            command = [
                str(rustc),
                str(source),
                "--crate-name",
                "molt_link_capture",
                "-o",
                str(output),
            ]
            if target:
                command.extend(("--target", target))
            command.extend(
                command_admission.rust_link_arguments(
                    _proof_rust_probe_arguments(tuple(command_argv[1:]), root)
                )
            )
        # rustc print_crate_info stops before compilation if ANY metadata-only
        # print is requested, even beside link-args. Keep the two command phases
        # disjoint for Cargo and direct rustc. Cargo fingerprints extra_args_for
        # the unit, so the changed print request is not a freshness retry.
        metadata_command = (
            [str(rustc), "--print", "sysroot"]
            if mode == "query"
            else [*command, "--print", "sysroot"]
        )
        # The driver dry run below re-reads this command's inputs; save-temps
        # keeps rustc's temporaries (symbols.o, codegen units) inside the
        # synthetic root, since clang rejects missing inputs even under -###.
        command = [*command, "-C", "save-temps", "--print", "link-args"]
        if mode != "query":
            from molt.disk_capacity import require_build_capacity

            require_build_capacity((root,), env=probe_env)
        metadata = _run_rust_link_probe(
            metadata_command,
            phase="selected-sysroot",
            unit=unit,
            cwd=cwd,
            compiler_cwd=compiler_cwd,
            env=probe_env,
            timeout=30.0,
            probes=probes,
        )
        if metadata.returncode != 0:
            raise ValueError("selected Rust sysroot metadata command failed")
        if cargo is not None:
            reported = metadata.stdout.strip()
            if reported and not Path(reported).is_absolute():
                raise ValueError(
                    "unsupported Cargo linker custody: configuration-selected relative "
                    f"sysroot {reported!r} for {unit}; synthetic compiler cwd is {compiler_cwd}, "
                    f"Cargo invocation cwd is {cwd}. Forwarded paths were projected from "
                    "Cargo metadata, but per-package configuration-relative paths require "
                    "a Cargo compiler-execution context hook before repository build."
                )
        selected_sysroot = rustc_printed_sysroot(metadata.stdout, cwd=compiler_cwd)
        completed = (
            metadata
            if mode == "query"
            else _run_rust_link_probe(
                command,
                phase="link-selection",
                unit=unit,
                cwd=cwd,
                compiler_cwd=compiler_cwd,
                env=probe_env,
                timeout=120.0,
                probes=probes,
            )
        )
        if cargo is not None:
            printed = _selected_command_lines(
                completed.stdout + "\n" + completed.stderr
            )
            if (
                len(printed) == 1
                and not Path(printed[0][0]).is_absolute()
                and any(separator in printed[0][0] for separator in "/\\")
            ):
                raise ValueError(
                    "unsupported Cargo linker custody: configuration-selected relative "
                    f"linker {printed[0][0]!r} for {unit}; synthetic compiler cwd {compiler_cwd}, "
                    f"Cargo invocation cwd {cwd}. Original per-package execution contexts "
                    "require a Cargo compiler-execution context hook before repository build."
                )
        if completed.returncode != 0:
            raise ValueError("synthetic Rust linker selection command failed")
        if not artifact_selection["link_required"]:
            if _selected_command_lines(completed.stdout + "\n" + completed.stderr):
                raise ValueError(
                    "non-linking Rust operation unexpectedly selected a linker"
                )
            return [], {
                "schema": "molt.proof-rust-link-unit.v2",
                "unit": unit,
                "artifact_selection": artifact_selection,
                "startup": startup,
                "artifact_context": artifact_context,
                "flag_context": flag_context,
                "process_image_refs": [],
                "process_resolution": [],
                "selected_process_count": 0,
                "compiler_cwd": str(compiler_cwd),
                "forwarded_compiler_context": forwarded_context,
                "path_projections": path_projections,
                "configuration_files": config_files,
                "target": target,
                "probe": "cargo-rustc" if cargo is not None else "rustc",
                "selection_probe_count": 0 if mode == "query" else 1,
                "metadata_probe_count": 1,
                "command_semantics_sha256": canonical_json_sha256(list(command_argv)),
                "link_argv_sha256": canonical_json_sha256([]),
                "capture_s": time.perf_counter() - started,
            }
        selected = _selected_rust_link_command(completed.stdout, completed.stderr)
        search = RustToolSearch(host, selected_sysroot, compiler_sysroot)
        primary, resolution = search.resolve(
            selected[0], cwd=compiler_cwd, env=probe_env
        )
        resolutions = [resolution]
        selected_paths = [primary]
        collect2_probe_count = 0
        driver_name = primary.name.casefold()
        if any(token in driver_name for token in ("clang", "gcc", "cc", "c++")):
            dry_run = _run_rust_link_probe(
                [str(primary), "-###", *selected[1:]],
                phase="driver-helpers",
                unit=unit,
                cwd=compiler_cwd,
                compiler_cwd=compiler_cwd,
                env=probe_env,
                timeout=30.0,
                probes=probes,
            )
            nested_commands = _driver_command_lines(
                dry_run.stdout + "\n" + dry_run.stderr
            )
            if dry_run.returncode != 0 or not nested_commands:
                raise ValueError(
                    "selected compiler driver did not expose its exact helper commands"
                )
            for nested in nested_commands:
                selected_path, resolution = search.resolve(
                    nested[0], cwd=compiler_cwd, env=probe_env
                )
                selected_paths.append(selected_path)
                resolutions.append(resolution)
                if _is_gcc_collect2(selected_path):
                    collect2_probe_count += 1
                    selected_path, resolution = _collect2_selected_linker(
                        primary,
                        selected[1:],
                        unit=unit,
                        cwd=compiler_cwd,
                        env=probe_env,
                        probes=probes,
                    )
                    selected_paths.append(selected_path)
                    resolutions.append(resolution)
        # A Rust gcc-ld wrapper selected by any driver, collect2 or rustc
        # itself execs the sysroot's rust-lld.
        for selected_path in list(selected_paths):
            bundled = search.bundled_lld(selected_path)
            if bundled is not None:
                selected_paths.append(bundled[0])
                resolutions.append(bundled[1])
        helper_policy = {
            str(linker).casefold(): tuple(str(helper) for helper in helpers)
            for linker, helpers in (linker_process_helpers or {}).items()
        }
        declared_helpers = helper_policy.get(driver_name, ())
        selected_helpers: list[Path] = []
        for helper_name in declared_helpers:
            if Path(helper_name).name != helper_name:
                raise ValueError("Rust linker helper policy requires basenames")
            helper = primary.with_name(helper_name)
            if helper.is_file():
                selected_helpers.append(helper.absolute())
        selected_paths.extend(selected_helpers)
        unique_paths = list(
            {str(path): path for path in map(custody_path, selected_paths)}.values()
        )
        images = []
        auxiliary_keys = {_image_path_key(path) for path in selected_helpers}
        for index, path in enumerate(unique_paths):
            captured = image(
                "rust-linker" if index == 0 else "rust-link-helper",
                path,
                root_exit_disposition=(
                    "terminate"
                    if _image_path_key(path) in auxiliary_keys
                    else "require-exit"
                ),
            )
            images.append(captured)
            if str(path.absolute()) != captured["path"]:
                images.append(
                    image(
                        str(captured["role"]),
                        path,
                        root_exit_disposition=str(
                            captured.get("root_exit_disposition", "require-exit")
                        ),
                        preserve_path=True,
                    )
                )
        build_tool_policy = {
            str(linker).casefold(): {
                str(tool): str(role) for tool, role in tools.items()
            }
            for linker, tools in (linker_build_tools or {}).items()
        }
        declared_build_tools = build_tool_policy.get(driver_name, {})
        selected_build_tools: list[Path] = []
        for tool_name, role in declared_build_tools.items():
            if Path(tool_name).name != tool_name:
                raise ValueError("Rust build-tool policy requires basenames")
            if not role.startswith("rust-build-"):
                raise ValueError(
                    "Rust build-tool policy requires typed rust-build roles"
                )
            tool = primary.with_name(tool_name)
            if not tool.is_file():
                raise ValueError(
                    f"selected Rust linker has no declared build tool {tool_name!r}"
                )
            selected_build_tools.append(tool.absolute())
            captured = image(role, tool, root_exit_disposition="require-exit")
            images.append(captured)
            if _image_path_key(tool) != captured["path"]:
                images.append(image(role, tool, preserve_path=True))
        images = canonical_images(images)
        telemetry = {
            "schema": "molt.proof-rust-link-unit.v2",
            "unit": unit,
            "artifact_selection": artifact_selection,
            "startup": startup,
            "artifact_context": artifact_context,
            "flag_context": flag_context,
            "process_image_refs": [
                {"role": image["role"], "path": image["path"]} for image in images
            ],
            "process_resolution": resolutions,
            "compiler_cwd": str(compiler_cwd),
            "forwarded_compiler_context": forwarded_context,
            "path_projections": path_projections,
            "configuration_files": config_files,
            "target": target,
            "probe": "cargo-rustc" if cargo is not None else "rustc",
            "selected_process_count": len(images),
            "selection_probe_count": 1,
            "metadata_probe_count": 1,
            "collect2_probe_count": collect2_probe_count,
            "declared_helper_count": len(declared_helpers),
            "selected_helper_count": len(selected_helpers),
            "declared_build_tool_count": len(declared_build_tools),
            "selected_build_tool_count": len(selected_build_tools),
            "helper_policy_sha256": hashlib.sha256(
                json.dumps(
                    {
                        linker: list(helpers)
                        for linker, helpers in sorted(helper_policy.items())
                    },
                    sort_keys=True,
                    separators=(",", ":"),
                ).encode()
            ).hexdigest(),
            "build_tool_policy_sha256": hashlib.sha256(
                json.dumps(
                    {
                        linker: dict(sorted(tools.items()))
                        for linker, tools in sorted(build_tool_policy.items())
                    },
                    sort_keys=True,
                    separators=(",", ":"),
                ).encode()
            ).hexdigest(),
            "command_semantics_sha256": canonical_json_sha256(list(command_argv)),
            "link_argv_sha256": hashlib.sha256(
                json.dumps(selected, separators=(",", ":")).encode()
            ).hexdigest(),
            "capture_s": time.perf_counter() - started,
        }
        return images, telemetry


def _validate_runtime_gate_projection(operation, projection, *, host, cargo, rustc):
    if operation.environment != "runtime-wasi":
        if projection is not None:
            raise ValueError("ordinary Cargo operation has a managed C projection")
        return list(operation.command)
    from molt.cli import cargo_command as cargo_argv
    from molt.wasi_sdk_identity import WasiCAbiProjection
    from molt.rust_toolchain import rust_flag_spans
    from tools.runtime_test_gate_plan import WASI_TARGET

    if not isinstance(projection, Mapping) or set(projection) != {
        "sdk",
        "c_abi",
        "c_environment",
        "rustflags",
        "tools",
        "wrappers",
        "forced_environment",
        "build_directory_base",
        "build_directory",
        "target_directory",
    }:
        raise ValueError("runtime WASI operation lacks its complete C projection")
    sdk = validate_wasi_sdk_selection(projection["sdk"])
    generation = sdk["generation"]
    abi = WasiCAbiProjection.from_facts(
        Path(sdk["sdk"]),
        sdk_version=generation["asset"]["sdk_version"],
        llvm_version=generation["asset"]["llvm_version"],
        tree_sha256=generation["tree"]["sha256"],
        facts=generation["facts"],
    )
    if WasiCAbiProjection.decode(projection["c_abi"]) != abi:
        raise ValueError("managed C operation differs from its SDK ABI identity")
    tools, wrappers = projection["tools"], projection["wrappers"]
    if not isinstance(tools, Mapping) or not isinstance(wrappers, Mapping):
        raise ValueError("runtime Cargo tool projection is malformed")
    if tools.get("cargo") != cargo:
        raise ValueError("runtime Cargo projection selected another Cargo executable")
    if rustc is not None and tools.get("rustc") != rustc:
        raise ValueError("runtime Cargo projection selected another Rust compiler")
    for role, sdk_role in {
        "cc": "clang",
        "cxx": "clang++",
        "ar": "llvm-ar",
        "ranlib": "llvm-ranlib",
        "linker": "wasm-ld",
        "final_linker": "wasm-ld",
    }.items():
        if role == "final_linker" and role not in tools:
            continue
        expected = str(
            Path(sdk["sdk"]) / generation["facts"]["tools"][sdk_role]["path"]
        )
        if tools.get(role) != expected:
            raise ValueError("managed C operation selected a foreign SDK tool")
    if not isinstance(projection["c_environment"], Mapping) or any(
        not isinstance(name, str)
        or not isinstance(values, list)
        or any(not isinstance(value, str) for value in values)
        for name, values in projection["c_environment"].items()
    ):
        raise ValueError("managed C flag projection is malformed")
    flags = projection["rustflags"]
    if not isinstance(flags, list) or any(not isinstance(flag, str) for flag in flags):
        raise ValueError("managed Rust flag projection is malformed")
    if tuple(flags) != abi.rustflags(flags) or not any(
        span.codegen == "link-arg=--export-table" for span in rust_flag_spans(flags)
    ):
        raise ValueError("runtime WASI operation lost its C ABI or table-export flags")
    forced = projection["forced_environment"]
    if not isinstance(forced, list) or any(
        not isinstance(name, str) for name in forced
    ):
        raise ValueError("runtime Cargo forced environment is malformed")
    base_raw = projection["build_directory_base"]
    directory_raw = projection["build_directory"]
    if not isinstance(base_raw, str) or not isinstance(directory_raw, str):
        raise ValueError("runtime Cargo output projection is malformed")
    base, directory = Path(base_raw), Path(directory_raw)
    target_raw = projection["target_directory"]
    if target_raw is not None and (
        not isinstance(target_raw, str) or not Path(target_raw).is_absolute()
    ):
        raise ValueError("runtime Cargo final output projection is malformed")
    if (
        not base.is_absolute()
        or directory.parent != base / ".molt-toolchains"
        or re.fullmatch(r"[0-9a-f]{64}", directory.name) is None
    ):
        raise ValueError("runtime Cargo intermediate output escaped its captured base")
    return list(
        cargo_argv._pin_cargo_command(
            operation.command,
            {name: Path(value) for name, value in tools.items()},
            {name: Path(value) for name, value in wrappers.items()},
            WASI_TARGET,
            tuple(forced),
            build_directory=directory,
            target_directory=None if target_raw is None else Path(target_raw),
        )
    )


def _validate_runtime_gate_rust_selection(
    identity,
    raw,
    *,
    required_native_units,
    full_capture,
    command_argv,
):
    from molt.wasi_sdk_identity import WasiCAbiProjection
    from tools.proof_queue_pkg import command_admission
    from tools.runtime_test_gate_plan import cargo_operations

    if set(raw) != {
        "schema",
        "producer_command",
        "admitted_command",
        "command_semantics_sha256",
        "compiler_host",
        "cargo",
        "target",
        "native_required",
        "native_build",
        "operations",
        "selected_process_count",
    }:
        raise ValueError("runtime Cargo selection fields differ from their schema")
    envelope = command_admission.envelope_for_command(raw["admitted_command"])
    args = command_admission.runtime_gate_arguments(envelope)
    if args is None:
        raise ValueError("runtime Cargo selection has no registered producer")
    host, cargo = raw["compiler_host"], raw["cargo"]
    if (
        not isinstance(host, str)
        or not host
        or not isinstance(cargo, str)
        or not Path(cargo).is_absolute()
    ):
        raise ValueError("runtime Cargo selection has invalid compiler coordinates")
    if raw["native_required"] != envelope["cargo_native_units"] or (
        required_native_units is not None
        and dict(required_native_units) != raw["native_required"]
    ):
        raise ValueError("runtime Cargo native requirements differ from admission")
    producer = raw["producer_command"]
    if raw["command_semantics_sha256"] != canonical_json_sha256(producer) or (
        command_argv is not None and list(command_argv) != producer
    ):
        raise ValueError("runtime Cargo producer differs from actual admission")
    python = cast(Mapping[str, object], envelope["python"])
    if (
        command_admission._python_invocation_argv(
            cast(Sequence[str], envelope["argv"]), python
        )[1:]
        != (command_admission._python_invocation_argv(producer, python)[1:])
    ):
        raise ValueError("runtime Cargo producer differs from its submitted payload")
    planned = cargo_operations(
        args.policy, args.profile, cargo=cargo, compiler_host=host
    )
    rows = raw["operations"]
    if not isinstance(rows, list) or len(rows) != len(planned):
        raise ValueError("runtime Cargo operation inventory is incomplete")
    images = canonical_images(identity["process_images"])
    image_map = {
        (row["role"], _image_path_key(Path(str(row["path"])))): row for row in images
    }
    operation_keys = set()
    native_expected = {}
    managed = {}
    for operation, row in zip(planned, rows, strict=True):
        if (
            not isinstance(row, Mapping)
            or set(row)
            != {
                "role",
                "source_command",
                "environment",
                "c_units",
                "native_required",
                "projection",
                "selection",
                "process_image_refs",
            }
            or (row["role"], row["source_command"], row["environment"], row["c_units"])
            != (
                operation.role,
                list(operation.command),
                operation.environment,
                operation.c_units,
            )
        ):
            raise ValueError("runtime Cargo operation differs from its source plan")
        if operation.c_units == "managed-wasi" and not {"wasi-clang", "wasm-ld"} <= set(
            cast(Sequence[str], envelope["toolchains"])
        ):
            raise ValueError("managed WASI operation lacks its SDK declaration")
        expected_command = _validate_runtime_gate_projection(
            operation,
            row["projection"],
            host=host,
            cargo=cargo,
            rustc=identity.get("content_path"),
        )
        selection = row["selection"]
        if operation.environment == "runtime-wasi" and isinstance(selection, Mapping):
            from molt.cli.runtime_cargo_plan import (
                require_cargo_build_directory_version,
            )

            require_cargo_build_directory_version(
                str(selection.get("cargo_version", ""))
            )
        if (
            not isinstance(selection, Mapping)
            or selection.get("producer_command") != expected_command
        ):
            raise ValueError("runtime Cargo child command differs from its operation")
        invocation = command_admission.parse_cargo_invocation(
            ["cargo", *expected_command[1:]]
        )
        targets = [
            value for name, value in invocation.option_values if name == "--target"
        ]
        if selection.get("compiler_host") != host:
            raise ValueError("runtime Cargo child selected another compiler host")
        refs = row["process_image_refs"]
        if not isinstance(refs, list) or any(
            not isinstance(ref, Mapping) or set(ref) != {"role", "path"} for ref in refs
        ):
            raise ValueError("runtime Cargo child image references are malformed")
        keys = {(ref["role"], _image_path_key(Path(ref["path"]))) for ref in refs}
        if len(keys) != len(refs) or not keys <= image_map.keys():
            raise ValueError("runtime Cargo child image membership is incomplete")
        if selection.get("admitted_command") != ["cargo", *expected_command[1:]]:
            raise ValueError(
                "runtime Cargo child source argv differs from its operation"
            )
        _validate_rust_compilation_selection(
            {"process_images": [image_map[key] for key in keys]},
            selection,
            invocation=invocation,
            cargo=True,
            declared={},
            submitted=["cargo", *expected_command[1:]],
            python=None,
            required_native_units={},
            full_capture=full_capture,
            command_argv=expected_command,
            wasi_command_abi=(
                None
                if row["projection"] is None
                else WasiCAbiProjection.decode(row["projection"]["c_abi"])
            ),
        )
        target_context = selection["units"][0]["flag_context"]
        captured_target = []
        _resolve_cargo_flag_context(target_context, selected_target=captured_target)
        if captured_target != [selection["target"]] or (
            targets and selection["target"] != targets[-1]
        ):
            raise ValueError("runtime Cargo child target differs from its operation")
        operation_keys.update(keys)
        requirements = raw["native_required"]
        if row[
            "native_required"
        ] != requirements or not operation.admits_native_requirements(requirements):
            raise ValueError("runtime Cargo operation lost its C source declaration")
        target = selection["target"] or host
        for unit, languages in requirements.items():
            triple = target if unit == "target" else host
            native_expected.setdefault(triple, {})[unit] = languages
            if triple != host:
                if operation.c_units != "managed-wasi" or row["projection"] is None:
                    raise ValueError(
                        "cross-target C operation lacks managed SDK custody"
                    )
                managed[triple] = row["projection"]
    native = raw["native_build"]
    if not isinstance(native, list):
        raise ValueError("runtime Cargo C source custody is missing")
    _validate_rust_native_build(
        native, native_expected, images, full_capture=full_capture
    )
    for row in native:
        target = row["selection"]["target"]
        if target in managed:
            projection = managed[target]
            from molt.cli.runtime_cargo_plan import _c_tool_environment_names

            abi = WasiCAbiProjection.decode(projection["c_abi"])

            for language, command in row["selection"]["compilers"].items():
                tool, flags = (
                    ("cc", "cflags") if language == "c" else ("cxx", "cxxflags")
                )
                expected = [projection["tools"][tool]]
                expected.extend(
                    token
                    for name in reversed(
                        _c_tool_environment_names(
                            flags, target=target, host_target=host
                        )
                    )
                    for token in projection["c_environment"].get(name, [])
                )
                if (
                    command != _managed_wasi_c_command(expected, abi)
                    or row["selection"]["archiver"] != projection["tools"]["ar"]
                ):
                    raise ValueError(
                        "managed C compiler/flags differ from the runtime plan"
                    )
    selected = {
        key
        for key in image_map
        if key[0] in {"rust-linker", "rust-link-helper"}
        or str(key[0]).startswith("rust-build-")
    }
    non_native = {
        key for key in selected if not str(key[0]).startswith("rust-build-native-")
    }
    if non_native != operation_keys or raw["selected_process_count"] != len(selected):
        raise ValueError("runtime Cargo process closure differs from its operations")
    return dict(raw)


def _validate_rust_native_build(
    native: Sequence[Mapping[str, object]],
    expected: Mapping[str, Mapping[str, Sequence[str]]],
    images: Sequence[Mapping[str, object]],
    *,
    full_capture: bool,
) -> None:
    seen: dict[str, dict[str, list[str]]] = {}
    for row in native:
        if not isinstance(row, Mapping) or set(row) != {
            "selection",
            "compilers",
            "resources",
        }:
            raise ValueError("native build unit record is malformed")
        selection = row["selection"]
        if not isinstance(selection, Mapping) or set(selection) != {
            "units",
            "target",
            "compilers",
            "archiver",
            "resource_roots",
        }:
            raise ValueError("native build unit selection is malformed")
        triple = selection["target"]
        if not isinstance(triple, str) or triple in seen or triple not in expected:
            raise ValueError("native build units must select each declared target once")
        if selection["units"] != expected[triple]:
            raise ValueError(
                "native build role/language custody differs from admission"
            )
        seen[triple] = selection["units"]
        languages = {
            language for values in expected[triple].values() for language in values
        }
        commands, compilers = selection["compilers"], row["compilers"]
        if (
            not isinstance(commands, Mapping)
            or set(commands) != languages
            or not isinstance(compilers, Mapping)
            or set(compilers) != languages
        ):
            raise ValueError("native build language custody is incomplete")
        for language, command in commands.items():
            if (
                not isinstance(command, list)
                or not command
                or any(not isinstance(value, str) or not value for value in command)
                or not Path(command[0]).is_absolute()
            ):
                raise ValueError("native compiler selection is malformed")
            role = _native_compiler_role(language, triple)
            validate_native_compiler_capture(
                compilers[language],
                images,
                command=command,
                language=language,
                target=triple,
                role=role,
            )
        archiver_role = "rust-build-native-archiver-" + triple
        archivers = [image for image in images if image["role"] == archiver_role]
        if (
            len(archivers) != 1
            or not isinstance(selection["archiver"], str)
            or not Path(selection["archiver"]).is_absolute()
            or _image_path_key(Path(archivers[0]["path"]))
            != _image_path_key(Path(selection["archiver"]))
        ):
            raise _image_membership_error(
                "native independent archiver custody is incomplete",
                unit=triple,
                role=archiver_role,
                selected=selection["archiver"],
                images=((image["role"], image["path"]) for image in archivers),
            )
        resources = row["resources"]
        roots = selection["resource_roots"]
        if (
            not isinstance(roots, list)
            or any(
                not isinstance(path, str) or not Path(path).is_absolute()
                for path in roots
            )
            or roots != sorted(set(roots))
        ):
            raise ValueError("native build resource roots are malformed")
        if resources is None and not full_capture:
            continue
        if (
            not isinstance(resources, list)
            or not isinstance(selection["resource_roots"], list)
            or [
                resource.get("selected_root")
                for resource in resources
                if isinstance(resource, Mapping)
            ]
            != selection["resource_roots"]
            or any(
                not isinstance(resource, Mapping)
                or not isinstance(resource.get("root", resource.get("path")), str)
                for resource in resources
            )
        ):
            raise ValueError("native build resource custody is incomplete")
        from tools.proof_queue_pkg.command_identity import (
            _validate_directory_manifest_identity,
        )

        for resource in resources:
            selected_root = resource["selected_root"]
            if (
                not isinstance(selected_root, str)
                or not Path(selected_root).is_absolute()
            ):
                raise ValueError("native build resource selection is not absolute")
            if "root" in resource:
                _validate_directory_manifest_identity(
                    {
                        key: value
                        for key, value in resource.items()
                        if key != "selected_root"
                    },
                    selected_root=Path(selected_root),
                )
            elif (
                set(resource) != {"selected_root", "path", "size_bytes", "sha256"}
                or resource["path"] != selected_root
                or type(resource["size_bytes"]) is not int
                or resource["size_bytes"] < 0
                or not isinstance(resource["sha256"], str)
                or re.fullmatch(r"[0-9a-f]{64}", resource["sha256"]) is None
            ):
                raise ValueError(
                    "native build file resource lacks bound content custody"
                )
    if seen != expected:
        raise ValueError("native build-unit custody is incomplete")
    native_roles = {
        _native_compiler_role(language, triple)
        for triple, requirements in expected.items()
        for languages in requirements.values()
        for language in languages
    } | {"rust-build-native-archiver-" + triple for triple in expected}
    if {
        row["role"]
        for row in images
        if str(row["role"]).startswith("rust-build-native-")
    } != native_roles:
        raise ValueError(
            "native compiler process roles differ from declared build units"
        )


def validate_rust_link_selection(
    identity: Mapping[str, object],
    *,
    required_native_units: Mapping[str, Sequence[str]] | None = None,
    full_capture: bool = False,
    command_argv: Sequence[str] | None = None,
) -> dict[str, object]:
    """One structural receiver for Rust links and declared native build languages."""
    from tools.proof_queue_pkg import command_admission

    raw = identity.get("link_selection")
    if (
        isinstance(raw, Mapping)
        and raw.get("schema") == RUNTIME_GATE_RUST_SELECTION_SCHEMA
    ):
        return _validate_runtime_gate_rust_selection(
            identity,
            raw,
            required_native_units=required_native_units,
            full_capture=full_capture,
            command_argv=command_argv,
        )
    if (
        not isinstance(raw, Mapping)
        or raw.get("schema") != "molt.proof-rust-link-selection-telemetry.v5"
    ):
        raise ValueError("Rust linker selection telemetry schema mismatch")
    admitted = raw.get("admitted_command")
    if (
        not isinstance(admitted, list)
        or not admitted
        or any(not isinstance(value, str) or not value for value in admitted)
    ):
        raise ValueError("Rust linker capture has no admitted command")
    admitted_envelope = command_admission.envelope_for_command(admitted)
    if command_admission.runtime_gate_arguments(admitted_envelope) is not None:
        raise ValueError("runtime gate requires its complete Cargo operation selection")
    invocation = (
        command_admission.cargo_invocation_for_envelope(admitted_envelope)
        if "cargo" in admitted_envelope["toolchains"]
        else None
    )
    declared = admitted_envelope["cargo_native_units"]
    cargo = "cargo" in admitted_envelope["toolchains"]
    payload = cast(
        Mapping[str, Any], admitted_envelope.get("delegated") or admitted_envelope
    )
    return _validate_rust_compilation_selection(
        identity,
        raw,
        invocation=invocation,
        cargo=cargo,
        declared=declared,
        submitted=payload["argv"],
        python=payload.get("python"),
        required_native_units=required_native_units,
        full_capture=full_capture,
        command_argv=command_argv,
    )


def _validate_rust_compilation_selection(
    identity,
    raw,
    *,
    invocation,
    cargo,
    declared,
    submitted,
    python,
    required_native_units,
    full_capture,
    command_argv,
    wasi_command_abi: WasiCAbiProjection | None = None,
):
    """Validate one compilation using its already admitted operation contract."""
    from tools import proof_command_argv
    from tools.proof_queue_pkg import command_admission

    if (
        not isinstance(raw, Mapping)
        or raw.get("schema") != "molt.proof-rust-link-selection-telemetry.v5"
    ):
        raise ValueError("Rust linker selection telemetry schema mismatch")
    telemetry = dict(raw)
    units = telemetry.get("units")
    if (
        not isinstance(units, list)
        or not units
        or any(not isinstance(row, Mapping) for row in units)
        or [row.get("unit") for row in units]
        not in (
            ["target"],
            ["target", "host-proc-macro"],
            ["target", "host-proc-macro", "target-documenter"],
        )
        or telemetry.get("selection_probe_count")
        != sum(row.get("selection_probe_count", -10) for row in units)
    ):
        raise ValueError("Rust linker units must each be selected exactly once pre-arm")
    admitted = telemetry.get("admitted_command")
    if (
        not isinstance(admitted, list)
        or not admitted
        or any(not isinstance(value, str) or not value for value in admitted)
    ):
        raise ValueError("Rust linker capture has no admitted command")
    if cargo and not (invocation and invocation.is_cargo_query):
        _require_stable_cargo_version(telemetry.get("cargo_version"))
    elif telemetry.get("cargo_version") is not None:
        raise ValueError("non-compiling operation has unexpected Cargo version custody")
    if [row["unit"] for row in units[:2]] != (
        ["target", "host-proc-macro"] if cargo else ["target"]
    ):
        raise ValueError("Rust linker unit family differs from its admitted command")
    producer = telemetry.get("producer_command")
    if (
        not isinstance(producer, list)
        or not producer
        or any(not isinstance(value, str) or not value for value in producer)
    ):
        raise ValueError("Rust linker capture has no exact producer command")
    command_digest = canonical_json_sha256(producer)
    if telemetry.get("command_semantics_sha256") != command_digest or (
        command_argv is not None and producer != list(command_argv)
    ):
        raise ValueError("Rust linker producer command differs from actual admission")
    compared_producer = producer
    if isinstance(python, Mapping):
        # Exact execution resolves wrappers, uv prefixes and interpreter paths.
        # Compare the existing typed Python payload, not transport spelling.
        submitted = command_admission._python_invocation_argv(submitted, python)
        compared_producer = command_admission._python_invocation_argv(producer, python)

    def executable_role(value: str) -> str:
        name = Path(value).name.lower()
        return (
            "python"
            if proof_command_argv._PYTHON_COMMAND.fullmatch(name)
            else name.removesuffix(".exe")
        )

    # A declared Cargo role may bind an explicitly selected executable with an
    # arbitrary basename. Exact producer argv is checked above; admission, not
    # its physical filename, owns the argument and artifact interpretation.
    same_role = invocation is not None or executable_role(
        submitted[0]
    ) == executable_role(compared_producer[0])
    if submitted[1:] != compared_producer[1:] or not same_role:
        raise ValueError("Rust producer command differs from its submitted payload")
    expected_documenter = None
    for row in units:
        flags = row.get("flag_context")
        mode = command_admission.rust_compilation_mode(
            producer, cargo=cargo, cargo_invocation=invocation, unit=row["unit"]
        )
        configured_flags = ()
        documenter_flags = ()
        if cargo and mode != "query":
            if (
                not isinstance(flags, Mapping)
                or flags.get("command") != producer
                or flags.get("host_unit") != (row["unit"] == "host-proc-macro")
                or flags.get("target")
                != (telemetry.get("target") or telemetry.get("compiler_host"))
            ):
                raise ValueError(
                    "Cargo flag inputs differ from admitted operation/unit"
                )
            if (
                identity.get("content_path") is not None
                and flags.get("rustc") != identity["content_path"]
            ) or (
                identity.get("probe_cwd") is not None
                and flags.get("cwd") != identity["probe_cwd"]
            ):
                raise ValueError("Cargo flag inputs differ from selected compiler/cwd")
            if flags.get("flag_kind") != (
                "rustdocflags" if row["unit"] == "target-documenter" else "rustflags"
            ):
                raise ValueError("Cargo compiler flag family differs from operation")
            configured_flags = _resolve_cargo_flag_context(flags)
            if row["unit"] == "target-documenter" and flags != expected_documenter:
                raise ValueError(
                    "Cargo documenter witness differs from selected documenter inputs"
                )
            documenter = flags.get("documenter")
            if (
                row["unit"] == "target"
                and invocation is not None
                and invocation.requires_documenter
            ):
                if (
                    not isinstance(documenter, Mapping)
                    or documenter.get("command") != producer
                    or documenter.get("flag_kind") != "rustdocflags"
                    or documenter.get("host_unit") is not False
                    or documenter.get("target") != flags.get("target")
                    or any(
                        documenter.get(key) != flags.get(key)
                        for key in (
                            "cwd",
                            "rustc",
                            "sources",
                            "environment",
                            "configuration_paths",
                        )
                    )
                ):
                    raise ValueError("Cargo documenter flags differ from operation")
                documenter_flags = _resolve_cargo_flag_context(documenter)
                if command_admission.rust_documenter_required(
                    invocation, documenter_flags
                ):
                    expected_documenter = documenter
            elif documenter is not None:
                raise ValueError("Cargo operation has unexpected documenter flags")
            mode = command_admission.rust_compilation_mode(
                producer,
                cargo=cargo,
                cargo_invocation=invocation,
                unit=row["unit"],
                configured_flags=configured_flags,
            )
        elif flags is not None:
            raise ValueError(
                "non-Cargo/query operation has unexpected Cargo flag context"
            )
        if row.get("selection_probe_count") != (0 if mode == "query" else 1):
            raise ValueError("Rust operation has an unexpected compilation probe count")
        context = row.get("artifact_context")
        base = (
            None if context is None else _cargo_artifact_crate_types(context, producer)
        )
        artifact = command_admission.rust_link_artifact_selection(
            producer,
            cargo=cargo,
            cargo_invocation=invocation,
            unit=row["unit"],
            manifest_crate_types=base,
            configured_flags=configured_flags,
        )
        _startup_source, startup = _rust_probe_startup(
            wasi_command_abi,
            unit=row["unit"],
            target=telemetry["target"],
            artifact=artifact,
        )
        if (
            row.get("schema") != "molt.proof-rust-link-unit.v2"
            or row.get("artifact_selection") != artifact
            or "startup" not in row
            or row["startup"] != startup
            or row.get("command_semantics_sha256") != command_digest
        ):
            raise ValueError(
                "Rust artifact selection differs from its admitted command"
            )
        count = row.get("selected_process_count")
        if (
            type(count) is not int
            or count < 0
            or (artifact["link_required"] and count == 0)
        ):
            raise ValueError("Rust artifact requires an observed linker selection")
        if not artifact["link_required"] and (
            count != 0
            or row.get("process_resolution") != []
            or row.get("link_argv_sha256") != canonical_json_sha256([])
        ):
            raise ValueError(
                "non-linking Rust operation has unexpected linker selection"
            )
    expected_units = ["target", "host-proc-macro"] if cargo else ["target"]
    if expected_documenter is not None:
        expected_units.append("target-documenter")
    if [row["unit"] for row in units] != expected_units:
        raise ValueError(
            "Rust documenter unit family differs from its admitted command"
        )
    if telemetry.get("native_required") != declared or (
        required_native_units is not None and dict(required_native_units) != declared
    ):
        raise ValueError("Rust native build requirement differs from admitted command")
    host, target = telemetry.get("compiler_host"), telemetry.get("target")
    if (
        not isinstance(host, str)
        or not host
        or (target is not None and not isinstance(target, str))
    ):
        raise ValueError("Rust linker capture has no selected compiler host/target")
    native = telemetry.get("native_build")
    if not isinstance(native, list):
        raise ValueError("Rust native build unit capture is missing")
    expected: dict[str, dict[str, list[str]]] = {}
    for unit, languages in declared.items():
        triple = (target or host) if unit == "target" else host
        if triple != host:
            raise ValueError(
                "native build unit lacks an effective cross-target command"
            )
        expected.setdefault(triple, {})[unit] = languages
    raw_images = identity.get("process_images")
    if not isinstance(raw_images, list):
        raise ValueError("Rust process image closure is missing")
    images = canonical_images(raw_images)
    image_keys = {
        (image["role"], _image_path_key(Path(image["path"]))) for image in images
    }
    unit_keys: set[tuple[str, str]] = set()
    for unit in units:
        refs = unit.get("process_image_refs")
        if not isinstance(refs, list) or any(
            not isinstance(row, Mapping)
            or set(row) != {"role", "path"}
            or not isinstance(row["role"], str)
            or not isinstance(row["path"], str)
            or not Path(row["path"]).is_absolute()
            for row in refs
        ):
            raise ValueError("Rust unit image references are malformed")
        keys = {(row["role"], _image_path_key(Path(row["path"]))) for row in refs}
        if (
            len(keys) != len(refs)
            or len(refs) != unit["selected_process_count"]
            or not keys <= image_keys
        ):
            missing = next(
                (
                    ref
                    for ref in refs
                    if (ref["role"], _image_path_key(Path(ref["path"])))
                    not in image_keys
                ),
                refs[0] if refs else {},
            )
            raise _image_membership_error(
                "Rust unit image membership is incomplete",
                unit=unit.get("unit"),
                role=missing.get("role"),
                selected=missing.get("path"),
                images=((image["role"], image["path"]) for image in images),
            )
        resolutions = unit.get("process_resolution")
        if not isinstance(resolutions, list) or any(
            not isinstance(row, Mapping)
            or any(
                not isinstance(row.get(field), str)
                or not Path(row[field]).is_absolute()
                for field in ("path", "content_path")
            )
            for row in resolutions
        ):
            raise ValueError("Rust unit process resolution is malformed")
        if unit["artifact_selection"]["link_required"]:
            if (
                not resolutions
                or ("rust-linker", _image_path_key(Path(resolutions[0]["path"])))
                not in keys
            ):
                first = resolutions[0] if resolutions else {}
                raise _image_membership_error(
                    "Rust target linker resolution has no captured image",
                    unit=unit.get("unit"),
                    role="rust-linker",
                    selected=first.get("path"),
                    content=first.get("content_path"),
                    images=((ref["role"], ref["path"]) for ref in refs),
                )
            paths = {path for _role, path in keys}
            for resolution in resolutions:
                if (
                    _image_path_key(Path(resolution["path"])) not in paths
                    or _image_path_key(Path(resolution["content_path"])) not in paths
                ):
                    role = next(
                        (
                            ref["role"]
                            for ref in refs
                            if _image_path_key(Path(ref["path"]))
                            == _image_path_key(Path(resolution["path"]))
                        ),
                        None,
                    )
                    raise _image_membership_error(
                        "Rust resolved process is missing from its unit images",
                        unit=unit.get("unit"),
                        role=role,
                        selected=resolution["path"],
                        content=resolution["content_path"],
                        images=((ref["role"], ref["path"]) for ref in refs),
                    )
        elif refs or resolutions:
            raise ValueError("archive-only Rust unit has process images")
        unit_keys.update(keys)
    _validate_rust_native_build(native, expected, images, full_capture=full_capture)
    selected = [
        row
        for row in images
        if row["role"] in {"rust-linker", "rust-link-helper"}
        or str(row["role"]).startswith("rust-build-")
    ]
    expected_unit_keys = {
        (row["role"], _image_path_key(Path(row["path"])))
        for row in selected
        if not str(row["role"]).startswith("rust-build-native-")
    }
    if unit_keys != expected_unit_keys:
        different = next(iter(sorted(unit_keys ^ expected_unit_keys)))
        raise _image_membership_error(
            "Rust unit image references differ from the captured closure",
            role=different[0],
            selected=different[1],
            images=((image["role"], image["path"]) for image in selected),
        )
    if len(selected) != telemetry.get("selected_process_count") or (
        not selected
        and any(row["artifact_selection"]["link_required"] for row in units)
    ):
        raise ValueError("pre-arm Rust linker process count is inconsistent")
    return telemetry


def capture_native_resources(selection: Mapping[str, object]) -> dict[str, object]:
    """Materialize selected roots once, after their live watchers have armed."""
    from tools.proof_queue_pkg.command_identity import (
        _directory_manifest_identity,
        _revalidate_directory_manifest_identity,
        _file_identity,
    )

    units = []
    for unit in selection["native_build"]:
        if unit["resources"] is None:
            resources = [
                {
                    "selected_root": path,
                    **(
                        _directory_manifest_identity(
                            Path(path), label="native build resource"
                        )
                        if Path(path).is_dir()
                        else _file_identity(Path(path))
                    ),
                }
                for path in unit["selection"]["resource_roots"]
            ]
            units.append({**unit, "resources": resources})
            continue
        for resource in unit["resources"]:
            root = Path(resource["selected_root"])
            captured = {
                key: value for key, value in resource.items() if key != "selected_root"
            }
            if "root" in captured:
                _revalidate_directory_manifest_identity(
                    captured, selected_root=root, label="native build resource"
                )
            elif _file_identity(root) != captured:
                raise ValueError(
                    "native build resource changed while live custody armed"
                )
        units.append(dict(unit))
    return {**selection, "native_build": units}


def revalidate_rust_link_process_images(
    selected_identity: Mapping[str, object],
    *,
    target: str | None,
    command_argv: Sequence[str] = (),
    required_native_units: Mapping[str, Sequence[str]] | None = None,
) -> tuple[list[dict[str, object]], dict[str, object]]:
    """Rehash frozen selections at the armed boundary, without running probes."""
    telemetry = validate_rust_link_selection(
        selected_identity,
        required_native_units=required_native_units,
        command_argv=command_argv,
    )
    if telemetry["target"] != target:
        raise ValueError("Rust linker target changed while live custody armed")
    if telemetry.get("command_semantics_sha256") != canonical_json_sha256(
        list(command_argv)
    ):
        raise ValueError(
            "Rust linker command semantics changed while live custody armed"
        )
    telemetry = capture_native_resources(telemetry)
    validate_rust_link_selection(
        {**selected_identity, "link_selection": telemetry},
        required_native_units=required_native_units,
        full_capture=True,
    )
    revalidate_rust_selection_sources(telemetry)
    for unit in (
        unit for member in rust_link_selections(telemetry) for unit in member["units"]
    ):
        for config in unit.get("configuration_files", []):
            path = Path(config["path"])
            if (
                not path.is_file()
                or hashlib.sha256(path.read_bytes()).hexdigest() != config["sha256"]
            ):
                raise ValueError("Cargo --config file changed while live custody armed")
    selected = [
        row
        for row in selected_identity["process_images"]
        if row["role"] in {"rust-linker", "rust-link-helper"}
        or str(row["role"]).startswith("rust-build-")
    ]
    try:
        return revalidate_images(selected), telemetry
    except ValueError as exc:
        raise ValueError(
            f"Rust linker process image changed while live custody armed: {exc}"
        ) from exc


def _digest(value: object) -> str | None:
    return value if isinstance(value, str) and len(value) == 64 else None


def frozen_files(payload: object) -> list[FrozenFile]:
    """Project every captured file row into one deduplicated content manifest."""
    files: dict[str, FrozenFile] = {}

    def add(raw_path: object, raw_digest: object, raw_size: object = None) -> None:
        digest = _digest(raw_digest)
        if not isinstance(raw_path, str) or digest is None:
            return
        path = Path(raw_path)
        if not path.is_absolute():
            return
        normalized = _image_path_key(path)
        size = raw_size if isinstance(raw_size, int) and raw_size >= 0 else None
        row = FrozenFile(str(path), digest, size)
        prior = files.get(normalized)
        if prior is not None and prior.sha256 != digest:
            raise ValueError(f"captured file has conflicting identities: {path}")
        files[normalized] = row if prior is None or prior.size is None else prior

    def visit(value: object) -> None:
        if isinstance(value, Mapping):
            if set(value) == {"root", "tree", "records"}:
                root = Path(str(value["root"]))
                if not root.is_absolute():
                    raise ValueError("SDK frozen root is not absolute")
                for relative, kind, size, digest in value["records"]:
                    if kind == "file":
                        add(str(root / relative), digest, size)
                return
            if {"root", "files", "directories", "manifest_sha256"}.issubset(value):
                # Strict directory custody records relative members once. Bind
                # them to their owned root here, not in a second provider manifest.
                raw_root = value["root"]
                members = value["files"]
                directories = value["directories"]
                if (
                    not isinstance(raw_root, str)
                    or not Path(raw_root).is_absolute()
                    or not isinstance(members, list)
                    or not isinstance(directories, list)
                    or type(value.get("file_count")) is not int
                    or value["file_count"] != len(members)
                ):
                    raise ValueError(
                        "owned directory has malformed frozen file custody"
                    )
                root = Path(raw_root)
                if str(root.resolve(strict=False)) != raw_root:
                    raise ValueError("owned directory frozen root is not canonical")

                def member_path(relative: object) -> Path:
                    if (
                        not isinstance(relative, str)
                        or not relative
                        or "\\" in relative
                        or "\0" in relative
                        or any(part in {"", ".", ".."} for part in relative.split("/"))
                        or Path(relative).is_absolute()
                        or re.match(r"^[A-Za-z]:", relative) is not None
                    ):
                        raise ValueError(
                            "owned directory has invalid relative member path"
                        )
                    path = root.joinpath(*relative.split("/"))
                    if not path.resolve(strict=False).is_relative_to(root):
                        raise ValueError(
                            "owned directory relative member escapes its root"
                        )
                    return path

                for directory in directories:
                    member_path(directory)
                for member in members:
                    if not isinstance(member, Mapping):
                        raise ValueError("owned directory frozen member is malformed")
                    path = member_path(member.get("relative_path"))
                    size, digest = member.get("size"), member.get("sha256")
                    if (
                        type(size) is not int
                        or size < 0
                        or not isinstance(digest, str)
                        or re.fullmatch(r"[0-9a-f]{64}", digest) is None
                    ):
                        raise ValueError(
                            "owned directory frozen member identity is malformed"
                        )
                    add(str(path), digest, size)
                return
            if value.get("schema") == "molt.proof-python-toolchain.v3":
                custody = value.get("file_custody")
                if not isinstance(custody, list) or not custody:
                    raise ValueError("Python capture has no frozen file custody")
                for row in custody:
                    if not isinstance(row, Mapping):
                        raise ValueError(
                            "Python capture has malformed frozen file custody"
                        )
                    path = row.get("path")
                    size = row.get("size")
                    digest = row.get("sha256")
                    if (
                        not isinstance(path, str)
                        or not Path(path).is_absolute()
                        or not isinstance(size, int)
                        or isinstance(size, bool)
                        or size < 0
                        or not isinstance(digest, str)
                        or re.fullmatch(r"[0-9a-f]{64}", digest) is None
                    ):
                        raise ValueError(
                            "Python capture has malformed frozen file custody"
                        )
            path = value.get("resolved_path") or value.get("path")
            add(path, value.get("sha256"), value.get("size", value.get("size_bytes")))
            add(value.get("executable"), value.get("executable_sha256"))
            add(value.get("path"), value.get("launcher_sha256"))
            add(value.get("content_path"), value.get("executable_sha256"))
            for nested in value.values():
                visit(nested)
        elif isinstance(value, (list, tuple)):
            for nested in value:
                visit(nested)

    visit(payload)
    return [files[key] for key in sorted(files)]


def _compact_process_inventory(identity: Mapping[str, object]) -> dict[str, object]:
    """Project image custody without duplicating its full CAS-owned inventory.

    Process admission consumes the full capture, not these bounded summaries.
    Counts aid inspection; digests bind every row and all selection telemetry.
    """
    summaries: dict[str, object] = {}
    for name, expected in (
        ("process_images", list),
        ("process_image_inventories", list),
        ("link_selection", Mapping),
    ):
        if name not in identity:
            continue
        value = identity[name]
        if not isinstance(value, expected):
            raise ValueError(f"toolchain {name} has malformed inventory")
        assert isinstance(value, (list, Mapping))
        summaries[name] = {
            "count": len(value),
            "semantic_sha256": canonical_json_sha256(value),
        }
    return summaries


def _compact_python(identity: Mapping[str, object]) -> dict[str, object]:
    compact: dict[str, object] = {
        key: value
        for key, value in identity.items()
        if key
        not in {"environment", "process_images", "file_custody", "inventory_profile"}
        and not isinstance(value, (dict, list))
    }
    environment = identity.get("environment")
    if not isinstance(environment, Mapping):
        raise ValueError("python toolchain has no environment closure")
    environment = cast(Mapping[str, object], environment)
    environment_summary: dict[str, object] = {
        key: value
        for key, value in environment.items()
        if key
        in {
            "schema",
            "implementation",
            "version",
            "cache_tag",
            "soabi",
            "operating_system",
            "architecture",
            "pointer_bits",
            "gil_disabled",
            "environment_closure_sha256",
            "distribution_inventory_sha256",
        }
    }
    runtime = environment.get("runtime")
    if isinstance(runtime, Mapping):
        environment_summary["runtime"] = {
            key: value
            for key, value in runtime.items()
            if not isinstance(value, (dict, list))
        }
    distributions = environment.get("distributions")
    if not isinstance(distributions, list):
        raise ValueError("python toolchain has no distribution inventory")
    compact_distributions: list[dict[str, object]] = []
    for distribution in distributions:
        if not isinstance(distribution, Mapping):
            raise ValueError("python toolchain has a malformed distribution")
        external = distribution.get("external_source")
        if external is None:
            continue
        if not isinstance(external, Mapping):
            raise ValueError("python toolchain has a malformed external source")
        # Eligibility consumes editable-source ownership. The complete package
        # inventory is already bound by its digest and retained in the CAS blob.
        row = {
            key: distribution.get(key)
            for key in (
                "name",
                "version",
                "file_manifest_sha256",
                "direct_url_sha256",
                "record_sha256",
            )
            if distribution.get(key) is not None
        }
        row["external_source"] = dict(external)
        compact_distributions.append(row)
    environment_summary["distribution_count"] = len(distributions)
    environment_summary["distributions"] = compact_distributions
    external_roots = environment.get("external_roots")
    environment_summary["external_roots"] = (
        [dict(row) for row in external_roots if isinstance(row, Mapping)]
        if isinstance(external_roots, list)
        else []
    )
    compact["environment"] = environment_summary
    location = identity.get("location")
    if not isinstance(location, Mapping):
        raise ValueError("python toolchain has no pre-arm location receipt")
    compact["location"] = dict(location)
    compact["executable"] = location.get("selected_executable")
    compact["implementation"] = environment.get("implementation")
    compact["version"] = environment.get("version")
    profile = identity.get("inventory_profile")
    if not isinstance(profile, Mapping):
        raise ValueError("python toolchain has no capture profile")
    compact["inventory_profile"] = dict(profile)
    compact.update(_compact_process_inventory(identity))
    return compact


def compact_toolchains(toolchains: Mapping[str, object]) -> dict[str, object]:
    from tools import proof_plan

    policies = {p.name: p for p in proof_plan.ProofPlan.load().toolchain_policies}
    summaries: dict[str, object] = {}
    for name, raw in toolchains.items():
        if not isinstance(raw, Mapping):
            raise ValueError(f"toolchain {name!r} has malformed identity")
        policy = policies.get(name)
        role = policy.python_identity_role if policy is not None else None
        if role == "environment":
            summaries[name] = _compact_python(cast(Mapping[str, object], raw))
        elif role == "runtime":
            selected = raw.get("python_runtime")
            if not isinstance(selected, Mapping):
                raise ValueError(f"{name}: incomplete selected Python runtime custody")
            proof_plan.validate_selected_python_runtime(policy, selected)
            summary = {
                key: value
                for key, value in raw.items()
                if not isinstance(value, (dict, list))
            }
            summary["python_runtime"] = {
                "launch_argv": selected["launch_argv"],
                "runtime_closure_sha256": selected["runtime"]["runtime_closure_sha256"],
                "file_count": len(selected["file_custody"]),
                "node_count": len(selected["node_custody"]),
                "startup_selection_sha256": canonical_json_sha256(
                    selected["startup_selection"]
                ),
            }
            summary.update(_compact_process_inventory(raw))
            summaries[name] = summary
        else:
            summary = {
                key: value
                for key, value in raw.items()
                if not isinstance(value, (dict, list))
            }
            summary.update(_compact_process_inventory(raw))
            summaries[name] = summary
    return summaries


def _validate_captured_toolchains(toolchains: Mapping[str, object]) -> None:
    """Full capture obligation follows declared roles, including absent fields."""
    from molt.exact_json import canonical_json_sha256
    from tools import proof_plan

    plan = proof_plan.ProofPlan.load()
    policies = {policy.name: policy for policy in plan.toolchain_policies}
    for name, raw in toolchains.items():
        if not isinstance(raw, Mapping):
            raise ValueError("captured toolchain identity is malformed")
        policy = policies.get(name)
        if policy is not None and policy.python_identity_role == "runtime":
            from tools.proof_queue_pkg.command_identity import (
                _validate_toolchain_identity,
            )

            _validate_toolchain_identity(plan, name, raw, full_capture=True)
        validate_llvm_sdk_selection(
            raw,
            required=policy is not None
            and policy.data.get("compiler_tool_selector") == "llvm-sdk",
        )
        if name == "rustc":
            validate_rust_link_selection(raw, full_capture=True)
            validate_clippy_selection(raw)
        if policy is not None and policy.data.get("node_package") is not None:
            from tools.proof_queue_pkg.command_identity import (
                _validate_node_package_identity,
            )

            _validate_node_package_identity(policy, raw, full_capture=True)
        if (
            policy is not None
            and policy.data.get("identity_provider") == "source-extension"
        ):
            from tools.proof_queue_pkg import target_derived_toolchains

            target_derived_toolchains.validate_identity(policy, raw, full_capture=True)
        required = policy is not None and sdk_required(policy.data, raw)
        if required:
            validate_wasi_sdk_closure(
                raw, full_capture=True, selected_role=policy.data.get("wasi_sdk_tool")
            )
            material = dict(raw)
            digest = material.pop("identity_sha256", None)
            if digest != canonical_json_sha256(material):
                raise ValueError("captured SDK toolchain digest is invalid")
        elif raw.get("wasi_sdk") is not None:
            raise ValueError("non-SDK toolchain has unexpected SDK custody")


def _store_sdk_resources(
    cas_root: Path, toolchains: Mapping[str, object]
) -> dict[str, object]:
    """Factor actual SDK inventories once in the existing custody CAS."""
    from molt.exact_json import canonical_json_sha256

    result: dict[str, object] = {}
    references: dict[str, dict[str, object]] = {}
    for name, raw in toolchains.items():
        identity = dict(cast(Mapping[str, object], raw))
        sdk = identity.get("wasi_sdk")
        if isinstance(sdk, Mapping):
            selection = {key: value for key, value in sdk.items() if key != "resources"}
            key = canonical_json_sha256(selection)
            if key not in references:
                references[key] = custody_cas.put_json(
                    cas_root,
                    {
                        "schema": custody_cas.ARTIFACT_SCHEMA,
                        "kind": _SDK_RESOURCE_CAPTURE_KIND,
                        "selection_sha256": key,
                        "resources": sdk["resources"],
                    },
                ).as_dict()
            identity["wasi_sdk"] = {
                **selection,
                "resources": {"artifact": references[key]},
            }
        result[name] = identity
    return result


def _load_sdk_resources(
    cas_root: Path, toolchains: Mapping[str, object]
) -> dict[str, object]:
    from molt.exact_json import canonical_json_sha256

    result: dict[str, object] = {}
    resources: dict[str, object] = {}
    for name, raw in toolchains.items():
        if not isinstance(raw, Mapping):
            raise ValueError("captured toolchain identity is malformed")
        identity = dict(raw)
        sdk = identity.get("wasi_sdk")
        if isinstance(sdk, Mapping):
            selection = {key: value for key, value in sdk.items() if key != "resources"}
            packed = sdk.get("resources")
            if (
                not isinstance(packed, Mapping)
                or set(packed) != {"artifact"}
                or not isinstance(packed["artifact"], Mapping)
            ):
                raise ValueError("SDK resource capture reference is incomplete")
            reference = packed["artifact"]
            key = canonical_json_sha256(reference)
            if key not in resources:
                payload = custody_cas.read_ref(reference, expected_root=cas_root)
                if (
                    set(payload) != {"schema", "kind", "selection_sha256", "resources"}
                    or payload["schema"] != custody_cas.ARTIFACT_SCHEMA
                    or payload["kind"] != _SDK_RESOURCE_CAPTURE_KIND
                    or payload["selection_sha256"] != canonical_json_sha256(selection)
                ):
                    raise ValueError(
                        "SDK resource capture belongs to another generation"
                    )
                resources[key] = (payload["selection_sha256"], payload["resources"])
            generation, inventory = resources[key]
            if generation != canonical_json_sha256(selection):
                raise ValueError("SDK resource reference selects another generation")
            identity["wasi_sdk"] = {**selection, "resources": inventory}
        result[name] = identity
    _validate_captured_toolchains(result)
    return result


def publish_capture(
    cas_root: Path, toolchains: Mapping[str, object]
) -> tuple[dict[str, object], dict[str, object], dict[str, object]]:
    started = time.perf_counter()
    _validate_captured_toolchains(toolchains)
    files = frozen_files(toolchains)
    artifact_payload: dict[str, object] = {
        "schema": custody_cas.ARTIFACT_SCHEMA,
        "kind": CAPTURE_SCHEMA,
        "toolchains": _store_sdk_resources(cas_root, toolchains),
        "files": [row.as_dict() for row in files],
    }
    reference = custody_cas.put_json(cas_root, artifact_payload).as_dict()
    summaries = compact_toolchains(toolchains)
    telemetry = {
        "schema": "molt.proof-toolchain-capture-telemetry.v1",
        "full_capture_count": 1,
        "frozen_file_count": len(files),
        "artifact_compressed_bytes": reference["compressed_bytes"],
        "artifact_uncompressed_bytes": reference["uncompressed_bytes"],
        "publish_s": time.perf_counter() - started,
    }
    return summaries, reference, telemetry


def load_capture(
    reference: Mapping[str, object],
    *,
    cas_root: Path,
    envelope: Mapping[str, object] | None = None,
) -> dict[str, object]:
    payload = custody_cas.read_ref(reference, expected_root=cas_root)
    if payload.get("kind") != CAPTURE_SCHEMA:
        raise ValueError("proof toolchain capture artifact kind mismatch")
    toolchains = payload.get("toolchains")
    files = payload.get("files")
    if not isinstance(toolchains, dict) or not isinstance(files, list):
        raise ValueError("proof toolchain capture artifact is incomplete")
    expanded = _load_sdk_resources(cas_root, toolchains)
    if envelope is not None and "rustc" in expanded:
        validate_clippy_selection(
            expanded["rustc"], required=command_requires_clippy(envelope)
        )
    if files != [row.as_dict() for row in frozen_files(expanded)]:
        raise ValueError("proof toolchain frozen files differ from its full capture")
    payload["toolchains"] = expanded
    return payload


def _rehash(
    row: Mapping[str, object],
) -> tuple[str, str | None, int | None, str | None]:
    raw_path = row.get("path")
    expected = row.get("sha256")
    if not isinstance(raw_path, str) or not isinstance(expected, str):
        return str(raw_path), None, None, "malformed-row"
    path = Path(raw_path)
    try:
        stat = path.stat()
        with path.open("rb") as stream:
            actual = hashlib.file_digest(stream, "sha256").hexdigest()
    except OSError as exc:
        return raw_path, None, None, type(exc).__name__
    return raw_path, actual, stat.st_size, None


def verify_capture(
    reference: Mapping[str, object], *, workers: int, cas_root: Path
) -> dict[str, object]:
    started = time.perf_counter()
    payload = load_capture(reference, cas_root=cas_root)
    rows = payload["files"]
    assert isinstance(rows, list)
    if workers < 1:
        raise ValueError("toolchain verification workers must be positive")
    with ThreadPoolExecutor(max_workers=workers) as executor:
        actual = list(executor.map(_rehash, rows))
    mismatches: list[dict[str, object]] = []
    bytes_hashed = 0
    for row, (path, digest, size, error) in zip(rows, actual, strict=True):
        assert isinstance(row, Mapping)
        if isinstance(size, int):
            bytes_hashed += size
        expected_size = row.get("size")
        if (
            error is not None
            or digest != row.get("sha256")
            or (isinstance(expected_size, int) and size != expected_size)
        ):
            mismatches.append(
                {
                    "path": path,
                    "expected_sha256": row.get("sha256"),
                    "actual_sha256": digest,
                    "expected_size": expected_size,
                    "actual_size": size,
                    "error": error,
                }
            )
    material = {
        "capture_semantic_sha256": reference.get("semantic_sha256"),
        "verified_file_count": len(rows),
        "bytes_hashed": bytes_hashed,
        "mismatches": mismatches,
    }
    return {
        "schema": VERIFICATION_SCHEMA,
        **material,
        "stable": not mismatches,
        "verification_s": time.perf_counter() - started,
        "identity_sha256": hashlib.sha256(
            json.dumps(material, sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest(),
    }


_SDK_RESOURCE_CAPTURE_KIND = "molt.proof-wasi-sdk-resources.v2"


def sdk_required(data: Mapping[str, object], identity: Mapping[str, object]) -> bool:
    """An admitted role/target requires SDK custody even if its fields are missing."""
    if data.get("wasi_sdk_tool") is not None:
        return True
    if data.get("identity_provider") == "source-extension":
        from molt.cli.source_extension_target import (
            source_extension_recorded_target_plan,
        )

        requested, triple = identity.get("target"), identity.get("target_triple")
        if not isinstance(requested, str) or not isinstance(triple, str):
            raise ValueError("source-extension recorded target is malformed")
        return (
            source_extension_recorded_target_plan(
                requested, target_triple=triple
            ).target_triple
            == "wasm32-wasip1"
        )
    return False


def capture_wasi_sdk_images(selection: Mapping[str, object]) -> list[dict[str, object]]:
    """Project shared finite SDK file capture into guarded process-image roles."""
    return canonical_images(
        [
            {
                **row,
                "schema": PROCESS_IMAGE_SCHEMA,
                "role": "wasi-sdk-" + str(row["role"]),
                "path_kind": "selection",
            }
            for row in capture_wasi_sdk_tool_files(selection)
        ]
    )


def validate_wasi_sdk_closure(
    identity: Mapping[str, object],
    *,
    full_capture: bool,
    selected_role: str | None = None,
) -> Mapping[str, object]:
    """Validate the exact selection/capture phase; neither phase permits omissions."""
    from molt.wasi_sdk_identity import SDK_RESOURCE_ROOTS

    closure = identity.get("wasi_sdk")
    expected = {"sdk", "receipt", "generation"} | (
        {"resources"} if full_capture else set()
    )
    if not isinstance(closure, Mapping) or set(closure) != expected:
        raise ValueError("WASI SDK closure is malformed or incomplete for its phase")
    selection = validate_wasi_sdk_selection(
        {key: closure[key] for key in ("sdk", "receipt", "generation")}
    )
    sdk = Path(selection["sdk"])
    generation = selection["generation"]
    if selected_role is not None:
        fact = generation["facts"]["tools"].get(selected_role)
        if not isinstance(fact, Mapping):
            raise ValueError("WASI SDK selected tool role is invalid")
        if identity.get("path") != _image_path_key(sdk / fact["path"]):
            raise ValueError("WASI SDK compiler selection differs from its role")
        expected_images = {
            "wasi-sdk-" + role: _image_path_key(
                sdk / generation["facts"]["tools"][role]["path"]
            )
            for role in SDK_TOOL_NAMES
        }
        images = identity.get("process_images")
        if (
            not isinstance(images, list)
            or len(images) != len(expected_images)
            or any(
                not isinstance(row, Mapping) or not isinstance(row.get("role"), str)
                for row in images
            )
            or {row.get("role"): row.get("path") for row in images} != expected_images
        ):
            raise ValueError("WASI SDK process helper closure is incomplete")
        for image in images:
            role = image["role"].removeprefix("wasi-sdk-")
            expected = generation["facts"]["tools"][role]
            if (
                image.get("sha256") != expected["sha256"]
                or image.get("size_bytes") != expected["size"]
            ):
                raise ValueError("WASI SDK process helper content differs from receipt")
    if full_capture:
        resources = closure["resources"]
        if not isinstance(resources, list) or len(resources) != len(SDK_RESOURCE_ROOTS):
            raise ValueError("WASI SDK resource capture is incomplete")
        for relative, resource in zip(SDK_RESOURCE_ROOTS, resources, strict=True):
            _validate_sdk_resource(
                resource,
                root=sdk / relative,
                expected=generation["facts"]["resources"][relative],
            )
    return closure


def _validate_sdk_resource(resource: object, *, root: Path, expected: object) -> None:
    from molt.wasi_sdk_identity import (
        _tree_from_records,
        _record_member,
        MAX_TREE_ENTRIES,
    )
    from molt.portable_paths import portable_relative_path, portable_path_identity

    if (
        not isinstance(resource, Mapping)
        or set(resource) != {"root", "tree", "records"}
        or resource["root"] != str(root)
    ):
        raise ValueError("WASI SDK captured resource root differs from its generation")
    raw = resource["records"]
    if not isinstance(raw, (list, tuple)) or not 0 < len(raw) <= MAX_TREE_ENTRIES:
        raise ValueError("WASI SDK resource records are malformed")
    records = []
    paths = set()
    for row in raw:
        if (
            not isinstance(row, (list, tuple))
            or len(row) != 4
            or not isinstance(row[0], str)
            or row[1] not in {"file", "directory", "link"}
            or type(row[2]) is not int
            or row[2] < 0
            or not isinstance(row[3], str)
        ):
            raise ValueError("WASI SDK resource row is malformed")
        portable_relative_path(row[0])
        key = portable_path_identity(row[0])
        if key in paths:
            raise ValueError("WASI SDK resource has duplicate paths")
        paths.add(key)
        if (
            (row[1] == "file" and re.fullmatch(r"[0-9a-f]{64}", row[3]) is None)
            or (row[1] != "file" and row[2] != 0)
            or (row[1] == "directory" and row[3] != "")
        ):
            raise ValueError("WASI SDK resource content is malformed")
        records.append(tuple(row))
    tree = _tree_from_records(tuple(records))
    if tree.as_record() != resource["tree"] or resource["tree"] != expected:
        raise ValueError("WASI SDK resource differs from its provisioned generation")
    nodes = {row[0]: row for row in tree.records}
    for row in tree.records:
        if row[1] == "link":
            _record_member(
                nodes, row[0]
            )  # Resource directory aliases remain forbidden.


def capture_wasi_sdk_resources(selection: Mapping[str, object]) -> dict[str, object]:
    """Capture once after broad live custody is armed, against the receipt facts."""
    from molt.wasi_sdk_identity import SDK_RESOURCE_ROOTS, wasi_sdk_tree_identity
    from tools.proof_queue_pkg.command_identity import _file_identity

    validate_wasi_sdk_closure({"wasi_sdk": selection}, full_capture=False)
    if (
        _file_identity(
            Path(str(cast(Mapping[str, object], selection["receipt"])["path"]))
        )
        != selection["receipt"]
    ):
        raise ValueError("WASI SDK receipt changed while proof custody armed")
    sdk = Path(str(selection["sdk"]))
    resources = []
    for relative in SDK_RESOURCE_ROOTS:
        tree = wasi_sdk_tree_identity(sdk / relative)
        resources.append(
            {
                "root": str(sdk / relative),
                "tree": tree.as_record(),
                "records": tree.records,
            }
        )
    result = {**selection, "resources": resources}
    validate_wasi_sdk_closure({"wasi_sdk": result}, full_capture=True)
    return result


def revalidate_wasi_sdk_selection(
    identity: Mapping[str, object],
) -> frozenset[FrozenFile]:
    from tools.proof_queue_pkg.command_identity import _file_identity

    closure = validate_wasi_sdk_closure(identity, full_capture=False)
    receipt = cast(Mapping[str, object], closure["receipt"])
    if _file_identity(Path(str(receipt["path"]))) != receipt:
        raise ValueError("WASI SDK receipt changed")
    observed = capture_wasi_sdk_images(closure)
    recorded = identity.get("process_images")
    if (
        isinstance(recorded, list)
        and recorded
        and str(recorded[0].get("role", "")).startswith("wasi-sdk-")
    ):
        if canonical_images(observed) != canonical_images(recorded):
            raise ValueError("WASI SDK process image changed")
    return frozenset(frozen_files({"receipt": receipt, "process_images": observed}))
