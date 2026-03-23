# edgebox/plugins/github -- GitHub integration plugin
#
# Refactored from boxes/github_pr/ into the edgebox plugin framework.
# This plugin tracks pull-request activity via webhook ingress, stores
# events in SQLite, and exposes MCP tools for AI agents.

from edgebox.plugin import EdgeboxPlugin
from edgebox.types import PluginConfig


# ---------------------------------------------------------------------------
# Plugin config (Django AppConfig equivalent)
# ---------------------------------------------------------------------------

class GithubPluginConfig(PluginConfig):
    name = "github"
    verbose_name = "GitHub Integration"
    version = "1.0.0"
    default_config = {
        "webhook_verify": True,
        "stale_days": 7,
        "db_path": "github_pr.db",
    }


# ---------------------------------------------------------------------------
# Plugin instance (Flask Blueprint equivalent)
# ---------------------------------------------------------------------------

plugin = EdgeboxPlugin("github", __name__, config_class=GithubPluginConfig)
