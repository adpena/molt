from __future__ import annotations

import importlib.util
from functools import cache
import sys
from pathlib import Path

from tools import guarded_entrypoints

REPO_ROOT = Path(__file__).resolve().parents[2]
SCRIPT_PATH = REPO_ROOT / "tools" / "process_sentinel.py"


@cache
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


def test_process_groups_require_repo_context_for_generic_molt_names() -> None:
    module = _load_process_sentinel()
    root = Path("/repo/molt")
    samples = {
        10: module.memory_guard.ProcessSample(
            pid=10,
            ppid=1,
            pgid=10,
            rss_kb=100,
            command="molt-backend --daemon",
        ),
        11: module.memory_guard.ProcessSample(
            pid=11,
            ppid=1,
            pgid=11,
            rss_kb=100,
            command="/other/checkout/target/dev-fast/molt-backend --daemon",
        ),
        12: module.memory_guard.ProcessSample(
            pid=12,
            ppid=1,
            pgid=12,
            rss_kb=100,
            command="/bin/zsh -c cd /repo/molt && python -m molt.cli run x.py",
        ),
    }

    groups = module.process_groups(samples, root=root, self_pid=9999)

    assert [group.pgid for group in groups] == [12]


def test_guarded_entrypoints_are_repo_sentinel_tokens() -> None:
    module = _load_process_sentinel()

    assert set(module.GUARDED_ENTRYPOINT_TOKENS) == (
        set(guarded_entrypoints.guarded_entrypoint_tokens(REPO_ROOT))
    )
    assert {
        "/bench/harness.py",
        "/bench/wasm_bench.py",
        "/bench/scripts/run_demo_bench.py",
        "/bench/scripts/run_db_stub.py",
        "/bench/luau/run_benchmarks.py",
        "/tests/benchmarks/bench_generator.py",
    }.issubset(module.GUARDED_ENTRYPOINT_TOKENS)


def test_guarded_entrypoint_scan_prefilters_non_candidates(
    tmp_path: Path, monkeypatch
) -> None:
    path = tmp_path / "large_generated.py"
    path.write_text("VALUE = " + repr("x" * 10_000), encoding="utf-8")

    def fail_parse(*_args, **_kwargs):
        raise AssertionError("non-candidate file reached ast.parse")

    monkeypatch.setattr(guarded_entrypoints.ast, "parse", fail_parse)

    assert not guarded_entrypoints._imports_harness_memory_guard(path)


def test_guarded_entrypoint_scan_skips_vendor_and_result_roots(
    tmp_path: Path,
) -> None:
    guarded_entrypoints._guarded_entrypoint_tokens.cache_clear()
    vendor = tmp_path / "bench" / "friends" / "repos" / "pkg" / "tool.py"
    result = tmp_path / "bench" / "results" / "run" / "generated.py"
    bench = tmp_path / "bench" / "harness.py"
    for path in (vendor, result, bench):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("from tools import harness_memory_guard\n", encoding="utf-8")

    try:
        tokens = guarded_entrypoints.guarded_entrypoint_tokens(tmp_path)
    finally:
        guarded_entrypoints._guarded_entrypoint_tokens.cache_clear()

    assert "/bench/harness.py" in tokens
    assert "/bench/friends/repos/pkg/tool.py" not in tokens
    assert "/bench/results/run/generated.py" not in tokens


def test_process_groups_match_guarded_entrypoints() -> None:
    module = _load_process_sentinel()
    root = Path("/repo/molt")
    samples = {
        idx: module.memory_guard.ProcessSample(
            pid=idx,
            ppid=1,
            pgid=idx,
            rss_kb=100,
            command=f"/usr/bin/python /repo/molt{token} --unit",
        )
        for idx, token in enumerate(module.GUARDED_ENTRYPOINT_TOKENS, start=10)
    }

    groups = module.process_groups(samples, root=root, self_pid=9999)

    assert [group.pgid for group in groups] == sorted(samples)


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


def test_process_groups_match_canonical_artifact_roots() -> None:
    module = _load_process_sentinel()
    root = Path("/repo/molt")
    samples = {
        10: module.memory_guard.ProcessSample(
            pid=10,
            ppid=1,
            pgid=10,
            rss_kb=100,
            command="/repo/molt/tmp/diff/case_1/main_molt",
        ),
        20: module.memory_guard.ProcessSample(
            pid=20,
            ppid=1,
            pgid=20,
            rss_kb=100,
            command="/repo/molt/dist/app_molt",
        ),
        30: module.memory_guard.ProcessSample(
            pid=30,
            ppid=1,
            pgid=30,
            rss_kb=100,
            command="/repo/molt/wasm/molt_runtime.wasm",
        ),
    }

    groups = module.process_groups(samples, root=root, self_pid=9999)

    assert [group.pgid for group in groups] == [10, 20, 30]


def test_process_groups_do_not_treat_generic_repo_artifact_paths_as_ownership() -> None:
    module = _load_process_sentinel()
    root = Path("/repo/molt")
    samples = {
        10: module.memory_guard.ProcessSample(
            pid=10,
            ppid=1,
            pgid=10,
            rss_kb=100,
            command="/repo/molt/tmp/user-script",
        ),
        20: module.memory_guard.ProcessSample(
            pid=20,
            ppid=1,
            pgid=20,
            rss_kb=100,
            command="/repo/molt/dist/user_tool",
        ),
        30: module.memory_guard.ProcessSample(
            pid=30,
            ppid=1,
            pgid=30,
            rss_kb=100,
            command="/repo/molt/build/unrelated",
        ),
        40: module.memory_guard.ProcessSample(
            pid=40,
            ppid=1,
            pgid=40,
            rss_kb=100,
            command="/repo/molt/wasm/unrelated.wasm",
        ),
        50: module.memory_guard.ProcessSample(
            pid=50,
            ppid=1,
            pgid=50,
            rss_kb=100,
            command="/repo/molt/.venv/bin/python /repo/molt/scripts/user_job.py",
        ),
        60: module.memory_guard.ProcessSample(
            pid=60,
            ppid=1,
            pgid=60,
            rss_kb=100,
            command="/usr/bin/python /repo/molt/tests/tools/test_process_sentinel.py",
        ),
    }

    groups = module.process_groups(samples, root=root, self_pid=9999)

    assert groups == []


def test_process_groups_match_windows_canonical_artifact_roots() -> None:
    module = _load_process_sentinel()
    root = Path("C:/Users/adpen/OneDrive/Documents/molt")
    samples = {
        10: module.memory_guard.ProcessSample(
            pid=10,
            ppid=1,
            pgid=10,
            rss_kb=100,
            command=(
                r"C:\Users\adpen\OneDrive\Documents\molt"
                r"\target\dev-fast\molt-backend.exe --daemon"
            ),
        ),
        20: module.memory_guard.ProcessSample(
            pid=20,
            ppid=1,
            pgid=20,
            rss_kb=100,
            command=(
                r"C:\Users\adpen\OneDrive\Documents\molt"
                r"\tmp\diff\case_1\main_molt.exe"
            ),
        ),
    }

    groups = module.process_groups(samples, root=root, self_pid=9999)

    assert [group.pgid for group in groups] == [10, 20]


def test_process_groups_propagate_to_nested_child_sessions() -> None:
    module = _load_process_sentinel()
    root = Path("/repo/molt")
    samples = {
        10: module.memory_guard.ProcessSample(
            pid=10,
            ppid=1,
            pgid=10,
            rss_kb=100,
            command="/usr/bin/python /repo/molt/tests/molt_diff.py",
        ),
        11: module.memory_guard.ProcessSample(
            pid=11,
            ppid=10,
            pgid=11,
            rss_kb=200,
            command="python child.py",
        ),
        12: module.memory_guard.ProcessSample(
            pid=12,
            ppid=11,
            pgid=12,
            rss_kb=300,
            command="node worker.js",
        ),
    }

    groups = module.process_groups(samples, root=root, self_pid=9999)

    assert [group.pgid for group in groups] == [10, 11, 12]
    assert [group.total_rss_kb for group in groups] == [100, 200, 300]


def test_process_groups_keep_observed_child_group_after_reparenting() -> None:
    module = _load_process_sentinel()
    root = Path("/repo/molt")
    samples = {
        12: module.memory_guard.ProcessSample(
            pid=12,
            ppid=1,
            pgid=12,
            rss_kb=300,
            command="node worker.js",
        ),
    }

    groups = module.process_groups(
        samples,
        root=root,
        self_pid=9999,
        known_process_identities={
            12: module.memory_guard.process_identity(samples[12]),
        },
    )

    assert len(groups) == 1
    assert groups[0].pgid == 12
    assert groups[0].total_rss_kb == 300


def test_process_groups_owned_filter_excludes_repo_matching_peer() -> None:
    module = _load_process_sentinel()
    root = Path("/repo/molt")
    samples = {
        20: module.memory_guard.ProcessSample(
            pid=20,
            ppid=10,
            pgid=20,
            rss_kb=100,
            command="/repo/molt/target/dev-fast/molt-backend --owned",
        ),
        30: module.memory_guard.ProcessSample(
            pid=30,
            ppid=1,
            pgid=30,
            rss_kb=200,
            command="/repo/molt/target/dev-fast/molt-backend --peer",
        ),
    }

    groups = module.process_groups(
        samples,
        root=root,
        self_pid=9999,
        owned_pids={20},
    )

    assert [group.pgid for group in groups] == [20]
    assert groups[0].command_text.endswith("--owned")


def test_process_groups_exclude_mixed_custody_group_with_owned_child() -> None:
    module = _load_process_sentinel()
    root = Path("/repo/molt")
    samples = {
        101: module.memory_guard.ProcessSample(
            pid=101,
            ppid=100,
            pgid=777,
            rss_kb=100,
            command="/repo/molt/target/dev-fast/molt-backend --owned",
        ),
        200: module.memory_guard.ProcessSample(
            pid=200,
            ppid=1,
            pgid=777,
            rss_kb=200,
            command="/Applications/Claude.app/Contents/MacOS/Claude",
        ),
    }

    groups = module.process_groups(samples, root=root, self_pid=9999)

    assert groups == []


def test_process_groups_explicit_custody_excludes_repo_scoped_shell() -> None:
    # CANONICAL custody guarantee the harness preflight relies on: requiring
    # EXPLICIT guard custody (an empty owned set, since a guard about to launch a
    # command owns nothing yet) yields ZERO kill candidates even for a repo-scope
    # heuristic match. A parent shell whose command line runs molt -- exactly what
    # Codex spawns -- is NEVER signalled. Locks the fix for the recurring
    # Codex-parent kill: harness_memory_guard `_prune_stale_repo_processes` passes
    # owned_pids=frozenset() so the preflight can never terminate a process it
    # cannot prove it owns.
    module = _load_process_sentinel()
    root = Path("/repo/molt")
    samples = {
        100: module.memory_guard.ProcessSample(
            pid=100,
            ppid=1,
            pgid=100,
            rss_kb=100,
            command="/bin/bash -c cd /repo/molt && cargo build -p molt-backend",
        ),
    }

    # Heuristic ownership WOULD match the shell (repo + molt build on its cmdline).
    heuristic = module.process_groups(samples, root=root, self_pid=9999)
    assert [group.pgid for group in heuristic] == [100]
    # Explicit custody (empty owned set) excludes it -- no kill candidate.
    explicit = module.process_groups(
        samples, root=root, self_pid=9999, owned_pids=frozenset()
    )
    assert explicit == []


def test_process_groups_exclude_windows_snapshot_helper_descendants(
    monkeypatch,
) -> None:
    module = _load_process_sentinel()
    root = Path("C:/repo/molt")
    samples = {
        100: module.memory_guard.ProcessSample(
            pid=100,
            ppid=999,
            pgid=None,
            rss_kb=100,
            command=(
                r"C:\repo\molt\.venv\Scripts\python.exe "
                r"C:\repo\molt\tools\memory_guard_core\windows_snapshot.py "
                "--molt-windows-process-snapshot-json"
            ),
        ),
        101: module.memory_guard.ProcessSample(
            pid=101,
            ppid=100,
            pgid=None,
            rss_kb=100,
            command=r"C:\Windows\System32\conhost.exe",
        ),
        200: module.memory_guard.ProcessSample(
            pid=200,
            ppid=1,
            pgid=None,
            rss_kb=200,
            command=r"C:\repo\molt\target\dev-fast\molt-backend.exe",
        ),
    }

    monkeypatch.setattr(module, "_is_windows_process_model", lambda: True)

    groups = module.process_groups(samples, root=root, self_pid=999)

    assert [group.pgid for group in groups] == [200]


def test_process_groups_exclude_explicitly_owned_windows_snapshot_helper_tree(
    monkeypatch,
) -> None:
    module = _load_process_sentinel()
    root = Path("C:/repo/molt")
    samples = {
        100: module.memory_guard.ProcessSample(
            pid=100,
            ppid=999,
            pgid=None,
            rss_kb=100,
            command=(
                r"C:\repo\molt\.venv\Scripts\python.exe "
                r"C:\repo\molt\tools\memory_guard_core\windows_snapshot.py "
                "--molt-windows-process-snapshot-json"
            ),
        ),
        101: module.memory_guard.ProcessSample(
            pid=101,
            ppid=100,
            pgid=None,
            rss_kb=100,
            command=r"C:\Windows\System32\conhost.exe",
        ),
        200: module.memory_guard.ProcessSample(
            pid=200,
            ppid=1,
            pgid=None,
            rss_kb=200,
            command=r"C:\repo\molt\target\dev-fast\molt-backend.exe",
        ),
    }

    monkeypatch.setattr(module, "_is_windows_process_model", lambda: True)

    groups = module.process_groups(
        samples,
        root=root,
        self_pid=999,
        owned_pids={100, 101, 200},
    )

    assert [group.pgid for group in groups] == [200]


def test_process_groups_exclude_codex_group_even_with_repo_child() -> None:
    module = _load_process_sentinel()
    root = Path("/repo/molt")
    samples = {
        100: module.memory_guard.ProcessSample(
            pid=100,
            ppid=1,
            pgid=100,
            rss_kb=500_000,
            command="/Applications/Codex.app/Contents/MacOS/Codex",
        ),
        101: module.memory_guard.ProcessSample(
            pid=101,
            ppid=100,
            pgid=100,
            rss_kb=250_000,
            command="/usr/bin/python /repo/molt/tests/molt_diff.py",
        ),
        200: module.memory_guard.ProcessSample(
            pid=200,
            ppid=1,
            pgid=200,
            rss_kb=100,
            command="/repo/molt/target/release-fast/molt-backend",
        ),
        300: module.memory_guard.ProcessSample(
            pid=300,
            ppid=1,
            pgid=300,
            rss_kb=500_000,
            command=(
                "node /opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/cli.js"
            ),
        ),
        301: module.memory_guard.ProcessSample(
            pid=301,
            ppid=300,
            pgid=300,
            rss_kb=250_000,
            command="/usr/bin/python /repo/molt/tests/molt_diff.py",
        ),
    }

    groups = module.process_groups(
        samples,
        root=root,
        self_pid=9999,
    )
    skipped = module.skipped_protected_process_groups(
        samples,
        root=root,
        self_pid=9999,
        observed_pgids={100, 300},
    )

    assert [group.pgid for group in groups] == [200]
    assert [group.pgid for group in skipped] == [100, 300]
    assert skipped[0].pids == [100, 101]
    assert skipped[1].pids == [300, 301]


def test_process_groups_exclude_claude_group_even_with_repo_child() -> None:
    module = _load_process_sentinel()
    root = Path("/repo/molt")
    samples = {
        100: module.memory_guard.ProcessSample(
            pid=100,
            ppid=1,
            pgid=100,
            rss_kb=500_000,
            command="claude --dangerously-skip-permissions",
        ),
        101: module.memory_guard.ProcessSample(
            pid=101,
            ppid=100,
            pgid=100,
            rss_kb=250_000,
            command="/usr/bin/python /repo/molt/tests/molt_diff.py",
        ),
        200: module.memory_guard.ProcessSample(
            pid=200,
            ppid=1,
            pgid=200,
            rss_kb=100,
            command="/repo/molt/target/release-fast/molt-backend",
        ),
    }

    groups = module.process_groups(
        samples,
        root=root,
        self_pid=9999,
    )
    skipped = module.skipped_protected_process_groups(
        samples,
        root=root,
        self_pid=9999,
        observed_pgids={100},
    )

    assert [group.pgid for group in groups] == [200]
    assert [group.pgid for group in skipped] == [100]
    assert skipped[0].pids == [100, 101]


def test_process_groups_exclude_external_codex_descendant_but_keep_owned_child() -> (
    None
):
    module = _load_process_sentinel()
    root = Path("/repo/molt")
    samples = {
        100: module.memory_guard.ProcessSample(
            pid=100,
            ppid=1,
            pgid=100,
            rss_kb=500_000,
            command="/Applications/Codex.app/Contents/MacOS/Codex",
        ),
        101: module.memory_guard.ProcessSample(
            pid=101,
            ppid=100,
            pgid=101,
            rss_kb=10_000,
            command="/bin/zsh -l",
        ),
        777: module.memory_guard.ProcessSample(
            pid=777,
            ppid=101,
            pgid=777,
            rss_kb=250_000,
            command="/repo/molt/target/dev-fast/molt-backend --daemon",
        ),
        999: module.memory_guard.ProcessSample(
            pid=999,
            ppid=100,
            pgid=999,
            rss_kb=30_000,
            command="/repo/molt/tools/process_sentinel.py --once --kill-all",
        ),
        200: module.memory_guard.ProcessSample(
            pid=200,
            ppid=999,
            pgid=200,
            rss_kb=250_000,
            command="/repo/molt/target/dev-fast/molt-backend --owned",
        ),
    }

    groups = module.process_groups(
        samples,
        root=root,
        self_pid=999,
        self_pgid=999,
    )
    skipped = module.skipped_protected_process_groups(
        samples,
        root=root,
        self_pid=999,
        self_pgid=999,
    )

    assert [group.pgid for group in groups] == [200]
    assert groups[0].pids == [200]
    assert [group.pgid for group in skipped] == [777]
    assert skipped[0].pids == [777]


def test_process_groups_exclude_external_codex_cli_descendant_but_keep_owned_child() -> (
    None
):
    module = _load_process_sentinel()
    root = Path("/repo/molt")
    samples = {
        100: module.memory_guard.ProcessSample(
            pid=100,
            ppid=1,
            pgid=100,
            rss_kb=500_000,
            command="/usr/local/bin/node /opt/homebrew/bin/codex",
        ),
        101: module.memory_guard.ProcessSample(
            pid=101,
            ppid=100,
            pgid=101,
            rss_kb=10_000,
            command="/bin/bash -lc pytest",
        ),
        777: module.memory_guard.ProcessSample(
            pid=777,
            ppid=101,
            pgid=777,
            rss_kb=250_000,
            command="/repo/molt/target/dev-fast/molt-backend --daemon",
        ),
        999: module.memory_guard.ProcessSample(
            pid=999,
            ppid=100,
            pgid=999,
            rss_kb=30_000,
            command="/repo/molt/tools/process_sentinel.py --once --kill-all",
        ),
        200: module.memory_guard.ProcessSample(
            pid=200,
            ppid=999,
            pgid=200,
            rss_kb=250_000,
            command="/repo/molt/target/dev-fast/molt-backend --owned",
        ),
    }

    groups = module.process_groups(
        samples,
        root=root,
        self_pid=999,
        self_pgid=999,
    )
    skipped = module.skipped_protected_process_groups(
        samples,
        root=root,
        self_pid=999,
        self_pgid=999,
    )

    assert [group.pgid for group in groups] == [200]
    assert groups[0].pids == [200]
    assert [group.pgid for group in skipped] == [777]
    assert skipped[0].pids == [777]


def test_process_groups_exclude_windows_external_codex_descendant_but_keep_owned_child(
    monkeypatch,
) -> None:
    module = _load_process_sentinel()
    root = Path("C:/Users/adpen/OneDrive/Documents/molt")
    samples = {
        100: module.memory_guard.ProcessSample(
            pid=100,
            ppid=1,
            pgid=None,
            rss_kb=500_000,
            command=(
                r"C:\Program Files\WindowsApps\OpenAI.Codex_26.609.4994.0_x64__2p2nqsd0c76g0"
                r"\app\resources\codex.exe"
            ),
        ),
        101: module.memory_guard.ProcessSample(
            pid=101,
            ppid=100,
            pgid=None,
            rss_kb=10_000,
            command="powershell.exe",
        ),
        777: module.memory_guard.ProcessSample(
            pid=777,
            ppid=101,
            pgid=None,
            rss_kb=250_000,
            command=(
                r"C:\Users\adpen\OneDrive\Documents\molt"
                r"\target\dev-fast\molt-backend.exe --daemon"
            ),
        ),
        999: module.memory_guard.ProcessSample(
            pid=999,
            ppid=1,
            pgid=None,
            rss_kb=30_000,
            command=(
                r"C:\Users\adpen\OneDrive\Documents\molt"
                r"\tools\process_sentinel.py --once --kill-all"
            ),
        ),
        200: module.memory_guard.ProcessSample(
            pid=200,
            ppid=999,
            pgid=None,
            rss_kb=250_000,
            command=(
                r"C:\Users\adpen\OneDrive\Documents\molt"
                r"\target\dev-fast\molt-backend.exe --owned"
            ),
        ),
    }

    monkeypatch.setattr(module, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(module.memory_guard, "_is_windows_process_model", lambda: True)
    groups = module.process_groups(
        samples,
        root=root,
        self_pid=999,
        self_pgid=None,
    )
    skipped = module.skipped_protected_process_groups(
        samples,
        root=root,
        self_pid=999,
        self_pgid=None,
        observed_pgids={777},
    )

    assert [group.pgid for group in groups] == [200]
    assert groups[0].pids == [200]
    assert [group.pgid for group in skipped] == [777]
    assert skipped[0].pids == [777]


def test_process_groups_exclude_windows_codex_claude_control_plane_paths(
    monkeypatch,
) -> None:
    module = _load_process_sentinel()
    root = Path("C:/Users/adpen/OneDrive/Documents/molt")
    samples = {
        100: module.memory_guard.ProcessSample(
            pid=100,
            ppid=1,
            pgid=None,
            rss_kb=100,
            command=(
                r"C:\Program Files\Git\usr\bin\tail.exe -f "
                r"C:\Users\adpen\AppData\Local\Temp\claude"
                r"\C--Users-adpen-OneDrive-Documents-molt\tasks\b1.output"
            ),
        ),
        101: module.memory_guard.ProcessSample(
            pid=101,
            ppid=1,
            pgid=None,
            rss_kb=100,
            command=(
                r"C:\Users\adpen\.codex\tmp\python.exe "
                r"C:\Users\adpen\OneDrive\Documents\molt\tests\molt_diff.py"
            ),
        ),
        102: module.memory_guard.ProcessSample(
            pid=102,
            ppid=1,
            pgid=None,
            rss_kb=100,
            command=(
                r"C:\Users\adpen\.claude\worktrees\e2e-stable"
                r"\tests\molt_diff.py --jobs 2"
            ),
        ),
        200: module.memory_guard.ProcessSample(
            pid=200,
            ppid=1,
            pgid=None,
            rss_kb=200,
            command=(
                r"C:\Users\adpen\OneDrive\Documents\molt"
                r"\target\dev-fast\molt-backend.exe --owned"
            ),
        ),
    }

    monkeypatch.setattr(module, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(module.memory_guard, "_is_windows_process_model", lambda: True)

    groups = module.process_groups(samples, root=root, self_pid=999)

    assert [group.pgid for group in groups] == [200]


def test_process_groups_exclude_external_claude_descendant_but_keep_owned_child() -> (
    None
):
    module = _load_process_sentinel()
    root = Path("/repo/molt")
    samples = {
        100: module.memory_guard.ProcessSample(
            pid=100,
            ppid=1,
            pgid=100,
            rss_kb=500_000,
            command="claude --dangerously-skip-permissions",
        ),
        101: module.memory_guard.ProcessSample(
            pid=101,
            ppid=100,
            pgid=101,
            rss_kb=10_000,
            command="/bin/zsh -c source /Users/adpena/.claude/shell-snapshots/snapshot-zsh",
        ),
        777: module.memory_guard.ProcessSample(
            pid=777,
            ppid=101,
            pgid=777,
            rss_kb=250_000,
            command="/repo/molt/target/dev-fast/molt-backend --daemon",
        ),
        999: module.memory_guard.ProcessSample(
            pid=999,
            ppid=1,
            pgid=999,
            rss_kb=30_000,
            command="/repo/molt/tools/process_sentinel.py --once --kill-all",
        ),
        200: module.memory_guard.ProcessSample(
            pid=200,
            ppid=999,
            pgid=200,
            rss_kb=250_000,
            command="/repo/molt/target/dev-fast/molt-backend --owned",
        ),
    }

    groups = module.process_groups(
        samples,
        root=root,
        self_pid=999,
        self_pgid=999,
    )
    skipped = module.skipped_protected_process_groups(
        samples,
        root=root,
        self_pid=999,
        self_pgid=999,
    )

    assert [group.pgid for group in groups] == [200]
    assert groups[0].pids == [200]
    assert [group.pgid for group in skipped] == [777]
    assert skipped[0].pids == [777]


def test_process_groups_exclude_ancestor_process_group() -> None:
    module = _load_process_sentinel()
    root = Path("/repo/molt")
    samples = {
        10: module.memory_guard.ProcessSample(
            pid=10,
            ppid=1,
            pgid=10,
            rss_kb=100,
            command="/Applications/Codex.app/Contents/MacOS/Codex",
        ),
        20: module.memory_guard.ProcessSample(
            pid=20,
            ppid=10,
            pgid=10,
            rss_kb=100,
            command="/Applications/Codex.app/Contents/Resources/app-server",
        ),
        9999: module.memory_guard.ProcessSample(
            pid=9999,
            ppid=20,
            pgid=10,
            rss_kb=100,
            command="/usr/bin/python -m pytest tests/test_memory_guard_tool.py",
        ),
        30: module.memory_guard.ProcessSample(
            pid=30,
            ppid=9999,
            pgid=10,
            rss_kb=100,
            command="/repo/molt/target/release-fast/molt-backend",
        ),
    }

    groups = module.process_groups(samples, root=root, self_pid=9999)
    skipped = module.skipped_protected_process_groups(
        samples,
        root=root,
        self_pid=9999,
    )

    assert groups == []
    assert [group.pgid for group in skipped] == [10]


def test_terminate_group_refuses_protected_codex_group(monkeypatch) -> None:
    module = _load_process_sentinel()
    samples = {
        100: module.memory_guard.ProcessSample(
            pid=100,
            ppid=1,
            pgid=100,
            rss_kb=500_000,
            command="/Applications/Codex.app/Contents/MacOS/Codex",
        ),
        101: module.memory_guard.ProcessSample(
            pid=101,
            ppid=100,
            pgid=100,
            rss_kb=250_000,
            command="/repo/molt/target/release-fast/molt-backend",
        ),
    }
    sent_groups: list[tuple[int, int]] = []
    monkeypatch.setattr(module, "sample_processes_for_sentinel", lambda: samples)
    monkeypatch.setattr(module.os, "getpid", lambda: 9999)
    monkeypatch.setattr(module, "_safe_getpgrp", lambda: 999)
    monkeypatch.setattr(
        module.os,
        "killpg",
        lambda pgid, sig: sent_groups.append((pgid, sig)),
        raising=False,
    )
    monkeypatch.setattr(
        module.os,
        "kill",
        lambda pid, sig: sent_groups.append((pid, sig)),
        raising=False,
    )

    module.terminate_group(100, grace=0.001, root=Path("/repo/molt"))

    assert sent_groups == []


def test_terminate_group_refuses_posix_group_without_expected_identities(
    monkeypatch,
) -> None:
    module = _load_process_sentinel()
    samples = {
        100: module.memory_guard.ProcessSample(
            pid=100,
            ppid=1,
            pgid=100,
            rss_kb=500_000,
            command="/repo/molt/target/release-fast/molt-backend",
        ),
    }
    sent_groups: list[tuple[int, int]] = []

    monkeypatch.setattr(module, "_is_windows_process_model", lambda: False)
    monkeypatch.setattr(module, "sample_processes_for_sentinel", lambda: samples)
    monkeypatch.setattr(module.os, "getpid", lambda: 9999)
    monkeypatch.setattr(module, "_safe_getpgrp", lambda: 999)
    monkeypatch.setattr(
        module.os,
        "killpg",
        lambda pgid, sig: sent_groups.append((pgid, sig)),
        raising=False,
    )

    module.terminate_group(100, grace=0.001, root=Path("/repo/molt"))

    assert sent_groups == []


def test_terminate_group_uses_pid_kill_without_getpgrp_on_windows(monkeypatch) -> None:
    module = _load_process_sentinel()
    sample = module.memory_guard.ProcessSample(
        pid=100,
        ppid=1,
        pgid=None,
        rss_kb=500_000,
        command="/repo/molt/target/release-fast/molt-backend",
    )
    samples = {100: sample}
    killed: list[tuple[int, int]] = []
    killpg_calls: list[tuple[int, int]] = []

    monkeypatch.setattr(module, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(module, "_safe_getpgrp", lambda: None)
    monkeypatch.setattr(module.os, "getpid", lambda: 9999)
    monkeypatch.setattr(module.os, "kill", lambda pid, sig: killed.append((pid, sig)))
    monkeypatch.setattr(
        module.os,
        "killpg",
        lambda pgid, sig: killpg_calls.append((pgid, sig)),
        raising=False,
    )
    monkeypatch.setattr(module.time, "sleep", lambda _seconds: None)
    monkeypatch.setattr(module, "sample_processes_for_sentinel", lambda: samples)

    module.terminate_group(
        100,
        grace=0.001,
        root=Path("/repo/molt"),
        expected_identity=module.memory_guard.process_identity(sample),
    )

    assert [pid for pid, _sig in killed] == [100, 100]
    assert killpg_calls == []


def test_terminate_group_refuses_windows_group_without_expected_identity(
    monkeypatch,
) -> None:
    module = _load_process_sentinel()
    samples = {
        100: module.memory_guard.ProcessSample(
            pid=100,
            ppid=1,
            pgid=None,
            rss_kb=500_000,
            command="/repo/molt/target/release-fast/molt-backend",
        ),
    }
    killed: list[tuple[int, int]] = []

    monkeypatch.setattr(module, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(module, "_safe_getpgrp", lambda: None)
    monkeypatch.setattr(module.os, "getpid", lambda: 9999)
    monkeypatch.setattr(module.os, "kill", lambda pid, sig: killed.append((pid, sig)))
    monkeypatch.setattr(module, "sample_processes_for_sentinel", lambda: samples)

    module.terminate_group(100, grace=0.001, root=Path("/repo/molt"))

    assert killed == []


def test_terminate_group_windows_refuses_reused_pid_identity(monkeypatch) -> None:
    module = _load_process_sentinel()
    selected = module.memory_guard.ProcessSample(
        pid=100,
        ppid=1,
        pgid=None,
        rss_kb=500_000,
        command="/repo/molt/target/release-fast/molt-backend",
        started_at_ns=111,
    )
    samples = {
        100: module.memory_guard.ProcessSample(
            pid=100,
            ppid=1,
            pgid=None,
            rss_kb=500_000,
            command="/repo/molt/target/release-fast/molt-backend",
            started_at_ns=222,
        ),
    }
    killed: list[tuple[int, int]] = []

    monkeypatch.setattr(module, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(module.memory_guard, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(module, "_safe_getpgrp", lambda: None)
    monkeypatch.setattr(module.os, "getpid", lambda: 9999)
    monkeypatch.setattr(module.os, "kill", lambda pid, sig: killed.append((pid, sig)))
    monkeypatch.setattr(module, "sample_processes_for_sentinel", lambda: samples)

    module.terminate_group(
        100,
        grace=0.001,
        root=Path("/repo/molt"),
        expected_identity=module.memory_guard.process_identity(selected),
    )

    assert killed == []


def test_terminate_group_refuses_windows_codex_app_server(monkeypatch) -> None:
    module = _load_process_sentinel()
    samples = {
        100: module.memory_guard.ProcessSample(
            pid=100,
            ppid=42,
            pgid=None,
            rss_kb=500_000,
            command=(
                r'"C:\Program Files\WindowsApps\OpenAI.Codex_26.609.4994.0_x64__2p2nqsd0c76g0'
                r'\app\resources\codex.exe" app-server --analytics-default-enabled'
            ),
        ),
        101: module.memory_guard.ProcessSample(
            pid=101,
            ppid=100,
            pgid=None,
            rss_kb=100,
            command=(
                r'"C:\Users\adpen\AppData\Local\OpenAI\Codex\runtimes'
                r'\cua_node\bin\node_repl.exe"'
            ),
        ),
    }
    killed: list[tuple[int, int]] = []

    monkeypatch.setattr(module, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(module.memory_guard, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(module, "_safe_getpgrp", lambda: None)
    monkeypatch.setattr(module.os, "getpid", lambda: 9999)
    monkeypatch.setattr(module.os, "kill", lambda pid, sig: killed.append((pid, sig)))
    monkeypatch.setattr(module, "sample_processes_for_sentinel", lambda: samples)

    module.terminate_group(100, grace=0.001)
    module.terminate_group(101, grace=0.001)

    assert killed == []


def test_terminate_group_windows_refuses_external_codex_descendant_repo_command(
    monkeypatch,
) -> None:
    module = _load_process_sentinel()
    samples = {
        100: module.memory_guard.ProcessSample(
            pid=100,
            ppid=1,
            pgid=None,
            rss_kb=500_000,
            command=(
                r"C:\Program Files\WindowsApps\OpenAI.Codex_26.609.4994.0_x64__2p2nqsd0c76g0"
                r"\app\resources\codex.exe"
            ),
        ),
        101: module.memory_guard.ProcessSample(
            pid=101,
            ppid=100,
            pgid=None,
            rss_kb=10_000,
            command="powershell.exe",
        ),
        777: module.memory_guard.ProcessSample(
            pid=777,
            ppid=101,
            pgid=None,
            rss_kb=250_000,
            command=(
                r"C:\Users\adpen\OneDrive\Documents\molt"
                r"\target\dev-fast\molt-backend.exe --daemon"
            ),
        ),
    }
    killed: list[tuple[int, int]] = []

    monkeypatch.setattr(module, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(module.memory_guard, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(module, "_safe_getpgrp", lambda: None)
    monkeypatch.setattr(module.os, "getpid", lambda: 9999)
    monkeypatch.setattr(module.os, "kill", lambda pid, sig: killed.append((pid, sig)))
    monkeypatch.setattr(module, "sample_processes_for_sentinel", lambda: samples)

    module.terminate_group(
        777,
        grace=0.001,
        root=Path("C:/Users/adpen/OneDrive/Documents/molt"),
    )

    assert killed == []


def test_terminate_group_windows_keeps_current_sentinel_child_killable(
    monkeypatch,
) -> None:
    module = _load_process_sentinel()
    root = Path("C:/Users/adpen/OneDrive/Documents/molt")
    samples = {
        100: module.memory_guard.ProcessSample(
            pid=100,
            ppid=1,
            pgid=None,
            rss_kb=500_000,
            command=(
                r"C:\Program Files\WindowsApps\OpenAI.Codex_26.609.4994.0_x64__2p2nqsd0c76g0"
                r"\app\resources\codex.exe"
            ),
        ),
        999: module.memory_guard.ProcessSample(
            pid=999,
            ppid=100,
            pgid=None,
            rss_kb=30_000,
            command=(
                r"C:\Users\adpen\OneDrive\Documents\molt"
                r"\tools\process_sentinel.py --once --kill-all"
            ),
        ),
        200: module.memory_guard.ProcessSample(
            pid=200,
            ppid=999,
            pgid=None,
            rss_kb=250_000,
            command=(
                r"C:\Users\adpen\OneDrive\Documents\molt"
                r"\target\dev-fast\molt-backend.exe --owned"
            ),
        ),
    }
    killed: list[tuple[int, int]] = []

    monkeypatch.setattr(module, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(module.memory_guard, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(module, "_safe_getpgrp", lambda: None)
    monkeypatch.setattr(module.os, "getpid", lambda: 999)
    monkeypatch.setattr(module.os, "kill", lambda pid, sig: killed.append((pid, sig)))
    monkeypatch.setattr(module.time, "sleep", lambda _seconds: None)
    monkeypatch.setattr(module, "sample_processes_for_sentinel", lambda: samples)

    module.terminate_group(
        200,
        grace=0.001,
        root=root,
        expected_identities={200: module.memory_guard.process_identity(samples[200])},
    )

    assert [pid for pid, _sig in killed] == [200, 200]


def test_terminate_group_rechecks_protection_before_sigterm(monkeypatch) -> None:
    module = _load_process_sentinel()
    first = {
        100: module.memory_guard.ProcessSample(
            pid=100,
            ppid=1,
            pgid=100,
            rss_kb=500_000,
            command="/repo/molt/target/release-fast/molt-backend",
        )
    }
    protected = {
        100: module.memory_guard.ProcessSample(
            pid=100,
            ppid=1,
            pgid=100,
            rss_kb=500_000,
            command="/Applications/Codex.app/Contents/MacOS/Codex",
        )
    }
    calls = 0

    def sample_processes():
        nonlocal calls
        calls += 1
        return first if calls == 1 else protected

    sent_groups: list[tuple[int, int]] = []
    monkeypatch.setattr(module, "_is_windows_process_model", lambda: False)
    monkeypatch.setattr(module, "sample_processes_for_sentinel", sample_processes)
    monkeypatch.setattr(module.os, "getpid", lambda: 9999)
    monkeypatch.setattr(module, "_safe_getpgrp", lambda: 999)
    monkeypatch.setattr(
        module.os,
        "killpg",
        lambda pgid, sig: sent_groups.append((pgid, sig)),
        raising=False,
    )

    module.terminate_group(100, grace=0.0)

    assert sent_groups == []


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
    payload = module.violation_payload(kill_all[0])
    assert payload["external_parent_pids"] == [1]
    assert payload["process_samples"] == [
        {
            "pid": 10,
            "ppid": 1,
            "pgid": 10,
            "rss_kb": 100,
            "elapsed_sec": None,
            "command": "root",
        },
        {
            "pid": 11,
            "ppid": 10,
            "pgid": 10,
            "rss_kb": 900,
            "elapsed_sec": None,
            "command": "child",
        },
    ]


def test_find_violations_marks_only_stale_orphaned_groups() -> None:
    module = _load_process_sentinel()
    stale_orphan = module.ProcessGroup(
        pgid=10,
        matched=True,
        samples=(
            module.memory_guard.ProcessSample(
                pid=10,
                ppid=1,
                pgid=10,
                rss_kb=100,
                command="molt-backend --daemon",
                elapsed_sec=4000,
            ),
        ),
    )
    attached_old = module.ProcessGroup(
        pgid=20,
        matched=True,
        samples=(
            module.memory_guard.ProcessSample(
                pid=20,
                ppid=999,
                pgid=20,
                rss_kb=100,
                command="cargo test -p molt-backend",
                elapsed_sec=4000,
            ),
        ),
    )

    violations = module.find_violations(
        [stale_orphan, attached_old],
        max_process_kb=10_000,
        max_group_kb=10_000,
        max_global_kb=10_000,
        stale_orphan_sec=3600,
    )

    assert len(violations) == 1
    assert violations[0].reason == "stale_orphan"
    assert violations[0].pgid == 10
    assert violations[0].oldest_elapsed_sec == 4000
    assert violations[0].stale_sec == 3600
    assert violations[0].orphaned is True
    assert module.violation_payload(violations[0])["oldest_elapsed_sec"] == 4000


def test_find_violations_uses_shorter_pytest_stale_threshold() -> None:
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
                command="/repo/molt/.venv/bin/python3 -m pytest tests/compliance",
                elapsed_sec=1200,
            ),
        ),
    )

    violations = module.find_violations(
        [group],
        max_process_kb=10_000,
        max_group_kb=10_000,
        max_global_kb=10_000,
        stale_orphan_sec=3600,
        stale_pytest_sec=900,
    )

    assert len(violations) == 1
    assert violations[0].reason == "stale_pytest_orphan"
    assert violations[0].stale_sec == 900


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


def test_main_once_refreshes_adaptive_limits_with_matched_group_rss(
    monkeypatch,
) -> None:
    module = _load_process_sentinel()
    group = module.ProcessGroup(
        pgid=10,
        matched=True,
        samples=(
            module.memory_guard.ProcessSample(
                pid=10,
                ppid=1,
                pgid=10,
                rss_kb=20 * 1024 * 1024,
                command=f"{module.repo_root()}/target/release-fast/molt-backend",
            ),
        ),
    )
    accounted: list[int] = []
    captured: dict[str, int] = {}

    def fake_budget(prefix, environ=None, *, accounted_rss_kb=0):
        assert prefix == "MOLT_SENTINEL"
        accounted.append(accounted_rss_kb)
        return module.memory_guard.AdaptiveMemoryBudget(
            max_process_rss_gb=2,
            max_total_rss_gb=3,
            max_global_rss_gb=4,
            reserve_gb=1,
            physical_gb=16,
            available_gb=12,
            source="test",
            accounted_rss_gb=accounted_rss_kb / (1024 * 1024),
        )

    def fake_find_violations(
        groups,
        *,
        max_process_kb,
        max_group_kb,
        max_global_kb,
        kill_all=False,
        stale_orphan_sec=None,
        stale_pytest_sec=None,
    ):
        del stale_orphan_sec, stale_pytest_sec
        captured["max_process_kb"] = max_process_kb
        captured["max_group_kb"] = max_group_kb
        captured["max_global_kb"] = max_global_kb
        return []

    monkeypatch.setattr(module.memory_guard, "adaptive_memory_budget", fake_budget)
    monkeypatch.setattr(module, "sample_processes_for_sentinel", lambda: {})
    monkeypatch.setattr(module, "process_groups", lambda *args, **kwargs: [group])
    monkeypatch.setattr(module, "find_violations", fake_find_violations)

    rc = module.main(["--once", "--dry-run"])

    assert rc == 0
    assert accounted[-1] == group.total_rss_kb
    assert captured == {
        "max_process_kb": 2 * 1024 * 1024,
        "max_group_kb": 3 * 1024 * 1024,
        "max_global_kb": 4 * 1024 * 1024,
    }


def test_parser_accepts_group_and_tree_rss_aliases() -> None:
    module = _load_process_sentinel()

    group_args = module._parser().parse_args(["--once", "--max-group-rss-gb", "2.5"])
    tree_args = module._parser().parse_args(["--once", "--max-tree-rss-gb", "3.5"])

    assert group_args.max_total_rss_gb == 2.5
    assert tree_args.max_total_rss_gb == 3.5


def test_sample_processes_for_sentinel_uses_windows_hard_sampler(monkeypatch) -> None:
    module = _load_process_sentinel()
    sample = module.memory_guard.ProcessSample(
        pid=7,
        ppid=1,
        pgid=None,
        rss_kb=9,
        command="python.exe",
    )

    def fail_hot_sampler():
        raise AssertionError("Windows sentinel cleanup must use hard-timeout sampler")

    monkeypatch.setattr(module, "_is_windows_process_model", lambda: True)
    monkeypatch.setattr(module.memory_guard, "sample_processes", fail_hot_sampler)
    monkeypatch.setattr(
        module.memory_guard,
        "sample_processes_windows_hard_timeout",
        lambda: {7: sample},
    )

    assert module.sample_processes_for_sentinel() == {7: sample}


def test_sample_processes_for_sentinel_uses_default_sampler_on_posix(
    monkeypatch,
) -> None:
    module = _load_process_sentinel()
    sample = module.memory_guard.ProcessSample(
        pid=7,
        ppid=1,
        pgid=7,
        rss_kb=9,
        command="python",
    )

    def fail_windows_sampler():
        raise AssertionError("POSIX sentinel cleanup must use default process sampler")

    monkeypatch.setattr(module, "_is_windows_process_model", lambda: False)
    monkeypatch.setattr(module.memory_guard, "sample_processes", lambda: {7: sample})
    monkeypatch.setattr(
        module.memory_guard,
        "sample_processes_windows_hard_timeout",
        fail_windows_sampler,
    )

    assert module.sample_processes_for_sentinel() == {7: sample}


def test_main_once_dry_run_reports_without_terminating(monkeypatch, capsys) -> None:
    module = _load_process_sentinel()
    terminated: list[int] = []

    monkeypatch.setattr(
        module,
        "sample_processes_for_sentinel",
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
        lambda pgid, *, grace, **_kwargs: terminated.append(pgid),
    )

    rc = module.main(["--once", "--dry-run", "--kill-all"])

    assert rc == 1
    assert "kill_all" in capsys.readouterr().err
    assert terminated == []


def test_main_once_reports_repo_match_without_terminating(
    monkeypatch,
    capsys,
) -> None:
    module = _load_process_sentinel()
    terminated: list[tuple[int, float]] = []
    clock = 100.0

    monkeypatch.setattr(module, "_utc_timestamp", lambda: "2026-05-24T10:00:00Z")
    monkeypatch.setattr(module.time, "monotonic", lambda: clock)
    monkeypatch.setattr(
        module,
        "sample_processes_for_sentinel",
        lambda: {
            10: module.memory_guard.ProcessSample(
                pid=10,
                ppid=1,
                pgid=10,
                rss_kb=100,
                command=f"{module.repo_root()}/target/release-fast/molt-backend",
                elapsed_sec=1200,
            )
        },
    )
    monkeypatch.setattr(
        module,
        "terminate_group",
        lambda pgid, *, grace, **_kwargs: terminated.append((pgid, grace)),
    )

    rc = module.main(["--once", "--kill-all", "--grace-sec", "0.25"])

    assert rc == 1
    assert terminated == []
    err = capsys.readouterr().err
    assert "action=dry_run" in err
    assert "kill_all" in err
    assert "pgid=10" in err
    assert "observed_at=2026-05-24T10:00:00Z" in err
    assert "elapsed=0.00s" in err
    assert "age=1200s" in err
    assert "grace=0.25s" in err
    assert "next action: rerun the interrupted build/test/bench command" in err


def test_main_json_reports_operator_incident_fields(monkeypatch, capsys) -> None:
    module = _load_process_sentinel()

    monkeypatch.setattr(module, "_utc_timestamp", lambda: "2026-05-24T10:00:00Z")
    monkeypatch.setattr(module.time, "monotonic", lambda: 50.0)
    monkeypatch.setenv("MOLT_SESSION_ID", "sentinel-unit")
    monkeypatch.setenv(
        "PYTEST_CURRENT_TEST",
        "tests/tools/test_process_sentinel.py::json_unit (call)",
    )
    monkeypatch.setattr(
        module,
        "sample_processes_for_sentinel",
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
        lambda pgid, *, grace, **_kwargs: None,
    )

    rc = module.main(["--once", "--dry-run", "--kill-all", "--json"])

    assert rc == 1
    payload = module.json.loads(capsys.readouterr().out)
    assert payload["event"] == "process_sentinel_violation"
    assert payload["action"] == "dry_run"
    assert payload["observed_at"] == "2026-05-24T10:00:00Z"
    assert payload["elapsed_s"] == 0.0
    assert payload["grace_sec"] == module.DEFAULT_GRACE_SEC
    assert payload["kill_scope"] == "repo"
    assert payload["killer_label"] == "tools/process_sentinel.py"
    assert payload["killer_pid"] == module.os.getpid()
    assert payload["killer_session_id"] == "sentinel-unit"
    assert payload["victim_pgid"] == 10
    assert payload["victim_command"].endswith("molt-backend")
    assert payload["owner_match_reason"] == "repo_scope"
    assert payload["termination"]["signal"]["name"] == "SIGTERM"
    assert payload["termination"]["fallback_signal"] == (
        module.memory_guard.fallback_kill_signal_payload()
    )
    assert payload["termination"]["grace_sec"] == module.DEFAULT_GRACE_SEC
    assert payload["termination"]["rss_triggered"] is False
    assert payload["violation"]["reason"] == "kill_all"
    assert payload["violation"]["pgid"] == 10
    assert payload["violation"]["external_parent_pids"] == [1]
    assert payload["violation"]["process_samples"][0]["pid"] == 10
    assert payload["repro"]["cwd"] == str(module.repo_root())
    assert payload["repro"]["env"]["MOLT_SESSION_ID"] == "sentinel-unit"
    assert payload["repro"]["pytest"]["current_test"].startswith(
        "tests/tools/test_process_sentinel.py::json_unit"
    )
    assert payload["repro"]["limits"]["max_global_rss_kb"] is not None
    assert payload["repro"]["sentinel"]["argv"][-4:] == [
        "--once",
        "--dry-run",
        "--kill-all",
        "--json",
    ]
    assert payload["next_action"].startswith("rerun the interrupted")


def test_main_until_clean_drains_delayed_launches(monkeypatch) -> None:
    module = _load_process_sentinel()
    calls = 0
    clock = 0.0
    terminated: list[int] = []
    first_pgid = 1_000_011
    second_pgid = 1_000_013

    def fake_sample_processes():
        nonlocal calls
        calls += 1
        pgid_by_call = {2: first_pgid, 4: second_pgid}
        if calls in {2, 4}:
            pgid = pgid_by_call[calls]
            return {
                pgid: module.memory_guard.ProcessSample(
                    pid=pgid,
                    ppid=1,
                    pgid=pgid,
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

    monkeypatch.setattr(module, "sample_processes_for_sentinel", fake_sample_processes)
    monkeypatch.setattr(module.time, "monotonic", fake_monotonic)
    monkeypatch.setattr(module.time, "sleep", fake_sleep)
    monkeypatch.setattr(
        module,
        "terminate_group",
        lambda pgid, *, grace, **_kwargs: terminated.append(pgid),
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
    assert terminated == []


def test_main_until_clean_waits_for_no_matched_groups(monkeypatch) -> None:
    module = _load_process_sentinel()
    calls = 0
    clock = 0.0
    matched_pgid = 1_000_021

    def fake_sample_processes():
        nonlocal calls
        calls += 1
        if calls <= 4:
            return {
                matched_pgid: module.memory_guard.ProcessSample(
                    pid=matched_pgid,
                    ppid=1,
                    pgid=matched_pgid,
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

    monkeypatch.setattr(module, "sample_processes_for_sentinel", fake_sample_processes)
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


def test_main_rejects_implausible_process_cap_with_scope(capsys) -> None:
    module = _load_process_sentinel()

    rc = module.main(["--once", "--max-process-rss-gb", "112"])

    assert rc == 2
    assert "max process RSS must stay below 112 GB" in capsys.readouterr().err


def test_main_rejects_implausible_group_cap_with_scope(capsys) -> None:
    module = _load_process_sentinel()

    rc = module.main(["--once", "--max-group-rss-gb", "112"])

    assert rc == 2
    assert "max group RSS must stay below 112 GB" in capsys.readouterr().err


def test_main_rejects_implausible_global_cap_without_margin(capsys) -> None:
    module = _load_process_sentinel()

    rc = module.main(["--once", "--max-global-rss-gb", "4096"])

    assert rc == 2
    assert "max global RSS must stay below 4096 GB" in capsys.readouterr().err
