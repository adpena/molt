// Wrangler imports a .wasm asset as a precompiled module, not runtime bytes.
declare module "*.wasm" {
  const module: WebAssembly.Module;
  export default module;
}
