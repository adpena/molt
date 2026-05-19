from __future__ import annotations

import importlib.util
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
SCRIPT_PATH = REPO_ROOT / "tools" / "process_sentinel.py"


def _load_process_sentinel():
    spec = importlib.util.spec_from_file_location(
        "molt_tools_process_sentinel", SCRIPT_PATH
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_process_groups_include_full_matched_group() -> None:
    module = _load_process_sentinel()
    root = Path("/repo/molt")
    samples = {
        10: module.memory_guard.ProcessSample(
            pid=10,
            ppid=1,
            pgid=10,
            rss_kb=100,
            command="/bin/zsh -c cd /repo/molt && cargo build -p molt-backend",
        ),
        11: module.memory_guard.ProcessSample(
            pid=11,
            ppid=10,
            pgid=10,
            rss_kb=200,
            command="/rustc --crate-name molt_backend runtime/molt-backend/src/lib.rs",
        ),
        20: module.memory_guard.ProcessSample(
            pid=20,
            ppid=1,
            pgid=20,
            rss_kb=999,
            command="cargo build unrelated",
        ),
    }

    groups = module.process_groups(samples, root=root, self_pid=9999)

    assert len(groups) == 1
    assert groups[0].pgid == 10
    assert groups[0].pids == [10, 11]
    assert groups[0].total_rss_kb == 300


def test_process_groups_exclude_current_process_group() -> None:
    module = _load_process_sentinel()
    root = Path("/repo/molt")
    samples = {
        10: module.memory_guard.ProcessSample(
            pid=10,
            ppid=1,
            pgid=10,
            rss_kb=100,
            command="/repo/molt/tools/process_sentinel.py --once --kill-all",
        )
    }

    groups = module.process_groups(samples, root=root, self_pid=9999, self_pgid=10)

    assert groups == []


def test_process_groups_ignore_process_inspection_commands() -> None:
    module = _load_process_sentinel()
    root = Path("/repo/molt")
    samples = {
        10: module.memory_guard.ProcessSample(
            pid=10,
            ppid=1,
            pgid=10,
            rss_kb=100,
            command="ps -axo pid,command | rg 'molt-backend|/rustc'",
        ),
        11: module.memory_guard.ProcessSample(
            pid=11,
            ppid=1,
            pgid=11,
            rss_kb=100,
            command="git diff -- runtime/molt-backend/src/lib.rs",
        ),
        12: module.memory_guard.ProcessSample(
            pid=12,
            ppid=1,
            pgid=12,
            rss_kb=100,
            command="find . -path '*bench_exception_heavy.ir.json' -print",
        ),
        13: module.memory_guard.ProcessSample(
            pid=13,
            ppid=1,
            pgid=13,
            rss_kb=100,
            command="tail -80 tmp/exception-repro/cargo_release_build.stderr",
        ),
    }

    groups = module.process_groups(samples, root=root, self_pid=9999)

    assert groups == []


def test_process_groups_does_not_match_repo_root_alone() -> None:
    module = _load_process_sentinel()
    root = Path("/repo/molt")
    samples = {
        10: module.memory_guard.ProcessSample(
            pid=10,
            ppid=1,
            pgid=10,
            rss_kb=100,
            command="/bin/zsh -c cd /repo/molt && echo ok",
        ),
        11: module.memory_guard.ProcessSample(
            pid=11,
            ppid=1,
            pgid=11,
            rss_kb=100,
            command="/usr/bin/python /repo/molt/tests/tools/test_process_sentinel.py",
        ),
    }

    groups = module.process_groups(samples, root=root, self_pid=9999)

    assert groups == []


def test_process_groups_match_repo_scoped_cached_binary() -> None:
    module = _load_process_sentinel()
    root = Path("/repo/molt")
    samples = {
        10: module.memory_guard.ProcessSample(
            pid=10,
            ppid=1,
            pgid=10,
            rss_kb=100,
            command="/repo/molt/.molt_cache/home/bin/bench_exception_heavy_molt",
        )
    }

    groups = module.process_groups(samples, root=root, self_pid=9999)

    assert len(groups) == 1
    assert groups[0].pgid == 10


def test_find_violations_can_kill_all_or_threshold() -> None:
    module = _load_process_sentinel()
    group = module.ProcessGroup(
        pgid=10,
        matched=True,
        samples=(
            module.memory_guard.ProcessSample(
                pid=10,
                ppid=1,
                pgid=10,
                rss_kb=100,
                command="root",
            ),
            module.memory_guard.ProcessSample(
                pid=11,
                ppid=10,
                pgid=10,
                rss_kb=900,
                command="child",
            ),
        ),
    )

    kill_all = module.find_violations(
        [group],
        max_process_kb=10_000,
        max_group_kb=10_000,
        max_global_kb=10_000,
        kill_all=True,
    )
    process_rss = module.find_violations(
        [group],
        max_process_kb=800,
        max_group_kb=10_000,
        max_global_kb=10_000,
    )
    group_rss = module.find_violations(
        [group],
        max_process_kb=10_000,
        max_group_kb=999,
        max_global_kb=10_000,
    )

    assert kill_all[0].reason == "kill_all"
    assert process_rss[0].reason == "process_rss"
    assert group_rss[0].reason == "group_rss"


def test_find_violations_catches_aggregate_global_rss() -> None:
    module = _load_process_sentinel()
    groups = [
        module.ProcessGroup(
            pgid=10,
            matched=True,
            samples=(
                module.memory_guard.ProcessSample(
                    pid=10,
                    ppid=1,
                    pgid=10,
                    rss_kb=600,
                    command="first",
                ),
            ),
        ),
        module.ProcessGroup(
            pgid=20,
            matched=True,
            samples=(
                module.memory_guard.ProcessSample(
                    pid=20,
                    ppid=1,
                    pgid=20,
                    rss_kb=600,
                    command="second",
                ),
            ),
        ),
    ]

    violations = module.find_violations(
        groups,
        max_process_kb=10_000,
        max_group_kb=10_000,
        max_global_kb=1_000,
    )

    assert [violation.reason for violation in violations] == [
        "global_rss",
        "global_rss",
    ]
    assert [violation.pgid for violation in violations] == [10, 20]


def test_main_once_dry_run_reports_without_terminating(monkeypatch, capsys) -> None:
    module = _load_process_sentinel()
    terminated: list[int] = []

    monkeypatch.setattr(
        module.memory_guard,
        "sample_processes",
        lambda: {
            10: module.memory_guard.ProcessSample(
                pid=10,
                ppid=1,
                pgid=10,
                rss_kb=100,
                command=f"{module.repo_root()}/target/release-fast/molt-backend",
            )
        },
    )
    monkeypatch.setattr(
        module,
        "terminate_group",
        lambda pgid, *, grace: terminated.append(pgid),
    )

    rc = module.main(["--once", "--dry-run", "--kill-all"])

    assert rc == 1
    assert "kill_all" in capsys.readouterr().err
    assert terminated == []


def test_main_until_clean_drains_delayed_launches(monkeypatch) -> None:
    module = _load_process_sentinel()
    calls = 0
    terminated: list[int] = []

    def fake_sample_processes():
        nonlocal calls
        calls += 1
        if calls in {1, 3}:
            return {
                10 + calls: module.memory_guard.ProcessSample(
                    pid=10 + calls,
                    ppid=1,
                    pgid=10 + calls,
                    rss_kb=100,
                    command=f"{module.repo_root()}/target/release-fast/molt-backend",
                )
            }
        return {}

    monkeypatch.setattr(module.memory_guard, "sample_processes", fake_sample_processes)
    monkeypatch.setattr(
        module,
        "terminate_group",
        lambda pgid, *, grace: terminated.append(pgid),
    )

    rc = module.main(
        [
            "--kill-all",
            "--until-clean-sec",
            "0.003",
            "--max-runtime-sec",
            "1",
            "--poll-interval",
            "0.001",
        ]
    )

    assert rc == 0
    assert terminated == [11, 13]


def test_main_until_clean_waits_for_no_matched_groups(monkeypatch) -> None:
    module = _load_process_sentinel()
    calls = 0
    clock = 0.0

    def fake_sample_processes():
        nonlocal calls
        calls += 1
        if calls <= 4:
            return {
                10: module.memory_guard.ProcessSample(
                    pid=10,
                    ppid=1,
                    pgid=10,
                    rss_kb=100,
                    command=f"{module.repo_root()}/target/release-fast/molt-backend",
                )
            }
        return {}

    def fake_monotonic():
        return clock

    def fake_sleep(seconds: float) -> None:
        nonlocal clock
        clock += seconds

    monkeypatch.setattr(module.memory_guard, "sample_processes", fake_sample_processes)
    monkeypatch.setattr(module.time, "monotonic", fake_monotonic)
    monkeypatch.setattr(module.time, "sleep", fake_sleep)

    rc = module.main(
        [
            "--until-clean-sec",
            "0.003",
            "--max-runtime-sec",
            "1",
            "--poll-interval",
            "0.001",
        ]
    )

    assert rc == 0
    assert calls > 4


def test_main_rejects_once_with_until_clean(capsys) -> None:
    module = _load_process_sentinel()

    rc = module.main(["--once", "--until-clean-sec", "1"])

    assert rc == 2
    assert "mutually exclusive" in capsys.readouterr().err


def test_main_rejects_global_cap_without_margin(capsys) -> None:
    module = _load_process_sentinel()

    rc = module.main(["--once", "--max-global-rss-gb", "58"])

    assert rc == 2
    assert "below 58" in capsys.readouterr().err
