from __future__ import annotations

from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]

PUBLIC_DEBUG_DOCS = (
    ROOT / "docs" / "OPERATIONS.md",
    ROOT / "docs" / "DEVELOPER_GUIDE.md",
    ROOT / "tests" / "translation_validation" / "README.md",
    ROOT
    / "docs"
    / "spec"
    / "areas"
    / "testing"
    / "0008_MINIMUM_MUST_PASS_MATRIX.md",
)

LEGACY_WRAPPERS = (
    "tools/ir_dump.py",
    "tools/profile_analyze.py",
    "tools/ir_probe_supervisor.py",
)


def test_public_debug_docs_prefer_canonical_cli_surface() -> None:
    for path in PUBLIC_DEBUG_DOCS:
        text = path.read_text(encoding="utf-8")
        for wrapper in LEGACY_WRAPPERS:
            assert wrapper not in text, f"{path} still documents legacy wrapper {wrapper}"


def test_public_debug_docs_reference_canonical_debug_commands() -> None:
    operations = (ROOT / "docs" / "OPERATIONS.md").read_text(encoding="utf-8")
    developer_guide = (ROOT / "docs" / "DEVELOPER_GUIDE.md").read_text(encoding="utf-8")
    translation_readme = (
        ROOT / "tests" / "translation_validation" / "README.md"
    ).read_text(encoding="utf-8")

    for text in (operations, developer_guide):
        assert "molt debug ir" in text
        assert "molt debug verify" in text

    assert "molt debug ir" in translation_readme
