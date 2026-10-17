//! Build-only target C errno publication. CPython owns names/order/policy;
//! the selected target compiler owns presence and integer constant evaluation.
//! The fixed-record/native object reader is reconciled from the preserved donor.
use object::{Object, ObjectSection, ObjectSymbol};
use std::collections::{BTreeMap, BTreeSet};

const SYMBOL: &str = "molt_target_errno_record";
const MAGIC: u32 = 0x4d45_5252;
const VERSION: u32 = 1;
const HEADER_WORDS: usize = 4;
const ROW_WORDS: usize = 3;
pub const MAX_OBJECT_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Publication {
    pub name: String,
    pub fallback: Option<String>,
    pub windows_prefers_fallback: bool,
    pub since: u16,
}
#[derive(Debug)]
pub struct Declaration {
    pub raw: Vec<String>,
    pub public: Vec<Publication>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Values {
    pub raw: Vec<Option<i32>>,
    pub public: Vec<Option<i32>>,
}
fn c_identifier(value: &str) -> bool {
    (value.starts_with('E') || value.starts_with("WSA"))
        && value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}
impl Declaration {
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.len() > 128 * 1024 {
            return Err("errno declaration exceeds bounded extent".into());
        }
        let mut seen = BTreeMap::new();
        let mut raw = BTreeSet::new();
        let mut public = Vec::new();
        for line in text
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
        {
            let fields: Vec<_> = line.split('\t').collect();
            if fields.len() != 4 || !c_identifier(fields[0]) {
                return Err(format!("invalid errno declaration: {line:?}"));
            }
            let fallback = match fields[1] {
                "-" => None,
                name if c_identifier(name) => Some(name.to_owned()),
                _ => return Err(format!("invalid errno fallback: {line:?}")),
            };
            let windows_prefers_fallback = match fields[2] {
                "native" => false,
                "wsa" if fallback.is_some() => true,
                _ => return Err(format!("invalid errno Windows selection: {line:?}")),
            };
            let since = match fields[3] {
                "312" => 312,
                "313" => 313,
                "314" => 314,
                "315" => 315,
                _ => return Err(format!("invalid errno Python version: {line:?}")),
            };
            raw.insert(fields[0].to_owned());
            raw.extend(fallback.iter().cloned());
            let row = Publication {
                name: fields[0].into(),
                fallback,
                windows_prefers_fallback,
                since,
            };
            // CPython repeats some insertions. Keep their reverse-alias effects,
            // while rejecting conflicting definitions of one symbolic name.
            if seen
                .insert(row.name.clone(), row.clone())
                .is_some_and(|prior| prior != row)
            {
                return Err(format!("conflicting errno declaration: {line:?}"));
            }
            public.push(row);
        }
        if public.is_empty() || public.len() > 1024 {
            return Err("invalid errno declaration extent".into());
        }
        Ok(Self {
            raw: raw.into_iter().collect(),
            public,
        })
    }
    pub fn byte_len(&self) -> usize {
        (HEADER_WORDS + ROW_WORDS * (self.raw.len() + self.public.len())) * 4
    }
    pub fn c_source(&self) -> String {
        // Namespace selection must precede stdint.h as well as errno.h:
        // either may include the libc feature-selection header.
        let mut out = String::from(include_str!("errno_feature_visibility.h"));
        out.push_str(
            "#include <stdint.h>\n#include <errno.h>\n#ifdef _WIN32\n#include <winsock2.h>\n#endif\n_Static_assert(sizeof(uint32_t) == 4, \"errno word width\");\n_Static_assert(sizeof(int) == 4, \"errno C int width\");\n",
        );
        for name in &self.raw {
            out.push_str(&format!("#ifdef {name}\n_Static_assert((int64_t)({name}) >= INT32_MIN && (int64_t)({name}) <= INT32_MAX, \"errno value width: {name}\");\n#endif\n"));
        }
        out.push_str(&format!(
            "const uint32_t {SYMBOL}[] = {{\n{MAGIC}u, {VERSION}u, {}u, {}u,\n",
            self.raw.len(),
            self.public.len()
        ));
        let present =
            |index: usize, name: &str| format!("{index}u, 1u, (uint32_t)(int32_t)({name}),\n");
        let absent = |index: usize| format!("{index}u, 0u, 0u,\n");
        for (index, name) in self.raw.iter().enumerate() {
            out.push_str(&format!(
                "#ifdef {name}\n{}#else\n{}#endif\n",
                present(index, name),
                absent(index)
            ));
        }
        for (index, row) in self.public.iter().enumerate() {
            if row.windows_prefers_fallback {
                let fallback = row.fallback.as_ref().expect("validated fallback");
                out.push_str(&format!(
                    "#ifdef _WIN32\n#ifdef {fallback}\n{}#else\n{}#endif\n#else\n",
                    present(index, fallback),
                    absent(index)
                ));
            }
            out.push_str(&format!(
                "#ifdef {}\n{}",
                row.name,
                present(index, &row.name)
            ));
            if let Some(fallback) = &row.fallback {
                out.push_str(&format!(
                    "#elif defined({fallback})\n{}",
                    present(index, fallback)
                ));
            }
            out.push_str(&format!("#else\n{}#endif\n", absent(index)));
            if row.windows_prefers_fallback {
                out.push_str("#endif\n");
            }
        }
        out.push_str("};\n");
        out
    }
    pub fn decode(&self, bytes: &[u8], little_endian: bool) -> Result<Values, String> {
        if bytes.len() != self.byte_len() {
            return Err("errno record extent differs from declaration".into());
        }
        let words: Vec<_> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&word| {
                if little_endian {
                    u32::from_le_bytes(word)
                } else {
                    u32::from_be_bytes(word)
                }
            })
            .collect();
        if words[..HEADER_WORDS]
            != [
                MAGIC,
                VERSION,
                self.raw.len() as u32,
                self.public.len() as u32,
            ]
        {
            return Err("errno record magic/version/endian/count mismatch".into());
        }
        let mut values = Vec::with_capacity(self.raw.len() + self.public.len());
        for (index, row) in words[HEADER_WORDS..]
            .as_chunks::<ROW_WORDS>()
            .0
            .iter()
            .enumerate()
        {
            let expected = if index < self.raw.len() {
                index
            } else {
                index - self.raw.len()
            };
            if row[0] != expected as u32 || row[1] > 1 || (row[1] == 0 && row[2] != 0) {
                return Err("errno record row identity/presence mismatch".into());
            }
            values.push((row[1] == 1).then_some(row[2] as i32));
        }
        let public = values.split_off(self.raw.len());
        Ok(Values {
            raw: values,
            public,
        })
    }
}

/// The maintained object reader owns native object parsing. Symbol addresses
/// are section-relative only after subtracting the containing section address;
/// integer reads are byte-based and use the observed target endianness.
pub fn native_record(
    bytes: &[u8],
    size: usize,
    architecture: object::Architecture,
    format: object::BinaryFormat,
    little: bool,
) -> Result<&[u8], String> {
    let file =
        object::File::parse(bytes).map_err(|error| format!("target errno object: {error}"))?;
    if file.kind() != object::ObjectKind::Relocatable
        || file.architecture() != architecture
        || file.format() != format
        || file.is_little_endian() != little
    {
        return Err("target errno object kind/architecture/endian mismatch".into());
    }
    if !matches!(
        file.format(),
        object::BinaryFormat::Elf | object::BinaryFormat::MachO | object::BinaryFormat::Coff
    ) {
        return Err("unsupported native errno object format".into());
    }
    let decorated = format!("_{SYMBOL}");
    let mut found = None;
    for symbol in file.symbols() {
        let name = symbol.name().map_err(|error| error.to_string())?;
        let is_name = name == SYMBOL
            || (matches!(
                file.format(),
                object::BinaryFormat::MachO | object::BinaryFormat::Coff
            ) && name == decorated);
        if !is_name {
            continue;
        }
        if found.is_some() || symbol.kind() != object::SymbolKind::Data || symbol.is_undefined() {
            return Err("errno record symbol duplicate or not defined data".into());
        }
        if symbol.size() != 0 && symbol.size() != size as u64 {
            return Err("errno symbol extent differs from record".into());
        }
        let index = symbol
            .section_index()
            .ok_or("errno record has no physical section")?;
        let section = file
            .section_by_index(index)
            .map_err(|error| error.to_string())?;
        let offset = symbol
            .address()
            .checked_sub(section.address())
            .ok_or("errno symbol precedes section")?;
        let end = offset
            .checked_add(size as u64)
            .ok_or("errno record offset overflow")?;
        if section.align() < 4 || symbol.address() % 4 != 0 {
            return Err("errno record symbol is unaligned".into());
        }
        // The record contains integers only. Any relocation in its span means
        // the compiler did not produce the promised self-contained constants.
        if section.relocations().any(|(at, relocation)| {
            let width = u64::from(relocation.size()).div_ceil(8).max(1);
            at < end
                && at
                    .checked_add(width)
                    .is_none_or(|relocation_end| relocation_end > offset)
        }) {
            return Err("errno record contains a relocation".into());
        }
        let data = section.data().map_err(|error| error.to_string())?;
        let start = usize::try_from(offset).map_err(|_| "errno offset exceeds host width")?;
        let end = usize::try_from(end).map_err(|_| "errno extent exceeds host width")?;
        found = Some(
            data.get(start..end)
                .ok_or("errno record exceeds section bounds")?,
        );
    }
    found.ok_or_else(|| "target compiler did not emit the errno record symbol".into())
}

impl Declaration {
    pub fn rust_source(&self, values: &Values) -> Result<String, String> {
        if values.raw.len() != self.raw.len() || values.public.len() != self.public.len() {
            return Err("errno values differ from declaration extent".into());
        }
        let mut out = String::from(
            "// @generated from selected target C headers; do not edit.\npub fn errno_constants(python_version: u16) -> impl Iterator<Item = (&'static str, i64)> + Clone {\nconst ROWS: &[(&str, i64, u16)] = &[\n",
        );
        for (row, value) in self.public.iter().zip(&values.public) {
            if let Some(value) = value {
                out.push_str(&format!("({:?}, {value}, {}),\n", row.name, row.since));
            }
        }
        out.push_str("];\nROWS.iter().filter_map(move |&(name, value, since)| (python_version >= since).then_some((name, value)))\n}\n");
        // Portable raw C anchors are independently supplied by the existing
        // target libc crate, as in libc-test/ctest's header-vs-Rust controls.
        // These assertions do not compare public WSA aliases to CRT values,
        // and numeric agreement is not a provider/SDK provenance proof.
        for (name, value) in self.raw.iter().zip(&values.raw) {
            if matches!(
                name.as_str(),
                "EAGAIN" | "EINVAL" | "ENOENT" | "EDOM" | "ERANGE"
            ) {
                let value =
                    value.ok_or_else(|| format!("required raw C errno anchor absent: {name}"))?;
                out.push_str(&format!("const _: () = assert!(libc::{name} == {value}, \"target C/libc errno anchor differs: {name}\");\n"));
            }
        }
        Ok(out)
    }
}

/// Cargo owns target selection; these are object encodings and C ABI witnesses,
/// not a second support/capability matrix. Backend admission stays unchanged.
#[derive(Clone, Debug)]
pub struct NativeTarget {
    pub architecture: object::Architecture,
    pub format: object::BinaryFormat,
    pub little_endian: bool,
    pub pointer_bits: u32,
    pub os: String,
    pub environment: String,
    pub abi: String,
}
impl NativeTarget {
    pub fn from_cargo(
        arch: &str,
        os: &str,
        environment: &str,
        abi: &str,
        pointer_width: &str,
        endian: &str,
    ) -> Result<Self, String> {
        use object::Architecture as A;
        let architecture = match (arch, pointer_width) {
            ("x86", "32") => A::I386,
            ("x86_64", "64") => A::X86_64,
            ("x86_64", "32") => A::X86_64_X32,
            ("aarch64", "64") => A::Aarch64,
            ("aarch64", "32") => A::Aarch64_Ilp32,
            ("arm", "32") => A::Arm,
            ("riscv32", "32") => A::Riscv32,
            ("riscv64", "64") => A::Riscv64,
            ("powerpc", "32") => A::PowerPc,
            ("powerpc64", "64") => A::PowerPc64,
            ("s390x", "64") => A::S390x,
            ("loongarch64", "64") => A::LoongArch64,
            ("mips", "32") => A::Mips,
            ("mips64", "64") => A::Mips64,
            ("mips64", "32") => A::Mips64_N32,
            ("sparc", "32") => A::Sparc,
            ("sparc64", "64") => A::Sparc64,
            ("m68k", "32") => A::M68k,
            _ => {
                return Err(format!(
                    "unknown errno object encoding {arch}/{pointer_width}"
                ));
            }
        };
        let format = if os == "windows" {
            object::BinaryFormat::Coff
        } else if matches!(os, "macos" | "ios" | "tvos" | "watchos" | "visionos") {
            object::BinaryFormat::MachO
        } else if super::c_codegen::target_uses_elf(os) {
            object::BinaryFormat::Elf
        } else {
            return Err(format!("unknown errno target object format for {os}"));
        };
        let little_endian = match endian {
            "little" => true,
            "big" => false,
            _ => return Err("invalid Cargo target byte order".into()),
        };
        Ok(Self {
            architecture,
            format,
            little_endian,
            pointer_bits: pointer_width.parse().map_err(|_| "invalid pointer width")?,
            os: os.into(),
            environment: environment.into(),
            abi: abi.into(),
        })
    }

    /// Assert the effective C OS/ABI as well as the produced object shape.
    /// Linux musl intentionally has no positive predefined macro: its selected
    /// provider identity remains a toolchain-custody obligation, not an inferred
    /// fact from the absence of __GLIBC__. Likewise GNU Windows does not itself
    /// select UCRT versus MSVCRT; the selected SDK/compiler owns that choice.
    pub fn c_assertions(&self) -> Result<String, String> {
        let condition = match self.os.as_str() {
            "linux" => "defined(__linux__) && !defined(__ANDROID__)",
            "android" => "defined(__linux__) && defined(__ANDROID__)",
            "windows" => "defined(_WIN32)",
            "macos" => "defined(__APPLE__) && defined(__MACH__) && TARGET_OS_OSX",
            "ios" => "defined(__APPLE__) && defined(__MACH__) && TARGET_OS_IOS",
            "tvos" => "defined(__APPLE__) && defined(__MACH__) && TARGET_OS_TV",
            "watchos" => "defined(__APPLE__) && defined(__MACH__) && TARGET_OS_WATCH",
            "visionos" => "defined(__APPLE__) && defined(__MACH__) && TARGET_OS_VISION",
            "freebsd" => "defined(__FreeBSD__)",
            "netbsd" => "defined(__NetBSD__)",
            "openbsd" => "defined(__OpenBSD__)",
            "dragonfly" => "defined(__DragonFly__)",
            "solaris" | "illumos" => "defined(__sun) && defined(__SVR4)",
            "haiku" => "defined(__HAIKU__)",
            "fuchsia" => "defined(__Fuchsia__)",
            other => return Err(format!("errno C OS witness is undefined for {other}")),
        };
        let mut out =
            String::from("\n/* Effective compiler/header ABI, after target headers. */\n");
        if self.format == object::BinaryFormat::MachO {
            out.push_str("#include <TargetConditionals.h>\n");
        }
        if matches!(self.os.as_str(), "linux" | "android") {
            out.push_str("#include <features.h>\n");
        }
        out.push_str(&format!("#if !({condition})\n#error errno C compiler OS differs from Cargo target\n#endif\n_Static_assert(sizeof(void *) * 8 == {}, \"errno target pointer width\");\n", self.pointer_bits));
        let crt = match (self.os.as_str(), self.environment.as_str()) {
            ("windows", "msvc") => Some("defined(_MSC_VER) && !defined(__MINGW32__)"),
            ("windows", "gnu" | "gnullvm") => Some("defined(__MINGW32__)"),
            ("linux", "gnu") => Some("defined(__GLIBC__) && !defined(__UCLIBC__)"),
            ("linux", "musl") => {
                Some("!defined(__GLIBC__) && !defined(__UCLIBC__) && !defined(__BIONIC__)")
            }
            ("linux", "uclibc") => Some("defined(__UCLIBC__)"),
            ("android", _) => Some("defined(__BIONIC__)"),
            ("windows" | "linux", other) => {
                return Err(format!(
                    "errno C environment witness is undefined for {other}"
                ));
            }
            _ => None,
        };
        if let Some(crt) = crt {
            out.push_str(&format!(
                "#if !({crt})\n#error errno C runtime differs from Cargo environment\n#endif\n"
            ));
        }
        if self.abi == "eabihf" {
            out.push_str("#if !defined(__ARM_PCS_VFP)\n#error errno C compiler lacks target hard-float calling convention\n#endif\n");
        }
        Ok(out)
    }

    pub fn record<'a>(&self, bytes: &'a [u8], size: usize) -> Result<&'a [u8], String> {
        if bytes.len() as u64 > MAX_OBJECT_BYTES {
            return Err("errno object exceeds bounded evaluator extent".into());
        }
        native_record(
            bytes,
            size,
            self.architecture,
            self.format,
            self.little_endian,
        )
    }
}

/// Compile the declaration once; never link or execute the target object.
/// Caller supplies the existing cc plan and its already-selected target. Tests
/// exercise this same path with actual C fixture headers, not a second parser.
pub fn evaluate(
    declaration: &Declaration,
    target: &NativeTarget,
    build: &mut cc::Build,
    out_dir: &std::path::Path,
) -> Result<Values, String> {
    use std::fs;
    use std::io::Read;
    let source_path = out_dir.join("molt_target_errno_record.c");
    let mut source = declaration.c_source();
    source.push_str(&target.c_assertions()?);
    fs::write(&source_path, source).map_err(|error| error.to_string())?;
    build.file(&source_path).opt_level(0).std("c11");
    let objects = build
        .try_compile_intermediates()
        .map_err(|error| error.to_string())?;
    let [path] = objects.as_slice() else {
        return Err("errno evaluator must emit exactly one object".into());
    };
    let size = fs::metadata(path).map_err(|error| error.to_string())?.len();
    if size > MAX_OBJECT_BYTES {
        return Err("errno object exceeds bounded evaluator extent".into());
    }
    // Bound the read itself as well as the pre-read metadata: an unexpected
    // replacement/growth must not turn a tiny compiler record into an unbounded
    // allocation. The record reader rejects the extra byte when over limit.
    let mut bytes = Vec::with_capacity(size as usize);
    fs::File::open(path)
        .map_err(|error| error.to_string())?
        .take(MAX_OBJECT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    declaration.decode(
        target.record(&bytes, declaration.byte_len())?,
        target.little_endian,
    )
}
