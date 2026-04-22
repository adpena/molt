# edgebox/plugins/github/events.py -- Event listeners for GitHub
#
# Registers event listeners on the github plugin for cross-plugin
# event handling.


from edgebox.plugins.github import plugin


@plugin.on_event("github.push", priority=50)
def on_push(box, event):
    """Handle push events -- could trigger CI checks, notifications, etc."""
    # For now, just a hook point that other plugins or custom code
    # can extend. The event data contains the full webhook payload.
    pass


@plugin.on_event("github.opened", priority=50)
def on_pr_opened(box, event):
    """Handle PR opened events."""
    pass
