from __future__ import annotations

import subprocess
import json
import os
from pathlib import Path
import sys
import textwrap
from typing import Any

import pytest

from molt.cargo_execution_policy import PROOF_COMMAND_TIMEOUT_ENV
from tests import process_guard_common


@pytest.mark.skipif(os.name != "posix", reason="fresh observer requires POSIX")
@pytest.mark.parametrize("disable_autoload", [False, True])
def test_fresh_guard_observer_keeps_pytest_cache_outside_a_fresh_project(
    tmp_path: Path, disable_autoload: bool
) -> None:
    root = Path(__file__).resolve().parents[1]
    project = tmp_path / "project"
    project.mkdir()
    # The project starts clean but imports the actual helper and plugin.
    for name in ("src", "tools", "tests"):
        (project / name).symlink_to(root / name, target_is_directory=True)
    (project / "pyproject.toml").write_text(
        '[tool.pytest.ini_options]\ncache_dir = "tmp/pytest-cache"\n'
        'addopts = ["-p", "molt.pytest_memory_guard_config_plugin"]\n',
        encoding="utf-8",
    )
    case = project / "test_observer_cache.py"
    case.write_text(
        textwrap.dedent("""
        import json
        import os
        from pathlib import Path
        from tests.process_guard_common import run_in_fresh_guard_observer

        def test_selected(request):
            assert run_in_fresh_guard_observer(request) is False
            cache_path = request.config.cache.mkdir("observer")
            request.config.cache.set("observer/selected", True)
            assert not hasattr(request.config, "workerinput")
            with Path(os.environ["GUARD_OBSERVER_REPORT"]).open("a", encoding="utf-8") as report:
                report.write(json.dumps({
                    "pid": os.getpid(), "nodeid": request.node.nodeid,
                    "cache_path": str(cache_path),
                }) + "\\n")

        def test_unselected():
            raise AssertionError("the observer must run only the selected node")
        """),
        encoding="utf-8",
    )
    before = {path.relative_to(project) for path in project.rglob("*")}
    report = tmp_path / "observed.jsonl"
    nodeid = "test_observer_cache.py::test_selected"
    source = (
        "from types import SimpleNamespace; "
        "from tests.process_guard_common import run_in_fresh_guard_observer; "
        f"config = SimpleNamespace(rootpath={str(project)!r}, "
        "pluginmanager=SimpleNamespace(get_plugins=lambda: [])); "
        f"node = SimpleNamespace(nodeid={nodeid!r}); "
        "assert run_in_fresh_guard_observer(SimpleNamespace(config=config, node=node))"
    )
    env = dict(os.environ)
    env["PYTHONPATH"] = os.pathsep.join((str(root / "src"), str(root)))
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    env["GUARD_OBSERVER_REPORT"] = str(report)
    if disable_autoload:
        env["PYTEST_DISABLE_PLUGIN_AUTOLOAD"] = "1"
    else:
        env.pop("PYTEST_DISABLE_PLUGIN_AUTOLOAD", None)
    result = process_guard_common.run_guarded_test_process(
        [sys.executable, "-B", "-c", source],
        cwd=project,
        env=env,
        timeout=90.0,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    calls = [
        json.loads(line) for line in report.read_text(encoding="utf-8").splitlines()
    ]
    assert len(calls) == 1
    assert calls[0]["nodeid"] == nodeid
    assert calls[0]["pid"] != os.getpid()
    cache_path = Path(calls[0]["cache_path"])
    assert cache_path.is_dir()
    assert not cache_path.resolve().is_relative_to(project.resolve())
    assert {path.relative_to(project) for path in project.rglob("*")} == before


@pytest.mark.skipif(sys.platform != "linux", reason="Linux exec lifetime RSS")
def test_fresh_guard_observer_excludes_the_inherited_parent_image(tmp_path) -> None:
    root = Path(__file__).resolve().parents[1]
    case = tmp_path / "test_fresh_observer.py"
    case.write_text(
        "import os, sys\n"
        "from tools import memory_guard\n"
        "from tests.process_guard_common import run_in_fresh_guard_observer\n"
        "def test_subject(request, monkeypatch, tmp_path):\n"
        "    if run_in_fresh_guard_observer(request): return\n"
        "    monkeypatch.setenv('GUARD_OBSERVER_FIXTURE', str(tmp_path))\n"
        "    result = memory_guard.run_guarded(\n"
        "        [sys.executable, '-c', 'import os; print(os.environ[\"GUARD_OBSERVER_FIXTURE\"])'],\n"
        "        max_rss_kb=256*1024, max_total_rss_kb=512*1024,\n"
        "        poll_interval=0.01, timeout=10)\n"
        "    assert result.returncode == 0, result.stderr\n"
        "    assert result.stdout.strip() == str(tmp_path)\n"
        "    assert result.violation is None\n",
        encoding="utf-8",
    )
    source = (
        "import sys; from types import SimpleNamespace; "
        "from tools import memory_guard; "
        "from tests.process_guard_common import run_in_fresh_guard_observer; "
        "allocation = bytearray(384*1024*1024); "
        "direct = memory_guard.run_guarded([sys.executable, '-c', 'pass'], "
        "max_rss_kb=256*1024, max_total_rss_kb=512*1024, "
        "poll_interval=0.01, timeout=10); "
        "assert direct.child_returncode == 0, direct; "
        "assert direct.returncode == 137, direct; "
        "assert direct.violation.scope == 'process_rusage', direct; "
        f"config = SimpleNamespace(rootpath={str(root)!r}, "
        "pluginmanager=SimpleNamespace(get_plugins=lambda: [])); "
        f"node = SimpleNamespace(nodeid={str(case) + '::test_subject'!r}); "
        "assert run_in_fresh_guard_observer(SimpleNamespace(config=config, node=node)); "
        "assert len(allocation) == 384*1024*1024"
    )
    result = process_guard_common.run_guarded_test_process(
        [sys.executable, "-B", "-c", source], cwd=root, timeout=90.0
    )
    assert result.returncode == 0, result.stdout + result.stderr


def test_run_guarded_test_process_preserves_prefix_and_timeout(monkeypatch) -> None:
    captured: dict[str, Any] = {}

    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        captured["cmd"] = cmd
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, "ok\n", "")

    monkeypatch.setattr(
        process_guard_common.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    result = process_guard_common.run_guarded_test_process(
        [sys.executable, "-c", "print('ok')"],
        prefix="MOLT_PYTEST",
        env={
            **process_guard_common.guard_custody_env(),
            "MOLT_PYTEST_TIMEOUT_SEC": "12",
        },
    )

    assert result.returncode == 0
    assert captured["cmd"] == [sys.executable, "-c", "print('ok')"]
    assert captured["kwargs"]["prefix"] == "MOLT_PYTEST"
    assert captured["kwargs"]["operation_role"] == "execution"
    assert captured["kwargs"]["timeout"] == 12


@pytest.mark.parametrize(
    ("command", "expected"),
    [
        ([sys.executable, "-m", "molt.cli", "build", "probe.py"], "build"),
        ([sys.executable, "-m", "molt", "build", "probe.py"], "build"),
        (["uv", "run", "python", "-m", "molt.cli", "build", "probe.py"], "build"),
        (["molt", "build", "probe.py"], "build"),
        (["cargo", "build", "--locked"], "build"),
        (["cmake", "--build", "out"], "build"),
        (["clang", "probe.c", "-o", "probe"], "build"),
        (["rustc", "probe.rs"], "build"),
        (["clang", "--version"], "execution"),
        (["rustc", "-vV"], "execution"),
        (["rustc", "--print", "target-list"], "execution"),
        ([sys.executable, "build"], "execution"),
        (["node", "build"], "execution"),
    ],
)
def test_guarded_process_role_uses_realized_command_grammar(
    command: list[str], expected: str
) -> None:
    assert process_guard_common.guarded_process_role(command).value == expected


def test_build_role_preserves_family_custody_and_uses_shared_default(
    monkeypatch,
) -> None:
    captured: dict[str, Any] = {}

    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, "ok\n", "")

    monkeypatch.setattr(
        process_guard_common.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    command = [sys.executable, "-m", "molt.cli", "build", "probe.py"]
    launch = process_guard_common.run_guarded_test_process
    launch(command, prefix="MOLT_WASM_TEST", env={})

    assert captured["kwargs"]["prefix"] == "MOLT_WASM_TEST"
    assert captured["kwargs"]["operation_role"] == "build"
    assert captured["kwargs"][
        "timeout"
    ] == process_guard_common.default_nested_process_timeout_seconds("build")


@pytest.mark.parametrize(
    ("env", "expected"),
    [
        ({"MOLT_WASM_TEST_TIMEOUT_SEC": "222", "MOLT_BUILD_TIMEOUT_SEC": "333"}, 222),
        ({"MOLT_BUILD_TIMEOUT_SEC": "333"}, 333),
        ({"MOLT_TEST_PROCESS_TIMEOUT_SEC": "444"}, 444),
    ],
)
def test_build_timeout_policy_is_compositional(
    monkeypatch, env: dict[str, str], expected: float
) -> None:
    captured: dict[str, Any] = {}

    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(
        process_guard_common.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )
    process_guard_common.run_guarded_test_process(
        ["cargo", "build"], prefix="MOLT_WASM_TEST", env=env
    )
    assert captured["kwargs"]["timeout"] == expected


@pytest.mark.parametrize(
    ("prefix", "command"),
    [
        ("MOLT_NATIVE_TEST", [sys.executable, "tools/bench.py"]),
        ("MOLT_WASM_TEST", ["cargo", "build", "--locked"]),
        ("MOLT_CLI_TEST", [sys.executable, "tool.py"]),
        ("MOLT_RUST_TEST", ["rustc", "probe.rs"]),
        ("MOLT_COMPLIANCE", [sys.executable, "probe.py"]),
        ("MOLT_MUTATION", [sys.executable, "probe.py"]),
        ("MOLT_SURFACE_TEST", [sys.executable, "probe.py"]),
    ],
)
def test_nested_guard_defaults_cannot_undercut_owning_proof_budget(
    monkeypatch,
    prefix: str,
    command: list[str],
) -> None:
    captured: dict[str, Any] = {}

    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(
        process_guard_common.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )
    process_guard_common.run_guarded_test_process(
        command,
        prefix=prefix,
        env={
            **process_guard_common.guard_custody_env(),
            PROOF_COMMAND_TIMEOUT_ENV: "1200",
            f"{prefix}_TIMEOUT_SEC": "300",
        },
    )

    assert captured["kwargs"]["timeout"] == 1200


def test_explicit_nested_operation_timeout_remains_narrower_than_owner(
    monkeypatch,
) -> None:
    captured: dict[str, Any] = {}

    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(
        process_guard_common.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )
    process_guard_common.run_guarded_test_process(
        ["node", "probe.js"],
        prefix="MOLT_WASM_TEST",
        env={
            **process_guard_common.guard_custody_env(),
            PROOF_COMMAND_TIMEOUT_ENV: "1200",
        },
        timeout=10,
    )

    assert captured["kwargs"]["timeout"] == 10


@pytest.mark.parametrize("raw", ["0", "nan", "invalid"])
def test_invalid_owning_proof_timeout_fails_closed(raw: str) -> None:
    with pytest.raises(ValueError, match=PROOF_COMMAND_TIMEOUT_ENV):
        process_guard_common.resolve_test_process_timeout(
            "MOLT_NATIVE_TEST",
            process_guard_common.GuardedProcessRole.EXECUTION,
            {PROOF_COMMAND_TIMEOUT_ENV: raw},
            explicit=None,
            default=None,
        )


def test_run_guarded_test_process_preserves_check_semantics(monkeypatch) -> None:
    guarded_result = subprocess.CompletedProcess(["false"], 17, "out", "err")
    guarded_result.child_returncode = 0
    guarded_result.infrastructure_failure = object()

    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        del cmd, kwargs
        return guarded_result

    monkeypatch.setattr(
        process_guard_common.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    try:
        process_guard_common.run_guarded_test_process(
            ["false"],
            prefix="MOLT_PYTEST",
            check=True,
        )
    except subprocess.CalledProcessError as exc:
        assert exc.returncode == 17
        assert exc.output == "out"
        assert exc.stderr == "err"
        assert getattr(exc, "guarded_result") is guarded_result
    else:  # pragma: no cover - assertion clarity
        raise AssertionError("expected CalledProcessError")


def test_run_guarded_test_process_preserves_timeout_semantics(monkeypatch) -> None:
    guarded_result = subprocess.CompletedProcess(
        ["sleep", "10"],
        process_guard_common.harness_memory_guard.memory_guard.TIMEOUT_RETURN_CODE,
        "",
        "memory_guard: timeout after 5s\n",
    )
    guarded_result.timed_out = True

    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        del cmd, kwargs
        return guarded_result

    monkeypatch.setattr(
        process_guard_common.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    try:
        process_guard_common.run_guarded_test_process(
            ["sleep", "10"],
            prefix="MOLT_PYTEST",
            timeout=5,
        )
    except subprocess.TimeoutExpired as exc:
        assert exc.timeout == 5
        assert exc.stderr == "memory_guard: timeout after 5s\n"
        receipt = json.loads(exc.__notes__[0])
        assert receipt["schema"] == "molt.test-process-timeout.v1"
        assert receipt["stderr_tail"] == "memory_guard: timeout after 5s\n"
        assert getattr(exc, "guarded_result") is guarded_result
    else:  # pragma: no cover - assertion clarity
        raise AssertionError("expected TimeoutExpired")


def test_run_guarded_test_process_does_not_infer_timeout_from_child_124(
    monkeypatch,
) -> None:
    guarded_result = subprocess.CompletedProcess(
        ["child"],
        process_guard_common.harness_memory_guard.memory_guard.TIMEOUT_RETURN_CODE,
        "",
        "memory_guard: timeout after is child output only\n",
    )
    guarded_result.timed_out = False
    monkeypatch.setattr(
        process_guard_common.harness_memory_guard,
        "guarded_completed_process",
        lambda *_args, **_kwargs: guarded_result,
    )

    result = process_guard_common.run_guarded_test_process(
        ["child"],
        prefix="MOLT_PYTEST",
        timeout=5,
        check=False,
    )

    assert result is guarded_result
    assert result.returncode == 124


def test_module_os_view_keeps_patches_out_of_the_process_wide_os(monkeypatch):
    import os
    from pathlib import Path

    from molt import file_locks
    from tests.process_guard_common import install_module_os_view
    from tools.memory_guard_core import process_custody

    real_pid = os.getpid()
    real_name = os.name
    other_name = "posix" if real_name == "nt" else "nt"
    view = install_module_os_view(monkeypatch, process_custody, name=other_name)
    monkeypatch.setattr(process_custody.os, "getpid", lambda: 999)
    monkeypatch.delattr(process_custody.os, "killpg", raising=False)

    assert process_custody.os is view
    assert process_custody.os.getpid() == 999
    assert process_custody.os.name == other_name
    assert not hasattr(process_custody.os, "killpg")
    assert process_custody.os.environ is os.environ
    # Host consumers keep the real module: pathlib still builds host paths
    # and file-lock ownership still reads the real process id.
    assert os.name == real_name and os.getpid() == real_pid
    assert file_locks.os.getpid() == real_pid
    assert Path(".").resolve().is_absolute()
    monkeypatch.undo()
    assert process_custody.os is os


def test_current_thread_only_leaves_other_threads_on_the_original():
    import threading

    from tests.process_guard_common import current_thread_only

    calls = []
    dispatch = current_thread_only(
        lambda value: calls.append(("double", value)) or "double",
        lambda value: calls.append(("original", value)) or "original",
    )
    seen = []
    worker = threading.Thread(target=lambda: seen.append(dispatch("worker")))
    worker.start()
    worker.join(5)

    assert dispatch("owner") == "double"
    assert seen == ["original"]
    assert sorted(calls) == [("double", "owner"), ("original", "worker")]


def test_thread_module_view_scopes_function_fakes_to_the_installing_thread(
    monkeypatch,
):
    import os
    import sys
    import threading

    from tests.process_guard_common import install_thread_module_view
    from tools.memory_guard_core import process_custody
    from molt import process_model

    real_pid = os.getpid()
    view = install_thread_module_view(
        monkeypatch, "os", os, process_custody, process_model, getpid=lambda: 999
    )
    platform_view = install_thread_module_view(
        monkeypatch, "sys", sys, process_model, platform="fake-os"
    )
    seen = []
    worker = threading.Thread(
        target=lambda: seen.append(
            (process_custody.os.getpid(), process_model.sys.platform)
        )
    )
    worker.start()
    worker.join(5)

    assert process_custody.os is view and process_model.os is view
    assert process_custody.os.getpid() == 999
    # Another thread reaches the real function; a value fake cannot be scoped.
    assert seen == [(real_pid, "fake-os")]
    assert process_model.sys is platform_view
    assert os.getpid() == real_pid and sys.platform != "fake-os"


def test_isolated_python_probe_excludes_concurrent_parent_allocations(tmp_path) -> None:
    from concurrent.futures import ThreadPoolExecutor
    import os
    import threading

    ready = tmp_path / "trace-started"
    polluted = tmp_path / "parent-allocation-held"
    stop = threading.Event()

    def pollute_parent() -> int:
        while not stop.wait(0.01):
            if ready.is_file():
                allocation = bytearray(8 * 1024 * 1024)
                polluted.write_text("held", encoding="utf-8")
                stop.wait()
                return len(allocation)
        return 0

    with ThreadPoolExecutor(max_workers=1) as pool:
        pollution = pool.submit(pollute_parent)
        try:
            observed = process_guard_common.run_isolated_python_probe(
                """
                import gc
                import json
                import os
                from pathlib import Path
                import sys
                import threading
                import time
                import tracemalloc
                from molt import exact_json

                payload = json.load(sys.stdin)
                ready, polluted = map(Path, sys.argv[1:])
                if tracemalloc.is_tracing():
                    raise RuntimeError("probe requires exclusive allocation tracing")
                # Parse both paths before tracing. pathlib interns their parts,
                # and the interned-string table resizes at a count that depends
                # on every module this interpreter imported.
                str(ready), str(polluted)
                gc.collect()
                tracemalloc.start()
                try:
                    ready.write_text("tracing", encoding="utf-8")
                    while not polluted.is_file():
                        time.sleep(0.01)
                    decoded = exact_json.loads_exact('{"value":7}')
                    _, peak = tracemalloc.get_traced_memory()
                finally:
                    tracemalloc.stop()
                print(json.dumps({
                    "pid": os.getpid(), "peak": peak, "decoded": decoded,
                    "payload": payload, "threads": threading.active_count(),
                    "pytest_imported": "pytest" in sys.modules,
                }))
                """,
                args=[ready, polluted],
                payload={"transport": "owned"},
            )
        finally:
            stop.set()
        assert pollution.result() == 8 * 1024 * 1024
    assert observed["pid"] != os.getpid()
    assert observed["threads"] == 1
    assert observed["pytest_imported"] is False
    assert observed["decoded"] == {"value": 7}
    assert observed["payload"] == {"transport": "owned"}
    assert observed["peak"] < 256 * 1024


@pytest.mark.parametrize("name", ["python", "python3", "python3.12", "Python.exe"])
def test_a_bare_interpreter_name_never_reaches_a_process(name: str) -> None:
    # HF-587: a name resolves through PATH (Apple's /usr/bin/python3 is 3.9).
    with pytest.raises(ValueError, match="HF-587"):
        process_guard_common.run_guarded_test_process([name, "-c", "pass"])


def test_an_interpreter_named_by_path_runs() -> None:
    result = process_guard_common.run_guarded_test_process(
        [sys.executable, "-c", "print('ok')"]
    )
    assert result.returncode == 0 and result.stdout.strip() == "ok"
