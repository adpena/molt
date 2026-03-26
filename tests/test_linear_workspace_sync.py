from __future__ import annotations

import tools.linear_workspace as linear_workspace


def test_build_sync_plan_creates_updates_and_dedupes() -> None:
    desired = [
        linear_workspace.DesiredIssue(
            title="[P1] Fix parser drift",
            description="new description",
            priority=2,
        ),
        linear_workspace.DesiredIssue(
            title="[P2] Add wasm parity lane",
            description="fresh lane",
            priority=3,
        ),
    ]
    existing = [
        {
            "id": "iss_1",
            "title": "[P1] Fix parser drift",
            "description": "old description",
            "priority": 1,
            "state": {"type": "started"},
            "createdAt": "2026-03-04T00:00:00.000Z",
        },
        {
            "id": "iss_2",
            "title": "[P1] Fix parser drift",
            "description": "old description",
            "priority": 1,
            "state": {"type": "started"},
            "createdAt": "2026-03-05T00:00:00.000Z",
        },
    ]

    plan = linear_workspace._build_sync_plan(
        desired=desired,
        existing=existing,
        update_existing=True,
        close_duplicates=True,
        close_missing=False,
        duplicate_state_id="state_canceled",
    )

    assert [item.title for item in plan.creates] == ["[P2] Add wasm parity lane"]
    assert [(issue_id, issue.priority) for issue_id, issue in plan.updates] == [
        ("iss_1", 2)
    ]
    assert list(plan.duplicate_updates) == ["iss_2"]
    assert plan.missing_updates == ()
    assert plan.existing_skipped == 1


def test_build_sync_plan_without_update_only_skips_existing() -> None:
    desired = [
        linear_workspace.DesiredIssue(
            title="[P1] Tighten intrinsic contract",
            description="same",
            priority=2,
        )
    ]
    existing = [
        {
            "id": "iss_9",
            "title": "[P1] Tighten intrinsic contract",
            "description": "same",
            "priority": 2,
            "state": {"type": "started"},
            "createdAt": "2026-03-04T00:00:00.000Z",
        }
    ]

    plan = linear_workspace._build_sync_plan(
        desired=desired,
        existing=existing,
        update_existing=False,
        close_duplicates=False,
        close_missing=False,
        duplicate_state_id=None,
    )

    assert plan.creates == ()
    assert plan.updates == ()
    assert plan.duplicate_updates == ()
    assert plan.missing_updates == ()
    assert plan.existing_skipped == 1


def test_build_sync_plan_matches_grouped_issues_by_group_key_not_title() -> None:
    desired = [
        linear_workspace.DesiredIssue(
            title="[P0][Critical Impact] Runtime stdlib intrinsic migration backlog",
            description=(
                "Auto-seeded from Molt roadmap/status TODO contracts.\n\n"
                "Grouped from 3 leaf items.\n\n---\n\n"
                "- area: stdlib-compat\n"
                "- group_key: runtime-and-intrinsics:stdlib-intrinsic-migration\n"
                "- kind: grouped\n"
                "- milestone: SL1\n"
                "- owner: stdlib\n"
                "- priority: P0\n"
                "- source: ROADMAP.md:10\n"
                "- status: missing"
            ),
            priority=1,
            sync_key="runtime-and-intrinsics:stdlib-intrinsic-migration",
        )
    ]
    existing = [
        {
            "id": "iss_grouped",
            "title": "[P1][High Impact] Runtime stdlib work backlog",
            "description": (
                "Auto-seeded from Molt roadmap/status TODO contracts.\n\n"
                "Older grouped issue.\n\n---\n\n"
                "- area: stdlib-compat\n"
                "- group_key: runtime-and-intrinsics:stdlib-intrinsic-migration\n"
                "- kind: grouped\n"
                "- milestone: SL3\n"
                "- owner: stdlib\n"
                "- priority: P1\n"
                "- source: ROADMAP.md:11\n"
                "- status: partial"
            ),
            "priority": 2,
            "state": {"type": "started"},
            "createdAt": "2026-03-04T00:00:00.000Z",
        }
    ]

    plan = linear_workspace._build_sync_plan(
        desired=desired,
        existing=existing,
        update_existing=True,
        close_duplicates=False,
        close_missing=False,
        duplicate_state_id=None,
    )

    assert plan.creates == ()
    assert [(issue_id, issue.title, issue.priority) for issue_id, issue in plan.updates] == [
        (
            "iss_grouped",
            "[P0][Critical Impact] Runtime stdlib intrinsic migration backlog",
            1,
        )
    ]
    assert plan.missing_updates == ()


def test_existing_sync_key_accepts_star_bullets_from_linear_markdown() -> None:
    issue = {
        "title": "[P1][High Impact] WASM Parity: wasm runtime and compatibility backlog",
        "description": (
            "Auto-seeded from Molt roadmap/status TODO contracts.\n\n"
            "* group_key: wasm-parity:wasm-runtime-and-compat\n"
            "* kind: grouped\n"
        ),
    }

    assert (
        linear_workspace._existing_sync_key(issue)
        == "wasm-parity:wasm-runtime-and-compat"
    )


def test_build_sync_plan_ignores_linear_markdown_rewrites_in_description() -> None:
    desired = [
        linear_workspace.DesiredIssue(
            title="[P1][High Impact] WASM Parity: wasm runtime and compatibility backlog",
            description=(
                "Auto-seeded from Molt roadmap/status TODO contracts.\n\n"
                "Representative sources: ROADMAP.md:1625; ROADMAP.md:1627.\n\n"
                "Leaf inventory:\n"
                "- [P2][RT3] zero-copy string passing for WASM)\n\n"
                "---\n\n"
                "- group_key: wasm-parity:wasm-runtime-and-compat\n"
                "- kind: grouped\n"
            ),
            priority=2,
            sync_key="wasm-parity:wasm-runtime-and-compat",
        )
    ]
    existing = [
        {
            "id": "iss_markdownized",
            "title": "[P1][High Impact] WASM Parity: wasm runtime and compatibility backlog",
            "description": (
                "Auto-seeded from Molt roadmap/status TODO contracts.\n\n"
                "Representative sources: [ROADMAP.md:1625](<http://ROADMAP.md:1625>); "
                "[ROADMAP.md:1627](<http://ROADMAP.md:1627>).\n\n"
                "Leaf inventory:\n"
                "* \\[P2\\]\\[RT3\\] zero-copy string passing for WASM)\n\n"
                "---\n\n"
                "* group_key: wasm-parity:wasm-runtime-and-compat\n"
                "* kind: grouped\n"
            ),
            "priority": 2,
            "state": {"type": "backlog"},
            "createdAt": "2026-03-04T00:00:00.000Z",
        }
    ]

    plan = linear_workspace._build_sync_plan(
        desired=desired,
        existing=existing,
        update_existing=True,
        close_duplicates=False,
        close_missing=False,
        duplicate_state_id=None,
    )

    assert plan.creates == ()
    assert plan.updates == ()
    assert plan.missing_updates == ()


def test_build_sync_plan_closes_missing_managed_grouped_issues_only() -> None:
    desired = [
        linear_workspace.DesiredIssue(
            title="[P1][High Impact] Runtime & Intrinsics: stdlib intrinsic migration backlog",
            description=(
                "Auto-seeded from Molt codebase TODO contracts.\n\n"
                "---\n\n"
                "- group_key: runtime-and-intrinsics:stdlib-intrinsic-migration\n"
                "- kind: grouped\n"
            ),
            priority=2,
            sync_key="runtime-and-intrinsics:stdlib-intrinsic-migration",
        )
    ]
    existing = [
        {
            "id": "iss_keep",
            "title": "keep",
            "description": (
                "Auto-seeded from Molt codebase TODO contracts.\n\n"
                "---\n\n"
                "- group_key: runtime-and-intrinsics:stdlib-intrinsic-migration\n"
                "- kind: grouped\n"
            ),
            "priority": 2,
            "state": {"type": "backlog"},
            "createdAt": "2026-03-04T00:00:00.000Z",
        },
        {
            "id": "iss_stale",
            "title": "stale",
            "description": (
                "Auto-seeded from Molt codebase TODO contracts.\n\n"
                "---\n\n"
                "- group_key: runtime-and-intrinsics:runtime-core-module-parity\n"
                "- kind: grouped\n"
            ),
            "priority": 2,
            "state": {"type": "backlog"},
            "createdAt": "2026-03-05T00:00:00.000Z",
        },
        {
            "id": "iss_unmanaged",
            "title": "unmanaged note",
            "description": "user-created issue without grouped metadata",
            "priority": 2,
            "state": {"type": "backlog"},
            "createdAt": "2026-03-06T00:00:00.000Z",
        },
    ]

    plan = linear_workspace._build_sync_plan(
        desired=desired,
        existing=existing,
        update_existing=False,
        close_duplicates=False,
        close_missing=True,
        duplicate_state_id="state_canceled",
    )

    assert plan.creates == ()
    assert plan.updates == ()
    assert plan.duplicate_updates == ()
    assert plan.missing_updates == ("iss_stale",)
