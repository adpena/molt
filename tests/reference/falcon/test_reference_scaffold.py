from __future__ import annotations

import json
import subprocess
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[3]
PIN_FILE = REPO_ROOT / "bench/friends/reference/pins.toml"
MANIFEST_FILE = REPO_ROOT / "bench/results/reference_manifest.json"


def _run_tool(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["python3", *args],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=False,
    )


def test_reference_fetch_generates_manifest(tmp_path: Path) -> None:
    output = tmp_path / "reference_manifest.json"
    res = _run_tool(
        "tools/reference_fetch.py",
        "--pins",
        str(PIN_FILE),
        "--output",
        str(output),
        "--json",
    )
    assert res.returncode == 0, res.stderr
    assert output.exists()

    payload = json.loads(output.read_text(encoding="utf-8"))
    assert payload["schema_version"] == 1
    assert payload["models"][0]["id"] == "falcon-1"
    assert [lane["backend"] for lane in payload["lanes"]] == ["tinygrad", "tinygpu"]


def test_reference_compare_validates_manifest_and_pins() -> None:
    res = _run_tool(
        "tools/reference_compare.py",
        "--pins",
        str(PIN_FILE),
        "--manifest",
        str(MANIFEST_FILE),
        "--json",
    )
    assert res.returncode == 0, res.stderr
    payload = json.loads(res.stdout)
    assert payload["status"] == "ok"
    assert payload["model_ids"] == ["falcon-1"]


def test_bench_reference_reports_ready_lanes(tmp_path: Path) -> None:
    output = tmp_path / "reference_manifest.json"
    fetch = _run_tool(
        "tools/reference_fetch.py",
        "--pins",
        str(PIN_FILE),
        "--output",
        str(output),
    )
    assert fetch.returncode == 0, fetch.stderr

    res = _run_tool(
        "tools/bench_reference.py",
        "--manifest",
        str(output),
        "--json",
    )
    assert res.returncode == 0, res.stderr
    payload = json.loads(res.stdout)
    assert payload["status"] == "ok"
    assert [lane["id"] for lane in payload["lanes"]] == [
        "falcon-1-tinygrad",
        "falcon-1-tinygpu",
    ]


def test_bench_reference_rejects_missing_lane_path(tmp_path: Path) -> None:
    manifest = tmp_path / "broken_manifest.json"
    manifest.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "workspace": {
                    "name": "reference",
                    "root": "bench/friends/reference",
                },
                "models": [
                    {
                        "id": "falcon-1",
                        "family": "falcon",
                        "display_name": "Falcon #1",
                        "status": "scaffold",
                    }
                ],
                "lanes": [
                    {
                        "id": "falcon-1-tinygrad",
                        "model": "falcon-1",
                        "backend": "tinygrad",
                        "path": "bench/friends/reference/does-not-exist",
                        "enabled": True,
                    }
                ],
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )

    res = _run_tool(
        "tools/bench_reference.py",
        "--manifest",
        str(manifest),
        "--json",
    )
    assert res.returncode != 0
    assert "does-not-exist" in res.stderr
