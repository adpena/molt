"""Tests for the Monty-through-Molt conformance runner."""

import json
import sys
from pathlib import Path

sys.path.insert(0, "tests/harness")

import run_molt_conformance


def test_find_molt_prefers_repo_checkout_cli(monkeypatch, tmp_path: Path):
    repo_root = tmp_path / "repo"
    cli_path = repo_root / "src" / "molt" / "cli.py"
    cli_path.parent.mkdir(parents=True)
    cli_path.write_text("print('ok')\n", encoding="utf-8")

    monkeypatch.delenv("MOLT_BIN", raising=False)
    monkeypatch.setattr(run_molt_conformance, "SRC_ROOT", repo_root / "src")
    monkeypatch.setattr(run_molt_conformance.shutil, "which", lambda *_: None)

    assert run_molt_conformance.find_molt() == [sys.executable, "-m", "molt.cli"]


def test_find_molt_parses_molt_bin_override(monkeypatch):
    monkeypatch.setenv("MOLT_BIN", "custom-molt --flag")

    assert run_molt_conformance.find_molt() == ["custom-molt", "--flag"]


def test_molt_build_env_sets_canonical_defaults(monkeypatch):
    repo_root = Path("/tmp/molt-repo")
    for key in (
        "MOLT_EXT_ROOT",
        "CARGO_TARGET_DIR",
        "MOLT_DIFF_CARGO_TARGET_DIR",
        "MOLT_CACHE",
        "MOLT_DIFF_ROOT",
        "MOLT_DIFF_TMPDIR",
        "UV_CACHE_DIR",
        "TMPDIR",
        "PYTHONPATH",
        "MOLT_SESSION_ID",
    ):
        monkeypatch.delenv(key, raising=False)

    env = run_molt_conformance._molt_build_env(repo_root)

    assert env["MOLT_EXT_ROOT"] == str(repo_root)
    assert env["CARGO_TARGET_DIR"] == str(repo_root / "target")
    assert env["MOLT_DIFF_CARGO_TARGET_DIR"] == str(repo_root / "target")
    assert env["MOLT_CACHE"] == str(repo_root / ".molt_cache")
    assert env["MOLT_DIFF_ROOT"] == str(repo_root / "tmp" / "diff")
    assert env["MOLT_DIFF_TMPDIR"] == str(repo_root / "tmp")
    assert env["UV_CACHE_DIR"] == str(repo_root / ".uv-cache")
    assert env["TMPDIR"] == str(repo_root / "tmp")
    assert env["PYTHONPATH"] == str(repo_root / "src")
    assert env["MOLT_SESSION_ID"] == "monty-conformance"


def test_molt_build_env_overrides_ambient_roots(monkeypatch):
    repo_root = Path("/tmp/molt-repo")
    monkeypatch.setenv("CARGO_TARGET_DIR", "/tmp/ambient-target")
    monkeypatch.setenv("TMPDIR", "/tmp/ambient-tmp")
    monkeypatch.setenv("PYTHONPATH", "/tmp/ambient-pythonpath")
    monkeypatch.setenv("MOLT_SESSION_ID", "ambient-session")
    monkeypatch.setenv("KEEP_ME", "1")

    env = run_molt_conformance._molt_build_env(repo_root)

    assert env["CARGO_TARGET_DIR"] == str(repo_root / "target")
    assert env["TMPDIR"] == str(repo_root / "tmp")
    assert env["PYTHONPATH"] == str(repo_root / "src")
    assert env["MOLT_SESSION_ID"] == "ambient-session"
    assert env["KEEP_ME"] == "1"


def test_exit_code_fails_on_compile_errors_and_timeouts():
    assert (
        run_molt_conformance._exit_code_for_stats(
            run_molt_conformance.Stats(passed=12, failed=0, compile_error=1, timeout=0)
        )
        == 1
    )
    assert (
        run_molt_conformance._exit_code_for_stats(
            run_molt_conformance.Stats(passed=12, failed=0, compile_error=0, timeout=1)
        )
        == 1
    )
    assert (
        run_molt_conformance._exit_code_for_stats(
            run_molt_conformance.Stats(passed=12, failed=1, compile_error=0, timeout=0)
        )
        == 1
    )
    assert (
        run_molt_conformance._exit_code_for_stats(
            run_molt_conformance.Stats(passed=12, failed=0, compile_error=0, timeout=0)
        )
        == 0
    )


def test_selected_test_files_supports_smoke_and_full_suites(tmp_path: Path):
    corpus_dir = tmp_path / "corpus"
    corpus_dir.mkdir()
    for name in ("beta.py", "alpha.py", "gamma.py"):
        (corpus_dir / name).write_text("print('ok')\n", encoding="utf-8")
    smoke_manifest = corpus_dir / "SMOKE.txt"
    smoke_manifest.write_text("beta.py\nalpha.py\n", encoding="utf-8")

    smoke = run_molt_conformance._selected_test_files(
        suite="smoke",
        category="",
        limit=0,
        corpus_dir=corpus_dir,
        smoke_manifest=smoke_manifest,
    )
    full = run_molt_conformance._selected_test_files(
        suite="full",
        category="",
        limit=0,
        corpus_dir=corpus_dir,
        smoke_manifest=smoke_manifest,
    )

    assert [path.name for path in smoke] == ["beta.py", "alpha.py"]
    assert [path.name for path in full] == ["alpha.py", "beta.py", "gamma.py"]


def test_stats_summary_contains_required_fields():
    summary = run_molt_conformance._stats_to_summary(
        run_molt_conformance.Stats(
            passed=7,
            failed=1,
            compile_error=2,
            timeout=0,
            skipped=3,
            failures=[("bad.py", "expected exit 0")],
            compile_errors=[("cerr.py", "compile failed")],
            timeouts=[],
        ),
        suite="smoke",
        manifest_path=Path("tests/harness/corpus/monty_compat/SMOKE.txt"),
        corpus_root=Path("tests/harness/corpus/monty_compat"),
        duration_s=4.25,
    )

    assert summary == {
        "suite": "smoke",
        "manifest_path": "tests/harness/corpus/monty_compat/SMOKE.txt",
        "corpus_root": "tests/harness/corpus/monty_compat",
        "duration_s": 4.25,
        "total": 13,
        "passed": 7,
        "failed": 1,
        "compile_error": 2,
        "timeout": 0,
        "skipped": 3,
        "failures": [{"path": "bad.py", "detail": "expected exit 0"}],
        "compile_errors": [{"path": "cerr.py", "detail": "compile failed"}],
        "timeouts": [],
    }


def test_main_writes_json_summary_for_requested_suite(tmp_path: Path, monkeypatch):
    corpus_dir = tmp_path / "corpus"
    corpus_dir.mkdir()
    test_file = corpus_dir / "alpha.py"
    test_file.write_text("print('ok')\n", encoding="utf-8")
    smoke_manifest = corpus_dir / "SMOKE.txt"
    smoke_manifest.write_text("alpha.py\n", encoding="utf-8")
    summary_path = tmp_path / "logs" / "conformance" / "smoke.json"

    monkeypatch.setattr(run_molt_conformance, "CORPUS_DIR", corpus_dir)
    monkeypatch.setattr(run_molt_conformance, "SMOKE_MANIFEST", smoke_manifest)
    monkeypatch.setattr(run_molt_conformance, "find_molt", lambda: "molt")
    monkeypatch.setattr(
        run_molt_conformance, "preflight", lambda molt, selected_files, tmpdir: True
    )
    monkeypatch.setattr(
        run_molt_conformance, "compile_file", lambda molt, src, out: (True, "")
    )
    monkeypatch.setattr(run_molt_conformance, "run_binary", lambda binary: (0, "", ""))
    monkeypatch.setattr(
        run_molt_conformance, "parse_expectation", lambda filepath: ("success", "")
    )

    rc = run_molt_conformance.main(
        ["--suite", "smoke", "--json-out", str(summary_path)]
    )

    assert rc == 0
    summary = json.loads(summary_path.read_text(encoding="utf-8"))
    assert summary["suite"] == "smoke"
    assert summary["manifest_path"] == str(smoke_manifest)
    assert summary["corpus_root"] == str(corpus_dir)
    assert summary["total"] == 1
    assert summary["passed"] == 1
    assert summary["failed"] == 0
    assert summary["compile_error"] == 0
    assert summary["timeout"] == 0
    assert summary["skipped"] == 0


def test_main_preflight_honors_requested_suite_selection(tmp_path: Path, monkeypatch):
    corpus_dir = tmp_path / "corpus"
    corpus_dir.mkdir()
    (corpus_dir / "alpha.py").write_text("print('alpha')\n", encoding="utf-8")
    (corpus_dir / "beta.py").write_text("print('beta')\n", encoding="utf-8")
    smoke_manifest = corpus_dir / "SMOKE.txt"
    smoke_manifest.write_text("alpha.py\n", encoding="utf-8")
    captured: dict[str, object] = {}

    def fake_preflight(molt, selected_files, tmpdir):
        captured["selected_files"] = selected_files
        return True

    monkeypatch.setattr(run_molt_conformance, "CORPUS_DIR", corpus_dir)
    monkeypatch.setattr(run_molt_conformance, "SMOKE_MANIFEST", smoke_manifest)
    monkeypatch.setattr(run_molt_conformance, "find_molt", lambda: "molt")
    monkeypatch.setattr(run_molt_conformance, "preflight", fake_preflight)
    monkeypatch.setattr(
        run_molt_conformance, "compile_file", lambda molt, src, out: (True, "")
    )
    monkeypatch.setattr(run_molt_conformance, "run_binary", lambda binary: (0, "", ""))
    monkeypatch.setattr(
        run_molt_conformance, "parse_expectation", lambda filepath: ("success", "")
    )

    rc = run_molt_conformance.main(["--suite", "smoke"])

    assert rc == 0
    assert [path.name for path in captured["selected_files"]] == ["alpha.py"]
