# Edgebox Plugin Author Guide

This guide covers everything you need to build, test, and publish an edgebox plugin. If you have written a Flask Blueprint or a Django app, the patterns will feel familiar.

## Plugin Directory Structure

A plugin is a standard Python package. The registry auto-discovers three well-known modules -- `tools.py`, `handlers.py`, and `events.py` -- just like Django discovers `models.py` and `admin.py`.

```
edgebox-plugin-acme/
    edgebox_acme/
        __init__.py       # Plugin instance + config class
        tools.py          # @plugin.tool() definitions
        handlers.py       # @plugin.handler() HTTP ingress routes
        events.py         # @plugin.on_event() listeners
        schema.sql        # DDL for plugin tables (optional)
    pyproject.toml
    tests/
        test_tools.py
        test_handlers.py
```

The `__init__.py` is the only required file. Everything else is optional -- include only what your plugin needs.

## The Plugin Interface

### EdgeboxPlugin class

Create a single `EdgeboxPlugin` instance in your package's `__init__.py` and assign it to a variable named `plugin`. The registry looks for this name during auto-discovery.

```python
# edgebox_acme/__init__.py
from edgebox.plugin import EdgeboxPlugin
from edgebox.types import PluginConfig


class AcmeConfig(PluginConfig):
    name = "acme"
    verbose_name = "Acme Notifications"
    version = "1.0.0"
    default_config = {
        "api_key": "",
        "timeout_ms": 5000,
        "retry_count": 3,
    }


plugin = EdgeboxPlugin("acme", __name__, config_class=AcmeConfig)
```

**Key points:**

- The first argument to `EdgeboxPlugin()` is the plugin name. It must be unique across all plugins loaded into a box.
- `config_class` is optional. If provided, its `default_config` dict registers fallback values that users can override via manifest config or environment variables.
- The `plugin` variable name is a convention. The registry also scans for any `EdgeboxPlugin` instance in the module, but naming it `plugin` is clearest.

### PluginConfig

`PluginConfig` is the Django `AppConfig` equivalent. Subclass it to declare metadata:

```python
class AcmeConfig(PluginConfig):
    name = "acme"                  # Machine name (must match EdgeboxPlugin name)
    verbose_name = "Acme Service"  # Human-readable name
    version = "1.0.0"             # Plugin version
    default_config = {             # Default settings (overridable)
        "api_key": "",
        "timeout_ms": 5000,
    }
```

Users override these defaults via the manifest `config:` section or environment variables following the pattern `EDGEBOX_<PLUGIN>_<KEY>` (e.g., `EDGEBOX_ACME_API_KEY=sk-123`).

## @plugin.tool() -- Register MCP-exposed tools

Tools are the primary interface between AI agents and your plugin. Each tool is callable via the MCP `tools/call` method and also via direct HTTP `POST /tool/<name>`.

```python
# edgebox_acme/tools.py
from edgebox_acme import plugin


@plugin.tool("send_notification",
             description="Send a notification to an Acme channel",
             input_schema={
                 "type": "object",
                 "properties": {
                     "channel": {"type": "string", "description": "Target channel"},
                     "message": {"type": "string", "description": "Message body"},
                     "priority": {"type": "string", "enum": ["low", "normal", "high"]},
                 },
                 "required": ["channel", "message"],
             })
def send_notification(box, channel="", message="", priority="normal"):
    """Send a notification. Returns confirmation with message ID."""
    api_key = box.settings.get("acme", "api_key")
    if not api_key:
        return {"error": "acme.api_key not configured"}

    # Store locally for audit trail
    row_id = box.db.execute(
        "INSERT INTO acme_notifications (channel, message, priority) VALUES (?, ?, ?)",
        [channel, message, priority],
    )

    # Emit event so other plugins can react
    from edgebox.types import Event
    box.emit_event(Event("acme.notification_sent", data={
        "channel": channel,
        "message_id": row_id,
    }, source="acme"))

    return {"ok": True, "message_id": row_id}


@plugin.tool("list_channels", description="List available notification channels")
def list_channels(box):
    """Return known channels from local storage."""
    return box.db.query("SELECT name, description FROM acme_channels ORDER BY name")
```

### Tool function signature

Every tool function receives `box` as its first argument -- the active `Box` instance. All other arguments must have defaults so they can be called with partial parameters from an agent.

```python
@plugin.tool("my_tool", description="Does a thing")
def my_tool(box, required_arg="", optional_arg=42):
    # box.db, box.settings, box.emit_event() are all available
    ...
```

### input_schema

The `input_schema` parameter is optional. If provided, it must be a JSON Schema dict describing the tool's input. This schema is returned verbatim in the MCP `tools/list` response so agents understand what arguments are expected. If omitted, a permissive `{"type": "object", "properties": {}}` schema is used.

## @plugin.handler() -- Register HTTP ingress handlers

Handlers receive inbound HTTP requests at specific method+path combinations. Use them for webhooks, API callbacks, or custom endpoints.

```python
# edgebox_acme/handlers.py
import json
from edgebox_acme import plugin


@plugin.handler("POST", "/acme/webhook")
def handle_acme_webhook(box, req):
    """Receive an inbound webhook from the Acme service."""
    body = req.json()
    if body is None:
        return json.dumps({"error": "empty body"})

    event_type = body.get("type", "unknown")

    # Store the event
    box.db.execute(
        "INSERT INTO acme_events (event_type, payload) VALUES (?, ?)",
        [event_type, json.dumps(body)],
    )

    # Emit on the event bus
    from edgebox.types import Event
    box.emit_event(Event("acme." + event_type, data=body, source="acme"))

    return json.dumps({"ok": True, "event": event_type})


@plugin.handler("GET", "/acme/status")
def acme_status(box, req):
    """Return plugin health status."""
    count = box.db.query("SELECT COUNT(*) as cnt FROM acme_events")
    return json.dumps({
        "plugin": "acme",
        "total_events": count[0]["cnt"] if count else 0,
    })
```

### Handler function signature

```python
def my_handler(box, req):
    # box  -- the active Box instance
    # req  -- a BoxRequest with .method, .path, .headers, .body, .params, .json()
    return json.dumps({"ok": True})  # must return a string
```

### BoxRequest attributes

| Attribute | Type | Description |
|-----------|------|-------------|
| `method` | str | HTTP method (GET, POST, etc.) |
| `path` | str | Request path (e.g., "/acme/webhook") |
| `headers` | dict | Header name to value mapping |
| `body` | str | Raw request body |
| `params` | dict | Parsed query parameters |
| `json()` | method | Parse body as JSON, returns dict or None |
| `header(name)` | method | Case-insensitive header lookup |

## @plugin.on_event() -- Subscribe to events

The event bus lets plugins communicate without direct imports. Events use dotted names with wildcard pattern matching.

```python
# edgebox_acme/events.py
from edgebox_acme import plugin


@plugin.on_event("github.opened", priority=80)
def on_pr_opened(box, event):
    """When a PR is opened, send a notification to the team channel."""
    pr_id = event.data.get("pr_id", 0)
    sender = event.data.get("sender", "unknown")

    box.db.execute(
        "INSERT INTO acme_notifications (channel, message, priority) VALUES (?, ?, ?)",
        ["#code-review", sender + " opened PR #" + str(pr_id), "normal"],
    )


@plugin.on_event("*.error", priority=10)
def on_any_error(box, event):
    """Catch all error events from any plugin."""
    box.db.execute(
        "INSERT INTO acme_notifications (channel, message, priority) VALUES (?, ?, ?)",
        ["#alerts", "Error in " + event.source + ": " + event.name, "high"],
    )
```

### Wildcard patterns

| Pattern | Matches |
|---------|---------|
| `"github.push"` | Exact match only |
| `"github.*"` | `github.push`, `github.opened`, `github.closed`, etc. |
| `"*.push"` | `github.push`, `gitlab.push`, etc. |
| `"*"` | Everything |
| `"a.*.c"` | `a.foo.c`, `a.bar.c`, etc. (one segment per `*`) |

Each `*` matches exactly one dotted segment.

### Priority

Lower numbers run first. Default is 100. Use priorities to ensure ordering when multiple plugins listen for the same event.

```python
@plugin.on_event("github.push", priority=10)   # runs first
def critical_handler(box, event):
    ...

@plugin.on_event("github.push", priority=200)  # runs last
def logging_handler(box, event):
    ...
```

### Stopping propagation

A listener can prevent subsequent listeners from receiving the event:

```python
@plugin.on_event("github.push", priority=10)
def gatekeeper(box, event):
    if event.data.get("branch") == "experimental":
        event.stop_propagation()  # no further listeners will fire
```

## Event objects

The `Event` class carries a name, payload, and source:

```python
from edgebox.types import Event

event = Event(
    "acme.notification_sent",
    data={"channel": "#ops", "message_id": 42},
    source="acme",
)

event.name          # "acme.notification_sent"
event.data          # {"channel": "#ops", "message_id": 42}
event.source        # "acme"
event.is_stopped    # False
event.stop_propagation()
```

Emit events from tool or handler code via `box.emit_event(event)`.

## Storage API

Every box has a `BoxDB` instance at `box.db`. It wraps SQLite with simple query helpers.

### box.db.query(sql, params)

Run a SELECT and return a list of dicts:

```python
rows = box.db.query(
    "SELECT id, name FROM channels WHERE active = ?",
    [1],
)
# rows = [{"id": 1, "name": "#general"}, {"id": 2, "name": "#ops"}]
```

### box.db.execute(sql, params)

Run an INSERT, UPDATE, or DELETE. Returns `lastrowid`:

```python
row_id = box.db.execute(
    "INSERT INTO channels (name, active) VALUES (?, ?)",
    ["#new-channel", 1],
)
```

### box.db.executemany(sql, param_list)

Run a statement for each parameter set:

```python
box.db.executemany(
    "INSERT INTO channels (name, active) VALUES (?, ?)",
    [["#a", 1], ["#b", 1], ["#c", 0]],
)
```

### box.db.execute_schema(sql_text)

Execute a multi-statement DDL file. Statements are separated by the `:do:` sentinel or by standard semicolons:

```python
@plugin.on_setup
def setup(box, settings):
    box.db.execute_schema("""
        CREATE TABLE IF NOT EXISTS acme_notifications (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            channel TEXT NOT NULL,
            message TEXT NOT NULL,
            priority TEXT NOT NULL DEFAULT 'normal',
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );
        :do:
        CREATE TABLE IF NOT EXISTS acme_channels (
            name TEXT PRIMARY KEY,
            description TEXT NOT NULL DEFAULT ''
        );
        :do:
        CREATE INDEX IF NOT EXISTS idx_acme_notif_channel
            ON acme_notifications(channel);
    """)
```

### Serialization helpers

```python
from edgebox.db import BoxDB

json_str = BoxDB.to_json({"key": "value"})   # serialize for storage
data = BoxDB.from_json(json_str)              # deserialize from storage
```

### Settings access

```python
api_key = box.settings.get("acme", "api_key")
timeout = box.settings.get("acme", "timeout_ms", fallback=3000)

# Get the full resolved config for your plugin
config = box.settings.get_plugin_config("acme")
# {"api_key": "sk-...", "timeout_ms": 5000, "retry_count": 3}
```

## Middleware

Plugins can register middleware that wraps every request through the box. Middleware runs in priority order (lower first).

```python
# edgebox_acme/middleware.py
from edgebox.types import Middleware
from edgebox_acme import plugin
import json


class AcmeAuthMiddleware(Middleware):
    name = "acme_auth"
    priority = 10  # runs early

    def before_request(self, box, req):
        """Check for a valid API key on protected routes."""
        if req.path.startswith("/acme/"):
            token = req.header("Authorization")
            expected = box.settings.get("acme", "api_key")
            if token != "Bearer " + expected:
                return json.dumps({"error": "unauthorized"})
        return None  # continue processing

    def after_request(self, box, req, response):
        """Add plugin version header to responses."""
        # In a real implementation you would modify response headers.
        # For now, pass through.
        return response


plugin.add_middleware(AcmeAuthMiddleware())
```

## Lifecycle Hooks

### on_setup

Runs once when the plugin is loaded into a box. Use it to create tables or initialize state:

```python
@plugin.on_setup
def setup(box, settings):
    box.db.execute_schema("""
        CREATE TABLE IF NOT EXISTS acme_notifications (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            channel TEXT NOT NULL,
            message TEXT NOT NULL,
            priority TEXT NOT NULL DEFAULT 'normal',
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );
    """)
    # Pre-populate default channels
    default_channels = settings.get("acme", "default_channels", fallback="")
    if default_channels:
        channels = default_channels.split(",")
        idx = 0
        while idx < len(channels):
            box.db.execute(
                "INSERT OR IGNORE INTO acme_channels (name) VALUES (?)",
                [channels[idx].strip()],
            )
            idx = idx + 1
```

### on_teardown

Runs when the box shuts down. Use it to clean up resources:

```python
@plugin.on_teardown
def teardown(box):
    # Clean up temporary data
    box.db.execute("DELETE FROM acme_temp_cache")
```

## Testing Locally

### Unit testing tools

Since tools are plain functions, you can test them directly with a mock box:

```python
# tests/test_tools.py
from edgebox.box import Box
from edgebox.db import BoxDB

# Import the tool functions
from edgebox_acme.tools import send_notification, list_channels


def test_send_notification():
    box = Box()
    box.db = BoxDB(":memory:")
    box.db.execute_schema("""
        CREATE TABLE acme_notifications (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            channel TEXT, message TEXT, priority TEXT
        );
    """)
    # Configure the API key so the tool does not reject the call
    box.settings.set("acme", "api_key", "test-key")

    result = send_notification(box, channel="#test", message="hello", priority="normal")
    assert result["ok"] is True
    assert result["message_id"] > 0

    rows = box.db.query("SELECT * FROM acme_notifications")
    assert len(rows) == 1
    assert rows[0]["channel"] == "#test"
```

### Integration testing with dispatch

Test the full request routing path:

```python
import os
import sys
import json
from edgebox.box import Box
from edgebox.db import BoxDB


def test_mcp_tools_list():
    # Simulate request environment
    os.environ["EDGEBOX_METHOD"] = "POST"
    os.environ["EDGEBOX_PATH"] = "/mcp"

    box = Box(manifest={
        "plugins": ["edgebox_acme"],
        "config": {"acme": {"api_key": "test"}},
    })
    box.db = BoxDB(":memory:")

    # Set argv to the JSON-RPC body
    sys.argv = ["box.py", json.dumps({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {},
    })]

    response = json.loads(box.dispatch())
    tools = response["result"]["tools"]
    tool_names = []
    idx = 0
    while idx < len(tools):
        tool_names.append(tools[idx]["name"])
        idx = idx + 1

    assert "send_notification" in tool_names
    assert "list_channels" in tool_names
```

### Testing event handlers

```python
from edgebox.box import Box
from edgebox.db import BoxDB
from edgebox.types import Event


def test_on_pr_opened():
    box = Box(manifest={
        "plugins": ["edgebox_acme"],
        "config": {"acme": {"api_key": "test"}},
    })
    box.db = BoxDB(":memory:")
    box.db.execute_schema("""
        CREATE TABLE acme_notifications (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            channel TEXT, message TEXT, priority TEXT
        );
    """)

    # Emit the event that the handler listens for
    box.emit_event(Event("github.opened", data={"pr_id": 99, "sender": "alice"}))

    rows = box.db.query("SELECT * FROM acme_notifications")
    assert len(rows) == 1
    assert "#99" in rows[0]["message"]
```

## Publishing to PyPI

Name your package `edgebox-plugin-<name>` so users can find it. The import name should be `edgebox_<name>` (underscores).

### pyproject.toml

```toml
[project]
name = "edgebox-plugin-acme"
version = "1.0.0"
description = "Acme notification plugin for edgebox"
requires-python = ">=3.12"
dependencies = []       # edgebox itself is the runtime, not a pip dependency
keywords = ["edgebox", "plugin", "acme", "mcp"]

[project.urls]
Repository = "https://github.com/yourname/edgebox-plugin-acme"

[build-system]
requires = ["setuptools>=68"]
build-backend = "setuptools.backends._legacy:_Backend"

[tool.setuptools.packages.find]
include = ["edgebox_acme*"]
```

### Installation

Users install your plugin:

```
pip install edgebox-plugin-acme
```

Then add it to their box manifest:

```python
class MyBox(Box):
    def __init__(self):
        super().__init__(manifest={
            "plugins": ["edgebox_acme"],
        })
```

Or in YAML:

```yaml
runtime:
  engine: molt
  entrypoint: my_box.py

plugins:
  - edgebox_acme
config:
  acme:
    api_key: "sk-..."
```

## Molt Compilation Constraints

Edgebox runs on [Molt](https://github.com/adpena/molt), which compiles Python to WebAssembly. Molt targets CPython 3.12 parity with a few restrictions. Your plugin code must respect these:

### No exec, eval, or compile

These are not available in Molt. All code paths must be statically known at compile time.

```python
# Will not work
exec("x = 1")
eval("1 + 2")
compile("pass", "<string>", "exec")
```

### No metaclasses

Molt does not support custom metaclasses. Use plain classes and composition instead.

```python
# Will not work
class Meta(type):
    pass

class MyClass(metaclass=Meta):
    pass

# Do this instead
class MyClass:
    pass
```

### Use while loops instead of for loops with iterators

Molt supports `for` loops over concrete sequences (lists, tuples, ranges, dicts), but complex iterator protocols may not work. When in doubt, use `while` loops with index variables:

```python
# Preferred -- always works
items = ["a", "b", "c"]
idx = 0
while idx < len(items):
    item = items[idx]
    idx = idx + 1
    # process item

# Also works -- for over a list literal or variable
for item in items:
    # process item
```

### No unrestricted reflection

`getattr`, `setattr`, and `hasattr` work on known attributes, but patterns like dynamic dispatch tables built via `getattr(module, name)` with arbitrary runtime strings may not resolve correctly.

### No runtime monkey-patching

Do not reassign methods on classes or modules at runtime. Decorate at import time and leave it alone.

```python
# Will not work
SomeClass.method = my_replacement

# Do this instead -- register via decorators at import time
@plugin.tool("my_tool")
def my_tool(box):
    ...
```

### Safe operations

Everything else works as expected:

- Standard library modules (json, os, sys, sqlite3, etc.)
- Classes with inheritance (no metaclasses)
- Closures and nested functions
- Default arguments and keyword arguments
- String operations, list/dict comprehensions
- Exception handling (try/except/finally)
- Context managers (with statements)
- f-strings and string formatting

### Testing Molt compatibility

Compile your plugin to verify it passes the Molt compiler:

```bash
molt compile edgebox_acme/tools.py -o /dev/null --check
```

If the compiler rejects a construct, it will report the line number and a description of the unsupported feature.

## Complete Plugin Example

Here is a minimal but complete plugin that adds a "ping" tool and a webhook handler:

```
edgebox-plugin-ping/
    edgebox_ping/
        __init__.py
        tools.py
        handlers.py
    pyproject.toml
```

### edgebox_ping/__init__.py

```python
from edgebox.plugin import EdgeboxPlugin
from edgebox.types import PluginConfig


class PingConfig(PluginConfig):
    name = "ping"
    verbose_name = "Ping Plugin"
    version = "1.0.0"
    default_config = {"message": "pong"}


plugin = EdgeboxPlugin("ping", __name__, config_class=PingConfig)
```

### edgebox_ping/tools.py

```python
from edgebox_ping import plugin


@plugin.tool("ping", description="Returns a configurable pong message")
def ping(box):
    message = box.settings.get("ping", "message")
    return {"response": message}
```

### edgebox_ping/handlers.py

```python
import json
from edgebox_ping import plugin


@plugin.handler("GET", "/ping")
def handle_ping(box, req):
    message = box.settings.get("ping", "message")
    return json.dumps({"ping": message})
```

### Usage

```python
from edgebox.box import Box

class MyBox(Box):
    def __init__(self):
        super().__init__(manifest={
            "plugins": ["edgebox_ping"],
            "config": {"ping": {"message": "hello from the edge!"}},
        })

if __name__ == "__main__":
    box = MyBox()
    print(box.dispatch())
```

---

Powered by [Molt](https://github.com/adpena/molt) -- Python compiled to WebAssembly.
