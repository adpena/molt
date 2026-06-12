from __future__ import annotations

import os
from collections.abc import Mapping, Sequence
from pathlib import Path
import sys


ROOT = Path(__file__).resolve().parents[1]
PYTEST_OUTER_GUARD_SUMMARY_DIR = ROOT / "tmp" / "pytest-memory-guard"
PYTEST_OUTER_GUARD_REEXEC_ENV = "MOLT_PYTEST_OUTER_GUARD_REEXEC"
TEST_SCRIPT_OUTER_GUARD_REEXEC_ENV = "MOLT_TEST_SCRIPT_OUTER_GUARD_REEXEC"
PYTEST_COMMAND_NAMES = frozenset({"pytest", "py.test", "pytest.exe", "py.test.exe"})
SAFE_CONF_CUT_DIRS = frozenset({ROOT, ROOT / "tests"})
PYTHON_OPTIONS_WITH_ARGUMENT = frozenset({"-W", "-X"})


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


def validate_pytest_guardable_args(args: Sequence[str]) -> None:
    idx = 0
    while idx < len(args):
        arg = args[idx]
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
        idx += 1


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
    summary_path = (
        PYTEST_OUTER_GUARD_SUMMARY_DIR / f"pytest-{os.getpid()}_outer_guard.json"
    )
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


def repo_test_script_outer_guard_argv(
    script_path: Path,
    args: Sequence[str] | None = None,
) -> list[str]:
    from tools import harness_memory_guard

    script_args = tuple(args or ())
    limits = harness_memory_guard.limits_from_env("MOLT_TEST_SUITE")
    summary_path = (
        PYTEST_OUTER_GUARD_SUMMARY_DIR
        / f"test-script-{os.getpid()}_outer_guard.json"
    )
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


def ensure_python_test_memory_guard() -> bool:
    return ensure_pytest_memory_guard() or ensure_repo_test_script_memory_guard()


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
    if outer_memory_guard_active(source):
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
    os.execvpe(argv[0], argv, env)
    raise RuntimeError("failed to re-exec pytest under tools/memory_guard.py")


def pytest_load_initial_conftests(
    early_config: object, parser: object, args: Sequence[str]
) -> None:
    del early_config, parser
    ensure_pytest_memory_guard(pytest_args=tuple(args))
