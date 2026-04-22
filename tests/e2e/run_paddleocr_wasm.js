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

    // --- Phase 1: WASM compilation + instantiation benchmark ---
    console.log('\n=== Phase 1: WASM Compilation & Instantiation ===');
    const compileStart = performance.now();
    const mod = new WebAssembly.Module(wasmBytes);
    const compileEnd = performance.now();
    console.log(`Compile: ${(compileEnd - compileStart).toFixed(1)} ms`);

    // Catalog all exports
    const allExports = WebAssembly.Module.exports(mod);
    const funcExports = allExports.filter(e => e.kind === 'function');
    const paddleExports = funcExports.filter(e => e.name.includes('paddleocr'));
    const runtimeExports = funcExports.filter(e => e.name.startsWith('molt_'));
    console.log(`Total exports: ${allExports.length} (${funcExports.length} functions)`);
    console.log(`PaddleOCR exports: ${paddleExports.map(e => e.name).join(', ')}`);
    console.log(`Runtime exports: ${runtimeExports.map(e => e.name).join(', ')}`);

    // --- Phase 2: Instantiation with WASI ---
    console.log('\n=== Phase 2: WASI Instantiation ===');
    const instantiateStart = performance.now();
    const wasi = new WASI({ version: 'preview1' });
    const memory = new WebAssembly.Memory({ initial: 1024, maximum: 65536 });
    const table = new WebAssembly.Table({ initial: 10000, element: 'anyfunc' });

    const env = {
        memory,
        __indirect_function_table: table,
    };

    // Add all required host imports as stubs
    const imports = WebAssembly.Module.imports(mod);
    const importsByModule = {};
    for (const imp of imports) {
        if (!importsByModule[imp.module]) importsByModule[imp.module] = [];
        importsByModule[imp.module].push(imp);
        if (imp.module === 'env' && !env[imp.name] && imp.kind === 'function') {
            env[imp.name] = (...args) => 0n;
        }
    }
    console.log(`Import modules: ${Object.keys(importsByModule).join(', ')}`);
    for (const [mod, imps] of Object.entries(importsByModule)) {
        console.log(`  ${mod}: ${imps.length} imports (${imps.filter(i => i.kind === 'function').length} functions)`);
    }

    try {
        const instance = new WebAssembly.Instance(mod, {
            env,
            wasi_snapshot_preview1: wasi.wasiImport,
        });
        const instantiateEnd = performance.now();
        console.log(`Instantiate: ${(instantiateEnd - instantiateStart).toFixed(1)} ms`);
        console.log(`Total startup: ${(instantiateEnd - compileStart).toFixed(1)} ms`);

        // --- Phase 3: Export availability check ---
        console.log('\n=== Phase 3: Export Availability ===');
        const paddleFuncs = {
            init: instance.exports['tinygrad_paddleocr_driver__init'],
            init_full: instance.exports['tinygrad_paddleocr_driver__init_full'],
            ocr: instance.exports['tinygrad_paddleocr_driver__ocr'],
            detect_only: instance.exports['tinygrad_paddleocr_driver__detect_only'],
            rgb_to_tensor: instance.exports['tinygrad_paddleocr_driver___rgb_bytes_to_tensor'],
        };
        for (const [name, fn] of Object.entries(paddleFuncs)) {
            console.log(`  ${name}: ${fn ? 'AVAILABLE' : 'MISSING'}`);
        }

        const runtimeFuncs = {
            molt_main: instance.exports['molt_main'],
            molt_alloc: instance.exports['molt_alloc'],
            molt_isolate_bootstrap: instance.exports['molt_isolate_bootstrap'],
        };
        for (const [name, fn] of Object.entries(runtimeFuncs)) {
            console.log(`  ${name}: ${fn ? 'AVAILABLE' : 'MISSING'}`);
        }

        // --- Phase 4: Weight file sizes ---
        console.log('\n=== Phase 4: Weight Budget ===');
        const detSize = detPath ? fs.statSync(detPath).size : 0;
        const recSize = recPath ? fs.statSync(recPath).size : 0;
        const totalWeights = detSize + recSize;
        console.log(`Detector weights:   ${(detSize / 1e6).toFixed(1)} MB`);
        console.log(`Recognizer weights: ${(recSize / 1e6).toFixed(1)} MB`);
        console.log(`Total weights:      ${(totalWeights / 1e6).toFixed(1)} MB`);
        console.log(`WASM binary:        ${(wasmBytes.length / 1e6).toFixed(1)} MB`);
        console.log(`Total payload:      ${((totalWeights + wasmBytes.length) / 1e6).toFixed(1)} MB`);

    } catch (e) {
        console.log(`Instantiation error: ${e.message}`);
    }
}

main().catch(console.error);
