from pathlib import Path
import subprocess
import tomllib


ROOT = Path(__file__).resolve().parents[1]
PROJECT_DOC_MAX_BYTES = 32_768
COMPACT_ROOT_BUDGET = 24 * 1024
DISCOVERABLE_AGENT_DOCS = {"AGENTS.md", "AGENTS.override.md"}
FULL_GUIDES = {
    "AGENTS.md": ROOT / "docs" / "agent" / "AGENTS.full.md",
    "CLAUDE.md": ROOT / "docs" / "agent" / "CLAUDE.full.md",
}


def _tracked_agent_docs() -> list[Path]:
    result = subprocess.run(
        ["git", "ls-files", "-z"],
        cwd=ROOT,
        check=True,
        stdout=subprocess.PIPE,
    )
    return sorted(
        ROOT / name.decode("utf-8")
        for name in result.stdout.split(b"\0")
        if name and Path(name.decode("utf-8")).name in DISCOVERABLE_AGENT_DOCS
    )


def _codex_agent_doc_chain_for(path: Path) -> list[Path]:
    dirs = list(path.relative_to(ROOT).parent.parents)
    dirs = [Path(".")] + list(reversed([d for d in dirs if str(d) != "."]))
    if path.parent != ROOT:
        dirs.append(path.relative_to(ROOT).parent)

    chain: list[Path] = []
    for rel_dir in dirs:
        directory = ROOT / rel_dir
        override = directory / "AGENTS.override.md"
        default = directory / "AGENTS.md"
        if override.exists():
            chain.append(override)
        elif default.exists():
            chain.append(default)
    return chain


def test_root_agent_contracts_fit_codex_project_doc_budget() -> None:
    for name in ("AGENTS.md", "CLAUDE.md"):
        path = ROOT / name
        payload = path.read_bytes()
        assert len(payload) <= COMPACT_ROOT_BUDGET, (
            f"{name} is {len(payload)} bytes; keep root agent contracts under "
            f"{COMPACT_ROOT_BUDGET} bytes so Codex has headroom below the "
            f"{PROJECT_DOC_MAX_BYTES}-byte project-doc default"
        )


def test_project_codex_config_pins_project_doc_loader_budget() -> None:
    config_path = ROOT / ".codex" / "config.toml"
    config = tomllib.loads(config_path.read_text(encoding="utf-8"))
    assert config["project_doc_max_bytes"] == PROJECT_DOC_MAX_BYTES
    assert config.get("project_doc_fallback_filenames") == []


def test_every_tracked_codex_agent_doc_chain_fits_loader_budget() -> None:
    for agent_doc in _tracked_agent_docs():
        chain = _codex_agent_doc_chain_for(agent_doc)
        payload_size = sum(path.stat().st_size for path in chain)
        pretty_chain = " -> ".join(
            path.relative_to(ROOT).as_posix() for path in chain
        )
        assert payload_size <= PROJECT_DOC_MAX_BYTES, (
            f"Codex instruction chain for {agent_doc.relative_to(ROOT).as_posix()} "
            f"is {payload_size} bytes, exceeding {PROJECT_DOC_MAX_BYTES}: "
            f"{pretty_chain}"
        )


def test_full_agent_guides_are_preserved_outside_root_contracts() -> None:
    for root_name, full_path in FULL_GUIDES.items():
        assert full_path.exists(), f"{full_path} must preserve the expanded {root_name}"
        assert full_path.stat().st_size > PROJECT_DOC_MAX_BYTES
        assert not full_path.read_bytes().startswith(b"\xef\xbb\xbf")
        root_text = (ROOT / root_name).read_text(encoding="utf-8")
        assert str(full_path.relative_to(ROOT)).replace("\\", "/") in root_text


def test_agent_contract_has_no_shadow_authority_files() -> None:
    shadow_paths = [
        ROOT / "AGENTS.override.md",
        ROOT / "docs" / "ops" / "CODEX_OPERATING_DOCTRINE.md",
    ]
    for shadow_path in shadow_paths:
        assert not shadow_path.exists(), f"{shadow_path} reintroduces agent-doc drift"


def test_agent_contract_blocks_unsupported_exec_interrupts() -> None:
    for name in ("AGENTS.md", "CLAUDE.md"):
        text = (ROOT / name).read_text(encoding="utf-8")
        assert "Ctrl-C" in text
        assert "unified exec backend" in text
        assert "incident record" in text
