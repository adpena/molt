interface Env {
  WEIGHTS: R2Bucket;
}

interface DriverScaffold {
  status: "scaffold";
  target: "falcon.browser_webgpu";
  request: {
    method: string;
    url: string;
  };
  bindings: {
    weights: boolean;
  };
  note: string;
}

function buildScaffoldResponse(request: Request, env: Partial<Env>): DriverScaffold {
  return {
    status: "scaffold",
    target: "falcon.browser_webgpu",
    request: {
      method: request.method,
      url: request.url,
    },
    bindings: {
      weights: typeof env.WEIGHTS !== "undefined",
    },
    note: "Falcon browser WebGPU deployment surface scaffold. Runtime wiring comes later.",
  };
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const payload = buildScaffoldResponse(request, env);
    return new Response(JSON.stringify(payload, null, 2) + "\n", {
      status: 501,
      headers: {
        "content-type": "application/json; charset=utf-8",
        "cache-control": "no-store",
      },
    });
  },
};
