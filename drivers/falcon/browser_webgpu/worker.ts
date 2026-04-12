import { createThinAssetWorker, type ThinAdapterEnv } from "../../cloudflare/thin_adapter/worker";

interface Env extends ThinAdapterEnv {
  WEIGHTS: R2Bucket;
}

export default createThinAssetWorker<Env>({
  target: "falcon.browser_webgpu",
  manifestRoute: "/driver-manifest.json",
  manifestAssetPath: "/driver-manifest.base.json",
});
