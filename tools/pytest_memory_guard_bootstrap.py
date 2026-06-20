from __future__ import annotations

import json
import os
import shlex
from collections.abc import Mapping, Sequence
from pathlib import Path
import sys
import time


ROOT = Path(__file__).resolve().parents[1]
PYTEST_OUTER_GUARD_SUMMARY_DIR = ROOT / "tmp" / "pytest-memory-guard"
PYTEST_OUTER_GUARD_REEXEC_ENV = "MOLT_PYTEST_OUTER_GUARD_REEXEC"
TEST_SCRIPT_OUTER_GUARD_REEXEC_ENV = "MOLT_TEST_SCRIPT_OUTER_GUARD_REEXEC"
PYTEST_CURRENT_TEST_FILE_ENV = "MOLT_PYTEST_CURRENT_TEST_FILE"
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


def _truthy_env(value: str | None) -> bool:
    return value is not None and value.strip().lower() not in {
        "",
        "0",
        "false",
        "no",
        "off",
    }


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
    if not _command_is_repo_memory_guard(guard_sample.command):
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
    os.execvpe(argv[0], argv, env)
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
    os.execvpe(argv[0], argv, env)
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
    os.execvpe(argv[0], argv, env)
    raise RuntimeError("failed to re-exec pytest under tools/memory_guard.py")


def pytest_load_initial_conftests(
    early_config: object, parser: object, args: Sequence[str]
) -> None:
    del early_config, parser
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
    _ATOMIC_REPLACE(tmp_path, path)


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
