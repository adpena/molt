import {
  WEBGPU_DISPATCH_HOST_IMPORT,
  TARGET_FEATURE_MANIFEST_ASSET_NAME,
} from './target_feature_constants.generated.js';

export { WEBGPU_DISPATCH_HOST_IMPORT, TARGET_FEATURE_MANIFEST_ASSET_NAME };

export const parsedImportsRequireWebGpuDispatch = (...parsedImports) =>
  parsedImports.some((imports) =>
    (imports?.funcImports || []).some(
      (entry) =>
        entry.module === 'env' && entry.name === WEBGPU_DISPATCH_HOST_IMPORT,
    ),
  );

export const assertBrowserTargetFeatureContract = (
  manifest,
  observedWebGpuImport,
) => {
  const targetFeatures = manifest?.target_features || null;
  if (!targetFeatures) {
    throw new Error('split-runtime manifest missing target_features authority');
  }
  if (targetFeatures.authority !== 'target_feature_manifest') {
    throw new Error(
      'split-runtime manifest target_features authority must be target_feature_manifest',
    );
  }
  const declaredWebGpuImports = targetFeatures.required_host_imports?.webgpu || [];
  if (!Array.isArray(declaredWebGpuImports)) {
    throw new Error(
      'split-runtime manifest target_features.required_host_imports.webgpu must be an array',
    );
  }
  const declaresWebGpu = declaredWebGpuImports.includes(
    WEBGPU_DISPATCH_HOST_IMPORT,
  );
  if (declaresWebGpu !== observedWebGpuImport) {
    throw new Error(
      'split-runtime manifest target_features WebGPU imports do not match wasm env imports',
    );
  }
  const webgpuProbeRequired =
    targetFeatures.browser_probes?.webgpu?.required === true;
  if (webgpuProbeRequired !== observedWebGpuImport) {
    throw new Error(
      'split-runtime manifest target_features browser_probes.webgpu.required disagrees with wasm imports',
    );
  }
  const expectedProfile = observedWebGpuImport
    ? 'wasm-browser-webgpu'
    : 'wasm-browser';
  if (targetFeatures.profile !== expectedProfile) {
    throw new Error(
      `split-runtime manifest target_features.profile must be ${expectedProfile}`,
    );
  }
};
