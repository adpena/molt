const fs = require('fs');
const path = require('path');
const { WASI } = require('node:wasi');

async function main() {
    // Load WASM module
    const wasmPath = '/tmp/paddleocr_final_linked.wasm';
    if (!fs.existsSync(wasmPath)) {
        console.log('PaddleOCR WASM not found. Build with:');
        console.log('  MOLT_HERMETIC_MODULE_ROOTS=1 molt build src/molt/stdlib/tinygrad/paddleocr_driver.py --target wasm');
        return;
    }

    const wasmBytes = fs.readFileSync(wasmPath);
    console.log(`WASM: ${(wasmBytes.length / 1e6).toFixed(1)} MB`);

    // Find ONNX weight files
    const findFile = (pattern) => {
        const results = [];
        const search = (dir) => {
            try {
                for (const f of fs.readdirSync(dir, { withFileTypes: true })) {
                    if (f.isDirectory()) search(path.join(dir, f.name));
                    else if (f.name.match(pattern)) results.push(path.join(dir, f.name));
                }
            } catch {}
        };
        search('/tmp/paddleocr-onnx');
        return results[0];
    };

    const detPath = findFile(/ch_PP-OCRv4_det\.onnx$/);
    const recPath = findFile(/english.*model\.onnx$/) || findFile(/ch_PP-OCRv4_rec\.onnx$/);
    const dictPath = path.join(__dirname, '../../deploy/models/en_ppocr_dict.txt');

    if (!detPath) { console.log('Detector ONNX not found'); return; }
    console.log(`Detector: ${detPath} (${(fs.statSync(detPath).size/1e6).toFixed(1)} MB)`);
    if (recPath) console.log(`Recognizer: ${recPath} (${(fs.statSync(recPath).size/1e6).toFixed(1)} MB)`);
    console.log(`Dict: ${dictPath}`);

    // Try loading via molt's run_wasm.js infrastructure
    // The WASM exports: tinygrad_paddleocr_driver__init, __ocr, __detect_only

    // For now, measure startup time
    const start = performance.now();
    const wasi = new WASI({ version: 'preview1' });
    const memory = new WebAssembly.Memory({ initial: 1024, maximum: 65536 });
    const table = new WebAssembly.Table({ initial: 10000, element: 'anyfunc' });

    // Minimal import stubs
    const env = {
        memory,
        __indirect_function_table: table,
    };

    // Add all required host imports as stubs
    const mod = new WebAssembly.Module(wasmBytes);
    const imports = WebAssembly.Module.imports(mod);
    for (const imp of imports) {
        if (imp.module === 'env' && !env[imp.name] && imp.kind === 'function') {
            env[imp.name] = (...args) => 0n;
        }
    }

    try {
        const instance = new WebAssembly.Instance(mod, {
            env,
            wasi_snapshot_preview1: wasi.wasiImport,
        });
        const elapsed = performance.now() - start;
        console.log(`\nWASM instantiate: ${elapsed.toFixed(1)} ms`);

        // List exports
        const exports = Object.keys(instance.exports).filter(k => k.includes('paddleocr'));
        console.log(`PaddleOCR exports: ${exports.join(', ')}`);
    } catch (e) {
        console.log(`Instantiation error: ${e.message}`);
    }
}

main().catch(console.error);
