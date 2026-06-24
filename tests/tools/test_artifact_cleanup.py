from __future__ import annotations

import importlib.util
import json
from pathlib import Path
from types import SimpleNamespace


REPO_ROOT = Path(__file__).resolve().parents[2]
ARTIFACT_CLEANUP = REPO_ROOT / "tools" / "artifact_cleanup.py"


def _load_artifact_cleanup():
    spec = importlib.util.spec_from_file_location(
        "molt_tools_artifact_cleanup", ARTIFACT_CLEANUP
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_default_pathspecs_exclude_stateful_roots() -> None:
    module = _load_artifact_cleanup()
    defaults = set(module.default_pathspecs())

    for pathspec in module.stateful_pathspecs():
        assert pathspec not in defaults


def test_default_pathspecs_avoid_recursive_globs() -> None:
    module = _load_artifact_cleanup()

    assert all("**" not in pathspec for pathspec in module.default_pathspecs())


def test_default_pathspecs_avoid_broad_leading_wildcards() -> None:
    module = _load_artifact_cleanup()

    assert all(
        not pathspec.lstrip(":").startswith("*")
        for pathspec in module.default_pathspecs()
    )


def test_default_pathspecs_cover_canonical_local_artifact_roots() -> None:
    module = _load_artifact_cleanup()

    assert {
        "target/",
        "target-*",
        "tmp/",
        ".molt_cache/",
        ".molt_cache-*/",
        ".uv-cache/",
        ".uv-cache-*/",
        "bin/",
        "logs/",
        "bench/results/",
        "bench/scoreboard/host_calibration/",
        "wasm/molt_runtime.wasm",
        "wasm/molt_runtime_reloc.wasm",
    }.issubset(set(module.default_pathspecs()))


def test_default_cleanup_intentionally_covers_cargo_quarantine_receipts() -> None:
    module = _load_artifact_cleanup()
    defaults = set(module.default_pathspecs())
    stateful = set(module.stateful_pathspecs())

    quarantine_receipt = (
        "target/.molt_state/quarantine/cargo_incremental/q/receipt.json"
    )
    assert quarantine_receipt.startswith("target/")
    assert "target/" in defaults
    assert "target/" not in stateful
    assert "target/.molt_state/quarantine/cargo_incremental/" not in stateful
    assert "target/" in module.build_git_clean_command(
        apply=True,
        pathspecs=module.default_pathspecs(),
    )


def test_extra_pathspecs_reject_stateful_roots() -> None:
    module = _load_artifact_cleanup()

    for pathspec in [".venv/", ".omx/cache", "third_party/tool"]:
        try:
            module.validate_extra_pathspecs([pathspec])
        except ValueError as exc:
            assert "stateful data" in str(exc)
        else:
            raise AssertionError(f"{pathspec} should have been rejected")


def test_extra_pathspecs_reject_nonliteral_paths() -> None:
    module = _load_artifact_cleanup()

    for pathspec in ["/tmp/cache", "../tmp", "tests/**/__pycache__/", ":(glob)tmp/*"]:
        try:
            module.validate_extra_pathspecs([pathspec])
        except ValueError:
            pass
        else:
            raise AssertionError(f"{pathspec} should have been rejected")


def test_repo_root_must_be_this_checkout(tmp_path: Path) -> None:
    module = _load_artifact_cleanup()
    other_repo = tmp_path / "other"
    other_repo.mkdir()
    (other_repo / "pyproject.toml").write_text("[project]\nname = 'molt'\n")

    try:
        module.validate_repo_root(other_repo)
    except ValueError as exc:
        assert "not this Molt checkout" in str(exc)
    else:
        raise AssertionError("foreign repo root should have been rejected")


def test_git_clean_command_is_dry_run_by_default() -> None:
    module = _load_artifact_cleanup()

    cmd = module.build_git_clean_command(
        apply=False,
        pathspecs=["target/", "tmp/"],
    )

    assert cmd == ["git", "clean", "-ndX", "--", "target/", "tmp/"]


def test_git_clean_command_apply_uses_delete_mode() -> None:
    module = _load_artifact_cleanup()

    cmd = module.build_git_clean_command(
        apply=True,
        pathspecs=["target/", "tmp/"],
    )

    assert cmd == ["git", "clean", "-fdX", "--", "target/", "tmp/"]


def test_main_dry_run_invokes_git_clean_without_process_kill(monkeypatch) -> None:
    module = _load_artifact_cleanup()
    calls: list[dict[str, object]] = []
    for key in (
        "MOLT_EXT_ROOT",
        "CARGO_TARGET_DIR",
        "MOLT_DIFF_CARGO_TARGET_DIR",
        "MOLT_CACHE",
        "MOLT_DIFF_ROOT",
        "MOLT_DIFF_TMPDIR",
        "UV_CACHE_DIR",
        "TMPDIR",
    ):
        monkeypatch.delenv(key, raising=False)

    def fake_guarded_completed_process(cmd, **kwargs):
        calls.append({"cmd": list(cmd), **kwargs})
        return SimpleNamespace(returncode=0)

    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    rc = module.main([])

    assert rc == 0
    assert calls[0]["cmd"] == module.build_git_clean_command(
        apply=False,
        pathspecs=module.default_pathspecs(),
    )
    assert calls[0]["prefix"] == "MOLT_DEV_CLEANUP"
    assert calls[0]["cwd"] == module.REPO_ROOT
    assert calls[0]["capture_output"] is False
    assert calls[0]["env"]["MOLT_EXT_ROOT"] == str(module.REPO_ROOT)


def test_main_json_reports_git_clean_entries(monkeypatch, capsys) -> None:
    module = _load_artifact_cleanup()

    def fake_guarded_completed_process(cmd, **_kwargs):
        return SimpleNamespace(
            returncode=0,
            stdout="Would remove target/\nWould remove tmp/cache/\n",
            stderr="",
        )

    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    rc = module.main(["--json"])

    assert rc == 0
    payload = json.loads(capsys.readouterr().out)
    assert payload["command"] == "artifact_cleanup"
    assert payload["status"] == "ok"
    assert payload["data"]["mode"] == "dry-run"
    assert payload["data"]["entries"] == [
        {"action": "would_remove", "path": "target/"},
        {"action": "would_remove", "path": "tmp/cache/"},
    ]


def test_main_rejects_stateful_extra_before_git_clean(monkeypatch) -> None:
    module = _load_artifact_cleanup()

    def fail_run(*_args, **_kwargs):
        raise AssertionError("git clean must not run for rejected extra pathspecs")

    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        fail_run,
    )

    rc = module.main(["--extra-path", ".venv/"])

    assert rc == 2


def test_main_apply_accepts_sentinel_kill_report(monkeypatch) -> None:
    module = _load_artifact_cleanup()
    calls: list[list[str]] = []

    def fake_guarded_completed_process(cmd, **_kwargs):
        calls.append(list(cmd))
        rc = 1 if "process_sentinel.py" in cmd[1] else 0
        return SimpleNamespace(returncode=rc)

    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    rc = module.main(["--apply", "--kill-processes"])

    assert rc == 0
    sentinel_path = Path(calls[0][1])
    assert sentinel_path.parts[-2:] == ("tools", "process_sentinel.py")
    assert calls[1] == module.build_git_clean_command(
        apply=True,
        pathspecs=module.default_pathspecs(),
    )


def test_run_process_sentinel_uses_json_when_capturing(monkeypatch, tmp_path) -> None:
    module = _load_artifact_cleanup()
    calls: list[list[str]] = []

    def fake_guarded_completed_process(cmd, **_kwargs):
        calls.append(list(cmd))
        return SimpleNamespace(returncode=0, stdout="", stderr="")

    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    module.run_process_sentinel(tmp_path, capture_output=True)

    assert calls
    sentinel_path = Path(calls[0][1])
    assert sentinel_path.parts[-2:] == ("tools", "process_sentinel.py")
    assert "--json" in calls[0]


def test_main_json_includes_sentinel_events_without_raw_logs(
    monkeypatch,
    capsys,
) -> None:
    module = _load_artifact_cleanup()

    sentinel_event = {
        "event": "process_sentinel_violation",
        "violation": {"pgid": 123, "process_samples": [{"pid": 123}]},
        "repro": {"pytest": {"current_test": "unit"}},
    }

    def fake_guarded_completed_process(cmd, **_kwargs):
        if "process_sentinel.py" in cmd[1]:
            return SimpleNamespace(
                returncode=1,
                stdout=module.json.dumps(sentinel_event) + "\n",
                stderr="sentinel text stderr\n",
            )
        return SimpleNamespace(
            returncode=0,
            stdout="Removing target/\n",
            stderr="git text stderr\n",
        )

    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    rc = module.main(["--apply", "--kill-processes", "--json"])

    assert rc == 0
    payload = json.loads(capsys.readouterr().out)
    assert payload["status"] == "ok"
    data = payload["data"]
    assert data["sentinel_returncode"] == 1
    assert data["sentinel_events"] == [sentinel_event]
    assert data["entries"] == [{"action": "removed", "path": "target/"}]
    assert "sentinel_stdout" not in data
    assert "sentinel_stderr" not in data


def test_main_json_rejects_malformed_sentinel_events_before_git_clean(
    monkeypatch,
    capsys,
) -> None:
    module = _load_artifact_cleanup()

    def fake_guarded_completed_process(cmd, **_kwargs):
        if "process_sentinel.py" in cmd[1]:
            return SimpleNamespace(
                returncode=1,
                stdout="{not-json}\n",
                stderr="sentinel text stderr\n",
            )
        raise AssertionError("git clean must not run with malformed sentinel JSON")

    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    rc = module.main(["--apply", "--kill-processes", "--json"])

    assert rc == 2
    payload = json.loads(capsys.readouterr().out)
    assert payload["status"] == "error"
    assert payload["data"]["sentinel_returncode"] == 1
    assert "malformed JSON" in payload["errors"][0]


def test_main_json_rejects_empty_sentinel_violation_before_git_clean(
    monkeypatch,
    capsys,
) -> None:
    module = _load_artifact_cleanup()

    def fake_guarded_completed_process(cmd, **_kwargs):
        if "process_sentinel.py" in cmd[1]:
            return SimpleNamespace(
                returncode=1,
                stdout="",
                stderr="traceback without sentinel JSON\n",
            )
        raise AssertionError("git clean must not run without sentinel events")

    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    rc = module.main(["--apply", "--kill-processes", "--json"])

    assert rc == 2
    payload = json.loads(capsys.readouterr().out)
    assert payload["status"] == "error"
    assert payload["data"]["sentinel_returncode"] == 1
    assert "no JSON events" in payload["errors"][0]
