from __future__ import annotations

import json
import re
from pathlib import Path

from molt.debug.contracts import (
    DebugFailureClass,
    DebugStatus,
    DebugSubcommand,
    normalize_debug_payload,
)
from molt.debug.manifest import (
    allocate_debug_paths,
    canonical_debug_root,
    new_debug_run_id,
    render_debug_json_summary,
    render_debug_text_summary,
    write_debug_manifest,
)


def test_canonical_debug_roots(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.chdir(tmp_path)
    tmp_root = canonical_debug_root(retained=False)
    assert tmp_root.name == "debug"
    assert tmp_root.parent.name == "tmp"
    assert tmp_root.parent.parent.samefile(tmp_path)

    logs_root = canonical_debug_root(retained=True)
    assert logs_root.name == "debug"
    assert logs_root.parent.name == "logs"
    assert logs_root.parent.parent.samefile(tmp_path)


def test_new_debug_run_id_shape() -> None:
    run_id = new_debug_run_id()
    assert re.fullmatch(r"\d{8}T\d{6}Z-[0-9a-f]{12}", run_id)


def test_allocate_debug_paths_default_to_tmp_debug(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.chdir(tmp_path)
    paths = allocate_debug_paths(DebugSubcommand.IR)

    assert paths.retained_output is None
    assert paths.artifact_root.name == paths.run_id
    assert paths.artifact_root.parent.name == "ir"
    assert paths.artifact_root.parent.parent.name == "debug"
    assert paths.artifact_root.parent.parent.parent.name == "tmp"
    assert paths.artifact_root.parent.parent.parent.parent.samefile(tmp_path)
    assert paths.manifest_path == paths.artifact_root / "manifest.json"


def test_allocate_debug_paths_with_out_uses_logs_debug(
    tmp_path: Path, monkeypatch
) -> None:
    monkeypatch.chdir(tmp_path)
    paths = allocate_debug_paths(
        DebugSubcommand.VERIFY,
        out=tmp_path / "custom" / "result.json",
        output_extension="json",
    )

    assert paths.retained_output is not None
    assert paths.artifact_root.name == paths.run_id
    assert paths.artifact_root.parent.name == "verify"
    assert paths.artifact_root.parent.parent.name == "debug"
    assert paths.artifact_root.parent.parent.parent.name == "logs"
    assert paths.artifact_root.parent.parent.parent.parent.samefile(tmp_path)
    assert paths.retained_output.parent == paths.artifact_root
    assert paths.retained_output.suffix == ".json"


def test_manifest_write_and_summary_helpers(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.chdir(tmp_path)
    paths = allocate_debug_paths(DebugSubcommand.DIFF)

    payload = normalize_debug_payload(
        subcommand=DebugSubcommand.DIFF,
        status=DebugStatus.UNSUPPORTED,
        run_id=paths.run_id,
        artifact_root=paths.artifact_root,
        manifest_path=paths.manifest_path,
        selectors={"backend": "native"},
        failure_class=DebugFailureClass.NOT_YET_WIRED,
        message="debug diff is not yet wired",
    )
    manifest_path = write_debug_manifest(paths.manifest_path, payload)

    assert manifest_path.is_file()
    loaded = json.loads(manifest_path.read_text(encoding="utf-8"))
    assert loaded["subcommand"] == "diff"
    assert loaded["status"] == "unsupported"
    assert loaded["dimensions"]["python_tag"].startswith("py3")
    assert loaded["dimensions"]["host_os"]

    text_summary = render_debug_text_summary(loaded)
    assert "Status: unsupported" in text_summary
    assert "Manifest:" in text_summary

    json_summary = render_debug_json_summary(loaded)
    assert json.loads(json_summary)["run_id"] == paths.run_id
