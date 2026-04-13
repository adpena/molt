export interface ThinAdapterEnv {
  ASSETS: Fetcher;
  WEIGHTS_BASE_URL?: string;
  DRIVER_TARGET?: string;
  WEIGHTS?: R2Bucket;
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

function deriveWeightsBaseUrl(request: Request, env: ThinAdapterEnv, base: ThinManifestBase): string | null {
  if (env.WEIGHTS_BASE_URL) return env.WEIGHTS_BASE_URL;
  if (base.weights.base_url) return base.weights.base_url;
  if (env.WEIGHTS) {
    return `${new URL(request.url).origin}/weights`;
  }
  return null;
}

function mergeManifest(
  request: Request,
  base: ThinManifestBase,
  env: ThinAdapterEnv,
  target: string,
): ThinManifestBase {
  const weightsBaseUrl = deriveWeightsBaseUrl(request, env, base);
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

function applyObjectHeaders(object: unknown, headers: Headers): void {
  const candidate = object as {
    writeHttpMetadata?: (headers: Headers) => void;
    httpMetadata?: { contentType?: string; cacheControl?: string; contentLanguage?: string; contentDisposition?: string; contentEncoding?: string };
    size?: number;
    etag?: string;
    httpEtag?: string;
  };
  if (typeof candidate.writeHttpMetadata === "function") {
    candidate.writeHttpMetadata(headers);
  }
  const metadata = candidate.httpMetadata;
  if (metadata?.contentType && !headers.has("content-type")) headers.set("content-type", metadata.contentType);
  if (metadata?.cacheControl && !headers.has("cache-control")) headers.set("cache-control", metadata.cacheControl);
  if (metadata?.contentLanguage && !headers.has("content-language")) headers.set("content-language", metadata.contentLanguage);
  if (metadata?.contentDisposition && !headers.has("content-disposition")) headers.set("content-disposition", metadata.contentDisposition);
  if (metadata?.contentEncoding && !headers.has("content-encoding")) headers.set("content-encoding", metadata.contentEncoding);
  if (typeof candidate.size === "number" && !headers.has("content-length")) {
    headers.set("content-length", String(candidate.size));
  }
  const etag = candidate.httpEtag ?? candidate.etag;
  if (etag && !headers.has("etag")) {
    headers.set("etag", JSON.stringify(etag));
  }
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
          const merged = mergeManifest(request, baseManifest, env, config.target);
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

      if (url.pathname.startsWith("/weights/")) {
        if (request.method !== "GET" && request.method !== "HEAD") {
          return errorResponse(405, "method_not_allowed", "weights route only supports GET/HEAD");
        }
        if (!env.WEIGHTS || typeof env.WEIGHTS.get !== "function") {
          return errorResponse(500, "missing_weights_binding", "WEIGHTS binding is required");
        }
        const key = url.pathname.slice("/weights/".length);
        if (!key) {
          return errorResponse(404, "weight_not_found", "weight path is required");
        }
        const object = await env.WEIGHTS.get(key);
        if (!object) {
          return errorResponse(404, "weight_not_found", `weight object not found: ${key}`);
        }
        const headers = new Headers();
        applyObjectHeaders(object, headers);
        if (!headers.has("cache-control")) {
          headers.set("cache-control", "public, max-age=31536000, immutable");
        }
        if (request.method === "HEAD") {
          return new Response(null, { status: 200, headers });
        }
        return new Response((object as { body?: BodyInit | null }).body ?? null, {
          status: 200,
          headers,
        });
      }

      return env.ASSETS.fetch(request);
    },
  };
}
