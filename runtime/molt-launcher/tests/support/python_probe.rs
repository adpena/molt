//! Test-only Python transport stand-in. Release builds select `--bin molt`.

use std::env;
use std::ffi::OsString;
use std::path::PathBuf;

const BOOTSTRAP: &str = include_str!("../../../../packaging/bootstrap.py");

fn main() {
    let args: Vec<OsString> = env::args_os().collect();
    let expected_guest = [
        OsString::from("literal [x] & y.py"),
        OsString::from("--output"),
        OsString::from("binary with spaces"),
        OsString::from("λ source"),
    ];
    assert_eq!(
        args.len(),
        6 + expected_guest.len(),
        "launcher argv: {args:?}"
    );
    assert_eq!(args[1].to_str(), Some("-I"));
    assert_eq!(args[2].to_str(), Some("-B"));
    assert_eq!(args[3].to_str(), Some("-c"));
    assert_eq!(args[4].to_str(), Some(BOOTSTRAP));
    assert_eq!(
        PathBuf::from(args[5].as_os_str()),
        PathBuf::from(env::var_os("MOLT_LAUNCHER_EXPECTED_ROOT").expect("expected root"))
    );
    assert_eq!(&args[6..], &expected_guest);
    assert_eq!(
        env::var_os("MOLT_PROJECT_ROOT"),
        env::var_os("MOLT_LAUNCHER_EXPECTED_PROJECT")
    );
    #[cfg(windows)]
    {
        assert_eq!(
            env::var("PYTHON_MANAGER_AUTOMATIC_INSTALL").as_deref(),
            Ok("false")
        );
        assert!(env::var_os("PYLAUNCHER_ALLOW_INSTALL").is_none());
        assert!(env::var_os("PYLAUNCHER_ALWAYS_INSTALL").is_none());
    }
    println!("transport-ok");
    std::process::exit(23);
}
