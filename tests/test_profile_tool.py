from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
PROFILE_TOOL_PATH = REPO_ROOT / "tools" / "profile.py"
PROFILE_TOOL_SPEC = importlib.util.spec_from_file_location(
    "profile_tool_under_test", PROFILE_TOOL_PATH
)
assert PROFILE_TOOL_SPEC is not None and PROFILE_TOOL_SPEC.loader is not None
profile_tool = importlib.util.module_from_spec(PROFILE_TOOL_SPEC)
sys.modules[PROFILE_TOOL_SPEC.name] = profile_tool
PROFILE_TOOL_SPEC.loader.exec_module(profile_tool)


def test_profile_tool_parses_runtime_json_profile() -> None:
    payload = {
        "profile": {
            "call_dispatch": 7,
            "alloc_count": 11,
            "alloc_string": 3,
            "alloc_tuple": 2,
            "alloc_dict": 1,
        },
        "hot_paths": {"dict_str_int_prehash_hit": 5},
        "deopt_reasons": {"guard_tag_type_mismatch": 4},
        "memory": {"peak_rss_bytes": 1234},
    }
    log_text = "noise\nmolt_profile_json " + json.dumps(payload) + "\n"

    profile = profile_tool._parse_molt_profile_json(log_text)

    assert profile == {
        "call_dispatch": 7,
        "alloc_count": 11,
        "alloc_string": 3,
        "alloc_tuple": 2,
        "alloc_dict": 1,
        "dict_str_int_prehash_hit": 5,
        "guard_tag_type_mismatch": 4,
        "peak_rss_bytes": 1234,
        "string_allocs": 3,
        "tuple_allocs": 2,
        "dict_allocs": 1,
    }


def test_profile_tool_prefers_json_over_legacy_text(tmp_path: Path) -> None:
    log_path = tmp_path / "run.log"
    log_path.write_text(
        "\n".join(
            [
                "molt_profile call_dispatch=1 alloc_count=2",
                "molt_profile_json "
                + json.dumps(
                    {
                        "profile": {"call_dispatch": 9, "alloc_count": 10},
                        "hot_paths": {"call_bind_ic_hit": 8},
                        "deopt_reasons": {},
                    }
                ),
            ]
        )
        + "\n",
        encoding="utf-8",
    )

    metrics = profile_tool._merge_profile_metrics({}, log_path, True)

    assert metrics["molt_profile"]["call_dispatch"] == 9
    assert metrics["molt_profile"]["alloc_count"] == 10
    assert metrics["molt_profile"]["call_bind_ic_hit"] == 8
