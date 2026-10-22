//! The runner's launch, end to end, against a recording runtime image.
//!
//! A fake runtime (a `cdylib` compiled here) defines every runtime entry the
//! launch calls and every runtime import a tiny program needs; each entry
//! appends its name to a log. The runner compiles the program into memory
//! and starts it, so the log is the runner's real call sequence. The test
//! checks it against the AOT main stub's order (`native_launch_abi.toml`),
//! and checks that the start receipt exists before the runtime starts and
//! that a program that cannot compile leaves none. Executing the image itself
//! starts the runner as its interpreter, with no shell between: the program
//! sees the environment its caller passed.

mod cargo_test_artifacts {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test_support/cargo_test_artifacts.rs"
    ));
}

use cargo_test_artifacts::CargoTestArtifacts;
use molt_backend_native::{FunctionIR, ModuleRegistryIR, OpIR};
use std::path::{Path, PathBuf};
use std::process::Command;

const RUNNER: &str = env!("CARGO_BIN_EXE_molt-jit");
/// What the program's `molt_isolate_bootstrap` returns; the fake runtime
/// calls the runner's export and logs the value.
const BOOTSTRAP_VALUE: i64 = 0x6a17;

fn fake_runtime_source() -> String {
    let witness = molt_codegen_abi::GENERATED_OBJECT_ABI_SYMBOL;
    format!(
        r#"
use std::io::Write;

fn log(line: &str) {{
    let path = std::env::var_os("FAKE_RUNTIME_LOG").expect("FAKE_RUNTIME_LOG");
    let mut file = std::fs::OpenOptions::new().append(true).create(true).open(path).unwrap();
    writeln!(file, "{{line}}").unwrap();
}}

unsafe extern "C" {{
    fn molt_isolate_bootstrap() -> u64;
    fn signal(signal: i32, handler: usize) -> usize;
    fn sigaltstack(new: *const u8, old: *mut AltStack) -> i32;
}}

#[cfg(target_os = "macos")]
#[repr(C)]
struct AltStack {{ sp: usize, size: usize, flags: i32 }}
#[cfg(not(target_os = "macos"))]
#[repr(C)]
struct AltStack {{ sp: usize, flags: i32, size: usize }}
const SS_DISABLE: i32 = if cfg!(target_os = "macos") {{ 4 }} else {{ 2 }};
const SIGBUS: i32 = if cfg!(target_os = "macos") {{ 10 }} else {{ 7 }};

/// The disposition of `number`, read by swapping it out and back.
fn disposition(number: i32) -> &'static str {{
    let previous = unsafe {{ signal(number, 0) }};
    unsafe {{ signal(number, previous) }};
    match previous {{
        0 => "default",
        1 => "ignored",
        _ => "handled",
    }}
}}

fn altstack() -> &'static str {{
    let mut current = AltStack {{ sp: 0, size: 0, flags: 0 }};
    unsafe {{ sigaltstack(std::ptr::null(), &mut current) }};
    if current.flags & SS_DISABLE != 0 {{ "off" }} else {{ "on" }}
}}

#[cfg(target_os = "macos")]
fn threads() -> usize {{
    unsafe extern "C" {{
        static mach_task_self_: u32;
        fn task_threads(task: u32, list: *mut *mut u32, count: *mut u32) -> i32;
    }}
    let mut list = std::ptr::null_mut();
    let mut count = 0;
    assert_eq!(unsafe {{ task_threads(mach_task_self_, &mut list, &mut count) }}, 0);
    count as usize
}}
#[cfg(not(target_os = "macos"))]
fn threads() -> usize {{
    std::fs::read_dir("/proc/self/task").unwrap().count()
}}

#[unsafe(no_mangle)]
pub static {witness}: u8 = 0;
static EXCEPTION_PENDING: u8 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn molt_register_compiled_sources(_paths: *const *const u8, count: u64) {{
    let target = std::env::var("MOLT_EXECUTION_TARGET").unwrap_or_default();
    log(&format!("molt_register_compiled_sources count={{count}} MOLT_EXECUTION_TARGET={{target}}"));
}}
#[unsafe(no_mangle)]
pub extern "C" fn molt_set_app_callable_resolver(resolver: u64) -> u64 {{
    log(&format!("molt_set_app_callable_resolver set={{}}", resolver != 0));
    0
}}
#[unsafe(no_mangle)]
pub extern "C" fn molt_module_registry_install(blob: *const u8) -> u64 {{
    log(&format!("molt_module_registry_install set={{}}", !blob.is_null()));
    0
}}
#[unsafe(no_mangle)]
pub extern "C" fn molt_runtime_init() -> u64 {{
    let receipt = std::env::var_os("FAKE_RUNTIME_RECEIPT").expect("FAKE_RUNTIME_RECEIPT");
    let started = std::path::Path::new(&receipt).exists();
    let bootstrap = unsafe {{ molt_isolate_bootstrap() }};
    log(&format!("molt_runtime_init receipt={{started}} bootstrap={{bootstrap:#x}}"));
    // A shell between the caller and the runner would set PWD and SHLVL; the
    // runner's allocator restart must leave no variable of its own behind.
    log(&format!(
        "env PWD={{:?}} SHLVL={{:?}} OLDPWD={{:?}} MallocLargeCache={{:?}} restart={{:?}}",
        std::env::var_os("PWD"),
        std::env::var_os("SHLVL"),
        std::env::var_os("OLDPWD"),
        std::env::var_os("MallocLargeCache"),
        std::env::var_os("MOLT_JIT_ALLOCATOR_RESTART")
    ));
    // The process state a C `main` gives a program: inherited dispositions,
    // no stack-overflow handler, no alternate signal stack, one thread.
    log(&format!(
        "process sigpipe={{}} sigsegv={{}} sigbus={{}} altstack={{}} threads={{}}",
        disposition(13),
        disposition(11),
        disposition(SIGBUS),
        altstack(),
        threads()
    ));
    0
}}
#[unsafe(no_mangle)]
pub extern "C" fn molt_runtime_ensure_gil() {{
    log("molt_runtime_ensure_gil");
}}
#[unsafe(no_mangle)]
pub extern "C" fn molt_set_argv(argc: i32, argv: *const *const std::ffi::c_char) {{
    let arguments: Vec<String> = (0..argc as usize)
        .map(|index| unsafe {{ std::ffi::CStr::from_ptr(*argv.add(index)) }}.to_string_lossy().into_owned())
        .collect();
    log(&format!("molt_set_argv {{}}", arguments.join(" ")));
}}
#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_pending() -> u64 {{
    log("molt_exception_pending");
    0
}}
#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_last_pending() -> u64 {{
    log("molt_exception_last_pending");
    0
}}
#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_report_uncaught(_exception: u64) -> u64 {{
    log("molt_exception_report_uncaught");
    1
}}
#[unsafe(no_mangle)]
pub extern "C" fn molt_dec_ref_obj(_bits: u64) {{}}
#[unsafe(no_mangle)]
pub extern "C" fn molt_runtime_exit(code: u64) -> u64 {{
    log(&format!("molt_runtime_exit {{code}}"));
    std::process::exit(code as i32)
}}
#[unsafe(no_mangle)]
pub extern "C" fn molt_dec_ref(_bits: u64) {{}}
#[unsafe(no_mangle)]
pub extern "C" fn molt_inc_ref_obj(_bits: u64) {{}}
#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_pending_fast() -> u64 {{
    0
}}
#[unsafe(no_mangle)]
pub extern "C" fn molt_exception_pending_flag_ptr() -> u64 {{
    std::ptr::addr_of!(EXCEPTION_PENDING) as u64
}}
#[unsafe(no_mangle)]
pub extern "C" fn molt_async_work_poll_and_exception_pending() -> u64 {{
    0
}}
"#
    )
}

/// Compile the fake runtime into a shared image beside the other artifacts.
fn fake_runtime(artifacts: &CargoTestArtifacts) -> PathBuf {
    let source = artifacts.path().join("fake_runtime.rs");
    std::fs::write(&source, fake_runtime_source()).expect("write fake runtime source");
    let image = artifacts.path().join(if cfg!(target_os = "macos") {
        "libfake_runtime.dylib"
    } else {
        "libfake_runtime.so"
    });
    #[expect(
        clippy::disallowed_methods,
        reason = "Cargo names the compiler the fixture is built with"
    )]
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let mut command = artifacts.command(rustc).expect("fixture compiler");
    command
        .args([
            "--edition=2024",
            "--crate-type=cdylib",
            "--crate-name=fake_runtime",
        ])
        .arg("-Cpanic=abort")
        .arg(artifacts.argument("", &source).unwrap())
        .arg("-o")
        .arg(artifacts.argument("", &image).unwrap());
    if cfg!(target_os = "macos") {
        // The runner, not the image, defines `molt_isolate_bootstrap`.
        command.arg("-Clink-arg=-Wl,-U,_molt_isolate_bootstrap");
    }
    let output = command.output().expect("run rustc");
    assert!(
        output.status.success(),
        "compile the fake runtime:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    image
}

fn op(kind: &str) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        ..OpIR::default()
    }
}

fn function(name: &str, returns_value: bool, ops: Vec<OpIR>) -> FunctionIR {
    FunctionIR {
        name: name.to_string(),
        return_abi: if returns_value {
            molt_ir::FunctionReturnAbi::Value
        } else {
            FunctionIR::default().return_abi
        },
        ops,
        ..FunctionIR::default()
    }
}

/// The program as the CLI's backend IR lease (MessagePack).
fn program(extra_import: Option<&str>) -> Vec<u8> {
    let mut main = vec![];
    if let Some(symbol) = extra_import {
        main.push(OpIR {
            s_value: Some(symbol.to_string()),
            ..op("call")
        });
    }
    main.push(op("ret_void"));
    let functions = vec![
        function("molt_main", false, main),
        function(
            "molt_isolate_bootstrap",
            true,
            vec![
                OpIR {
                    out: Some("value".to_string()),
                    value: Some(BOOTSTRAP_VALUE),
                    ..op("const")
                },
                OpIR {
                    args: Some(vec!["value".to_string()]),
                    ..op("ret")
                },
            ],
        ),
    ];
    #[derive(serde::Serialize)]
    struct Document {
        functions: Vec<FunctionIR>,
        module_registry: ModuleRegistryIR,
    }
    rmp_serde::to_vec_named(&Document {
        functions,
        module_registry: ModuleRegistryIR {
            blob: vec![0; 8],
            ..ModuleRegistryIR::default()
        },
    })
    .expect("encode the program")
}

/// The JIT image layout `src/molt/cli/jit_image.py` writes.
fn write_image(path: &Path, runtime_image: &Path, program: &[u8]) {
    let header = serde_json::json!({
        "runtime_image": runtime_image,
        // Backend timing is a diagnostic, so its output streams, as the CLI
        // sets it (`_env_requests_backend_diagnostics`).
        "backend_environment": {"MOLT_BACKEND_TIMING": "1"},
        "compiler_diagnostics": "stream",
        "launch": {
            "environment": {"MOLT_EXECUTION_TARGET": "native"},
            "runtime_module_roots": [],
            "compiled_source_paths": ["/src/app.py"],
        },
    })
    .to_string();
    let mut bytes = format!(
        "#!{RUNNER} --image\n#molt.jit-image.v2 {} {}\n{header}",
        header.len(),
        program.len()
    )
    .into_bytes();
    bytes.extend_from_slice(program);
    std::fs::write(path, bytes).expect("write the JIT image");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .expect("make the JIT image executable");
}

/// How the test starts the program.
enum Start {
    /// As `molt run` does: the runner, with a start receipt.
    Runner,
    /// As `subprocess` or a spawned worker does: `execve` of the image, from
    /// a caller whose PWD is stale and whose environment has no SHLVL.
    Image,
}

struct Launch {
    status: std::process::ExitStatus,
    stderr: String,
    log: Vec<String>,
    receipt: bool,
}

fn launch(label: &str, extra_import: Option<&str>, start: Start) -> Launch {
    let artifacts = CargoTestArtifacts::new(label).expect("Cargo test artifact root");
    let runtime = fake_runtime(&artifacts);
    let image = artifacts.path().join("app_molt.jit");
    write_image(&image, &runtime, &program(extra_import));
    let log = artifacts.path().join("calls.log");
    let receipt = artifacts.path().join("started.json");
    let mut command = match start {
        Start::Runner => {
            let mut command = Command::new(RUNNER);
            command
                .arg("--start-receipt")
                .arg(&receipt)
                .arg("--image")
                .arg(&image)
                .args(["first", "second"]);
            command
        }
        Start::Image => {
            let mut command = Command::new(&image);
            command
                .args(["first", "two words"])
                .env("PWD", "/stale/dir")
                .env("OLDPWD", "/previous/dir")
                // A caller's own allocator setting stays the program's.
                .env("MallocLargeCache", "1")
                .env_remove("SHLVL");
            command
        }
    };
    let output = command
        .env("FAKE_RUNTIME_LOG", &log)
        .env("FAKE_RUNTIME_RECEIPT", &receipt)
        .env_remove("MOLT_EXECUTION_TARGET")
        .output()
        .expect("run molt-jit");
    Launch {
        status: output.status,
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        log: std::fs::read_to_string(&log)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect(),
        receipt: receipt.exists(),
    }
}

/// The `start` entries of the launch ABI's one source, in declared order.
fn declared_start_sequence() -> Vec<String> {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../native_launch_abi.toml"
    ))
    .expect("read the launch ABI");
    let document: toml::Table = source.parse().expect("parse the launch ABI");
    document["start"]
        .as_array()
        .expect("start entries")
        .iter()
        .map(|entry| entry["name"].as_str().expect("entry name").to_string())
        .collect()
}

#[test]
fn the_runner_starts_a_program_in_the_aot_stub_order() {
    let run = launch("jit-launch-order", None, Start::Runner);
    assert_eq!(run.status.code(), Some(0), "{}", run.stderr);
    assert!(run.receipt);
    let called: Vec<&str> = run
        .log
        .iter()
        .map(|line| line.split(' ').next().unwrap_or_default())
        .filter(|name| *name != "process" && *name != "env")
        .collect();
    // The declared start sequence, then the program's own exit: a program
    // leaves through `molt_runtime_exit` from its entry.
    let mut expected = declared_start_sequence();
    expected.push("molt_runtime_exit".to_string());
    assert_eq!(called, expected, "{}", run.stderr);
    let line = |name: &str| {
        run.log
            .iter()
            .find(|line| line.starts_with(name))
            .map(String::as_str)
            .unwrap_or_default()
    };
    // The launch environment is installed before the first runtime call.
    assert_eq!(
        line("molt_register_compiled_sources"),
        "molt_register_compiled_sources count=1 MOLT_EXECUTION_TARGET=native"
    );
    assert_eq!(
        line("molt_set_app_callable_resolver"),
        "molt_set_app_callable_resolver set=true"
    );
    assert_eq!(
        line("molt_module_registry_install"),
        "molt_module_registry_install set=true"
    );
    // The receipt exists before the runtime starts, and the runtime reaches
    // the program's isolate initializer through the runner's export. The
    // initializer returns a boxed int whose payload is the constant.
    let init = line("molt_runtime_init");
    assert!(
        init.starts_with("molt_runtime_init receipt=true bootstrap="),
        "{init}"
    );
    let bootstrap = u64::from_str_radix(init.rsplit("0x").next().unwrap_or_default(), 16)
        .expect("bootstrap value");
    assert_eq!(bootstrap & 0xffff_ffff, BOOTSTRAP_VALUE as u64, "{init}");
    // The program starts in the state an AOT executable's C `main` gives it:
    // SIGPIPE keeps the disposition the runner inherited (Command resets it
    // to the default), Rust's SIGSEGV/SIGBUS stack-overflow handlers and
    // alternate signal stack are absent, and no compiler thread remains.
    assert_eq!(
        line("process"),
        "process sigpipe=default sigsegv=default sigbus=default altstack=off threads=1"
    );
    // The runner restarted itself with its allocator setting (macOS) and
    // removed it again: the program sees only what its caller passed.
    let env = line("env");
    assert!(env.ends_with("MallocLargeCache=None restart=None"), "{env}");
    #[cfg(target_os = "macos")]
    assert!(
        run.stderr.contains(", large-block cache off\n"),
        "{}",
        run.stderr
    );
    // argv[0] is the image, so sys.executable re-runs the program.
    let argv = line("molt_set_argv");
    assert!(argv.ends_with("app_molt.jit first second"), "{argv}");
    assert_eq!(line("molt_runtime_exit"), "molt_runtime_exit 0");
}

#[test]
fn a_program_that_cannot_compile_leaves_no_start_receipt() {
    // The fake runtime defines no `molt_missing_helper`: the runner refuses
    // the program before relocation, so nothing of the launch runs.
    let run = launch(
        "jit-launch-refused",
        Some("molt_missing_helper"),
        Start::Runner,
    );
    assert_eq!(run.status.code(), Some(125), "{}", run.stderr);
    assert!(run.stderr.contains("molt_missing_helper"), "{}", run.stderr);
    assert!(run.log.is_empty(), "{:?}", run.log);
    assert!(!run.receipt);
}

#[test]
fn executing_the_image_starts_its_runner_with_no_shell_between() {
    // subprocess.run([sys.executable]) and spawn-mode workers execute the
    // image. The runner is its interpreter, so the program keeps its caller's
    // environment (a shell would reset a stale PWD, drop OLDPWD and set SHLVL)
    // and its argv is the image path the kernel was given, then the arguments.
    let run = launch("jit-launch-direct", None, Start::Image);
    assert_eq!(run.status.code(), Some(0), "{}", run.stderr);
    assert!(!run.receipt);
    let line = |name: &str| {
        run.log
            .iter()
            .find(|line| line.starts_with(name))
            .map(String::as_str)
            .unwrap_or_default()
    };
    assert_eq!(
        line("env"),
        r#"env PWD=Some("/stale/dir") SHLVL=None OLDPWD=Some("/previous/dir") MallocLargeCache=Some("1") restart=None"#
    );
    let argv = line("molt_set_argv");
    assert!(
        argv.starts_with("molt_set_argv /") && argv.ends_with("/app_molt.jit first two words"),
        "{argv}"
    );
    assert_eq!(line("molt_runtime_exit"), "molt_runtime_exit 0");
    // The caller's own allocator setting stands: no restart.
    #[cfg(target_os = "macos")]
    assert!(
        run.stderr.contains(", large-block cache on\n"),
        "{}",
        run.stderr
    );
}
