"""A checkout's lock-declared inputs, provisioned on demand.

``uv run`` syncs the environment from ``uv.lock`` before it runs, rustup
installs the toolchain that ``rust-toolchain.toml`` names on first use, and
``npm ci`` installs exactly its lock file. This module gives a checkout the
same contract for every input it declares (docs/design/landing_queue.md,
"Declarative checkout provisioning"). `CheckoutInputName` is the closed table:

- ``uv-environment``: the project environment that ``uv sync --frozen``
  builds from ``uv.lock``. ``--frozen`` installs the lock as written and
  resolves nothing. Installed editable packages also bind their source
  checkout; uv reads every package from its cache and uses the network only for a package the
  cache lacks. ``--locked`` would also re-resolve to check the lock against
  ``pyproject.toml``, which can need the index; that check belongs to the
  lock gate, not to provisioning. The sync is exact, as CI's setup is: it
  removes packages the lock does not declare, so a gate cannot pass on an
  undeclared import. ``uv run`` keeps owning its own (inexact) sync; this
  input exists so that ``lane start`` provisions a new worktree at once and
  ``integrate`` builds the environment its gates select.
- ``browser-acorn``: the Acorn parser tree under
  ``tools/browser_asset_graph``, from its ``package-lock.json``. ``npm ci``
  installs the lock exactly; ``--ignore-scripts`` runs no package code,
  ``--prefer-offline`` reads the npm cache first, and ``--no-audit
  --no-fund`` skip two registry round trips (0.19 s instead of 0.37 s to
  0.55 s from a warm cache, measured 2026-10-20).
- ``edgebox-typescript``: the Edgebox Worker's locked development tree.
  Corepack selects npm from its ``packageManager``; the same guarded,
  script-free ``npm ci`` and stamp contract as Acorn owns this tree. It is
  selected explicitly by its proof, rather than installed for every lane.
- ``git-hooks``: Molt's managed Git hooks in the Git common directory.
  ``tools/install_git_hooks.py`` owns their content and its own
  current/stale comparison (``--check``); this module only runs it. The
  hooks serve every worktree, so this input is repository-scoped: ``ensure``
  installs the sources of the checkout it is given. ``lane start`` runs it
  from a new lane at main's tip, so the installed hooks follow main.

Stamps
------
A locked tree keeps its stamp inside the tree it describes
(``node_modules/.molt-provisioned.json``, ``<environment>/.molt-provisioned.json``),
as Cargo keeps fingerprints inside ``target`` and npm keeps its hidden
lockfile in ``node_modules``. The stamp records the action digest (the input,
its provisioner arguments and the digest of each lock file) and the tool and
version that built the tree. A stamp in checkout scratch state could outlive
its tree; here, deleting the tree deletes the stamp. ``npm ci`` empties
``node_modules`` and ``uv`` recreates an environment it replaces, so either
tool removes a stale stamp itself. ``ensure`` deletes the stamp before it runs
the provisioner and writes it atomically after success, so a failed or
interrupted run leaves no stamp, and the next ``ensure`` runs the provisioner
again.

The stamp is keyed on the locked action, not on the tool binary. UV also
binds the resolved checkout and rechecks its exact editable PEP 610 record;
a foreign owner is refused rather than automatically rewritten. npm's
integrity-pinned tree remains location-independent. This host has two npm
installations (the shell's and the pinned Node release that toolchain
admission puts first on PATH); a key on the tool path would re-run ``npm ci``
at every switch and empty ``node_modules`` under a concurrent reader. The
tool and its version stay in the stamp as provenance.

Each provisioner runs under the memory guard, which fills unset variables
with dx defaults: a managed ``UV_PROJECT_ENVIRONMENT`` and a ``UV_CACHE_DIR``
on the artifact volume. The uv command therefore names its target tree and
the caller's uv cache explicitly (``uv cache dir`` when the caller sets
none). The tree is then the one ``uv run`` in that checkout selects, built by
a same-volume clone (1.0 s) instead of a 136 MB copy (14 s to 37 s). A
caller's cache on another volume cannot be cloned or hardlinked from, so a
tree there would copy; when dx's artifact-volume cache shares the tree's
filesystem, the command names that cache instead (2026-10-21, loaded USB
volume: a 137 MB environment took 1.85 s as a clone and 81.5 s as a copy).

Cost: a current UV input additionally reads its recorded editable PEP 610
metadata, when present, and validates its source path. It does not spawn a
probe or enumerate installed distributions. A cold UV admission uses the
selected interpreter with site startup disabled. The existing Git-scoped
provisioner lock is unchanged: this admission is not a reader lease or a
cross-checkout concurrent writer exclusion protocol.
A current input otherwise costs one read of the stamp, one read of each lock
file and one ``stat`` of the tree's sentinel file (0.07 ms for Acorn, 0.3 ms
for the 0.5 MB ``uv.lock``, measured 2026-10-20). A stale input takes its
existing Git-scoped lock, checks the stamp again, then runs
one guarded provisioner command. There is no retry, no offline-then-online
sequence and no fallback: a failure raises `ProvisioningError` with the
command and its output.
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
import tomllib
from typing import Literal, assert_never, get_args

from molt import process_guard
from molt.custody_layout import git_directories
from molt.exact_json import ExactJsonError, read_exact, write_exact
from molt.file_locks import _acquire_file_lock, _release_file_lock
from molt.process_environment import ProcessEnvironment

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

    def executable(self) -> str | None:
        if self.tool_variable and self.environment.get(self.tool_variable):
            return self.environment[self.tool_variable]
        return shutil.which(self.tool, path=self.environment.get("PATH"))


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


CheckoutInput = LockedTree | OwnedInstallation


@dataclass(frozen=True, slots=True)
class Ensured:
    name: str
    provisioned: bool
    seconds: float


def _uv_environment_path(checkout: Path, environ: Mapping[str, str]) -> Path:
    """The environment ``uv`` selects in ``checkout``: uv's own rule."""
    explicit = environ.get("UV_PROJECT_ENVIRONMENT", "").strip()
    if not explicit:
        return checkout / ".venv"
    path = Path(explicit).expanduser()
    return path if path.is_absolute() else checkout / path


def checkout_input(
    name: CheckoutInputName,
    checkout: Path,
    environ: Mapping[str, str] | None = None,
    *,
    guard_source_root: Path | None = None,
) -> CheckoutInput:
    """The declared input ``name`` of ``checkout`` under ``environ``."""
    _require_guard_source_owner(guard_source_root)
    checkout = checkout.expanduser().resolve()
    environment = ProcessEnvironment(os.environ if environ is None else environ)
    match name:
        case "uv-environment":
            tree = _uv_environment_path(checkout, environment)
            return LockedTree(
                name=name,
                guard_source_root=guard_source_root,
                checkout=checkout,
                lock_files=("uv.lock", "pyproject.toml", ".python-version"),
                tree=tree,
                sentinel="pyvenv.cfg",
                tool="uv",
                arguments=("sync", "--frozen"),
                # The guarded child would otherwise inherit dx's default
                # UV_PROJECT_ENVIRONMENT and sync a tree this stamp does not
                # describe. uv ignores a foreign VIRTUAL_ENV with a warning.
                environment=environment.without(("VIRTUAL_ENV",)).updated(
                    {"UV_PROJECT_ENVIRONMENT": str(tree)}
                ),
                # The lane's own `uv run` reads the caller's cache (see the
                # module docstring for the measured cost of dx's default).
                caller_choices=(("UV_CACHE_DIR", ("cache", "dir")),),
                same_volume_choices=("UV_CACHE_DIR",),
                tool_variable="UV",
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
                tool="corepack",
                arguments=(
                    "npm",
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

    ``uv run`` syncs only a project it finds. A declared input whose other
    lock files are missing still fails loud in ``ensure``.
    """
    if isinstance(spec, LockedTree):
        return (spec.checkout / spec.lock_files[0]).is_file()
    return (spec.checkout / spec.owner).is_file()


def locked_input_files(name: CheckoutInputName) -> tuple[str, ...]:
    """The checkout-relative lock files of a locked input; empty otherwise."""
    spec = checkout_input(name, Path(os.curdir), {})
    return spec.lock_files if isinstance(spec, LockedTree) else ()


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


def _lock_digests(spec: LockedTree) -> dict[str, str]:
    digests: dict[str, str] = {}
    for relative in spec.lock_files:
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
    # Default-cwd actions keep their existing digest/stamp. Corepack needs
    # the package directory to select its packageManager, not npm's --prefix.
    if spec.command_cwd != ".":
        digest.update(b"\2" + spec.command_cwd.encode("utf-8"))
    if spec.name == "uv-environment":
        # Editable output depends on the source location, unlike locked npm trees.
        digest.update(b"\3" + os.fsencode(spec.checkout.resolve()))
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
    if spec.name == "uv-environment":
        if record.get("checkout") != str(spec.checkout.resolve()):
            return "stale"
        if "editable_project" not in record:
            return "stale"
        editable = record["editable_project"]
        if editable is not None:
            try:
                _require_editable_owner(spec, editable)
            except (OSError, ValueError, ProvisioningError):
                return "stale"
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


def _tool_version(spec: LockedTree, executable: str) -> str:
    argv = [executable, "--version"]
    result = _run(
        spec.name,
        argv,
        cwd=spec.checkout,
        environment=spec.environment,
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


def _with_caller_choices(spec: LockedTree, executable: str) -> ProcessEnvironment:
    """Pin each choice the caller left to its tool, as the tool itself makes it.

    A choice in ``same_volume_choices`` is a store the tool clones or
    hardlinks from, so it must share the tree's filesystem; a value on another
    volume yields to dx's artifact-volume default when that one shares it, and
    stays otherwise (the tool then copies).
    """
    pinned: dict[str, str] = {}
    for variable, arguments in spec.caller_choices:
        if spec.environment.get(variable, "").strip():
            continue
        argv = [executable, *arguments]
        result = _run(
            spec.name,
            argv,
            cwd=spec.checkout,
            environment=spec.environment,
            guarded=False,
            guard_source_root=spec.guard_source_root,
        )
        lines = (result.stdout or "").strip().splitlines()
        if result.returncode != 0 or len(lines) != 1:
            raise _failure(spec.name, argv, spec.checkout, result)
        pinned[variable] = lines[0].strip()
    device = _device_of(spec.tree)
    for variable in spec.same_volume_choices:
        chosen = pinned.get(variable) or spec.environment.get(variable, "").strip()
        if not chosen or device is None or _device_of(Path(chosen)) == device:
            continue
        alternative = _dx_choice(variable, spec)
        if (
            alternative is not None
            and alternative.is_dir()
            and _device_of(alternative) == device
        ):
            pinned[variable] = str(alternative)
    return spec.environment.updated(pinned) if pinned else spec.environment


def _foreign_environment(spec: LockedTree, owner: str) -> ProvisioningError:
    return ProvisioningError(
        f"{spec.name}: {spec.tree} belongs to editable checkout {owner}, "
        f"not {spec.checkout}; refusing to rewrite another checkout's environment. "
        "Use MOLT_VENV to borrow it read-only, or select a distinct environment "
        "with tools/uv_project_env.py --python <version> --purpose <purpose>."
    )


def _require_editable_owner(spec: LockedTree, relative: object) -> None:
    from molt.python_environment_location import editable_direct_url_path
    from molt.python_identity_common import _valid_relative_payload_path

    if not isinstance(relative, str) or not _valid_relative_payload_path(relative):
        raise ProvisioningError(f"{spec.name}: invalid editable metadata path")
    metadata = spec.tree / relative
    try:
        if not metadata.resolve(strict=True).is_relative_to(spec.tree.resolve()):
            raise ValueError("editable metadata escapes environment")
        owner = editable_direct_url_path(metadata.read_bytes(), distribution="project")
    except (OSError, ValueError) as exc:
        raise ProvisioningError(
            f"{spec.name}: cannot verify {metadata}: {exc}"
        ) from exc
    if owner != spec.checkout.resolve():
        raise _foreign_environment(spec, str(owner))


def _uv_editable_project(spec: LockedTree) -> str | None:
    """Cold admission uses the existing isolated no-site Python probe."""
    if not (spec.tree / spec.sentinel).is_file():
        return None
    from molt.python_environment_identity import python_identity_probe_arguments
    from molt.python_environment_location import environment_python_path

    try:
        project = tomllib.loads(
            (spec.checkout / "pyproject.toml").read_text(encoding="utf-8")
        )
        name = project.get("project", {}).get("name")
        if not isinstance(name, str):
            return None
        argv = [
            str(environment_python_path(spec.tree)),
            *python_identity_probe_arguments(
                ("--editable-project", str(spec.tree), name), no_site=True
            ),
        ]
        result = _run(
            spec.name,
            argv,
            cwd=spec.checkout,
            environment=spec.environment,
            guarded=False,
            guard_source_root=spec.guard_source_root,
        )
        if result.returncode != 0:
            raise _failure(spec.name, argv, spec.checkout, result)
        relative = json.loads(result.stdout)
        if relative is not None:
            _require_editable_owner(spec, relative)
        return relative
    except (OSError, ValueError) as exc:
        raise ProvisioningError(
            f"{spec.name}: editable-source admission failed: {exc}"
        ) from exc


def _provision_locked(spec: LockedTree) -> bool:
    """Make ``spec`` current; True when this call ran its provisioner."""
    if _locked_state(spec) == "current":
        return False
    with _ProvisioningLock(spec):
        locks = _lock_digests(spec)
        digest = _action_digest(spec, locks)
        if _locked_state(spec, digest) == "current":
            return False  # Another process provisioned it while we waited.
        if spec.name == "uv-environment":
            _uv_editable_project(
                spec
            )  # Refuse foreign/invalid metadata before mutation.
        executable = spec.executable()
        if executable is None:
            raise ProvisioningError(
                f"{spec.name}: `{spec.tool}` is not on PATH; it builds "
                f"{spec.tree} from {', '.join(spec.lock_files)}"
            )
        environment = _with_caller_choices(spec, executable)
        spec.stamp.unlink(missing_ok=True)
        argv = [executable, *spec.arguments]
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
        source_binding = {}
        if spec.name == "uv-environment":
            source_binding = {
                "checkout": str(spec.checkout.resolve()),
                "editable_project": _uv_editable_project(spec),
            }
        write_exact(
            spec.stamp,
            {
                "schema": SCHEMA,
                **source_binding,
                "input": spec.name,
                "digest": digest,
                "locks": locks,
                "tool": {
                    "name": spec.tool,
                    "path": executable,
                    "version": _tool_version(spec, executable),
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
    return [sys.executable, "-I", "-S", str(spec.checkout / spec.owner), *arguments]


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
    if isinstance(spec, LockedTree):
        return _locked_state(spec)
    return _owned_state(spec)


def ensure_input(spec: CheckoutInput) -> Ensured:
    """Make ``spec`` current; raise `ProvisioningError` when it cannot be."""
    started = time.monotonic()
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
) -> Ensured:
    """Make checkout input ``name`` current, as ``uv run`` syncs first."""
    return ensure_input(
        checkout_input(name, checkout, environ, guard_source_root=guard_source_root)
    )


def ensure_all(
    checkout: Path,
    environ: Mapping[str, str] | None = None,
    names: Iterable[CheckoutInputName] = DEFAULT_CHECKOUT_INPUT_NAMES,
    *,
    guard_source_root: Path | None = None,
) -> tuple[Ensured, ...]:
    """Provision every named input; report every failure together.

    Each input is atomic: a failure leaves that input unstamped and keeps the
    inputs that did succeed.
    """
    done: list[Ensured] = []
    failures: list[str] = []
    for name in names:
        try:
            done.append(
                ensure(name, checkout, environ, guard_source_root=guard_source_root)
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
    parser.add_argument("action", choices=("ensure", "status"))
    parser.add_argument(
        "inputs",
        nargs="*",
        metavar="INPUT",
        help=f"one of {', '.join(CHECKOUT_INPUT_NAMES)} "
        f"(default: {', '.join(DEFAULT_CHECKOUT_INPUT_NAMES)})",
    )
    parser.add_argument("--checkout", type=Path, default=compiler_source_root())
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)
    unknown = sorted(set(args.inputs) - set(CHECKOUT_INPUT_NAMES))
    if unknown:
        parser.error(f"unknown checkout input: {', '.join(unknown)}")
    names = (
        tuple(name for name in CHECKOUT_INPUT_NAMES if name in args.inputs)
        if args.inputs
        else DEFAULT_CHECKOUT_INPUT_NAMES
    )
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
        done = ensure_all(args.checkout, names=names)
    except ProvisioningError as exc:
        print(exc, file=sys.stderr)
        return 1
    for item in done:
        action = "provisioned" if item.provisioned else "current"
        print(f"{item.name}: {action} ({item.seconds:.3f} s)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
