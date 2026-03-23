# edgebox

Edgebox is a plugin-based framework for building AI agent toolboxes that run at the edge. You write plain Python -- define tools, webhook handlers, and event listeners -- and edgebox compiles it to WebAssembly via [Molt](https://github.com/adpena/molt) and deploys it to Cloudflare Workers (with Vercel and local dev planned). Each "box" is a self-contained unit with its own storage, MCP endpoint, and plugin registry, designed so AI agents can discover and call your tools over the Model Context Protocol.

## Quick Start

```
# 1. Clone the repo and enter the edgebox example
git clone https://github.com/adpena/molt.git
cd molt/examples/edgebox

# 2. Write your box (or use the built-in github-pr example)
cat python/boxes/github_pr/box.py

# 3. Compile to WebAssembly with Molt
molt compile python/boxes/github_pr/box.py -o dist/github_pr.wasm

# 4. Configure your deployment target
cp manifests/github-pr.yaml manifests/my-box.yaml
# Edit identity, plugins, and limits to taste

# 5. Deploy to Cloudflare Workers
cd packages/edgebox-cloudflare
npm run deploy
```

Your box is now live. Point an AI agent at `https://your-worker.workers.dev/mcp` and it will discover your tools automatically.

## Architecture

```
                          +---------------------------+
                          |      AI Agent (Claude,    |
                          |      GPT, custom, ...)    |
                          +------------+--------------+
                                       |
                                  MCP JSON-RPC
                                  POST /mcp
                                       |
               +-----------------------v------------------------+
               |                  Cloudflare Worker             |
               |                                                |
               |   +------------------------------------------+ |
               |   |              Box (your code)              | |
               |   |                                           | |
               |   |  +----------+  +----------+  +---------+ | |
               |   |  | Plugin A |  | Plugin B |  | @tool() | | |
               |   |  | (github) |  | (slack)  |  | methods | | |
               |   |  +----+-----+  +----+-----+  +----+----+ | |
               |   |       |             |              |      | |
               |   |  +----v-------------v--------------v----+ | |
               |   |  |          PluginRegistry              | | |
               |   |  |  tools | handlers | events | mw      | | |
               |   |  +-----------------+---+----------------+ | |
               |   |                    |   |                  | |
               |   |  +-----------+   +-v---v--+               | |
               |   |  | EventBus  |   | BoxDB  |               | |
               |   |  | pub/sub   |   | SQLite |               | |
               |   |  +-----------+   +--------+               | |
               |   +------------------------------------------+ |
               |                                                |
               |   Molt WASM Runtime (Python 3.12 compiled)     |
               +------------------------------------------------+
```

## How Plugins Work

If you have used Flask Blueprints or Django apps, you already know the pattern. A plugin is a Python package that groups related tools, HTTP handlers, and event listeners under a single namespace. The box discovers and loads plugins at startup.

**Flask analogy:** `EdgeboxPlugin` is a Blueprint. You create one per plugin, then register tools and handlers with decorators.

**Django analogy:** `PluginRegistry` is `INSTALLED_APPS`. The registry auto-imports `tools.py`, `handlers.py`, and `events.py` from each plugin package -- just like Django discovers `models.py` and `admin.py`.

### Defining a plugin

```python
# edgebox/plugins/github/__init__.py
from edgebox.plugin import EdgeboxPlugin
from edgebox.types import PluginConfig

class GithubPluginConfig(PluginConfig):
    name = "github"
    verbose_name = "GitHub Integration"
    version = "1.0.0"
    default_config = {
        "webhook_verify": True,
        "stale_days": 7,
    }

plugin = EdgeboxPlugin("github", __name__, config_class=GithubPluginConfig)
```

### Registering tools on a plugin

```python
# edgebox/plugins/github/tools.py
from edgebox.plugins.github import plugin

@plugin.tool("get_timeline", description="Get PR event timeline")
def get_timeline(box, pr_id=0, limit=50):
    return box.db.query(
        "SELECT * FROM events WHERE pr_id = ? ORDER BY id DESC LIMIT ?",
        [pr_id, limit],
    )
```

### Loading plugins in a box

```python
from edgebox.box import Box

class MyBox(Box):
    def __init__(self):
        super().__init__(manifest={
            "plugins": [
                "edgebox.plugins.github",
                "edgebox.plugins.slack",
            ],
            "config": {
                "github": {"stale_days": 14},
                "slack": {"channel": "#pr-reviews"},
            },
        })
```

## Available Plugins

| Plugin | Module path | Status | Description |
|--------|-------------|--------|-------------|
| **github** | `edgebox.plugins.github` | Shipped | PR tracking, webhook ingress, timeline tools |
| **slack** | `edgebox.plugins.slack` | Stub | Post notifications to Slack channels |
| **cron** | `edgebox.plugins.cron` | Stub | Scheduled task execution via alarms |

Community plugins are published to PyPI as `edgebox-plugin-*`. See [PLUGIN_AUTHORS.md](PLUGIN_AUTHORS.md) for how to build your own.

## Platform Support

| Platform | Status | Notes |
|----------|--------|-------|
| **Cloudflare Workers** | Supported | Durable Objects + SQLite + R2 storage |
| **Vercel Edge Functions** | Planned | -- |
| **Local dev** | Planned | Native binary via `molt run` |

The Cloudflare deployment uses Durable Objects for per-box isolation, SQLite for structured storage, and R2 buckets for blob/artifact storage. See `packages/edgebox-cloudflare/wrangler.toml` for the binding configuration.

## How MCP Integration Works

Every box exposes a `/mcp` endpoint that speaks the [Model Context Protocol](https://modelcontextprotocol.io/) over JSON-RPC 2.0. When an AI agent connects, the flow is:

1. **Initialize** -- the agent sends `initialize` and receives the server's capabilities.
2. **Discover tools** -- the agent calls `tools/list` and gets back every tool registered on the box (both `@tool()` methods on the Box subclass and tools from installed plugins).
3. **Call tools** -- the agent calls `tools/call` with a tool name and arguments. The box dispatches to the correct handler and returns the result.

```
Agent                          Box (/mcp)
  |  -- initialize ---------->  |
  |  <-- capabilities ---------  |
  |  -- tools/list ---------->  |
  |  <-- [get_timeline, ...]--  |
  |  -- tools/call ---------->  |
  |     name: "get_timeline"    |
  |     arguments: {pr_id: 42}  |
  |  <-- result ---------------  |
```

Tools from plugins are merged into the same `tools/list` response. An agent does not need to know whether a tool comes from the box or a plugin -- the namespace is flat.

### External webhooks feed the event bus

Webhooks (e.g., GitHub `POST /webhook`) are received by plugin handlers, stored in the database, and emitted as events on the internal event bus. Other plugins can subscribe to these events with wildcard patterns:

```python
@plugin.on_event("github.*")
def on_any_github_event(box, event):
    # event.name is "github.push", "github.opened", etc.
    # event.data contains the webhook payload
    pass
```

## How to Create Your First Box

### Option A: Box with inline tools (no plugins)

The simplest approach -- define everything on the Box subclass:

```python
# my_box.py
from edgebox.box import Box, tool, alarm
from edgebox.db import BoxDB

class MyBox(Box):
    def __init__(self):
        super().__init__()
        self.db = BoxDB("my_data.db")

    @tool(name="greet", description="Say hello")
    def greet(self, name="world"):
        return "Hello, " + name + "!"

    @alarm("cleanup")
    def cleanup(self):
        self.db.execute("DELETE FROM logs WHERE ts < datetime('now', '-30 days')")
        return {"cleaned": True}

if __name__ == "__main__":
    box = MyBox()
    print(box.dispatch())
```

### Option B: Box with plugins

For larger projects, split functionality into plugins:

```python
# my_box.py
from edgebox.box import Box
from edgebox.db import BoxDB

class MyBox(Box):
    def __init__(self):
        super().__init__(manifest={
            "plugins": ["edgebox.plugins.github"],
            "config": {"github": {"stale_days": 14}},
        })
        self.db = BoxDB("my_data.db")

if __name__ == "__main__":
    box = MyBox()
    print(box.dispatch())
```

### Write a manifest

Create a YAML manifest in `manifests/` to declare your box's identity, runtime, storage, ingress routes, tools, alarms, and resource limits. See `manifests/github-pr.yaml` for a complete example.

### Deploy

```bash
molt compile my_box.py -o dist/my_box.wasm
cd packages/edgebox-cloudflare
npm run deploy
```

## Configuration

Settings are resolved in priority order:

1. **Environment variables** -- `EDGEBOX_<PLUGIN>_<KEY>` (e.g., `EDGEBOX_GITHUB_STALE_DAYS=14`)
2. **Manifest config** -- the `config:` section in your manifest or the dict passed to `Box.__init__(manifest=...)`
3. **Plugin defaults** -- `PluginConfig.default_config` in each plugin's `__init__.py`

```python
# Access config in a tool handler
stale_days = box.settings.get("github", "stale_days")  # resolves via hierarchy
```

## API Reference

### Box

| Method | Description |
|--------|-------------|
| `dispatch()` | Route the inbound request to the correct handler |
| `list_tools()` | List all tools (box + plugins) |
| `list_plugins()` | List installed plugin names |
| `emit_event(event)` | Publish an event on the event bus |
| `on_event(pattern, handler)` | Subscribe to events |
| `install_plugin(plugin)` | Manually register a plugin instance |

### Routes

| Path | Method | Description |
|------|--------|-------------|
| `/mcp` | POST | MCP JSON-RPC endpoint |
| `/webhook` | POST | Legacy webhook handler (or plugin ingress) |
| `/tool/<name>` | POST | Direct HTTP tool invocation |
| `/alarm/<name>` | POST | Trigger a named alarm |
| `/health` | GET | Health check |

### Context access

```python
from edgebox.plugin import get_current_box

box = get_current_box()  # works anywhere during a request, like Flask's current_app
```

## Community Plugin Guide

See **[PLUGIN_AUTHORS.md](PLUGIN_AUTHORS.md)** for the full guide on writing, testing, and publishing edgebox plugins.

---

Powered by [Molt](https://github.com/adpena/molt) -- Python compiled to WebAssembly.
