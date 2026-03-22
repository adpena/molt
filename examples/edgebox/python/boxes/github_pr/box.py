# boxes/github_pr/box.py -- GitHub PR Box
#
# A concrete Box that tracks pull-request activity. It receives GitHub
# webhook events, stores them in SQLite, and exposes MCP tools for
# AI agents to query the PR timeline, post review comments, and
# summarize diffs.
#
# Entry point: instantiate the box and print(box.dispatch())

import json
import os
import sys

from edgebox.box import Box, tool, alarm
from edgebox.db import BoxDB


# Path to the schema file, relative to this module
_HERE = os.path.dirname(os.path.abspath(__file__))
_SCHEMA_PATH = os.path.join(_HERE, "schema.sql")

# Default database path (overridable via EDGEBOX_DB_PATH env var)
_DEFAULT_DB = os.environ.get("EDGEBOX_DB_PATH", "github_pr.db")


class GithubPRBox(Box):
    """Box for GitHub pull-request lifecycle management.

    Webhook events are ingested via on_webhook() and stored in the
    events table. Tools let an AI agent query the timeline, post
    review comments, and get diff summaries. A stale_check alarm
    fires periodically to flag inactive PRs.
    """

    def __init__(self, db_path=None):
        super().__init__()
        if db_path is None:
            db_path = _DEFAULT_DB
        self.db = BoxDB(db_path)
        self._init_schema()

    def _init_schema(self):
        """Load and execute the DDL schema."""
        schema_file = open(_SCHEMA_PATH, "r")
        sql = schema_file.read()
        schema_file.close()
        self.db.execute_schema(sql)

    # -- Webhook ingress ----------------------------------------------------

    def on_webhook(self, req):
        """Handle an inbound GitHub webhook event.

        Expects a JSON body with at least:
            action   - the event action (opened, closed, etc.)
            pr_id    - the pull request number
            sender   - the actor login

        The full payload is stored verbatim for later querying.
        """
        body = req.json()
        if body is None:
            return json.dumps({"error": "empty webhook body"})

        action = body.get("action", "unknown")
        pr_id = body.get("pr_id", 0)
        sender = body.get("sender", "")
        payload_str = json.dumps(body)

        self.db.execute(
            "INSERT INTO events (pr_id, event_type, actor, payload) VALUES (?, ?, ?, ?)",
            [pr_id, action, sender, payload_str],
        )

        return json.dumps({"ok": True, "event": action, "pr_id": pr_id})

    # -- Tools --------------------------------------------------------------

    @tool(name="get_timeline", description="Get the event timeline for a pull request")
    def get_timeline(self, pr_id=0, limit=50):
        """Return recent events for a PR, newest first."""
        rows = self.db.query(
            "SELECT id, event_type, actor, payload, created_at "
            "FROM events WHERE pr_id = ? ORDER BY id DESC LIMIT ?",
            [pr_id, limit],
        )
        return rows

    @tool(name="add_review_comment", description="Post a review comment on a file in the PR")
    def add_review_comment(self, pr_id=0, path="", line=0, body="", author="edgebox"):
        """Insert a review comment into local storage."""
        row_id = self.db.execute(
            "INSERT INTO review_comments (pr_id, path, line, body, author) "
            "VALUES (?, ?, ?, ?, ?)",
            [pr_id, path, line, body, author],
        )
        return {"comment_id": row_id, "pr_id": pr_id}

    @tool(name="get_diff_summary", description="Get a summary of files changed in the PR")
    def get_diff_summary(self, pr_id=0):
        """Summarize the PR by counting events and comments."""
        event_count = self.db.query(
            "SELECT COUNT(*) as cnt FROM events WHERE pr_id = ?",
            [pr_id],
        )
        comment_count = self.db.query(
            "SELECT COUNT(*) as cnt FROM review_comments WHERE pr_id = ?",
            [pr_id],
        )

        # Get distinct event types
        event_types = self.db.query(
            "SELECT DISTINCT event_type FROM events WHERE pr_id = ?",
            [pr_id],
        )
        type_list = []
        idx = 0
        while idx < len(event_types):
            type_list.append(event_types[idx]["event_type"])
            idx = idx + 1

        return {
            "pr_id": pr_id,
            "total_events": event_count[0]["cnt"] if event_count else 0,
            "total_comments": comment_count[0]["cnt"] if comment_count else 0,
            "event_types": type_list,
        }

    @tool(name="query_timeline", description="Run a filtered query against the event timeline")
    def query_timeline(self, pr_id=0, event_type="", actor="", limit=20):
        """Query events with optional filters on type and actor."""
        sql = "SELECT id, event_type, actor, payload, created_at FROM events WHERE pr_id = ?"
        params = [pr_id]

        if event_type:
            sql = sql + " AND event_type = ?"
            params.append(event_type)

        if actor:
            sql = sql + " AND actor = ?"
            params.append(actor)

        sql = sql + " ORDER BY id DESC LIMIT ?"
        params.append(limit)

        return self.db.query(sql, params)

    # -- Alarms -------------------------------------------------------------

    @alarm("stale_check")
    def stale_check(self):
        """Check for PRs with no activity in the last 7 days.

        Returns a list of PR IDs that appear stale. The runtime can
        use this to send notifications or trigger follow-up actions.
        """
        stale_rows = self.db.query(
            "SELECT DISTINCT pr_id FROM events "
            "GROUP BY pr_id "
            "HAVING MAX(created_at) < datetime('now', '-7 days')"
        )
        stale_ids = []
        idx = 0
        while idx < len(stale_rows):
            stale_ids.append(stale_rows[idx]["pr_id"])
            idx = idx + 1
        return {"stale_prs": stale_ids}


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    box = GithubPRBox()
    print(box.dispatch())
