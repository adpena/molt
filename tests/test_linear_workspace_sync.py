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
        duplicate_state_id="state_canceled",
    )

    assert [item.title for item in plan.creates] == ["[P2] Add wasm parity lane"]
    assert [(issue_id, issue.priority) for issue_id, issue in plan.updates] == [
        ("iss_1", 2)
    ]
    assert list(plan.duplicate_updates) == ["iss_2"]
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
        duplicate_state_id=None,
    )

    assert plan.creates == ()
    assert plan.updates == ()
    assert plan.duplicate_updates == ()
    assert plan.existing_skipped == 1
