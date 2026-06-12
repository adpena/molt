from __future__ import annotations

import importlib.util
import stat
from pathlib import Path

import pytest


REPO_ROOT = Path(__file__).resolve().parents[2]
ARTIFACT_PUBLISH = REPO_ROOT / "tools" / "artifact_publish.py"


def _load_artifact_publish():
    spec = importlib.util.spec_from_file_location(
        "molt_tools_artifact_publish", ARTIFACT_PUBLISH
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_publish_validated_outputs_replaces_only_complete_staged_set(
    tmp_path: Path,
) -> None:
    module = _load_artifact_publish()
    final_a = tmp_path / "final-a.bin"
    final_b = tmp_path / "nested" / "final-b.bin"
    staged_a = tmp_path / ".final-a.staged"
    staged_b = tmp_path / ".final-b.staged"
    final_a.write_bytes(b"old-a")
    final_b.parent.mkdir()
    final_b.write_bytes(b"old-b")
    staged_a.write_bytes(b"new-a")
    staged_b.write_bytes(b"new-b")

    module.publish_validated_outputs([(staged_a, final_a), (staged_b, final_b)])

    assert final_a.read_bytes() == b"new-a"
    assert final_b.read_bytes() == b"new-b"
    assert not staged_a.exists()
    assert not staged_b.exists()


def test_publish_validated_outputs_missing_staged_preserves_old_final_bytes(
    tmp_path: Path,
) -> None:
    module = _load_artifact_publish()
    final_a = tmp_path / "final-a.bin"
    final_b = tmp_path / "final-b.bin"
    staged_a = tmp_path / ".final-a.staged"
    missing_staged_b = tmp_path / ".final-b.staged"
    final_a.write_bytes(b"old-a")
    final_b.write_bytes(b"old-b")
    staged_a.write_bytes(b"new-a")

    with pytest.raises(FileNotFoundError, match="staged artifact missing"):
        module.publish_validated_outputs(
            [(staged_a, final_a), (missing_staged_b, final_b)]
        )

    assert final_a.read_bytes() == b"old-a"
    assert final_b.read_bytes() == b"old-b"


def test_publish_validated_outputs_rolls_back_after_partial_replace_failure(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module = _load_artifact_publish()
    final_a = tmp_path / "final-a.bin"
    final_b = tmp_path / "final-b.bin"
    staged_a = tmp_path / ".final-a.staged"
    staged_b = tmp_path / ".final-b.staged"
    final_a.write_bytes(b"old-a")
    final_b.write_bytes(b"old-b")
    staged_a.write_bytes(b"new-a")
    staged_b.write_bytes(b"new-b")
    original_replace = module.os.replace
    first_final_replaced = False

    def failing_replace(src: Path, dst: Path) -> None:
        nonlocal first_final_replaced
        src_path = Path(src)
        dst_path = Path(dst)
        if src_path == staged_b and dst_path == final_b:
            assert first_final_replaced
            raise OSError("simulated second final replace failure")
        original_replace(src, dst)
        if src_path == staged_a and dst_path == final_a:
            first_final_replaced = True

    monkeypatch.setattr(module.os, "replace", failing_replace)

    with pytest.raises(OSError, match="simulated second final replace failure"):
        module.publish_validated_outputs([(staged_a, final_a), (staged_b, final_b)])

    assert final_a.read_bytes() == b"old-a"
    assert final_b.read_bytes() == b"old-b"
    assert not list(tmp_path.glob(".*.old"))


def test_atomic_copy_file_copies_bytes_and_mode_through_publish_path(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module = _load_artifact_publish()
    src = tmp_path / "source.bin"
    dst = tmp_path / "out" / "final.bin"
    src.write_bytes(b"\x00molt-bytes\xff")
    src.chmod(0o640)
    original_publish = module.publish_validated_outputs
    published_pairs: list[list[tuple[Path, Path]]] = []

    def spy_publish(pairs: list[tuple[Path, Path]]) -> None:
        normalized = [(Path(staged), Path(final)) for staged, final in pairs]
        published_pairs.append(normalized)
        original_publish(pairs)

    monkeypatch.setattr(module, "publish_validated_outputs", spy_publish)

    module.atomic_copy_file(src, dst)

    assert dst.read_bytes() == b"\x00molt-bytes\xff"
    assert stat.S_IMODE(dst.stat().st_mode) == 0o640
    assert len(published_pairs) == 1
    assert len(published_pairs[0]) == 1
    staged, final = published_pairs[0][0]
    assert final == dst
    assert staged.parent == dst.parent
    assert staged != dst
    assert not staged.exists()


def test_atomic_write_bytes_preserves_existing_bytes_when_replace_fails(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module = _load_artifact_publish()
    final = tmp_path / "artifact.bin"
    final.write_bytes(b"old-bytes")
    original_replace = module.os.replace

    def failing_final_replace(src: Path, dst: Path) -> None:
        src_path = Path(src)
        dst_path = Path(dst)
        if src_path.suffix == ".tmp" and dst_path == final:
            raise OSError("simulated atomic write replace failure")
        original_replace(src, dst)

    monkeypatch.setattr(module.os, "replace", failing_final_replace)

    with pytest.raises(OSError, match="simulated atomic write replace failure"):
        module.atomic_write_bytes(final, b"new-bytes")

    assert final.read_bytes() == b"old-bytes"
    assert not list(tmp_path.glob(".*.tmp"))
    assert not list(tmp_path.glob(".*.old"))


def test_atomic_write_text_encodes_with_requested_encoding(tmp_path: Path) -> None:
    module = _load_artifact_publish()
    final = tmp_path / "artifact.txt"

    module.atomic_write_text(final, "alpha\nbeta", encoding="utf-16le")

    assert final.read_bytes() == "alpha\nbeta".encode("utf-16le")


def test_atomic_write_json_writes_sorted_indented_text_with_trailing_newline(
    tmp_path: Path,
) -> None:
    module = _load_artifact_publish()
    final = tmp_path / "payload.json"

    module.atomic_write_json(
        final,
        {"z": 2, "a": {"c": 3, "b": 1}},
        indent=4,
        sort_keys=True,
    )

    assert final.read_text(encoding="utf-8") == (
        "{\n"
        '    "a": {\n'
        '        "b": 1,\n'
        '        "c": 3\n'
        "    },\n"
        '    "z": 2\n'
        "}\n"
    )
