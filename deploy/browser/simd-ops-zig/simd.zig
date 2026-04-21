const std = @import("std");

export fn matmul_f32(a: [*]const f32, b: [*]const f32, out: [*]f32, m: u32, k: u32, n: u32) void {
    var i: u32 = 0;
    while (i < m) : (i += 1) {
        var j: u32 = 0;
        while (j < n) : (j += 1) {
            var sum: f32 = 0;
            var p: u32 = 0;
            while (p < k) : (p += 1) {
                sum += a[i * k + p] * b[p * n + j];
            }
            out[i * n + j] = sum;
        }
    }
}

export fn add_f32(a: [*]const f32, b: [*]const f32, out: [*]f32, n: u32) void {
    var i: u32 = 0;
    while (i < n) : (i += 1) {
        out[i] = a[i] + b[i];
    }
}

export fn softmax_f32(input: [*]const f32, out: [*]f32, n: u32) void {
    var max_val: f32 = input[0];
    var i: u32 = 1;
    while (i < n) : (i += 1) {
        if (input[i] > max_val) max_val = input[i];
    }
    var sum: f32 = 0;
    i = 0;
    while (i < n) : (i += 1) {
        out[i] = @exp2((input[i] - max_val) * 1.4426950408889634);
        sum += out[i];
    }
    i = 0;
    while (i < n) : (i += 1) {
        out[i] /= sum;
    }
}
