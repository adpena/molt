from __future__ import annotations

import argparse

import pytest

import tools.linear_workspace as linear_workspace


def test_resolve_issue_id_prefers_direct_lookup(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    def _fake_graphql(
        query: str, variables: dict[str, object] | None = None
    ) -> dict[str, object]:
        assert query == linear_workspace.QUERY_ISSUE_BY_ID
        assert variables == {"id": "MOL-42"}
        return {"issue": {"id": "issue-42"}}

    monkeypatch.setattr(linear_workspace, "graphql", _fake_graphql)
    issue_id = linear_workspace._resolve_issue_id("team-id", "MOL-42")
    assert issue_id == "issue-42"


def test_resolve_issue_id_falls_back_to_team_scan(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    def _fake_graphql(
        query: str, variables: dict[str, object] | None = None
    ) -> dict[str, object]:
        raise RuntimeError("lookup_failed")

    monkeypatch.setattr(linear_workspace, "graphql", _fake_graphql)
    monkeypatch.setattr(
        linear_workspace,
        "_fetch_issues",
        lambda _team_id, _project_id: [
            {"id": "issue-100", "identifier": "MOL-100"},
            {"id": "issue-101", "identifier": "MOL-101"},
        ],
    )
    assert linear_workspace._resolve_issue_id("team-id", "MOL-101") == "issue-101"


def test_resolve_issue_id_uses_identifier_query_before_full_scan(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    calls: list[str] = []

    def _fake_graphql(
        query: str, variables: dict[str, object] | None = None
    ) -> dict[str, object]:
        calls.append(query)
        if query == linear_workspace.QUERY_ISSUE_BY_ID:
            raise RuntimeError("not_by_id")
        if query == linear_workspace.QUERY_ISSUE_BY_IDENTIFIER:
            assert variables == {"teamId": "team-id", "identifier": "MOL-42"}
            return {"issues": {"nodes": [{"id": "issue-42", "identifier": "MOL-42"}]}}
        raise AssertionError(f"unexpected query {query}")

    monkeypatch.setattr(linear_workspace, "graphql", _fake_graphql)
    issue_id = linear_workspace._resolve_issue_id("team-id", "MOL-42")
    assert issue_id == "issue-42"
    assert linear_workspace.QUERY_ISSUE_BY_IDENTIFIER in calls


def test_viewer_uses_lru_cache(monkeypatch: pytest.MonkeyPatch) -> None:
    linear_workspace._viewer.cache_clear()
    calls = {"count": 0}

    def _fake_graphql(
        query: str, variables: dict[str, object] | None = None
    ) -> dict[str, object]:
        assert query == linear_workspace.QUERY_VIEWER
        calls["count"] += 1
        return {"viewer": {"id": "u1", "teams": {"nodes": []}}}

    monkeypatch.setattr(linear_workspace, "graphql", _fake_graphql)
    linear_workspace._viewer()
    linear_workspace._viewer()
    assert calls["count"] == 1
    linear_workspace._viewer.cache_clear()


def test_cmd_update_issue_builds_expected_payload(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    calls: list[tuple[str, dict[str, object]]] = []

    monkeypatch.setattr(linear_workspace, "_resolve_team_id", lambda _team: "team-id")
    monkeypatch.setattr(
        linear_workspace, "_resolve_issue_id", lambda _team_id, _issue: "issue-id"
    )
    monkeypatch.setattr(
        linear_workspace, "_resolve_project_id", lambda _team_id, _project: "project-id"
    )
    monkeypatch.setattr(
        linear_workspace, "_state_id_by_name", lambda _team_id, _state: "state-id"
    )

    def _fake_graphql(
        query: str, variables: dict[str, object] | None = None
    ) -> dict[str, object]:
        calls.append((query, variables or {}))
        return {
            "issueUpdate": {
                "success": True,
                "issue": {"id": "issue-id", "identifier": "MOL-1", "title": "Updated"},
            }
        }

    monkeypatch.setattr(linear_workspace, "graphql", _fake_graphql)
    args = argparse.Namespace(
        team="MOL",
        issue="MOL-1",
        title="Updated",
        description="new body",
        priority=1,
        project="proj",
        state="In Progress",
    )
    rc = linear_workspace.cmd_update_issue(args)
    assert rc == 0
    assert len(calls) == 1
    query, variables = calls[0]
    assert query == linear_workspace.MUTATION_ISSUE_UPDATE
    assert variables == {
        "id": "issue-id",
        "input": {
            "title": "Updated",
            "description": "new body",
            "priority": 1,
            "projectId": "project-id",
            "stateId": "state-id",
        },
    }


def test_cmd_update_issue_requires_at_least_one_field(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(linear_workspace, "_resolve_team_id", lambda _team: "team-id")
    monkeypatch.setattr(
        linear_workspace, "_resolve_issue_id", lambda _team_id, _issue: "issue-id"
    )
    args = argparse.Namespace(
        team="MOL",
        issue="MOL-1",
        title=None,
        description=None,
        priority=None,
        project=None,
        state=None,
    )
    with pytest.raises(RuntimeError):
        linear_workspace.cmd_update_issue(args)


def test_cmd_comment_issue_posts_expected_payload(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    calls: list[tuple[str, dict[str, object]]] = []
    monkeypatch.setattr(linear_workspace, "_resolve_team_id", lambda _team: "team-id")
    monkeypatch.setattr(
        linear_workspace, "_resolve_issue_id", lambda _team_id, _issue: "issue-id"
    )

    def _fake_graphql(
        query: str, variables: dict[str, object] | None = None
    ) -> dict[str, object]:
        calls.append((query, variables or {}))
        return {
            "commentCreate": {
                "success": True,
                "comment": {"id": "comment-1", "body": "hello"},
            }
        }

    monkeypatch.setattr(linear_workspace, "graphql", _fake_graphql)
    args = argparse.Namespace(team="MOL", issue="MOL-1", body="hello")
    rc = linear_workspace.cmd_comment_issue(args)
    assert rc == 0
    assert calls == [
        (
            linear_workspace.MUTATION_COMMENT_CREATE,
            {"input": {"issueId": "issue-id", "body": "hello"}},
        )
    ]


def test_cmd_get_issue_returns_resolved_issue(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(linear_workspace, "_resolve_team_id", lambda _team: "team-id")
    monkeypatch.setattr(
        linear_workspace,
        "_resolve_issue",
        lambda _team_id, _issue: {"id": "issue-id", "identifier": "MOL-1"},
    )
    args = argparse.Namespace(team="MOL", issue="MOL-1")
    assert linear_workspace.cmd_get_issue(args) == 0


def test_cmd_list_comments_paginates(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(linear_workspace, "_resolve_team_id", lambda _team: "team-id")
    monkeypatch.setattr(
        linear_workspace, "_resolve_issue_id", lambda _team_id, _issue: "issue-id"
    )
    calls: list[dict[str, object]] = []

    def _fake_graphql(
        query: str, variables: dict[str, object] | None = None
    ) -> dict[str, object]:
        assert query == linear_workspace.QUERY_ISSUE_COMMENTS
        payload = variables or {}
        calls.append(payload)
        if payload.get("after") is None:
            return {
                "issue": {
                    "comments": {
                        "nodes": [{"id": "c1"}, {"id": "c2"}],
                        "pageInfo": {"hasNextPage": True, "endCursor": "cursor-1"},
                    }
                }
            }
        return {
            "issue": {
                "comments": {
                    "nodes": [{"id": "c3"}],
                    "pageInfo": {"hasNextPage": False, "endCursor": None},
                }
            }
        }

    monkeypatch.setattr(linear_workspace, "graphql", _fake_graphql)
    args = argparse.Namespace(team="MOL", issue="MOL-1", limit=3, page_size=2)
    assert linear_workspace.cmd_list_comments(args) == 0
    assert len(calls) == 2
    assert calls[0]["first"] == 2
    assert calls[1]["after"] == "cursor-1"


def test_issue_branch_name_prefers_linear_branch_name() -> None:
    issue = {"identifier": "MOL-7", "title": "Hello", "branchName": "adpena/mol-7"}
    assert linear_workspace._issue_branch_name(issue) == "adpena/mol-7"


def test_issue_branch_name_derives_slug_when_missing() -> None:
    issue = {"identifier": "MOL-7", "title": "Fix Linear CLI Drift!!!"}
    assert linear_workspace._issue_branch_name(issue) == "mol-7/fix-linear-cli-drift"


def test_cmd_checkout_branch_prefers_local(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(linear_workspace, "_resolve_team_id", lambda _team: "team-id")
    monkeypatch.setattr(
        linear_workspace,
        "_resolve_issue",
        lambda _team_id, _issue: {"identifier": "MOL-7", "title": "Fix drift"},
    )
    monkeypatch.setattr(
        linear_workspace, "_git_branch_exists_local", lambda _branch: True
    )
    monkeypatch.setattr(
        linear_workspace, "_git_branch_exists_remote", lambda _remote, _branch: False
    )
    called: list[str] = []
    monkeypatch.setattr(
        linear_workspace,
        "_git_checkout_local",
        lambda branch: called.append(f"local:{branch}"),
    )
    args = argparse.Namespace(
        team="MOL",
        issue="MOL-7",
        branch=None,
        remote="origin",
        create_if_missing=False,
        dry_run=False,
    )
    assert linear_workspace.cmd_checkout_branch(args) == 0
    assert called == ["local:mol-7/fix-drift"]


def test_cmd_checkout_branch_creates_when_missing(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(linear_workspace, "_resolve_team_id", lambda _team: "team-id")
    monkeypatch.setattr(
        linear_workspace,
        "_resolve_issue",
        lambda _team_id, _issue: {"identifier": "MOL-8", "title": "New Feature"},
    )
    monkeypatch.setattr(
        linear_workspace, "_git_branch_exists_local", lambda _branch: False
    )
    monkeypatch.setattr(
        linear_workspace, "_git_branch_exists_remote", lambda _remote, _branch: False
    )
    called: list[str] = []
    monkeypatch.setattr(
        linear_workspace, "_git_checkout_new", lambda branch: called.append(branch)
    )
    args = argparse.Namespace(
        team="MOL",
        issue="MOL-8",
        branch=None,
        remote="origin",
        create_if_missing=True,
        dry_run=False,
    )
    assert linear_workspace.cmd_checkout_branch(args) == 0
    assert called == ["mol-8/new-feature"]
