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
        root / "docs/CANONICALS.md",
        "\n".join(
            [
                "# Canonicals",
                "",
                "- [manifest](design/foundation/authority_manifest.toml)",
                "- [51](design/foundation/51_ten_year_roadmap.md)",
                "- [52](design/foundation/52_autonomous_operating_charter.md)",
                "",
            ]
        ),
    )
    _write_file(
        root / "docs/INDEX.md",
        "\n".join(
            [
                "# Index",
                "",
                "- [manifest](design/foundation/authority_manifest.toml)",
                "- [51](design/foundation/51_ten_year_roadmap.md)",
                "- [52](design/foundation/52_autonomous_operating_charter.md)",
                "",
            ]
        ),
    )
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
    _write_file(
        root / "docs/spec/README.md",
        "# Spec Index\n\n- [Status](STATUS.md)\n",
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
    _write_file(
        root / "docs/design/foundation/00_integrated_parallel_program.md",
        "\n".join(
            [
                "<!-- Supersedes stale 2026-06-05 claims. -->",
                "",
                "The live codebase, executable tests, and generated evidence are authoritative.",
                "",
            ]
        ),
    )
    _write_file(
        root / "docs/design/foundation/authority_manifest.toml",
        "\n".join(
            [
                "[[authority]]",
                'path = "docs/design/foundation/00_integrated_parallel_program.md"',
                "required_markers = [",
                '  "Supersedes stale",',
                '  "live codebase, executable tests, and generated evidence are authoritative",',
                "]",
                "",
                "[[authority]]",
                'path = "docs/design/foundation/51_ten_year_roadmap.md"',
                'index_ref = "design/foundation/51_ten_year_roadmap.md"',
                'canonicals_ref = "design/foundation/51_ten_year_roadmap.md"',
                "required_markers = [",
                '  "Status: NORTH STAR",',
                '  "Faster than CPython",',
                '  "SEMANTIC FACT PLANE",',
                "]",
                "",
                "[[authority]]",
                'path = "docs/design/foundation/52_autonomous_operating_charter.md"',
                'index_ref = "design/foundation/52_autonomous_operating_charter.md"',
                'canonicals_ref = "design/foundation/52_autonomous_operating_charter.md"',
                "required_markers = [",
                '  "Status: BINDING OPERATING DOCTRINE",',
                '  "design docs go stale",',
                '  "The verifier is the product",',
                "]",
                "",
            ]
        ),
    )
    _write_file(
        root / "docs/design/foundation/51_ten_year_roadmap.md",
        "\n".join(
            [
                "# 51 — Ten-year roadmap",
                "",
                "Status: NORTH STAR",
                "",
                "Faster than CPython.",
                "",
                "SEMANTIC FACT PLANE",
                "",
            ]
        ),
    )
    _write_file(
        root / "docs/design/foundation/52_autonomous_operating_charter.md",
        "\n".join(
            [
                "# 52 — Autonomous operating charter",
                "",
                "Status: BINDING OPERATING DOCTRINE",
                "",
                "RECON: design docs go stale in hours.",
                "",
                "The verifier is the product.",
                "",
            ]
        ),
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


def test_checker_requires_long_horizon_routing_and_authority_markers(
    tmp_path: Path,
) -> None:
    module = _load_module()
    _seed_valid_repo(tmp_path)
    _write_file(tmp_path / "docs/INDEX.md", "# Index\n")
    _write_file(
        tmp_path / "docs/design/foundation/52_autonomous_operating_charter.md",
        "Status: BINDING OPERATING DOCTRINE\n",
    )
    module.ROOT = tmp_path

    errors = module.check_repo()

    assert any(
        "docs/INDEX.md" in error and "51_ten_year_roadmap.md" in error
        for error in errors
    )
    assert any(
        "52_autonomous_operating_charter.md" in error
        and "design docs go stale" in error
        for error in errors
    )


def test_checker_requires_planning_authority_manifest(
    tmp_path: Path,
) -> None:
    module = _load_module()
    _seed_valid_repo(tmp_path)
    (tmp_path / "docs/design/foundation/authority_manifest.toml").unlink()
    module.ROOT = tmp_path

    errors = module.check_repo()

    assert any("authority_manifest.toml" in error for error in errors)


def test_checker_requires_foundation_portfolio_numbering_to_match_filename(
    tmp_path: Path,
) -> None:
    module = _load_module()
    _seed_valid_repo(tmp_path)
    _write_file(
        tmp_path / "docs/design/foundation/64_perf_scoreboards_and_harness.md",
        "\n".join(
            [
                "<!-- Foundation blueprint 53.",
                "doc: 53",
                "-->",
                "",
                "# 53 — The Perf Measurement Plane",
                "",
            ]
        ),
    )
    module.ROOT = tmp_path

    errors = module.check_repo()

    assert any("64_perf_scoreboards_and_harness.md" in error for error in errors)
    assert any(
        "heading number must match filename prefix 64" in error for error in errors
    )
    assert any("Foundation blueprint metadata" in error for error in errors)
    assert any("doc metadata" in error for error in errors)


def test_checker_rejects_duplicate_foundation_portfolio_number_authority(
    tmp_path: Path,
) -> None:
    module = _load_module()
    _seed_valid_repo(tmp_path)
    _write_file(
        tmp_path / "docs/design/foundation/64_perf_scoreboards.md",
        "# 64 — Performance scoreboards\n",
    )
    _write_file(
        tmp_path / "docs/design/foundation/64_runtime_scoreboards.md",
        "# 64 — Runtime scoreboards\n",
    )
    module.ROOT = tmp_path

    errors = module.check_repo()

    assert any(
        error.startswith("foundation portfolio number 64 has multiple authorities:")
        for error in errors
    )


def test_checker_allows_unnumbered_supplementary_foundation_authority(
    tmp_path: Path,
) -> None:
    module = _load_module()
    _seed_valid_repo(tmp_path)
    _write_file(
        tmp_path / "docs/design/foundation/generational_gc_authority.md",
        "# Generational cyclic-GC authority\n",
    )
    module.ROOT = tmp_path

    assert module.check_repo() == []


def test_checker_resolves_local_markdown_links_from_the_owning_document(
    tmp_path: Path,
) -> None:
    module = _load_module()
    _seed_valid_repo(tmp_path)
    _write_file(
        tmp_path / "docs/spec/README.md",
        "# Spec Index\n\n- [Status](STATUS.md#current)\n- [Missing][missing]\n\n"
        "[missing]: areas/core/missing.md\n",
    )
    module.ROOT = tmp_path

    errors = module.check_repo()

    assert errors == [
        "docs/spec/README.md:4: broken local Markdown target "
        "'areas/core/missing.md'; expected docs/spec/areas/core/missing.md"
    ]


def test_checker_ignores_anchor_url_and_code_sample_targets(tmp_path: Path) -> None:
    module = _load_module()
    _seed_valid_repo(tmp_path)
    _write_file(
        tmp_path / "docs/spec/README.md",
        "\n".join(
            [
                "# Spec Index",
                "",
                "[Anchor](#spec-index)",
                "[HTTPS](https://example.com/missing.md)",
                "[Mail](mailto:docs@example.com)",
                "[Protocol relative](//example.com/missing.md)",
                "[Site root](/docs/missing.md)",
                "",
                "```markdown",
                "[Code sample](missing.md)",
                "```",
                "",
            ]
        ),
    )
    module.ROOT = tmp_path

    assert module.check_repo() == []


def test_checker_rejects_local_markdown_links_that_escape_the_repo(
    tmp_path: Path,
) -> None:
    module = _load_module()
    _seed_valid_repo(tmp_path)
    _write_file(
        tmp_path / "docs/spec/README.md",
        "# Spec Index\n\n[Escape](../../../outside.md)\n",
    )
    module.ROOT = tmp_path

    errors = module.check_repo()

    assert errors == [
        "docs/spec/README.md:3: local Markdown target escapes repository: "
        "'../../../outside.md'"
    ]


def test_checker_fails_closed_when_a_markdown_link_authority_is_missing(
    tmp_path: Path,
) -> None:
    module = _load_module()
    _seed_valid_repo(tmp_path)
    (tmp_path / "docs/spec/README.md").unlink()
    module.ROOT = tmp_path

    errors = module.check_repo()

    assert "docs/spec/README.md: missing Markdown link authority" in errors


def test_local_markdown_path_case_check_is_platform_independent(tmp_path: Path) -> None:
    module = _load_module()
    _seed_valid_repo(tmp_path)

    assert module._has_exact_path_case(
        tmp_path / "docs/spec/STATUS.md", root=tmp_path, directory_entries={}
    )
    assert not module._has_exact_path_case(
        tmp_path / "docs/spec/status.md", root=tmp_path, directory_entries={}
    )


def test_checker_passes_for_valid_repo(
    tmp_path: Path,
) -> None:
    module = _load_module()
    _seed_valid_repo(tmp_path)
    module.ROOT = tmp_path

    assert module.check_repo() == []


def _write_ledger(root: Path, rows: list[str]) -> None:
    _write_file(
        root / "docs/agent/V1_HANDOFF_FINDINGS.md",
        "# Ledger\n\n| ID | Finding | Fix |\n|----|----|----|\n"
        + "".join(f"{row}\n" for row in rows),
    )


def test_checker_rejects_one_finding_id_on_two_ledger_rows(tmp_path: Path) -> None:
    module = _load_module()
    _seed_valid_repo(tmp_path)
    _write_ledger(
        tmp_path,
        [
            "| HF-F59 | A sentinel race. | Fixed. |",
            "| HF-F60 | A target dir. | Fixed. |",
            "| HF-F59 (was HF-70) | A drift gate. | Fixed. |",
        ],
    )
    module.ROOT = tmp_path

    errors = module.check_repo()

    assert [error for error in errors if "names 2 rows" in error] == [
        "docs/agent/V1_HANDOFF_FINDINGS.md: finding HF-F59 names 2 rows "
        "(lines 5, 7); give each finding one ID"
    ]


def test_checker_accepts_distinct_ledger_ids_and_prose_mentions(
    tmp_path: Path,
) -> None:
    module = _load_module()
    _seed_valid_repo(tmp_path)
    _write_ledger(
        tmp_path,
        [
            "| HF-12 | Runtime suite. | See HF-12 and V1-25. |",
            "| HF-F57 (was HF-33) | Generators. | Fixed. |",
            "| V1-25 | Publication. | Open. |",
            "| V1-25 (output custody) | One fixed part. | Fixed. |",
        ],
    )
    module.ROOT = tmp_path

    assert not [error for error in module.check_repo() if "names" in error]


def test_checker_rejects_an_open_row_its_fixed_row_closed(tmp_path: Path) -> None:
    module = _load_module()
    _seed_valid_repo(tmp_path)
    _write_ledger(
        tmp_path,
        [
            "| HF-91 | The intrinsics gate. | Open again after a merge. |",
            "| HF-37 | Remaining Rust truth. | Open. |",
            "| HF-F102 (was HF-91) | The intrinsics gate. | Fixed. |",
            "| HF-F37 (was part of HF-37) | The macOS cell. | Fixed. |",
        ],
    )
    module.ROOT = tmp_path

    errors = [error for error in module.check_repo() if "fixed row says" in error]

    assert errors == [
        "docs/agent/V1_HANDOFF_FINDINGS.md: HF-91 is open, but a fixed row says it "
        'was HF-91; remove the resurrected row, or write "(was part of HF-91)" '
        "when the fix was partial"
    ]
