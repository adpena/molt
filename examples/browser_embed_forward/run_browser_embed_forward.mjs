const [browserEmbedUrl, baseUrl] = process.argv.slice(2);

if (!browserEmbedUrl || !baseUrl) {
  throw new Error(
    'usage: node examples/browser_embed_forward/run_browser_embed_forward.mjs <browser_embed.js URL> <artifact base URL>',
  );
}

const { loadMoltBrowserKernel } = await import(browserEmbedUrl);

const kernel = await loadMoltBrowserKernel({
  baseUrl,
  exportName: 'forward',
  resultType: 'float32',
});

const input = new Float32Array([1.25, -2.5, 0, 4.75]);
const output = await kernel.forward(input);

console.log(JSON.stringify({
  ctor: output.constructor.name,
  exportName: kernel.exportName,
  values: Array.from(output),
}));
