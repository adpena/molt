# edgebox/box.py -- Base Box class with plugin registry and event bus
#
# A Box is a self-contained unit of logic that exposes tools (callable
# endpoints) and alarms (scheduled callbacks). The dispatch() method
# reads the inbound request path and routes to the correct handler.
#
# Now integrates with the plugin framework: tools and handlers can come
# from both the Box subclass (via decorators) and from installed plugins.

import json
import sys

from edgebox.plugin import PluginRegistry, _set_current_box
from edgebox.events import EventBus
from edgebox.settings import Settings, load_manifest_config, load_manifest_plugins
from edgebox.types import Event


# ---------------------------------------------------------------------------
# Decorator: @tool(name=..., description=...)
# Marks a method as an MCP-callable tool.
# ---------------------------------------------------------------------------

def tool(name="", description=""):
    """Decorator that registers a method as a Box tool."""

    def decorator(fn):
        fn._tool_name = name
        fn._tool_description = description
        fn._is_tool = True
        return fn

    return decorator


# ---------------------------------------------------------------------------
# Decorator: @alarm(name)
# Marks a method as a scheduled alarm handler.
# ---------------------------------------------------------------------------

def alarm(name):
    """Decorator that registers a method as a Box alarm handler."""

    def decorator(fn):
        fn._alarm_name = name
        fn._is_alarm = True
        return fn

    return decorator


# ---------------------------------------------------------------------------
# Base Box class
# ---------------------------------------------------------------------------

class Box:
    """Base class for all edgebox boxes.

    Subclasses register tools and alarms via decorators, then call
    dispatch() to route an inbound request to the right handler.

    The Box now maintains a PluginRegistry for loading plugins and an
    EventBus for publish/subscribe event handling.

    Usage with plugins:
        class MyBox(Box):
            def __init__(self):
                super().__init__(manifest={
                    "plugins": ["edgebox.plugins.github"],
                    "config": {"github": {"stale_days": 14}},
                })

    Usage without plugins (backwards compatible):
        class MyBox(Box):
            def __init__(self):
                super().__init__()

            @tool(name="hello")
            def hello(self):
                return "world"
    """

    def __init__(self, manifest=None):
        self._tools = {}
        self._alarms = {}
        self._registry = PluginRegistry()

        # Set this box as the current box for context-local access
        _set_current_box(self)

        # Collect tools/alarms defined on the class via decorators
        self._collect_handlers()

        # Load plugins from manifest if provided
        if manifest is not None:
            self._load_manifest(manifest)

    @property
    def registry(self):
        """Access the plugin registry."""
        return self._registry

    @property
    def event_bus(self):
        """Access the event bus."""
        return self._registry.event_bus

    @property
    def settings(self):
        """Access the settings."""
        return self._registry.settings

    # -- plugin loading -----------------------------------------------------

    def _load_manifest(self, manifest):
        """Load plugins and config from a manifest dict."""
        # Apply manifest config to settings
        config = load_manifest_config(manifest)
        self._registry._settings = Settings(manifest_config=config)

        # Discover and register plugins
        plugin_paths = load_manifest_plugins(manifest)
        self._registry.discover_all(plugin_paths)

        # Run setup hooks
        self._registry.setup_all(self)

    def install_plugin(self, plugin):
        """Manually register a plugin instance (no auto-discovery)."""
        self._registry.register(plugin)

    # -- event bus shorthand ------------------------------------------------

    def emit_event(self, event):
        """Emit an event on the event bus.

        Args:
            event: An Event instance, or a string event name.

        Returns:
            The Event instance.
        """
        return self._registry.event_bus.emit(self, event)

    def on_event(self, pattern, handler, priority=100):
        """Register an event listener directly on the box."""
        return self._registry.event_bus.on(pattern, handler, priority=priority)

    # -- introspection ------------------------------------------------------

    def _collect_handlers(self):
        """Walk the class hierarchy and index all @tool / @alarm methods."""
        attrs = dir(self)
        idx = 0
        while idx < len(attrs):
            attr_name = attrs[idx]
            idx = idx + 1
            method = getattr(self, attr_name, None)
            if method is None:
                continue
            if getattr(method, "_is_tool", False):
                self._tools[method._tool_name] = method
            if getattr(method, "_is_alarm", False):
                self._alarms[method._alarm_name] = method

    def list_tools(self):
        """Return a list of tool descriptors for MCP tools/list.

        Merges tools from both the Box subclass and installed plugins.
        """
        result = []

        # Box-level tools (from @tool decorator on methods)
        names = list(self._tools.keys())
        idx = 0
        while idx < len(names):
            name = names[idx]
            method = self._tools[name]
            result.append({
                "name": name,
                "description": getattr(method, "_tool_description", ""),
            })
            idx = idx + 1

        # Plugin tools
        plugin_tools = self._registry.list_tools()
        idx = 0
        while idx < len(plugin_tools):
            t = plugin_tools[idx]
            idx = idx + 1
            result.append({
                "name": t.name,
                "description": t.description,
            })

        return result

    def list_alarms(self):
        """Return a list of alarm names."""
        return list(self._alarms.keys())

    def list_plugins(self):
        """Return names of all installed plugins."""
        return self._registry.list_plugins()

    # -- dispatch -----------------------------------------------------------

    def dispatch(self):
        """Read the request path and route to the appropriate handler.

        Routing rules:
            /mcp          -> MCP JSON-RPC endpoint (handled by mcp module)
            /webhook      -> on_webhook() hook or plugin handler
            /alarm/<name> -> named alarm handler
            /tool/<name>  -> named tool handler (direct HTTP call)
            /health       -> simple health check
            /*            -> try plugin ingress handlers

        Middleware is run before and after dispatch for all routes.
        """
        # Set current box for context-local access
        _set_current_box(self)

        # Import here to get request context
        from edgebox.http import BoxRequest

        req = BoxRequest.from_env()
        path = req.path
        method = req.method

        # Run before-request middleware
        mw_response = self._registry.run_middleware_before(self, req)
        if mw_response is not None:
            return self._apply_after_middleware(req, mw_response)

        # Health check
        if path == "/health":
            response = json.dumps({"status": "ok"})
            return self._apply_after_middleware(req, response)

        # MCP endpoint
        if path == "/mcp":
            from edgebox.mcp import handle_mcp
            response = handle_mcp(self, req)
            return self._apply_after_middleware(req, response)

        # Alarm dispatch: /alarm/<name>
        if path.startswith("/alarm/"):
            alarm_name = path[7:]  # len("/alarm/") == 7
            handler = self._alarms.get(alarm_name)
            if handler is None:
                response = json.dumps({"error": "unknown alarm: " + alarm_name})
                return self._apply_after_middleware(req, response)
            result = handler()
            response = json.dumps({"ok": True, "result": result})
            return self._apply_after_middleware(req, response)

        # Tool dispatch: /tool/<name>
        if path.startswith("/tool/"):
            tool_name = path[6:]  # len("/tool/") == 6
            response = self._dispatch_tool(tool_name, req)
            return self._apply_after_middleware(req, response)

        # Try plugin ingress handlers
        plugin_handler = self._registry.get_handler(method, path)
        if plugin_handler is not None:
            response = plugin_handler.handler(self, req)
            return self._apply_after_middleware(req, response)

        # Legacy webhook hook (backwards compat)
        if path == "/webhook":
            response = self.on_webhook(req)
            return self._apply_after_middleware(req, response)

        response = json.dumps({"error": "not found", "path": path})
        return self._apply_after_middleware(req, response)

    def _dispatch_tool(self, tool_name, req):
        """Dispatch a tool call by name -- checks both Box and plugin tools."""
        # Parse arguments from request body
        args = {}
        if req.body:
            args = json.loads(req.body)

        # Check Box-level tools first
        handler = self._tools.get(tool_name)
        if handler is not None:
            result = handler(**args)
            return json.dumps({"ok": True, "result": result})

        # Check plugin tools
        plugin_tool = self._registry.get_tool(tool_name)
        if plugin_tool is not None:
            result = plugin_tool.handler(self, **args)
            return json.dumps({"ok": True, "result": result})

        return json.dumps({"error": "unknown tool: " + tool_name})

    def _apply_after_middleware(self, req, response):
        """Run after-request middleware."""
        return self._registry.run_middleware_after(self, req, response)

    # -- hooks (override in subclass) ---------------------------------------

    def on_webhook(self, req):
        """Override in subclass to handle inbound webhooks."""
        return json.dumps({"error": "on_webhook not implemented"})
