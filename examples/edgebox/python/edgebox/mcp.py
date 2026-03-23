# edgebox/mcp.py -- MCP JSON-RPC dispatch for edgebox
#
# Implements the Model Context Protocol (MCP) server-side handlers:
#   - initialize       -> capabilities and server info
#   - tools/list       -> enumerate registered @tool methods
#   - tools/call       -> invoke a tool by name with arguments
#
# The MCP transport is JSON-RPC 2.0 over HTTP POST to /mcp.

import json

# MCP protocol version
MCP_VERSION = "2025-03-26"

# Server metadata
SERVER_NAME = "edgebox"
SERVER_VERSION = "0.1.0"


def handle_mcp(box, req):
    """Route an MCP JSON-RPC request to the appropriate handler.

    Args:
        box: The Box instance with registered tools.
        req: A BoxRequest with a JSON-RPC body.

    Returns:
        A JSON string containing the JSON-RPC response.
    """
    body = req.json()
    if body is None:
        return _error_response(None, -32700, "Parse error: empty body")

    method = body.get("method", "")
    request_id = body.get("id")
    params = body.get("params", {})

    if method == "initialize":
        return _handle_initialize(request_id, params)

    if method == "tools/list":
        return _handle_tools_list(request_id, box)

    if method == "tools/call":
        return _handle_tools_call(request_id, params, box)

    if method == "notifications/initialized":
        # Client acknowledgement -- no response needed for notifications
        return json.dumps({"jsonrpc": "2.0", "id": request_id, "result": {}})

    return _error_response(request_id, -32601, "Method not found: " + method)


# ---------------------------------------------------------------------------
# Handler: initialize
# ---------------------------------------------------------------------------

def _handle_initialize(request_id, params):
    """Respond with server capabilities."""
    result = {
        "protocolVersion": MCP_VERSION,
        "serverInfo": {
            "name": SERVER_NAME,
            "version": SERVER_VERSION,
        },
        "capabilities": {
            "tools": {"listChanged": False},
        },
    }
    return _success_response(request_id, result)


# ---------------------------------------------------------------------------
# Handler: tools/list
# ---------------------------------------------------------------------------

def _handle_tools_list(request_id, box):
    """Return the list of tools registered on the box.

    Merges Box-level tools and plugin tools into a single MCP list.
    """
    raw_tools = box.list_tools()
    mcp_tools = []
    idx = 0
    while idx < len(raw_tools):
        t = raw_tools[idx]
        idx = idx + 1
        schema = t.get("inputSchema", {"type": "object", "properties": {}})
        mcp_tools.append({
            "name": t["name"],
            "description": t.get("description", ""),
            "inputSchema": schema,
        })

    result = {"tools": mcp_tools}
    return _success_response(request_id, result)


# ---------------------------------------------------------------------------
# Handler: tools/call
# ---------------------------------------------------------------------------

def _handle_tools_call(request_id, params, box):
    """Invoke a named tool and return its result.

    Checks both Box-level tools (@tool decorator on methods) and
    plugin tools (registered via EdgeboxPlugin.tool decorator).
    """
    tool_name = params.get("name", "")
    arguments = params.get("arguments", {})

    # Check Box-level tools first
    handler = box._tools.get(tool_name)
    plugin_tool = None
    if handler is None:
        # Check plugin tools
        plugin_tool = box._registry.get_tool(tool_name)
        if plugin_tool is None:
            return _error_response(request_id, -32602,
                                   "Unknown tool: " + tool_name)

    try:
        if plugin_tool is not None:
            output = plugin_tool.handler(box, **arguments)
        else:
            output = handler(**arguments)
    except Exception as exc:
        return _error_response(request_id, -32000, "Tool error: " + str(exc))

    # Wrap the result per MCP spec
    content = []
    if isinstance(output, str):
        content.append({"type": "text", "text": output})
    else:
        content.append({"type": "text", "text": json.dumps(output)})

    result = {"content": content, "isError": False}
    return _success_response(request_id, result)


# ---------------------------------------------------------------------------
# JSON-RPC response helpers
# ---------------------------------------------------------------------------

def _success_response(request_id, result):
    """Build a JSON-RPC 2.0 success response."""
    return json.dumps({
        "jsonrpc": "2.0",
        "id": request_id,
        "result": result,
    })


def _error_response(request_id, code, message):
    """Build a JSON-RPC 2.0 error response."""
    return json.dumps({
        "jsonrpc": "2.0",
        "id": request_id,
        "error": {
            "code": code,
            "message": message,
        },
    })
