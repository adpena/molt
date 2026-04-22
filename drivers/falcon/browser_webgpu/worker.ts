import { createThinAssetWorker, type ThinAdapterEnv } from "../../cloudflare/thin_adapter/worker";

interface Env extends ThinAdapterEnv {
  WEIGHTS: R2Bucket;
}

export default createThinAssetWorker<Env>({
  target: "falcon.browser_webgpu",
  // Legacy driver-lane manifest discovery. The enjoice handoff path uses
  // deploy/enjoice direct-WASM adapters instead of this bootstrap worker.
  manifestRoute: "/driver-manifest.json",
  manifestAssetPath: "/driver-manifest.base.json",
});
