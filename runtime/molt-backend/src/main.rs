use std::io::{self, Read};
use molt_backend::{SimpleBackend, SimpleIR};
use molt_backend::wasm::WasmBackend;
use std::fs::File;
use std::io::Write;
use std::env;

fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().collect();
    let is_wasm = args.contains(&"--target".to_string()) && args.contains(&"wasm".to_string());

    let mut buffer = String::new();
    io::stdin().read_to_string(&mut buffer)?;

    let ir: SimpleIR = serde_json::from_str(&buffer).expect("Invalid IR JSON");
    
    let output_file = if is_wasm { "output.wasm" } else { "output.o" };
    let mut file = File::create(output_file)?;

    if is_wasm {
        let backend = WasmBackend::new();
        let wasm_bytes = backend.compile(ir);
        file.write_all(&wasm_bytes)?;
        println!("Successfully compiled to output.wasm");
    } else {
        let backend = SimpleBackend::new();
        let obj_bytes = backend.compile(ir);
        file.write_all(&obj_bytes)?;
        println!("Successfully compiled to output.o");
    }
    
    Ok(())
}