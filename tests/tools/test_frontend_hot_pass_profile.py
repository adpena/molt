from __future__ import annotations

import importlib.util
import sys
from pathlib import Path


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
    assert report["status_counts"] == {"pass": 1}
    assert report["ranked_midend_passes"], report
    assert report["ranked_frontend_functions"], report
    assert report["sources"][0]["op_count"] > 0
