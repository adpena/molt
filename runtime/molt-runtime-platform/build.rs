use std::env;
use std::fs;
use std::path::PathBuf;

#[path = "../build_support/build_python.rs"]
mod build_python;

fn main() {
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    if target_arch != "wasm32" {
        emit_errno_constants();
    }
    println!("cargo:rerun-if-changed=build.rs");
}

fn emit_errno_constants() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR missing"));
    let build_python = build_python::resolve();
    let script = r#"
import errno
names = []
for name in dir(errno):
    if not name.startswith("E"):
        continue
    if not name[1:].isupper():
        continue
    val = getattr(errno, name)
    if isinstance(val, int):
        names.append((name, val))
for name, val in sorted(set(names)):
    print(f"{name},{val}")
"#;
    let stdout = build_python::run_script(&build_python, script, "errno constants");
    let mut entries: Vec<(String, i64)> = Vec::new();
    for line in stdout.lines() {
        let name = line.trim();
        if name.is_empty() {
            continue;
        }
        let Some((name, value)) = name.split_once(',') else {
            continue;
        };
        let value: i64 = match value.parse() {
            Ok(val) => val,
            Err(_) => continue,
        };
        entries.push((name.to_string(), value));
    }
    if entries.is_empty() {
        panic!("build Python `{build_python}` errno generation returned no entries");
    }
    let mut out = String::new();
    out.push_str("pub fn collect_errno_constants() -> Vec<(&'static str, i64)> {\n");
    out.push_str("    vec![\n");
    for (name, value) in entries {
        out.push_str(&format!("        (\"{name}\", {value}i64),\n"));
    }
    out.push_str("    ]\n");
    out.push_str("}\n");
    fs::write(out_dir.join("errno_constants.rs"), out).expect("failed to write errno_constants.rs");
}
