//! Read-only native artifact facts. `object` owns object decoding; names never
//! travel through a human-readable dump. Normal compilation never calls this.
use molt_ir::content_digest::bytes_to_lower_hex;
use object::read::elf::FileHeader;
use object::read::macho::{FatArch, MachHeader, MachOFatFile32, MachOFatFile64};
use object::{BinaryFormat, Object, ObjectSection, ObjectSymbol};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::{self, Write};

const SCHEMA: u32 = 1;

fn invalid(message: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

struct BoundedResponse {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for BoundedResponse {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(invalid("native facts response exceeds request byte limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Serialize, Clone, Copy)]
struct Range {
    offset: u64,
    size: u64,
}
fn range(offset: u64, size: u64, limit: usize) -> io::Result<Range> {
    if offset > limit as u64 || size > limit as u64 - offset {
        return Err(invalid("native metadata range exceeds its input image"));
    }
    Ok(Range { offset, size })
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum Name<'a> {
    Utf8(&'a str),
    Bytes(&'a [u8]),
}
fn name(bytes: &[u8]) -> Name<'_> {
    match std::str::from_utf8(bytes) {
        Ok(text) => Name::Utf8(text),
        Err(_) => Name::Bytes(bytes),
    }
}

#[derive(Serialize)]
struct Section<'a> {
    index: usize,
    name: Name<'a>,
    address: u64,
    declared_size: u64,
    file_range: Option<Range>,
    compression: String,
    uncompressed_size: u64,
}
#[derive(Serialize)]
struct Symbol<'a> {
    name: Name<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    demangled_name: Option<String>,
    section_index: Option<usize>,
    address: u64,
    arm_thumb: bool,
    declared_size: Option<u64>,
    kind: String,
    is_definition: bool,
}

fn json(out: &mut BoundedResponse, value: &impl Serialize) -> io::Result<()> {
    serde_json::to_writer(out, value).map_err(io::Error::other)
}

fn scan_image(out: &mut BoundedResponse, bytes: &[u8], origin: u64) -> io::Result<()> {
    let file = object::File::parse(bytes).map_err(invalid)?;
    let format = match file.format() {
        BinaryFormat::Elf => "elf",
        BinaryFormat::MachO => "macho",
        _ => {
            return Err(invalid(
                "native size facts support ELF and Mach-O images only",
            ));
        }
    };
    let (machine, subtype) = match &file {
        object::File::Elf32(file) => (u32::from(file.elf_header().e_machine(file.endian())), None),
        object::File::Elf64(file) => (u32::from(file.elf_header().e_machine(file.endian())), None),
        object::File::MachO32(file) => (
            file.macho_header().cputype(file.endian()),
            Some(file.macho_header().cpusubtype(file.endian())),
        ),
        object::File::MachO64(file) => (
            file.macho_header().cputype(file.endian()),
            Some(file.macho_header().cpusubtype(file.endian())),
        ),
        _ => return Err(invalid("unsupported native facts format")),
    };
    // Match the admitted native header family. object::ObjectKind currently
    // calls MH_BUNDLE/MH_DYLINKER Unknown, although both are shared images.
    let macho_type = match &file {
        object::File::MachO32(file) => Some(file.macho_header().filetype(file.endian())),
        object::File::MachO64(file) => Some(file.macho_header().filetype(file.endian())),
        _ => None,
    };
    let kind = if let Some(filetype) = macho_type {
        match filetype {
            object::macho::MH_OBJECT => "object",
            object::macho::MH_EXECUTE => "executable",
            object::macho::MH_DYLIB | object::macho::MH_DYLINKER | object::macho::MH_BUNDLE => {
                "dynamic"
            }
            _ => return Err(invalid("unsupported native image kind")),
        }
    } else {
        match file.kind() {
            object::ObjectKind::Relocatable => "object",
            object::ObjectKind::Executable => "executable",
            object::ObjectKind::Dynamic => "dynamic",
            _ => return Err(invalid("unsupported native image kind")),
        }
    };
    write!(
        out,
        "{{\"offset\":{origin},\"size\":{},\"format\":",
        bytes.len()
    )?;
    json(out, &format)?;
    out.write_all(b",\"kind\":")?;
    json(out, &kind)?;
    out.write_all(b",\"machine\":")?;
    json(out, &machine)?;
    out.write_all(b",\"subtype\":")?;
    json(out, &subtype)?;
    write!(
        out,
        ",\"bits\":{},\"little_endian\":{},\"sections\":[",
        if file.is_64() { 64 } else { 32 },
        file.is_little_endian()
    )?;
    let mut first = true;
    for section in file.sections() {
        let physical = section
            .file_range()
            .map(|(offset, size)| range(offset, size, bytes.len()))
            .transpose()?;
        let compression = section.compressed_file_range().map_err(invalid)?;
        if physical.is_some() {
            range(compression.offset, compression.compressed_size, bytes.len())?;
        }
        if !first {
            out.write_all(b",")?;
        }
        first = false;
        json(
            out,
            &Section {
                index: section.index().0,
                name: name(section.name_bytes().map_err(invalid)?),
                address: section.address(),
                declared_size: section.size(),
                file_range: physical,
                compression: format!("{:?}", compression.format),
                uncompressed_size: compression.uncompressed_size,
            },
        )?;
    }
    out.write_all(b"],\"symbols\":[")?;
    first = true;
    for symbol in file.symbols().chain(file.dynamic_symbols()) {
        let raw_name = symbol.name_bytes().map_err(invalid)?;
        let normalized = if format == "macho" {
            raw_name.strip_prefix(b"_").unwrap_or(raw_name)
        } else {
            raw_name
        };
        let demangled = std::str::from_utf8(normalized)
            .ok()
            .and_then(|name| rustc_demangle::try_demangle(name).ok())
            .map(|name| format!("{name:#}"))
            .filter(|demangled| {
                demangled.as_bytes() != normalized && !demangled.contains("{size limit reached}")
            });
        if !first {
            out.write_all(b",")?;
        }
        first = false;
        // AAELF32 5.5.3: only EM_ARM STT_FUNC uses bit zero as Thumb state.
        // Preserve st_value verbatim; the receiver maps only this typed tag.
        // https://github.com/ARM-software/abi-aa/blob/main/aaelf32/aaelf32.rst#553-symbol-values
        let arm_thumb = format == "elf"
            && machine == u32::from(object::elf::EM_ARM)
            && symbol.address() & 1 != 0
            && matches!(symbol.flags(), object::SymbolFlags::Elf { st_info, .. }
                if st_info & 0x0f == object::elf::STT_FUNC);
        json(
            out,
            &Symbol {
                name: name(raw_name),
                demangled_name: demangled,
                section_index: symbol.section_index().map(|index| index.0),
                address: symbol.address(),
                arm_thumb,
                declared_size: if format == "macho" {
                    None
                } else {
                    Some(symbol.size())
                },
                kind: format!("{:?}", symbol.kind()),
                is_definition: symbol.is_definition(),
            },
        )?;
    }
    out.write_all(b"]}")
}

fn fat_ranges<F: FatArch>(arches: &[F], bytes: &[u8]) -> io::Result<Vec<Range>> {
    if arches.is_empty() {
        return Err(invalid("universal image has no slices"));
    }
    let table_size = std::mem::size_of::<object::macho::FatHeader>()
        .checked_add(
            arches
                .len()
                .checked_mul(std::mem::size_of::<F>())
                .ok_or_else(|| invalid("universal table size overflow"))?,
        )
        .ok_or_else(|| invalid("universal table size overflow"))?;
    let mut identities = std::collections::BTreeSet::new();
    let mut ranges = Vec::with_capacity(arches.len());
    for arch in arches {
        let (offset, size) = arch.file_range();
        let item = range(offset, size, bytes.len())?;
        if size == 0
            || offset < table_size as u64
            || arch.align() >= 64
            || offset % (1u64 << arch.align()) != 0
        {
            return Err(invalid(
                "invalid universal slice alignment or header overlap",
            ));
        }
        if !identities.insert((arch.cputype(), arch.cpusubtype())) {
            return Err(invalid("universal slice identity duplicated"));
        }
        let image = object::File::parse(arch.data(bytes).map_err(invalid)?).map_err(invalid)?;
        let identity = match &image {
            object::File::MachO32(file) => (
                file.macho_header().cputype(file.endian()),
                file.macho_header().cpusubtype(file.endian()),
            ),
            object::File::MachO64(file) => (
                file.macho_header().cputype(file.endian()),
                file.macho_header().cpusubtype(file.endian()),
            ),
            _ => return Err(invalid("universal slice is not Mach-O")),
        };
        if identity != (arch.cputype(), arch.cpusubtype()) {
            return Err(invalid("universal slice target disagrees with its table"));
        }
        ranges.push(item);
    }
    let mut sorted = ranges.clone();
    sorted.sort_by_key(|item| item.offset);
    if sorted
        .windows(2)
        .any(|pair| pair[0].offset + pair[0].size > pair[1].offset)
    {
        return Err(invalid("universal slice ranges overlap"));
    }
    Ok(ranges)
}

fn scan(bytes: &[u8], limit: usize) -> io::Result<Vec<u8>> {
    let slices = match object::FileKind::parse(bytes).map_err(invalid)? {
        object::FileKind::MachOFat32 => fat_ranges(
            MachOFatFile32::parse(bytes).map_err(invalid)?.arches(),
            bytes,
        )?,
        object::FileKind::MachOFat64 => {
            let file = MachOFatFile64::parse(bytes).map_err(invalid)?;
            if file
                .arches()
                .iter()
                .any(|arch| arch.reserved.get(object::endian::BigEndian) != 0)
            {
                return Err(invalid("universal 64-bit reserved field is nonzero"));
            }
            fat_ranges(file.arches(), bytes)?
        }
        object::FileKind::Elf32
        | object::FileKind::Elf64
        | object::FileKind::MachO32
        | object::FileKind::MachO64 => vec![range(0, bytes.len() as u64, bytes.len())?],
        _ => {
            return Err(invalid(
                "native facts require ELF or thin/universal Mach-O; archives and COFF are unsupported",
            ));
        }
    };
    let mut out = BoundedResponse {
        bytes: Vec::new(),
        limit,
    };
    write!(
        out,
        "{{\"schema_version\":{SCHEMA},\"ok\":true,\"facts\":{{\"size\":{},\"sha256\":\"{}\",\"slices\":[",
        bytes.len(),
        bytes_to_lower_hex(Sha256::digest(bytes).as_ref())
    )?;
    for (index, slice) in slices.iter().enumerate() {
        if index != 0 {
            out.write_all(b",")?;
        }
        scan_image(
            &mut out,
            &bytes[slice.offset as usize..(slice.offset + slice.size) as usize],
            slice.offset,
        )?;
    }
    out.write_all(b"]}}\n")?;
    Ok(out.bytes)
}

pub(crate) fn emit_cli(args: &[String]) -> io::Result<()> {
    let result = (|| {
        let [path] = args else {
            return Err(invalid(
                "--scan-native-artifact-facts requires exactly one path",
            ));
        };
        let file = crate::backend_process::open_regular_artifact(std::path::Path::new(path))?;
        let limit = crate::backend_process::stdin_request_limit_bytes();
        if file.metadata()?.len() > limit as u64 {
            return Err(invalid(
                "native facts input must be a regular file within request byte limit",
            ));
        }
        let bytes =
            crate::backend_process::read_bounded_request_bytes(file, limit, "native facts input")?;
        scan(&bytes, limit)
    })();
    match result {
        Ok(bytes) => io::stdout().lock().write_all(&bytes),
        Err(error) => {
            serde_json::to_writer(
                io::stdout().lock(),
                &serde_json::json!({"schema_version": SCHEMA, "ok": false, "error": error.to_string()}),
            )?;
            println!();
            std::process::exit(2);
        }
    }
}
