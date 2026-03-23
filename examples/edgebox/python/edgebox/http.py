# edgebox/http.py -- BoxRequest: parse inbound request from sys.argv + env vars
#
# The edgebox runtime passes request metadata via environment variables
# and the request body via sys.argv (JSON-encoded) or stdin. This module
# reconstructs a simple request object from that context.

import json
import os
import sys


class BoxRequest:
    """Represents an inbound HTTP request to a Box.

    Attributes:
        method:  HTTP method (GET, POST, etc.)
        path:    Request path (e.g. "/mcp", "/webhook")
        headers: Dict of header name -> value
        body:    Raw request body string (may be empty)
        params:  Parsed query parameters as a dict
    """

    def __init__(self, method="GET", path="/", headers=None, body="", params=None):
        self.method = method
        self.path = path
        self.headers = headers if headers is not None else {}
        self.body = body
        self.params = params if params is not None else {}

    @classmethod
    def from_env(cls):
        """Build a BoxRequest from environment variables and sys.argv.

        Environment variables read:
            EDGEBOX_METHOD  -- HTTP method (default GET)
            EDGEBOX_PATH    -- request path (default /)
            EDGEBOX_HEADERS -- JSON-encoded headers dict
            EDGEBOX_QUERY   -- JSON-encoded query params dict

        The request body is taken from EDGEBOX_BODY env var.
        """
        method = os.environ.get("EDGEBOX_METHOD", "GET")
        path = os.environ.get("EDGEBOX_PATH", "/")

        # Parse headers from env
        headers_raw = os.environ.get("EDGEBOX_HEADERS", "{}")
        headers = json.loads(headers_raw)

        # Parse query params from env — supports both JSON-encoded
        # dicts and URL-encoded query strings (key=val&key2=val2).
        params_raw = os.environ.get("EDGEBOX_QUERY", "")
        if params_raw.startswith("{"):
            params = json.loads(params_raw)
        elif params_raw:
            params = {}
            for part in params_raw.split("&"):
                if "=" in part:
                    k, v = part.split("=", 1)
                    params[k] = v
                elif part:
                    params[part] = ""
        else:
            params = {}

        # Body from env var (not argv — argv is used for WASI args)
        body = os.environ.get("EDGEBOX_BODY", "")

        return cls(
            method=method,
            path=path,
            headers=headers,
            body=body,
            params=params,
        )

    def json(self):
        """Parse the body as JSON and return the result."""
        if not self.body:
            return None
        return json.loads(self.body)

    def header(self, name, default=""):
        """Get a header value, case-insensitive."""
        # Check exact match first
        val = self.headers.get(name)
        if val is not None:
            return val
        # Case-insensitive fallback
        lower_name = name.lower()
        keys = list(self.headers.keys())
        idx = 0
        while idx < len(keys):
            k = keys[idx]
            if k.lower() == lower_name:
                return self.headers[k]
            idx = idx + 1
        return default
