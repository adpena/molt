use super::add_reloc_sections;
use super::symbols::is_manifest_call_indirect_import_name;
use crate::wasm_abi::{NATIVE_CALLABLE_IMPORT_MODULE, RUNTIME_IMPORT_MODULE, TypeSectionExt};
use crate::wasm_binary::{emit_call_indirect, encode_u32_leb128_padded, strip_unused_imports};
use crate::wasm_data::WasmDataSegments;
use crate::wasm_table::{
    TableRelocSite, WasmCallableTableAddress, WasmCallableTableRole, WasmCallableTableTarget,
    WasmFunctionSymbol, WasmTableRelocations,
};
use std::borrow::Cow;
use std::collections::BTreeSet;
use std::fs;
use std::process::Command;
use wasm_encoder::{
    CodeSection, ConstExpr, CustomSection, ElementMode, ElementSection, ElementSegment, Elements,
    Encode, EntityType, ExportKind, ExportSection, Function, FunctionSection, ImportSection,
    Instruction, Module, RawSection, RefType, TableType, TypeSection,
};
use wasmparser::{Parser, Payload};

fn read_varuint(data: &[u8], mut offset: usize) -> (u32, usize) {
    let mut result = 0u32;
    let mut shift = 0u32;
    loop {
        let byte = data[offset];
        offset += 1;
        result |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return (result, offset);
        }
        shift += 7;
    }
}

fn bytes_contain_ascii(bytes: &[u8], needle: &str) -> bool {
    bytes
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

fn code_body_ranges(wasm: &[u8]) -> (usize, Vec<std::ops::Range<usize>>) {
    let mut code_start = None;
    let mut ranges = Vec::new();
    for payload in Parser::new(0).parse_all(wasm) {
        match payload.expect("valid wasm payload") {
            Payload::CodeSectionStart { range, .. } => {
                code_start = Some(range.start);
            }
            Payload::CodeSectionEntry(body) => {
                ranges.push(body.range());
            }
            _ => {}
        }
    }
    (code_start.expect("code section start"), ranges)
}

fn reloc_code_memory_addr_offsets(wasm: &[u8]) -> Vec<u32> {
    let mut offsets = Vec::new();
    for payload in Parser::new(0).parse_all(wasm) {
        let Payload::CustomSection(section) = payload.expect("valid wasm payload") else {
            continue;
        };
        if section.name() != "reloc.CODE" {
            continue;
        }
        let data = section.data();
        let (_, cursor) = read_varuint(data, 0);
        let (count, mut cursor) = read_varuint(data, cursor);
        for _ in 0..count {
            let ty = data[cursor];
            cursor += 1;
            let (offset, next) = read_varuint(data, cursor);
            cursor = next;
            let (_, next) = read_varuint(data, cursor);
            cursor = next;
            if matches!(ty, 4 | 5) {
                let (_, next) = read_varuint(data, cursor);
                cursor = next;
            }
            if ty == 4 {
                offsets.push(offset);
            }
        }
    }
    offsets
}

fn reloc_code_entries(wasm: &[u8], requested_type: u8) -> Vec<(u32, u32)> {
    let mut entries = Vec::new();
    for payload in Parser::new(0).parse_all(wasm) {
        let Payload::CustomSection(section) = payload.expect("valid wasm payload") else {
            continue;
        };
        if section.name() != "reloc.CODE" {
            continue;
        }
        let data = section.data();
        let (_, cursor) = read_varuint(data, 0);
        let (count, mut cursor) = read_varuint(data, cursor);
        for _ in 0..count {
            let ty = data[cursor];
            cursor += 1;
            let (offset, next) = read_varuint(data, cursor);
            cursor = next;
            let (index, next) = read_varuint(data, cursor);
            cursor = next;
            if matches!(ty, 4 | 5) {
                let (_, next) = read_varuint(data, cursor);
                cursor = next;
            }
            if ty == requested_type {
                entries.push((offset, index));
            }
        }
    }
    entries
}

#[test]
fn call_indirect_symbol_preservation_uses_manifest_membership() {
    assert!(is_manifest_call_indirect_import_name("molt_call_indirect0"));
    assert!(is_manifest_call_indirect_import_name(
        "molt_call_indirect13"
    ));
    assert!(!is_manifest_call_indirect_import_name(
        "molt_call_indirect99"
    ));
    assert!(!is_manifest_call_indirect_import_name("molt_call_indirect"));
}

#[test]
fn reloc_symbol_table_preserves_native_callable_import_symbols() {
    let mut types = TypeSection::new();
    types.function([], []);

    let mut imports = ImportSection::new();
    imports.import(
        RUNTIME_IMPORT_MODULE,
        "types_bootstrap",
        EntityType::Function(0),
    );
    imports.import(
        NATIVE_CALLABLE_IMPORT_MODULE,
        "PyInit__nd_image",
        EntityType::Function(0),
    );

    let mut funcs = FunctionSection::new();
    funcs.function(0);

    let mut body = Function::new([]);
    body.instruction(&Instruction::Call(0));
    body.instruction(&Instruction::Call(1));
    body.instruction(&Instruction::End);
    let mut codes = CodeSection::new();
    codes.function(&body);

    let mut module = Module::new();
    module.section(&types);
    module.section(&imports);
    module.section(&funcs);
    module.section(&codes);

    let relocated = add_reloc_sections(module.finish(), &[], &[], &[]);
    assert!(
        bytes_contain_ascii(&relocated, "PyInit__nd_image"),
        "native callable import symbol must be preserved for object-closure linking"
    );
}

#[test]
#[should_panic(expected = "unsupported WASM function import module")]
fn reloc_symbol_table_rejects_unknown_function_import_modules() {
    let mut types = TypeSection::new();
    types.function([], []);

    let mut imports = ImportSection::new();
    imports.import("env", "PyInit__nd_image", EntityType::Function(0));

    let mut funcs = FunctionSection::new();
    funcs.function(0);

    let mut body = Function::new([]);
    body.instruction(&Instruction::Call(0));
    body.instruction(&Instruction::End);
    let mut codes = CodeSection::new();
    codes.function(&body);

    let mut module = Module::new();
    module.section(&types);
    module.section(&imports);
    module.section(&funcs);
    module.section(&codes);

    let _ = add_reloc_sections(module.finish(), &[], &[], &[]);
}

#[test]
fn data_reloc_sites_follow_defined_body_ordinal_after_import_strip() {
    let mut types = TypeSection::new();
    types.function([], []);

    let mut imports = ImportSection::new();
    imports.import(
        RUNTIME_IMPORT_MODULE,
        "types_bootstrap",
        EntityType::Function(0),
    );
    imports.import(
        RUNTIME_IMPORT_MODULE,
        "abc_bootstrap",
        EntityType::Function(0),
    );

    let mut funcs = FunctionSection::new();
    funcs.function(0);
    funcs.function(0);

    let mut codes = CodeSection::new();
    let mut first = Function::new([]);
    first.instruction(&Instruction::End);
    codes.function(&first);

    let mut data_segments = WasmDataSegments::new(64 * 1024 * 1024);
    let data_ref = data_segments.add_segment(true, b"molt");
    let mut second = Function::new([]);
    data_segments.emit_ptr_i32(true, 1, &mut second, data_ref);
    second.instruction(&Instruction::Drop);
    second.instruction(&Instruction::End);
    codes.function(&second);

    let mut module = Module::new();
    module.section(&types);
    module.section(&imports);
    module.section(&funcs);
    module.section(&codes);
    module.section(data_segments.section());

    let mut unused = BTreeSet::new();
    unused.insert("types_bootstrap".to_string());
    let stripped = strip_unused_imports(module.finish(), &unused);
    let relocated = add_reloc_sections(
        stripped,
        data_segments.segments(),
        data_segments.relocs(),
        &[],
    );

    let (code_start, body_ranges) = code_body_ranges(&relocated);
    assert_eq!(body_ranges.len(), 2);
    let second_body = &body_ranges[1];
    let second_start = (second_body.start - code_start) as u32;
    let second_end = (second_body.end - code_start) as u32;

    let offsets = reloc_code_memory_addr_offsets(&relocated);
    assert_eq!(offsets.len(), 1);
    assert!(
        (second_start..second_end).contains(&offsets[0]),
        "data relocation offset {} must target second defined body range {}..{}",
        offsets[0],
        second_start,
        second_end,
    );
}

#[test]
fn table_reloc_sites_use_symbol_identity_after_import_strip() {
    let mut types = TypeSection::new();
    types.function([], []);

    let mut imports = ImportSection::new();
    imports.import(
        RUNTIME_IMPORT_MODULE,
        "types_bootstrap",
        EntityType::Function(0),
    );
    imports.import(
        RUNTIME_IMPORT_MODULE,
        "abc_bootstrap",
        EntityType::Function(0),
    );

    let mut funcs = FunctionSection::new();
    funcs.function(0);
    funcs.function(0);

    let mut codes = CodeSection::new();
    let mut first = Function::new([]);
    first.instruction(&Instruction::End);
    codes.function(&first);

    let mut table_relocations = WasmTableRelocations::default();
    let mut second = Function::new([]);
    table_relocations.emit_i64(
        true,
        2,
        3,
        &mut second,
        &WasmCallableTableTarget {
            current_table_index: 4101,
            address: WasmCallableTableAddress::Relocatable(WasmFunctionSymbol::RuntimeImport(
                crate::wasm_abi_generated::wasm_runtime_import("abc_bootstrap")
                    .expect("generated abc_bootstrap import"),
            )),
            role: WasmCallableTableRole::DirectCallable,
        },
    );
    second.instruction(&Instruction::Drop);
    table_relocations.emit_i64(
        true,
        2,
        3,
        &mut second,
        &WasmCallableTableTarget {
            current_table_index: 4102,
            address: WasmCallableTableAddress::Relocatable(WasmFunctionSymbol::Defined {
                defined_func_index: 0,
            }),
            role: WasmCallableTableRole::Trampoline,
        },
    );
    second.instruction(&Instruction::Drop);
    second.instruction(&Instruction::End);
    codes.function(&second);

    let mut module = Module::new();
    module.section(&types);
    module.section(&imports);
    module.section(&funcs);
    module.section(&codes);

    let mut unused = BTreeSet::new();
    unused.insert("types_bootstrap".to_string());
    let stripped = strip_unused_imports(module.finish(), &unused);
    let relocated = add_reloc_sections(stripped, &[], &[], table_relocations.relocs());

    let (code_start, body_ranges) = code_body_ranges(&relocated);
    let second_start = (body_ranges[1].start - code_start) as u32;
    let second_end = (body_ranges[1].end - code_start) as u32;
    let entries = reloc_code_entries(&relocated, 1);

    assert_eq!(
        entries.len(),
        2,
        "both callable addresses need R_WASM_TABLE_INDEX_SLEB"
    );
    assert!(
        entries
            .iter()
            .all(|(offset, _)| (second_start..second_end).contains(offset)),
        "table relocation offsets must remain in the owning defined body after import stripping"
    );
    assert_eq!(
        entries
            .iter()
            .map(|(_, symbol)| *symbol)
            .collect::<Vec<_>>(),
        vec![0, 1],
        "the surviving runtime import and first defined function must resolve through their post-strip symbol identities"
    );
}

#[test]
fn fixed_shared_runtime_table_address_emits_no_linker_relocation() {
    let mut relocations = WasmTableRelocations::default();
    let mut body = Function::new([]);
    relocations.emit_i64(
        true,
        0,
        0,
        &mut body,
        &WasmCallableTableTarget {
            current_table_index: 4097,
            address: WasmCallableTableAddress::FixedSharedRuntimeAbi {
                finalized_app_base: 8192,
            },
            role: WasmCallableTableRole::DirectCallable,
        },
    );

    assert!(
        relocations.relocs().is_empty(),
        "the explicitly fixed split-runtime ABI prefix must not be handed to the linker"
    );
}

#[test]
#[should_panic(expected = "callable-table relocation owner body 1 is missing")]
fn missing_callable_table_relocation_owner_fails_closed() {
    let mut types = TypeSection::new();
    types.ty().function([], []);
    let mut functions = FunctionSection::new();
    functions.function(0);
    let mut codes = CodeSection::new();
    let mut body = Function::new([]);
    body.instruction(&Instruction::End);
    codes.function(&body);
    let mut module = Module::new();
    module.section(&types);
    module.section(&functions);
    module.section(&codes);

    let missing_owner = TableRelocSite {
        defined_func_index: 1,
        offset_in_func: 1,
        target: WasmFunctionSymbol::Defined {
            defined_func_index: 0,
        },
        role: WasmCallableTableRole::DirectCallable,
    };
    add_reloc_sections(module.finish(), &[], &[], &[missing_owner]);
}

#[test]
fn wasm_ld_applies_shifted_table_relocations_to_indirect_calls() {
    let wasm_ld_probe = Command::new("wasm-ld").arg("--version").output();
    if !matches!(wasm_ld_probe, Ok(output) if output.status.success()) {
        if std::env::var_os("CI").is_some()
            || std::env::var_os("MOLT_REQUIRE_REAL_WASM_LD_TESTS").is_some()
        {
            panic!("real wasm-ld integration proof is required but wasm-ld is unavailable");
        }
        eprintln!(
            "SKIP real wasm-ld integration proof: wasm-ld is unavailable; set \
             MOLT_REQUIRE_REAL_WASM_LD_TESTS=1 to make this a hard failure"
        );
        return;
    }

    const FIXED_RUNTIME_SLOT: u32 = 3;
    const FINALIZED_APP_BASE: u32 = 8;

    let mut types = TypeSection::new();
    types.ty().function([], [wasm_encoder::ValType::I32]);
    let mut funcs = FunctionSection::new();
    for _ in 0..3 {
        funcs.function(0);
    }
    let mut imports = ImportSection::new();
    imports.import(
        "env",
        "__indirect_function_table",
        EntityType::Table(TableType {
            element_type: RefType::FUNCREF,
            table64: false,
            minimum: 0,
            maximum: None,
            shared: false,
        }),
    );
    let mut exports = ExportSection::new();
    for (name, index) in [
        ("candidate_target", 0),
        ("prefix_invoke", 1),
        ("candidate_invoke", 2),
    ] {
        exports.export(name, ExportKind::Func, index);
    }

    let mut relocations = WasmTableRelocations::default();
    let mut codes = CodeSection::new();
    let mut candidate_target = Function::new([]);
    candidate_target.instruction(&Instruction::I32Const(42));
    candidate_target.instruction(&Instruction::End);
    codes.function(&candidate_target);

    let mut prefix_invoke = Function::new([]);
    relocations.emit_i32(
        true,
        0,
        1,
        &mut prefix_invoke,
        &WasmCallableTableTarget {
            current_table_index: FIXED_RUNTIME_SLOT,
            address: WasmCallableTableAddress::FixedSharedRuntimeAbi {
                finalized_app_base: FINALIZED_APP_BASE,
            },
            role: WasmCallableTableRole::DirectCallable,
        },
    );
    emit_call_indirect(&mut prefix_invoke, true, 0, 0);
    prefix_invoke.instruction(&Instruction::End);
    codes.function(&prefix_invoke);

    let mut candidate_invoke = Function::new([]);
    for _ in 0..4096 {
        candidate_invoke.instruction(&Instruction::Nop);
    }
    relocations.emit_i32(
        true,
        0,
        2,
        &mut candidate_invoke,
        &WasmCallableTableTarget {
            current_table_index: 0,
            address: WasmCallableTableAddress::Relocatable(WasmFunctionSymbol::Defined {
                defined_func_index: 0,
            }),
            role: WasmCallableTableRole::DirectCallable,
        },
    );
    emit_call_indirect(&mut candidate_invoke, true, 0, 0);
    candidate_invoke.instruction(&Instruction::End);
    codes.function(&candidate_invoke);

    let mut module = Module::new();
    module.section(&types);
    module.section(&imports);
    module.section(&funcs);
    module.section(&exports);
    module.section(&CustomSection {
        name: "molt.callable_table.layout".into(),
        data: Cow::Borrowed(&[1, 0, 0, 8, 1]),
    });
    let mut passive_elements = Vec::new();
    1u32.encode(&mut passive_elements);
    passive_elements.push(0x01);
    passive_elements.push(0x00);
    1u32.encode(&mut passive_elements);
    encode_u32_leb128_padded(0, &mut passive_elements);
    module.section(&RawSection {
        id: 9,
        data: &passive_elements,
    });
    module.section(&codes);
    let object = add_reloc_sections(module.finish(), &[], &[], relocations.relocs());

    let mut runtime_elements = ElementSection::new();
    let runtime_offset = ConstExpr::i32_const(FIXED_RUNTIME_SLOT as i32);
    runtime_elements.segment(ElementSegment {
        mode: ElementMode::Active {
            table: Some(0),
            offset: &runtime_offset,
        },
        elements: Elements::Functions(Cow::Owned(vec![0])),
    });
    let mut runtime_code = CodeSection::new();
    let mut runtime_target = Function::new([]);
    runtime_target.instruction(&Instruction::I32Const(7));
    runtime_target.instruction(&Instruction::End);
    runtime_code.function(&runtime_target);
    let mut runtime = Module::new();
    runtime.section(&types);
    runtime.section(&imports);
    let mut runtime_funcs = FunctionSection::new();
    runtime_funcs.function(0);
    runtime.section(&runtime_funcs);
    runtime.section(&runtime_elements);
    runtime.section(&runtime_code);
    let runtime = runtime.finish();

    let temp = std::env::temp_dir().join(format!(
        "molt-wasm-table-reloc-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock after epoch")
            .as_nanos()
    ));
    fs::create_dir_all(&temp).expect("create wasm-ld integration temp dir");
    struct RemoveTemp(std::path::PathBuf);
    impl Drop for RemoveTemp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _remove_temp = RemoveTemp(temp.clone());
    let object_path = temp.join("callable.o.wasm");
    let linked_path = temp.join("callable.wasm");
    let runtime_path = temp.join("runtime.wasm");
    fs::write(&object_path, object).expect("write relocatable wasm object");
    fs::write(&runtime_path, runtime).expect("write fixed-prefix runtime wasm");
    let status = Command::new("wasm-ld")
        .args([
            "--no-entry",
            "--import-table",
            "--table-base=8",
            "--export=__molt_output_export_1",
            "--export=__molt_output_export_2",
            "-o",
        ])
        .arg(&linked_path)
        .arg(&object_path)
        .status()
        .expect("run wasm-ld");
    assert!(status.success(), "wasm-ld must link table relocations");

    let script = temp.join("verify.js");
    fs::write(
        &script,
        r#"const fs=require('fs');
const bytes=fs.readFileSync(process.argv[2]);
const runtimeBytes=fs.readFileSync(process.argv[3]);
const table=new WebAssembly.Table({initial:16,element:'anyfunc'});
WebAssembly.instantiate(runtimeBytes,{env:{__indirect_function_table:table}})
.then(()=>WebAssembly.instantiate(bytes,{env:{__indirect_function_table:table}})).then(({instance})=>{
  const prefix=instance.exports.prefix_invoke();
  const candidate=instance.exports.candidate_invoke();
  if(prefix!==7 || candidate!==42) throw new Error(`bad results ${prefix},${candidate}`);
}).catch(error=>{console.error(error);process.exit(1);});
"#,
    )
    .expect("write node relocation verifier");
    let node_status = Command::new("node")
        .arg(&script)
        .arg(&linked_path)
        .arg(&runtime_path)
        .status()
        .expect("run node relocation verifier");
    assert!(
        node_status.success(),
        "both shifted indirect calls must reach their linker-assigned targets"
    );
}
