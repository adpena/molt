const fs = require('fs');

// Read the WGSL Conv2d shader from webgpu-engine.js
const engineSrc = fs.readFileSync('deploy/browser/webgpu-engine.js', 'utf8');

// Extract the Conv2d WGSL shader
const conv2dMatch = engineSrc.match(/CONV2D_WGSL\s*=\s*\/\*\s*wgsl\s*\*\/\s*`([\s\S]*?)`/);
if (conv2dMatch) {
    console.log('Conv2d WGSL shader found:', conv2dMatch[1].length, 'chars');
    console.log('Contains @compute:', conv2dMatch[1].includes('@compute'));
    console.log('Contains workgroup_size:', conv2dMatch[1].includes('workgroup_size'));
    console.log('Contains fma:', conv2dMatch[1].includes('fma'));
} else {
    console.log('Conv2d WGSL not found — checking for inline definition');
}

// Benchmark the CPU Conv2d for comparison
function cpuConv2d(input, weight, bias, H, W, C_in, C_out, KH, KW, stride, padding) {
    const OH = Math.floor((H + 2*padding - KH) / stride) + 1;
    const OW = Math.floor((W + 2*padding - KW) / stride) + 1;
    const output = new Float32Array(C_out * OH * OW);

    for (let oc = 0; oc < C_out; oc++) {
        for (let oh = 0; oh < OH; oh++) {
            for (let ow = 0; ow < OW; ow++) {
                let sum = bias ? bias[oc] : 0;
                for (let ic = 0; ic < C_in; ic++) {
                    for (let kh = 0; kh < KH; kh++) {
                        for (let kw = 0; kw < KW; kw++) {
                            const ih = oh * stride + kh - padding;
                            const iw = ow * stride + kw - padding;
                            if (ih >= 0 && ih < H && iw >= 0 && iw < W) {
                                sum += input[ic*H*W + ih*W + iw] *
                                       weight[oc*C_in*KH*KW + ic*KH*KW + kh*KW + kw];
                            }
                        }
                    }
                }
                output[oc*OH*OW + oh*OW + ow] = sum;
            }
        }
    }
    return output;
}

// Benchmark: PaddleOCR detector has 62 Conv layers
// Typical: 3x3 conv, 64->64 channels, 80x80 feature map
const H = 80, W = 80, C_in = 64, C_out = 64, KH = 3, KW = 3;
const input = new Float32Array(C_in * H * W).fill(0).map(() => Math.random());
const weight = new Float32Array(C_out * C_in * KH * KW).fill(0).map(() => Math.random());
const bias = new Float32Array(C_out).fill(0);

const iterations = 5;
const start = performance.now();
for (let i = 0; i < iterations; i++) {
    cpuConv2d(input, weight, bias, H, W, C_in, C_out, KH, KW, 1, 1);
}
const elapsed = performance.now() - start;
console.log(`\nCPU Conv2d (64->64, 3x3, 80x80):`);
console.log(`  ${(elapsed/iterations).toFixed(1)} ms/conv`);
console.log(`  ${(C_out * C_in * KH * KW * H * W * 2 / (elapsed/iterations) / 1e6).toFixed(1)} MFLOPS`);
console.log(`\nWebGPU target: < ${(elapsed/iterations/10).toFixed(1)} ms/conv (10x speedup)`);
