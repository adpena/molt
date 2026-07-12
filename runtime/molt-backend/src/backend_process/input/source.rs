use std::io;

pub(super) fn open_ir_file(ir_path: &str) -> io::Result<std::fs::File> {
    std::fs::File::open(ir_path)
        .map_err(|err| io::Error::other(format!("failed to open IR file '{ir_path}': {err}")))
}

pub(super) fn invalid_ir_exit(context: &str, err: impl std::fmt::Display) -> ! {
    eprintln!("invalid {context}: {err}");
    std::process::exit(1);
}
