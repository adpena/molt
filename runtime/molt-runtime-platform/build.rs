use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn resolve_build_python() -> String {
    println!("cargo:rerun-if-env-changed=MOLT_BUILD_PYTHON");
    println!("cargo:rerun-if-env-changed=PYTHON");
    for key in ["MOLT_BUILD_PYTHON", "PYTHON"] {
        if let Ok(value) = env::var(key) {
            let value = value.trim();
            if !value.is_empty() {
                return value.to_string();
            }
        }
    }
    if cfg!(windows) {
        "python".to_string()
    } else {
        "python3".to_string()
    }
}

fn main() {
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    if target_arch != "wasm32" {
        emit_errno_constants();
    }
    println!("cargo:rerun-if-changed=build.rs");
}

fn emit_errno_constants() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR missing"));
    let build_python = resolve_build_python();
    let output = Command::new(&build_python)
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(mut stdin) = child.stdin.take() {
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
                stdin.write_all(script.as_bytes())?;
            }
            child.wait_with_output()
        });
    let output = match output {
        Ok(out) => out,
        Err(err) => {
            panic!(
                "failed to run build Python `{build_python}` to generate errno constants: {err}"
            );
        }
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!("build Python `{build_python}` errno generation failed: {stderr}");
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
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
