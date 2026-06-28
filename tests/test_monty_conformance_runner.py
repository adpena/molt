"""Tests for the Monty-through-Molt conformance runner."""

import json
import sys
from pathlib import Path

import molt.dx as molt_dx

sys.path.insert(0, "tests/harness")

import run_molt_conformance


class _FakeConformanceBatchCompiler:
    instances: list["_FakeConformanceBatchCompiler"] = []

    def __init__(
        self,
        molt_cmd: list[str],
        env: dict[str, str],
        *,
        repo_root: Path,
    ) -> None:
        self.molt_cmd = molt_cmd
        self.env = env
        self.repo_root = repo_root
        self.entered = False
        self.exited = False
        self.pinged = False
        _FakeConformanceBatchCompiler.instances.append(self)

    @property
    def command(self) -> list[str]:
        return [*self.molt_cmd, "internal-batch-build-server"]

    def __enter__(self) -> "_FakeConformanceBatchCompiler":
        self.entered = True
        return self

    def __exit__(self, exc_type: object, exc: object, tb: object) -> None:
        self.exited = True

    def ping(self) -> None:
        self.pinged = True


def _install_fake_batch_compiler(monkeypatch):
    _FakeConformanceBatchCompiler.instances.clear()
    monkeypatch.setattr(
        run_molt_conformance,
        "ConformanceBatchCompiler",
        _FakeConformanceBatchCompiler,
    )
    return _FakeConformanceBatchCompiler.instances


def test_find_molt_prefers_repo_checkout_cli(monkeypatch, tmp_path: Path):
    repo_root = tmp_path / "repo"
    cli_path = repo_root / "src" / "molt" / "cli" / "__init__.py"
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

    artifact_root = Path(env["MOLT_EXT_ROOT"])
    assert env["CARGO_TARGET_DIR"] == str(
        molt_dx.cargo_target_dir_for_artifact_root(artifact_root, "monty-conformance")
    )
    assert env["MOLT_DIFF_CARGO_TARGET_DIR"] == env["CARGO_TARGET_DIR"]
    assert env["MOLT_CACHE"] == str(artifact_root / ".molt_cache")
    assert env["MOLT_DIFF_ROOT"] == str(artifact_root / "tmp" / "diff")
    assert env["MOLT_DIFF_TMPDIR"] == str(artifact_root / "tmp")
    assert env["UV_CACHE_DIR"] == str(artifact_root / ".uv-cache")
    assert env["TMPDIR"] == str(artifact_root / "tmp")
    assert env["PYTHONPATH"] == str(repo_root.resolve() / "src")
    assert env["MOLT_SESSION_ID"] == "monty-conformance"


def test_molt_build_env_overrides_ambient_roots(monkeypatch):
    repo_root = Path("/tmp/molt-repo")
    monkeypatch.setenv("CARGO_TARGET_DIR", "/tmp/ambient-target")
    monkeypatch.setenv("TMPDIR", "/tmp/ambient-tmp")
    monkeypatch.setenv("PYTHONPATH", "/tmp/ambient-pythonpath")
    monkeypatch.setenv("MOLT_SESSION_ID", "ambient-session")
    monkeypatch.setenv("KEEP_ME", "1")

    env = run_molt_conformance._molt_build_env(repo_root)

    artifact_root = Path(env["MOLT_EXT_ROOT"])
    assert env["CARGO_TARGET_DIR"] == str(
        molt_dx.cargo_target_dir_for_artifact_root(artifact_root, "ambient-session")
    )
    assert env["TMPDIR"] == str(artifact_root / "tmp")
    assert env["PYTHONPATH"] == str(repo_root.resolve() / "src")
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


def test_batch_build_params_are_native_error_fallback_and_canonical_env():
    src = Path("/tmp/corpus/alpha.py")
    out = Path("/tmp/out/alpha")
    env = {
        "MOLT_EXT_ROOT": "/repo",
        "CARGO_TARGET_DIR": "/repo/target",
        "MOLT_DIFF_CARGO_TARGET_DIR": "/repo/target",
        "MOLT_CACHE": "/repo/.molt_cache",
        "MOLT_DIFF_ROOT": "/repo/tmp/diff",
        "MOLT_DIFF_TMPDIR": "/repo/tmp",
        "UV_CACHE_DIR": "/repo/.uv-cache",
        "TMPDIR": "/repo/tmp",
        "PYTHONPATH": "/repo/src",
        "MOLT_SESSION_ID": "conformance-test",
        "MOLT_CODEC": "json",
    }

    params = run_molt_conformance._molt_batch_build_params(src, out, env)

    assert params["file_path"] == str(src)
    assert params["output"] == str(out)
    assert params["target"] == "native"
    assert params["fallback"] == "error"
    assert params["trusted"] is False
    assert params["profile"] == "release"
    assert params["codec"] == "json"
    assert params["env_overrides"] == {
        key: env[key]
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
        )
    }


def test_conformance_batch_server_starts_in_guarded_process_group(monkeypatch):
    captured: dict[str, object] = {}

    class FakeClient:
        def __init__(self, cmd, **kwargs) -> None:
            captured["cmd"] = cmd
            captured.update(kwargs)

        def request(self, op: str, *, timeout: float, params=None):
            return {"ok": True, "pong": True, "id": 1, "op": op}

        def close(self, *, force: bool = False, timeout: float = 60.0) -> None:
            captured["closed"] = (force, timeout)

    monkeypatch.setattr(run_molt_conformance, "BatchCompileServerClient", FakeClient)
    compiler = run_molt_conformance.ConformanceBatchCompiler(
        ["molt"], {}, repo_root=Path("/tmp/repo")
    )

    compiler.start()
    compiler.close()

    guard_context = captured["guard_context"]
    assert guard_context.prefix == "MOLT_CONFORMANCE"
    assert guard_context.limits.enabled is True
    assert "process_group_kwargs" not in captured
    assert "force_close" not in captured


def test_run_binary_reports_guard_timeout_as_timeout(monkeypatch, tmp_path: Path):
    binary = tmp_path / "molt-bin"
    binary.write_text("binary", encoding="utf-8")

    def fake_guard(command, **kwargs):
        return run_molt_conformance.subprocess.CompletedProcess(
            command,
            run_molt_conformance.harness_memory_guard.memory_guard.TIMEOUT_RETURN_CODE,
            "",
            "memory_guard: timeout\n",
        )

    monkeypatch.setattr(
        run_molt_conformance.harness_memory_guard,
        "guarded_completed_process",
        fake_guard,
    )

    rc, stdout, stderr = run_molt_conformance.run_binary(binary)

    assert rc is None
    assert stdout == ""
    assert "timeout" in stderr


def test_compile_file_uses_batch_compiler_response(tmp_path: Path):
    src = tmp_path / "alpha.py"
    src.write_text("print('ok')\n", encoding="utf-8")
    out = tmp_path / "alpha_molt"
    calls: list[tuple[Path, Path, float]] = []

    class FakeCompiler:
        def build(self, build_src: Path, build_out: Path, *, timeout: float):
            calls.append((build_src, build_out, timeout))
            build_out.write_text("binary", encoding="utf-8")
            return {"ok": True, "returncode": 0}

    result = run_molt_conformance.compile_file(FakeCompiler(), src, out, timeout=4.5)

    assert result == run_molt_conformance.CompileResult(True)
    assert calls == [(src, out, 4.5)]


def test_compile_file_reports_batch_error_without_host_fallback(tmp_path: Path):
    src = tmp_path / "bad.py"
    out = tmp_path / "bad_molt"

    class FakeCompiler:
        def build(self, build_src: Path, build_out: Path, *, timeout: float):
            return {
                "ok": False,
                "returncode": 17,
                "stdout": "stdout detail",
                "stderr": "stderr detail",
                "error": "server detail",
            }

    result = run_molt_conformance.compile_file(FakeCompiler(), src, out)

    assert not result.ok
    assert not result.timed_out
    assert "compile failed (rc=17)" in result.detail
    assert "stderr detail" in result.detail
    assert "stdout detail" in result.detail
    assert "server detail" in result.detail
    assert not out.exists()


def test_compile_file_restarts_batch_server_after_timeout(tmp_path: Path):
    src = tmp_path / "slow.py"
    out = tmp_path / "slow_molt"
    restarted: list[bool] = []

    class FakeCompiler:
        def build(self, build_src: Path, build_out: Path, *, timeout: float):
            raise TimeoutError("slow build")

        def restart(self) -> None:
            restarted.append(True)

    result = run_molt_conformance.compile_file(FakeCompiler(), src, out, timeout=0.01)

    assert result == run_molt_conformance.CompileResult(
        False, "compile timeout", timed_out=True
    )
    assert restarted == [True]


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
    batch_instances = _install_fake_batch_compiler(monkeypatch)
    monkeypatch.setattr(
        run_molt_conformance,
        "preflight",
        lambda compiler, selected_files, tmpdir: True,
    )
    monkeypatch.setattr(
        run_molt_conformance,
        "compile_file",
        lambda compiler, src, out: run_molt_conformance.CompileResult(True),
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
    assert summary["manifest_path"] == smoke_manifest.as_posix()
    assert summary["corpus_root"] == corpus_dir.as_posix()
    assert summary["total"] == 1
    assert summary["passed"] == 1
    assert summary["failed"] == 0
    assert summary["compile_error"] == 0
    assert summary["timeout"] == 0
    assert summary["skipped"] == 0
    assert len(batch_instances) == 1
    assert batch_instances[0].molt_cmd == ["molt"]
    assert batch_instances[0].entered
    assert batch_instances[0].pinged
    assert batch_instances[0].exited


def test_main_preflight_honors_requested_suite_selection(tmp_path: Path, monkeypatch):
    corpus_dir = tmp_path / "corpus"
    corpus_dir.mkdir()
    (corpus_dir / "alpha.py").write_text("print('alpha')\n", encoding="utf-8")
    (corpus_dir / "beta.py").write_text("print('beta')\n", encoding="utf-8")
    smoke_manifest = corpus_dir / "SMOKE.txt"
    smoke_manifest.write_text("alpha.py\n", encoding="utf-8")
    captured: dict[str, object] = {}

    def fake_preflight(compiler, selected_files, tmpdir):
        captured["selected_files"] = selected_files
        return True

    monkeypatch.setattr(run_molt_conformance, "CORPUS_DIR", corpus_dir)
    monkeypatch.setattr(run_molt_conformance, "SMOKE_MANIFEST", smoke_manifest)
    monkeypatch.setattr(run_molt_conformance, "find_molt", lambda: "molt")
    _install_fake_batch_compiler(monkeypatch)
    monkeypatch.setattr(run_molt_conformance, "preflight", fake_preflight)
    monkeypatch.setattr(
        run_molt_conformance,
        "compile_file",
        lambda compiler, src, out: run_molt_conformance.CompileResult(True),
    )
    monkeypatch.setattr(run_molt_conformance, "run_binary", lambda binary: (0, "", ""))
    monkeypatch.setattr(
        run_molt_conformance, "parse_expectation", lambda filepath: ("success", "")
    )

    rc = run_molt_conformance.main(["--suite", "smoke"])

    assert rc == 0
    assert [path.name for path in captured["selected_files"]] == ["alpha.py"]
