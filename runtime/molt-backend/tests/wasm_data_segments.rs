#![cfg(feature = "wasm-backend")]

//! Tests for WASM data segment layout: string constants go into data segments,
//! data offsets are 8-byte aligned, the manifest segment exists, and duplicate
//! strings are deduplicated.

use molt_backend::wasm::WasmBackend;
use molt_backend::{FunctionIR, OpIR, SimpleIR};
use wasmparser::{DataKind, Parser, Payload};

fn op(kind: &str) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        ..OpIR::default()
    }
}

fn use_value(name: &str) -> OpIR {
    let mut op = op("print_obj");
    op.args = Some(vec![name.to_string()]);
    op
}

fn compile_ir(ir: SimpleIR) -> Vec<u8> {
    WasmBackend::new().compile(ir)
}

struct DataSegment {
    offset: u32,
    data: Vec<u8>,
}

fn extract_data_segments(wasm: &[u8]) -> Vec<DataSegment> {
    let mut segments = Vec::new();
    for payload in Parser::new(0).parse_all(wasm) {
        if let Payload::DataSection(section) = payload.expect("valid payload") {
            for data in section.into_iter() {
                let data = data.expect("valid data");
                if let DataKind::Active {
                        memory_index: 0,
                        offset_expr,
                    } = data.kind {
                    // Parse the const expr to get the offset.
                    let mut reader = offset_expr.get_operators_reader();
                    let mut offset = 0u32;
                    while let Ok(op) = reader.read() {
                        match op {
                            wasmparser::Operator::I32Const { value } => {
                                offset = value as u32;
                            }
                            wasmparser::Operator::End => break,
                            _ => {}
                        }
                    }
                    segments.push(DataSegment {
                        offset,
                        data: data.data.to_vec(),
                    });
                }
            }
        }
    }
    segments
}

// -----------------------------------------------------------------------
// Data segment presence tests
// -----------------------------------------------------------------------

#[test]
fn empty_module_has_data_segments() {
    let wasm = compile_ir(SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![op("ret_void")],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    });

    let segments = extract_data_segments(&wasm);
    // Even an empty module should have at least the manifest segment
    // and the scratch buffers.
    assert!(
        !segments.is_empty(),
        "should have at least one data segment"
    );
}

#[test]
fn const_str_creates_data_segment_with_string_bytes() {
    let mut c = op("const_str");
    c.s_value = Some("hello".to_string());
    c.out = Some("v0".to_string());

    let wasm = compile_ir(SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![c, use_value("v0"), op("ret_void")],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    });

    let segments = extract_data_segments(&wasm);
    let has_hello = segments.iter().any(|seg| seg.data == b"hello");
    assert!(
        has_hello,
        "should have a data segment containing 'hello' bytes"
    );
}

#[test]
fn multiple_const_strs_create_separate_segments() {
    let mut c1 = op("const_str");
    c1.s_value = Some("alpha".to_string());
    c1.out = Some("v0".to_string());

    let mut c2 = op("const_str");
    c2.s_value = Some("beta".to_string());
    c2.out = Some("v1".to_string());

    let wasm = compile_ir(SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![c1, c2, use_value("v0"), use_value("v1"), op("ret_void")],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    });

    let segments = extract_data_segments(&wasm);
    let has_alpha = segments.iter().any(|seg| seg.data == b"alpha");
    let has_beta = segments.iter().any(|seg| seg.data == b"beta");
    assert!(has_alpha, "should have 'alpha' segment");
    assert!(has_beta, "should have 'beta' segment");
}

#[test]
fn duplicate_const_strs_are_deduplicated() {
    let mut c1 = op("const_str");
    c1.s_value = Some("dedup_test".to_string());
    c1.out = Some("v0".to_string());

    let mut c2 = op("const_str");
    c2.s_value = Some("dedup_test".to_string());
    c2.out = Some("v1".to_string());

    let wasm = compile_ir(SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![c1, c2, use_value("v0"), use_value("v1"), op("ret_void")],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    });

    let segments = extract_data_segments(&wasm);
    let dedup_count = segments
        .iter()
        .filter(|seg| seg.data == b"dedup_test")
        .count();
    assert_eq!(
        dedup_count, 1,
        "identical strings should be deduplicated into 1 segment, found {dedup_count}"
    );
}

// -----------------------------------------------------------------------
// Data segment alignment tests
// -----------------------------------------------------------------------

#[test]
fn data_segments_are_8_byte_aligned() {
    let mut c1 = op("const_str");
    c1.s_value = Some("short".to_string()); // 5 bytes
    c1.out = Some("v0".to_string());

    let mut c2 = op("const_str");
    c2.s_value = Some("longer_string_here".to_string()); // 18 bytes
    c2.out = Some("v1".to_string());

    let mut c3 = op("const_str");
    c3.s_value = Some("x".to_string()); // 1 byte
    c3.out = Some("v2".to_string());

    let wasm = compile_ir(SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![c1, c2, c3, op("ret_void")],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    });

    let segments = extract_data_segments(&wasm);
    for seg in &segments {
        assert_eq!(
            seg.offset % 8,
            0,
            "data segment at offset {} is not 8-byte aligned (data len={})",
            seg.offset,
            seg.data.len()
        );
    }
}

#[test]
fn data_segments_do_not_overlap() {
    let mut c1 = op("const_str");
    c1.s_value = Some("first_string".to_string());
    c1.out = Some("v0".to_string());

    let mut c2 = op("const_str");
    c2.s_value = Some("second_string_longer".to_string());
    c2.out = Some("v1".to_string());

    let wasm = compile_ir(SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![c1, c2, op("ret_void")],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    });

    let mut segments = extract_data_segments(&wasm);
    segments.sort_by_key(|s| s.offset);

    for i in 1..segments.len() {
        let prev_end = segments[i - 1].offset + segments[i - 1].data.len() as u32;
        assert!(
            segments[i].offset >= prev_end,
            "data segment {} (offset={}) overlaps with previous segment ending at {}",
            i,
            segments[i].offset,
            prev_end
        );
    }
}

// -----------------------------------------------------------------------
// Scratch buffer tests
// -----------------------------------------------------------------------

#[test]
fn scratch_buffer_for_const_str_exists() {
    // The const_str scratch slot is an 8-byte mutable data segment
    // used as the `out` parameter for string_from_bytes calls.
    let mut c = op("const_str");
    c.s_value = Some("test".to_string());
    c.out = Some("v0".to_string());

    let wasm = compile_ir(SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![c, use_value("v0"), op("ret_void")],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    });

    let segments = extract_data_segments(&wasm);
    // There should be a zeroed 8-byte segment (the scratch slot).
    let has_scratch = segments
        .iter()
        .any(|seg| seg.data.len() == 8 && seg.data.iter().all(|&b| b == 0));
    assert!(
        has_scratch,
        "should have an 8-byte zeroed scratch segment for const_str"
    );
}

// -----------------------------------------------------------------------
// Empty string constant test
// -----------------------------------------------------------------------

#[test]
fn empty_string_constant_does_not_allocate_payload_segment() {
    let mut c = op("const_str");
    c.s_value = Some(String::new());
    c.out = Some("v0".to_string());

    let wasm = compile_ir(SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![c, op("ret_void")],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        profile: None,
    });

    let segments = extract_data_segments(&wasm);
    let has_empty = segments.iter().any(|seg| seg.data.is_empty());
    assert!(
        !has_empty,
        "empty string payloads should not allocate zero-length data segments"
    );
}
