# edgebox -- AI agent toolbox runtime for the edge
#
# Public API:
#   Box, tool, alarm       -- core box class and decorators
#   EdgeboxPlugin          -- plugin blueprint (Flask-inspired)
#   PluginRegistry         -- plugin discovery and management
#   EventBus, Event        -- publish/subscribe event system
#   Settings               -- config resolution (manifest -> env -> defaults)
#   Middleware              -- request middleware base class
#   PluginConfig            -- plugin metadata base class
#   Tool, IngressHandler    -- type descriptors
#   BoxDB                  -- SQLite storage wrapper
#   BoxRequest             -- HTTP request abstraction
#   get_current_box        -- context-local box access (Flask current_app)
#   create_plugin          -- plugin factory function

__version__ = "0.2.0"

from edgebox.box import Box, tool, alarm
from edgebox.plugin import EdgeboxPlugin, PluginRegistry, create_plugin, get_current_box
from edgebox.events import EventBus
from edgebox.types import (
    Tool,
    IngressHandler,
    EventListener,
    Middleware,
    PluginConfig,
    Event,
)
from edgebox.settings import Settings
from edgebox.db import BoxDB
from edgebox.http import BoxRequest
