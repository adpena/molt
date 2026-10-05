from pathlib import Path

import pytest

from tools import check_type_coverage_todos as check


TYPE_RECORD = (
    "TODO(type-coverage, owner:runtime, milestone:TC3, priority:P2, status:partial): "
    "character stores after release."
)
STDLIB_RECORD = (
    "TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:missing): "
    "buffer protocol parity."
)


def _corpus(root: Path) -> None:
    for path, record in zip(check.MATRICES.values(), [TYPE_RECORD, STDLIB_RECORD]):
        target = root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(f"- {record}\n", encoding="utf-8")
    (root / "ROADMAP.md").write_text(
        "\n".join(
            f"[Canonical matrix](./{path.as_posix()}#gaps)"
            for path in check.MATRICES.values()
        ),
        encoding="utf-8",
    )


def test_matrices_own_records_without_roadmap_duplicates(tmp_path: Path) -> None:
    _corpus(tmp_path)
    assert check.main(tmp_path) == 0


def test_roadmap_duplicate_must_match_description(tmp_path: Path) -> None:
    _corpus(tmp_path)
    roadmap = tmp_path / "ROADMAP.md"
    links = roadmap.read_text(encoding="utf-8")
    roadmap.write_text(f"{links}\n- {TYPE_RECORD}\n", encoding="utf-8")
    assert check.main(tmp_path) == 0
    stale = TYPE_RECORD.replace("character stores after release.", "old unrelated gap.")
    roadmap.write_text(f"{links}\n- {stale}\n", encoding="utf-8")
    assert check.main(tmp_path) == 1


@pytest.mark.parametrize(
    "missing", ["type-coverage", "stdlib-compat", "roadmap", "link", "records"]
)
def test_required_inputs_fail_closed(tmp_path: Path, missing: str) -> None:
    _corpus(tmp_path)
    if missing in check.MATRICES:
        (tmp_path / check.MATRICES[missing]).unlink()
    elif missing == "roadmap":
        (tmp_path / "ROADMAP.md").unlink()
    elif missing == "link":
        (tmp_path / "ROADMAP.md").write_text(
            "Matrix mentioned without link.\n", encoding="utf-8"
        )
    else:
        (tmp_path / check.MATRICES["type-coverage"]).write_text(
            "No gap records.\n", encoding="utf-8"
        )
    assert check.main(tmp_path) == 1


def test_wrapped_records_preserve_description_and_ignore_syntax_examples() -> None:
    text = f"| status | ({TYPE_RECORD}) |\nSyntax: `TODO(stdlib-compat, ...)`."
    assert check._extract_todos(text) == {TYPE_RECORD}
    reordered = TYPE_RECORD.replace(
        "owner:runtime, milestone:TC3", "milestone:TC3, owner:runtime"
    )
    assert check._extract_todos(reordered) == {TYPE_RECORD}


@pytest.mark.parametrize(
    "record",
    [
        TYPE_RECORD.replace("owner:runtime, ", ""),
        TYPE_RECORD.replace("priority:P2", "priority:P9"),
        TYPE_RECORD.replace("status:partial", "status:"),
        TYPE_RECORD.replace("status:partial", "status:partial, owner:other"),
        TYPE_RECORD.replace("character stores after release.", ""),
        TYPE_RECORD.replace("): character stores after release.", ""),
        f"({TYPE_RECORD}",
        "`TODO(type-coverage, owner:runtime)`",
    ],
)
def test_malformed_records_fail_closed(record: str) -> None:
    with pytest.raises(ValueError):
        check._extract_todos(record)


@pytest.mark.parametrize("suffix", [")", ")."])
def test_prose_wrapped_record_accepts_sentence_punctuation(suffix: str) -> None:
    # Canonical matrix prose wraps records after other parenthesized content.
    line = f"- Runtime/IR: buffer interop (future) ({STDLIB_RECORD}{suffix}"
    assert check._extract_todos(line) == {STDLIB_RECORD}
