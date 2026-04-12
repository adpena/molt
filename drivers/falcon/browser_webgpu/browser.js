import { loadMoltWasm } from "../../../wasm/browser_host.js";

const DEFAULT_PATCH_SIZE = 16;
const DEFAULT_MAX_NEW_TOKENS = 512;

async function fetchRequiredJson(url) {
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`Failed to load manifest: ${response.status}`);
  }
  return response.json();
}

function resolveArtifactUrl(baseUrl, relativeOrAbsoluteUrl) {
  return new URL(relativeOrAbsoluteUrl, baseUrl).toString();
}

function normalizeManifestUrl(manifestUrl) {
  try {
    return new URL(manifestUrl).toString();
  } catch {
    const baseHref =
      typeof globalThis.location?.href === "string" ? globalThis.location.href : null;
    if (!baseHref) {
      throw new Error(
        `Relative manifestUrl requires a browser location base: ${String(manifestUrl)}`,
      );
    }
    return new URL(manifestUrl, baseHref).toString();
  }
}

function manifestArtifactUrl(manifest, manifestUrl, key) {
  const artifact = manifest?.artifacts?.[key];
  if (!artifact || typeof artifact.url !== "string" || !artifact.url) {
    throw new Error(`Falcon manifest is missing artifacts.${key}.url`);
  }
  return resolveArtifactUrl(manifestUrl, artifact.url);
}

function manifestWeightUrl(manifest, manifestUrl) {
  const weights = manifest?.weights;
  const first = Array.isArray(weights?.files) ? weights.files[0] : null;
  if (!first || typeof first.url !== "string" || !first.url) {
    throw new Error("Falcon manifest is missing weights.files[0].url");
  }
  const baseUrl =
    typeof weights?.base_url === "string" && weights.base_url ? weights.base_url : manifestUrl;
  return resolveArtifactUrl(baseUrl, first.url);
}

export async function imageToRgbPatchAligned(blob, patchSize = DEFAULT_PATCH_SIZE) {
  const bitmap = await createImageBitmap(blob);
  const width = Math.floor(bitmap.width / patchSize) * patchSize;
  const height = Math.floor(bitmap.height / patchSize) * patchSize;
  if (width === 0 || height === 0) {
    throw new Error(
      `Image too small (${bitmap.width}x${bitmap.height}). Minimum ${patchSize}x${patchSize}.`,
    );
  }
  const canvas = new OffscreenCanvas(width, height);
  const ctx = canvas.getContext("2d");
  if (!ctx) {
    throw new Error("Canvas 2D context not available");
  }
  ctx.drawImage(bitmap, 0, 0, width, height);
  const imageData = ctx.getImageData(0, 0, width, height).data;
  const rgb = new Uint8Array(width * height * 3);
  for (let i = 0, j = 0; i < imageData.length; i += 4, j += 3) {
    rgb[j] = imageData[i];
    rgb[j + 1] = imageData[i + 1];
    rgb[j + 2] = imageData[i + 2];
  }
  bitmap.close();
  return { width, height, rgb };
}

export async function initFalconBrowserWebGpu({
  manifestUrl,
  wasmUrl,
  runtimeUrl,
  weightsUrl,
  configUrl,
  initExport = "main_molt__init",
  browserHostOptions = {},
}) {
  let manifest = null;
  const resolvedManifestUrl = manifestUrl ? normalizeManifestUrl(manifestUrl) : null;
  if (manifestUrl) {
    manifest = await fetchRequiredJson(resolvedManifestUrl);
  }
  const resolvedWasmUrl =
    wasmUrl ?? (manifest ? manifestArtifactUrl(manifest, resolvedManifestUrl, "app_wasm") : null);
  const resolvedRuntimeUrl =
    runtimeUrl ??
    (manifest ? manifestArtifactUrl(manifest, resolvedManifestUrl, "runtime_wasm") : null);
  const resolvedWeightsUrl =
    weightsUrl ?? (manifest ? manifestWeightUrl(manifest, resolvedManifestUrl) : null);
  const resolvedConfigUrl =
    configUrl ??
    (manifest ? manifestArtifactUrl(manifest, resolvedManifestUrl, "config_json") : null);
  if (!resolvedWasmUrl || !resolvedRuntimeUrl || !resolvedWeightsUrl || !resolvedConfigUrl) {
    throw new Error(
      "Falcon browser driver requires wasmUrl, runtimeUrl, weightsUrl, and configUrl or a manifestUrl that resolves them.",
    );
  }
  const [host, weightsBytes, configJson] = await Promise.all([
    loadMoltWasm({
      wasmUrl: resolvedWasmUrl,
      runtimeUrl: resolvedRuntimeUrl,
      preferLinked: false,
      env: { MOLT_GPU_BACKEND: "webgpu" },
      ...browserHostOptions,
    }),
    fetch(resolvedWeightsUrl).then((r) => {
      if (!r.ok) throw new Error(`Failed to load weights: ${r.status}`);
      return r.arrayBuffer();
    }),
    fetch(resolvedConfigUrl).then((r) => {
      if (!r.ok) throw new Error(`Failed to load config: ${r.status}`);
      return r.text();
    }),
  ]);

  await host.invokeExport(initExport, [new Uint8Array(weightsBytes), configJson]);

  return {
    host,
    async ocrTokens({
      width,
      height,
      rgb,
      promptIds,
      maxNewTokens = DEFAULT_MAX_NEW_TOKENS,
      exportName = "main_molt__ocr_tokens",
    }) {
      const result = await host.invokeExport(exportName, [
        width,
        height,
        rgb,
        promptIds,
        maxNewTokens,
      ]);
      if (!Array.isArray(result.resultJson)) {
        throw new Error(
          `Falcon ocr_tokens returned a non-list payload: ${result.resultRepr ?? "null"}`,
        );
      }
      return result.resultJson;
    },
  };
}
