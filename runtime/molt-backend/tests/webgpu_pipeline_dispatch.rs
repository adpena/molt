//! Integration coverage for WebGPU dispatch and pipeline glue.

use molt_backend::tir::gpu_dispatch::compile_and_launch_wgsl;
use molt_backend::tir::gpu_pipeline::execute_webgpu_wgsl_kernel;

#[test]
#[cfg(feature = "gpu-webgpu")]
fn dispatch_compile_and_launch_wgsl_noop() {
    let wgsl = "@compute @workgroup_size(1) fn main() {}";
    compile_and_launch_wgsl("main", wgsl, [1, 1, 1], [1, 1, 1], &[])
        .expect("dispatch WGSL path should compile and launch");
}

#[test]
#[cfg(feature = "gpu-webgpu")]
fn pipeline_execute_webgpu_wgsl_roundtrip_u32() {
    let wgsl = r#"
@group(0) @binding(0) var<storage, read> in_buf: array<u32>;
@group(0) @binding(1) var<storage, read_write> out_buf: array<u32>;
@compute @workgroup_size(1)
fn copy_first(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x == 0u) {
        out_buf[0] = in_buf[0];
    }
}
"#;
    let value: u32 = 0x1234_5678;
    let input = value.to_le_bytes();
    let output = execute_webgpu_wgsl_kernel(
        "copy_first",
        wgsl,
        &[&input],
        std::mem::size_of::<u32>(),
        [1, 1, 1],
        [1, 1, 1],
    )
    .expect("pipeline WGSL path should execute");
    let got = u32::from_le_bytes(output.as_slice().try_into().expect("u32 output"));
    assert_eq!(got, value);
}

#[test]
#[cfg(not(feature = "gpu-webgpu"))]
fn dispatch_and_pipeline_require_webgpu_feature() {
    let dispatch_err = compile_and_launch_wgsl(
        "main",
        "@compute @workgroup_size(1) fn main() {}",
        [1, 1, 1],
        [1, 1, 1],
        &[],
    )
    .expect_err("dispatch should require gpu-webgpu");
    assert!(
        dispatch_err.to_string().contains("gpu-webgpu"),
        "unexpected dispatch error: {dispatch_err}"
    );

    let pipeline_err = execute_webgpu_wgsl_kernel(
        "main",
        "@compute @workgroup_size(1) fn main() {}",
        &[],
        0,
        [1, 1, 1],
        [1, 1, 1],
    )
    .expect_err("pipeline should require gpu-webgpu");
    assert!(
        pipeline_err.to_string().contains("gpu-webgpu"),
        "unexpected pipeline error: {pipeline_err}"
    );
}
