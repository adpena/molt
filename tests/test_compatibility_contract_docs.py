from __future__ import annotations

from pathlib import Path
import shlex

from markdown_it import MarkdownIt
import pytest


REPO_ROOT = Path(__file__).resolve().parents[1]


def _read_text(rel_path: str) -> str:
    return (REPO_ROOT / rel_path).read_text(encoding="utf-8")


def test_readme_declares_parity_target_and_no_host_python_fallback() -> None:
    text = _read_text("README.md")
    assert "CPython `>=3.12` parity target" in text
    assert "host Python installation" in text
    assert "dynamic_execution_policy_contract.md" in text
    assert "parity remain incomplete" in text


def test_readme_links_to_getting_started_and_status_and_drops_internal_sections() -> (
    None
):
    text = _read_text("README.md")
    assert "docs/getting-started.md" in text
    assert "docs/spec/STATUS.md" in text
    assert "Optimization Program Kickoff" not in text
    assert "Capabilities (Current)" not in text
    assert "Limitations (Current)" not in text


def test_getting_started_exists_with_install_verify_and_first_run() -> None:
    text = _read_text("docs/getting-started.md")
    assert "molt doctor --json" in text
    assert "examples/hello.py" in text
    assert "Getting Started" in text


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
    assert "explicit, test-backed verified subset" in dynamic
    assert "not a claim that everything" in dynamic
    assert "mutation of global state after startup" not in breaks
    assert "runtime monkeypatching" in verified


def test_packaging_docs_call_out_standalone_binary_contract() -> None:
    packaging_readme = _read_text("packaging/README.md")
    install_doc = _read_text("packaging/INSTALL.md")

    assert "standalone artifacts" in packaging_readme
    assert "host Python installation" in packaging_readme
    assert "hidden host-CPython fallback" in packaging_readme
    assert "without any" in install_doc
    assert "host Python installation" in install_doc


@pytest.mark.parametrize("rel_path", ["README.md", "docs/getting-started.md"])
def test_source_quickstart_commands_use_uv_and_match_cli(rel_path: str) -> None:
    from molt.cli.entrypoint_parser import _build_entrypoint_parser

    parser = _build_entrypoint_parser()
    commands = []
    for block in MarkdownIt().parse(_read_text(rel_path)):
        if block.type != "fence" or block.info not in {"bash", "powershell"}:
            continue
        for line in block.content.splitlines():
            if line.lstrip().startswith(("./", ".\\")):
                continue
            words = shlex.split(line, comments=True)
            if not words:
                continue
            assert words[:1] == ["uv"], f"Unmanaged quickstart command: {line}"
            if words[1:2] == ["sync"]:
                assert words[-2:] == ["--python", "3.12"]
                continue
            assert words[:4] == ["uv", "run", "--python", "3.12"]
            payload = words[4:]
            if payload[:3] == ["python", "-m", "molt.cli"]:
                payload = payload[3:]
            else:
                assert payload[:1] == ["molt"]
                payload = payload[1:]
            commands.append(parser.parse_args(payload).command)
    assert {"doctor", "run", "compare"} <= set(commands)


def test_public_status_separates_implementation_from_acceptance() -> None:
    status = _read_text("docs/spec/STATUS.md")
    assert "## Reading Support Claims" in status
    assert "## Implemented Surfaces" in status
    assert "packaging/PACKAGING.md" in status
    deployment = _read_text("docs/deployment/PRODUCTION_STATUS.md")
    assert "(Historical)" in deployment
    assert "full parity across" not in deployment
