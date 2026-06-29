# Browser Embed Forward

This example is the minimal browser-kernel embed path:

1. Compile `forward.py` with the existing split-runtime browser WASM authority.
2. Serve the generated output directory.
3. Import `browser_embed.js` directly and call `forward(Float32Array) -> Float32Array`.

```powershell
python -m molt.cli build examples/browser_embed_forward/forward.py --build-profile dev --profile browser --target wasm --wasm-profile pure --type-hints ignore --split-runtime --out-dir tmp/browser_embed_forward
```

```powershell
node examples/browser_embed_forward/run_browser_embed_forward.mjs file:///ABS/PATH/tmp/browser_embed_forward/browser_embed.js http://127.0.0.1:PORT/
```

No prebuilt WASM or copied embed loader is checked in here. `wasm/browser_embed.js`
is the single browser embed authority; this example only supplies user source and
plain JS that consumes a generated build directory.
