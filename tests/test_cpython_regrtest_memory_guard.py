from __future__ import annotations

import importlib.util
import io
import json
import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace

import molt.dx as molt_dx
from molt import custody_layout
import pytest


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


def test_cpython_source_authority_is_commit_pinned() -> None:
    module = _load_regrtest_module()
    sources = module.load_cpython_sources()

    assert sources == {
        "3.12": module.CPythonSource(
            python="3.12",
            revision="3bb231a6a5dc02b95658877318bf61501a7209e9",
            tag="v3.12.13",
            git_url="https://github.com/python/cpython.git",
        ),
        "3.13": module.CPythonSource(
            python="3.13",
            revision="cbc944f4bc59639a444dd971c737788ba2283a91",
            tag="v3.13.16",
            git_url="https://github.com/python/cpython.git",
        ),
        "3.14": module.CPythonSource(
            python="3.14",
            revision="8e6e75d9102e39bed2a2b279203a396741180f12",
            tag="v3.14.8",
            git_url="https://github.com/python/cpython.git",
        ),
    }
    # The regrtest default is the first row.
    assert next(iter(sources)) == "3.12"


def test_existing_cpython_checkout_fails_closed_on_revision_drift(
    tmp_path: Path,
    monkeypatch,
) -> None:
    module = _load_regrtest_module()
    checkout = tmp_path / "cpython"
    checkout.mkdir()
    source = next(iter(module.load_cpython_sources().values()))
    monkeypatch.setattr(
        module,
        "COMMANDS",
        SimpleNamespace(
            run=lambda *_args, **_kwargs: subprocess.CompletedProcess(
                ["git"], 0, "f" * 40 + "\n", ""
            )
        ),
    )

    with pytest.raises(RuntimeError, match="source revision drift"):
        module.ensure_cpython_checkout(
            checkout,
            source,
            allow_clone=False,
            log_handle=io.StringIO(),
            dry_run=False,
        )


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
    artifact_root = molt_dx.checkout_custody(module.REPO_ROOT).custody_root
    assert contexts[0]["env"]["MOLT_EXT_ROOT"] == str(artifact_root)
    assert contexts[0]["env"]["CARGO_TARGET_DIR"] == str(
        molt_dx.cargo_target_dir_for_environment(artifact_root, contexts[0]["env"])
    )
    assert contexts[0]["env"]["TMPDIR"] == str(
        custody_layout.scratch_root(artifact_root, module.REPO_ROOT)
    )
    assert calls[0]["cwd"] == Path("/tmp")
    assert calls[0]["env"]["X"] == "1"
    text = log.getvalue()
    assert "cmd: python -c pass" in text
    assert "stdout" in text
    assert "stderr" in text


@pytest.mark.usefixtures("developer_host_context")
def test_build_env_canonicalizes_repo_local_artifact_roots(
    tmp_path: Path,
    monkeypatch,
) -> None:
    module = _load_regrtest_module()
    # Ambient roots must stay writable: the live pytest guard plugin also reads
    # this process environment while the test body runs.
    ambient = tmp_path / "ambient"
    monkeypatch.setenv("MOLT_EXT_ROOT", str(ambient))
    monkeypatch.setenv("CARGO_TARGET_DIR", str(ambient / "target"))
    config = module.RegrtestConfig(
        repo_root=tmp_path,
        cpython_dir=tmp_path / "cpython",
        cpython_source=module.CPythonSource(
            python="3.12",
            revision="0" * 40,
            tag="v3.12.13",
            git_url="https://github.com/python/cpython.git",
        ),
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
        molt_dx.cargo_target_dir_for_environment(
            tmp_path.resolve(),
            env,
        )
    )
    assert env["MOLT_DIFF_CARGO_TARGET_DIR"] == env["CARGO_TARGET_DIR"]
    assert env["TMPDIR"] == str(
        custody_layout.scratch_root(tmp_path.resolve(), tmp_path)
    )
    assert env["UV_CACHE_DIR"] == str(tmp_path / ".uv-cache")
    assert env["PYTHONHASHSEED"] == "0"


def test_write_summary_records_memory_guard(tmp_path: Path) -> None:
    module = _load_regrtest_module()
    config = module.RegrtestConfig(
        repo_root=tmp_path,
        cpython_dir=tmp_path / "cpython",
        cpython_source=module.CPythonSource(
            python="3.12",
            revision="0" * 40,
            tag="v3.12.13",
            git_url="https://github.com/python/cpython.git",
        ),
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
