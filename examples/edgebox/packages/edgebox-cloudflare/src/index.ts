// edgebox Worker router
// Routes /box/gh/{owner}/{repo}/pr/{pr_number}/* to a Durable Object keyed by the route params.

import { EdgeBox } from "./box";

export { EdgeBox };

interface Env {
  EDGE_BOX: DurableObjectNamespace;
  ARTIFACTS: R2Bucket;
  BOX_SCHEMA_SQL: string;
}

const CORS_HEADERS: Record<string, string> = {
  "access-control-allow-origin": "*",
  "access-control-allow-methods": "GET, POST, PUT, DELETE, OPTIONS",
  "access-control-allow-headers": "content-type, authorization",
  "access-control-max-age": "86400",
};

function corsResponse(status = 204): Response {
  return new Response(null, { status, headers: CORS_HEADERS });
}

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", ...CORS_HEADERS },
  });
}

// Match: /box/gh/{owner}/{repo}/pr/{pr_number}
// Everything after the pr_number is forwarded as the sub-path to the DO.
const BOX_ROUTE = /^\/box\/gh\/([^/]+)\/([^/]+)\/pr\/(\d+)(\/.*)?$/;

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    if (request.method === "OPTIONS") {
      return corsResponse();
    }

    const url = new URL(request.url);
    const match = BOX_ROUTE.exec(url.pathname);

    if (!match) {
      return jsonResponse({ error: "not found", hint: "/box/gh/{owner}/{repo}/pr/{number}" }, 404);
    }

    const [, owner, repo, prNumber, subPath] = match;
    const boxKey = `gh/${owner}/${repo}/pr/${prNumber}`;

    // Derive a stable DO id from the box key
    const doId = env.EDGE_BOX.idFromName(boxKey);
    const stub = env.EDGE_BOX.get(doId);

    // Build forwarded request with edgebox metadata headers
    const forwardUrl = new URL(request.url);
    forwardUrl.pathname = subPath || "/";

    const forwardHeaders = new Headers(request.headers);
    forwardHeaders.set("x-edgebox-key", boxKey);
    forwardHeaders.set("x-edgebox-owner", owner);
    forwardHeaders.set("x-edgebox-repo", repo);
    forwardHeaders.set("x-edgebox-pr", prNumber);

    const forwardRequest = new Request(forwardUrl.toString(), {
      method: request.method,
      headers: forwardHeaders,
      body: request.body,
    });

    const response = await stub.fetch(forwardRequest);

    // Merge CORS headers into the DO response
    const merged = new Response(response.body, response);
    for (const [k, v] of Object.entries(CORS_HEADERS)) {
      merged.headers.set(k, v);
    }
    return merged;
  },
};
