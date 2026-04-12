import { loadMoltWasm } from "../../../wasm/browser_host.js";

const DEFAULT_PATCH_SIZE = 16;
const DEFAULT_MAX_NEW_TOKENS = 512;

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
  wasmUrl,
  runtimeUrl,
  weightsUrl,
  configUrl,
  initExport = "main_molt__init",
  browserHostOptions = {},
}) {
  const [host, weightsBytes, configJson] = await Promise.all([
    loadMoltWasm({
      wasmUrl,
      runtimeUrl,
      preferLinked: false,
      env: { MOLT_GPU_BACKEND: "webgpu" },
      ...browserHostOptions,
    }),
    fetch(weightsUrl).then((r) => {
      if (!r.ok) throw new Error(`Failed to load weights: ${r.status}`);
      return r.arrayBuffer();
    }),
    fetch(configUrl).then((r) => {
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
