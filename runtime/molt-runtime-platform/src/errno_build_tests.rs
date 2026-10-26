//! Native errno generator controls. Independent C literals and CPython source
//! contracts are the oracles; no test substitutes a second errno evaluator.
#[expect(dead_code, reason = "Data-only evaluator does not optimize C routines")]
#[path = "../../build_support/c_codegen.rs"]
mod c_codegen;
#[path = "../../test_support/cargo_test_artifacts.rs"]
mod cargo_test_artifacts;
#[path = "../../build_support/target_errno.rs"]
mod target_errno;

use cargo_test_artifacts::CargoTestArtifacts;
use std::fs;
use std::path::Path;
use target_errno::{Declaration, NativeTarget};

fn target() -> NativeTarget {
    NativeTarget::from_cargo(
        include_str!(concat!(env!("OUT_DIR"), "/errno_arch.txt")),
        include_str!(concat!(env!("OUT_DIR"), "/errno_os.txt")),
        include_str!(concat!(env!("OUT_DIR"), "/errno_environment.txt")),
        include_str!(concat!(env!("OUT_DIR"), "/errno_abi.txt")),
        include_str!(concat!(env!("OUT_DIR"), "/errno_pointer_width.txt")),
        include_str!(concat!(env!("OUT_DIR"), "/errno_endian.txt")),
    )
    .expect("Cargo test coordinate")
}

fn compiler(out: &Path) -> cc::Build {
    let triple = include_str!(concat!(env!("OUT_DIR"), "/errno_target.txt"));
    let mut build = cc::Build::new();
    build
        .target(triple)
        .host(triple)
        .out_dir(out)
        .opt_level(0)
        .debug(false)
        .cargo_metadata(false)
        .std("c11");
    build
}

fn object(source: &str, owner: &CargoTestArtifacts) -> Vec<u8> {
    let path = owner.path().join("fixture.c");
    fs::write(&path, source).unwrap();
    let paths = compiler(owner.path())
        .file(path)
        .try_compile_intermediates()
        .unwrap();
    assert_eq!(paths.len(), 1);
    fs::read(&paths[0]).unwrap()
}

#[test]
fn c_source_and_generated_debug_metadata_are_relocatable() {
    let owner = CargoTestArtifacts::new("c-source-relocation").unwrap();
    let mut generations = Vec::new();
    for name in ["first tree", "second tree"] {
        let work = owner.path().join(name);
        let manifest = work.join("source/runtime/probe");
        let out = work.join("generated");
        fs::create_dir_all(&manifest).unwrap();
        fs::create_dir_all(&out).unwrap();
        let source = manifest.join("source.c");
        let generated = out.join("generated.c");
        fs::write(&source, "const char source_file[] = __FILE__;\n").unwrap();
        fs::write(&generated, "const char generated_file[] = __FILE__;\n").unwrap();
        let mut build = compiler(&out);
        build.debug(true).file(&source).file(&generated);
        c_codegen::apply_source_path_policy(&mut build, &manifest, &out);
        let bodies: Vec<_> = build
            .try_compile_intermediates()
            .unwrap()
            .iter()
            .map(|path| fs::read(path).unwrap())
            .collect();
        assert_eq!(bodies.len(), 2);
        let spelling = work.to_string_lossy();
        for body in &bodies {
            assert!(
                !body
                    .windows(spelling.len())
                    .any(|window| window == spelling.as_bytes())
            );
        }
        assert!(
            bodies[0]
                .windows(b"/molt/source".len())
                .any(|window| window == b"/molt/source")
        );
        assert!(
            bodies[1]
                .windows(b"/molt/generated".len())
                .any(|window| window == b"/molt/generated")
        );
        generations.push(bodies);
    }
    assert_eq!(generations[0], generations[1]);
}

#[test]
fn actual_c_expression_presence_aliases_and_generated_versioned_consumer() {
    let owner = CargoTestArtifacts::new("errno-target-header").unwrap();
    // Deliberately neither Linux11 nor Darwin35: host errno substitution fails.
    fs::write(owner.path().join("errno.h"),
        "#define EARETRY (111 + 12)\n#define EZALIAS EARETRY\n#define EHWPOISON (7 * 9)\n#define EFALLBACK (10000 + 64)\n").unwrap();
    let declaration = Declaration::parse(concat!(
        "EZALIAS\t-\tnative\t312\n",
        "EARETRY\t-\tnative\t312\n",
        "EZALIAS\t-\tnative\t312\n",
        "EHOSTDOWN\tEFALLBACK\tnative\t312\n",
        "EMISSING\t-\tnative\t312\n",
        "EHWPOISON\t-\tnative\t314\n",
    ))
    .unwrap();
    let values = target_errno::evaluate(
        &declaration,
        &target(),
        compiler(owner.path()).include(owner.path()),
        owner.path(),
    )
    .unwrap();
    assert_eq!(
        values.public,
        [Some(123), Some(123), Some(123), Some(10064), None, Some(63)]
    );
    let generated = declaration.rust_source(&values).unwrap();
    let source = owner.path().join("consumer.rs");
    fs::write(
        &source,
        format!(
            "{generated}\n{}",
            r#"
fn main() {
    let expected = vec![("EZALIAS",123), ("EARETRY",123), ("EZALIAS",123), ("EHOSTDOWN",10064)];
    assert_eq!(errno_constants(312).collect::<Vec<_>>(), expected);
    assert_eq!(errno_constants(313).collect::<Vec<_>>(), expected);
    let mut newer = expected.clone(); newer.push(("EHWPOISON",63));
    assert_eq!(errno_constants(314).collect::<Vec<_>>(), newer);
    assert_eq!(errno_constants(315).collect::<Vec<_>>(), newer);
    let reverse: std::collections::HashMap<_,_> = errno_constants(312)
        .map(|(name, value)| (value, name)).collect();
    assert_eq!(reverse.get(&123), Some(&"EZALIAS"));
}
"#
        ),
    )
    .unwrap();
    let binary = owner.path().join(if cfg!(windows) {
        "consumer.exe"
    } else {
        "consumer"
    });
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output = owner
        .command(rustc)
        .unwrap()
        .arg("--edition=2024")
        .arg(owner.argument("", &source).unwrap())
        .arg("-o")
        .arg(owner.argument("", &binary).unwrap())
        .output()
        .unwrap();
    fs::write(owner.path().join("rustc.stderr"), &output.stderr).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = owner.command(&binary).unwrap().output().unwrap();
    fs::write(owner.path().join("consumer.stderr"), &output.stderr).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn native_public_constants_preserve_cpython_order_and_windows_domain() {
    let rows = crate::socket_constants::errno_constants(312);
    let forward: std::collections::HashMap<_, _> = rows.clone().collect();
    let reverse: std::collections::HashMap<_, _> =
        rows.map(|(name, number)| (number, name)).collect();
    assert_eq!(forward["EINVAL"], i64::from(libc::EINVAL));
    assert_eq!(forward["EAGAIN"], i64::from(libc::EAGAIN));
    #[cfg(not(windows))]
    {
        assert_eq!(forward["EWOULDBLOCK"], i64::from(libc::EWOULDBLOCK));
        if libc::EWOULDBLOCK == libc::EAGAIN {
            assert_eq!(reverse[&i64::from(libc::EAGAIN)], "EAGAIN");
        }
    }
    #[cfg(windows)]
    {
        // CPython errnomodule.c undefines these CRT names, prefers Winsock,
        // and subsequently publishes the WSA name as reverse-map winner.
        assert_eq!(forward["EWOULDBLOCK"], 10035);
        assert_eq!(forward["ETIMEDOUT"], 10060);
        assert_eq!(forward["WSAEWOULDBLOCK"], 10035);
        assert_eq!(forward["WSAETIMEDOUT"], 10060);
        assert_eq!(reverse[&10035], "WSAEWOULDBLOCK");
        assert_eq!(reverse[&10060], "WSAETIMEDOUT");
        assert_ne!(forward["EWOULDBLOCK"], i64::from(libc::EWOULDBLOCK));
    }
    assert!(!forward.contains_key("EHWPOISON"));
}

#[test]
fn malformed_declarations_and_record_words_fail_closed() {
    // Genuine CPython Winsock names need not start with E or WSAE.
    let wsa = Declaration::parse("WSABASEERR\t-\tnative\t312\n").unwrap();
    assert_eq!(wsa.raw, ["WSABASEERR"]);
    for name in [
        "errno",
        "WSAerror",
        "WSAé",
        "WSA-X",
        "WSA) + 1",
        "XWSAEINTR",
        "1EAGAIN",
    ] {
        assert!(Declaration::parse(&format!("{name}\t-\tnative\t312\n")).is_err());
        assert!(Declaration::parse(&format!("EIO\t{name}\tnative\t312\n")).is_err());
    }
    for text in [
        "",
        "EIO\t-\twsa\t312\n",
        "EIO\t-\tnative\t0312\n",
        "EIO\t-\tnative\t312\nEIO\t-\tnative\t315\n",
    ] {
        assert!(Declaration::parse(text).is_err());
    }
    let declaration = Declaration::parse("EIO\t-\tnative\t312\n").unwrap();
    // Independent wire witness: magic/version/counts; raw row then public row.
    let words = [0x4d45_5252u32, 1, 1, 1, 0, 1, 77, 0, 1, 77];
    for little in [true, false] {
        let encode = |words: &[u32]| {
            words
                .iter()
                .flat_map(|word| {
                    if little {
                        word.to_le_bytes()
                    } else {
                        word.to_be_bytes()
                    }
                })
                .collect::<Vec<_>>()
        };
        let bytes = encode(&words);
        assert_eq!(
            declaration.decode(&bytes, little).unwrap().public,
            [Some(77)]
        );
        assert!(
            declaration
                .decode(&bytes[..bytes.len() - 1], little)
                .is_err()
        );
        assert!(declaration.decode(&bytes, !little).is_err());
        for (index, value) in [
            (0, 0),
            (1, 2),
            (2, 2),
            (3, 2),
            (4, 1),
            (5, 2),
            (7, 1),
            (8, 0),
        ] {
            let mut bad = words;
            bad[index] = value;
            assert!(
                declaration.decode(&encode(&bad), little).is_err(),
                "word {index}"
            );
        }
    }
}

#[test]
fn actual_object_rejects_relocation_missing_symbol_and_wrong_shape() {
    let shape = target();
    let owner = CargoTestArtifacts::new("errno-object-relocation").unwrap();
    let bytes = object(
        "#include <stdint.h>\nconst int external = 9;\nconst struct { const void *pointer; unsigned char tail[40 - sizeof(void *)]; } molt_target_errno_record = { &external, {0} };\n",
        &owner,
    );
    let error = shape.record(&bytes, 40).unwrap_err();
    assert!(error.contains("relocation"), "{error}");
    let owner = CargoTestArtifacts::new("errno-object-identity").unwrap();
    let bytes = object(
        "#include <stdint.h>\nconst uint32_t molt_target_errno_record[10] = {0};\n",
        &owner,
    );
    assert_eq!(shape.record(&bytes, 40).unwrap().len(), 40);
    assert!(shape.record(&bytes, bytes.len() + 1).is_err());
    assert!(shape.record(&bytes[..8], 40).is_err());
    let mut other = shape.clone();
    other.little_endian = !other.little_endian;
    assert!(other.record(&bytes, 40).is_err());
    other = shape.clone();
    other.architecture = object::Architecture::Unknown;
    assert!(other.record(&bytes, 40).is_err());
    other = shape.clone();
    other.format = if shape.format == object::BinaryFormat::Elf {
        object::BinaryFormat::Coff
    } else {
        object::BinaryFormat::Elf
    };
    assert!(other.record(&bytes, 40).is_err());
    let owner = CargoTestArtifacts::new("errno-object-non-data").unwrap();
    let bytes = object("void molt_target_errno_record(void) {}\n", &owner);
    assert!(
        shape
            .record(&bytes, 40)
            .unwrap_err()
            .contains("not defined data")
    );
    let owner = CargoTestArtifacts::new("errno-object-missing").unwrap();
    let bytes = object("const int unrelated = 7;\n", &owner);
    assert!(
        shape
            .record(&bytes, 40)
            .unwrap_err()
            .contains("record symbol")
    );
}

#[test]
fn actual_compiler_rejects_incompatible_os_witness() {
    let owner = CargoTestArtifacts::new("errno-target-refusal").unwrap();
    let declaration = Declaration::parse("EINVAL\t-\tnative\t312\n").unwrap();
    let mut wrong = target();
    if wrong.os == "windows" {
        wrong.os = "freebsd".into();
    } else {
        wrong.os = "windows".into();
        wrong.environment = "msvc".into();
    }
    let error = target_errno::evaluate(
        &declaration,
        &wrong,
        &mut compiler(owner.path()),
        owner.path(),
    )
    .unwrap_err();
    assert!(
        error.contains("command") || error.contains("failed"),
        "{error}"
    );
}

// CPython's pyconfig.h enables Darwin extensions before all system headers.
// The explicit POSIX selector is the independent missing-policy falsifier:
// without the preheader both public entries disappear from actual SDK errno.h.
#[cfg(target_os = "macos")]
#[test]
fn darwin_namespace_precedes_stdint_under_explicit_posix_flags() {
    let owner = CargoTestArtifacts::new("errno-darwin-namespace").unwrap();
    let declaration =
        Declaration::parse("EPROCLIM\t-\tnative\t312\nEAUTH\t-\tnative\t312\n").unwrap();
    let mut build = compiler(owner.path());
    build.define("_POSIX_C_SOURCE", "200809L");
    let values = target_errno::evaluate(&declaration, &target(), &mut build, owner.path()).unwrap();
    // Darwin SDK sys/errno.h and CPython's direct-header configuration are
    // independent of the generated Rust/publication table.
    assert_eq!(values.public, [Some(67), Some(80)]);
}

// An independent GNU header witness catches a missing/late namespace prelude
// even though this provider's errno integer macros are themselves unconditional.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[test]
fn gnu_namespace_preserves_defined_zero_before_system_headers() {
    let declaration = Declaration::parse("EINVAL\t-\tnative\t312\n").unwrap();
    for selected in [None, Some("0")] {
        let owner = CargoTestArtifacts::new("errno-gnu-namespace").unwrap();
        let mut source = String::from("#undef _GNU_SOURCE\n");
        if let Some(value) = selected {
            source.push_str(&format!("#define _GNU_SOURCE {value}\n"));
        }
        source.push_str(&declaration.c_source());
        source.push_str(concat!(
            "\n#ifndef __GLIBC__\n#error expected_glibc_provider\n#endif\n",
            "#ifndef __USE_GNU\n#error GNU_namespace_missing\n#endif\n",
            "_Static_assert(sizeof(error_t) == sizeof(int), \"GNU errno type\");\n",
        ));
        source.push_str(&format!(
            "_Static_assert(_GNU_SOURCE == {}, \"explicit GNU selector\");\n",
            selected.unwrap_or("1")
        ));
        let _object = object(&source, &owner);
    }
}
