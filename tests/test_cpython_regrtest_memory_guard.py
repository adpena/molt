from __future__ import annotations

import importlib.util
import io
import json
import subprocess
import sys
from pathlib import Path

import molt.dx as molt_dx


REPO_ROOT = Path(__file__).resolve().parents[1]
REGRTEST_TOOL_PATH = REPO_ROOT / "tools" / "cpython_regrtest.py"


def _load_regrtest_module():
    spec = importlib.util.spec_from_file_location(
        "cpython_regrtest_memory_guard_under_test", REGRTEST_TOOL_PATH
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_run_command_uses_memory_guard_and_preserves_log(monkeypatch) -> None:
    module = _load_regrtest_module()
    contexts: list[dict[str, object]] = []
    calls: list[dict[str, object]] = []

    class FakeContext:
        def run(self, command, **kwargs):
            calls.append({"command": command, **kwargs})
            return subprocess.CompletedProcess(command, 17, "stdout\n", "stderr\n")

    def fake_from_env(prefix, env, **kwargs):
        contexts.append({"prefix": prefix, "env": env, **kwargs})
        return FakeContext()

    def fail_direct_guard(command, **kwargs):
        calls.append({"command": command, **kwargs})
        raise AssertionError("regrtest must use HarnessExecutionContext")

    monkeypatch.setattr(
        module.harness_memory_guard,
        "HarnessExecutionContext",
        type(
            "FakeHarnessExecutionContext", (), {"from_env": staticmethod(fake_from_env)}
        ),
    )
    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_completed_process",
        fail_direct_guard,
    )
    log = io.StringIO()

    rc = module.run_command(
        ["python", "-c", "pass"],
        cwd=Path("/tmp"),
        env={"X": "1"},
        log_handle=log,
        dry_run=False,
    )

    assert rc == 17
    assert contexts[0]["prefix"] == "MOLT_REGRTEST"
    assert contexts[0]["repo_root"] == module.REPO_ROOT
    assert contexts[0]["env"]["X"] == "1"
    assert contexts[0]["env"]["MOLT_EXT_ROOT"] == str(module.REPO_ROOT)
    assert contexts[0]["env"]["CARGO_TARGET_DIR"] == str(
        molt_dx.cargo_target_dir_for_artifact_root(
            module.REPO_ROOT,
            contexts[0]["env"]["MOLT_SESSION_ID"],
        )
    )
    assert contexts[0]["env"]["TMPDIR"] == str(module.REPO_ROOT / "tmp")
    assert calls[0]["cwd"] == Path("/tmp")
    assert calls[0]["env"]["X"] == "1"
    text = log.getvalue()
    assert "cmd: python -c pass" in text
    assert "stdout" in text
    assert "stderr" in text


def test_build_env_canonicalizes_repo_local_artifact_roots(
    tmp_path: Path,
    monkeypatch,
) -> None:
    module = _load_regrtest_module()
    monkeypatch.setenv("MOLT_EXT_ROOT", "/ambient")
    monkeypatch.setenv("CARGO_TARGET_DIR", "/ambient/target")
    config = module.RegrtestConfig(
        repo_root=tmp_path,
        cpython_dir=tmp_path / "cpython",
        cpython_branch="v3.12.x",
        host_python="python",
        use_uv=False,
        uv_project=None,
        uv_python=[],
        uv_prepare=False,
        uv_add=[],
        molt_cmd=["python", "-m", "molt.cli", "run"],
        molt_capabilities=None,
        molt_shim_path=tmp_path / "shim.py",
        output_root=tmp_path,
        output_dir=tmp_path,
        skip_file=None,
        workers=1,
        rerun_failures=False,
        match=[],
        match_file=None,
        ignore=[],
        ignore_file=None,
        resources=[],
        timeout=None,
        junit_xml=tmp_path / "junit.xml",
        tests=[],
        regrtest_args=[],
        enable_coverage=False,
        coverage_source=[],
        coverage_dir=tmp_path / "coverage",
        stdlib_version="3.12",
        stdlib_source="sys",
        matrix_path=tmp_path / "matrix.md",
        matrix_format="json",
        type_matrix_path=tmp_path / "types.md",
        semantics_matrix_path=tmp_path / "semantics.md",
        diff_enabled=False,
        diff_paths=[],
        diff_python_version=None,
        core_only=False,
        core_file=tmp_path / "core.txt",
        property_tests=None,
        rust_coverage=False,
        rust_coverage_dir=tmp_path / "rust",
        dry_run=False,
        allow_clone=False,
    )

    env = module.build_env(config)

    assert env["MOLT_EXT_ROOT"] == str(tmp_path.resolve())
    assert env["CARGO_TARGET_DIR"] == str(
        molt_dx.cargo_target_dir_for_artifact_root(
            tmp_path.resolve(),
            env["MOLT_SESSION_ID"],
        )
    )
    assert env["MOLT_DIFF_CARGO_TARGET_DIR"] == env["CARGO_TARGET_DIR"]
    assert env["TMPDIR"] == str(tmp_path / "tmp")
    assert env["UV_CACHE_DIR"] == str(tmp_path / ".uv-cache")
    assert env["PYTHONHASHSEED"] == "0"


def test_write_summary_records_memory_guard(tmp_path: Path) -> None:
    module = _load_regrtest_module()
    config = module.RegrtestConfig(
        repo_root=tmp_path,
        cpython_dir=tmp_path / "cpython",
        cpython_branch="v3.12.x",
        host_python="python",
        use_uv=False,
        uv_project=None,
        uv_python=[],
        uv_prepare=False,
        uv_add=[],
        molt_cmd=["python", "-m", "molt.cli", "run"],
        molt_capabilities=None,
        molt_shim_path=tmp_path / "shim.py",
        output_root=tmp_path,
        output_dir=tmp_path,
        skip_file=None,
        workers=1,
        rerun_failures=False,
        match=[],
        match_file=None,
        ignore=[],
        ignore_file=None,
        resources=[],
        timeout=None,
        junit_xml=tmp_path / "junit.xml",
        tests=[],
        regrtest_args=[],
        enable_coverage=False,
        coverage_source=[],
        coverage_dir=tmp_path / "coverage",
        stdlib_version="3.12",
        stdlib_source="sys",
        matrix_path=tmp_path / "matrix.md",
        matrix_format="json",
        type_matrix_path=tmp_path / "types.md",
        semantics_matrix_path=tmp_path / "semantics.md",
        diff_enabled=False,
        diff_paths=[],
        diff_python_version=None,
        core_only=False,
        core_file=tmp_path / "core.txt",
        property_tests=None,
        rust_coverage=False,
        rust_coverage_dir=tmp_path / "rust",
        dry_run=False,
        allow_clone=False,
    )
    matrix_report = module.MatrixReport(
        json_path=tmp_path / "matrix.json",
        md_path=tmp_path / "matrix.md",
        summary={},
    )

    module.write_summary(
        config,
        summary=None,
        coverage=None,
        stdlib_paths=(None, None),
        python_version=None,
        returncode=0,
        diff_summary=None,
        matrix_report=matrix_report,
        rust_coverage=None,
        memory_guard={"enabled": True, "max_global_rss_gb": 4.0},
    )

    payload = json.loads((tmp_path / "summary.json").read_text(encoding="utf-8"))
    assert payload["memory_guard"] == {"enabled": True, "max_global_rss_gb": 4.0}
