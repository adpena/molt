from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

import pytest


REPO_ROOT = Path(__file__).resolve().parents[2]
TOOL_PATH = REPO_ROOT / "tools" / "frontend_hot_pass_profile.py"


def _load_tool():
    spec = importlib.util.spec_from_file_location(
        "molt_tools_frontend_hot_pass_profile",
        TOOL_PATH,
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


@pytest.mark.parametrize("fail_on_error", [False, True])
def test_main_summary_reports_actual_exit_code(
    tmp_path: Path, capsys: pytest.CaptureFixture[str], fail_on_error: bool
) -> None:
    tool = _load_tool()
    source = tmp_path / "broken.py"
    source.write_text("def broken(\n", encoding="utf-8")
    args = ["--source", str(source), "--out-dir", str(tmp_path / "profile")]
    if fail_on_error:
        args.append("--fail-on-error")
    rc = tool.main(args)
    output = capsys.readouterr().out
    assert rc == int(fail_on_error)
    assert f"rc={rc} " in output
    assert "'error': 1" in output


def test_rank_passes_sorts_by_total_ms() -> None:
    tool = _load_tool()
    aggregate = {
        "cse": {
            "pass": "cse",
            "total_ms": 2.0,
            "max_ms": 1.5,
            "attempted": 2,
            "accepted": 1,
            "degraded": 0,
            "samples_ms": [0.5, 1.5],
            "functions": {"f"},
            "sources": {"a.py"},
        },
        "verifier": {
            "pass": "verifier",
            "total_ms": 5.0,
            "max_ms": 5.0,
            "attempted": 1,
            "accepted": 1,
            "degraded": 0,
            "samples_ms": [5.0],
            "functions": {"f"},
            "sources": {"a.py"},
        },
    }

    ranked = tool._rank_passes(aggregate, limit=2)

    assert [row["pass"] for row in ranked] == ["verifier", "cse"]
    assert ranked[0]["source_count"] == 1
    assert ranked[0]["attempted"] == 1


def test_profile_sources_emits_pass_and_cprofile_tables(tmp_path: Path) -> None:
    tool = _load_tool()
    source = tmp_path / "sample.py"
    source.write_text(
        "\n".join(
            [
                "def total(n):",
                "    acc = 0",
                "    for i in range(n):",
                "        acc += i",
                "    return acc",
                "print(total(8))",
            ]
        )
        + "\n",
        encoding="utf-8",
    )

    report = tool.profile_sources([source], optimization_profile="dev", top=5)

    assert report["source_count"] == 1
    assert report["schema_version"] == "1.2"
    assert report["status_counts"] == {"pass": 1}
    assert report["corpus_digest"]
    assert report["profile_inputs"]["resolved_sources"] == [tool._repo_rel(source)]
    assert report["ranked_midend_passes"], report
    assert report["ranked_frontend_functions"], report
    assert report["sources"][0]["op_count"] > 0


def test_corpus_digest_includes_ordered_source_hashes() -> None:
    tool = _load_tool()
    rows = [
        {"path": "a.py", "sha256": "111"},
        {"path": "b.py", "sha256": "222"},
    ]

    digest = tool._corpus_digest(rows)

    assert digest == tool._corpus_digest(rows)
    assert digest != tool._corpus_digest(list(reversed(rows)))
    assert digest != tool._corpus_digest([{"path": "a.py", "sha256": "999"}])


def test_version_excluded_source_never_reaches_parser_or_generator(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    tool = _load_tool()
    source = tmp_path / "future.py"
    source.write_text(
        "# MOLT_META: min_py=3.13\ntype Alias[T = int] = T\n", encoding="utf-8"
    )

    def forbidden(*args, **kwargs):
        raise AssertionError("version-excluded source reached compilation")

    monkeypatch.setattr(tool.ast, "parse", forbidden)
    monkeypatch.setattr(tool, "SimpleTIRGenerator", forbidden)
    report = tool.profile_sources(
        [source], optimization_profile="dev", top=3, target_python=(3, 12)
    )
    assert report["status_counts"] == {"skip": 1}
    assert report["source_count"] == 1
    assert report["target_python"] == "3.12"
    assert report["sources"][0]["exclusion_reason"] == "min_py 3.13"
    assert report["sources"][0]["sha256"]
    assert report["ranked_midend_passes"] == []
    markdown = tool.format_markdown(report)
    assert "## Excluded Sources" in markdown
    assert "## Errors" not in markdown


def test_profile_target_reaches_generator(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    tool = _load_tool()
    source = tmp_path / "simple.py"
    source.write_text("# MOLT_META: min_py=3.13\nvalue = 1\n", encoding="utf-8")
    original = tool.SimpleTIRGenerator
    seen = []

    def generator(**kwargs):
        seen.append(kwargs["target_python"])
        return original(**kwargs)

    monkeypatch.setattr(tool, "SimpleTIRGenerator", generator)
    result, _ = tool.profile_one(
        source, optimization_profile="dev", top_functions=3, target_python=(3, 13)
    )
    assert result["status"] == "pass", result
    assert seen == [(3, 13)]


def test_cli_version_skip_is_reported_without_fail_on_error_failure(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    tool = _load_tool()
    source = tmp_path / "future.py"
    source.write_text(
        "# MOLT_META: min_py=3.13\ntype Alias[T = int] = T\n", encoding="utf-8"
    )
    out_dir = tmp_path / "profile"
    rc = tool.main(
        [
            "--source",
            str(source),
            "--target-python",
            "3.12",
            "--out-dir",
            str(out_dir),
            "--fail-on-error",
        ]
    )
    report = json.loads(
        (out_dir / "frontend_hot_pass_profile.json").read_text(encoding="utf-8")
    )
    assert rc == 0
    assert report["schema_version"] == "1.2"
    assert report["status_counts"] == {"skip": 1}
    assert report["sources"][0]["exclusion_reason"] == "min_py 3.13"
    assert "rc=0 sources=1 statuses={'skip': 1}" in capsys.readouterr().out


def test_main_records_manifest_input_custody(tmp_path: Path) -> None:
    tool = _load_tool()
    source = tmp_path / "sample.py"
    source.write_text("def f():\n    return 1\nprint(f())\n", encoding="utf-8")
    manifest = tmp_path / "manifest.txt"
    manifest.write_text(f"{source}\n", encoding="utf-8")
    out_dir = tmp_path / "profile"

    rc = tool.main(
        [
            "--manifest",
            str(manifest),
            "--optimization-profile",
            "dev",
            "--out-dir",
            str(out_dir),
            "--top",
            "3",
            "--fail-on-error",
        ]
    )

    payload = json.loads(
        (out_dir / "frontend_hot_pass_profile.json").read_text(encoding="utf-8")
    )
    markdown = (out_dir / "frontend_hot_pass_profile.md").read_text(encoding="utf-8")
    assert rc == 0
    assert payload["schema_version"] == "1.2"
    assert payload["profile_inputs"]["manifest"]["path"] == tool._repo_rel(manifest)
    assert payload["profile_inputs"]["manifest"]["sha256"] == tool._sha256_text(
        manifest.read_text(encoding="utf-8")
    )
    assert payload["profile_inputs"]["source_args"] == []
    assert payload["profile_inputs"]["limit"] is None
    assert payload["profile_inputs"]["resolved_sources"] == [tool._repo_rel(source)]
    assert payload["corpus_digest"]
    assert f"- Corpus digest: {payload['corpus_digest']}" in markdown
