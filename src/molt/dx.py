from __future__ import annotations

from collections.abc import Collection
import hashlib
import json
import os
import platform
import shlex
import shutil
import tempfile
import tomllib
import uuid
from pathlib import Path
from typing import Literal, Mapping, Sequence, cast


TEST_PYTHONS = ["3.12", "3.13", "3.14"]
CANONICAL_ROOT_ENV_KEYS = (
    "MOLT_EXT_ROOT",
    "CARGO_TARGET_DIR",
    "MOLT_DIFF_CARGO_TARGET_DIR",
    "MOLT_CACHE",
    "MOLT_DIFF_ROOT",
    "MOLT_DIFF_TMPDIR",
    "UV_CACHE_DIR",
    "UV_PROJECT_ENVIRONMENT",
    "PIP_CACHE_DIR",
    "PYTHONPYCACHEPREFIX",
    "TMPDIR",
    "TMP",
    "TEMP",
)
CANONICAL_RUN_ENV_KEYS = (
    *CANONICAL_ROOT_ENV_KEYS,
    "CARGO_INCREMENTAL",
    "MOLT_SESSION_ID",
)
DX_ENV_KEYS = (
    *CANONICAL_RUN_ENV_KEYS,
    "MOLT_BACKEND_DAEMON_SOCKET_DIR",
    "MOLT_USE_SCCACHE",
    "MOLT_DIFF_ALLOW_RUSTC_WRAPPER",
    "SCCACHE_DIR",
    "SCCACHE_CACHE_SIZE",
    "MOLT_CACHE_MAX_GB",
    "MOLT_CACHE_MAX_AGE_DAYS",
)
DEFAULT_POSIX_EXTERNAL_ARTIFACT_ROOTS = (
    "/Volumes/VertigoDataTier/Molt",
    "/Volumes/APDataStore/Molt",
)
DEFAULT_WINDOWS_EXTERNAL_ARTIFACT_DIRNAME = "Molt"
DEFAULT_SCCACHE_CACHE_SIZE = "10G"
DEFAULT_MOLT_CACHE_MAX_GB = "30"
DEFAULT_MOLT_CACHE_MAX_AGE_DAYS = "30"
DEVELOPMENT_ARTIFACT_REQUEST_ENV_KEYS = (
    "MOLT_REQUIRE_EXTERNAL_ARTIFACTS",
    "MOLT_PREFER_EXTERNAL_ARTIFACTS",
    "MOLT_USE_EXTERNAL_ARTIFACTS",
)
TRUE_VALUES = {"1", "true", "yes", "on"}
FALSE_VALUES = {"0", "false", "no", "off"}


class DxConfigError(RuntimeError):
    pass


def session_artifact_component(session_id: str) -> str:
    return "".join(c if c.isalnum() or c in "-_" else "_" for c in session_id)[:32]


def session_scoped_target_dir(target_root: Path, session_id: str | None) -> Path:
    if session_id:
        return target_root / "sessions" / session_artifact_component(session_id)
    return target_root


def cargo_target_dir_for_artifact_root(
    artifact_root: Path,
    session_id: str | None,
) -> Path:
    return session_scoped_target_dir(artifact_root / "target", session_id)


def _env_bool(
    env: Mapping[str, str],
    names: Collection[str],
    *,
    default: bool,
) -> bool:
    for name in names:
        raw = env.get(name)
        if raw is None:
            continue
        normalized = raw.strip().lower()
        if normalized in TRUE_VALUES:
            return True
        if normalized in FALSE_VALUES:
            return False
    return default


def _env_float(
    env: Mapping[str, str],
    name: str,
    *,
    default: float,
) -> float:
    raw = env.get(name, "").strip()
    if not raw:
        return default
    try:
        parsed = float(raw)
    except ValueError:
        return default
    return parsed if parsed >= 0 else default


def development_artifacts_requested(env: Mapping[str, str]) -> bool:
    """Return whether a development wrapper requested guarded artifact custody.

    This is intentionally a development control-plane predicate. Public compile
    paths keep Cargo/default output behavior unless the operator set one of
    these Molt development knobs or an explicit output/target flag.
    """

    return _env_bool(env, DEVELOPMENT_ARTIFACT_REQUEST_ENV_KEYS, default=False)


def _looks_like_ambient_tmpdir(raw: str) -> bool:
    spelling = raw.strip().replace("\\", "/")
    if spelling in {"/tmp", "/var/tmp"} or spelling.startswith("/var/folders/"):
        return True
    lowered = spelling.lower()
    if (
        lowered.endswith("/appdata/local/temp")
        or "/appdata/local/temp/" in lowered
        or lowered in {"c:/windows/temp", "c:/temp", "c:/tmp"}
        or lowered.startswith("c:/windows/temp/")
        or lowered.startswith("c:/temp/")
        or lowered.startswith("c:/tmp/")
    ):
        return True
    normalized = str(Path(raw).expanduser()).rstrip(os.sep)
    return normalized in {"/tmp", "/var/tmp"} or normalized.startswith("/var/folders/")


def _drop_ambient_tmpdir(env: dict[str, str], *, prefer_external: bool) -> None:
    if not prefer_external:
        return
    if _env_bool(env, ("MOLT_PRESERVE_AMBIENT_TMPDIR",), default=False):
        return
    for key in ("TMPDIR", "TMP", "TEMP"):
        raw = env.get(key)
        if raw and _looks_like_ambient_tmpdir(raw):
            env.pop(key, None)


def _dedupe_paths(paths: list[Path]) -> tuple[Path, ...]:
    seen: set[str] = set()
    deduped: list[Path] = []
    for path in paths:
        key = os.path.normcase(str(path))
        if key in seen:
            continue
        seen.add(key)
        deduped.append(path)
    return tuple(deduped)


def _default_external_artifact_roots(env: Mapping[str, str]) -> tuple[Path, ...]:
    roots: list[Path] = []
    if os.name == "nt":
        roots.extend(_default_windows_external_artifact_roots())
    else:
        roots.extend(
            Path(path).expanduser() for path in DEFAULT_POSIX_EXTERNAL_ARTIFACT_ROOTS
        )
    return _dedupe_paths(roots)


def _default_windows_external_artifact_roots() -> tuple[Path, ...]:
    roots: list[Path] = []
    for letter in "DEFGHIJKLMNOPQRSTUVWXYZ":
        drive_root = Path(f"{letter}:\\")
        if drive_root.exists():
            roots.append(drive_root / DEFAULT_WINDOWS_EXTERNAL_ARTIFACT_DIRNAME)
    return tuple(roots)


def _requires_external_artifacts(
    repo_root: Path,
    env: Mapping[str, str],
    *,
    prefer_external: bool,
) -> bool:
    del repo_root, prefer_external
    if _env_bool(env, ("MOLT_ALLOW_C_DRIVE_ARTIFACTS",), default=False):
        return False
    return _env_bool(env, ("MOLT_REQUIRE_EXTERNAL_ARTIFACTS",), default=False)


def _allow_c_drive_artifacts(env: Mapping[str, str]) -> bool:
    return _env_bool(env, ("MOLT_ALLOW_C_DRIVE_ARTIFACTS",), default=False)


def _is_windows_c_drive_path(path: Path) -> bool:
    return os.name == "nt" and path.drive.upper() == "C:"


def _reject_c_drive_artifact_path(
    key: str,
    path: Path,
    env: Mapping[str, str],
    *,
    repo_root: Path,
    prefer_external: bool,
) -> None:
    if not _requires_external_artifacts(
        repo_root,
        env,
        prefer_external=prefer_external,
    ):
        return
    if _allow_c_drive_artifacts(env):
        return
    if _is_windows_c_drive_path(path.resolve()):
        raise DxConfigError(
            f"{key} resolved to {path}; Molt build artifacts must live on an "
            "external non-C: drive. Set MOLT_EXTERNAL_ARTIFACT_ROOTS=E:\\Molt "
            "or another external root, or set MOLT_ALLOW_C_DRIVE_ARTIFACTS=1 "
            "only for an explicit emergency override."
        )


def _candidate_roots(env: Mapping[str, str]) -> tuple[Path, ...]:
    raw = (
        env.get("MOLT_EXTERNAL_ARTIFACT_ROOTS")
        or env.get("MOLT_EXTERNAL_ARTIFACT_CANDIDATES")
        or ""
    )
    candidates = raw.split(os.pathsep) if raw.strip() else ()
    roots: list[Path] = []
    for candidate in candidates:
        text = candidate.strip()
        if not text:
            continue
        roots.append(Path(text).expanduser())
    return _dedupe_paths(roots) if roots else _default_external_artifact_roots(env)


def _nearest_existing_parent(path: Path) -> Path | None:
    current = path
    while not current.exists():
        parent = current.parent
        if parent == current:
            return None
        current = parent
    return current if current.is_dir() else current.parent


def _artifact_root_accepts_child_dirs(path: Path, *, create_dirs: bool) -> bool:
    if not create_dirs:
        parent = _nearest_existing_parent(path)
        return parent is not None and os.access(parent, os.W_OK)
    probe = path / f".molt-write-probe-{os.getpid()}-{uuid.uuid4().hex}"
    try:
        path.mkdir(parents=True, exist_ok=True)
        probe.mkdir()
        list(probe.iterdir())
    except OSError:
        return False
    finally:
        try:
            shutil.rmtree(probe)
        except OSError:
            pass
    return True


def select_external_artifact_root(
    repo_root: Path,
    env: Mapping[str, str],
    *,
    create_dirs: bool,
    prefer_external: bool,
) -> Path | None:
    """Return the first healthy external artifact root, or None for repo-local."""

    if env.get("MOLT_EXT_ROOT"):
        return None
    require_external = _requires_external_artifacts(
        repo_root,
        env,
        prefer_external=prefer_external,
    )
    if not _env_bool(
        env,
        ("MOLT_PREFER_EXTERNAL_ARTIFACTS", "MOLT_USE_EXTERNAL_ARTIFACTS"),
        default=prefer_external,
    ) and not require_external:
        return None

    min_free_gb = _env_float(env, "MOLT_EXTERNAL_MIN_FREE_GB", default=20.0)
    repo_root = repo_root.resolve()
    for raw_candidate in _candidate_roots(env):
        candidate = (
            raw_candidate if raw_candidate.is_absolute() else repo_root / raw_candidate
        )
        candidate = candidate.resolve()
        if candidate == repo_root or repo_root in candidate.parents:
            continue
        parent = _nearest_existing_parent(candidate)
        if parent is None:
            continue
        try:
            usage = shutil.disk_usage(parent)
        except OSError:
            continue
        if usage.free < min_free_gb * 1024 * 1024 * 1024:
            continue
        if not _artifact_root_accepts_child_dirs(
            candidate,
            create_dirs=create_dirs,
        ):
            continue
        return candidate
    if require_external:
        raise DxConfigError(
            "no healthy external non-C: Molt artifact root was found. Attach an "
            "external drive or set MOLT_EXTERNAL_ARTIFACT_ROOTS=E:\\Molt with "
            "sufficient free space; set MOLT_ALLOW_C_DRIVE_ARTIFACTS=1 only for "
            "an explicit emergency override."
        )
    return None


def require_external_artifact_root(
    repo_root: Path,
    env: Mapping[str, str],
    *,
    create_dirs: bool,
    prefer_external: bool,
) -> Path | None:
    selected = select_external_artifact_root(
        repo_root,
        env,
        create_dirs=create_dirs,
        prefer_external=prefer_external,
    )
    if selected is not None:
        return selected
    if _requires_external_artifacts(
        repo_root,
        env,
        prefer_external=prefer_external,
    ):
        candidates = ", ".join(str(path) for path in _candidate_roots(env)) or "<none>"
        raise DxConfigError(
            "Molt build artifacts must not be placed on C:. Configure a healthy "
            "non-C artifact root with MOLT_EXTERNAL_ARTIFACT_ROOTS or MOLT_EXT_ROOT. "
            f"Checked candidates: {candidates}"
        )
    return None


def _validate_windows_artifact_root(
    artifact_root: Path,
    *,
    repo_root: Path,
    env: Mapping[str, str],
    prefer_external: bool,
) -> None:
    if not _requires_external_artifacts(
        repo_root,
        env,
        prefer_external=prefer_external,
    ):
        return
    if not _is_windows_c_drive_path(artifact_root.resolve()):
        return
    raise DxConfigError(
        "Molt build artifacts must not be placed on C:. "
        f"Rejected artifact root: {artifact_root}"
    )


def _backend_daemon_socket_root(env: Mapping[str, str]) -> Path:
    raw = env.get("MOLT_BACKEND_DAEMON_SOCKET_ROOT", "").strip()
    if raw:
        return Path(raw).expanduser()
    for key in ("TMPDIR", "TMP", "TEMP"):
        raw = env.get(key, "").strip()
        if raw:
            return Path(raw).expanduser()
    if os.name == "nt":
        return Path(tempfile.gettempdir())
    return Path("/tmp")


def backend_daemon_socket_dir(repo_root: Path, env: Mapping[str, str]) -> Path:
    """Resolve the short local backend-daemon socket directory for this checkout."""

    root_hash = hashlib.sha256(str(repo_root.resolve()).encode()).hexdigest()[:12]
    return (_backend_daemon_socket_root(env) / f"molt-backend-{root_hash}").resolve()


def _install_dx_defaults(repo_root: Path, env: dict[str, str]) -> None:
    artifact_root = Path(env["MOLT_EXT_ROOT"]).expanduser()
    env.setdefault(
        "MOLT_BACKEND_DAEMON_SOCKET_DIR",
        str(backend_daemon_socket_dir(repo_root, env)),
    )
    env.setdefault("MOLT_USE_SCCACHE", "1")
    env.setdefault("MOLT_DIFF_ALLOW_RUSTC_WRAPPER", "1")
    env.setdefault("SCCACHE_DIR", str((artifact_root / ".sccache").resolve()))
    env.setdefault("SCCACHE_CACHE_SIZE", DEFAULT_SCCACHE_CACHE_SIZE)
    env.setdefault("MOLT_CACHE_MAX_GB", DEFAULT_MOLT_CACHE_MAX_GB)
    env.setdefault("MOLT_CACHE_MAX_AGE_DAYS", DEFAULT_MOLT_CACHE_MAX_AGE_DAYS)


def _host_facts() -> dict[str, str]:
    return {
        "os": platform.system().lower() or os.name,
        "platform": platform.platform(),
        "arch": platform.machine().lower(),
        "python": platform.python_version(),
    }


def dx_env_payload(env: Mapping[str, str], keys: Sequence[str]) -> dict[str, object]:
    return {
        "schema_version": "1.0",
        "kind": "molt_dx_env",
        "host": _host_facts(),
        "keys": list(keys),
        "env": {key: env[key] for key in keys if key in env},
    }


def _posix_quote(value: str) -> str:
    escaped = (
        value.replace("\\", "\\\\")
        .replace('"', '\\"')
        .replace("$", "\\$")
        .replace("`", "\\`")
    )
    return f'"{escaped}"'


def _powershell_quote(value: str) -> str:
    return "'" + value.replace("'", "''") + "'"


def _cmd_quote(value: str) -> str:
    return value.replace("^", "^^").replace("&", "^&").replace("|", "^|")


EnvRenderFormat = Literal["dotenv", "posix", "powershell", "cmd", "json"]


def render_env(
    env: Mapping[str, str], keys: Sequence[str], fmt: EnvRenderFormat
) -> str:
    present = [(key, env[key]) for key in keys if key in env]
    if fmt == "json":
        return json.dumps(dx_env_payload(env, keys), indent=2, sort_keys=True)
    if fmt == "posix":
        return "\n".join(
            f"export {key}={_posix_quote(value)}" for key, value in present
        )
    if fmt == "powershell":
        return "\n".join(
            f"$env:{key} = {_powershell_quote(value)}" for key, value in present
        )
    if fmt == "cmd":
        return "\n".join(f'set "{key}={_cmd_quote(value)}"' for key, value in present)
    return "\n".join(f"{key}={value}" for key, value in present)


class RunContext:
    """Canonical artifact roots and session identity for dev subprocesses."""

    def __init__(
        self,
        root: Path,
        *,
        session_prefix: str = "dev",
        prefer_external_artifacts: bool = False,
    ) -> None:
        self.root = root.expanduser().resolve()
        self.session_prefix = session_prefix
        self.prefer_external_artifacts = prefer_external_artifacts

    def _resolve_env_path(self, raw: str) -> Path:
        path = Path(raw).expanduser()
        if not path.is_absolute():
            path = self.root / path
        return path.resolve()

    def uv_project_env_dir(self, env: Mapping[str, str]) -> Path:
        explicit = env.get("UV_PROJECT_ENVIRONMENT", "").strip()
        if explicit:
            return self._resolve_env_path(explicit)
        ext_root = self._resolve_env_path(env.get("MOLT_EXT_ROOT", str(self.root)))
        session = env.get("MOLT_SESSION_ID", f"{self.session_prefix}-{os.getpid()}")
        return (ext_root / "tmp" / "uv-project-envs" / session).resolve()

    def canonical_env(
        self,
        base: Mapping[str, str] | None = None,
        *,
        create_dirs: bool = True,
        force_default_keys: Collection[str] = (),
    ) -> dict[str, str]:
        env = dict(os.environ if base is None else base)
        _drop_ambient_tmpdir(env, prefer_external=self.prefer_external_artifacts)
        forced = set(force_default_keys)

        if "MOLT_EXT_ROOT" in forced or not env.get("MOLT_EXT_ROOT"):
            ext_root = (
                None
                if "MOLT_EXT_ROOT" in forced
                else require_external_artifact_root(
                    self.root,
                    env,
                    create_dirs=create_dirs,
                    prefer_external=self.prefer_external_artifacts,
                )
            ) or self.root
        else:
            ext_root = self._resolve_env_path(env["MOLT_EXT_ROOT"])
        _validate_windows_artifact_root(
            ext_root,
            repo_root=self.root,
            env=env,
            prefer_external=self.prefer_external_artifacts,
        )
        env["MOLT_EXT_ROOT"] = str(ext_root)

        def install_default(key: str, value: Path | str) -> None:
            if key in forced or not env.get(key):
                env[key] = str(value)

        install_default("MOLT_SESSION_ID", f"{self.session_prefix}-{os.getpid()}")
        install_default(
            "CARGO_TARGET_DIR",
            cargo_target_dir_for_artifact_root(ext_root, env.get("MOLT_SESSION_ID")),
        )
        install_default("MOLT_DIFF_CARGO_TARGET_DIR", env["CARGO_TARGET_DIR"])
        install_default("CARGO_INCREMENTAL", "0")
        install_default("MOLT_CACHE", ext_root / ".molt_cache")
        install_default("MOLT_DIFF_ROOT", ext_root / "tmp" / "diff")
        install_default("MOLT_DIFF_TMPDIR", ext_root / "tmp")
        install_default("UV_CACHE_DIR", ext_root / ".uv-cache")
        install_default("UV_PROJECT_ENVIRONMENT", self.uv_project_env_dir(env))
        install_default("PIP_CACHE_DIR", ext_root / ".pip-cache")
        install_default("PYTHONPYCACHEPREFIX", ext_root / "tmp" / "pycache")
        install_default("TMPDIR", ext_root / "tmp")
        install_default("TMP", env["TMPDIR"])
        install_default("TEMP", env["TMPDIR"])

        for key in CANONICAL_ROOT_ENV_KEYS:
            value = env.get(key)
            if value:
                env[key] = str(self._resolve_env_path(value))
                value = env[key]
                _reject_c_drive_artifact_path(
                    key,
                    Path(value).expanduser(),
                    env,
                    repo_root=self.root,
                    prefer_external=self.prefer_external_artifacts,
                )

        if create_dirs:
            for key in CANONICAL_ROOT_ENV_KEYS:
                value = env.get(key)
                if value:
                    Path(value).expanduser().mkdir(parents=True, exist_ok=True)
        return env

    def dx_env(
        self,
        base: Mapping[str, str] | None = None,
        *,
        create_dirs: bool = True,
        force_default_keys: Collection[str] = (),
    ) -> dict[str, str]:
        env = self.canonical_env(
            base,
            create_dirs=create_dirs,
            force_default_keys=force_default_keys,
        )
        _install_dx_defaults(self.root, env)
        if create_dirs:
            for key in ("MOLT_BACKEND_DAEMON_SOCKET_DIR", "SCCACHE_DIR"):
                value = env.get(key)
                if value:
                    Path(value).expanduser().mkdir(parents=True, exist_ok=True)
        return env


def development_artifact_env(
    repo_root: Path,
    base: Mapping[str, str] | None = None,
    *,
    session_prefix: str = "dev",
    session_id: str | None = None,
    create_dirs: bool = True,
) -> dict[str, str]:
    """Resolve Molt developer build/cache/temp roots through the DX authority."""

    env = dict(os.environ if base is None else base)
    if session_id:
        env["MOLT_SESSION_ID"] = session_id
    env = RunContext(
        repo_root,
        session_prefix=session_prefix,
        prefer_external_artifacts=True,
    ).dx_env(env, create_dirs=create_dirs)
    src = repo_root.resolve() / "src"
    existing = env.get("PYTHONPATH", "")
    parts = [part for part in existing.split(os.pathsep) if part]
    if str(src) not in parts:
        env["PYTHONPATH"] = str(src) if not existing else f"{src}{os.pathsep}{existing}"
    return env


class DxProject:
    def __init__(self, root: Path) -> None:
        self.root = root.resolve()

    @classmethod
    def from_current_repo(cls) -> "DxProject":
        return cls(Path(__file__).resolve().parents[2])

    def load_config(self) -> dict[str, object]:
        pyproject = self.root / "pyproject.toml"
        if not pyproject.exists():
            return {}
        with pyproject.open("rb") as fh:
            data = tomllib.load(fh)
        tool = data.get("tool", {})
        if not isinstance(tool, dict):
            return {}
        molt = tool.get("molt", {})
        if not isinstance(molt, dict):
            return {}
        dx = molt.get("dx", {})
        return dx if isinstance(dx, dict) else {}

    def commands(self) -> dict[str, object]:
        commands = self.load_config().get("commands", {})
        return cast(dict[str, object], commands) if isinstance(commands, dict) else {}

    def project_env_dir(self) -> Path:
        return self.root / ".venv"

    def uv_project_env_dir(self, env: Mapping[str, str]) -> Path:
        explicit = env.get("UV_PROJECT_ENVIRONMENT", "").strip()
        if explicit:
            path = Path(explicit).expanduser()
            if not path.is_absolute():
                path = self.root / path
            return path.resolve()
        artifact_root = Path(env.get("MOLT_EXT_ROOT", str(self.root))).expanduser()
        if not artifact_root.is_absolute():
            artifact_root = self.root / artifact_root
        session = env.get("MOLT_SESSION_ID", f"dev-{os.getpid()}")
        return (artifact_root.resolve() / "tmp" / "uv-project-envs" / session).resolve()

    def project_python(self, env: Mapping[str, str] | None = None) -> Path:
        if env is not None:
            project_env = self.uv_project_env_dir(env)
            if os.name == "nt":
                return project_env / "Scripts" / "python.exe"
            return project_env / "bin" / "python3"
        if os.name == "nt":
            return self.project_env_dir() / "Scripts" / "python.exe"
        return self.project_env_dir() / "bin" / "python3"

    def normalized_uv_run_env(
        self,
        env: Mapping[str, str],
        *,
        python: str | None,
        project_env_matches_python: bool | None = None,
    ) -> dict[str, str]:
        run_env = dict(env)
        run_env.setdefault("PYTHONUNBUFFERED", "1")
        run_env["UV_PROJECT_ENVIRONMENT"] = str(self.uv_project_env_dir(run_env))
        for name in ("VIRTUAL_ENV", "PYTHONHOME", "CONDA_PREFIX", "CONDA_DEFAULT_ENV"):
            run_env.pop(name, None)
        if run_env.get("UV_NO_SYNC") == "1":
            env_matches = project_env_matches_python
            if env_matches is None:
                raise DxConfigError(
                    "UV_NO_SYNC normalization requires a guarded project "
                    "Python version probe result"
                )
            if not env_matches:
                run_env.pop("UV_NO_SYNC", None)
        return run_env

    def canonical_env(
        self,
        base: Mapping[str, str] | None = None,
        *,
        create_dirs: bool = True,
    ) -> dict[str, str]:
        dx = self.load_config()
        env = dict(os.environ if base is None else base)
        for name in ("VIRTUAL_ENV", "PYTHONHOME", "CONDA_PREFIX", "CONDA_DEFAULT_ENV"):
            env.pop(name, None)
        prefer_external = bool(dx.get("prefer_external_artifacts"))
        _drop_ambient_tmpdir(env, prefer_external=prefer_external)
        if env.get("MOLT_EXT_ROOT"):
            artifact_root = Path(env["MOLT_EXT_ROOT"]).expanduser()
            if not artifact_root.is_absolute():
                artifact_root = self.root / artifact_root
            artifact_root = artifact_root.resolve()
        else:
            artifact_root = (
                require_external_artifact_root(
                    self.root,
                    env,
                    create_dirs=create_dirs,
                    prefer_external=prefer_external,
                )
                or self.root
            )
        _validate_windows_artifact_root(
            artifact_root,
            repo_root=self.root,
            env=env,
            prefer_external=prefer_external,
        )
        env_cfg = dx.get("env", {})
        if isinstance(env_cfg, dict):
            for key, raw_value in env_cfg.items():
                if not isinstance(key, str) or not isinstance(raw_value, str):
                    continue
                if key in CANONICAL_RUN_ENV_KEYS and env.get(key):
                    continue
                value = raw_value.format(
                    root=str(self.root),
                    artifact_root=str(artifact_root),
                )
                if key in CANONICAL_ROOT_ENV_KEYS or key == "PYTHONPATH":
                    value = str(Path(value).expanduser().resolve())
                env[key] = value
        env = RunContext(
            self.root,
            session_prefix="dev",
            prefer_external_artifacts=prefer_external,
        ).canonical_env(
            env,
            create_dirs=create_dirs,
        )
        env.setdefault("MOLT_SESSION_ID", f"dev-{os.getpid()}")
        env.setdefault("MOLT_BACKEND_DAEMON", "1" if dx.get("backend_daemon") else "0")
        env.setdefault("CARGO_BUILD_JOBS", str(dx.get("cargo_build_jobs", 2)))
        return env

    def dx_env(
        self,
        base: Mapping[str, str] | None = None,
        *,
        create_dirs: bool = True,
    ) -> dict[str, str]:
        env = self.canonical_env(base, create_dirs=create_dirs)
        _install_dx_defaults(self.root, env)
        if create_dirs:
            for key in ("MOLT_BACKEND_DAEMON_SOCKET_DIR", "SCCACHE_DIR"):
                value = env.get(key)
                if value:
                    Path(value).expanduser().mkdir(parents=True, exist_ok=True)
        return env

    def require_project_python(
        self,
        context: str,
        env: Mapping[str, str] | None = None,
    ) -> Path:
        python = self.project_python(env)
        if not python.exists():
            raise DxConfigError(
                f"{python} is missing; run `tools/dev.py install` before {context}"
            )
        return python

    def format_command(
        self,
        command: str,
        env: Mapping[str, str] | None = None,
    ) -> str:
        return command.format(
            root=str(self.root),
            project_python=str(self.project_python(env)),
        )

    def split_command(
        self,
        command: object,
        name: str,
        env: Mapping[str, str] | None = None,
    ) -> list[str]:
        if not isinstance(command, str) or not command.strip():
            raise DxConfigError(f"Missing [tool.molt.dx.commands].{name}")
        return shlex.split(self.format_command(command, env), posix=os.name != "nt")

    def split_command_sequence(
        self,
        command: object,
        name: str,
        *,
        env: Mapping[str, str] | None = None,
        commands: dict[str, object] | None = None,
        stack: tuple[str, ...] = (),
    ) -> list[list[str]]:
        commands = self.commands() if commands is None else commands

        def split_item(item: str, item_name: str) -> list[list[str]]:
            stripped = item.strip()
            if stripped.startswith("@"):
                ref = stripped[1:]
                if not ref or any(ch.isspace() for ch in ref):
                    raise DxConfigError(
                        f"Invalid [tool.molt.dx.commands].{item_name} reference: {item!r}"
                    )
                if ref in stack:
                    chain = " -> ".join((*stack, ref))
                    raise DxConfigError(
                        f"Cyclic [tool.molt.dx.commands] reference: {chain}"
                    )
                if ref not in commands:
                    raise DxConfigError(
                        f"Missing [tool.molt.dx.commands].{ref} referenced by {item_name}"
                    )
                return self.split_command_sequence(
                    commands[ref],
                    ref,
                    env=env,
                    commands=commands,
                    stack=(*stack, ref),
                )
            return [self.split_command(item, item_name, env)]

        if isinstance(command, str):
            return split_item(command, name)
        if isinstance(command, list) and command:
            split: list[list[str]] = []
            for idx, item in enumerate(command):
                if not isinstance(item, str) or not item.strip():
                    raise DxConfigError(
                        f"Invalid [tool.molt.dx.commands].{name}[{idx}]: "
                        "expected command string"
                    )
                split.extend(split_item(item, f"{name}[{idx}]"))
            return split
        raise DxConfigError(f"Missing [tool.molt.dx.commands].{name}")
