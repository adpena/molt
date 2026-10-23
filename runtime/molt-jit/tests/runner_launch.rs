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

/// Append `line` to the log file, or write it to stdout when the log is
/// `-` (a start under a file size limit writes no file).
fn log(line: &str) {{
    let path = std::env::var_os("FAKE_RUNTIME_LOG").expect("FAKE_RUNTIME_LOG");
    if path == "-" {{
        println!("{{line}}");
        return;
    }}
    let mut file = std::fs::OpenOptions::new().append(true).create(true).open(path).unwrap();
    writeln!(file, "{{line}}").unwrap();
}}

unsafe extern "C" {{
    fn molt_isolate_bootstrap() -> u64;
    fn signal(signal: i32, handler: usize) -> usize;
    fn sigaltstack(new: *const u8, old: *mut AltStack) -> i32;
    fn sigpending(set: *mut u64) -> i32;
}}

#[cfg(target_os = "macos")]
#[repr(C)]
struct AltStack {{ sp: usize, size: usize, flags: i32 }}
#[cfg(not(target_os = "macos"))]
#[repr(C)]
struct AltStack {{ sp: usize, flags: i32, size: usize }}
const SS_DISABLE: i32 = if cfg!(target_os = "macos") {{ 4 }} else {{ 2 }};
const SIGBUS: i32 = if cfg!(target_os = "macos") {{ 10 }} else {{ 7 }};
const SIGXFSZ: i32 = 25;

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
    // The process state a C `main` gives a program: inherited dispositions
    // (the runner ignores SIGPIPE and SIGXFSZ only while it works), no
    // stack-overflow handler, no alternate signal stack, one thread.
    log(&format!(
        "process sigpipe={{}} sigxfsz={{}} sigsegv={{}} sigbus={{}} altstack={{}} threads={{}}",
        disposition(13),
        disposition(SIGXFSZ),
        disposition(11),
        disposition(SIGBUS),
        altstack(),
        threads()
    ));
    // A signal pending when the runner started stays pending (sigset_t is a
    // 32-bit mask on macOS and starts with a 64-bit word on Linux).
    let mut pending = [0_u64; 16];
    unsafe {{ sigpending(pending.as_mut_ptr()) }};
    log(&format!("pending sigxfsz={{}}", (pending[0] >> (SIGXFSZ - 1)) & 1 == 1));
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
pub extern "C" fn molt_thread_record() -> u64 {{
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

/// The program as the CLI's backend IR lease (MessagePack), with `filler`
/// extra functions that only return, to make compilation take a while.
fn program(extra_import: Option<&str>, filler: usize) -> Vec<u8> {
    let mut main = vec![];
    if let Some(symbol) = extra_import {
        main.push(OpIR {
            s_value: Some(symbol.to_string()),
            ..op("call")
        });
    }
    main.push(op("ret_void"));
    let mut functions = vec![
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
    functions.extend(
        (0..filler).map(|index| function(&format!("filler_{index}"), false, vec![op("ret_void")])),
    );
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

/// How the image asks for compiler diagnostics.
#[derive(Clone, Copy)]
enum Diagnostics {
    /// Backend timing, a diagnostic: compiler output streams, as the CLI sets
    /// it (`_env_requests_backend_diagnostics`).
    Timing,
    /// The default: compiler output appears only if compilation fails.
    OnFailure,
}

/// The JIT image layout `src/molt/cli/jit_image.py` writes, with its TIR
/// cache in `tir-cache` beside the image.
fn write_image(path: &Path, runtime_image: &Path, program: &[u8], diagnostics: Diagnostics) {
    let (backend_environment, compiler_diagnostics) = match diagnostics {
        Diagnostics::Timing => (serde_json::json!({"MOLT_BACKEND_TIMING": "1"}), "stream"),
        Diagnostics::OnFailure => (serde_json::json!({}), "on_failure"),
    };
    let header = serde_json::json!({
        "runtime_image": runtime_image,
        "backend_environment": backend_environment,
        "compiler_diagnostics": compiler_diagnostics,
        "tir_cache_root": tir_cache_root(path),
        "launch": {
            "environment": {"MOLT_EXECUTION_TARGET": "native"},
            "runtime_module_roots": [],
            "compiled_source_paths": ["/src/app.py"],
        },
    })
    .to_string();
    let mut bytes = format!(
        "#!{RUNNER} {}\n{} {} {}\n{header}",
        runner_protocol("image_flag"),
        runner_protocol("image_marker"),
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

/// The TIR cache root `write_image` names for the image at `image`.
fn tir_cache_root(image: &Path) -> PathBuf {
    image.with_file_name("tir-cache")
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
    receipt_contents: String,
}

fn launch(label: &str, extra_import: Option<&str>, start: Start) -> Launch {
    let artifacts = CargoTestArtifacts::new(label).expect("Cargo test artifact root");
    let runtime = fake_runtime(&artifacts);
    let image = artifacts.path().join("app_molt.jit");
    write_image(
        &image,
        &runtime,
        &program(extra_import, 0),
        Diagnostics::Timing,
    );
    let log = artifacts.path().join("calls.log");
    let receipt = artifacts.path().join("started.json");
    let mut command = match start {
        Start::Runner => {
            let mut command = Command::new(RUNNER);
            command
                .arg(runner_protocol("start_receipt_flag"))
                .arg(&receipt)
                .arg(runner_protocol("image_flag"))
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
                // A caller's own allocator setting stays the program's, even
                // beside a variable named like the marker the restart once
                // used (HF-371 review v4): the marker now travels in argv.
                .env("MallocLargeCache", "1")
                .env("MOLT_JIT_ALLOCATOR_RESTART", "1")
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
        receipt_contents: std::fs::read_to_string(&receipt).unwrap_or_default(),
    }
}

/// The launch ABI's one declaration: its runtime entries, the runner's command
/// line and the image format.
fn launch_abi() -> toml::Table {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../native_launch_abi.toml"
    ))
    .expect("read the launch ABI");
    source.parse().expect("parse the launch ABI")
}

/// One value of the launch ABI's `[runner]` table.
fn runner_protocol(key: &str) -> String {
    launch_abi()["runner"][key]
        .as_str()
        .expect("a runner protocol string")
        .to_string()
}

/// The `start` entries of the launch ABI's one source, in declared order.
fn declared_start_sequence() -> Vec<String> {
    let document = launch_abi();
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
    assert_eq!(run.receipt_contents, runner_protocol("start_receipt"));
    let called: Vec<&str> = run
        .log
        .iter()
        .map(|line| line.split(' ').next().unwrap_or_default())
        .filter(|name| !matches!(*name, "process" | "env" | "pending"))
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
        "process sigpipe=default sigxfsz=default sigsegv=default sigbus=default altstack=off threads=1"
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
        r#"env PWD=Some("/stale/dir") SHLVL=None OLDPWD=Some("/previous/dir") MallocLargeCache=Some("1") restart=Some("1")"#
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

/// The direct children of `pid`, as `pgrep -P` lists them.
fn children_of(pid: u32) -> Vec<u32> {
    let output = Command::new("pgrep")
        .arg("-P")
        .arg(pid.to_string())
        .output()
        .expect("run pgrep");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .collect()
}

#[test]
fn a_keeper_killed_alone_leaves_the_program_running() {
    // The compiler writes a progress line with eprintln! every 50 functions,
    // and eprintln! panics on a write error. Another process kills only the
    // compiler output keeper while the runner compiles 20,000 functions: the
    // remaining progress lines must still be written, and the program must
    // start and exit as it would have.
    let artifacts = CargoTestArtifacts::new("jit-keeper-killed").expect("Cargo test artifact root");
    let runtime = fake_runtime(&artifacts);
    let image = artifacts.path().join("app_molt.jit");
    write_image(
        &image,
        &runtime,
        &program(None, 20_000),
        Diagnostics::OnFailure,
    );
    let log = artifacts.path().join("calls.log");
    let receipt = artifacts.path().join("started.json");
    let runner = Command::new(RUNNER)
        .arg(runner_protocol("start_receipt_flag"))
        .arg(&receipt)
        .arg(runner_protocol("image_flag"))
        .arg(&image)
        .env("FAKE_RUNTIME_LOG", &log)
        .env("FAKE_RUNTIME_RECEIPT", &receipt)
        .env_remove("MOLT_EXECUTION_TARGET")
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("start molt-jit");
    let started = std::time::Instant::now();
    let keeper = loop {
        if let Some(&keeper) = children_of(runner.id()).first() {
            break keeper;
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(30),
            "the runner started no keeper"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    };
    // The runner writes its start receipt right after it compiles: the kill
    // below must land before that, while the keeper holds the capture.
    assert!(!receipt.exists(), "the runner compiled before the kill");
    let killed = Command::new("kill")
        .args(["-KILL", &keeper.to_string()])
        .status()
        .expect("run kill");
    assert!(killed.success());
    let output = runner.wait_with_output().expect("wait for molt-jit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    // The kill landed while the runner compiled: it never wrote its receipt
    // before the keeper died, and it wrote it afterwards.
    assert!(receipt.exists(), "{stderr}");
    let calls = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(calls.contains("molt_runtime_exit 0"), "{calls}\n{stderr}");
    assert_eq!(stderr, "", "a discarded compile writes nothing");
}

/// A JIT image of `program` (diagnostics on failure) beside a fresh runtime
/// image, with an empty current directory for the program: `(image, cwd)`.
fn image_with_empty_directory(
    artifacts: &CargoTestArtifacts,
    program: &[u8],
) -> (PathBuf, PathBuf) {
    let runtime = fake_runtime(artifacts);
    let image = artifacts.path().join("app_molt.jit");
    write_image(&image, &runtime, program, Diagnostics::OnFailure);
    let directory = artifacts.path().join("cwd");
    std::fs::create_dir(&directory).expect("create the program's directory");
    (image, directory)
}

/// What `directory` holds, by name.
fn entries(directory: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(directory)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// Start the image as `subprocess` would: no receipt, the program's log on
/// stdout (`-`), MOLT_CACHE unset and TMPDIR unusable.
fn start_image(image: &Path, directory: &Path) -> Command {
    let mut command = Command::new(image);
    command
        .current_dir(directory)
        .env("FAKE_RUNTIME_LOG", "-")
        .env("FAKE_RUNTIME_RECEIPT", directory.join("no-receipt"))
        .env("TMPDIR", "/nonexistent/molt-jit-tmp")
        .env_remove("MOLT_CACHE")
        .env_remove("MOLT_EXECUTION_TARGET");
    command
}

#[test]
fn a_start_uses_only_the_images_cache_and_no_temporary_directory() {
    // An AOT executable starts without a usable TMPDIR and writes no file. The
    // runner keeps its TIR cache under the root the image names; the
    // program's environment (no MOLT_CACHE) and current directory are not the
    // runner's to use (HF-371 review v5).
    let artifacts =
        CargoTestArtifacts::new("jit-start-file-state").expect("Cargo test artifact root");
    let (image, directory) = image_with_empty_directory(&artifacts, &program(None, 64));
    let output = start_image(&image, &directory)
        .output()
        .expect("run the image");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stdout}\n{stderr}");
    assert!(stdout.contains("molt_runtime_exit 0"), "{stdout}");
    assert_eq!(stderr, "", "a discarded compile writes nothing");
    assert_eq!(entries(&directory), Vec::<String>::new());
    assert!(
        !entries(&tir_cache_root(&image)).is_empty(),
        "the runner's TIR cache namespace lives under the image's root"
    );
}

#[test]
fn a_file_size_limit_of_zero_does_not_stop_the_start() {
    // With `ulimit -f 0` and SIGXFSZ at its default, any file write past the
    // limit ends the process. The runner writes no capture file, and its cold
    // TIR cache writes must fail quietly: it ignores SIGXFSZ while it works
    // and gives the program back the default.
    let artifacts = CargoTestArtifacts::new("jit-start-fsize").expect("Cargo test artifact root");
    let (image, directory) = image_with_empty_directory(&artifacts, &program(None, 64));
    let mut command = start_image(&image, &directory);
    // SAFETY: the closure runs in the forked child before exec and only
    // calls async-signal-safe functions.
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(|| {
            libc::signal(libc::SIGXFSZ, libc::SIG_DFL);
            let mut limit = std::mem::zeroed::<libc::rlimit>();
            libc::getrlimit(libc::RLIMIT_FSIZE, &mut limit);
            limit.rlim_cur = 0;
            if libc::setrlimit(libc::RLIMIT_FSIZE, &limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let output = command.output().expect("run the image");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stdout}\n{stderr}");
    assert!(
        stdout.contains("process sigpipe=default sigxfsz=default "),
        "{stdout}"
    );
    assert!(stdout.contains("molt_runtime_exit 0"), "{stdout}");
    assert_eq!(stderr, "");
    assert_eq!(entries(&directory), Vec::<String>::new());
}

#[test]
fn a_runner_that_cannot_capture_compiler_output_streams_it_and_starts() {
    // With one free descriptor the capture's socket pair cannot exist. The
    // runner must say so and stream compiler output, as with diagnostics,
    // not refuse the program (HF-371 review v5).
    let artifacts =
        CargoTestArtifacts::new("jit-start-no-capture").expect("Cargo test artifact root");
    let (image, directory) = image_with_empty_directory(&artifacts, &program(None, 64));
    let mut command = start_image(&image, &directory);
    // SAFETY: the closure runs in the forked child before exec and only
    // calls async-signal-safe functions.
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(|| {
            // fds 0-2 and one more: the image read, the runtime image load
            // and the program's own files each need one at a time.
            let mut limit = std::mem::zeroed::<libc::rlimit>();
            libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit);
            limit.rlim_cur = 4;
            if libc::setrlimit(libc::RLIMIT_NOFILE, &limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let output = command.output().expect("run the image");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stdout}\n{stderr}");
    assert!(stdout.contains("molt_runtime_exit 0"), "{stdout}");
    assert!(
        stderr.starts_with("molt-jit: compiler output goes to stderr: "),
        "{stderr}"
    );
    assert!(stderr.contains("MOLT_BACKEND"), "{stderr}");
}

#[test]
fn a_runner_whose_stderr_is_a_broken_pipe_still_fails_with_its_status() {
    // The runner's own messages ignore write errors: with stderr a pipe
    // nobody reads, a program that cannot compile still ends with the
    // runner's failure status, not a panic's abort or SIGPIPE.
    let artifacts = CargoTestArtifacts::new("jit-broken-stderr").expect("Cargo test artifact root");
    let (image, directory) =
        image_with_empty_directory(&artifacts, &program(Some("molt_missing_helper"), 0));
    let mut child = start_image(&image, &directory)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("start the image");
    drop(child.stderr.take());
    let status = child.wait().expect("wait for the image");
    assert_eq!(status.code(), Some(125), "{status:?}");
}

#[test]
fn a_signal_pending_at_start_is_still_pending_for_the_program() {
    // The runner ignores SIGXFSZ while it works; ignoring a pending signal
    // would discard it. A caller that blocked SIGXFSZ and left one pending
    // must hand it to the program, as an AOT executable receives it.
    let artifacts = CargoTestArtifacts::new("jit-start-pending").expect("Cargo test artifact root");
    let (image, directory) = image_with_empty_directory(&artifacts, &program(None, 0));
    let mut command = start_image(&image, &directory);
    // SAFETY: the closure runs in the forked child before exec and only
    // calls async-signal-safe functions.
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(|| {
            let mut blocked = std::mem::zeroed::<libc::sigset_t>();
            libc::sigemptyset(&mut blocked);
            libc::sigaddset(&mut blocked, libc::SIGXFSZ);
            libc::sigprocmask(libc::SIG_BLOCK, &blocked, std::ptr::null_mut());
            libc::raise(libc::SIGXFSZ);
            Ok(())
        });
    }
    let output = command.output().expect("run the image");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stdout}\n{stderr}");
    assert!(stdout.contains("pending sigxfsz=true"), "{stdout}");
}
