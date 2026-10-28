"""A checkout's lock-declared inputs, provisioned on demand.

Managed development Python is published by the existing exact-address
source-build environment owner. The checkout's existing Git control directory
keeps a bounded selection projection, keyed by interpreter/group intent. That
projection selects an immutable generation; it never certifies its live bytes
or authorizes mutation of a published image. A lock or editable binding change
selects a new generation at the explicit ``ensure`` boundary.

Browser/Edgebox npm trees retain their pinned, script-free exact-install and
in-tree stamp contract. Repository Git hooks retain their owning check/install
protocol. Tool caches and process custody remain with their existing owners.
"""

from __future__ import annotations

import argparse
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass
from datetime import UTC, datetime
import hashlib
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import time
from typing import Literal, assert_never, get_args

from molt import python_environment_contract as _contract
from molt import process_guard
from molt.custody_layout import git_directories
from molt.exact_json import (
    ExactJsonError,
    canonical_json_sha256,
    read_exact,
    write_exact,
)
from molt.file_locks import _acquire_file_lock, _release_file_lock
from molt.process_environment import ProcessEnvironment
from molt.toolchain_identity import StableRegularFileError

SCHEMA = "molt.checkout-provisioning.v1"
STAMP_NAME = ".molt-provisioned.json"
GUARD_PREFIX = "MOLT_CHECKOUT_PROVISIONING"
PROVISION_TIMEOUT_S = 900.0
# A waiter outlasts the holder's whole provisioner budget.
LOCK_TIMEOUT_S = PROVISION_TIMEOUT_S + 60.0
_PROBE_TIMEOUT_S = 60.0
_MAX_STAMP_BYTES = 16 * 1024
_OUTPUT_TAIL_CHARS = 4000

CheckoutInputName = Literal[
    "uv-environment", "browser-acorn", "edgebox-typescript", "git-hooks"
]
CHECKOUT_INPUT_NAMES: tuple[CheckoutInputName, ...] = get_args(CheckoutInputName)
# Lane setup and sandbox preparation keep the original checkout prerequisites.
# Registered optional inputs are admitted only when a proof selects them; the
# complete table still recognizes their stamps during preservation/retirement.
DEFAULT_CHECKOUT_INPUT_NAMES: tuple[CheckoutInputName, ...] = (
    "uv-environment",
    "browser-acorn",
    "git-hooks",
)
InputScope = Literal["checkout", "repository"]
InputState = Literal["current", "stale", "absent"]

ACORN_ROOT = "tools/browser_asset_graph"
EDGEBOX_ROOT = "examples/edgebox/packages/edgebox-cloudflare"
HOOK_INSTALLER = "tools/install_git_hooks.py"


class ProvisioningError(RuntimeError):
    """An input could not be provisioned; the message names the command."""


def _require_guard_source_owner(owner: Path | None) -> None:
    if owner is not None and not owner.is_absolute():
        raise ProvisioningError("guard source owner must be an absolute path")


@dataclass(frozen=True, slots=True)
class LockedTree:
    """A tree that one package-manager command builds from lock files.

    Lock paths are relative to ``checkout``; command arguments are relative
    to its ``command_cwd``. Both stay checkout-relative, so the action digest
    does not depend on where the checkout lives, except an editable uv project.
    """

    name: str
    checkout: Path
    lock_files: tuple[str, ...]
    tree: Path
    sentinel: str
    tool: str
    arguments: tuple[str, ...]
    environment: ProcessEnvironment
    scope: InputScope = "checkout"
    command_cwd: str = "."
    # Variables the caller's tool would choose for itself, each with the tool
    # arguments that print that choice. The guarded command receives them
    # explicitly, so the memory guard's dx defaults cannot redirect it.
    caller_choices: tuple[tuple[str, tuple[str, ...]], ...] = ()
    # Choices the tool clones or hardlinks from, so they must share the tree's
    # filesystem; one on another volume yields to dx's artifact-volume default
    # when that default shares it (module docstring).
    same_volume_choices: tuple[str, ...] = ()
    # A variable in which a running tool names its own binary (`uv run` sets
    # UV); it outranks PATH.
    tool_variable: str | None = None
    # The tool is a pinned release in config/tool_releases.toml: provisioned
    # and run through molt.tool_releases (npm on its pinned Node), never PATH.
    pinned: bool = False
    # The package.json whose packageManager must name the pinned version.
    package_manifest: str | None = None
    # Execution implementation custody is independent of provisioned content.
    guard_source_root: Path | None = None

    def __post_init__(self) -> None:
        _require_guard_source_owner(self.guard_source_root)
        directory = Path(self.command_cwd)
        if directory.is_absolute() or ".." in directory.parts:
            raise ValueError("command_cwd must remain inside the checkout")

    @property
    def stamp(self) -> Path:
        return self.tree / STAMP_NAME

    def command(self) -> tuple[tuple[str, ...], ProcessEnvironment] | None:
        """The argv prefix that runs the tool and the environment it runs in;
        None when PATH has no such tool."""
        if self.pinned:
            from molt.tool_releases import ToolReleaseError, pinned_command

            try:
                pinned = pinned_command(
                    self.tool,
                    self.checkout,
                    environ=self.environment,
                    cwd=self.checkout,
                    provision=True,
                )
            except ToolReleaseError as exc:
                raise ProvisioningError(f"{self.name}: {exc}") from exc
            return pinned.argv, pinned.environment(self.environment)
        if self.tool_variable and self.environment.get(self.tool_variable):
            return (self.environment[self.tool_variable],), self.environment
        found = shutil.which(self.tool, path=self.environment.get("PATH"))
        return None if found is None else ((found,), self.environment)


@dataclass(frozen=True, slots=True)
class OwnedInstallation:
    """An input whose owning tool decides currency and installs it.

    The owner is a repository script with ``--check`` (exit 0 current, 1
    stale) and a no-argument idempotent install.
    """

    name: str
    checkout: Path
    owner: str
    environment: ProcessEnvironment
    scope: InputScope = "repository"
    # A repository-scoped install serves every worktree. It installs only
    # sources identical to the published ref's, so an older or unlanded
    # checkout cannot change what every other worktree runs.
    sources: str | None = None
    published_ref: str = "origin/main"
    guard_source_root: Path | None = None

    def __post_init__(self) -> None:
        _require_guard_source_owner(self.guard_source_root)


@dataclass(frozen=True, slots=True)
class DevelopmentEnvironment:
    """One immutable development image selected by the shared publisher."""

    name: str
    checkout: Path
    environment: ProcessEnvironment
    python: str | None = None
    groups: tuple[str, ...] | None = None
    scope: InputScope = "checkout"
    guard_source_root: Path | None = None
    lock_files: tuple[str, ...] = ("uv.lock", "pyproject.toml", ".python-version")


CheckoutInput = LockedTree | OwnedInstallation | DevelopmentEnvironment


@dataclass(frozen=True, slots=True)
class Ensured:
    name: str
    provisioned: bool
    seconds: float
    environment_root: Path | None = None


def checkout_input(
    name: CheckoutInputName,
    checkout: Path,
    environ: Mapping[str, str] | None = None,
    *,
    guard_source_root: Path | None = None,
    python: str | None = None,
    groups: tuple[str, ...] | None = None,
) -> CheckoutInput:
    """The declared input ``name`` of ``checkout`` under ``environ``."""
    _require_guard_source_owner(guard_source_root)
    checkout = checkout.expanduser().resolve()
    environment = ProcessEnvironment(os.environ if environ is None else environ)
    match name:
        case "uv-environment":
            return DevelopmentEnvironment(
                name=name,
                checkout=checkout,
                environment=environment,
                guard_source_root=guard_source_root,
                python=python,
                groups=groups,
            )
        case "browser-acorn":
            return LockedTree(
                name=name,
                guard_source_root=guard_source_root,
                checkout=checkout,
                lock_files=(
                    f"{ACORN_ROOT}/package-lock.json",
                    f"{ACORN_ROOT}/package.json",
                ),
                tree=checkout / ACORN_ROOT / "node_modules",
                sentinel="acorn/package.json",
                tool="npm",
                pinned=True,
                package_manifest=f"{ACORN_ROOT}/package.json",
                arguments=(
                    "ci",
                    "--ignore-scripts",
                    "--prefer-offline",
                    "--no-audit",
                    "--no-fund",
                    "--prefix",
                    ACORN_ROOT,
                ),
                environment=environment,
            )
        case "edgebox-typescript":
            return LockedTree(
                name=name,
                guard_source_root=guard_source_root,
                checkout=checkout,
                lock_files=(
                    f"{EDGEBOX_ROOT}/package-lock.json",
                    f"{EDGEBOX_ROOT}/package.json",
                ),
                tree=checkout / EDGEBOX_ROOT / "node_modules",
                sentinel="typescript/bin/tsc",
                tool="npm",
                pinned=True,
                package_manifest=f"{EDGEBOX_ROOT}/package.json",
                arguments=(
                    "ci",
                    "--ignore-scripts",
                    "--prefer-offline",
                    "--no-audit",
                    "--no-fund",
                ),
                environment=environment,
                command_cwd=EDGEBOX_ROOT,
            )
        case "git-hooks":
            return OwnedInstallation(
                name=name,
                guard_source_root=guard_source_root,
                checkout=checkout,
                owner=HOOK_INSTALLER,
                environment=environment,
                sources=".githooks",
            )
        case _:
            assert_never(name)


def declares(spec: CheckoutInput) -> bool:
    """Whether the checkout declares ``spec``: its primary lock file exists.

    A declared input whose other lock files are missing still fails loudly
    at the explicit ``ensure`` boundary.
    """
    if isinstance(spec, (LockedTree, DevelopmentEnvironment)):
        return (spec.checkout / spec.lock_files[0]).is_file()
    return (spec.checkout / spec.owner).is_file()


def locked_input_files(name: CheckoutInputName) -> tuple[str, ...]:
    """The checkout-relative lock files of a locked input; empty otherwise."""
    spec = checkout_input(name, Path(os.curdir), {})
    return (
        spec.lock_files
        if isinstance(spec, (LockedTree, DevelopmentEnvironment))
        else ()
    )


def _command_text(argv: Sequence[str]) -> str:
    return shlex.join(str(part) for part in argv)


def _tail(text: str | None) -> str:
    text = (text or "").strip()
    if len(text) > _OUTPUT_TAIL_CHARS:
        return "..." + text[-_OUTPUT_TAIL_CHARS:]
    return text


def _failure(
    name: str, argv: Sequence[str], cwd: Path, result: subprocess.CompletedProcess
) -> ProvisioningError:
    output = "\n".join(
        part for part in (_tail(result.stdout), _tail(result.stderr)) if part
    )
    return ProvisioningError(
        f"{name}: `{_command_text(argv)}` in {cwd} failed "
        f"(exit {result.returncode})" + (f":\n{output}" if output else "")
    )


def _run(
    name: str,
    argv: Sequence[str],
    *,
    cwd: Path,
    environment: ProcessEnvironment,
    guarded: bool,
    guard_source_root: Path | None = None,
) -> subprocess.CompletedProcess[str]:
    """One command; a probe runs unguarded under a bounded timeout."""
    try:
        return process_guard.run_completed_command(
            list(argv),
            guard_source_root=guard_source_root,
            env=environment,
            cwd=cwd,
            capture_output=True,
            text=True,
            timeout=PROVISION_TIMEOUT_S if guarded else _PROBE_TIMEOUT_S,
            memory_guard_prefix=GUARD_PREFIX if guarded else None,
        )
    except (OSError, subprocess.SubprocessError) as exc:
        raise ProvisioningError(
            f"{name}: `{_command_text(argv)}` in {cwd} did not complete: {exc}"
        ) from exc


def _lock_digests(spec: LockedTree | DevelopmentEnvironment) -> dict[str, str]:
    digests: dict[str, str] = {}
    if isinstance(spec, DevelopmentEnvironment):
        from molt.source_build_environment_selection import capture_source_build_inputs

        try:
            return {
                key: value
                for key, value in capture_source_build_inputs(
                    spec.checkout, editable=True
                ).digests.items()
                if value is not None
            }
        except (OSError, ValueError) as exc:
            raise ProvisioningError(f"uv-environment: {exc}") from exc
    for relative in spec.lock_files:
        if relative in digests:
            continue
        path = spec.checkout / relative
        try:
            digests[relative] = hashlib.sha256(path.read_bytes()).hexdigest()
        except OSError as exc:
            raise ProvisioningError(
                f"{spec.name}: declared lock file {path} is unreadable: {exc}"
            ) from exc
    return digests


def _action_digest(spec: LockedTree, locks: Mapping[str, str]) -> str:
    digest = hashlib.sha256(SCHEMA.encode("ascii"))
    for part in (spec.name, spec.tool, *spec.arguments):
        digest.update(b"\0" + part.encode("utf-8"))
    # Default-cwd actions keep their existing digest/stamp; a project that
    # runs in its own directory binds that directory.
    if spec.command_cwd != ".":
        digest.update(b"\2" + spec.command_cwd.encode("utf-8"))
    for relative, file_digest in locks.items():
        digest.update(b"\1" + relative.encode("utf-8") + b"\0")
        digest.update(file_digest.encode("ascii"))
    return digest.hexdigest()


def _stamp_record(spec: LockedTree) -> dict | None:
    try:
        value = read_exact(
            spec.stamp, max_bytes=_MAX_STAMP_BYTES, label=f"{spec.name} stamp"
        )
    except (OSError, ExactJsonError, ValueError):
        return None
    if (
        not isinstance(value, dict)
        or value.get("schema") != SCHEMA
        or value.get("input") != spec.name
        or not isinstance(value.get("digest"), str)
    ):
        return None
    return value


def _locked_state(spec: LockedTree, digest: str | None = None) -> InputState:
    record = _stamp_record(spec)
    if record is None:
        return "absent"
    if digest is None:
        digest = _action_digest(spec, _lock_digests(spec))
    if record["digest"] != digest:
        return "stale"
    return "current" if (spec.tree / spec.sentinel).is_file() else "stale"


def _lock_path(spec: CheckoutInput) -> Path:
    try:
        directories = git_directories(spec.checkout)
    except ValueError as exc:
        raise ProvisioningError(f"{spec.name}: {exc}") from exc
    if directories is None:
        raise ProvisioningError(
            f"{spec.name}: {spec.checkout} is not a Git checkout; provisioning "
            "serializes on a lock in its Git directory"
        )
    git_dir, common_dir = directories
    base = git_dir if spec.scope == "checkout" else common_dir
    return base / "molt" / "checkout-provisioning" / f"{spec.name}.lock"


def _development_intent(spec: DevelopmentEnvironment) -> dict:

    try:
        groups = spec.groups
        if groups is None:
            from molt.dx import DxConfigError, DxProject

            try:
                groups = DxProject(spec.checkout).dependency_groups()
            except DxConfigError as exc:
                raise ProvisioningError(str(exc)) from exc
        python = (
            spec.python or spec.environment.get("MOLT_UV_PROJECT_PYTHON", "").strip()
        )
        if not python:
            python = (
                (spec.checkout / ".python-version").read_text(encoding="utf-8").strip()
            )
        if not python:
            raise ProvisioningError("uv-environment: empty Python selection")
        from molt.dx import DEFAULT_UV_PROJECT_PURPOSE, artifact_root

        return {
            "producer_contract": _contract.source_build_environment_contract(),
            "python": python,
            "groups": sorted(set(groups)),
            "purpose": spec.environment.get(
                "MOLT_UV_PROJECT_PURPOSE", DEFAULT_UV_PROJECT_PURPOSE
            ),
            "artifact_root": str(artifact_root(spec.checkout, spec.environment)),
        }
    except (OSError, ValueError) as exc:
        raise ProvisioningError(
            f"uv-environment: cannot read selection inputs: {exc}"
        ) from exc


def _development_selection_path(spec: DevelopmentEnvironment, intent: Mapping) -> Path:
    control = _lock_path(spec).parent
    return control / f"uv-environment.{canonical_json_sha256(intent)}.{STAMP_NAME}"


def selected_development_environment(
    checkout: Path,
    environ: Mapping[str, str],
    *,
    python: str | None = None,
    groups: tuple[str, ...] | None = None,
) -> Path | None:
    """Read a bounded generation projection, never an admitted image or bytes.

    Path getters use this coordinate only. Operation boundaries load the real
    published recipe before calling an input current or launching its Python.
    """
    checkout = checkout.expanduser().resolve()
    spec = DevelopmentEnvironment(
        "uv-environment", checkout, ProcessEnvironment(environ), python, groups
    )
    if not declares(spec):
        return None
    intent = _development_intent(spec)
    explicit = environ.get("UV_PROJECT_ENVIRONMENT", "").strip()
    if explicit:
        selected = Path(explicit).expanduser()
        if not selected.is_absolute():
            selected = checkout / selected
        return selected.absolute()
    if git_directories(checkout) is None:
        return None
    path = _development_selection_path(spec, intent)
    try:
        record = read_exact(
            path, max_bytes=_MAX_STAMP_BYTES, label="development selection"
        )
    except (OSError, ExactJsonError, ValueError) as exc:
        # The stable reader wraps an absent opening path. A changed or
        # malformed existing record remains a refusal, never fresh intent.
        if type(exc) is StableRegularFileError and isinstance(
            exc.__cause__, FileNotFoundError
        ):
            return None
        raise ProvisioningError(
            f"uv-environment: invalid selection {path}: {exc}"
        ) from exc
    if (
        not isinstance(record, dict)
        or set(record)
        != {"schema", "input", "checkout", "intent", "locks", "generation"}
        or record["schema"] != SCHEMA
        or record["input"] != spec.name
        or record["checkout"] != str(checkout)
        or record["intent"] != intent
        or not isinstance(record["generation"], str)
        or not Path(record["generation"]).is_absolute()
    ):
        raise ProvisioningError(f"uv-environment: invalid selection {path}")
    if record["locks"] != _lock_digests(spec):
        return None
    return Path(record["generation"])


def _current_development_environment(spec: DevelopmentEnvironment) -> Path | None:
    """Validate a selection's published recipe at an operation boundary."""
    from molt.source_build_environment_selection import (
        read_development_environment_selection,
    )

    selected = selected_development_environment(
        spec.checkout, spec.environment, python=spec.python, groups=spec.groups
    )
    if selected is None:
        return None
    intent = _development_intent(spec)
    try:
        if not spec.environment.get("UV_PROJECT_ENVIRONMENT", "").strip():
            try:
                selected.lstat()
            except FileNotFoundError:
                # Selection records outlive reconstructible image payloads.
                # Only a missing physical generation is a miss: a dangling
                # root/ancestor or a foreign coordinate remains a refusal.
                from molt.checkout_state import checkout_state_kind_of
                from molt.dx import artifact_root, checkout_component
                from molt.file_publication import resolve_owned_path

                physical = resolve_owned_path(selected)
                if (
                    physical != selected
                    or selected.parent.name != checkout_component(spec.checkout)
                    or checkout_state_kind_of(
                        selected, artifact_root(spec.checkout, spec.environment)
                    )
                    != "python-environment"
                ):
                    raise ProvisioningError(
                        "uv-environment: selected development image is outside "
                        "its physical checkout generation namespace"
                    )
                return None
        return read_development_environment_selection(
            selected,
            repo_root=spec.checkout,
            python=intent["python"],
            groups=tuple(intent["groups"]),
        ).root
    except (OSError, ValueError, _contract.SourceBuildEnvironmentError) as exc:
        raise ProvisioningError(f"uv-environment: {exc}") from exc


class _ProvisioningLock:
    def __init__(self, spec: CheckoutInput) -> None:
        self._path = _lock_path(spec)
        self._name = spec.name
        self._handle = None

    def __enter__(self) -> None:
        self._path.parent.mkdir(parents=True, exist_ok=True)
        try:
            self._handle = _acquire_file_lock(
                self._path,
                timeout_s=LOCK_TIMEOUT_S,
                timeout_message=(
                    f"{self._name}: another provisioner held {self._path} for "
                    f"{LOCK_TIMEOUT_S:.0f} s"
                ),
            )
        except RuntimeError as exc:
            raise ProvisioningError(str(exc)) from exc

    def __exit__(self, *_exc: object) -> None:
        if self._handle is not None:
            _release_file_lock(self._handle)


def _require_package_manager(spec: LockedTree) -> None:
    """Refuse a project whose packageManager names another npm than the pin."""
    if spec.package_manifest is None:
        return
    from molt.tool_releases import ToolReleaseError, tool_release

    try:
        expected = f"{spec.tool}@{tool_release(spec.tool, spec.checkout).version}"
    except ToolReleaseError as exc:
        raise ProvisioningError(f"{spec.name}: {exc}") from exc
    manifest = spec.checkout / spec.package_manifest
    try:
        declared = json.loads(manifest.read_text(encoding="utf-8")).get(
            "packageManager"
        )
    except (OSError, ValueError, AttributeError) as exc:
        raise ProvisioningError(
            f"{spec.name}: cannot read packageManager from {manifest}: {exc}"
        ) from exc
    if declared != expected:
        raise ProvisioningError(
            f"{spec.name}: {spec.package_manifest} declares packageManager "
            f"{declared!r}, but config/tool_releases.toml pins {expected}; "
            f"`python3 tools/pin_freshness.py --update {spec.tool}` moves both"
        )


def _tool_version(
    spec: LockedTree, command: Sequence[str], environment: ProcessEnvironment
) -> str:
    argv = [*command, "--version"]
    result = _run(
        spec.name,
        argv,
        cwd=spec.checkout,
        environment=environment,
        guarded=False,
        guard_source_root=spec.guard_source_root,
    )
    lines = (result.stdout or "").strip().splitlines()
    if result.returncode != 0 or not lines:
        raise _failure(spec.name, argv, spec.checkout, result)
    return lines[0].strip()


def _device_of(path: Path) -> int | None:
    """The filesystem device of ``path``, or of its nearest existing ancestor."""
    for candidate in (path, *path.parents):
        try:
            return os.stat(candidate).st_dev
        except OSError:
            continue
    return None


def _dx_choice(variable: str, spec: LockedTree) -> Path | None:
    """The value dx gives ``variable`` when the caller leaves it unset.

    dx owns the layout (for ``UV_CACHE_DIR``, the cache under its artifact
    root, or a ``[tool.molt.dx.env]`` entry); this asks dx rather than repeat
    its rule.
    """
    from molt.dx import DxConfigError, DxProject

    try:
        environment = DxProject(spec.checkout).canonical_env(
            spec.environment.without((variable,)), create_dirs=False
        )
    except (DxConfigError, OSError, ValueError):
        return None
    value = environment.get(variable, "").strip()
    return Path(value) if value else None


def _with_caller_choices(
    spec: LockedTree, command: Sequence[str], environment: ProcessEnvironment
) -> ProcessEnvironment:
    """Pin each choice the caller left to its tool, as the tool itself makes it.

    A choice in ``same_volume_choices`` is a store the tool clones or
    hardlinks from, so it must share the tree's filesystem; a value on another
    volume yields to dx's artifact-volume default when that one shares it, and
    stays otherwise (the tool then copies).
    """
    pinned: dict[str, str] = {}
    for variable, arguments in spec.caller_choices:
        if environment.get(variable, "").strip():
            continue
        argv = [*command, *arguments]
        result = _run(
            spec.name,
            argv,
            cwd=spec.checkout,
            environment=environment,
            guarded=False,
            guard_source_root=spec.guard_source_root,
        )
        lines = (result.stdout or "").strip().splitlines()
        if result.returncode != 0 or len(lines) != 1:
            raise _failure(spec.name, argv, spec.checkout, result)
        pinned[variable] = lines[0].strip()
    device = _device_of(spec.tree)
    for variable in spec.same_volume_choices:
        chosen = pinned.get(variable) or environment.get(variable, "").strip()
        if not chosen or device is None or _device_of(Path(chosen)) == device:
            continue
        alternative = _dx_choice(variable, spec)
        if (
            alternative is not None
            and alternative.is_dir()
            and _device_of(alternative) == device
        ):
            pinned[variable] = str(alternative)
    return environment.updated(pinned) if pinned else environment


def _provision_locked(spec: LockedTree) -> bool:
    """Make ``spec`` current; True when this call ran its provisioner."""
    if _locked_state(spec) == "current":
        return False
    with _ProvisioningLock(spec):
        locks = _lock_digests(spec)
        digest = _action_digest(spec, locks)
        if _locked_state(spec, digest) == "current":
            return False  # Another process provisioned it while we waited.
        _require_package_manager(spec)
        launch = spec.command()
        if launch is None:
            raise ProvisioningError(
                f"{spec.name}: `{spec.tool}` is not on PATH; it builds "
                f"{spec.tree} from {', '.join(spec.lock_files)}"
            )
        command, tool_environment = launch
        environment = _with_caller_choices(spec, command, tool_environment)
        spec.stamp.unlink(missing_ok=True)
        argv = [*command, *spec.arguments]
        started = time.monotonic()
        command_cwd = spec.checkout / spec.command_cwd
        result = _run(
            spec.name,
            argv,
            cwd=command_cwd,
            environment=environment,
            guarded=True,
            guard_source_root=spec.guard_source_root,
        )
        seconds = time.monotonic() - started
        if result.returncode != 0:
            raise _failure(spec.name, argv, command_cwd, result)
        if not (spec.tree / spec.sentinel).is_file():
            raise ProvisioningError(
                f"{spec.name}: `{_command_text(argv)}` exited 0 but "
                f"{spec.tree / spec.sentinel} does not exist"
            )
        if _lock_digests(spec) != locks:
            raise ProvisioningError(
                f"{spec.name}: {', '.join(spec.lock_files)} changed while "
                f"`{_command_text(argv)}` ran; the tree was not stamped"
            )
        write_exact(
            spec.stamp,
            {
                "schema": SCHEMA,
                "input": spec.name,
                "digest": digest,
                "locks": locks,
                "tool": {
                    "name": spec.tool,
                    "command": list(command),
                    "version": _tool_version(spec, command, tool_environment),
                },
                "provisioned_at_utc": datetime.now(UTC).isoformat(timespec="seconds"),
                "seconds": round(seconds, 3),
            },
        )
    return True


def _owner_argv(spec: OwnedInstallation, *arguments: str) -> list[str]:
    # A repository tool imports only the standard library and its own
    # checkout. `-I -S` keeps this interpreter's site-packages, whose editable
    # install and sitecustomize name another checkout's molt, out of it.
    from molt.python_environment_location import environment_bytecode_arguments

    return [
        sys.executable,
        *environment_bytecode_arguments(Path(sys.prefix)),
        "-I",
        "-S",
        str(spec.checkout / spec.owner),
        *arguments,
    ]


def _owned_state(spec: OwnedInstallation) -> InputState:
    argv = _owner_argv(spec, "--check")
    # The owner's check is a bounded read-only probe.
    result = _run(
        spec.name,
        argv,
        cwd=spec.checkout,
        environment=spec.environment,
        guarded=False,
        guard_source_root=spec.guard_source_root,
    )
    if result.returncode == 0:
        return "current"
    if result.returncode == 1:
        return "stale"
    raise _failure(spec.name, argv, spec.checkout, result)


def _require_published_sources(spec: OwnedInstallation) -> None:
    if spec.sources is None:
        return
    argv = ["git", "-C", str(spec.checkout), "diff", "--quiet", spec.published_ref]
    argv += ["--", spec.sources]
    result = _run(
        spec.name,
        argv,
        cwd=spec.checkout,
        environment=spec.environment,
        guarded=False,
        guard_source_root=spec.guard_source_root,
    )
    if result.returncode == 0:
        return
    if result.returncode == 1:
        raise ProvisioningError(
            f"{spec.name}: {spec.checkout / spec.sources} differs from "
            f"{spec.published_ref}; these hooks serve every worktree, so they "
            f"install only from a checkout that matches {spec.published_ref}"
        )
    raise _failure(spec.name, argv, spec.checkout, result)


def _provision_owned(spec: OwnedInstallation) -> bool:
    with _ProvisioningLock(spec):
        if _owned_state(spec) == "current":
            return False
        _require_published_sources(spec)
        argv = _owner_argv(spec)
        result = _run(
            spec.name,
            argv,
            cwd=spec.checkout,
            environment=spec.environment,
            guarded=True,
            guard_source_root=spec.guard_source_root,
        )
        if result.returncode != 0:
            raise _failure(spec.name, argv, spec.checkout, result)
        if _owned_state(spec) != "current":
            raise ProvisioningError(
                f"{spec.name}: `{_command_text(argv)}` exited 0 but "
                f"`{_command_text(_owner_argv(spec, '--check'))}` still "
                "reports it stale"
            )
    return True


def input_state(spec: CheckoutInput) -> InputState:
    """Whether ``spec`` is current, without provisioning it."""
    if isinstance(spec, DevelopmentEnvironment):
        return (
            "current"
            if _current_development_environment(spec) is not None
            else "absent"
        )
    if isinstance(spec, LockedTree):
        return _locked_state(spec)
    return _owned_state(spec)


def ensure_input(spec: CheckoutInput, *, refresh_runtime: bool = False) -> Ensured:
    """Make ``spec`` current; optionally recapture its development runtime recipe."""
    if refresh_runtime:
        if not isinstance(spec, DevelopmentEnvironment):
            raise ProvisioningError("runtime refresh requires uv-environment")
        if spec.environment.get("UV_PROJECT_ENVIRONMENT", "").strip():
            raise ProvisioningError(
                "runtime refresh cannot replace explicit UV_PROJECT_ENVIRONMENT"
            )
    started = time.monotonic()
    if isinstance(spec, DevelopmentEnvironment):
        from molt.python_interpreter import is_python_version_selector
        from molt.source_build_environment import development_environment

        try:
            # Current selection is a cheap recipe projection, not a claim that
            # published executable bytes have passed operation admission.
            intent = _development_intent(spec)
            # A command/path selector or explicit refresh needs the publisher's
            # exact runtime probe; bounded metadata cannot certify current bytes.
            selected = (
                _current_development_environment(spec)
                if not refresh_runtime and is_python_version_selector(intent["python"])
                else None
            )
            if selected is not None:
                return Ensured(spec.name, False, time.monotonic() - started, selected)
            with _ProvisioningLock(spec):
                intent = _development_intent(spec)
                locks = _lock_digests(spec)
                image = development_environment(
                    spec.checkout,
                    python=intent["python"],
                    groups=tuple(intent["groups"]),
                    environ=spec.environment,
                    provision=True,
                )
                if _lock_digests(spec) != locks:
                    raise ProvisioningError(
                        "uv-environment: declared inputs changed during publication"
                    )
                write_exact(
                    _development_selection_path(spec, intent),
                    {
                        "schema": SCHEMA,
                        "input": spec.name,
                        "checkout": str(spec.checkout),
                        "intent": intent,
                        "locks": locks,
                        "generation": str(image.root),
                    },
                )
            return Ensured(
                spec.name, image.provisioned, time.monotonic() - started, image.root
            )
        except (_contract.SourceBuildEnvironmentError, OSError, ValueError) as exc:
            raise ProvisioningError(f"uv-environment: {exc}") from exc
    if isinstance(spec, LockedTree):
        provisioned = _provision_locked(spec)
    else:
        provisioned = _provision_owned(spec)
    return Ensured(spec.name, provisioned, time.monotonic() - started)


def ensure(
    name: CheckoutInputName,
    checkout: Path,
    environ: Mapping[str, str] | None = None,
    *,
    guard_source_root: Path | None = None,
    python: str | None = None,
    groups: tuple[str, ...] | None = None,
    refresh_runtime: bool = False,
) -> Ensured:
    """Publish/select the declared input through its existing owner."""
    return ensure_input(
        checkout_input(
            name,
            checkout,
            environ,
            guard_source_root=guard_source_root,
            python=python,
            groups=groups,
        ),
        refresh_runtime=refresh_runtime,
    )


def ensure_all(
    checkout: Path,
    environ: Mapping[str, str] | None = None,
    names: Iterable[CheckoutInputName] = DEFAULT_CHECKOUT_INPUT_NAMES,
    *,
    guard_source_root: Path | None = None,
    refresh_runtime: bool = False,
) -> tuple[Ensured, ...]:
    """Provision every named input; report every failure together.

    Each input is atomic: a failure leaves that input unstamped and keeps the
    inputs that did succeed.
    """
    names = tuple(names)
    if refresh_runtime:
        if "uv-environment" not in names:
            raise ProvisioningError("runtime refresh requires uv-environment")
        environment = ProcessEnvironment(os.environ if environ is None else environ)
        if environment.get("UV_PROJECT_ENVIRONMENT", "").strip():
            raise ProvisioningError(
                "runtime refresh cannot replace explicit UV_PROJECT_ENVIRONMENT"
            )
    done: list[Ensured] = []
    failures: list[str] = []
    for name in names:
        try:
            done.append(
                ensure(
                    name,
                    checkout,
                    environ,
                    guard_source_root=guard_source_root,
                    refresh_runtime=refresh_runtime and name == "uv-environment",
                )
            )
        except ProvisioningError as exc:
            failures.append(str(exc))
    if failures:
        raise ProvisioningError("\n".join(failures))
    return tuple(done)


def provisioning_command(name: CheckoutInputName) -> str:
    """The command a person or a setup step runs to provision ``name``."""
    return f"python -m molt.checkout_provisioning ensure {name}"


def main(argv: Sequence[str] | None = None) -> int:
    from molt.source_root import compiler_source_root

    parser = argparse.ArgumentParser(
        description="Provision or inspect a checkout's lock-declared inputs."
    )
    parser.add_argument("action", choices=("ensure", "status", "artifact-root"))
    parser.add_argument(
        "inputs",
        nargs="*",
        metavar="INPUT",
        help=f"one of {', '.join(CHECKOUT_INPUT_NAMES)} "
        f"(default: {', '.join(DEFAULT_CHECKOUT_INPUT_NAMES)})",
    )
    parser.add_argument("--checkout", type=Path, default=compiler_source_root())
    parser.add_argument("--json", action="store_true")
    binding = parser.add_mutually_exclusive_group()
    binding.add_argument(
        "--bind",
        type=Path,
        help="Bind an existing healthy artifact root for this Git family.",
    )
    binding.add_argument(
        "--clear", action="store_true", help="Clear the family's artifact-root binding."
    )
    parser.add_argument(
        "--refresh-runtime",
        action="store_true",
        help="Recapture the current Python runtime through the uv-environment publisher.",
    )
    args = parser.parse_args(argv)
    if args.action == "artifact-root":
        if args.inputs or args.refresh_runtime:
            parser.error("artifact-root accepts no inputs or --refresh-runtime")
        from molt.dx import (
            DxConfigError,
            artifact_root_binding_path,
            artifact_root_selection,
            prefers_external_artifacts,
            set_artifact_root_binding,
        )

        try:
            if args.bind is not None or args.clear:
                record = set_artifact_root_binding(args.checkout, args.bind)
            else:
                record = artifact_root_binding_path(args.checkout)
            selection = artifact_root_selection(
                args.checkout,
                prefer_external=prefers_external_artifacts(args.checkout, os.environ),
            )
        except (DxConfigError, OSError, ValueError) as exc:
            print(exc, file=sys.stderr)
            return 1
        payload = {
            "root": str(selection.root),
            "source": selection.source,
            "binding_record": None if record is None else str(record),
        }
        if args.json:
            print(json.dumps(payload, sort_keys=True))
        else:
            print(f"artifact root: {selection.root} ({selection.source})")
            print(
                f"binding record: {record if record is not None else 'none (no Git family)'}"
            )
        return 0
    if args.bind is not None or args.clear:
        parser.error("--bind and --clear require artifact-root")
    unknown = sorted(set(args.inputs) - set(CHECKOUT_INPUT_NAMES))
    if unknown:
        parser.error(f"unknown checkout input: {', '.join(unknown)}")
    names = (
        tuple(name for name in CHECKOUT_INPUT_NAMES if name in args.inputs)
        if args.inputs
        else DEFAULT_CHECKOUT_INPUT_NAMES
    )
    if args.refresh_runtime and args.action != "ensure":
        parser.error("--refresh-runtime requires ensure")
    if args.refresh_runtime and "uv-environment" not in names:
        parser.error("--refresh-runtime requires uv-environment")
    if args.action == "status":
        try:
            states = {
                name: input_state(checkout_input(name, args.checkout)) for name in names
            }
        except ProvisioningError as exc:
            print(exc, file=sys.stderr)
            return 1
        if args.json:
            print(json.dumps(states, sort_keys=True))
        else:
            for name, state in states.items():
                print(f"{name}: {state}")
        return 0 if all(state == "current" for state in states.values()) else 1
    try:
        done = ensure_all(
            args.checkout, names=names, refresh_runtime=args.refresh_runtime
        )
    except ProvisioningError as exc:
        print(exc, file=sys.stderr)
        return 1
    for item in done:
        action = "provisioned" if item.provisioned else "current"
        print(f"{item.name}: {action} ({item.seconds:.3f} s)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
