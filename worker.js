// Auto-generated split-runtime Cloudflare Workers shim
import runtimeModule from "./molt_runtime.wasm";
import appModule from "./app.wasm";

export default {
  async fetch(request) {
    // Instantiate runtime first, then app with runtime exports
    const rtInstance = await WebAssembly.instantiate(runtimeModule, {});
    const appInstance = await WebAssembly.instantiate(appModule, {
      molt_runtime: rtInstance.exports,
    });
    if (appInstance.exports.molt_table_init) {
      appInstance.exports.molt_table_init();
    }
    appInstance.exports.molt_main();
  }
};
