from __future__ import annotations

import io
import json
import subprocess
from pathlib import Path
from types import SimpleNamespace

import molt.dx as molt_dx
from molt import custody_layout
from molt.checkout_state import scratch_state_root
import pytest

from tools import cpython_regrtest


REPO_ROOT = Path(__file__).resolve().parents[1]


def test_cpython_source_authority_is_commit_pinned() -> None:
    module = cpython_regrtest
    sources = module.load_cpython_sources()

    assert sources == {
        "3.12": module.CPythonSource(
            python="3.12",
            revision="e848e4b09ca4bc67c0041c9ce7526dae471d4c36",
            tag="v3.12.15",
            git_url="https://github.com/python/cpython.git",
        ),
        "3.13": module.CPythonSource(
            python="3.13",
            revision="64e080d38154391ff2d2caa1161c298bb7cd8f30",
            tag="v3.13.17",
            git_url="https://github.com/python/cpython.git",
        ),
        "3.14": module.CPythonSource(
            python="3.14",
            revision="ead82c07db1f111d613926ed1b6667411d768125",
            tag="v3.14.9",
            git_url="https://github.com/python/cpython.git",
        ),
        "3.15": module.CPythonSource(
            python="3.15",
            revision="c37e74cb6bcdadcd5f46f289498f6b3298ddee61",
            tag="v3.15.1",
            git_url="https://github.com/python/cpython.git",
        ),
    }
    # The regrtest default is the first row.
    assert next(iter(sources)) == "3.12"


@pytest.mark.parametrize(
    "mutation",
    [None, "version", "revision", "tag", "url", "duplicate"],
)
def test_pending_major_source_keeps_exact_pin_validation(tmp_path, mutation):
    from molt.target_python import _parse_target_python_version

    row = {
        "python": "4.0",
        "revision": "a" * 40,
        "tag": "v4.0.0",
        "git_url": "https://github.com/python/cpython.git",
    }
    if mutation == "version":
        row["python"] = "04.0"
    elif mutation == "revision":
        row["revision"] = "main"
    elif mutation == "tag":
        row["tag"] = "v4.1.0"
    elif mutation == "url":
        row["git_url"] = "https://example.invalid/cpython.git"
    text = ['schema = "molt.cpython-regrtest-sources.v1"']
    for _ in range(2 if mutation == "duplicate" else 1):
        text += [
            "",
            "[[source]]",
            *(f"{key} = {json.dumps(value)}" for key, value in row.items()),
        ]
    path = tmp_path / "sources.toml"
    path.write_text("\n".join(text) + "\n", encoding="utf-8")
    if mutation is not None:
        with pytest.raises(ValueError):
            cpython_regrtest.load_cpython_sources(path)
    else:
        assert cpython_regrtest.load_cpython_sources(path)["4.0"].revision == "a" * 40
        with pytest.raises(ValueError, match="unsupported Python target"):
            _parse_target_python_version("4.0")


@pytest.mark.parametrize(
    "version", ["4", "4.0.0", "4.0rc1", "4.0+local", "4.0.dev1", "v4.0", "1!4.0"]
)
def test_cpython_source_version_requires_canonical_major_minor(tmp_path, version):
    path = tmp_path / "sources.toml"
    path.write_text(
        'schema = "molt.cpython-regrtest-sources.v1"\n[[source]]\n'
        f'python = "{version}"\nrevision = "{"a" * 40}"\n'
        'tag = "v4.0.0"\ngit_url = "https://github.com/python/cpython.git"\n',
        encoding="utf-8",
    )
    with pytest.raises(ValueError, match="invalid CPython version authority"):
        cpython_regrtest.load_cpython_sources(path)


def test_existing_cpython_checkout_fails_closed_on_revision_drift(
    tmp_path: Path,
    monkeypatch,
) -> None:
    module = cpython_regrtest
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


def test_run_command_uses_memory_guard_and_preserves_log(
    monkeypatch, developer_host_context, unbound_checkout_family
) -> None:
    module = cpython_regrtest
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
        molt_dx.cargo_target_dir_for_environment(
            artifact_root, contexts[0]["env"], source_root=module.REPO_ROOT
        )
    )
    assert contexts[0]["env"]["TMPDIR"] == str(
        scratch_state_root(
            custody_layout.scratch_root(artifact_root, module.REPO_ROOT),
            owner=module.REPO_ROOT,
            create=False,
        )
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
    module = cpython_regrtest
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
        cpython_custody=module.GitCustody(),
        host_python="python",
        use_uv=False,
        uv_project=None,
        uv_python=[],
        uv_prepare=False,
        uv_add=[],
        molt_cmd=["python", "-m", "molt.cli", "run"],
        molt_capability_tier="full",
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
        admit_until=None,
        results_jsonl=None,
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
            tmp_path.resolve(), env, source_root=config.repo_root
        )
    )
    assert env["MOLT_DIFF_CARGO_TARGET_DIR"] == env["CARGO_TARGET_DIR"]
    assert env["TMPDIR"] == str(
        scratch_state_root(
            custody_layout.scratch_root(tmp_path.resolve(), tmp_path),
            owner=tmp_path,
            create=False,
        )
    )
    assert env["UV_CACHE_DIR"] == str(tmp_path / ".uv-cache")
    assert env["PYTHONHASHSEED"] == "0"


def test_write_summary_records_memory_guard(tmp_path: Path) -> None:
    module = cpython_regrtest
    config = module.RegrtestConfig(
        repo_root=tmp_path,
        cpython_dir=tmp_path / "cpython",
        cpython_source=module.CPythonSource(
            python="3.12",
            revision="0" * 40,
            tag="v3.12.13",
            git_url="https://github.com/python/cpython.git",
        ),
        cpython_custody=module.GitCustody(),
        host_python="python",
        use_uv=False,
        uv_project=None,
        uv_python=[],
        uv_prepare=False,
        uv_add=[],
        molt_cmd=["python", "-m", "molt.cli", "run"],
        molt_capability_tier="full",
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
        admit_until=None,
        results_jsonl=None,
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
