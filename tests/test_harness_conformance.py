from __future__ import annotations

import importlib
import json
from pathlib import Path


def _load_module():
    return importlib.import_module("molt.harness_conformance")


def test_build_env_sets_canonical_roots_and_session_id(tmp_path: Path) -> None:
    module = _load_module()

    env = module.build_molt_conformance_env(tmp_path, "smoke-suite")

    assert env["MOLT_EXT_ROOT"] == str(tmp_path)
    assert env["CARGO_TARGET_DIR"] == str(tmp_path / "target")
    assert env["MOLT_DIFF_CARGO_TARGET_DIR"] == str(tmp_path / "target")
    assert env["MOLT_CACHE"] == str(tmp_path / ".molt_cache")
    assert env["MOLT_DIFF_ROOT"] == str(tmp_path / "tmp" / "diff")
    assert env["MOLT_DIFF_TMPDIR"] == str(tmp_path / "tmp")
    assert env["TMPDIR"] == str(tmp_path / "tmp")
    assert env["PYTHONPATH"] == str(tmp_path / "src")
    assert env["MOLT_SESSION_ID"] == "smoke-suite"


def test_ensure_dirs_creates_expected_paths(tmp_path: Path) -> None:
    module = _load_module()
    env = module.build_molt_conformance_env(tmp_path, "suite")

    module.ensure_molt_conformance_dirs(env)

    for key in (
        "CARGO_TARGET_DIR",
        "MOLT_CACHE",
        "MOLT_DIFF_ROOT",
        "MOLT_DIFF_TMPDIR",
        "TMPDIR",
    ):
        assert Path(env[key]).is_dir(), key


def test_smoke_suite_manifest_preserves_order_and_ignores_comments(
    tmp_path: Path,
) -> None:
    module = _load_module()
    corpus_dir = tmp_path / "tests" / "harness" / "corpus" / "monty_compat"
    corpus_dir.mkdir(parents=True)
    first = corpus_dir / "alpha.py"
    second = corpus_dir / "beta.py"
    third = corpus_dir / "gamma.py"
    for path in (first, second, third):
        path.write_text("print('ok')\n", encoding="utf-8")
    smoke_manifest = corpus_dir / "SMOKE.txt"
    smoke_manifest.write_text(
        "\n".join(
            [
                "# keep comments",
                "",
                "beta.py",
                "alpha.py",
                "",
                "# trailing comment",
            ]
        ),
        encoding="utf-8",
    )

    suite = module.load_molt_conformance_suite(corpus_dir, "smoke", smoke_manifest)

    assert suite == [second, first]


def test_full_suite_uses_sorted_corpus(tmp_path: Path) -> None:
    module = _load_module()
    corpus_dir = tmp_path / "tests" / "harness" / "corpus" / "monty_compat"
    corpus_dir.mkdir(parents=True)
    for name in ("zeta.py", "alpha.py", "mid.py"):
        (corpus_dir / name).write_text("print('ok')\n", encoding="utf-8")
    smoke_manifest = corpus_dir / "SMOKE.txt"
    smoke_manifest.write_text("alpha.py\n", encoding="utf-8")

    suite = module.load_molt_conformance_suite(corpus_dir, "full", smoke_manifest)

    assert [path.name for path in suite] == ["alpha.py", "mid.py", "zeta.py"]


def test_write_summary_and_exit_code_contract(tmp_path: Path) -> None:
    module = _load_module()
    summary_path = tmp_path / "logs" / "conformance" / "smoke.json"
    summary = {
        "suite": "smoke",
        "manifest_path": "tests/harness/corpus/monty_compat/SMOKE.txt",
        "corpus_root": "tests/harness/corpus/monty_compat",
        "duration_s": 1.25,
        "total": 4,
        "passed": 2,
        "failed": 1,
        "compile_error": 0,
        "timeout": 0,
        "skipped": 1,
        "failures": [{"path": "beta.py", "detail": "expected exit 0"}],
        "compile_errors": [],
        "timeouts": [],
    }

    module.write_molt_conformance_summary(summary_path, summary)

    assert json.loads(summary_path.read_text(encoding="utf-8")) == summary
    assert module.conformance_exit_code(summary) == 1
    assert (
        module.conformance_exit_code(
            {
                **summary,
                "failed": 0,
                "compile_error": 1,
                "failures": [],
            }
        )
        == 1
    )
    assert (
        module.conformance_exit_code(
            {
                **summary,
                "failed": 0,
                "compile_error": 0,
                "timeout": 1,
                "failures": [],
            }
        )
        == 1
    )
    assert (
        module.conformance_exit_code(
            {
                **summary,
                "failed": 0,
                "compile_error": 0,
                "timeout": 0,
                "failures": [],
                "skipped": 2,
                "passed": 2,
            }
        )
        == 0
    )
