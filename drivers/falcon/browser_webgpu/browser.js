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

function normalizeDirectoryUrl(url) {
  return url.endsWith("/") ? url : `${url}/`;
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

function optionalManifestArtifactUrl(manifest, manifestUrl, key) {
  const artifact = manifest?.artifacts?.[key];
  if (!artifact || typeof artifact.url !== "string" || !artifact.url) {
    return null;
  }
  return resolveArtifactUrl(manifestUrl, artifact.url);
}

function manifestWeightUrl(manifest, manifestUrl) {
  const weights = manifest?.weights;
  const files = Array.isArray(weights?.files) ? weights.files : [];
  const modelWeights =
    files.find(
      (entry) =>
        entry &&
        typeof entry === "object" &&
        ((typeof entry.path === "string" && entry.path.endsWith("model.safetensors")) ||
          (typeof entry.url === "string" && entry.url.endsWith("model.safetensors"))),
    ) ?? null;
  const resolvedEntry = modelWeights ?? (files.length === 1 ? files[0] : null);
  if (!resolvedEntry || typeof resolvedEntry.url !== "string" || !resolvedEntry.url) {
    throw new Error("Falcon manifest is missing weights model.safetensors url");
  }
  const baseUrl =
    typeof weights?.base_url === "string" && weights.base_url
      ? resolveArtifactUrl(manifestUrl, weights.base_url)
      : manifestUrl;
  return resolveArtifactUrl(normalizeDirectoryUrl(baseUrl), resolvedEntry.url);
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
  initExport,
  browserHostOptions = {},
}) {
  if (!manifestUrl) {
    throw new Error("Falcon browser driver requires manifestUrl.");
  }
  const resolvedManifestUrl = normalizeManifestUrl(manifestUrl);
  const manifest = await fetchRequiredJson(resolvedManifestUrl);
  const resolvedWasmUrl = manifestArtifactUrl(manifest, resolvedManifestUrl, "app_wasm");
  const resolvedRuntimeUrl = manifestArtifactUrl(manifest, resolvedManifestUrl, "runtime_wasm");
  const resolvedWeightsUrl = manifestWeightUrl(manifest, resolvedManifestUrl);
  const resolvedConfigUrl = manifestArtifactUrl(manifest, resolvedManifestUrl, "config_json");
  const resolvedTokenizerUrl = optionalManifestArtifactUrl(
    manifest,
    resolvedManifestUrl,
    "tokenizer_json",
  );
  const resolvedInitExport = initExport ?? manifest?.exports?.init ?? "main_molt__init";
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

  host.run();
  await host.invokeExport(resolvedInitExport, [new Uint8Array(weightsBytes), configJson]);

  return {
    host,
    manifest,
    manifestUrl: resolvedManifestUrl,
    tokenizerUrl: resolvedTokenizerUrl,
    async invokeExport(exportName, args = []) {
      return host.invokeExport(exportName, args);
    },
    async ocrTokens({
      width,
      height,
      rgb,
      promptIds,
      maxNewTokens = DEFAULT_MAX_NEW_TOKENS,
      exportName,
    }) {
      const resolvedOcrTokensExport =
        exportName ?? manifest?.exports?.ocrTokens ?? "main_molt__ocr_tokens";
      const result = await host.invokeExport(resolvedOcrTokensExport, [
        width,
        height,
        rgb,
        promptIds,
        maxNewTokens,
      ]);
      if (!Array.isArray(result.resultJson)) {
        throw new Error(
          `Falcon ${resolvedOcrTokensExport} returned a non-list payload: ${result.resultRepr ?? "null"}`,
        );
      }
      return result.resultJson;
    },
  };
}
