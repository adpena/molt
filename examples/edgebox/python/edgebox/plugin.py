# edgebox/plugin.py -- Plugin framework (Flask Blueprint + Django discovery)
#
# EdgeboxPlugin is the unit of composition. Each plugin registers tools,
# ingress handlers, event listeners, and middleware via decorators.
# PluginRegistry manages discovery and loading of installed plugins.
#
# No metaclasses, no exec/eval, no monkey-patching.

import json
import os
import sys

from edgebox.types import (
    Tool, IngressHandler, EventListener, Middleware, PluginConfig, Event,
)
from edgebox.events import EventBus
from edgebox.settings import Settings


# ---------------------------------------------------------------------------
# EdgeboxPlugin -- the Blueprint
# ---------------------------------------------------------------------------

class EdgeboxPlugin:
    """A plugin that groups tools, handlers, events, and middleware.

    Inspired by Flask Blueprints. Create one per plugin module:

        github = EdgeboxPlugin("github", __name__)

        @github.tool("get_timeline")
        def get_timeline(box, limit=50):
            ...

        @github.handler("POST", "/webhook")
        def handle_webhook(box, request):
            ...

        @github.on_event("*.push")
        def on_push(box, event):
            ...
    """

    def __init__(self, name, import_name="", config_class=None):
        self.name = name
        self.import_name = import_name

        # Plugin config (Django AppConfig equivalent)
        if config_class is not None:
            self.config = config_class
        else:
            self.config = PluginConfig

        self._tools = []           # list of Tool
        self._handlers = []        # list of IngressHandler
        self._event_listeners = [] # list of (pattern, fn, priority)
        self._middleware = []       # list of Middleware instances
        self._setup_hooks = []     # list of callable(box, settings)
        self._teardown_hooks = []  # list of callable(box)

    # -- tool decorator -----------------------------------------------------

    def tool(self, name, description="", input_schema=None):
        """Register a function as a tool.

        Usage:
            @plugin.tool("get_timeline", description="Get PR timeline")
            def get_timeline(box, pr_id=0, limit=50):
                ...
        """
        def decorator(fn):
            t = Tool(
                name=name,
                handler=fn,
                description=description,
                plugin_name=self.name,
                input_schema=input_schema,
            )
            self._tools.append(t)
            return fn
        return decorator

    # -- handler decorator --------------------------------------------------

    def handler(self, method, path):
        """Register a function as an HTTP ingress handler.

        Usage:
            @plugin.handler("POST", "/webhook")
            def handle_webhook(box, request):
                ...
        """
        def decorator(fn):
            h = IngressHandler(
                method=method,
                path=path,
                handler=fn,
                plugin_name=self.name,
            )
            self._handlers.append(h)
            return fn
        return decorator

    # -- event decorator ----------------------------------------------------

    def on_event(self, pattern, priority=100):
        """Register a function as an event listener.

        Usage:
            @plugin.on_event("github.push")
            def on_push(box, event):
                ...

            @plugin.on_event("*.push", priority=50)
            def on_any_push(box, event):
                ...
        """
        def decorator(fn):
            self._event_listeners.append((pattern, fn, priority))
            return fn
        return decorator

    # -- middleware ----------------------------------------------------------

    def add_middleware(self, middleware_instance):
        """Register a middleware instance.

        Usage:
            class AuthMiddleware(Middleware):
                def before_request(self, box, req):
                    ...

            plugin.add_middleware(AuthMiddleware())
        """
        middleware_instance.plugin_name = self.name
        self._middleware.append(middleware_instance)

    # -- lifecycle hooks ----------------------------------------------------

    def on_setup(self, fn):
        """Register a setup hook called when the plugin is loaded into a box.

        Usage:
            @plugin.on_setup
            def setup(box, settings):
                box.db.execute_schema(open("schema.sql").read())
        """
        self._setup_hooks.append(fn)
        return fn

    def on_teardown(self, fn):
        """Register a teardown hook called when the box shuts down.

        Usage:
            @plugin.on_teardown
            def teardown(box):
                box.db.close()
        """
        self._teardown_hooks.append(fn)
        return fn


# ---------------------------------------------------------------------------
# Plugin factory -- create_plugin(config) pattern (Flask-inspired)
# ---------------------------------------------------------------------------

def create_plugin(name, config_class=None):
    """Factory function to create a new plugin instance.

    Usage:
        github = create_plugin("github", config_class=GithubConfig)
    """
    return EdgeboxPlugin(name, config_class=config_class)


# ---------------------------------------------------------------------------
# PluginRegistry -- manages installed plugins (Django INSTALLED_APPS)
# ---------------------------------------------------------------------------

class PluginRegistry:
    """Registry that discovers, loads, and indexes all installed plugins.

    Handles:
        - Auto-discovery of tools.py, handlers.py, events.py from plugins
        - Merging tools/handlers/events into the box
        - Config resolution via Settings
        - Lifecycle management (setup/teardown)
    """

    # The well-known module names to auto-import from each plugin package
    DISCOVERY_MODULES = ["tools", "handlers", "events"]

    def __init__(self):
        self._plugins = {}       # name -> EdgeboxPlugin
        self._load_order = []    # list of plugin names in load order
        self._tools = {}         # tool_name -> Tool
        self._handlers = []      # list of IngressHandler
        self._middleware = []     # list of Middleware (sorted by priority)
        self._event_bus = EventBus()
        self._settings = Settings()

    @property
    def event_bus(self):
        return self._event_bus

    @property
    def settings(self):
        return self._settings

    # -- registration -------------------------------------------------------

    def register(self, plugin):
        """Register an EdgeboxPlugin instance.

        This indexes the plugin's tools, handlers, events, and middleware
        into the registry's central collections.
        """
        if plugin.name in self._plugins:
            return  # already registered, skip

        self._plugins[plugin.name] = plugin
        self._load_order.append(plugin.name)

        # Register default config
        if hasattr(plugin.config, "default_config"):
            self._settings.register_defaults(
                plugin.name, plugin.config.default_config
            )

        # Index tools
        idx = 0
        while idx < len(plugin._tools):
            tool = plugin._tools[idx]
            idx = idx + 1
            if tool.name in self._tools:
                existing = self._tools[tool.name]
                raise ValueError(
                    "Tool name conflict: '" + tool.name
                    + "' registered by both '" + existing.plugin_name
                    + "' and '" + tool.plugin_name + "'"
                )
            self._tools[tool.name] = tool

        # Index handlers
        idx = 0
        while idx < len(plugin._handlers):
            self._handlers.append(plugin._handlers[idx])
            idx = idx + 1

        # Index event listeners
        idx = 0
        while idx < len(plugin._event_listeners):
            pattern, fn, priority = plugin._event_listeners[idx]
            idx = idx + 1
            self._event_bus.on(pattern, fn, plugin_name=plugin.name,
                               priority=priority)

        # Index middleware
        idx = 0
        while idx < len(plugin._middleware):
            self._middleware.append(plugin._middleware[idx])
            idx = idx + 1
        self._sort_middleware()

    # -- discovery ----------------------------------------------------------

    def discover(self, plugin_module_path):
        """Auto-discover and register a plugin from a dotted module path.

        Given "edgebox.plugins.github", this will:
            1. Import edgebox.plugins.github (the __init__.py)
            2. Look for a `plugin` attribute (an EdgeboxPlugin instance)
            3. Import edgebox.plugins.github.tools (if it exists)
            4. Import edgebox.plugins.github.handlers (if it exists)
            5. Import edgebox.plugins.github.events (if it exists)
            6. Register the plugin

        The sub-modules (tools, handlers, events) typically use decorators
        on the plugin instance created in __init__.py, so importing them
        is sufficient to register their handlers.
        """
        # Import the main package
        mod = _import_module(plugin_module_path)
        if mod is None:
            raise ImportError("Cannot import plugin: " + plugin_module_path)

        # Auto-import sub-modules (tools.py, handlers.py, events.py)
        didx = 0
        while didx < len(self.DISCOVERY_MODULES):
            sub_name = self.DISCOVERY_MODULES[didx]
            didx = didx + 1
            sub_path = plugin_module_path + "." + sub_name
            _import_module(sub_path)  # ignore if not found

        # Find the plugin instance
        plugin = getattr(mod, "plugin", None)
        if plugin is None:
            # Try looking for any EdgeboxPlugin attribute
            attrs = dir(mod)
            aidx = 0
            while aidx < len(attrs):
                attr_name = attrs[aidx]
                aidx = aidx + 1
                obj = getattr(mod, attr_name, None)
                if isinstance(obj, EdgeboxPlugin):
                    plugin = obj
                    break

        if plugin is None:
            raise ValueError(
                "No EdgeboxPlugin instance found in " + plugin_module_path
                + ". Define a `plugin` variable in __init__.py."
            )

        self.register(plugin)
        return plugin

    def discover_all(self, plugin_paths):
        """Discover and register multiple plugins from a list of paths.

        Args:
            plugin_paths: List of dotted module paths, e.g.:
                ["edgebox.plugins.github", "edgebox.plugins.slack"]
        """
        idx = 0
        while idx < len(plugin_paths):
            self.discover(plugin_paths[idx])
            idx = idx + 1

    # -- setup / teardown ---------------------------------------------------

    def setup_all(self, box):
        """Run setup hooks for all registered plugins, in load order."""
        idx = 0
        while idx < len(self._load_order):
            name = self._load_order[idx]
            idx = idx + 1
            plugin = self._plugins[name]
            hidx = 0
            while hidx < len(plugin._setup_hooks):
                plugin._setup_hooks[hidx](box, self._settings)
                hidx = hidx + 1

    def teardown_all(self, box):
        """Run teardown hooks for all registered plugins, in reverse order."""
        idx = len(self._load_order) - 1
        while idx >= 0:
            name = self._load_order[idx]
            idx = idx - 1
            plugin = self._plugins[name]
            hidx = 0
            while hidx < len(plugin._teardown_hooks):
                plugin._teardown_hooks[hidx](box)
                hidx = hidx + 1

    # -- lookup -------------------------------------------------------------

    def get_tool(self, name):
        """Look up a tool by name. Returns Tool or None."""
        return self._tools.get(name)

    def list_tools(self):
        """Return all registered tools as a list of Tool objects."""
        return list(self._tools.values())

    def get_handler(self, method, path):
        """Find an ingress handler matching the given method and path."""
        idx = 0
        while idx < len(self._handlers):
            h = self._handlers[idx]
            idx = idx + 1
            if h.method == method and h.path == path:
                return h
        return None

    def get_plugin(self, name):
        """Look up a plugin by name. Returns EdgeboxPlugin or None."""
        return self._plugins.get(name)

    def list_plugins(self):
        """Return names of all registered plugins in load order."""
        return list(self._load_order)

    # -- middleware ----------------------------------------------------------

    def run_middleware_before(self, box, req):
        """Run all before_request middleware. Returns response or None."""
        idx = 0
        while idx < len(self._middleware):
            mw = self._middleware[idx]
            idx = idx + 1
            result = mw.before_request(box, req)
            if result is not None:
                return result  # short-circuit
        return None

    def run_middleware_after(self, box, req, response):
        """Run all after_request middleware. Returns final response."""
        idx = 0
        while idx < len(self._middleware):
            mw = self._middleware[idx]
            idx = idx + 1
            response = mw.after_request(box, req, response)
        return response

    def _sort_middleware(self):
        """Sort middleware by priority (insertion sort)."""
        i = 1
        while i < len(self._middleware):
            current = self._middleware[i]
            j = i - 1
            while j >= 0 and self._middleware[j].priority > current.priority:
                self._middleware[j + 1] = self._middleware[j]
                j = j - 1
            self._middleware[j + 1] = current
            i = i + 1


# ---------------------------------------------------------------------------
# Context local -- current_box proxy (Flask current_app inspired)
# ---------------------------------------------------------------------------

# Module-level reference to the active box. Set by Box.__init__ or
# Box.dispatch. Avoids thread-local / contextvars complexity -- Molt
# runs single-threaded per invocation anyway.

_current_box = None


def _set_current_box(box):
    """Set the active box (called by Box internally)."""
    global _current_box
    _current_box = box


def get_current_box():
    """Get the currently active Box instance.

    Returns None if no box is active. In handler/tool code this will
    always return the box that is processing the current request.
    """
    return _current_box


# ---------------------------------------------------------------------------
# Module import helper
# ---------------------------------------------------------------------------

def _import_module(dotted_path):
    """Import a module by dotted path, returning it or None on failure.

    Uses __import__ with fromlist to get the leaf module.
    No eval or exec -- just __import__.
    """
    try:
        # __import__("a.b.c", fromlist=["c"]) returns the "c" module
        parts = dotted_path.split(".")
        last = parts[len(parts) - 1]
        mod = __import__(dotted_path, fromlist=[last])
        return mod
    except ImportError:
        return None
    except Exception:
        return None
