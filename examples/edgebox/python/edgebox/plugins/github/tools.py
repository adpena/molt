# edgebox/plugins/github/tools.py -- MCP tools for GitHub PR tracking
#
# Registers tools on the github plugin instance. These are discovered
# automatically when the plugin is loaded via auto-discovery.

from edgebox.plugins.github import plugin


@plugin.tool("get_timeline", description="Get the event timeline for a pull request")
def get_timeline(box, pr_id=0, limit=50):
    """Return recent events for a PR, newest first."""
    rows = box.db.query(
        "SELECT id, event_type, actor, payload, created_at "
        "FROM events WHERE pr_id = ? ORDER BY id DESC LIMIT ?",
        [pr_id, limit],
    )
    return rows


@plugin.tool("add_review_comment",
             description="Post a review comment on a file in the PR")
def add_review_comment(box, pr_id=0, path="", line=0, body="",
                       author="edgebox"):
    """Insert a review comment into local storage."""
    row_id = box.db.execute(
        "INSERT INTO review_comments (pr_id, path, line, body, author) "
        "VALUES (?, ?, ?, ?, ?)",
        [pr_id, path, line, body, author],
    )
    return {"comment_id": row_id, "pr_id": pr_id}


@plugin.tool("get_diff_summary",
             description="Get a summary of files changed in the PR")
def get_diff_summary(box, pr_id=0):
    """Summarize the PR by counting events and comments."""
    event_count = box.db.query(
        "SELECT COUNT(*) as cnt FROM events WHERE pr_id = ?",
        [pr_id],
    )
    comment_count = box.db.query(
        "SELECT COUNT(*) as cnt FROM review_comments WHERE pr_id = ?",
        [pr_id],
    )

    # Get distinct event types
    event_types = box.db.query(
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


@plugin.tool("query_timeline",
             description="Run a filtered query against the event timeline")
def query_timeline(box, pr_id=0, event_type="", actor="", limit=20):
    """Query events with optional filters on type and actor."""
    sql = ("SELECT id, event_type, actor, payload, created_at "
           "FROM events WHERE pr_id = ?")
    params = [pr_id]

    if event_type:
        sql = sql + " AND event_type = ?"
        params.append(event_type)

    if actor:
        sql = sql + " AND actor = ?"
        params.append(actor)

    sql = sql + " ORDER BY id DESC LIMIT ?"
    params.append(limit)

    return box.db.query(sql, params)
