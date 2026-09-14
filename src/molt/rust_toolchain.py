"""Rust-owned host tool lookup for compiler-selected linker commands.

Rust 1.96 Session::get_tools_search_paths uses each selected sysroot's
lib/rustlib/<compiler host>/bin, not the compilation target's bin directory.
Keep this lookup separate from PATH discovery and preserve its provenance.
"""

from __future__ import annotations

from dataclasses import dataclass
import os
from pathlib import Path
import re
from typing import Mapping, Sequence

from molt import process_guard
from molt.toolchain_identity import (
    executable_environment_value,
    find_executable,
    resolve_executable,
    stable_executable_probe,
)


class RustupProxyUnavailable(ValueError):
    """A selected Rustup proxy could not provide the requested component."""

    def __init__(self, role: str, diagnostic: dict[str, object]) -> None:
        super().__init__(f"rustup {role} selection failed: {diagnostic['stderr']}")
        self.diagnostic = diagnostic


def resolve_rustup_proxy(
    path: Path, *, role: str, root: Path, env: Mapping[str, str]
) -> Path:
    """Resolve a content-proven rustup proxy, preserving custom tool binaries."""
    name = "rustup.exe" if os.name == "nt" else "rustup"
    candidates = tuple(
        candidate
        for candidate in dict.fromkeys(
            (path.parent / name, path.resolve(strict=True).parent / name)
        )
        if candidate.is_file()
    )
    if not candidates:
        return path
    with stable_executable_probe(path, label=f"Rust {role} selection") as (
        _,
        selected,
    ):
        for rustup in candidates:
            with stable_executable_probe(rustup, label="rustup selection") as (
                entrypoint,
                proxy,
            ):
                if selected.sha256 != proxy.sha256:
                    continue
                result = process_guard.run_completed_command(
                    [os.fspath(entrypoint), "which", role],
                    cwd=root,
                    env=dict(env),
                    check=False,
                    capture_output=True,
                    text=True,
                    encoding="utf-8",
                    timeout=30,
                    memory_guard_prefix=None,
                )
                if result.returncode != 0:
                    raise RustupProxyUnavailable(
                        role,
                        {
                            "phase": "rustup-component-selection",
                            "unit": role,
                            "argv": [os.fspath(entrypoint), "which", role],
                            "cwd": os.fspath(root),
                            "returncode": result.returncode,
                            "stdout": result.stdout,
                            "stderr": result.stderr,
                        },
                    )
                value = result.stdout.strip()
                if (
                    not value
                    or "\n" in value
                    or "\r" in value
                    or not Path(value).is_absolute()
                ):
                    raise ValueError(
                        f"rustup {role} selector must print one absolute path"
                    )
                return resolve_executable(
                    value, environment=env, label=f"selected Rust {role}"
                )
    return path


def rustc_host(version: str) -> str:
    hosts = re.findall(r"(?m)^host:\s*([A-Za-z0-9_.-]+)\s*$", version)
    if len(hosts) != 1:
        raise ValueError("selected rustc has no unique compiler host triple")
    return hosts[0]


def rustc_printed_sysroot(stdout: str, *, cwd: Path) -> Path:
    lines = [line.strip() for line in stdout.splitlines() if line.strip()]
    if len(lines) != 1:
        raise ValueError("selected rustc has no unique printed sysroot")
    path = Path(lines[0])
    path = path if path.is_absolute() else cwd / path
    if not path.is_dir():
        raise ValueError("selected rustc printed an unavailable sysroot directory")
    return path.resolve(strict=True)


def cargo_config_arguments(command: Sequence[str], *, cwd: Path) -> list[str]:
    """Retain Cargo's invocation-relative --config files and inline TOML."""
    result: list[str] = []
    index = 1
    while index < len(command) and command[index] != "--":
        value = command[index]
        if value == "--config":
            index += 1
            if index == len(command):
                raise ValueError("Cargo --config requires a value")
            argument = command[index]
        elif value.startswith("--config="):
            argument = value.split("=", 1)[1]
        else:
            index += 1
            continue
        if "=" not in argument:
            path = Path(argument)
            path = path if path.is_absolute() else cwd / path
            if not path.is_file():
                raise ValueError(f"Cargo --config file is unavailable: {path}")
            argument = str(path.absolute())
        result.extend(("--config", argument))
        index += 1
    return result


def cargo_configuration_paths(root: Path, env: Mapping[str, str]) -> tuple[Path, ...]:
    """Cargo's conventional configuration files, in low-to-high priority order.

    Explicit --config files are separate invocation inputs. An empty CARGO_HOME
    selects the default home, and Cargo does not expand a literal tilde.
    """
    home_value = executable_environment_value(env, "CARGO_HOME")
    if home_value:
        home = Path(home_value)
    else:
        profile = executable_environment_value(
            env, "USERPROFILE" if os.name == "nt" else "HOME"
        )
        home = (Path(profile) if profile else Path.home()) / ".cargo"
    if not home.is_absolute():
        home = root / home
    locations = (home, *(path / ".cargo" for path in reversed((root, *root.parents))))
    paths: list[Path] = []
    seen: set[Path] = set()
    for location in locations:
        location = location.resolve(strict=False)
        if location in seen:
            continue
        seen.add(location)
        for name in ("config", "config.toml"):
            candidate = location / name
            if candidate.is_file():
                paths.append(candidate)
                break
    return tuple(paths)


def relative_rustc_tool_paths(arguments: Sequence[str]) -> list[tuple[int, str, str]]:
    """Find path operands without interpreting Cargo's flag precedence."""
    result = []
    for index, argument in enumerate(arguments):
        prefix = ""
        if index and arguments[index - 1] == "--sysroot":
            path = argument
        elif argument.startswith("--sysroot="):
            prefix, path = "--sysroot=", argument[len("--sysroot=") :]
        elif argument.startswith("-Clinker="):
            prefix, path = "-Clinker=", argument[len("-Clinker=") :]
        elif index and arguments[index - 1] == "-C" and argument.startswith("linker="):
            prefix, path = "linker=", argument[len("linker=") :]
        else:
            continue
        if "linker=" in prefix and not any(separator in path for separator in "/\\"):
            continue  # A bare linker name is a tool-search selection, not a path.
        if not Path(path).is_absolute():
            result.append((index, prefix, path))
    return result


@dataclass(frozen=True, slots=True)
class RustToolSearch:
    host: str
    selected_sysroot: Path
    compiler_sysroot: Path

    def directories(self) -> tuple[Path, ...]:
        return tuple(
            dict.fromkeys(
                root / "lib" / "rustlib" / self.host / "bin"
                for root in (self.selected_sysroot, self.compiler_sysroot)
            )
        )

    def resolve(
        self, executable: str, *, cwd: Path, env: Mapping[str, str]
    ) -> tuple[Path, dict[str, object]]:
        candidate = Path(executable)
        if candidate.is_absolute() or "/" in executable or "\\" in executable:
            path = find_executable(executable, cwd=cwd, environment=env)
            if path is None:
                raise ValueError(
                    f"selected explicit Rust process image is unavailable: {executable!r}"
                )
            origin = "explicit-path"
        else:
            # Tool suffix belongs to the compiler host even for a WASM target.
            name = executable
            if "windows" in self.host.split("-") and not name.lower().endswith(".exe"):
                name += ".exe"
            path = next(
                (root / name for root in self.directories() if (root / name).is_file()),
                None,
            )
            origin = "rust-sysroot-host-tool"
            if path is None:
                selected = find_executable(executable, cwd=cwd, environment=env)
                if selected is None:
                    raise ValueError(
                        f"selected Rust process image is unavailable: {executable!r}"
                    )
                path, origin = Path(selected), "process-path"
        # Invocation spelling selects driver mode (clang++ versus clang, etc.).
        # Image custody separately follows this entrypoint to its content bytes.
        path = path.absolute()
        if not path.is_file() or not os.access(path, os.X_OK):
            raise ValueError(f"selected Rust process image is not executable: {path}")
        return path, {
            "requested": executable,
            "path": str(path),
            "content_path": str(path.resolve(strict=True)),
            "origin": origin,
            "compiler_host": self.host,
            "selected_sysroot": str(self.selected_sysroot),
            "compiler_sysroot": str(self.compiler_sysroot),
            "tool_search_directories": [str(value) for value in self.directories()],
        }
