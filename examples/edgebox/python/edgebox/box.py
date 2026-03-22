# edgebox/box.py -- Base Box class with @tool and @alarm decorators
#
# A Box is a self-contained unit of logic that exposes tools (callable
# endpoints) and alarms (scheduled callbacks). The dispatch() method
# reads the inbound request path and routes to the correct handler.

import json
import sys


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
    """

    def __init__(self):
        self._tools = {}
        self._alarms = {}
        self._collect_handlers()

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
        """Return a list of tool descriptors for MCP tools/list."""
        result = []
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
        return result

    def list_alarms(self):
        """Return a list of alarm names."""
        return list(self._alarms.keys())

    # -- dispatch -----------------------------------------------------------

    def dispatch(self):
        """Read the request path and route to the appropriate handler.

        Routing rules:
            /mcp          -> MCP JSON-RPC endpoint (handled by mcp module)
            /webhook      -> on_webhook() hook
            /alarm/<name> -> named alarm handler
            /tool/<name>  -> named tool handler (direct HTTP call)
            /health       -> simple health check
        """
        # Import here to get request context
        from edgebox.http import BoxRequest

        req = BoxRequest.from_env()
        path = req.path

        # Health check
        if path == "/health":
            return json.dumps({"status": "ok"})

        # MCP endpoint
        if path == "/mcp":
            from edgebox.mcp import handle_mcp
            return handle_mcp(self, req)

        # Webhook ingress
        if path == "/webhook":
            return self.on_webhook(req)

        # Alarm dispatch: /alarm/<name>
        if path.startswith("/alarm/"):
            alarm_name = path[7:]  # len("/alarm/") == 7
            handler = self._alarms.get(alarm_name)
            if handler is None:
                return json.dumps({"error": "unknown alarm: " + alarm_name})
            result = handler()
            return json.dumps({"ok": True, "result": result})

        # Tool dispatch: /tool/<name>
        if path.startswith("/tool/"):
            tool_name = path[6:]  # len("/tool/") == 6
            handler = self._tools.get(tool_name)
            if handler is None:
                return json.dumps({"error": "unknown tool: " + tool_name})
            # Parse arguments from request body
            args = {}
            if req.body:
                args = json.loads(req.body)
            result = handler(**args)
            return json.dumps({"ok": True, "result": result})

        return json.dumps({"error": "not found", "path": path})

    # -- hooks (override in subclass) ---------------------------------------

    def on_webhook(self, req):
        """Override in subclass to handle inbound webhooks."""
        return json.dumps({"error": "on_webhook not implemented"})
