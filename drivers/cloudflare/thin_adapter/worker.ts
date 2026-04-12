export interface ThinAdapterEnv {
  ASSETS: Fetcher;
  WEIGHTS_BASE_URL?: string;
  DRIVER_TARGET?: string;
}

export interface ThinArtifactEntry {
  url: string;
  sha256?: string;
  size_bytes?: number;
}

export interface ThinWeightEntry {
  path: string;
  url: string;
  sha256?: string;
  size_bytes?: number;
}

export interface ThinManifestBase {
  version: number;
  target: string;
  artifacts: Record<string, ThinArtifactEntry>;
  weights: {
    base_url: string | null;
    files: ThinWeightEntry[];
  };
  exports?: Record<string, string>;
}

export interface ThinAssetWorkerConfig {
  target: string;
  manifestRoute?: string;
  manifestAssetPath?: string;
}

const DEFAULT_MANIFEST_ROUTE = "/driver-manifest.json";
const DEFAULT_MANIFEST_ASSET_PATH = "/driver-manifest.base.json";

function jsonResponse(payload: unknown, status = 200, headers: HeadersInit = {}): Response {
  return new Response(JSON.stringify(payload, null, 2) + "\n", {
    status,
    headers: {
      "content-type": "application/json; charset=utf-8",
      ...headers,
    },
  });
}

function errorResponse(status: number, code: string, message: string): Response {
  return jsonResponse(
    {
      status: "error",
      code,
      message,
    },
    status,
    { "cache-control": "no-store" },
  );
}

async function fetchAssetJson(
  request: Request,
  env: ThinAdapterEnv,
  assetPath: string,
): Promise<ThinManifestBase> {
  const assetUrl = new URL(assetPath, request.url);
  const assetRequest = new Request(assetUrl.toString(), {
    method: "GET",
    headers: request.headers,
  });
  const response = await env.ASSETS.fetch(assetRequest);
  if (!response.ok) {
    throw new Error(`asset ${assetPath} returned ${response.status}`);
  }
  return (await response.json()) as ThinManifestBase;
}

function mergeManifest(base: ThinManifestBase, env: ThinAdapterEnv, target: string): ThinManifestBase {
  const weightsBaseUrl = env.WEIGHTS_BASE_URL ?? base.weights.base_url ?? null;
  if (!weightsBaseUrl) {
    throw new Error("WEIGHTS_BASE_URL is required for the thin adapter manifest");
  }
  return {
    ...base,
    target,
    weights: {
      ...base.weights,
      base_url: weightsBaseUrl,
    },
  };
}

export function createThinAssetWorker<Env extends ThinAdapterEnv>(
  config: ThinAssetWorkerConfig,
) {
  const manifestRoute = config.manifestRoute ?? DEFAULT_MANIFEST_ROUTE;
  const manifestAssetPath = config.manifestAssetPath ?? DEFAULT_MANIFEST_ASSET_PATH;

  return {
    async fetch(request: Request, env: Env): Promise<Response> {
      if (!env.ASSETS || typeof env.ASSETS.fetch !== "function") {
        return errorResponse(500, "missing_assets_binding", "ASSETS binding is required");
      }

      const url = new URL(request.url);
      if (url.pathname === manifestRoute) {
        if (request.method !== "GET" && request.method !== "HEAD") {
          return errorResponse(405, "method_not_allowed", "manifest route only supports GET/HEAD");
        }
        try {
          const baseManifest = await fetchAssetJson(request, env, manifestAssetPath);
          const merged = mergeManifest(baseManifest, env, config.target);
          if (request.method === "HEAD") {
            return new Response(null, {
              status: 200,
              headers: {
                "content-type": "application/json; charset=utf-8",
                "cache-control": "no-store",
              },
            });
          }
          return jsonResponse(merged, 200, { "cache-control": "no-store" });
        } catch (error) {
          const message =
            error instanceof Error ? error.message : "failed to materialize manifest";
          return errorResponse(500, "manifest_unavailable", message);
        }
      }

      return env.ASSETS.fetch(request);
    },
  };
}
