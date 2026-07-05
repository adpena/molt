use std::env;
use std::fs;
use std::io::{self, Read};

pub(crate) fn read_input() -> Result<String, String> {
    let mut args = env::args().skip(1);
    let mut input_path = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--stdin" => {}
            "--ir-file" => {
                input_path = Some(
                    args.next()
                        .ok_or_else(|| "--ir-file requires a path".to_string())?,
                );
            }
            "--json" => {}
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    if let Some(path) = input_path {
        fs::read_to_string(&path).map_err(|err| format!("failed to read {path}: {err}"))
    } else {
        let mut input = String::new();
        io::stdin()
            .read_to_string(&mut input)
            .map_err(|err| format!("failed to read stdin: {err}"))?;
        Ok(input)
    }
}
