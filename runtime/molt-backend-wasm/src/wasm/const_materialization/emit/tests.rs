use crate::OpIR;
use crate::wasm::const_materialization::WasmConstOpPolicy;
use crate::wasm::test_execution::{
    run_execution_command, run_node_test_script, wasm_ld_path, wasm_test_temp_dir,
};
use crate::wasm::{WasmBackend, WasmCompileOptions};
use crate::wasm_abi::RUNTIME_IMPORT_MODULE;
use crate::wasm_binary::add_reloc_sections;
use crate::wasm_data::WasmDataSegments;
use serde_json::json;
use std::{fs, path::PathBuf, process::Command};
use wasm_encoder::{
    CodeSection, EntityType, ExportKind, ExportSection, Function, FunctionSection, ImportSection,
    Instruction, MemoryType, Module, TypeSection, ValType,
};

#[test]
fn literal_operands_preserve_payloads_and_output_after_linker_data_shift() {
    let (temp, _remove_temp) = wasm_test_temp_dir();
    // Independent source bytes, including empty payloads, binary data and
    // minimal positive/negative signed bigint encodings. No emitted address or
    // materializer result supplies the oracle.
    let cases = [
        (molt_ir::OpKind::ConstStr, vec![]),
        (molt_ir::OpKind::ConstStr, "a\0β雪".as_bytes().to_vec()),
        (molt_ir::OpKind::ConstBytes, vec![0, 255, 128, 1]),
        (molt_ir::OpKind::ConstBytes, vec![]),
        (molt_ir::OpKind::ConstBigint, vec![0, 0x80]),
        (molt_ir::OpKind::ConstBigint, vec![0xff, 0x7f]),
    ];
    let mut config = Vec::new();
    for reloc in [false, true] {
        let mut backend = WasmBackend::with_options(WasmCompileOptions {
            reloc_enabled: reloc,
            ..WasmCompileOptions::default()
        });
        backend.func_import_count = 3;
        backend.data_segments = WasmDataSegments::new(64);
        let scratch = backend.add_data_segment_mutable(reloc, &[0; 8]);
        let mut types = TypeSection::new();
        types
            .ty()
            .function([ValType::I32, ValType::I64, ValType::I32], [ValType::I32]);
        types
            .ty()
            .function([ValType::I32, ValType::I64], [ValType::I64]);
        types.ty().function([], [ValType::I64]);
        let mut imports = ImportSection::new();
        for (name, ty) in [
            ("string_from_bytes", 0),
            ("bytes_from_bytes", 0),
            ("bigint_from_bytes", 1),
        ] {
            imports.import(RUNTIME_IMPORT_MODULE, name, EntityType::Function(ty));
        }
        imports.import(
            "env",
            "memory",
            EntityType::Memory(MemoryType {
                minimum: 1,
                maximum: None,
                memory64: false,
                shared: false,
                page_size_log2: None,
            }),
        );
        let mut functions = FunctionSection::new();
        let mut exports = ExportSection::new();
        let mut code = CodeSection::new();
        for (index, (kind, bytes)) in cases.iter().enumerate() {
            let op = OpIR {
                out: Some("result".into()),
                bytes: Some(bytes.clone()),
                ..OpIR::new(*kind)
            };
            let policy = WasmConstOpPolicy::for_op(&op).unwrap();
            let mut function = Function::new([(1, ValType::I64)]);
            let import = match *kind {
                molt_ir::OpKind::ConstStr => 0,
                molt_ir::OpKind::ConstBytes => 1,
                molt_ir::OpKind::ConstBigint => 2,
                _ => unreachable!(),
            };
            policy.simple_ir_materialization_into(&op, 0).emit(
                &mut backend,
                &mut function,
                index as u32 + 3,
                reloc,
                import,
                scratch,
            );
            function.instruction(&Instruction::LocalGet(0));
            function.instruction(&Instruction::End);
            functions.function(2);
            exports.export(
                &format!("literal_{index}"),
                ExportKind::Func,
                index as u32 + 3,
            );
            code.function(&function);
        }
        let mut module = Module::new();
        module.section(&types);
        module.section(&imports);
        module.section(&functions);
        module.section(&exports);
        module.section(&code);
        module.section(backend.data_segments.section());
        let wasm = module.finish();
        let path = temp.join(format!("literal-{reloc}.wasm"));
        if reloc {
            let object = add_reloc_sections(
                wasm,
                backend.data_segments.segments(),
                backend.data_segments.relocs(),
                &[],
            );
            let object_path = temp.join("literal.o.wasm");
            fs::write(&object_path, object).unwrap();
            // A nonzero, non-page-aligned base exposes stale absolute pointers,
            // including both references to the independent output segment.
            run_execution_command(
                Command::new(wasm_ld_path())
                    .args([
                        "--no-entry",
                        "--import-memory",
                        "--allow-undefined",
                        "--global-base=8192",
                        "-z",
                        "stack-size=4096",
                        "--export-all",
                        "-o",
                    ])
                    .arg(&path)
                    .arg(&object_path),
                "link relocated literal operands",
            );
        } else {
            fs::write(&path, wasm).unwrap();
        }
        let wasm = fs::read(&path).unwrap();
        wasmparser::Validator::new().validate_all(&wasm).unwrap();
        config.push(json!({"path":path,"reloc":reloc}));
    }
    let config_path = temp.join("cases.json");
    fs::write(
        &config_path,
        serde_json::to_vec(&json!({"modules":config,"cases":cases})).unwrap(),
    )
    .unwrap();
    run_node_test_script(
        &PathBuf::from("node"),
        r#"
const fs=require('fs'), assert=require('assert/strict');
const config=JSON.parse(fs.readFileSync(process.argv[1],'utf8'));
for (const test of config.modules) {
  const module=new WebAssembly.Module(fs.readFileSync(test.path));
  const memory=new WebAssembly.Memory({initial:4});
  let expected, calls=0, output=null;
  const view=()=>new DataView(memory.buffer);
  const read=(kind,ptr,len)=>{
    assert.equal(kind,expected[0]); assert.equal(Number(len),expected[1].length);
    assert.ok(test.reloc ? ptr>=8192 : ptr<8192, 'payload address must follow actual linker layout');
    assert.deepEqual([...new Uint8Array(memory.buffer,ptr,Number(len))],expected[1]);
    calls++; return 0x123400000000n+BigInt(calls);
  };
  const stored=(kind,ptr,len,out)=>{
    const result=read(kind,ptr,len);
    assert.ok(test.reloc ? out>=8192 : out<8192, 'output segment must relocate independently');
    assert.equal(out%8,0); output=out;
    view().setBigUint64(out,result,true); return -99; // raw return is not the boxed owner
  };
  const runtime={string_from_bytes:(p,l,o)=>stored('const_str',p,l,o),bytes_from_bytes:(p,l,o)=>stored('const_bytes',p,l,o),bigint_from_bytes:(p,l)=>read('const_bigint',p,l)};
  const imports={};
  for (const imp of WebAssembly.Module.imports(module)) {
    imports[imp.module]??={};
    imports[imp.module][imp.name]=imp.kind==='memory' ? memory : runtime[imp.name];
    assert.ok(imports[imp.module][imp.name], 'unexpected import '+imp.name);
  }
  const app=new WebAssembly.Instance(module,imports).exports;
  for (let index=0;index<config.cases.length;index++) {
    expected=config.cases[index];
    if(output!==null) view().setBigUint64(output,0xdeadn,true);
    const fn=app['literal_'+index]??app['__molt_output_export_'+index];
    assert.equal(typeof fn,'function');
    assert.equal(fn(),0x123400000000n+BigInt(index+1), 'must reload fresh output, preserving full i64');
  }
  assert.equal(calls,config.cases.length);
}
"#,
        &[&config_path],
        "literal operand bytes and shifted data relocation",
    );
}
