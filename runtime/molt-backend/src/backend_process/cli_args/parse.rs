use std::io;

#[cfg(feature = "native-backend")]
use super::model::NativeTargetOptions;
#[cfg(feature = "wasm-backend")]
use super::model::WasmTargetOptions;
use super::model::{BackendCommand, BackendCompileArgs, BackendTarget, FactGraphCliRequest};

/// Every option the backend accepts. Each may appear once, in any order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackendOption {
    Features,
    CaptureOrigin,
    EmissionScope,
    Daemon,
    Socket,
    NativeBatchJobFile,
    Target,
    Output,
    IrFile,
    IrFormat,
    FactGraphOutput,
    FactGraphFunction,
    TargetTriple,
    NativeOutputKind,
    WasmLink,
    WasmDataBase,
    WasmTableBase,
    WasmSplitRuntimeAppTableBase,
    WasmRuntimePollSlots,
}

impl BackendOption {
    const ALL: [Self; 19] = [
        Self::Features,
        Self::CaptureOrigin,
        Self::EmissionScope,
        Self::Daemon,
        Self::Socket,
        Self::NativeBatchJobFile,
        Self::Target,
        Self::Output,
        Self::IrFile,
        Self::IrFormat,
        Self::FactGraphOutput,
        Self::FactGraphFunction,
        Self::TargetTriple,
        Self::NativeOutputKind,
        Self::WasmLink,
        Self::WasmDataBase,
        Self::WasmTableBase,
        Self::WasmSplitRuntimeAppTableBase,
        Self::WasmRuntimePollSlots,
    ];

    fn flag(self) -> &'static str {
        match self {
            Self::Features => "--features",
            Self::CaptureOrigin => "--capture-origin",
            Self::EmissionScope => "--emission-scope",
            Self::Daemon => "--daemon",
            Self::Socket => "--socket",
            Self::NativeBatchJobFile => "--native-batch-job-file",
            Self::Target => "--target",
            Self::Output => "--output",
            Self::IrFile => "--ir-file",
            Self::IrFormat => "--ir-format",
            Self::FactGraphOutput => "--fact-graph-output",
            Self::FactGraphFunction => "--fact-graph-function",
            Self::TargetTriple => "--target-triple",
            Self::NativeOutputKind => "--native-output-kind",
            Self::WasmLink => "--wasm-link",
            Self::WasmDataBase => "--wasm-data-base",
            Self::WasmTableBase => "--wasm-table-base",
            Self::WasmSplitRuntimeAppTableBase => "--wasm-split-runtime-app-table-base",
            Self::WasmRuntimePollSlots => "--wasm-runtime-poll-slots",
        }
    }

    fn takes_value(self) -> bool {
        !matches!(self, Self::Features | Self::Daemon | Self::WasmLink)
    }

    /// The `--target` whose backend reads this option, if only one does.
    fn owning_target(self) -> Option<&'static str> {
        match self {
            Self::TargetTriple | Self::NativeOutputKind => Some("native"),
            Self::WasmLink
            | Self::WasmDataBase
            | Self::WasmTableBase
            | Self::WasmSplitRuntimeAppTableBase
            | Self::WasmRuntimePollSlots => Some("wasm"),
            _ => None,
        }
    }

    fn from_flag(flag: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|option| option.flag() == flag)
    }
}

/// The options one command line supplied; a flag without a value records "".
struct SuppliedOptions<'a> {
    values: [Option<&'a str>; BackendOption::ALL.len()],
}

impl<'a> SuppliedOptions<'a> {
    fn parse(args: &'a [String]) -> io::Result<Self> {
        let mut supplied = Self {
            values: [None; BackendOption::ALL.len()],
        };
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            let option = BackendOption::from_flag(arg)
                .ok_or_else(|| invalid(format!("unknown molt-backend argument {arg:?}")))?;
            let value = if option.takes_value() {
                args.next()
                    .ok_or_else(|| invalid(format!("{} requires a value", option.flag())))?
                    .as_str()
            } else {
                ""
            };
            let slot = &mut supplied.values[option as usize];
            if slot.is_some() {
                return Err(invalid(format!("{} may appear only once", option.flag())));
            }
            *slot = Some(value);
        }
        Ok(supplied)
    }

    fn get(&self, option: BackendOption) -> Option<&'a str> {
        self.values[option as usize]
    }

    fn has(&self, option: BackendOption) -> bool {
        self.get(option).is_some()
    }

    /// Refuse every supplied option outside `allowed`; `command` names the
    /// option that selected the command.
    fn only(&self, command: BackendOption, allowed: &[BackendOption]) -> io::Result<()> {
        match BackendOption::ALL
            .into_iter()
            .find(|option| self.has(*option) && *option != command && !allowed.contains(option))
        {
            Some(extra) => Err(invalid(format!(
                "{} cannot be combined with {}",
                command.flag(),
                extra.flag()
            ))),
            None => Ok(()),
        }
    }
}

impl<'a> BackendCommand<'a> {
    /// Parse the arguments after the program name.
    pub(crate) fn parse(args: &'a [String]) -> io::Result<Self> {
        let supplied = SuppliedOptions::parse(args)?;
        if supplied.has(BackendOption::Features) {
            supplied.only(BackendOption::Features, &[])?;
            return Ok(Self::Features);
        }
        if supplied.has(BackendOption::Daemon) {
            supplied.only(
                BackendOption::Daemon,
                &[BackendOption::Socket, BackendOption::EmissionScope],
            )?;
            return daemon_command(&supplied);
        }
        if let Some(job_file) = supplied.get(BackendOption::NativeBatchJobFile) {
            supplied.only(
                BackendOption::NativeBatchJobFile,
                &[BackendOption::Output, BackendOption::EmissionScope],
            )?;
            return native_batch_command(
                job_file,
                supplied.get(BackendOption::Output),
                supplied.get(BackendOption::EmissionScope),
            );
        }
        if supplied.has(BackendOption::Socket) {
            return Err(invalid("--socket requires --daemon"));
        }
        compile_args(&supplied).map(Self::Compile)
    }
}

#[cfg(molt_backend_daemon)]
fn daemon_command<'a>(supplied: &SuppliedOptions<'a>) -> io::Result<BackendCommand<'a>> {
    let socket_path = supplied
        .get(BackendOption::Socket)
        .ok_or_else(|| invalid("--daemon requires --socket"))?;
    Ok(BackendCommand::Daemon {
        socket_path,
        emission_scope: supplied.get(BackendOption::EmissionScope),
    })
}

#[cfg(not(molt_backend_daemon))]
fn daemon_command<'a>(_: &SuppliedOptions<'a>) -> io::Result<BackendCommand<'a>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "--daemon needs a Unix host and a molt-backend built with native-backend or wasm-backend",
    ))
}

#[cfg(feature = "native-backend")]
fn native_batch_command<'a>(
    job_file: &'a str,
    output_path: Option<&'a str>,
    emission_scope: Option<&'a str>,
) -> io::Result<BackendCommand<'a>> {
    Ok(BackendCommand::NativeBatchJob {
        job_file,
        emission_scope,
        output_path: output_path
            .ok_or_else(|| invalid("--native-batch-job-file requires --output"))?,
    })
}

#[cfg(not(feature = "native-backend"))]
fn native_batch_command<'a>(
    _: &'a str,
    _: Option<&'a str>,
    _: Option<&'a str>,
) -> io::Result<BackendCommand<'a>> {
    Err(missing_backend("native-backend"))
}

fn compile_args<'a>(supplied: &SuppliedOptions<'a>) -> io::Result<BackendCompileArgs<'a>> {
    let target_name = supplied.get(BackendOption::Target).unwrap_or("native");
    if let Some((option, owner)) = BackendOption::ALL.into_iter().find_map(|option| {
        option
            .owning_target()
            .filter(|owner| supplied.has(option) && *owner != target_name)
            .map(|owner| (option, owner))
    }) {
        return Err(invalid(format!(
            "{} requires --target {owner}",
            option.flag()
        )));
    }
    let fact_graph = match (
        supplied.get(BackendOption::FactGraphOutput),
        supplied.get(BackendOption::FactGraphFunction),
    ) {
        (Some(output_path), Some(function_name)) => Some(FactGraphCliRequest {
            output_path,
            function_name,
        }),
        (None, None) => None,
        _ => {
            return Err(invalid(
                "--fact-graph-output and --fact-graph-function must be supplied together",
            ));
        }
    };
    let target = match target_name {
        "native" => native_target(supplied)?,
        "wasm" => wasm_target(supplied)?,
        "luau" => luau_target()?,
        "rust" => rust_target()?,
        other => {
            return Err(invalid(format!(
                "unknown --target {other:?}; expected native, wasm, luau or rust"
            )));
        }
    };
    let capture_origin = supplied.get(BackendOption::CaptureOrigin);
    let emission_scope = supplied.get(BackendOption::EmissionScope);
    if emission_scope.is_some() && capture_origin.is_none() {
        return Err(invalid(
            "--emission-scope requires --capture-origin for one-shot compilation",
        ));
    }
    if capture_origin.is_some()
        && (fact_graph.is_some()
            || !matches!(target_name, "native" | "wasm")
            || supplied.get(BackendOption::IrFormat) != Some("msgpack"))
    {
        return Err(invalid(
            "origin capture requires a native/WASM MessagePack compilation without fact-graph emission",
        ));
    }
    Ok(BackendCompileArgs {
        capture_origin,
        emission_scope,
        target,
        output_path: supplied.get(BackendOption::Output),
        ir_file_path: supplied.get(BackendOption::IrFile),
        ir_format: crate::backend_process::input::IrFormat::parse(
            supplied.get(BackendOption::IrFormat).unwrap_or("json"),
        )?,
        fact_graph,
    })
}

#[cfg(feature = "native-backend")]
fn native_target(supplied: &SuppliedOptions<'_>) -> io::Result<BackendTarget> {
    let output_kind = supplied
        .get(BackendOption::NativeOutputKind)
        .map(crate::backend_process::NativeArtifactKind::parse)
        .transpose()
        .map_err(invalid)?
        .unwrap_or_default();
    Ok(BackendTarget::Native(NativeTargetOptions {
        target_triple: supplied.get(BackendOption::TargetTriple).map(str::to_owned),
        output_kind,
    }))
}

#[cfg(not(feature = "native-backend"))]
fn native_target(_: &SuppliedOptions<'_>) -> io::Result<BackendTarget> {
    Err(missing_backend("native-backend"))
}

#[cfg(feature = "wasm-backend")]
fn wasm_target(supplied: &SuppliedOptions<'_>) -> io::Result<BackendTarget> {
    let u32_value = |option: BackendOption| {
        supplied
            .get(option)
            .map(|raw| {
                raw.parse::<u32>().map_err(|error| {
                    invalid(format!("{} {raw:?} is not a u32: {error}", option.flag()))
                })
            })
            .transpose()
    };
    Ok(BackendTarget::Wasm(WasmTargetOptions {
        link_relocs: supplied.has(BackendOption::WasmLink),
        data_base: u32_value(BackendOption::WasmDataBase)?,
        table_base: u32_value(BackendOption::WasmTableBase)?,
        split_runtime_app_table_base: u32_value(BackendOption::WasmSplitRuntimeAppTableBase)?,
        runtime_poll_slots: supplied
            .get(BackendOption::WasmRuntimePollSlots)
            .map(|raw| {
                let flag = BackendOption::WasmRuntimePollSlots.flag();
                raw.parse::<u64>()
                    .map_err(|error| invalid(format!("{flag} {raw:?} is not a u64: {error}")))
                    .and_then(|mask| {
                        molt_backend::WasmRuntimePollSlots::from_mask(mask)
                            .map_err(|error| invalid(format!("{flag}: {error}")))
                    })
            })
            .transpose()?,
    }))
}

#[cfg(not(feature = "wasm-backend"))]
fn wasm_target(_: &SuppliedOptions<'_>) -> io::Result<BackendTarget> {
    Err(missing_backend("wasm-backend"))
}

#[cfg(feature = "luau-backend")]
fn luau_target() -> io::Result<BackendTarget> {
    Ok(BackendTarget::Luau)
}

#[cfg(not(feature = "luau-backend"))]
fn luau_target() -> io::Result<BackendTarget> {
    Err(missing_backend("luau-backend"))
}

#[cfg(feature = "rust-backend")]
fn rust_target() -> io::Result<BackendTarget> {
    Ok(BackendTarget::Rust)
}

#[cfg(not(feature = "rust-backend"))]
fn rust_target() -> io::Result<BackendTarget> {
    Err(missing_backend("rust-backend"))
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

#[cfg(not(all(
    feature = "native-backend",
    feature = "wasm-backend",
    feature = "luau-backend",
    feature = "rust-backend"
)))]
fn missing_backend(feature: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        format!(
            "molt-backend was built without {feature}; rebuild with: cargo build -p molt-backend --features {feature}"
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_parsed<R>(args: &[&str], check: impl FnOnce(io::Result<BackendCommand<'_>>) -> R) -> R {
        let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
        check(BackendCommand::parse(&args))
    }

    fn rejection(args: &[&str]) -> (io::ErrorKind, String) {
        with_parsed(args, |parsed| {
            let error = parsed.expect_err("the backend must refuse this command line");
            (error.kind(), error.to_string())
        })
    }

    #[cfg(any(feature = "wasm-backend", feature = "rust-backend"))]
    fn compile_target(args: &[&str]) -> BackendTarget {
        with_parsed(args, |parsed| match parsed.expect("compile request") {
            BackendCommand::Compile(request) => request.target,
            other => panic!("expected a compile request, got {other:?}"),
        })
    }

    #[test]
    fn option_table_indexes_each_option_once() {
        for (index, option) in BackendOption::ALL.into_iter().enumerate() {
            assert_eq!(option as usize, index, "{option:?}");
            assert_eq!(BackendOption::from_flag(option.flag()), Some(option));
        }
    }

    #[test]
    fn malformed_command_lines_fail_instead_of_being_ignored() {
        for (args, expected) in [
            (&["--frobnicate"][..], "unknown molt-backend argument"),
            (&["wasm"][..], "unknown molt-backend argument"),
            (
                &["--output", "a", "--output", "b"][..],
                "may appear only once",
            ),
            (&["--output"][..], "--output requires a value"),
            (&["--target", "jvm"][..], "unknown --target \"jvm\""),
            (&["--wasm-link"][..], "--wasm-link requires --target wasm"),
            (
                &["--target", "wasm", "--native-output-kind", "archive"][..],
                "--native-output-kind requires --target native",
            ),
            (
                &[
                    "--target",
                    "luau",
                    "--target-triple",
                    "x86_64-unknown-linux-gnu",
                ][..],
                "--target-triple requires --target native",
            ),
            (
                &["--fact-graph-output", "graph.json"][..],
                "must be supplied together",
            ),
            (
                &["--features", "--output", "out"][..],
                "--features cannot be combined with --output",
            ),
            (
                &["--socket", "backend.sock"][..],
                "--socket requires --daemon",
            ),
            (
                &["--daemon", "--socket", "backend.sock", "--target", "wasm"][..],
                "--daemon cannot be combined with --target",
            ),
        ] {
            let (kind, message) = rejection(args);
            assert_eq!(kind, io::ErrorKind::InvalidInput, "{args:?}: {message}");
            assert!(message.contains(expected), "{args:?}: {message}");
        }
    }

    #[test]
    fn features_is_a_command_of_its_own() {
        with_parsed(&["--features"], |parsed| {
            assert_eq!(parsed.expect("features"), BackendCommand::Features);
        });
    }

    #[cfg(feature = "native-backend")]
    #[test]
    fn native_artifact_kind_is_explicit_and_fail_closed() {
        use crate::backend_process::NativeArtifactKind;
        for (args, expected) in [
            (&[][..], Some(NativeArtifactKind::Object)),
            (
                &["--native-output-kind", "object"][..],
                Some(NativeArtifactKind::Object),
            ),
            (
                &["--native-output-kind", "archive"][..],
                Some(NativeArtifactKind::Archive),
            ),
            (&["--native-output-kind"][..], None),
            (&["--native-output-kind", "static-library"][..], None),
            (
                &[
                    "--native-output-kind",
                    "object",
                    "--native-output-kind",
                    "archive",
                ][..],
                None,
            ),
        ] {
            let kind = with_parsed(args, |parsed| match parsed {
                Ok(BackendCommand::Compile(BackendCompileArgs {
                    target: BackendTarget::Native(options),
                    ..
                })) => Some(options.output_kind),
                Ok(other) => panic!("{args:?} parsed as {other:?}"),
                Err(_) => None,
            });
            assert_eq!(kind, expected, "{args:?}");
        }
    }

    #[cfg(feature = "native-backend")]
    #[test]
    fn python_native_compile_argv_parses_exactly() {
        // src/molt/cli/factgraph.py::backend_command_prefix plus backend_compile.py.
        let args = [
            "--target-triple",
            "aarch64-apple-darwin",
            "--native-output-kind",
            "archive",
            "--output",
            "build/app.a",
            "--ir-file",
            "build/app.ir.json",
        ];
        with_parsed(&args, |parsed| {
            assert_eq!(
                parsed.expect("native compile"),
                BackendCommand::Compile(BackendCompileArgs {
                    target: BackendTarget::Native(NativeTargetOptions {
                        target_triple: Some("aarch64-apple-darwin".to_string()),
                        output_kind: crate::backend_process::NativeArtifactKind::Archive,
                    }),
                    output_path: Some("build/app.a"),
                    ir_file_path: Some("build/app.ir.json"),
                    ir_format: crate::backend_process::input::IrFormat::Json,
                    fact_graph: None,
                    capture_origin: None,
                    emission_scope: None,
                })
            );
        });
    }

    #[cfg(feature = "native-backend")]
    #[test]
    fn native_batch_worker_argv_parses_and_requires_its_output() {
        // native_batch/worker/spawn.rs spawns exactly this shape.
        with_parsed(
            &["--native-batch-job-file", "job.json", "--output", "job.o"],
            |parsed| {
                assert_eq!(
                    parsed.expect("batch job"),
                    BackendCommand::NativeBatchJob {
                        job_file: "job.json",
                        emission_scope: None,
                        output_path: "job.o",
                    }
                );
            },
        );
        let (_, message) = rejection(&["--native-batch-job-file", "job.json"]);
        assert!(message.contains("requires --output"), "{message}");
        let (_, message) = rejection(&[
            "--native-batch-job-file",
            "job.json",
            "--output",
            "o",
            "--ir-file",
            "i",
        ]);
        assert!(
            message.contains("cannot be combined with --ir-file"),
            "{message}"
        );
    }

    #[cfg(feature = "wasm-backend")]
    #[test]
    fn python_wasm_compile_argv_parses_every_layout_override() {
        let target = compile_target(&[
            "--target",
            "wasm",
            "--wasm-link",
            "--wasm-data-base",
            "1048576",
            "--wasm-table-base",
            "4096",
            "--wasm-split-runtime-app-table-base",
            "8192",
            "--wasm-runtime-poll-slots",
            "6",
            "--output",
            "app.wasm",
        ]);
        assert_eq!(
            target,
            BackendTarget::Wasm(WasmTargetOptions {
                link_relocs: true,
                data_base: Some(1_048_576),
                table_base: Some(4096),
                split_runtime_app_table_base: Some(8192),
                runtime_poll_slots: Some(molt_backend::WasmRuntimePollSlots::from_mask(6).unwrap()),
            })
        );
        let (kind, message) = rejection(&["--target", "wasm", "--wasm-runtime-poll-slots", "1"]);
        assert_eq!(kind, io::ErrorKind::InvalidInput);
        assert!(
            message.contains("--wasm-runtime-poll-slots: runtime poll-slot mask 0x1"),
            "{message}"
        );
        let (kind, message) = rejection(&["--target", "wasm", "--wasm-data-base", "12x"]);
        assert_eq!(kind, io::ErrorKind::InvalidInput);
        assert!(
            message.contains("--wasm-data-base \"12x\" is not a u32"),
            "{message}"
        );
    }

    #[cfg(feature = "luau-backend")]
    #[test]
    fn target_comes_from_the_target_option_not_from_any_argument_spelling_it() {
        // The former parser set the WASM target for any argument equal to
        // "wasm" once --target appeared anywhere.
        let args = ["--output", "wasm", "--target", "luau"];
        with_parsed(&args, |parsed| match parsed.expect("luau compile") {
            BackendCommand::Compile(request) => {
                assert_eq!(request.target, BackendTarget::Luau);
                assert_eq!(request.output_path, Some("wasm"));
            }
            other => panic!("{other:?}"),
        });
    }

    #[cfg(feature = "rust-backend")]
    #[test]
    fn rust_target_has_no_fact_graph() {
        let error = compile_target(&["--target", "rust"])
            .fact_graph_target_info()
            .expect_err("rust fact graphs are unsupported");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("rust target"), "{error}");
    }

    #[cfg(molt_backend_daemon)]
    #[test]
    fn daemon_command_requires_its_socket() {
        with_parsed(&["--daemon", "--socket", "backend.sock"], |parsed| {
            assert_eq!(
                parsed.expect("daemon"),
                BackendCommand::Daemon {
                    socket_path: "backend.sock",
                    emission_scope: None
                }
            );
        });
        let (_, message) = rejection(&["--daemon"]);
        assert!(message.contains("--daemon requires --socket"), "{message}");
    }

    #[cfg(not(feature = "luau-backend"))]
    #[test]
    fn absent_backend_fails_with_its_rebuild_command() {
        let (kind, message) = rejection(&["--target", "luau"]);
        assert_eq!(kind, io::ErrorKind::Unsupported);
        assert!(
            message.contains("cargo build -p molt-backend --features luau-backend"),
            "{message}"
        );
    }
}
