"""Compact one-capture toolchain custody and frozen-manifest verification."""

from __future__ import annotations

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
from typing import TYPE_CHECKING, Iterable, Mapping, Sequence, cast

from molt.exact_json import canonical_json_sha256
from molt.toolchain_identity import (
    executable_environment_value,
    find_executable,
    stable_regular_file_identity,
    verify_stable_regular_file_identity,
)
from molt.wasi_sdk_identity import (
    SDK_BUILD_TOOL_NAMES,
    capture_wasi_sdk_tool_files,
    validate_wasi_sdk_selection,
)
from molt.rust_toolchain import (
    RustToolSearch,
    RustupProxyUnavailable,
    cargo_config_arguments,
    resolve_rustup_proxy,
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
    env: Mapping[str, str],
    timeout: float,
    probes: list[dict[str, object]],
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
            env=dict(env),
            check=False,
            capture_output=True,
            text=True,
            timeout=timeout,
            encoding="utf-8",
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
    `COLLECT_GCC_OPTIONS=`, ...) start at column zero.
    """
    commands: list[list[str]] = []
    for line in output.splitlines():
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


def revalidate_rust_artifact_manifests(selection: Mapping[str, object]) -> None:
    """Reuse checks finite inputs; armed custody checks these frozen files."""
    for unit in selection["units"]:
        context = unit.get("artifact_context")
        if context is None:
            continue
        _cargo_artifact_crate_types(context, selection["producer_command"])
        for expected in context["input_files"]:
            actual = stable_regular_file_identity(
                Path(expected["path"]), label="Cargo artifact input"
            )
            if actual.sha256 != expected["sha256"] or actual.size != expected["size"]:
                raise ValueError(
                    "Cargo artifact manifest/source changed since selection"
                )


def select_cargo_native_c_units(
    *,
    required: Sequence[str],
    target: str | None,
    host: str,
    cwd: Path,
    env: Mapping[str, str],
) -> list[dict[str, object]]:
    """Resolve only declared native units, without compiler probes or hashing."""
    from molt.cli.runtime_cargo_plan import (
        _CargoEnvironment,
        _c_tool_environment_names,
        _resolve_c_build_resources,
        runtime_c_flag_tokens,
        runtime_c_tool_selection,
    )

    if any(unit not in {"target", "host"} for unit in required) or len(
        set(required)
    ) != len(required):
        raise ValueError("native C units must be unique target/host roles")
    result: list[dict[str, object]] = []
    selected_env = _CargoEnvironment(env)
    by_target: dict[str, dict[str, object]] = {}
    for unit in required:
        triple = (target or host) if unit == "target" else host
        if triple.startswith("wasm"):
            raise ValueError("managed WASI C tools are not native C build units")
        if triple in by_target:
            by_target[triple]["units"].append(unit)
            continue
        tools = {}
        for role in ("cc", "ar"):
            names = _c_tool_environment_names(role, target=triple, host_target=host)
            selected = next(
                (selected_env[name] for name in names if selected_env.get(name)), None
            )
            if (
                selected is not None
                and not Path(selected).is_absolute()
                and any(sep in selected for sep in ("/", "\\"))
            ):
                raise ValueError("native C selector has unresolved build-script cwd")
            path = runtime_c_tool_selection(
                role, root=cwd, env=selected_env, target=triple, host_target=host
            )
            if path is None:
                raise ValueError(f"native C unit {triple} requires an explicit {role}")
            tools[role] = str(path)
        # The same flag/resource grammar used by ordinary Cargo owns rejection
        # of response/plugin/prefix and package-relative input ambiguities.
        resources = _resolve_c_build_resources(
            selected_env, target=triple, host_target=host
        )
        shell = selected_env.get("CC_SHELL_ESCAPED_FLAGS", "") not in {
            "",
            "0",
            "false",
            "no",
        }
        flags = [
            token
            for name in reversed(
                _c_tool_environment_names("cflags", target=triple, host_target=host)
            )
            for token in runtime_c_flag_tokens(
                selected_env.get(name, ""), shell_escaped=shell
            )
        ]
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
                "native C compiler wrapper requires executable child custody"
            )
        record = {
            "units": [unit],
            "target": triple,
            "compiler": [tools["cc"], *flags],
            "archiver": tools["ar"],
            "resource_roots": sorted({str(root.path) for root in resources.roots}),
        }
        result.append(record)
        by_target[triple] = record
    return result


def native_c_environment(selection: Mapping[str, object]) -> dict[str, str]:
    """Publish recorded selection without selecting tools a second time."""
    updates: dict[str, str] = {}
    for row in selection.get("native_c", []):
        triple = row["selection"]["target"]
        for role, value in (
            ("CC", row["selection"]["compiler"][0]),
            ("AR", row["selection"]["archiver"]),
        ):
            for form in (triple, triple.replace("-", "_").replace(".", "_")):
                updates[f"{role}_{form}"] = value
    return updates


def _compiler_phase_languages(language: str, target: str) -> tuple[str, ...]:
    # WebAssembly has no GNU assembly translation unit. Its native LLVM
    # provider still exposes the actual C/C++ frontend helper phase.
    return (
        (language,) if target.startswith("wasm") else (language, "assembler-with-cpp")
    )


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
    from molt.cli.compiler_target import is_zig_compiler_command
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
    native_c_units: Sequence[str] = (),
    admitted_command: Sequence[str] | None = None,
) -> tuple[list[dict[str, object]], dict[str, object]]:
    """Capture both Cargo target and host-unit linker families exactly once."""
    if not command_argv:
        raise ValueError("Rust link capture requires its actual command")
    from tools.proof_queue_pkg import command_admission

    admitted = list(command_argv if admitted_command is None else admitted_command)
    admitted_envelope = command_admission.envelope_for_command(admitted)
    cargo_invocation = (
        command_admission.cargo_invocation_for_envelope(admitted_envelope)
        if "cargo" in admitted_envelope["toolchains"]
        else None
    )
    compiler_probes: list[dict[str, object]] = []

    def metadata(arguments: list[str]) -> str:
        result = _run_rust_link_probe(
            [str(rustc), *arguments],
            phase="compiler-metadata",
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
        return result.stdout

    try:
        host = rustc_host(
            rustc_version if rustc_version is not None else metadata(["-vV"])
        )
        compiler_sysroot = rustc_printed_sysroot(
            metadata(["--print", "sysroot"]), cwd=cwd
        )
    except RustLinkCaptureError:
        raise
    except Exception as exc:
        raise RustLinkCaptureError(
            str(exc), unit="compiler", probes=compiler_probes
        ) from exc
    native_selections = select_cargo_native_c_units(
        required=native_c_units, target=target, host=host, cwd=cwd, env=env
    )
    if any(row["target"] != host for row in native_selections):
        raise ValueError(
            "native C cross-target proof requires an effective cc-rs compiler command; CC/AR paths alone do not attest target flags"
        )
    units = ("target", "host-proc-macro") if cargo is not None else ("target",)
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
                probes=probes,
            )
        except RustLinkCaptureError:
            raise
        except Exception as exc:
            raise RustLinkCaptureError(str(exc), unit=unit, probes=probes) from exc
        images.extend(selected_images)
        selections.append(selection)
    native_c = []
    for selection in native_selections:
        role = "rust-build-native-c-" + selection["target"]
        selected_images, compiler = capture_native_compiler_process_images(
            selection["compiler"],
            role=role,
            language="c",
            target=selection["target"],
            cwd=cwd,
            env=env,
            captured_images=images,
        )
        archiver = capture_image(
            role + "-archiver", Path(selection["archiver"]), preserve_path=True
        )
        images.extend([*selected_images, archiver])
        native_c.append(
            {"selection": selection, "compiler": compiler, "resources": None}
        )
    images = canonical_images(images)
    return images, {
        "schema": "molt.proof-rust-link-selection-telemetry.v4",
        "producer_command": list(command_argv),
        "admitted_command": admitted,
        "native_c_required": list(native_c_units),
        "native_c": native_c,
        "target": target,
        "compiler_host": host,
        "selection_probe_count": len(units),
        "selected_process_count": len(images),
        "units": selections,
        "command_semantics_sha256": canonical_json_sha256(list(command_argv)),
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
    probes: list[dict[str, object]],
) -> tuple[list[dict[str, object]], dict[str, object]]:
    """Ask the selected Rust toolchain to link a zero-dependency synthetic crate.

    The probe runs outside the repository source/target and uses the exact captured
    environment and Cargo configuration. It executes no repository build script.
    `--print link-args` supplies the actual selected driver argv; compiler-driver
    dry-run output then exposes internally selected linker helpers such as mold.
    """
    from tools.proof_queue_pkg import command_admission

    started = time.perf_counter()
    probe_env = dict(env)
    # Cargo's synthetic and forwarded-context probes must use the same retained
    # compiler as metadata and payload, never select another Rustup component.
    if cargo is not None:
        from molt.rust_toolchain import rust_toolchain_library_environment

        probe_env["RUSTC"] = str(rustc)
        probe_env.update(rust_toolchain_library_environment(rustc, probe_env))
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
    )
    cargo_crate_types = artifact_selection["cargo_crate_types"]
    explicit_library = bool(cargo_crate_types or base) and "bin" not in (
        cargo_crate_types or base
    )
    with tempfile.TemporaryDirectory(prefix="molt-rust-link-capture-") as raw_root:
        root = Path(raw_root).resolve()
        compiler_cwd = root if cargo is not None else cwd
        forwarded_context: dict[str, object] | None = None
        path_projections: list[dict[str, object]] = []
        config_files: list[dict[str, object]] = []
        source = root / ("host.rs" if unit == "host-proc-macro" else "main.rs")
        source.write_text(
            "extern crate proc_macro;\n"
            if unit == "host-proc-macro"
            else "fn main() {}\n#[test]\nfn proof_link_test() {}\n",
            encoding="utf-8",
        )
        output = root / ("probe.exe" if os.name == "nt" else "probe")
        if cargo is not None:
            manifest = root / "Cargo.toml"
            feature_names: set[str] = set()
            custom_profiles: set[str] = set()
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
            forwarded = command[separator + 1 :] if separator < len(command) else []
            index = 0
            while index < len(cargo_args):
                value = cargo_args[index]
                if value in {"--release", "--all-features", "--no-default-features"}:
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
                        cargo_profile_args.extend((value, argument))
                        if argument not in {"dev", "release", "test", "bench"}:
                            if not argument or not all(
                                character.isalnum() or character in "_-"
                                for character in argument
                            ):
                                raise ValueError("Cargo profile name is not canonical")
                            custom_profiles.add(argument)
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
                    cargo_profile_args.append(value)
                    profile = value.split("=", 1)[1]
                    if profile not in {"dev", "release", "test", "bench"}:
                        if not profile or not all(
                            character.isalnum() or character in "_-"
                            for character in profile
                        ):
                            raise ValueError("Cargo profile name is not canonical")
                        custom_profiles.add(profile)
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
            rustc_link_args = list(command_admission.rust_link_arguments(forwarded))
            relative_paths = (
                relative_rustc_tool_paths(rustc_link_args) if unit == "target" else []
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
            profile_table = "".join(
                f'\n[profile.{name}]\ninherits="release"\n'
                for name in sorted(custom_profiles)
            )
            target_table = (
                '[lib]\nproc-macro=true\npath="host.rs"\n'
                if unit == "host-proc-macro"
                else '[lib]\npath="main.rs"\n'
                if explicit_library
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
                + feature_table
                + profile_table,
                encoding="utf-8",
            )
            probe_env["CARGO_TARGET_DIR"] = str(root / "target")
            probe_env["CARGO_INCREMENTAL"] = "0"
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
            command.extend(config_args)
            command.extend(
                ("--lib",)
                if unit == "host-proc-macro" or explicit_library
                else ("--bin", "molt_link_capture")
            )
            if unit == "target" and cargo_crate_types is not None:
                command.extend(("--crate-type", ",".join(cargo_crate_types)))
            command.extend(
                (
                    "--",
                    *(rustc_link_args if unit == "target" else ()),
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
            command.extend(command_admission.rust_link_arguments(command_argv[1:]))
        # rustc print_crate_info stops before compilation if ANY metadata-only
        # print is requested, even beside link-args. Keep the two command phases
        # disjoint for Cargo and direct rustc. Cargo fingerprints extra_args_for
        # the unit, so the changed print request is not a freshness retry.
        metadata_command = [*command, "--print", "sysroot"]
        # The driver dry run below re-reads this command's inputs; save-temps
        # keeps rustc's temporaries (symbols.o, codegen units) inside the
        # synthetic root, since clang rejects missing inputs even under -###.
        command = [*command, "-C", "save-temps", "--print", "link-args"]
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
        completed = _run_rust_link_probe(
            command,
            phase="link-selection",
            unit=unit,
            cwd=cwd,
            compiler_cwd=compiler_cwd,
            env=probe_env,
            timeout=120.0,
            probes=probes,
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
                    "archive-only Rust operation unexpectedly selected a linker"
                )
            return [], {
                "schema": "molt.proof-rust-link-unit.v2",
                "unit": unit,
                "artifact_selection": artifact_selection,
                "artifact_context": artifact_context,
                "process_image_refs": [],
                "process_resolution": [],
                "selected_process_count": 0,
                "compiler_cwd": str(compiler_cwd),
                "forwarded_compiler_context": forwarded_context,
                "path_projections": path_projections,
                "configuration_files": config_files,
                "target": target,
                "probe": "cargo-rustc" if cargo is not None else "rustc",
                "selection_probe_count": 1,
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
            image = capture_image(
                "rust-linker" if index == 0 else "rust-link-helper",
                path,
                root_exit_disposition=(
                    "terminate"
                    if _image_path_key(path) in auxiliary_keys
                    else "require-exit"
                ),
            )
            images.append(image)
            if str(path.absolute()) != image["path"]:
                images.append(
                    capture_image(
                        str(image["role"]),
                        path,
                        root_exit_disposition=str(
                            image.get("root_exit_disposition", "require-exit")
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
            image = capture_image(role, tool, root_exit_disposition="require-exit")
            images.append(image)
            if _image_path_key(tool) != image["path"]:
                images.append(capture_image(role, tool, preserve_path=True))
        images = canonical_images(images)
        telemetry = {
            "schema": "molt.proof-rust-link-unit.v2",
            "unit": unit,
            "artifact_selection": artifact_selection,
            "artifact_context": artifact_context,
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


def validate_rust_link_selection(
    identity: Mapping[str, object],
    *,
    required_native_c: Sequence[str] | None = None,
    full_capture: bool = False,
    command_argv: Sequence[str] | None = None,
) -> dict[str, object]:
    """One structural receiver for Rust link and declared native C images."""
    from tools.proof_queue_pkg import command_admission

    raw = identity.get("link_selection")
    if (
        not isinstance(raw, Mapping)
        or raw.get("schema") != "molt.proof-rust-link-selection-telemetry.v4"
    ):
        raise ValueError("Rust linker selection telemetry schema mismatch")
    telemetry = dict(raw)
    units = telemetry.get("units")
    if (
        not isinstance(units, list)
        or not units
        or any(not isinstance(row, Mapping) for row in units)
        or [row.get("unit") for row in units]
        not in (["target"], ["target", "host-proc-macro"])
        or telemetry.get("selection_probe_count") != len(units)
        or any(row.get("selection_probe_count") != 1 for row in units)
    ):
        raise ValueError("Rust linker units must each be selected exactly once pre-arm")
    admitted = telemetry.get("admitted_command")
    if (
        not isinstance(admitted, list)
        or not admitted
        or any(not isinstance(value, str) or not value for value in admitted)
    ):
        raise ValueError("Rust linker capture has no admitted command")
    admitted_envelope = command_admission.envelope_for_command(admitted)
    invocation = (
        command_admission.cargo_invocation_for_envelope(admitted_envelope)
        if "cargo" in admitted_envelope["toolchains"]
        else None
    )
    declared = admitted_envelope["cargo_native_c_units"]
    cargo = "cargo" in admitted_envelope["toolchains"]
    if [row["unit"] for row in units] != (
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
    payload_envelope = admitted_envelope.get("delegated") or admitted_envelope
    submitted = payload_envelope["argv"]
    python = payload_envelope.get("python")
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
            if command_admission._PYTHON_COMMAND.fullmatch(name)
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
    for row in units:
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
        )
        if (
            row.get("schema") != "molt.proof-rust-link-unit.v2"
            or row.get("artifact_selection") != artifact
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
                "archive-only Rust operation has unexpected linker selection"
            )
    if telemetry.get("native_c_required") != declared or (
        required_native_c is not None and list(required_native_c) != declared
    ):
        raise ValueError("Rust native C requirement differs from admitted command")
    host, target = telemetry.get("compiler_host"), telemetry.get("target")
    if (
        not isinstance(host, str)
        or not host
        or (target is not None and not isinstance(target, str))
    ):
        raise ValueError("Rust linker capture has no selected compiler host/target")
    native = telemetry.get("native_c")
    if not isinstance(native, list):
        raise ValueError("Rust native C unit capture is missing")
    expected: dict[str, list[str]] = {}
    for unit in declared:
        triple = (target or host) if unit == "target" else host
        if triple != host:
            raise ValueError("native C unit lacks an effective cross-target command")
        expected.setdefault(triple, []).append(unit)
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
    seen: dict[str, list[str]] = {}
    for row in native:
        if not isinstance(row, Mapping) or set(row) != {
            "selection",
            "compiler",
            "resources",
        }:
            raise ValueError("native C unit record is malformed")
        selection = row["selection"]
        if not isinstance(selection, Mapping) or set(selection) != {
            "units",
            "target",
            "compiler",
            "archiver",
            "resource_roots",
        }:
            raise ValueError("native C unit selection is malformed")
        triple = selection["target"]
        if not isinstance(triple, str) or triple in seen:
            raise ValueError("native C units must select each target once")
        seen[triple] = selection["units"]
        command = selection["compiler"]
        if (
            not isinstance(command, list)
            or not command
            or any(not isinstance(value, str) or not value for value in command)
            or not Path(command[0]).is_absolute()
        ):
            raise ValueError("native C compiler selection is malformed")
        role = "rust-build-native-c-" + triple
        validate_native_compiler_capture(
            row["compiler"],
            images,
            command=command,
            language="c",
            target=triple,
            role=role,
        )
        archivers = [image for image in images if image["role"] == role + "-archiver"]
        if (
            len(archivers) != 1
            or not isinstance(selection["archiver"], str)
            or not Path(selection["archiver"]).is_absolute()
            or _image_path_key(Path(archivers[0]["path"]))
            != _image_path_key(Path(selection["archiver"]))
        ):
            raise _image_membership_error(
                "native C independent archiver custody is incomplete",
                unit=triple,
                role=role + "-archiver",
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
            raise ValueError("native C resource roots are malformed")
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
            raise ValueError("native C resource custody is incomplete")
        from tools.proof_queue_pkg.command_identity import (
            _validate_directory_manifest_identity,
        )

        for resource in resources:
            selected_root = resource["selected_root"]
            if (
                not isinstance(selected_root, str)
                or not Path(selected_root).is_absolute()
            ):
                raise ValueError("native C resource selection is not absolute")
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
                raise ValueError("native C file resource lacks bound content custody")
    if seen != expected:
        raise ValueError("native C build-unit custody is incomplete")
    native_roles = {
        "rust-build-native-c-" + triple + suffix
        for triple in expected
        for suffix in ("", "-archiver")
    }
    if {
        row["role"]
        for row in images
        if str(row["role"]).startswith("rust-build-native-c-")
    } != native_roles:
        raise ValueError("native C process roles differ from declared build units")
    selected = [
        row
        for row in images
        if row["role"] in {"rust-linker", "rust-link-helper"}
        or str(row["role"]).startswith("rust-build-")
    ]
    expected_unit_keys = {
        (row["role"], _image_path_key(Path(row["path"])))
        for row in selected
        if not str(row["role"]).startswith("rust-build-native-c-")
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


def capture_native_c_resources(selection: Mapping[str, object]) -> dict[str, object]:
    """Materialize selected roots once, after their live watchers have armed."""
    from tools.proof_queue_pkg.command_identity import (
        _directory_manifest_identity,
        _revalidate_directory_manifest_identity,
        _file_identity,
    )

    units = []
    for unit in selection["native_c"]:
        if unit["resources"] is None:
            resources = [
                {
                    "selected_root": path,
                    **(
                        _directory_manifest_identity(
                            Path(path), label="native C resource"
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
                    captured, selected_root=root, label="native C resource"
                )
            elif _file_identity(root) != captured:
                raise ValueError("native C resource changed while live custody armed")
        units.append(dict(unit))
    return {**selection, "native_c": units}


def revalidate_rust_link_process_images(
    selected_identity: Mapping[str, object],
    *,
    target: str | None,
    command_argv: Sequence[str] = (),
    required_native_c: Sequence[str] | None = None,
) -> tuple[list[dict[str, object]], dict[str, object]]:
    """Rehash frozen selections at the armed boundary, without running probes."""
    telemetry = validate_rust_link_selection(
        selected_identity,
        required_native_c=required_native_c,
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
    telemetry = capture_native_c_resources(telemetry)
    validate_rust_link_selection(
        {**selected_identity, "link_selection": telemetry},
        required_native_c=required_native_c,
        full_capture=True,
    )
    for unit in telemetry["units"]:
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
    summaries: dict[str, object] = {}
    for name, raw in toolchains.items():
        if not isinstance(raw, Mapping):
            raise ValueError(f"toolchain {name!r} has malformed identity")
        if name == "python":
            summaries[name] = _compact_python(cast(Mapping[str, object], raw))
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

    policies = {
        policy.name: policy for policy in proof_plan.ProofPlan.load().toolchain_policies
    }
    for name, raw in toolchains.items():
        if not isinstance(raw, Mapping):
            raise ValueError("captured toolchain identity is malformed")
        policy = policies.get(name)
        if name == "rustc":
            validate_rust_link_selection(raw, full_capture=True)
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
    reference: Mapping[str, object], *, cas_root: Path
) -> dict[str, object]:
    payload = custody_cas.read_ref(reference, expected_root=cas_root)
    if payload.get("kind") != CAPTURE_SCHEMA:
        raise ValueError("proof toolchain capture artifact kind mismatch")
    toolchains = payload.get("toolchains")
    files = payload.get("files")
    if not isinstance(toolchains, dict) or not isinstance(files, list):
        raise ValueError("proof toolchain capture artifact is incomplete")
    expanded = _load_sdk_resources(cas_root, toolchains)
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
            for role in SDK_BUILD_TOOL_NAMES
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
