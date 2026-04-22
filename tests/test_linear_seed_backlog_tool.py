from __future__ import annotations

from pathlib import Path

import tools.linear_seed_backlog as linear_seed_backlog


def test_extract_todos_uses_repo_relative_source_path(tmp_path: Path) -> None:
    roadmap = tmp_path / "ROADMAP.md"
    roadmap.write_text(
        "- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): "
        "add deterministic pass telemetry sink\n",
        encoding="utf-8",
    )

    todos = linear_seed_backlog.extract_todos(roadmap, repo_root=tmp_path)

    assert len(todos) == 1
    assert todos[0]["source_file"] == "ROADMAP.md"


def test_normalize_issue_sanitizes_title_noise() -> None:
    issue = linear_seed_backlog.normalize_issue(
        {
            "area": "compiler",
            "owner": "compiler",
            "milestone": "LF2",
            "priority": "P0",
            "status": "partial",
            "body": "expand async generator lowering coverage) |",
            "source_file": "ROADMAP.md",
            "source_line": 123,
        }
    )

    assert issue["title"] == "[P0][LF2] expand async generator lowering coverage)"


def test_normalize_issue_strips_nested_todo_suffix_and_quote_noise() -> None:
    assert linear_seed_backlog._sanitize_body('fixture partial marker. "') == (
        "fixture partial marker"
    )
    assert (
        linear_seed_backlog._sanitize_body(
            "wire local timezone + locale on wasm hosts). "
            "(TODO(stdlib-compat, owner:stdlib, milestone:SL2, "
            "priority:P2, status:partial): deterministic clock policy) |"
        )
        == "wire local timezone + locale on wasm hosts)"
    )


def test_group_manifest_items_rolls_up_priority_impact_and_leaf_inventory() -> None:
    grouped = linear_seed_backlog.group_manifest_items(
        "Runtime & Intrinsics",
        [
            {
                "title": "[P0][SL1] replace `_io` top-level stub with full intrinsic-backed lowering",
                "description": "leaf 1",
                "priority": 1,
                "metadata": {
                    "area": "stdlib-compat",
                    "owner": "stdlib",
                    "milestone": "SL1",
                    "priority": "P0",
                    "status": "missing",
                    "source": "ROADMAP.md:10",
                },
            },
            {
                "title": "[P1][SL3] replace `_hashlib` top-level stub with full intrinsic-backed lowering",
                "description": "leaf 2",
                "priority": 2,
                "metadata": {
                    "area": "stdlib-compat",
                    "owner": "stdlib",
                    "milestone": "SL3",
                    "priority": "P1",
                    "status": "partial",
                    "source": "ROADMAP.md:11",
                },
            },
            {
                "title": "[P1][SL3] replace `_csv` top-level stub with full intrinsic-backed lowering",
                "description": "leaf 3",
                "priority": 2,
                "metadata": {
                    "area": "stdlib-compat",
                    "owner": "stdlib",
                    "milestone": "SL3",
                    "priority": "P1",
                    "status": "planned",
                    "source": "ROADMAP.md:12",
                },
            },
        ],
    )

    assert len(grouped) == 1
    issue = grouped[0]
    assert issue["priority"] == 1
    assert issue["title"].startswith("[P0][Critical Impact]")
    assert "stdlib intrinsic migration backlog" in issue["title"].lower()
    assert "3 leaf items" in issue["description"]
    assert "Source of truth: codebase TODO contracts" in issue["description"]
    assert "P0: 1" in issue["description"]
    assert "missing: 1" in issue["description"]
    assert issue["metadata"]["kind"] == "grouped"
    assert issue["metadata"]["leaf_count"] == 3
    assert issue["metadata"]["p0_count"] == 1
    assert issue["metadata"]["p1_count"] == 2
    assert issue["metadata"]["missing_count"] == 1
    assert issue["metadata"]["group_key"].startswith("runtime-and-intrinsics:")


def test_group_manifest_items_tracks_code_backed_vs_doc_backed_pressure() -> None:
    grouped = linear_seed_backlog.group_manifest_items(
        "Tooling & DevEx",
        [
            {
                "title": "[P1][TL2] add build cache telemetry",
                "description": "code leaf",
                "priority": 2,
                "metadata": {
                    "area": "tooling",
                    "owner": "tooling",
                    "milestone": "TL2",
                    "priority": "P1",
                    "status": "partial",
                    "source": "tools/build_cache.py:10",
                },
            },
            {
                "title": "[P1][TL2] add distributed cache playbook",
                "description": "doc leaf",
                "priority": 2,
                "metadata": {
                    "area": "tooling",
                    "owner": "tooling",
                    "milestone": "TL2",
                    "priority": "P1",
                    "status": "planned",
                    "source": "ROADMAP.md:1555",
                },
            },
        ],
    )

    assert len(grouped) == 1
    issue = grouped[0]
    assert issue["metadata"]["code_source_count"] == 1
    assert issue["metadata"]["doc_source_count"] == 1
    assert issue["metadata"]["codebacked_leaf_count"] == 1
    assert issue["metadata"]["docbacked_leaf_count"] == 1
    assert "Codebase-backed pressure: 1 leaf items." in issue["description"]
    assert "Secondary non-code pressure: 1 leaf items." in issue["description"]
    assert issue["metadata"]["secondary_signal_count"] == 1
    assert issue["metadata"]["codebase_source_of_truth"] == "true"


def test_build_seed_backlog_discovers_code_todos_outside_doc_seed_files(
    tmp_path: Path,
) -> None:
    (tmp_path / "ROADMAP.md").write_text(
        "- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): "
        "stale roadmap-only compiler work\n",
        encoding="utf-8",
    )
    src_dir = tmp_path / "src" / "molt"
    src_dir.mkdir(parents=True)
    (src_dir / "example.py").write_text(
        "# TODO(tooling, owner:tooling, milestone:TL2, priority:P1, status:partial): "
        "add deterministic cache diagnostics in code path\n",
        encoding="utf-8",
    )

    backlog = linear_seed_backlog.build_seed_backlog(tmp_path, max_items=None)
    titles = [item["title"] for item in backlog["selected"]]

    assert titles == ["[P1][TL2] add deterministic cache diagnostics in code path"]
    assert backlog["source_mode"] == "codebase"


def test_build_seed_backlog_can_include_doc_seed_files_when_requested(
    tmp_path: Path,
) -> None:
    (tmp_path / "ROADMAP.md").write_text(
        "- TODO(compiler, owner:compiler, milestone:LF2, priority:P0, status:partial): "
        "include roadmap signal when explicitly requested\n",
        encoding="utf-8",
    )
    src_dir = tmp_path / "src" / "molt"
    src_dir.mkdir(parents=True)
    (src_dir / "example.py").write_text(
        "# TODO(tooling, owner:tooling, milestone:TL2, priority:P1, status:partial): "
        "keep codebase signal first\n",
        encoding="utf-8",
    )

    backlog = linear_seed_backlog.build_seed_backlog(
        tmp_path, max_items=None, source_mode="all"
    )

    titles = [item["title"] for item in backlog["selected"]]
    assert "[P0][LF2] include roadmap signal when explicitly requested" in titles
    assert "[P1][TL2] keep codebase signal first" in titles
    assert backlog["source_mode"] == "all"


def test_dedupe_prefers_codebacked_source_over_doc_source() -> None:
    items = [
        {
            "title": "[P1][SL3] replace `mailcap` top-level stub with full intrinsic-backed lowering",
            "description": "doc leaf",
            "priority": 2,
            "metadata": {
                "area": "stdlib-parity",
                "owner": "stdlib",
                "milestone": "SL3",
                "priority": "P1",
                "status": "planned",
                "source": "ROADMAP.md:10",
            },
        },
        {
            "title": "[P1][SL3] replace `mailcap` top-level stub with full intrinsic-backed lowering",
            "description": "code leaf",
            "priority": 2,
            "metadata": {
                "area": "stdlib-parity",
                "owner": "stdlib",
                "milestone": "SL3",
                "priority": "P1",
                "status": "planned",
                "source": "src/molt/stdlib/mailcap.py:14",
            },
        },
    ]

    deduped = linear_seed_backlog.dedupe(items)

    assert len(deduped) == 1
    assert deduped[0]["metadata"]["source"] == "src/molt/stdlib/mailcap.py:14"


def test_extract_todos_supports_multiline_contract_bodies(tmp_path: Path) -> None:
    module = tmp_path / "multiline.py"
    module.write_text(
        "# TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P2, status:partial):\n"
        "# Support per-thread initializer in ThreadPoolExecutor via Rust intrinsic.\n",
        encoding="utf-8",
    )
    todos = linear_seed_backlog.extract_todos(module)

    assert len(todos) == 1
    assert todos[0]["body"] == (
        "Support per-thread initializer in ThreadPoolExecutor via Rust intrinsic."
    )


def test_build_seed_backlog_includes_formal_and_demo_roots_in_codebase_mode(
    tmp_path: Path,
) -> None:
    formal_dir = tmp_path / "formal" / "lean"
    formal_dir.mkdir(parents=True)
    (formal_dir / "Proof.lean").write_text(
        "-- TODO(formal, owner:compiler, milestone:M5, priority:P1, status:partial):\n"
        "-- Prove SCCP transfer preserves abstract interpretation soundness.\n",
        encoding="utf-8",
    )
    demo_dir = tmp_path / "demo" / "molt_worker_app" / "app"
    demo_dir.mkdir(parents=True)
    (demo_dir / "entrypoints.py").write_text(
        '"""\n'
        "TODO(offload, owner:runtime, milestone:SL1, priority:P1, status:partial): "
        "compile entrypoints into molt_worker.\n"
        '"""\n',
        encoding="utf-8",
    )

    backlog = linear_seed_backlog.build_seed_backlog(tmp_path, max_items=None)
    titles = [item["title"] for item in backlog["selected"]]

    assert (
        "[P1][M5] Prove SCCP transfer preserves abstract interpretation soundness"
        in titles
    )
    assert "[P1][SL1] compile entrypoints into molt_worker" in titles
