//! Real inspector tests; captured LLVM objects and adversarial writer fixtures
//! have separate provenance in tests/tools/fixtures/native_size_facts.json.
use object::{
    Architecture, BinaryFormat, Endianness, SectionKind, SymbolFlags, SymbolKind, SymbolScope,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
struct TempFile(PathBuf);
impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn temporary(bytes: &[u8]) -> TempFile {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "molt-native-facts-{}-{nonce}.o",
        std::process::id()
    ));
    fs::write(&path, bytes).unwrap();
    TempFile(path)
}
fn command(path: &TempFile) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_molt-backend"));
    cmd.arg("--scan-native-artifact-facts").arg(&path.0);
    cmd
}
fn captured() -> Value {
    serde_json::from_str(include_str!(
        "../../../tests/tools/fixtures/native_size_facts.json"
    ))
    .unwrap()
}
fn scan(bytes: &[u8]) -> Value {
    let file = temporary(bytes);
    let result = command(&file).output().expect("actual scanner");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(result.stderr.is_empty());
    let value: Value = serde_json::from_slice(&result.stdout).expect("complete JSON");
    assert_eq!(value["ok"], true);
    assert_eq!(value["facts"]["size"], bytes.len());
    // Decode the emitted wire value independently of the compiler's encoder.
    let encoded = value["facts"]["sha256"].as_str().unwrap();
    assert_eq!(encoded.len(), 64);
    assert_eq!(encoded, encoded.to_ascii_lowercase());
    let decoded: Vec<u8> = (0..encoded.len())
        .step_by(2)
        .map(|offset| u8::from_str_radix(&encoded[offset..offset + 2], 16).unwrap())
        .collect();
    assert_eq!(decoded.as_slice(), Sha256::digest(bytes).as_slice());
    value
}
#[test]
fn actual_llvm_elf_and_macho_objects_have_independent_ranges_and_names() {
    for case in captured()["cases"].as_array().unwrap() {
        let bytes: Vec<u8> = serde_json::from_value(case["artifact_bytes"].clone()).unwrap();
        let value = scan(&bytes);
        let image = &value["facts"]["slices"][0];
        let macho = case["name"] == "macho-arm64";
        assert_eq!(image["format"], if macho { "macho" } else { "elf" });
        let sections = image["sections"].as_array().unwrap();
        let physical: u64 = sections
            .iter()
            .filter_map(|s| s["file_range"]["size"].as_u64())
            .sum();
        assert_eq!(physical, if macho { 76 } else { 864 });
        let text = sections
            .iter()
            .find(|s| s["name"]["utf8"] == if macho { "__text" } else { ".text" })
            .unwrap();
        assert_eq!(text["file_range"]["offset"], if macho { 568 } else { 64 });
        assert_eq!(text["file_range"]["size"], 64);
        let bss = sections
            .iter()
            .find(|s| s["name"]["utf8"] == if macho { "__common" } else { ".bss" })
            .unwrap();
        assert_eq!(bss["declared_size"], 512);
        assert!(bss["file_range"].is_null());
        let symbols = image["symbols"].as_array().unwrap();
        let indirect = symbols
            .iter()
            .find(|s| {
                s["name"]["utf8"]
                    .as_str()
                    .unwrap_or("")
                    .contains("_RNvCsgrakSpcflzr_")
            })
            .unwrap();
        assert_eq!(
            indirect["demangled_name"],
            "molt_runtime::molt_call_indirect0"
        );
        if macho {
            assert!(indirect["declared_size"].is_null());
        } else {
            assert_eq!(indirect["declared_size"], 8);
        }
        if !macho {
            let function = symbols
                .iter()
                .find(|s| s["name"]["utf8"] == "fixture_function")
                .unwrap();
            let alias = symbols
                .iter()
                .find(|s| s["name"]["utf8"] == "fixture_alias")
                .unwrap();
            assert_eq!(function["address"], 0);
            assert_eq!(function["declared_size"], 28);
            assert_eq!(function["address"], alias["address"]);
            assert_eq!(function["declared_size"], alias["declared_size"]);
        }
    }
}
fn adversarial(format: BinaryFormat) -> (Vec<u8>, Vec<Vec<u8>>) {
    let mut obj = object::write::Object::new(format, Architecture::Aarch64, Endianness::Little);
    // This fixture authors exact wire names, including Mach-O's leading underscore.
    // object::write otherwise applies its default global prefix a second time.
    obj.set_mangling(object::write::Mangling::None);
    let section = obj.add_section(Vec::new(), b"__t\nS {\nN: }".to_vec(), SectionKind::Text);
    obj.append_section_data(section, &[0; 16], 4);
    let mut names = vec![
        b"raw\n  }\nSymbol {\n Name: injected".to_vec(),
        b"invalid\xffname".to_vec(),
        b"name with spaces".to_vec(),
        b"_ZN12molt_runtime3foo17h0123456789abcdefE".to_vec(),
    ];
    if format == BinaryFormat::MachO {
        for name in &mut names {
            name.insert(0, b'_');
        }
    }
    for (index, name) in names.iter().enumerate() {
        obj.add_symbol(object::write::Symbol {
            name: name.clone(),
            value: index as u64 * 4,
            size: 4,
            kind: SymbolKind::Text,
            scope: SymbolScope::Linkage,
            weak: false,
            section: object::write::SymbolSection::Section(section),
            flags: SymbolFlags::None,
        });
    }
    (obj.write().unwrap(), names)
}
#[test]
fn escaped_and_non_utf8_names_never_inject_records() {
    for format in [BinaryFormat::Elf, BinaryFormat::MachO] {
        let (bytes, names) = adversarial(format);
        let value = scan(&bytes);
        let image = &value["facts"]["slices"][0];
        let symbols = image["symbols"].as_array().unwrap();
        for expected in names {
            let count = symbols
                .iter()
                .filter(|s| {
                    let raw = &s["name"];
                    if let Some(text) = raw["utf8"].as_str() {
                        text.as_bytes() == expected
                    } else {
                        serde_json::from_value::<Vec<u8>>(raw["bytes"].clone())
                            .is_ok_and(|actual| actual == expected)
                    }
                })
                .count();
            assert_eq!(count, 1, "lossless {format:?} symbol {expected:?}");
        }
        assert_eq!(
            symbols
                .iter()
                .filter(|s| s["demangled_name"] == "molt_runtime::foo")
                .count(),
            1
        );
        assert!(!symbols.iter().any(|s| s["name"]["utf8"] == "injected"));
        assert!(
            image["sections"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["name"]["utf8"] == "__t\nS {\nN: }")
        );
    }
}
#[test]
fn malformed_input_and_budget_failures_emit_only_failure_json() {
    for bytes in [Vec::new(), b"!<arch>\n".to_vec(), b"MZfake".to_vec()] {
        let file = temporary(&bytes);
        let result = command(&file).output().unwrap();
        assert_eq!(result.status.code(), Some(2));
        let value: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(value["ok"], false);
        assert!(value.get("facts").is_none());
    }
    let bytes: Vec<u8> =
        serde_json::from_value(captured()["cases"][0]["artifact_bytes"].clone()).unwrap();
    let file = temporary(&bytes);
    for (limit, expected) in [("1024", "input"), ("2048", "response")] {
        let result = command(&file)
            .env("MOLT_BACKEND_STDIN_REQUEST_LIMIT_BYTES", limit)
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(2));
        let value: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(value["ok"], false);
        assert!(value["error"].as_str().unwrap().contains(expected));
        assert!(value.get("facts").is_none());
    }
}

// Source-authored universal containers built from the maintained object writer,
// not claimed llvm-lipo captures. Table order deliberately differs from offsets.
fn universal(wide: bool) -> Vec<u8> {
    use object::endian::{BigEndian, U32, U64};
    use object::macho::{FatArch32, FatArch64, FatHeader};
    let mut images = Vec::new();
    for architecture in [Architecture::X86_64, Architecture::Aarch64] {
        let mut image =
            object::write::Object::new(BinaryFormat::MachO, architecture, Endianness::Little);
        image.set_macho_cpu_subtype(0);
        let section = image.add_section(b"__TEXT".to_vec(), b"__text".to_vec(), SectionKind::Text);
        image.append_section_data(section, &[0; 16], 4);
        images.push(image.write().unwrap());
    }
    assert!(images.iter().all(|image| image.len() < 1024));
    let mut bytes = object::pod::bytes_of(&FatHeader {
        magic: U32::new(
            BigEndian,
            if wide {
                object::macho::FAT_MAGIC_64
            } else {
                object::macho::FAT_MAGIC
            },
        ),
        nfat_arch: U32::new(BigEndian, 2),
    })
    .to_vec();
    for ((image, offset), cpu) in images.iter().zip([2048u64, 1024]).zip([
        object::macho::CPU_TYPE_X86_64,
        object::macho::CPU_TYPE_ARM64,
    ]) {
        if wide {
            bytes.extend_from_slice(object::pod::bytes_of(&FatArch64 {
                cputype: U32::new(BigEndian, cpu),
                cpusubtype: U32::new(BigEndian, 0),
                offset: U64::new(BigEndian, offset),
                size: U64::new(BigEndian, image.len() as u64),
                align: U32::new(BigEndian, 10),
                reserved: U32::new(BigEndian, 0),
            }));
        } else {
            bytes.extend_from_slice(object::pod::bytes_of(&FatArch32 {
                cputype: U32::new(BigEndian, cpu),
                cpusubtype: U32::new(BigEndian, 0),
                offset: U32::new(BigEndian, offset as u32),
                size: U32::new(BigEndian, image.len() as u32),
                align: U32::new(BigEndian, 10),
            }));
        }
    }
    bytes.resize(2048 + images[0].len(), 0);
    bytes[2048..2048 + images[0].len()].copy_from_slice(&images[0]);
    bytes[1024..1024 + images[1].len()].copy_from_slice(&images[1]);
    bytes
}
#[test]
fn universal_tables_preserve_order_and_reject_bounds_identity_and_overlap() {
    for wide in [false, true] {
        let bytes = universal(wide);
        let value = scan(&bytes);
        let images = value["facts"]["slices"].as_array().unwrap();
        assert_eq!(images.len(), 2);
        assert_eq!(images[0]["offset"], 2048);
        assert_eq!(images[1]["offset"], 1024);
        assert_eq!(images[0]["machine"], object::macho::CPU_TYPE_X86_64);
        assert_eq!(images[1]["machine"], object::macho::CPU_TYPE_ARM64);
        let stride = if wide { 32 } else { 20 };
        let mut controls = Vec::new();
        let mut bad = bytes.clone();
        bad[12..16].copy_from_slice(&1u32.to_be_bytes());
        controls.push(bad); // wrong subtype
        let mut bad = bytes.clone();
        bad[8 + stride..12 + stride].copy_from_slice(&object::macho::CPU_TYPE_X86_64.to_be_bytes());
        controls.push(bad); // duplicate identity
        let mut bad = bytes.clone();
        if wide {
            bad[16..24].copy_from_slice(&0u64.to_be_bytes());
        } else {
            bad[16..20].copy_from_slice(&0u32.to_be_bytes());
        }
        controls.push(bad); // table overlap
        let mut bad = bytes.clone();
        let size_field = if wide { 24 } else { 20 };
        if wide {
            bad[size_field..size_field + 8].copy_from_slice(&4096u64.to_be_bytes());
        } else {
            bad[size_field..size_field + 4].copy_from_slice(&4096u32.to_be_bytes());
        }
        controls.push(bad); // out-of-file range
        let mut bad = bytes.clone();
        let second_size_field = 8 + stride + if wide { 16 } else { 12 };
        if wide {
            bad[second_size_field..second_size_field + 8]
                .copy_from_slice(&((bytes.len() - 1024) as u64).to_be_bytes());
        } else {
            bad[second_size_field..second_size_field + 4]
                .copy_from_slice(&((bytes.len() - 1024) as u32).to_be_bytes());
        }
        controls.push(bad); // valid slice headers but overlapping physical ranges
        if wide {
            let mut bad = bytes.clone();
            bad[36..40].copy_from_slice(&1u32.to_be_bytes());
            controls.push(bad);
        }
        for bad in controls {
            let input = temporary(&bad);
            let output = command(&input).output().unwrap();
            assert_eq!(output.status.code(), Some(2));
            let result: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(result["ok"], false);
            assert!(result.get("facts").is_none());
        }
    }
}

// Source-authored ABI controls. object owns file emission; expectations come
// from AAELF32 symbol-value semantics, independently of inspector projection.
#[test]
fn arm_thumb_tag_is_only_an_arm_function_value_projection() {
    for (architecture, kind, value, elf_type, expected) in [
        (
            Architecture::Arm,
            SymbolKind::Text,
            1,
            object::elf::STT_FUNC,
            true,
        ),
        (
            Architecture::Arm,
            SymbolKind::Text,
            0,
            object::elf::STT_FUNC,
            false,
        ),
        (
            Architecture::Arm,
            SymbolKind::Data,
            1,
            object::elf::STT_OBJECT,
            false,
        ),
        (
            Architecture::Aarch64,
            SymbolKind::Text,
            1,
            object::elf::STT_FUNC,
            false,
        ),
    ] {
        let mut image =
            object::write::Object::new(BinaryFormat::Elf, architecture, Endianness::Little);
        let section = image.add_section(Vec::new(), b".text".to_vec(), SectionKind::Text);
        image.append_section_data(section, &[0; 4], 4);
        image.add_symbol(object::write::Symbol {
            name: b"fixture".to_vec(),
            value,
            size: if expected { 4 } else { 1 },
            kind,
            scope: SymbolScope::Linkage,
            weak: false,
            section: object::write::SymbolSection::Section(section),
            flags: SymbolFlags::Elf {
                st_info: (object::elf::STB_GLOBAL << 4) | elf_type,
                st_other: 0,
            },
        });
        let facts = scan(&image.write().unwrap());
        let symbol = facts["facts"]["slices"][0]["symbols"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["name"]["utf8"] == "fixture")
            .unwrap();
        assert_eq!(symbol["address"], value); // Raw st_value must survive.
        assert_eq!(symbol["arm_thumb"], expected);
        assert_eq!(symbol["declared_size"], if expected { 4 } else { 1 });
    }
}

#[test]
fn macho_kind_projection_includes_dylib_dyld_and_bundle() {
    // Header-kind variants derived from a retained LLVM object, not claimed
    // linked/loader-valid captures. They test this exact typed field projection.
    let fixture = captured();
    let case = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == "macho-arm64")
        .unwrap();
    let original: Vec<u8> = serde_json::from_value(case["artifact_bytes"].clone()).unwrap();
    for (filetype, expected) in [
        (1u32, "object"),
        (2, "executable"),
        (6, "dynamic"),
        (7, "dynamic"),
        (8, "dynamic"),
    ] {
        let mut bytes = original.clone();
        bytes[12..16].copy_from_slice(&filetype.to_le_bytes());
        let facts = scan(&bytes);
        assert_eq!(facts["facts"]["slices"][0]["kind"], expected);
    }
    let mut bytes = original;
    bytes[12..16].copy_from_slice(&object::macho::MH_CORE.to_le_bytes());
    let input = temporary(&bytes);
    let result = command(&input).output().unwrap();
    assert_eq!(result.status.code(), Some(2));
    let facts: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(facts["ok"], false);
    assert!(facts.get("facts").is_none());
}

#[cfg(unix)]
#[test]
fn no_writer_fifo_is_rejected_before_read_for_each_inspector() {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let input = temporary(&[]);
    fs::remove_file(&input.0).unwrap();
    let name = CString::new(input.0.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let result = command(&input).output().unwrap();
    assert_eq!(result.status.code(), Some(2));
    let facts: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(facts["ok"], false);
    assert!(facts["error"].as_str().unwrap().contains("regular file"));
    #[cfg(feature = "wasm-backend")]
    for mode in ["--scan-wasm-link-facts", "--publish-wasm-link-facts"] {
        let output = temporary(b"preserve");
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_molt-backend"));
        cmd.arg(mode).arg(&input.0);
        if mode == "--publish-wasm-link-facts" {
            cmd.arg("--output").arg(&output.0);
        }
        let result = cmd.output().unwrap();
        assert!(!result.status.success());
        assert_eq!(fs::read(&output.0).unwrap(), b"preserve");
        let facts: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(facts["ok"], false);
        assert!(facts["error"].as_str().unwrap().contains("regular file"));
    }
}

#[cfg(windows)]
#[test]
fn windows_character_device_is_not_a_regular_artifact() {
    let result = Command::new(env!("CARGO_BIN_EXE_molt-backend"))
        .args(["--scan-native-artifact-facts", "NUL"])
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(2));
    let facts: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(facts["ok"], false);
    assert!(facts.get("facts").is_none());
}
