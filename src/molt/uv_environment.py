"""One uv-managed Python environment per key, shared by every checkout.

Owning design: ``docs/design/one_environment.md``. A key is an exact CPython
release and a set of dependency groups. The environment's directory name also
carries the digest of the lock inputs, so the directory is content-addressed:
its finished record proves it current, and the warm path is one ``stat``.

The project itself is not installed. Each checkout puts its own ``src`` first
on ``PYTHONPATH`` (``dx.ensure_repo_src_pythonpath``), so every worktree with
the same lock shares one environment instead of building its own.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import tomllib
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path

from molt.exact_json import canonical_json_sha256, read_exact, write_exact
from molt.file_locks import _acquire_file_lock, _release_file_lock
from molt.process_environment import ProcessEnvironment, ProcessEnvironmentBuilder

RECORD = "molt-environment.json"
SCHEMA = "molt.uv-environment.v1"
_MAX_RECORD_BYTES = 64 * 1024
SYNC_TIMEOUT_S = 1800.0
_LOCK_TIMEOUT_S = 3600.0
_RELEASE = re.compile(r"\d+\.\d+\.\d+")
_GROUP = re.compile(r"[a-z0-9][a-z0-9._-]*")
# The pyproject entries that decide what `uv sync --no-install-project`
# installs, besides `uv.lock` itself. Other edits (tool settings, the project's
# description) leave the environment unchanged and keep its directory.
_LOCK_INPUTS: tuple[tuple[str, ...], ...] = (
    ("project", "requires-python"),
    ("project", "dependencies"),
    ("project", "optional-dependencies"),
    ("dependency-groups",),
    ("tool", "uv"),
)
# One child prints the facts a receipt or a release record needs about the
# interpreter, so no consumer probes it again.
_IDENTITY_PROBE = (
    "import json, platform, sys, sysconfig\n"
    "print(json.dumps({\n"
    "    'implementation': sys.implementation.name,\n"
    "    'version': '.'.join(map(str, sys.version_info[:3])),\n"
    "    'hexversion': sys.hexversion,\n"
    "    'cache_tag': sys.implementation.cache_tag,\n"
    "    'abiflags': getattr(sys, 'abiflags', ''),\n"
    "    'gil_disabled': bool(sysconfig.get_config_var('Py_GIL_DISABLED')),\n"
    "    'machine': platform.machine(),\n"
    "    'platform': sys.platform,\n"
    "    'base_prefix': sys.base_prefix,\n"
    "}))\n"
)


class UvEnvironmentError(RuntimeError):
    """An environment could not be named or synced."""


@dataclass(frozen=True, slots=True, order=True)
class EnvironmentKey:
    """An exact CPython release and the dependency groups installed for it."""

    python: str
    groups: tuple[str, ...] = ()

    def __post_init__(self) -> None:
        if _RELEASE.fullmatch(self.python) is None:
            raise UvEnvironmentError(
                f"environment key needs an exact CPython release, not {self.python!r}"
            )
        if self.groups != tuple(sorted(set(self.groups))) or any(
            _GROUP.fullmatch(group) is None for group in self.groups
        ):
            raise UvEnvironmentError(
                f"environment groups must be sorted unique names: {self.groups!r}"
            )

    @property
    def component(self) -> str:
        return f"{self.python}-{'+'.join(self.groups) or 'base'}"


def pinned_python(checkout: Path) -> str:
    """The checkout's `.python-version`: the one interpreter pin."""
    path = checkout / ".python-version"
    try:
        return path.read_text(encoding="utf-8").strip()
    except OSError as exc:
        raise UvEnvironmentError(f"cannot read the interpreter pin {path}: {exc}")


def resolve_release(
    selector: str, checkout: Path, environ: Mapping[str, str]
) -> str:
    """The exact release ``selector`` names: itself, or uv's managed match.

    A minor selector (``3.13``) resolves to the managed interpreter uv finds
    for it; the key then names that exact release.
    """
    if _RELEASE.fullmatch(selector) is not None:
        return selector
    if re.fullmatch(r"\d+\.\d+", selector) is None:
        raise UvEnvironmentError(
            f"an environment needs a CPython release or minor, not {selector!r}"
        )
    uv = environ.get("UV", "").strip() or shutil.which("uv", path=environ.get("PATH"))
    if not uv:
        raise UvEnvironmentError("uv is not on PATH; install uv")
    found = _run(
        [uv, "python", "find", "--managed-python", "--no-project", selector],
        checkout=checkout,
        env=ProcessEnvironment(environ),
        what=f"uv python find {selector}",
    ).strip()
    version = _run(
        [found, "-I", "-c", "import platform; print(platform.python_version())"],
        checkout=checkout,
        env=ProcessEnvironment(environ),
        what="interpreter version probe",
    ).strip()
    if _RELEASE.fullmatch(version) is None or not version.startswith(f"{selector}."):
        raise UvEnvironmentError(f"uv found CPython {version!r} for {selector}")
    return version


def default_key(checkout: Path) -> EnvironmentKey:
    """The development environment: the pin and `[tool.molt.dx]` groups."""
    from molt.dx import DxConfigError, DxProject

    try:
        groups = DxProject(checkout).dependency_groups()
    except DxConfigError as exc:
        raise UvEnvironmentError(str(exc)) from exc
    return EnvironmentKey(pinned_python(checkout), groups)


def _table_entry(document: Mapping[str, object], path: Sequence[str]) -> object:
    value: object = document
    for name in path:
        if not isinstance(value, Mapping) or name not in value:
            return None
        value = value[name]
    return value


def lock_digest(checkout: Path) -> str:
    """The digest of everything that decides an environment's contents."""
    try:
        lock = (checkout / "uv.lock").read_bytes()
        pyproject = tomllib.loads(
            (checkout / "pyproject.toml").read_text(encoding="utf-8")
        )
    except (OSError, tomllib.TOMLDecodeError) as exc:
        raise UvEnvironmentError(f"cannot read the lock inputs: {exc}") from exc
    declared = {
        ".".join(path): _table_entry(pyproject, path) for path in _LOCK_INPUTS
    }
    digest = hashlib.sha256(lock)
    digest.update(canonical_json_sha256(declared).encode("ascii"))
    return digest.hexdigest()


def environments_root(checkout: Path, environ: Mapping[str, str]) -> Path:
    """The one directory that holds every checkout's shared environments."""
    from molt.custody_layout import storage_rule
    from molt.dx import artifact_root

    return artifact_root(checkout, environ).joinpath(
        *storage_rule("python-environment").relative
    )


def environment_root(
    checkout: Path, key: EnvironmentKey, environ: Mapping[str, str]
) -> Path:
    """Where ``key``'s environment for ``checkout``'s lock lives."""
    return environments_root(checkout, environ) / (
        f"{key.component}-{lock_digest(checkout)[:16]}"
    )


def python_path(root: Path) -> Path:
    """The interpreter entry of a standard environment (not its base)."""
    return root / ("Scripts/python.exe" if os.name == "nt" else "bin/python")


def bytecode_prefix(root: Path) -> Path:
    """The shared bytecode cache of every environment beside ``root``.

    It mirrors each source's absolute path and names each file by cache tag,
    so checkouts and interpreters never collide. Bytecode stays out of the
    checkouts and out of the environments.
    """
    return root.parent / ".bytecode"


def bytecode_arguments(root: Path) -> tuple[str, str]:
    """Interpreter options that select ``root``'s bytecode cache.

    ``-I`` drops ``PYTHONPYCACHEPREFIX``; a launch that isolates the
    interpreter passes these instead.
    """
    return ("-X", f"pycache_prefix={bytecode_prefix(root)}")


def read_record(root: Path) -> dict[str, object] | None:
    """The finished record of ``root``, or None while it is absent or partial."""
    try:
        record = read_exact(
            root / RECORD, max_bytes=_MAX_RECORD_BYTES, label="environment record"
        )
    except FileNotFoundError:
        return None
    except (OSError, ValueError) as exc:
        cause = exc.__cause__
        if isinstance(cause, FileNotFoundError):
            return None
        raise UvEnvironmentError(f"unreadable environment record: {exc}") from exc
    if not isinstance(record, dict) or record.get("schema") != SCHEMA:
        raise UvEnvironmentError(f"environment record has another schema: {root}")
    return record


def selected(
    checkout: Path,
    environ: Mapping[str, str],
    key: EnvironmentKey | None = None,
) -> Path | None:
    """The finished environment of ``key`` (default: development), if any.

    A path getter: it never syncs. ``UV_PROJECT_ENVIRONMENT`` names an
    explicit environment that wins over the key.
    """
    explicit = environ.get("UV_PROJECT_ENVIRONMENT", "").strip()
    if explicit:
        path = Path(explicit).expanduser()
        return (path if path.is_absolute() else checkout / path).absolute()
    root = environment_root(checkout, key or default_key(checkout), environ)
    return root if (root / RECORD).is_file() else None


def _sync_environment(environ: Mapping[str, str], root: Path) -> ProcessEnvironment:
    child = ProcessEnvironmentBuilder(environ)
    for name in ("VIRTUAL_ENV", "UV_NO_SYNC", "PYTHONHOME", "PYTHONPATH"):
        child.pop(name, None)
    child["UV_PROJECT_ENVIRONMENT"] = str(root)
    cache = child.get("UV_CACHE_DIR", "").strip()
    if cache and not _shares_volume(Path(cache), root.parent):
        # uv links files from its cache into the environment; across volumes
        # it copies every file instead (130.7 s against 12.9 s, 2026-11-20).
        child.pop("UV_CACHE_DIR")
    return ProcessEnvironment(child)


def _shares_volume(path: Path, directory: Path) -> bool:
    path = path.expanduser()
    while not path.exists() and path != path.parent:
        path = path.parent
    try:
        return path.stat().st_dev == directory.stat().st_dev
    except OSError:
        return False


def sync_argv(uv: str, key: EnvironmentKey) -> list[str]:
    """The one uv command that makes ``key``'s environment current."""
    argv = [
        uv,
        "sync",
        "--locked",
        "--no-install-project",
        "--managed-python",
        "--python",
        key.python,
        "--no-default-groups",
    ]
    for group in key.groups:
        argv.extend(("--group", group))
    return argv


def _run(
    argv: Sequence[str], *, checkout: Path, env: ProcessEnvironment, what: str
) -> str:
    from molt import process_guard

    completed = process_guard.run_completed_command(
        list(argv),
        env=env,
        cwd=checkout,
        timeout=SYNC_TIMEOUT_S,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        tail = "\n".join((completed.stderr or "").strip().splitlines()[-20:])
        raise UvEnvironmentError(f"{what} failed (exit {completed.returncode}):\n{tail}")
    return completed.stdout or ""


def environment(
    checkout: Path,
    key: EnvironmentKey | None = None,
    *,
    environ: Mapping[str, str] | None = None,
) -> Path:
    """Return ``key``'s current environment, syncing it on first use.

    The warm path is one ``stat`` of the finished record. A first use takes
    the environment's lock, runs ``uv sync`` once, records the interpreter's
    identity and publishes the record last, so a crash leaves no record and
    the next use syncs again.
    """
    checkout = checkout.expanduser().resolve()
    source = os.environ if environ is None else environ
    key = key or default_key(checkout)
    root = environment_root(checkout, key, source)
    if (root / RECORD).is_file():
        return root
    root.parent.mkdir(parents=True, exist_ok=True)
    lock = root.with_name(f"{root.name}.lock")
    try:
        handle = _acquire_file_lock(
            lock,
            timeout_s=_LOCK_TIMEOUT_S,
            timeout_message=f"another process held {lock} for {_LOCK_TIMEOUT_S:.0f} s",
        )
    except RuntimeError as exc:
        raise UvEnvironmentError(str(exc)) from exc
    try:
        if (root / RECORD).is_file():
            return root
        uv = source.get("UV", "").strip() or shutil.which("uv", path=source.get("PATH"))
        if not uv:
            raise UvEnvironmentError("uv is not on PATH; install uv")
        env = _sync_environment(source, root)
        _run(sync_argv(uv, key), checkout=checkout, env=env, what="uv sync")
        identity = json.loads(
            _run(
                [str(python_path(root)), "-I", "-c", _IDENTITY_PROBE],
                checkout=checkout,
                env=env,
                what="interpreter identity probe",
            )
        )
        if identity.get("version") != key.python:
            raise UvEnvironmentError(
                f"uv selected CPython {identity.get('version')}, not {key.python}"
            )
        write_exact(
            root / RECORD,
            {
                "schema": SCHEMA,
                "python": key.python,
                "groups": list(key.groups),
                "lock_digest": lock_digest(checkout),
                "interpreter": identity,
            },
        )
    finally:
        _release_file_lock(handle)
    return root


def interpreter_identity(root: Path) -> dict[str, object]:
    """The recorded identity of ``root``'s interpreter (see ``environment``)."""
    record = read_record(root)
    if record is None:
        raise UvEnvironmentError(f"environment has no finished record: {root}")
    identity = record.get("interpreter")
    if not isinstance(identity, dict):
        raise UvEnvironmentError(f"environment record names no interpreter: {root}")
    return identity
