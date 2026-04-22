from __future__ import annotations

import importlib.util
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "tools" / "check_docs_architecture.py"


def _load_module():
    spec = importlib.util.spec_from_file_location(
        "check_docs_architecture_under_test", SCRIPT_PATH
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _write_file(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def _seed_valid_repo(root: Path) -> None:
    _write_file(
        root / "README.md",
        "\n".join(
            [
                "# Molt",
                "",
                "See [Getting Started](docs/getting-started.md) and [STATUS](docs/spec/STATUS.md).",
                "",
            ]
        ),
    )
    _write_file(root / "docs/getting-started.md", "# Getting Started\n")
    _write_file(
        root / "docs/spec/STATUS.md",
        "\n".join(
            [
                "# STATUS",
                "",
                "<!-- GENERATED:compat-summary:start -->",
                "- compat",
                "<!-- GENERATED:compat-summary:end -->",
                "",
                "<!-- GENERATED:bench-summary:start -->",
                "- bench",
                "<!-- GENERATED:bench-summary:end -->",
                "",
            ]
        ),
    )
    _write_file(root / "ROADMAP.md", "# Roadmap\n")
    _write_file(
        root / "SUPPORTED.md",
        "Pointer doc. See docs/spec/STATUS.md and docs/proofs/STANDALONE_BINARY_PROOF_WORKFLOW.md.\n",
    )
    _write_file(root / "docs/BENCHMARKING.md", "# Benchmarking\n")
    _write_file(root / "docs/ROADMAP_90_DAYS.md", "# 90 Day Roadmap\n")
    _write_file(
        root / "docs/proofs/STANDALONE_BINARY_PROOF_WORKFLOW.md", "# Proof Workflow\n"
    )
    _write_file(root / "AGENTS.md", "# Agents\n")


def test_checker_flags_banned_readme_sections(
    tmp_path: Path,
) -> None:
    module = _load_module()
    _seed_valid_repo(tmp_path)
    _write_file(
        tmp_path / "README.md",
        "\n".join(
            [
                "# Molt",
                "",
                "See [Getting Started](docs/getting-started.md) and [STATUS](docs/spec/STATUS.md).",
                "",
                "## Optimization Program Kickoff",
                "",
                "## Capabilities (Current)",
                "",
            ]
        ),
    )
    module.ROOT = tmp_path

    errors = module.check_repo()

    assert any("Optimization Program Kickoff" in error for error in errors)
    assert any("Capabilities (Current)" in error for error in errors)


def test_checker_requires_status_markers(
    tmp_path: Path,
) -> None:
    module = _load_module()
    _seed_valid_repo(tmp_path)
    _write_file(tmp_path / "docs/spec/STATUS.md", "# STATUS\n")
    module.ROOT = tmp_path

    errors = module.check_repo()

    assert any("compat-summary" in error for error in errors)
    assert any("bench-summary" in error for error in errors)


def test_checker_flags_stale_readme_and_supported_contract_patterns(
    tmp_path: Path,
) -> None:
    module = _load_module()
    _seed_valid_repo(tmp_path)
    _write_file(
        tmp_path / "ROADMAP.md",
        "Last updated: 2026-04-03\nCurrent Validation Note\n",
    )
    _write_file(
        tmp_path / "SUPPORTED.md",
        "This file is the operator-facing support contract for Molt.\nWhat Molt currently supports\n",
    )
    _write_file(
        tmp_path / "docs/BENCHMARKING.md",
        "Use --update-readme to refresh the Performance block in README.\n",
    )
    module.ROOT = tmp_path

    errors = module.check_repo()

    assert any("ROADMAP.md" in error and "Last updated:" in error for error in errors)
    assert any("Current Validation Note" in error for error in errors)
    assert any(
        "SUPPORTED.md" in error and "support contract" in error for error in errors
    )
    assert any("--update-readme" in error for error in errors)


def test_checker_flags_old_sync_language_in_agents_and_90_day_plan(
    tmp_path: Path,
) -> None:
    module = _load_module()
    _seed_valid_repo(tmp_path)
    _write_file(
        tmp_path / "AGENTS.md",
        "README and [ROADMAP.md](ROADMAP.md) are kept in sync.\n",
    )
    _write_file(
        tmp_path / "docs/ROADMAP_90_DAYS.md",
        "This plan must stay aligned with both docs/spec/STATUS.md and ROADMAP.md.\n",
    )
    module.ROOT = tmp_path

    errors = module.check_repo()

    assert any(
        "AGENTS.md" in error and "README and [ROADMAP.md]" in error for error in errors
    )
    assert any(
        "docs/ROADMAP_90_DAYS.md" in error and "stay aligned with both" in error
        for error in errors
    )


def test_checker_passes_for_valid_repo(
    tmp_path: Path,
) -> None:
    module = _load_module()
    _seed_valid_repo(tmp_path)
    module.ROOT = tmp_path

    assert module.check_repo() == []
