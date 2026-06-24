from __future__ import annotations

import errno
import json
import os
import shlex
import shutil
import subprocess
from collections.abc import Mapping, Sequence
from pathlib import Path
import sys
import time
import uuid


ROOT = Path(__file__).resolve().parents[1]
SRC_ROOT = ROOT / "src"
if str(SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(SRC_ROOT))

from molt._host_exit import process_returncode_for_direct_os_exit  # noqa: E402

PYTEST_OUTER_GUARD_SUMMARY_DIR = ROOT / "tmp" / "pytest-memory-guard"
PYTEST_TEMP_ROOT = ROOT / "tmp" / "pytest-temproot"
PYTEST_CACHE_DIR = ROOT / "tmp" / "pytest-cache"
WINDOWS_PYTEST_TEMP_ROOT_NAME = "pytest-temproot"
WINDOWS_PYTEST_CACHE_DIR_NAME = "pytest-cache"
PYTEST_OUTER_GUARD_REEXEC_ENV = "MOLT_PYTEST_OUTER_GUARD_REEXEC"
TEST_SCRIPT_OUTER_GUARD_REEXEC_ENV = "MOLT_TEST_SCRIPT_OUTER_GUARD_REEXEC"
PYTEST_CURRENT_TEST_FILE_ENV = "MOLT_PYTEST_CURRENT_TEST_FILE"
ACTIVE_GUARD_TOKEN_ENV = "MOLT_MEMORY_GUARD_TOKEN"
ACTIVE_GUARD_MARKER_ENV = "MOLT_MEMORY_GUARD_MARKER"
ACTIVE_GUARD_MARKER_DIR = ROOT / "tmp" / "memory_guard" / "active"
PYTEST_COMMAND_NAMES = frozenset({"pytest", "py.test", "pytest.exe", "py.test.exe"})
PYTEST_GUARD_PLUGIN_NAMES = frozenset(
    {
        "molt_memory_guard",
        "molt.pytest_memory_guard_bootstrap",
        "molt.pytest_memory_guard_config_plugin",
    }
)
SAFE_CONF_CUT_DIRS = frozenset({ROOT, ROOT / "tests"})
SAFE_CONFIG_FILES = frozenset({ROOT / "pyproject.toml"})
PYTHON_OPTIONS_WITH_ARGUMENT = frozenset({"-W", "-X"})
MAX_CURRENT_TEST_TEXT = 4096
_ATOMIC_REPLACE = os.replace
_ATOMIC_REPLACE_RETRYABLE_WINERRORS = frozenset({5, 32})


def _truthy_env(value: str | None) -> bool:
    return value is not None and value.strip().lower() not in {
        "",
        "0",
        "false",
        "no",
        "off",
    }


def _is_windows_process_model() -> bool:
    return os.name == "nt"


def _is_retryable_atomic_replace_error(exc: OSError) -> bool:
    if not _is_windows_process_model():
        return False
    winerror = getattr(exc, "winerror", None)
    if winerror in _ATOMIC_REPLACE_RETRYABLE_WINERRORS:
        return True
    return isinstance(exc, PermissionError) and exc.errno in {
        errno.EACCES,
        errno.EPERM,
    }


def _atomic_replace_with_retry(src: Path, dst: Path) -> None:
    attempts = 40 if _is_windows_process_model() else 1
    for attempt in range(attempts):
        try:
            _ATOMIC_REPLACE(src, dst)
            return
        except OSError as exc:
            if (
                attempt + 1 >= attempts
                or not _is_retryable_atomic_replace_error(exc)
            ):
                raise
            time.sleep(min(0.25, 0.01 * (attempt + 1)))


def _flush_standard_streams() -> None:
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.flush()
        except (OSError, ValueError):
            pass


def _process_exit_code(returncode: int | None) -> int:
    return process_returncode_for_direct_os_exit(
        returncode,
        windows=_is_windows_process_model(),
    )


def _windows_process_group_kwargs() -> dict[str, object]:
    creationflags = getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0)
    return {"creationflags": creationflags} if creationflags else {}


def handoff_to_outer_guard(argv: Sequence[str], env: Mapping[str, str]) -> None:
    if _is_windows_process_model():
        try:
            completed = subprocess.run(
                list(argv),
                env=dict(env),
                check=False,
                **_windows_process_group_kwargs(),
            )
        except KeyboardInterrupt:
            _flush_standard_streams()
            os._exit(130)
        except OSError as exc:
            print(f"pytest memory guard bootstrap: spawn failed: {exc}", file=sys.stderr)
            _flush_standard_streams()
            os._exit(127)
        _flush_standard_streams()
        os._exit(_process_exit_code(completed.returncode))
    os.execvpe(argv[0], argv, env)


def _orig_argv_pytest_module_args(orig: Sequence[str]) -> tuple[str, ...] | None:
    idx = 1
    while idx < len(orig):
        arg = orig[idx]
        if arg == "-m":
            if idx + 1 < len(orig) and orig[idx + 1] == "pytest":
                return tuple(orig[idx + 2 :])
            return None
        if arg == "-c":
            return None
        if not arg.startswith("-"):
            return None
        if arg in PYTHON_OPTIONS_WITH_ARGUMENT:
            idx += 2
            continue
        idx += 1
    return None


def pytest_invocation_args(
    *,
    orig_argv: Sequence[str] | None = None,
    runtime_argv: Sequence[str] | None = None,
) -> tuple[str, ...] | None:
    orig = list(orig_argv if orig_argv is not None else getattr(sys, "orig_argv", []))
    runtime = list(runtime_argv if runtime_argv is not None else sys.argv)
    module_args = _orig_argv_pytest_module_args(orig)
    if module_args is not None:
        return module_args
    if len(orig) >= 2 and Path(orig[1]).name in PYTEST_COMMAND_NAMES:
        return tuple(orig[2:])
    if runtime and Path(runtime[0]).name in PYTEST_COMMAND_NAMES:
        return tuple(runtime[1:])
    return None


def _resolve_cli_path(raw: str) -> Path:
    path = Path(raw).expanduser()
    if not path.is_absolute():
        path = Path.cwd() / path
    return path.resolve()


def _pytest_custody_artifact_path(
    kind: str, suffix: str, *, pid: int | None = None
) -> Path:
    safe_kind = "".join(ch if ch.isalnum() else "-" for ch in kind.lower()).strip("-")
    safe_suffix = "".join(ch if ch.isalnum() else "-" for ch in suffix.lower()).strip(
        "-"
    )
    return PYTEST_OUTER_GUARD_SUMMARY_DIR / (
        f"{safe_kind or 'pytest'}-{os.getpid() if pid is None else pid}_{safe_suffix}.json"
    )


def pytest_current_test_file_path(*, pid: int | None = None) -> Path:
    return _pytest_custody_artifact_path("pytest", "current-test", pid=pid)


def _path_is_under(path: Path, root: Path) -> bool:
    try:
        path.resolve(strict=False).relative_to(root.resolve(strict=False))
    except ValueError:
        return False
    return True


def _canonical_pytest_current_test_file_path(raw_path: str | None = None) -> Path:
    path = Path(raw_path).expanduser() if raw_path else pytest_current_test_file_path()
    if not path.is_absolute():
        path = ROOT / path
    path = path.resolve(strict=False)
    if not _path_is_under(path, PYTEST_OUTER_GUARD_SUMMARY_DIR):
        return pytest_current_test_file_path()
    return path


def install_pytest_current_test_file_env() -> Path:
    path = _canonical_pytest_current_test_file_path(
        os.environ.get(PYTEST_CURRENT_TEST_FILE_ENV)
    )
    os.environ[PYTEST_CURRENT_TEST_FILE_ENV] = str(path)
    return path


def _safe_current_test_record_part(value: str, default: str) -> str:
    cleaned = "".join(ch if ch.isalnum() or ch in {"-", "_"} else "-" for ch in value)
    cleaned = cleaned.strip("-_")
    return cleaned or default


def _pytest_current_test_worker_record_path(root_path: Path) -> Path:
    worker = os.environ.get("PYTEST_XDIST_WORKER", "").strip()
    if not worker:
        return root_path
    safe_worker = _safe_current_test_record_part(worker, "worker")
    return root_path.with_name(f"{root_path.name}.d") / (
        f"{safe_worker}-{os.getpid()}_current-test.json"
    )


def _bounded_text(value: object) -> str:
    text = "" if value is None else str(value)
    if len(text) <= MAX_CURRENT_TEST_TEXT:
        return text
    return f"{text[:MAX_CURRENT_TEST_TEXT]}...<truncated>"


def _guard_plugin_disable_reason(raw: str) -> str | None:
    value = raw.strip().lower()
    if not value.startswith("no:"):
        return None
    plugin_name = value[3:]
    if plugin_name in PYTEST_GUARD_PLUGIN_NAMES:
        return plugin_name
    return None


def _pytest_plugin_enable_name(raw: str) -> str | None:
    value = raw.strip()
    if not value or value.lower().startswith("no:"):
        return None
    return value


def _pytest_args_enable_guard_config_plugin(args: Sequence[str]) -> bool:
    idx = 0
    while idx < len(args):
        arg = args[idx]
        plugin_name: str | None = None
        if arg == "-p":
            if idx + 1 >= len(args):
                return False
            plugin_name = _pytest_plugin_enable_name(args[idx + 1])
            idx += 2
        elif arg.startswith("-p") and arg != "-p":
            plugin_name = _pytest_plugin_enable_name(arg[2:].lstrip("="))
            idx += 1
        else:
            idx += 1
        if plugin_name == "molt.pytest_memory_guard_config_plugin":
            return True
    return False


def _pytest_args_disable_cacheprovider(args: Sequence[str]) -> bool:
    idx = 0
    while idx < len(args):
        arg = args[idx]
        if arg == "-p":
            if idx + 1 < len(args) and args[idx + 1] == "no:cacheprovider":
                return True
            idx += 2
            continue
        if arg in {"-pno:cacheprovider", "-p=no:cacheprovider"}:
            return True
        idx += 1
    return False


def _pytest_args_have_cache_dir(args: Sequence[str]) -> bool:
    idx = 0
    while idx < len(args):
        arg = args[idx]
        if arg == "-o":
            if idx + 1 < len(args) and args[idx + 1].startswith("cache_dir="):
                return True
            idx += 2
            continue
        if arg.startswith("-o"):
            option = arg[2:].lstrip("=")
            if option.startswith("cache_dir="):
                return True
        idx += 1
    return False


def install_windows_pytest_cache_dir_arg(args: list[str]) -> bool:
    if not _is_windows_process_model():
        return False
    if _pytest_args_disable_cacheprovider(args) or _pytest_args_have_cache_dir(args):
        return False
    args.extend(["-o", f"cache_dir={windows_pytest_cache_dir()}"])
    return True


def install_windows_pytest_cache_dir_config(
    early_config: object,
    args: Sequence[str],
) -> bool:
    if not _is_windows_process_model():
        return False
    if _pytest_args_disable_cacheprovider(args) or _pytest_args_have_cache_dir(args):
        return False
    inicfg = getattr(early_config, "_inicfg", None)
    if not isinstance(inicfg, dict):
        return False
    from _pytest.config.findpaths import ConfigValue

    inicfg["cache_dir"] = ConfigValue(
        str(windows_pytest_cache_dir()),
        origin="override",
        mode="ini",
    )
    inicache = getattr(early_config, "_inicache", None)
    if isinstance(inicache, dict):
        inicache.pop("cache_dir", None)
    return True


def _ensure_windows_readable_dir(path: Path) -> None:
    mode = 0o755 if _is_windows_process_model() else 0o777
    path.mkdir(mode=mode, parents=True, exist_ok=True)


def _artifact_root_accepts_child_dirs(path: Path, *, create_dirs: bool) -> bool:
    probe = path / f".molt-write-probe-{os.getpid()}-{uuid.uuid4().hex}"
    try:
        if create_dirs:
            path.mkdir(mode=0o755, parents=True, exist_ok=True)
        probe.mkdir(mode=0o755)
        list(probe.iterdir())
    except OSError:
        return False
    finally:
        try:
            shutil.rmtree(probe)
        except OSError:
            pass
    return True


def _default_windows_pytest_artifact_roots() -> tuple[Path, ...]:
    roots: list[Path] = []
    for key in ("LOCALAPPDATA", "TEMP", "TMP"):
        raw = os.environ.get(key)
        if raw:
            roots.append(Path(raw).expanduser() / "Molt" / "tmp")
    roots.append(ROOT / "tmp")
    seen: set[str] = set()
    deduped: list[Path] = []
    for root in roots:
        key = os.path.normcase(str(root))
        if key in seen:
            continue
        seen.add(key)
        deduped.append(root)
    return tuple(deduped)


def _windows_pytest_artifact_base() -> Path:
    explicit = os.environ.get("MOLT_EXT_ROOT")
    if explicit:
        return Path(explicit).expanduser() / "tmp"
    for root in _default_windows_pytest_artifact_roots():
        if _artifact_root_accepts_child_dirs(root, create_dirs=True):
            return root
    return ROOT / "tmp"


def windows_pytest_temp_root() -> Path:
    if not _is_windows_process_model():
        return PYTEST_TEMP_ROOT
    token = f"{os.getpid()}-{uuid.uuid4().hex}"
    return _windows_pytest_artifact_base() / f"{WINDOWS_PYTEST_TEMP_ROOT_NAME}-{token}"


def windows_pytest_cache_dir() -> Path:
    if not _is_windows_process_model():
        return PYTEST_CACHE_DIR
    return _windows_pytest_artifact_base() / WINDOWS_PYTEST_CACHE_DIR_NAME


def _pytest_user_temp_root(temproot: Path) -> Path:
    try:
        from _pytest.tmpdir import get_user

        user = get_user() or "unknown"
    except Exception:
        user = "unknown"
    return temproot / f"pytest-of-{user}"


def install_windows_pytest_custody_roots() -> bool:
    if not _is_windows_process_model():
        return False
    raw_temproot = os.environ.get("PYTEST_DEBUG_TEMPROOT")
    temproot = Path(raw_temproot).expanduser() if raw_temproot else windows_pytest_temp_root()
    cache_dir = windows_pytest_cache_dir()
    _ensure_windows_readable_dir(temproot)
    _ensure_windows_readable_dir(_pytest_user_temp_root(temproot))
    _ensure_windows_readable_dir(cache_dir)
    _ensure_windows_readable_dir(cache_dir / "v" / "cache")
    if raw_temproot:
        return False
    os.environ["PYTEST_DEBUG_TEMPROOT"] = str(temproot)
    return True


def _repo_pytest_addopts() -> tuple[str, ...]:
    try:
        import tomllib

        payload = tomllib.loads((ROOT / "pyproject.toml").read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError):
        return ()
    tool = payload.get("tool")
    if not isinstance(tool, dict):
        return ()
    pytest_section = tool.get("pytest")
    if not isinstance(pytest_section, dict):
        return ()
    ini_options = pytest_section.get("ini_options")
    if not isinstance(ini_options, dict):
        return ()
    addopts = ini_options.get("addopts")
    if isinstance(addopts, list) and all(isinstance(item, str) for item in addopts):
        return tuple(addopts)
    if isinstance(addopts, str):
        try:
            return tuple(shlex.split(addopts))
        except ValueError:
            return ()
    return ()


def _pytest_addopts_from_env(environ: Mapping[str, str]) -> tuple[str, ...]:
    raw = environ.get("PYTEST_ADDOPTS", "")
    if not raw.strip():
        return ()
    try:
        return tuple(shlex.split(raw))
    except ValueError as exc:
        raise SystemExit(f"Invalid PYTEST_ADDOPTS: {exc}") from exc


def validate_pytest_guardable_args(args: Sequence[str]) -> None:
    idx = 0
    while idx < len(args):
        arg = args[idx]
        if arg == "-c":
            if idx + 1 >= len(args):
                raise SystemExit("pytest -c requires a config path")
            config_path = _resolve_cli_path(args[idx + 1])
            if config_path not in SAFE_CONFIG_FILES:
                raise SystemExit(
                    "Molt pytest custody requires the repo pytest config; "
                    f"unsafe -c {args[idx + 1]!r} is not allowed."
                )
            idx += 2
            continue
        if arg.startswith("-c") and arg != "-c":
            raw = arg[2:].lstrip("=")
            config_path = _resolve_cli_path(raw)
            if config_path not in SAFE_CONFIG_FILES:
                raise SystemExit(
                    "Molt pytest custody requires the repo pytest config; "
                    f"unsafe -c {raw!r} is not allowed."
                )
        if arg == "--noconftest":
            raise SystemExit(
                "Molt pytest custody requires tests/conftest.py; "
                "--noconftest is not allowed. Run under tools/memory_guard.py "
                "and the repo pytest hooks instead."
            )
        if arg == "--confcutdir":
            if idx + 1 >= len(args):
                raise SystemExit("pytest --confcutdir requires a path")
            confcutdir = _resolve_cli_path(args[idx + 1])
            if confcutdir not in SAFE_CONF_CUT_DIRS:
                raise SystemExit(
                    "Molt pytest custody requires the repo pytest hooks; "
                    f"unsafe --confcutdir={args[idx + 1]!r} is not allowed."
                )
            idx += 2
            continue
        if arg.startswith("--confcutdir="):
            raw = arg.split("=", 1)[1]
            confcutdir = _resolve_cli_path(raw)
            if confcutdir not in SAFE_CONF_CUT_DIRS:
                raise SystemExit(
                    "Molt pytest custody requires the repo pytest hooks; "
                    f"unsafe --confcutdir={raw!r} is not allowed."
                )
        if arg == "-p":
            if idx + 1 >= len(args):
                raise SystemExit("pytest -p requires a plugin name")
            disabled = _guard_plugin_disable_reason(args[idx + 1])
            if disabled is not None:
                raise SystemExit(
                    "Molt pytest custody requires the memory-guard pytest "
                    f"plugins; disabling {disabled!r} is not allowed."
                )
            idx += 2
            continue
        if arg.startswith("-p") and arg != "-p":
            disabled = _guard_plugin_disable_reason(arg[2:].lstrip("="))
            if disabled is not None:
                raise SystemExit(
                    "Molt pytest custody requires the memory-guard pytest "
                    f"plugins; disabling {disabled!r} is not allowed."
                )
        idx += 1


def validate_pytest_guardable_env(
    environ: Mapping[str, str],
    *,
    args: Sequence[str] = (),
) -> None:
    env_addopts = _pytest_addopts_from_env(environ)
    if env_addopts:
        validate_pytest_guardable_args(env_addopts)
    if not _truthy_env(environ.get("PYTEST_DISABLE_PLUGIN_AUTOLOAD")):
        return
    combined_args = tuple(args) + env_addopts + _repo_pytest_addopts()
    validate_pytest_guardable_args(combined_args)
    if not _pytest_args_enable_guard_config_plugin(combined_args):
        raise SystemExit(
            "Molt pytest custody requires the explicit "
            "molt.pytest_memory_guard_config_plugin plugin when "
            "PYTEST_DISABLE_PLUGIN_AUTOLOAD is set."
        )


def _guard_pid_from_env(environ: Mapping[str, str]) -> int | None:
    if environ.get("MOLT_MEMORY_GUARD_ACTIVE") != "1":
        return None
    try:
        guard_pid = int(environ.get("MOLT_MEMORY_GUARD_PID", ""))
    except ValueError:
        return None
    if guard_pid <= 0:
        return None
    return guard_pid


def _command_is_repo_memory_guard(command: str) -> bool:
    guard_path = str(ROOT / "tools" / "memory_guard.py")
    return guard_path in command or "tools/memory_guard.py" in command


def _active_guard_marker_valid(
    environ: Mapping[str, str],
    *,
    guard_pid: int,
) -> bool:
    token = environ.get(ACTIVE_GUARD_TOKEN_ENV, "").strip()
    marker_raw = environ.get(ACTIVE_GUARD_MARKER_ENV, "").strip()
    if guard_pid <= 0 or len(token) < 16 or not marker_raw:
        return False
    marker = Path(marker_raw).expanduser()
    try:
        marker_resolved = marker.resolve(strict=False)
        marker_root = ACTIVE_GUARD_MARKER_DIR.resolve(strict=False)
    except OSError:
        return False
    if marker_resolved.parent != marker_root:
        return False
    try:
        payload = json.loads(marker.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return False
    if not isinstance(payload, dict):
        return False
    if payload.get("pid") != guard_pid or payload.get("token") != token:
        return False
    guard_path = payload.get("path")
    if not isinstance(guard_path, str):
        return False
    try:
        return Path(guard_path).resolve(strict=False) == (
            ROOT / "tools" / "memory_guard.py"
        ).resolve(strict=False)
    except OSError:
        return False


def outer_memory_guard_active(environ: Mapping[str, str] | None = None) -> bool:
    source = os.environ if environ is None else environ
    guard_pid = _guard_pid_from_env(source)
    if guard_pid is None:
        return False

    from tools import memory_guard

    samples = memory_guard.sample_processes()
    if not samples:
        return False
    guard_sample = samples.get(guard_pid)
    if guard_sample is None:
        return False
    if not _command_is_repo_memory_guard(
        guard_sample.command
    ) and not _active_guard_marker_valid(source, guard_pid=guard_pid):
        return False

    current = os.getpid()
    seen: set[int] = set()
    while current > 0 and current not in seen:
        if current == guard_pid:
            return True
        seen.add(current)
        sample = samples.get(current)
        if sample is None or sample.ppid <= 0 or sample.ppid == current:
            break
        current = sample.ppid
    return False


def pytest_outer_guard_argv(args: Sequence[str] | None = None) -> list[str]:
    from tools import harness_memory_guard

    pytest_args = tuple(args or ())
    limits = harness_memory_guard.limits_from_env("MOLT_PYTEST")
    summary_path = _pytest_custody_artifact_path("pytest", "outer-guard")
    return [
        sys.executable,
        str(ROOT / "tools" / "memory_guard.py"),
        "--max-rss-gb",
        str(limits.max_process_rss_gb),
        "--max-total-rss-gb",
        str(limits.max_total_rss_gb),
        "--poll-interval",
        str(limits.poll_interval),
        "--child-rlimit-gb",
        str(0 if limits.child_rlimit_gb is None else limits.child_rlimit_gb),
        "--summary-json",
        str(summary_path),
        "--",
        sys.executable,
        "-m",
        "pytest",
        *pytest_args,
    ]


def repo_test_script_invocation_args(
    *,
    runtime_argv: Sequence[str] | None = None,
) -> tuple[Path, tuple[str, ...]] | None:
    runtime = list(runtime_argv if runtime_argv is not None else sys.argv)
    if not runtime:
        return None
    raw_script = runtime[0]
    if not raw_script or raw_script == "-c" or raw_script == "-m":
        return None
    script = Path(raw_script).expanduser()
    if not script.is_absolute():
        script = Path.cwd() / script
    try:
        resolved = script.resolve()
        tests_root = (ROOT / "tests").resolve()
        resolved.relative_to(tests_root)
    except (OSError, ValueError):
        return None
    if resolved.suffix != ".py" or not resolved.is_file():
        return None
    return resolved, tuple(runtime[1:])


def _orig_argv_repo_test_module_args(
    orig_argv: Sequence[str] | None = None,
) -> tuple[str, tuple[str, ...]] | None:
    orig = list(orig_argv if orig_argv is not None else getattr(sys, "orig_argv", []))
    idx = 1
    while idx < len(orig):
        arg = orig[idx]
        if arg == "-m":
            if idx + 1 >= len(orig):
                return None
            module = orig[idx + 1]
            if module == "tests" or module.startswith("tests."):
                return module, tuple(orig[idx + 2 :])
            return None
        if arg == "-c" or not arg.startswith("-"):
            return None
        if arg in PYTHON_OPTIONS_WITH_ARGUMENT:
            idx += 2
            continue
        idx += 1
    return None


def repo_test_module_outer_guard_argv(
    module_name: str,
    args: Sequence[str] | None = None,
) -> list[str]:
    from tools import harness_memory_guard

    module_args = tuple(args or ())
    limits = harness_memory_guard.limits_from_env("MOLT_TEST_SUITE")
    summary_path = _pytest_custody_artifact_path("test-module", "outer-guard")
    return [
        sys.executable,
        str(ROOT / "tools" / "memory_guard.py"),
        "--max-rss-gb",
        str(limits.max_process_rss_gb),
        "--max-total-rss-gb",
        str(limits.max_total_rss_gb),
        "--poll-interval",
        str(limits.poll_interval),
        "--child-rlimit-gb",
        str(0 if limits.child_rlimit_gb is None else limits.child_rlimit_gb),
        "--summary-json",
        str(summary_path),
        "--",
        sys.executable,
        "-m",
        module_name,
        *module_args,
    ]


def ensure_repo_test_module_memory_guard(
    *,
    orig_argv: Sequence[str] | None = None,
    environ: Mapping[str, str] | None = None,
) -> bool:
    invocation = _orig_argv_repo_test_module_args(orig_argv)
    if invocation is None:
        return False
    source = os.environ if environ is None else environ
    if outer_memory_guard_active(source):
        return True
    if source.get(TEST_SCRIPT_OUTER_GUARD_REEXEC_ENV):
        raise RuntimeError(
            "repo test module was re-execed for memory custody but no live "
            "ancestor tools/memory_guard.py process could be verified"
        )
    PYTEST_OUTER_GUARD_SUMMARY_DIR.mkdir(parents=True, exist_ok=True)
    module_name, module_args = invocation
    argv = repo_test_module_outer_guard_argv(module_name, module_args)
    env = dict(os.environ)
    env[TEST_SCRIPT_OUTER_GUARD_REEXEC_ENV] = "1"
    handoff_to_outer_guard(argv, env)
    raise RuntimeError("failed to re-exec repo test module under tools/memory_guard.py")


def repo_test_script_outer_guard_argv(
    script_path: Path,
    args: Sequence[str] | None = None,
) -> list[str]:
    from tools import harness_memory_guard

    script_args = tuple(args or ())
    limits = harness_memory_guard.limits_from_env("MOLT_TEST_SUITE")
    summary_path = _pytest_custody_artifact_path("test-script", "outer-guard")
    return [
        sys.executable,
        str(ROOT / "tools" / "memory_guard.py"),
        "--max-rss-gb",
        str(limits.max_process_rss_gb),
        "--max-total-rss-gb",
        str(limits.max_total_rss_gb),
        "--poll-interval",
        str(limits.poll_interval),
        "--child-rlimit-gb",
        str(0 if limits.child_rlimit_gb is None else limits.child_rlimit_gb),
        "--summary-json",
        str(summary_path),
        "--",
        sys.executable,
        str(script_path),
        *script_args,
    ]


def ensure_repo_test_script_memory_guard(
    *,
    runtime_argv: Sequence[str] | None = None,
    environ: Mapping[str, str] | None = None,
) -> bool:
    invocation = repo_test_script_invocation_args(runtime_argv=runtime_argv)
    if invocation is None:
        return False
    source = os.environ if environ is None else environ
    if outer_memory_guard_active(source):
        return True
    if source.get(TEST_SCRIPT_OUTER_GUARD_REEXEC_ENV):
        raise RuntimeError(
            "repo test script was re-execed for memory custody but no live "
            "ancestor tools/memory_guard.py process could be verified"
        )
    PYTEST_OUTER_GUARD_SUMMARY_DIR.mkdir(parents=True, exist_ok=True)
    script_path, script_args = invocation
    argv = repo_test_script_outer_guard_argv(script_path, script_args)
    env = dict(os.environ)
    env[TEST_SCRIPT_OUTER_GUARD_REEXEC_ENV] = "1"
    handoff_to_outer_guard(argv, env)
    raise RuntimeError("failed to re-exec repo test script under tools/memory_guard.py")


def ensure_current_file_test_script_memory_guard(
    file: str | os.PathLike[str],
    *,
    argv: Sequence[str] | None = None,
    environ: Mapping[str, str] | None = None,
) -> bool:
    script_args = tuple(sys.argv[1:] if argv is None else argv)
    return ensure_repo_test_script_memory_guard(
        runtime_argv=(str(Path(file).resolve()), *script_args),
        environ=environ,
    )


def ensure_python_test_memory_guard() -> bool:
    return (
        ensure_pytest_memory_guard()
        or ensure_repo_test_module_memory_guard()
        or ensure_repo_test_script_memory_guard()
    )


def install_windows_pytest_tempdir_mode_patch() -> bool:
    if not _is_windows_process_model():
        return False
    import _pytest.pathlib as pytest_pathlib
    import _pytest.tmpdir as pytest_tmpdir

    current = pytest_pathlib.make_numbered_dir
    already_patched = getattr(current, "_molt_windows_tempdir_mode_patch", False)

    if already_patched:
        patched = current
    else:
        original = current

        def make_numbered_dir_windows_readable(root, prefix, mode=0o700):
            safe_mode = 0o755 if mode == 0o700 else mode
            return original(root, prefix, mode=safe_mode)

        make_numbered_dir_windows_readable._molt_windows_tempdir_mode_patch = True
        patched = make_numbered_dir_windows_readable

    changed = (
        pytest_pathlib.make_numbered_dir is not patched
        or pytest_tmpdir.make_numbered_dir is not patched
    )
    pytest_pathlib.make_numbered_dir = patched
    pytest_tmpdir.make_numbered_dir = patched
    return changed


def ensure_pytest_memory_guard(
    *,
    orig_argv: Sequence[str] | None = None,
    runtime_argv: Sequence[str] | None = None,
    pytest_args: Sequence[str] | None = None,
    environ: Mapping[str, str] | None = None,
) -> bool:
    args = (
        tuple(pytest_args)
        if pytest_args is not None
        else pytest_invocation_args(orig_argv=orig_argv, runtime_argv=runtime_argv)
    )
    if args is None:
        return False
    validate_pytest_guardable_args(args)
    source = os.environ if environ is None else environ
    validate_pytest_guardable_env(source, args=args)
    if outer_memory_guard_active(source):
        if environ is None:
            PYTEST_OUTER_GUARD_SUMMARY_DIR.mkdir(parents=True, exist_ok=True)
            install_pytest_current_test_file_env()
        return True
    if source.get(PYTEST_OUTER_GUARD_REEXEC_ENV):
        raise RuntimeError(
            "pytest was re-execed for memory custody but no live ancestor "
            "tools/memory_guard.py process could be verified"
        )
    PYTEST_OUTER_GUARD_SUMMARY_DIR.mkdir(parents=True, exist_ok=True)
    argv = pytest_outer_guard_argv(args)
    env = dict(os.environ)
    env[PYTEST_OUTER_GUARD_REEXEC_ENV] = "1"
    env[PYTEST_CURRENT_TEST_FILE_ENV] = str(
        _canonical_pytest_current_test_file_path(env.get(PYTEST_CURRENT_TEST_FILE_ENV))
    )
    handoff_to_outer_guard(argv, env)
    raise RuntimeError("failed to re-exec pytest under tools/memory_guard.py")


def pytest_load_initial_conftests(
    early_config: object, parser: object, args: Sequence[str]
) -> None:
    del parser
    install_windows_pytest_custody_roots()
    install_windows_pytest_tempdir_mode_patch()
    install_windows_pytest_cache_dir_config(early_config, args)
    if isinstance(args, list):
        install_windows_pytest_cache_dir_arg(args)
    ensure_pytest_memory_guard(pytest_args=tuple(args))


def _write_pytest_current_test(
    *,
    nodeid: str,
    phase: str,
    location: object | None = None,
) -> None:
    root_path = install_pytest_current_test_file_env()
    path = _pytest_current_test_worker_record_path(root_path)
    path.parent.mkdir(parents=True, exist_ok=True)
    payload: dict[str, object] = {
        "schema_version": 1,
        "recorded_at": time.time(),
        "pid": os.getpid(),
        "aggregate_path": str(root_path),
        "record_path": str(path),
        "phase": _bounded_text(phase),
        "nodeid": _bounded_text(nodeid),
        "pytest_current_test": _bounded_text(os.environ.get("PYTEST_CURRENT_TEST", "")),
        "xdist_worker": _bounded_text(os.environ.get("PYTEST_XDIST_WORKER", "")),
    }
    if location is not None:
        payload["location"] = _bounded_text(location)
    tmp_path = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    tmp_path.write_text(json.dumps(payload, sort_keys=True) + "\n", encoding="utf-8")
    _atomic_replace_with_retry(tmp_path, path)


def pytest_runtest_logstart(nodeid: str, location: object) -> None:
    _write_pytest_current_test(nodeid=nodeid, phase="start", location=location)


def pytest_runtest_setup(item: object) -> None:
    _write_pytest_current_test(nodeid=getattr(item, "nodeid", ""), phase="setup")


def pytest_runtest_call(item: object) -> None:
    _write_pytest_current_test(nodeid=getattr(item, "nodeid", ""), phase="call")


def pytest_runtest_teardown(item: object, nextitem: object | None) -> None:
    del nextitem
    _write_pytest_current_test(nodeid=getattr(item, "nodeid", ""), phase="teardown")


def pytest_runtest_logfinish(nodeid: str, location: object) -> None:
    _write_pytest_current_test(nodeid=nodeid, phase="finish", location=location)
