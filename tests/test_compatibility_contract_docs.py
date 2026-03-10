from __future__ import annotations

from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]


def _read_text(rel_path: str) -> str:
    return (REPO_ROOT / rel_path).read_text(encoding="utf-8")


def test_readme_declares_parity_target_and_no_host_python_fallback() -> None:
    text = _read_text("README.md")
    assert "CPython `>=3.12` parity target" in text
    assert "host Python installation" in text
    assert "runtime monkeypatching" in text


def test_status_and_roadmap_keep_same_core_contract() -> None:
    for rel_path in ("docs/spec/STATUS.md", "ROADMAP.md"):
        text = _read_text(rel_path)
        assert "full CPython `>=3.12`" in text
        assert "runtime monkeypatching" in text
        assert (
            "host CPython fallback" in text
            or "host Python installation" in text
            or "host-CPython fallback" in text
            or "host CPython runtime" in text
        )


def test_core_policy_docs_keep_carveouts_and_standalone_binary_story() -> None:
    vision = _read_text("docs/spec/areas/core/0000-vision.md")
    breaks = _read_text("docs/spec/areas/core/0800_WHAT_MOLT_IS_WILLING_TO_BREAK.md")
    fallback = _read_text(
        "docs/spec/areas/compat/contracts/compatibility_fallback_contract.md"
    )
    dynamic = _read_text(
        "docs/spec/areas/compat/contracts/dynamic_execution_policy_contract.md"
    )
    verified = _read_text(
        "docs/spec/areas/compat/contracts/verified_subset_contract.md"
    )

    assert "CPython `>=3.12` parity target" in vision
    assert "host-CPython fallback" in vision
    assert "unrestricted `eval`/`exec`" in breaks
    assert "host-Python fallback" in breaks
    assert "No host CPython in binaries" in fallback
    assert "full CPython `>=3.12` parity" in fallback
    assert "except for the carve-outs below" in dynamic
    assert "runtime monkeypatching" in verified


def test_packaging_docs_call_out_standalone_binary_contract() -> None:
    packaging_readme = _read_text("packaging/README.md")
    install_doc = _read_text("packaging/INSTALL.md")

    assert "standalone artifacts" in packaging_readme
    assert "host Python installation" in packaging_readme
    assert "hidden host-CPython fallback" in packaging_readme
    assert "without any" in install_doc
    assert "host Python installation" in install_doc
