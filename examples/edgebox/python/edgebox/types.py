# edgebox/types.py -- Core types for the plugin framework
#
# Defines the data structures used by plugins to register tools,
# ingress handlers, event listeners, and middleware. All types are
# simple classes with no metaclass magic -- compatible with Molt.

import json


# ---------------------------------------------------------------------------
# PluginConfig -- metadata for a plugin (Django AppConfig-inspired)
# ---------------------------------------------------------------------------

class PluginConfig:
    """Base class for plugin configuration.

    Subclass this in a plugin's __init__.py to declare metadata:

        class PluginConfig:
            name = "edgebox_github"
            verbose_name = "GitHub Integration"
            version = "1.0.0"
            default_config = {"webhook_verify": True}
    """

    name = ""
    verbose_name = ""
    version = "0.0.0"
    default_config = {}


# ---------------------------------------------------------------------------
# Tool -- a registered tool callable by MCP or HTTP
# ---------------------------------------------------------------------------

class Tool:
    """Descriptor for a tool registered by a plugin.

    Attributes:
        name:        Tool name (unique across the box).
        description: Human-readable description for MCP tools/list.
        handler:     The callable (function or bound method).
        plugin_name: Name of the plugin that registered this tool.
        input_schema: JSON Schema dict for the tool's input parameters.
    """

    def __init__(self, name, handler, description="", plugin_name="",
                 input_schema=None):
        self.name = name
        self.handler = handler
        self.description = description
        self.plugin_name = plugin_name
        self.input_schema = input_schema if input_schema is not None else {
            "type": "object",
            "properties": {},
        }

    def to_mcp_dict(self):
        """Return the MCP tools/list representation."""
        return {
            "name": self.name,
            "description": self.description,
            "inputSchema": self.input_schema,
        }


# ---------------------------------------------------------------------------
# IngressHandler -- an HTTP route handler registered by a plugin
# ---------------------------------------------------------------------------

class IngressHandler:
    """Descriptor for an HTTP ingress handler registered by a plugin.

    Attributes:
        method:      HTTP method (GET, POST, etc.).
        path:        URL path (e.g. "/webhook").
        handler:     The callable(box, request) -> response.
        plugin_name: Name of the plugin that registered this handler.
    """

    def __init__(self, method, path, handler, plugin_name=""):
        self.method = method
        self.path = path
        self.handler = handler
        self.plugin_name = plugin_name


# ---------------------------------------------------------------------------
# EventListener -- a callback for events from the event bus
# ---------------------------------------------------------------------------

class EventListener:
    """Descriptor for an event listener registered by a plugin.

    Attributes:
        pattern:     Event pattern with optional wildcards (e.g. "*.push").
        handler:     The callable(box, event) -> None.
        plugin_name: Name of the plugin that registered this listener.
        priority:    Ordering hint (lower runs first, default 100).
    """

    def __init__(self, pattern, handler, plugin_name="", priority=100):
        self.pattern = pattern
        self.handler = handler
        self.plugin_name = plugin_name
        self.priority = priority


# ---------------------------------------------------------------------------
# Middleware -- wraps every request through the box
# ---------------------------------------------------------------------------

class Middleware:
    """Base class for plugin middleware.

    Subclass and override before_request / after_request:

        class AuthMiddleware(Middleware):
            def before_request(self, box, req):
                if not req.header("Authorization"):
                    return {"error": "unauthorized"}
                return None  # continue processing

            def after_request(self, box, req, response):
                return response  # or modify it
    """

    name = ""
    plugin_name = ""
    priority = 100  # lower runs first

    def before_request(self, box, req):
        """Called before dispatch. Return a response to short-circuit,
        or None to continue."""
        return None

    def after_request(self, box, req, response):
        """Called after dispatch. May modify and return the response."""
        return response


# ---------------------------------------------------------------------------
# Event -- a typed event passed through the event bus
# ---------------------------------------------------------------------------

class Event:
    """An event that flows through the event bus.

    Attributes:
        name:    Dotted event name (e.g. "github.push", "pr.opened").
        data:    Arbitrary event payload dict.
        source:  Name of the plugin or system that emitted the event.
    """

    def __init__(self, name, data=None, source=""):
        self.name = name
        self.data = data if data is not None else {}
        self.source = source
        self._stopped = False

    def stop_propagation(self):
        """Prevent further listeners from receiving this event."""
        self._stopped = True

    @property
    def is_stopped(self):
        return self._stopped
