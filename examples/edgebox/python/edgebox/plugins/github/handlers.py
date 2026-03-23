# edgebox/plugins/github/handlers.py -- HTTP ingress handlers
#
# Registers webhook and health endpoints on the github plugin.

import json

from edgebox.plugins.github import plugin


@plugin.handler("POST", "/webhook")
def handle_webhook(box, req):
    """Handle an inbound GitHub webhook event.

    Expects a JSON body with at least:
        action   - the event action (opened, closed, pushed, etc.)
        pr_id    - the pull request number
        sender   - the actor login

    The full payload is stored verbatim for later querying.
    After storing, emits an event on the bus: "github.<action>".
    """
    body = req.json()
    if body is None:
        return json.dumps({"error": "empty webhook body"})

    action = body.get("action", "unknown")
    pr_id = body.get("pr_id", 0)
    sender = body.get("sender", "")
    payload_str = json.dumps(body)

    box.db.execute(
        "INSERT INTO events (pr_id, event_type, actor, payload) "
        "VALUES (?, ?, ?, ?)",
        [pr_id, action, sender, payload_str],
    )

    # Emit event so other plugins can react
    from edgebox.types import Event
    box.emit_event(Event(
        "github." + action,
        data={"pr_id": pr_id, "sender": sender, "payload": body},
        source="github",
    ))

    return json.dumps({"ok": True, "event": action, "pr_id": pr_id})
