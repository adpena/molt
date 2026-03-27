// === FILE: runtime/molt-runtime/src/builtins/tarfile.rs ===
//! `tarfile` module intrinsics for Molt.
//!
//! Implements tar format parsing (POSIX ustar and GNU extensions) with
//! optional gzip and bzip2 decompression backed by `flate2` and `bzip2`.
//!
//! Modes:
//!   "r"     — raw tar (no compression)
//!   "r:gz"  — gzip-compressed tar
//!   "r:bz2" — bzip2-compressed tar
//!   "w"     — write raw tar (new archive)
//!   "w:gz"  — write gzip-compressed tar
//!   "w:bz2" — write bzip2-compressed tar
//!   "a"     — append to existing tar
//!
//! TarInfo tuple returned from getmembers:
//!   (name, size, mtime, mode, typeflag, linkname, uid, gid, uname, gname)
//!
//! ABI: NaN-boxed u64 in/out.

use crate::bridge::*;
use molt_runtime_core::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicI64, Ordering};

// ── Tar block constants ────────────────────────────────────────────────────

const BLOCK_SIZE: usize = 512;

// Typeflag values (POSIX ustar).
const REGTYPE: u8 = b'0';
const AREGTYPE: u8 = b'\0'; // Old-style regular file.
const LNKTYPE: u8 = b'1';
const SYMTYPE: u8 = b'2';
const CHRTYPE: u8 = b'3';
const BLKTYPE: u8 = b'4';
const DIRTYPE: u8 = b'5';
const FIFOTYPE: u8 = b'6';
const CONTTYPE: u8 = b'7';
// GNU extension types.
const GNU_LONGNAME: u8 = b'L';
const GNU_LONGLINK: u8 = b'K';
// PAX extension.
const PAX_HEADER: u8 = b'x';
const PAX_GLOBAL: u8 = b'g';

// ── TarInfo record ────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct TarInfo {
    name: String,
    size: u64,
    mtime: u64,
    mode: u32,
    typeflag: u8,
    linkname: String,
    uid: u32,
    gid: u32,
    uname: String,
    gname: String,
    /// Byte offset within the *decompressed* tar stream where the data starts.
    data_offset: usize,
}

impl TarInfo {
    fn is_regular(&self) -> bool {
        self.typeflag == REGTYPE || self.typeflag == AREGTYPE || self.typeflag == CONTTYPE
    }
}

// ── Handle-id counter ─────────────────────────────────────────────────────

static NEXT_TAR_ID: AtomicI64 = AtomicI64::new(1);

fn next_tar_id() -> i64 {
    NEXT_TAR_ID.fetch_add(1, Ordering::Relaxed)
}

// ── Archive state ─────────────────────────────────────────────────────────

enum ArchiveMode {
    Read,
    Write,
    Append,
}

struct TarArchive {
    /// Full decompressed bytes for read mode; accumulated bytes for write mode.
    data: Vec<u8>,
    members: Vec<TarInfo>,
    mode: ArchiveMode,
    name: String,
    compression: TarCompression,
}

#[derive(Clone, Copy, PartialEq)]
enum TarCompression {
    None,
    Gzip,
    Bzip2,
}

// ── Thread-local handle map ────────────────────────────────────────────────

thread_local! {
    static TAR_MAP: RefCell<HashMap<i64, TarArchive>> = RefCell::new(HashMap::new());
}

// ── Parsing helpers ────────────────────────────────────────────────────────

/// Read a NUL-terminated octal ASCII field from a tar header slice.
fn parse_octal(field: &[u8]) -> u64 {
    let s = field.split(|&b| b == 0 || b == b' ').next().unwrap_or(&[]);
    u64::from_str_radix(std::str::from_utf8(s).unwrap_or("0").trim(), 8).unwrap_or(0)
}

/// Read a NUL-terminated string from a tar header field.
fn parse_str(field: &[u8]) -> String {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).to_string()
}

/// Check whether a 512-byte block is an all-zero end-of-archive block.
fn is_eof_block(block: &[u8]) -> bool {
    block.iter().all(|&b| b == 0)
}

/// Parse all member headers from a flat decompressed tar byte buffer.
fn parse_tar_members(data: &[u8]) -> Vec<TarInfo> {
    let mut members = Vec::new();
    let mut pos = 0usize;
    let mut pending_longname: Option<String> = None;
    let mut pending_longlink: Option<String> = None;

    loop {
        if pos + BLOCK_SIZE > data.len() {
            break;
        }
        let block = &data[pos..pos + BLOCK_SIZE];
        if is_eof_block(block) {
            // Check for second EOF block.
            if pos + 2 * BLOCK_SIZE <= data.len()
                && is_eof_block(&data[pos + BLOCK_SIZE..pos + 2 * BLOCK_SIZE])
            {
                break;
            }
            // Single EOF block is also valid.
            break;
        }

        // Parse ustar header fields.
        let raw_name = parse_str(&block[0..100]);
        let mode = parse_octal(&block[100..108]) as u32;
        let uid = parse_octal(&block[108..116]) as u32;
        let gid = parse_octal(&block[116..124]) as u32;
        let size = parse_octal(&block[124..136]);
        let mtime = parse_octal(&block[136..148]);
        // block[148..156] = checksum — not validated here.
        let typeflag = block[156];
        let linkname = parse_str(&block[157..257]);
        let magic = &block[257..263];

        // ustar prefix field (bytes 345-499) for long names.
        let name = if magic == b"ustar\0" || magic == b"ustar " {
            let prefix = parse_str(&block[345..500]);
            if prefix.is_empty() {
                raw_name.clone()
            } else {
                format!("{prefix}/{raw_name}")
            }
        } else {
            raw_name.clone()
        };

        let uname = parse_str(&block[265..297]);
        let gname = parse_str(&block[297..329]);

        pos += BLOCK_SIZE;

        // Data blocks follow (rounded up to BLOCK_SIZE).
        let data_offset = pos;
        let data_blocks = size.div_ceil(BLOCK_SIZE as u64) as usize;
        let data_end = pos + data_blocks * BLOCK_SIZE;

        match typeflag {
            GNU_LONGNAME => {
                // The data is a long filename.
                if pos + size as usize <= data.len() {
                    let name_bytes = &data[pos..pos + size as usize];
                    pending_longname = Some(parse_str(name_bytes));
                }
                pos = data_end;
                continue;
            }
            GNU_LONGLINK => {
                if pos + size as usize <= data.len() {
                    let link_bytes = &data[pos..pos + size as usize];
                    pending_longlink = Some(parse_str(link_bytes));
                }
                pos = data_end;
                continue;
            }
            PAX_HEADER | PAX_GLOBAL => {
                // Skip PAX extended header blocks — we parse the basic fields.
                pos = data_end;
                continue;
            }
            _ => {}
        }

        let final_name = pending_longname.take().unwrap_or(name);
        let final_link = pending_longlink.take().unwrap_or(linkname);

        members.push(TarInfo {
            name: final_name,
            size,
            mtime,
            mode,
            typeflag,
            linkname: final_link,
            uid,
            gid,
            uname,
            gname,
            data_offset,
        });

        pos = data_end;
    }

    members
}

// ── Decompression ─────────────────────────────────────────────────────────

fn decompress_gzip(data: &[u8]) -> Result<Vec<u8>, String> {
    use flate2::read::GzDecoder;
    let mut decoder = GzDecoder::new(data);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

fn decompress_bzip2(data: &[u8]) -> Result<Vec<u8>, String> {
    use bzip2::read::BzDecoder;
    let mut decoder = BzDecoder::new(data);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

fn compress_gzip(data: &[u8]) -> Result<Vec<u8>, String> {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    enc.write_all(data).map_err(|e| e.to_string())?;
    enc.finish().map_err(|e| e.to_string())
}

fn compress_bzip2(data: &[u8]) -> Result<Vec<u8>, String> {
    use bzip2::write::BzEncoder;
    use bzip2::Compression;
    let mut enc = BzEncoder::new(Vec::new(), Compression::default());
    enc.write_all(data).map_err(|e| e.to_string())?;
    enc.finish().map_err(|e| e.to_string())
}

// ── Tar write helpers ─────────────────────────────────────────────────────

fn write_tar_header(out: &mut Vec<u8>, info: &TarInfo, data: &[u8]) {
    let mut block = [0u8; BLOCK_SIZE];

    // name (100 bytes)
    let name_bytes = info.name.as_bytes();
    let copy_len = name_bytes.len().min(99);
    block[..copy_len].copy_from_slice(&name_bytes[..copy_len]);

    // mode (8 bytes octal)
    let mode_str = format!("{:07o}\0", info.mode);
    block[100..108].copy_from_slice(mode_str.as_bytes());
    // uid
    let uid_str = format!("{:07o}\0", info.uid);
    block[108..116].copy_from_slice(uid_str.as_bytes());
    // gid
    let gid_str = format!("{:07o}\0", info.gid);
    block[116..124].copy_from_slice(gid_str.as_bytes());
    // size
    let size_str = format!("{:011o}\0", data.len());
    block[124..136].copy_from_slice(size_str.as_bytes());
    // mtime
    let mtime_str = format!("{:011o}\0", info.mtime);
    block[136..148].copy_from_slice(mtime_str.as_bytes());
    // typeflag
    block[156] = if info.typeflag == DIRTYPE {
        DIRTYPE
    } else {
        REGTYPE
    };
    // ustar magic
    block[257..263].copy_from_slice(b"ustar\0");
    block[263..265].copy_from_slice(b"00");
    // uname
    let un = info.uname.as_bytes();
    block[265..265 + un.len().min(32)].copy_from_slice(&un[..un.len().min(32)]);
    // gname
    let gn = info.gname.as_bytes();
    block[297..297 + gn.len().min(32)].copy_from_slice(&gn[..gn.len().min(32)]);

    // checksum (simple sum).
    // Fill checksum field with spaces first.
    block[148..156].fill(b' ');
    let checksum: u32 = block.iter().map(|&b| b as u32).sum();
    let chk_str = format!("{:06o}\0 ", checksum);
    block[148..156].copy_from_slice(chk_str.as_bytes());

    out.extend_from_slice(&block);

    // Data blocks.
    out.extend_from_slice(data);
    // Padding.
    let pad = (BLOCK_SIZE - data.len() % BLOCK_SIZE) % BLOCK_SIZE;
    out.extend(std::iter::repeat_n(0u8, pad));
}

fn write_eof_blocks(out: &mut Vec<u8>) {
    out.extend_from_slice(&[0u8; BLOCK_SIZE * 2]);
}

// ── Helpers for extracting string path from bits ──────────────────────────

fn path_from_bits_local(_py: &PyToken, bits: u64) -> Option<String> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return None;
    }
    string_obj_to_owned(obj)
}

fn return_none() -> u64 {
    MoltObject::none().bits()
}

// ── Public intrinsics ─────────────────────────────────────────────────────
pub extern "C" fn molt_tarfile_open(name_bits: u64, mode_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let name = path_from_bits_local(_py, name_bits).unwrap_or_default();
        let mode_str =
            string_obj_to_owned(obj_from_bits(mode_bits)).unwrap_or_else(|| "r".to_string());

        let (archive_mode, compression) = match mode_str.as_str() {
            "r" | "r:" => (ArchiveMode::Read, TarCompression::None),
            "r:gz" => (ArchiveMode::Read, TarCompression::Gzip),
            "r:bz2" => (ArchiveMode::Read, TarCompression::Bzip2),
            "w" | "w:" => (ArchiveMode::Write, TarCompression::None),
            "w:gz" => (ArchiveMode::Write, TarCompression::Gzip),
            "w:bz2" => (ArchiveMode::Write, TarCompression::Bzip2),
            "a" => (ArchiveMode::Append, TarCompression::None),
            other => {
                return raise_exception(
                    _py,
                    "ValueError",
                    &format!("unsupported tarfile mode: '{other}'"),
                );
            }
        };

        let raw_data: Vec<u8> = match archive_mode {
            ArchiveMode::Read | ArchiveMode::Append => match std::fs::read(&name) {
                Ok(d) => d,
                Err(e) => {
                    return raise_exception(
                        _py,
                        "FileNotFoundError",
                        &format!("cannot open '{name}': {e}"),
                    );
                }
            },
            ArchiveMode::Write => Vec::new(),
        };

        let decompressed = match compression {
            TarCompression::None => raw_data,
            TarCompression::Gzip => match decompress_gzip(&raw_data) {
                Ok(d) => d,
                Err(e) => {
                    return raise_exception(
                        _py,
                        "tarfile.ReadError",
                        &format!("gzip decompression failed: {e}"),
                    );
                }
            },
            TarCompression::Bzip2 => match decompress_bzip2(&raw_data) {
                Ok(d) => d,
                Err(e) => {
                    return raise_exception(
                        _py,
                        "tarfile.ReadError",
                        &format!("bzip2 decompression failed: {e}"),
                    );
                }
            },
        };

        let members = match archive_mode {
            ArchiveMode::Read | ArchiveMode::Append => parse_tar_members(&decompressed),
            ArchiveMode::Write => Vec::new(),
        };

        let id = next_tar_id();
        TAR_MAP.with(|m| {
            m.borrow_mut().insert(
                id,
                TarArchive {
                    data: decompressed,
                    members,
                    mode: archive_mode,
                    name: name.clone(),
                    compression,
                },
            )
        });
        int_bits_from_i64(_py, id)
    })
}
pub extern "C" fn molt_tarfile_getnames(handle_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => return raise_exception(_py, "TypeError", "tarfile handle must be int"),
        };
        let names: Option<Vec<String>> = TAR_MAP.with(|m| {
            m.borrow()
                .get(&id)
                .map(|a| a.members.iter().map(|m| m.name.clone()).collect())
        });
        match names {
            None => raise_exception(_py, "ValueError", "invalid tarfile handle"),
            Some(ns) => {
                let mut bits = Vec::with_capacity(ns.len());
                for n in &ns {
                    let ptr = alloc_string(_py, n.as_bytes());
                    if ptr.is_null() {
                        return raise_exception(_py, "MemoryError", "out of memory");
                    }
                    bits.push(MoltObject::from_ptr(ptr).bits());
                }
                let list_ptr = alloc_list(_py, &bits);
                if list_ptr.is_null() {
                    return raise_exception(_py, "MemoryError", "out of memory");
                }
                MoltObject::from_ptr(list_ptr).bits()
            }
        }
    })
}

/// Build a TarInfo tuple: (name, size, mtime, mode, typeflag_str, linkname, uid, gid, uname, gname)
fn tarinfo_to_tuple(_py: &PyToken, info: &TarInfo) -> u64 {
    let make_str = |s: &str| -> u64 {
        let ptr = alloc_string(_py, s.as_bytes());
        if ptr.is_null() {
            0
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    };
    let typeflag_str = match info.typeflag {
        REGTYPE | AREGTYPE | CONTTYPE => "f",
        LNKTYPE => "h",
        SYMTYPE => "s",
        DIRTYPE => "d",
        CHRTYPE => "c",
        BLKTYPE => "b",
        FIFOTYPE => "p",
        _ => "?",
    };
    let name_bits = make_str(&info.name);
    let size_bits = int_bits_from_i64(_py, info.size as i64);
    let mtime_bits = int_bits_from_i64(_py, info.mtime as i64);
    let mode_bits = int_bits_from_i64(_py, info.mode as i64);
    let type_bits = make_str(typeflag_str);
    let link_bits = make_str(&info.linkname);
    let uid_bits = int_bits_from_i64(_py, info.uid as i64);
    let gid_bits = int_bits_from_i64(_py, info.gid as i64);
    let uname_bits = make_str(&info.uname);
    let gname_bits = make_str(&info.gname);

    if [name_bits, type_bits, link_bits, uname_bits, gname_bits].contains(&0) {
        return raise_exception(_py, "MemoryError", "out of memory");
    }

    let tuple_ptr = alloc_tuple(
        _py,
        &[
            name_bits, size_bits, mtime_bits, mode_bits, type_bits, link_bits, uid_bits, gid_bits,
            uname_bits, gname_bits,
        ],
    );
    if tuple_ptr.is_null() {
        return raise_exception(_py, "MemoryError", "out of memory");
    }
    MoltObject::from_ptr(tuple_ptr).bits()
}
pub extern "C" fn molt_tarfile_getmembers(handle_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => return raise_exception(_py, "TypeError", "tarfile handle must be int"),
        };
        let members_clone: Option<Vec<TarInfo>> =
            TAR_MAP.with(|m| m.borrow().get(&id).map(|a| a.members.clone()));
        match members_clone {
            None => raise_exception(_py, "ValueError", "invalid tarfile handle"),
            Some(members) => {
                let mut tuple_bits = Vec::with_capacity(members.len());
                for info in &members {
                    let tb = tarinfo_to_tuple(_py, info);
                    if exception_pending(_py) {
                        return tb;
                    }
                    tuple_bits.push(tb);
                }
                let list_ptr = alloc_list(_py, &tuple_bits);
                if list_ptr.is_null() {
                    return raise_exception(_py, "MemoryError", "out of memory");
                }
                MoltObject::from_ptr(list_ptr).bits()
            }
        }
    })
}
pub extern "C" fn molt_tarfile_extractall(handle_bits: u64, path_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => return raise_exception(_py, "TypeError", "tarfile handle must be int"),
        };
        let dest = path_from_bits_local(_py, path_bits).unwrap_or_else(|| ".".to_string());

        let archive_data: Option<(Vec<TarInfo>, Vec<u8>)> = TAR_MAP.with(|m| {
            m.borrow()
                .get(&id)
                .map(|a| (a.members.clone(), a.data.clone()))
        });
        let (members, data) = match archive_data {
            Some(v) => v,
            None => return raise_exception(_py, "ValueError", "invalid tarfile handle"),
        };

        for info in &members {
            // Guard against path traversal.
            if info.name.contains("..") || info.name.starts_with('/') {
                continue;
            }
            let out_path = format!("{dest}/{}", info.name);
            if info.typeflag == DIRTYPE || info.name.ends_with('/') {
                if let Err(e) = std::fs::create_dir_all(&out_path) {
                    return raise_exception(
                        _py,
                        "OSError",
                        &format!("cannot create directory '{out_path}': {e}"),
                    );
                }
            } else if info.is_regular() {
                // Ensure parent directories exist.
                if let Some(parent) = std::path::Path::new(&out_path).parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let end = info.data_offset + info.size as usize;
                if end > data.len() {
                    return raise_exception(
                        _py,
                        "tarfile.ReadError",
                        &format!("member '{0}' data out of bounds", info.name),
                    );
                }
                let content = &data[info.data_offset..end];
                if let Err(e) = std::fs::write(&out_path, content) {
                    return raise_exception(
                        _py,
                        "OSError",
                        &format!("cannot write '{out_path}': {e}"),
                    );
                }
            }
        }
        return_none()
    })
}
pub extern "C" fn molt_tarfile_extract(handle_bits: u64, member_bits: u64, path_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => return raise_exception(_py, "TypeError", "tarfile handle must be int"),
        };
        let dest = path_from_bits_local(_py, path_bits).unwrap_or_else(|| ".".to_string());
        let member_name = match string_obj_to_owned(obj_from_bits(member_bits)) {
            Some(n) => n,
            None => return raise_exception(_py, "TypeError", "member must be a str"),
        };

        let archive_data: Option<(Vec<TarInfo>, Vec<u8>)> = TAR_MAP.with(|m| {
            m.borrow()
                .get(&id)
                .map(|a| (a.members.clone(), a.data.clone()))
        });
        let (members, data) = match archive_data {
            Some(v) => v,
            None => return raise_exception(_py, "ValueError", "invalid tarfile handle"),
        };

        let info = match members.iter().find(|m| m.name == member_name) {
            Some(m) => m.clone(),
            None => {
                return raise_exception(
                    _py,
                    "KeyError",
                    &format!("member '{member_name}' not found"),
                );
            }
        };

        if info.name.contains("..") || info.name.starts_with('/') {
            return raise_exception(
                _py,
                "tarfile.ExtractError",
                "refusing to extract member with unsafe path",
            );
        }

        let out_path = format!("{dest}/{}", info.name);
        if info.typeflag == DIRTYPE {
            if let Err(e) = std::fs::create_dir_all(&out_path) {
                return raise_exception(
                    _py,
                    "OSError",
                    &format!("cannot create directory '{out_path}': {e}"),
                );
            }
        } else if info.is_regular() {
            if let Some(parent) = std::path::Path::new(&out_path).parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let end = info.data_offset + info.size as usize;
            if end > data.len() {
                return raise_exception(
                    _py,
                    "tarfile.ReadError",
                    &format!("member '{member_name}' data out of bounds"),
                );
            }
            if let Err(e) = std::fs::write(&out_path, &data[info.data_offset..end]) {
                return raise_exception(_py, "OSError", &format!("cannot write '{out_path}': {e}"));
            }
        }
        return_none()
    })
}
pub extern "C" fn molt_tarfile_extractfile(handle_bits: u64, member_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => return raise_exception(_py, "TypeError", "tarfile handle must be int"),
        };
        let member_name = match string_obj_to_owned(obj_from_bits(member_bits)) {
            Some(n) => n,
            None => return raise_exception(_py, "TypeError", "member must be a str"),
        };

        let result: Option<Result<Vec<u8>, String>> = TAR_MAP.with(|m| {
            let map = m.borrow();
            let archive = map.get(&id)?;
            let info = archive.members.iter().find(|m| m.name == member_name)?;
            if !info.is_regular() {
                return Some(Err(format!("'{member_name}' is not a regular file")));
            }
            let end = info.data_offset + info.size as usize;
            if end > archive.data.len() {
                return Some(Err(format!("'{member_name}' data out of bounds")));
            }
            Some(Ok(archive.data[info.data_offset..end].to_vec()))
        });

        match result {
            None => raise_exception(
                _py,
                "KeyError",
                &format!("member '{member_name}' not found"),
            ),
            Some(Err(msg)) => raise_exception(_py, "tarfile.ReadError", &msg),
            Some(Ok(data)) => {
                let ptr = alloc_bytes(_py, &data);
                if ptr.is_null() {
                    raise_exception(_py, "MemoryError", "out of memory")
                } else {
                    MoltObject::from_ptr(ptr).bits()
                }
            }
        }
    })
}
pub extern "C" fn molt_tarfile_add(handle_bits: u64, name_bits: u64, arcname_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => return raise_exception(_py, "TypeError", "tarfile handle must be int"),
        };
        let fs_name = match path_from_bits_local(_py, name_bits) {
            Some(n) => n,
            None => return raise_exception(_py, "TypeError", "name must be a str"),
        };
        let arc_name = path_from_bits_local(_py, arcname_bits).unwrap_or_else(|| fs_name.clone());

        let file_data = match std::fs::read(&fs_name) {
            Ok(d) => d,
            Err(e) => {
                return raise_exception(_py, "OSError", &format!("cannot read '{fs_name}': {e}"));
            }
        };

        let meta = std::fs::metadata(&fs_name).ok();
        let mtime = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let mode = {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                meta.as_ref().map(|m| m.mode()).unwrap_or(0o644) as u32
            }
            #[cfg(not(unix))]
            {
                0o644u32
            }
        };

        let info = TarInfo {
            name: arc_name.clone(),
            size: file_data.len() as u64,
            mtime,
            mode,
            typeflag: REGTYPE,
            linkname: String::new(),
            uid: 0,
            gid: 0,
            uname: String::new(),
            gname: String::new(),
            data_offset: 0, // will be set when writing
        };

        TAR_MAP.with(|m| {
            let mut map = m.borrow_mut();
            if let Some(archive) = map.get_mut(&id) {
                write_tar_header(&mut archive.data, &info, &file_data);
                // Update data_offset in a new TarInfo entry for reading back.
                let data_offset = archive.data.len()
                    - file_data.len()
                    - (BLOCK_SIZE - file_data.len() % BLOCK_SIZE) % BLOCK_SIZE;
                let mut info2 = info;
                info2.data_offset = data_offset + BLOCK_SIZE; // header block consumed
                archive.members.push(info2);
            }
        });

        return_none()
    })
}
pub extern "C" fn molt_tarfile_close(handle_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => return raise_exception(_py, "TypeError", "tarfile handle must be int"),
        };

        let archive = TAR_MAP.with(|m| m.borrow_mut().remove(&id));
        if let Some(mut archive) = archive {
            match archive.mode {
                ArchiveMode::Write | ArchiveMode::Append => {
                    write_eof_blocks(&mut archive.data);
                    let final_data = match archive.compression {
                        TarCompression::None => archive.data,
                        TarCompression::Gzip => match compress_gzip(&archive.data) {
                            Ok(d) => d,
                            Err(e) => {
                                return raise_exception(
                                    _py,
                                    "OSError",
                                    &format!("gzip compression failed: {e}"),
                                );
                            }
                        },
                        TarCompression::Bzip2 => match compress_bzip2(&archive.data) {
                            Ok(d) => d,
                            Err(e) => {
                                return raise_exception(
                                    _py,
                                    "OSError",
                                    &format!("bzip2 compression failed: {e}"),
                                );
                            }
                        },
                    };
                    if !archive.name.is_empty() {
                        if let Err(e) = std::fs::write(&archive.name, &final_data) {
                            return raise_exception(
                                _py,
                                "OSError",
                                &format!("cannot write '{0}': {e}", archive.name),
                            );
                        }
                    }
                }
                ArchiveMode::Read => {} // nothing to flush
            }
        }
        return_none()
    })
}
pub extern "C" fn molt_tarfile_drop(handle_bits: u64) -> u64 {
    molt_tarfile_close(handle_bits)
}
pub extern "C" fn molt_tarfile_is_tarfile(name_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let path = match string_obj_to_owned(obj_from_bits(name_bits)) {
            Some(p) => p,
            None => return raise_exception(_py, "TypeError", "path must be a str"),
        };
        let is_tar = std::fs::read(&path)
            .map(|data| {
                // Check plain tar or gzip-wrapped tar.
                if data.len() >= 262 {
                    // POSIX ustar magic at offset 257.
                    let magic = &data[257..263];
                    if magic == b"ustar\0" || magic == b"ustar " {
                        return true;
                    }
                }
                // Gzip magic.
                if data.len() >= 2 && data[0] == 0x1f && data[1] == 0x8b {
                    // Decompress first block and check.
                    if let Ok(inner) = decompress_gzip(&data) {
                        if inner.len() >= 262 {
                            let magic = &inner[257..263];
                            return magic == b"ustar\0" || magic == b"ustar ";
                        }
                        // Any valid gzip with content ≥512 is likely a tar.gz.
                        return inner.len() >= BLOCK_SIZE;
                    }
                }
                // Bzip2 magic.
                if data.len() >= 3 && &data[0..3] == b"BZh" {
                    if let Ok(inner) = decompress_bzip2(&data) {
                        if inner.len() >= 262 {
                            let magic = &inner[257..263];
                            return magic == b"ustar\0" || magic == b"ustar ";
                        }
                        return inner.len() >= BLOCK_SIZE;
                    }
                }
                // Raw tar: check that the first block looks like a header
                // (non-null name and valid checksum structure).
                if data.len() >= BLOCK_SIZE {
                    return data[0] != 0;
                }
                false
            })
            .unwrap_or(false);
        MoltObject::from_bool(is_tar).bits()
    })
}
