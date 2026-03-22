# GitHub PR Box -- Agent Integration Guide

## Overview

The GitHub PR Box is an edgebox module that tracks pull-request lifecycle
events and exposes tools via the Model Context Protocol (MCP). AI agents
connect to the `/mcp` endpoint to query PR timelines, post review comments,
and monitor for stale PRs.

## MCP Endpoint

All tool interactions use JSON-RPC 2.0 over HTTP POST:

    POST /mcp
    Content-Type: application/json
    Authorization: Bearer <token>

## Connection Flow

1. Send `initialize` to negotiate capabilities.
2. Wait for confirmation, then send `notifications/initialized`.
3. Call `tools/list` to discover available tools.
4. Call `tools/call` with tool name and arguments.

## Available Tools

### get_timeline

Get the event timeline for a pull request.

Arguments:
- `pr_id` (integer, required) -- the pull request number
- `limit` (integer, default 50) -- max events to return

Returns: list of event objects with id, event_type, actor, payload, created_at.

### add_review_comment

Post a review comment on a specific file and line.

Arguments:
- `pr_id` (integer, required) -- the pull request number
- `path` (string, required) -- file path relative to repo root
- `line` (integer, required) -- line number in the diff
- `body` (string, required) -- comment text
- `author` (string, default "edgebox") -- comment author

Returns: object with comment_id and pr_id.

### get_diff_summary

Get an overview of PR activity.

Arguments:
- `pr_id` (integer, required) -- the pull request number

Returns: object with pr_id, total_events, total_comments, event_types list.

### query_timeline

Run a filtered query against the event timeline.

Arguments:
- `pr_id` (integer, required) -- the pull request number
- `event_type` (string, optional) -- filter by event type
- `actor` (string, optional) -- filter by actor login
- `limit` (integer, default 20) -- max events to return

Returns: list of matching event objects.

## Alarms

### stale_check

Fires daily at 09:00 UTC. Returns a list of PR IDs with no activity in the
last 7 days. Agents can subscribe to alarm results to trigger follow-up
actions (e.g., pinging reviewers, posting reminders).

## Example: Initialize and List Tools

Request:

    {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}

Response:

    {
      "jsonrpc": "2.0",
      "id": 1,
      "result": {
        "protocolVersion": "2025-03-26",
        "serverInfo": {"name": "edgebox", "version": "0.1.0"},
        "capabilities": {"tools": {"listChanged": false}}
      }
    }

## Example: Call a Tool

Request:

    {
      "jsonrpc": "2.0",
      "id": 2,
      "method": "tools/call",
      "params": {
        "name": "get_timeline",
        "arguments": {"pr_id": 42, "limit": 10}
      }
    }

Response:

    {
      "jsonrpc": "2.0",
      "id": 2,
      "result": {
        "content": [{"type": "text", "text": "[...]"}],
        "isError": false
      }
    }

## Storage

Events and comments are persisted in SQLite. The schema is defined in
`schema.sql` and is automatically applied on box startup. The database
is limited to 64 MB by the manifest.

## Limits

- Memory: 128 MB
- CPU per request: 5000 ms
- Database: 64 MB
- Request body: 512 KB
