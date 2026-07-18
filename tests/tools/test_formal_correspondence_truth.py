from __future__ import annotations

from pathlib import Path

from tools import check_correspondence


def _write_sources(tmp_path: Path, lean: str, rust: str) -> tuple[Path, Path]:
    lean_path = tmp_path / "LuauEmit.lean"
    rust_path = tmp_path / "luau.rs"
    lean_path.write_text(lean, encoding="utf-8")
    rust_path.write_text(rust, encoding="utf-8")
    return lean_path, rust_path


def test_formal_correspondence_fails_when_zero_builtin_mappings_parse(
    tmp_path: Path, monkeypatch
) -> None:
    lean, rust = _write_sources(tmp_path, "def unrelated := 1\n", "fn print() {}\n")
    backend = tmp_path / "backend"
    backend.mkdir()
    (backend / "builtins.rs").write_text(rust.read_text(), encoding="utf-8")
    monkeypatch.setattr(check_correspondence, "LUAU_EMIT_LEAN", lean)
    monkeypatch.setattr(check_correspondence, "LUAU_BACKEND_SRC", backend)

    result = check_correspondence.check_luau_builtins()
    receipt = check_correspondence.json_report([result])

    assert result.ok is False
    assert result.metrics == {"builtin_mappings_parsed": 0, "builtin_mappings_mapped": 0}
    assert receipt["status"] == "failure"
    assert receipt["zero_work"] is False
    assert receipt["categories"][0]["metrics"]["builtin_mappings_mapped"] == 0


def test_formal_correspondence_receipt_counts_parsed_and_mapped_builtins(
    tmp_path: Path, monkeypatch
) -> None:
    lean, _rust = _write_sources(
        tmp_path,
        'def builtinMapping := [("Print", "print"), ("Length", "len")]\n',
        "",
    )
    backend = tmp_path / "backend"
    backend.mkdir()
    (backend / "builtins.rs").write_text(
        "fn Print() { print(); }\nfn Length() { len(); }\n", encoding="utf-8"
    )
    monkeypatch.setattr(check_correspondence, "LUAU_EMIT_LEAN", lean)
    monkeypatch.setattr(check_correspondence, "LUAU_BACKEND_SRC", backend)

    result = check_correspondence.check_luau_builtins()
    receipt = check_correspondence.json_report([result])

    assert result.ok is True
    assert receipt["status"] == "success"
    assert result.metrics["builtin_mappings_parsed"] == 2
    assert result.metrics["builtin_mappings_mapped"] == 2
    assert receipt["categories"][0]["metrics"]["builtin_mappings_mapped"] == 2


def test_general_correspondence_receipt_rejects_no_categories() -> None:
    receipt = check_correspondence.json_report([])
    assert receipt["status"] == "failure"
    assert receipt["executed"] == 0
    assert receipt["zero_work"] is True


def test_correspondence_receipt_enforces_counted_invariants() -> None:
    categories = [
        check_correspondence.CategoryResult(
            category="one",
            description="passing category",
            items=[
                check_correspondence.CheckItem("a", True),
                check_correspondence.CheckItem("b", True),
            ],
        ),
        check_correspondence.CategoryResult(
            category="two",
            description="failing category",
            items=[check_correspondence.CheckItem("c", False)],
        ),
    ]

    receipt = check_correspondence.json_report(categories)
    assert receipt["selected"] == receipt["executed"] == 3
    assert receipt["executed"] == (
        receipt["passed"] + receipt["failed"] + receipt["errors"]
    )
    assert receipt["status"] == "failure"
    for category in receipt["categories"]:
        assert category["selected"] == category["executed"]
        assert category["executed"] == (
            category["passed"] + category["failed"] + category["errors"]
        )
        assert category["zero_work"] is False
