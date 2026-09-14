use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use ar_archive_writer::{ArchiveKind, DEFAULT_OBJECT_READER, NewArchiveMember};
use object::{Architecture, BinaryFormat, Endianness, Object, ObjectKind, SubArchitecture};

use molt_artifact_publish::write_atomically;

/// Package ordered ordinary objects without resolving relocations or changing
/// symbol inclusion. The final link must whole-load these compiler archives.
/// Member names and metadata depend only on the compilation plan, never on
/// temporary paths, filesystem timestamps, ownership, or host platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NativeArchiveTarget {
    format: BinaryFormat,
    architecture: Architecture,
    endianness: Endianness,
    sub_architecture: Option<SubArchitecture>,
}

impl NativeArchiveTarget {
    fn archive_kind(self) -> io::Result<ArchiveKind> {
        match self.format {
            BinaryFormat::Elf => Ok(ArchiveKind::Gnu),
            BinaryFormat::MachO => Ok(ArchiveKind::Darwin),
            BinaryFormat::Coff => Ok(ArchiveKind::Coff),
            format => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("unsupported native archive object format: {format:?}"),
            )),
        }
    }

    fn validate_member_count(self, count: usize) -> io::Result<()> {
        // Reject the known COFF index limit before writing large input sets.
        // Actual emitted-container validation also covers offset-driven promotion.
        if self.format == BinaryFormat::Coff && count > 0xfffe {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "COFF native archive exceeds 65534 member index limit",
            ));
        }
        Ok(())
    }
}

fn native_archive_target<'a>(
    members: impl IntoIterator<Item = io::Result<&'a [u8]>>,
) -> io::Result<NativeArchiveTarget> {
    let mut identity = None;
    let mut member_count = 0usize;
    for member in members {
        let bytes = member?;
        let object = object::File::parse(bytes).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid native archive member: {error}"),
            )
        })?;
        if object.kind() != ObjectKind::Relocatable {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "native archive member is not a relocatable object",
            ));
        }
        let current = NativeArchiveTarget {
            format: object.format(),
            architecture: object.architecture(),
            endianness: object.endianness(),
            sub_architecture: object.sub_architecture(),
        };
        if identity.is_some_and(|expected| expected != current) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "native archive members have incompatible format, architecture, endianness, or sub-architecture",
            ));
        }
        identity = Some(current);
        member_count += 1;
    }
    let target = identity.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "native archive requires at least one object",
        )
    })?;
    target.archive_kind()?;
    target.validate_member_count(member_count)?;
    Ok(target)
}

fn publish_native_archive(output: &Path, members: &[NewArchiveMember<'_>]) -> io::Result<()> {
    let target = native_archive_target(members.iter().map(|member| Ok((*member.buf).as_ref())))?;
    let kind = target.archive_kind()?;
    publish_validated_native_archive(output, target, |writer| {
        ar_archive_writer::write_archive_to_stream(writer, members, kind, false, None)
    })
}

fn publish_validated_native_archive(
    output: &Path,
    target: NativeArchiveTarget,
    write: impl FnOnce(&mut BufWriter<File>) -> io::Result<()>,
) -> io::Result<()> {
    write_atomically(output, |writer| {
        write(writer)?;
        writer.flush()?;
        // This private publication handle owns the completed bytes. Validate
        // actual transport, including upstream COFF -> GNU/GNU64 promotion,
        // before commit. The mapping drops before atomic replacement on Windows.
        let bytes = unsafe { memmap2::MmapOptions::new().map(writer.get_ref()) }?;
        let emitted = validate_native_archive_bytes(&bytes)?;
        if emitted != target {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "native archive emitted target {emitted:?} differs from input target {target:?}"
                ),
            ));
        }
        Ok(())
    })
}

/// Cache admission shares the producer's member identity contract. An old
/// merged object (even with matching opaque sidecars) is never an archive hit.
pub(crate) fn validate_native_archive_file(path: &Path) -> io::Result<()> {
    let file = std::fs::File::open(path)?;
    // Shared cache artifacts are atomically replaced, never mutated in place.
    // A mapping therefore owns a stable snapshot for this validation.
    let bytes = unsafe { memmap2::MmapOptions::new().map(&file) }?;
    validate_native_archive_bytes(&bytes).map(|_| ())
}

fn validate_native_archive_bytes(bytes: &[u8]) -> io::Result<NativeArchiveTarget> {
    let archive = object::read::archive::ArchiveFile::parse(bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if archive.is_thin() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "native compiler artifact cannot contain a thin archive",
        ));
    }
    let target = native_archive_target(archive.members().map(|member| {
        member
            .and_then(|member| member.data(bytes))
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }))?;
    use object::read::archive::ArchiveKind as ParsedKind;
    let matches = matches!(
        (target.archive_kind()?, archive.kind()),
        (ArchiveKind::Gnu, ParsedKind::Gnu | ParsedKind::Gnu64)
            | (ArchiveKind::Darwin, ParsedKind::Bsd | ParsedKind::Bsd64)
            | (ArchiveKind::Coff, ParsedKind::Coff)
    );
    if !matches {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "native archive container disagrees with its object target format",
        ));
    }
    Ok(target)
}

pub(crate) fn write_native_archive_bytes(output: &Path, bytes: &[u8]) -> io::Result<()> {
    publish_native_archive(
        output,
        &[NewArchiveMember::new(
            bytes,
            &DEFAULT_OBJECT_READER,
            "molt_00000000.o".to_string(),
        )],
    )
}

pub(crate) fn write_native_archive_objects(output: &Path, paths: &[PathBuf]) -> io::Result<()> {
    let members = paths
        .iter()
        .enumerate()
        .map(|(index, path)| {
            let file = std::fs::File::open(path)?;
            // These are completed, private batch-worker outputs. Their producer
            // has exited; no writer may mutate them until this publication and
            // the enclosing batch cleanup have finished. Map instead of copying
            // every member into a second archive-sized resident allocation.
            let bytes = unsafe { memmap2::MmapOptions::new().map(&file) }?;
            Ok(NewArchiveMember::new(
                bytes,
                &DEFAULT_OBJECT_READER,
                format!("molt_{index:08}.o"),
            ))
        })
        .collect::<io::Result<Vec<_>>>()?;
    publish_native_archive(output, &members)
}

#[cfg(test)]
mod tests {
    use super::*;
    use object::{ObjectSymbol, SectionKind, SymbolFlags, SymbolKind, SymbolScope};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NONCE: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "molt-native-archive-{}-{}",
                std::process::id(),
                NONCE.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).expect("create test directory");
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).expect("clean test directory");
        }
    }

    fn object_bytes(format: BinaryFormat, architecture: Architecture, symbol: &str) -> Vec<u8> {
        let mut object = object::write::Object::new(format, architecture, Endianness::Little);
        let section = object.add_section(Vec::new(), b".text".to_vec(), SectionKind::Text);
        object.append_section_data(section, &[0, 0, 0, 0], 4);
        object.add_symbol(object::write::Symbol {
            name: symbol.as_bytes().to_vec(),
            value: 0,
            size: 4,
            kind: SymbolKind::Text,
            scope: SymbolScope::Linkage,
            weak: false,
            section: object::write::SymbolSection::Section(section),
            flags: SymbolFlags::None,
        });
        object.write().expect("write native object fixture")
    }

    #[test]
    fn archives_preserve_target_members_and_are_host_path_independent() {
        let directory = TestDirectory::new();
        for format in [BinaryFormat::Elf, BinaryFormat::Coff, BinaryFormat::MachO] {
            for architecture in [Architecture::X86_64, Architecture::Aarch64] {
                let first = object_bytes(format, architecture, "first");
                let second = object_bytes(format, architecture, "second");
                let paths = [
                    directory.0.join("source-one.o"),
                    directory.0.join("source-two.o"),
                ];
                std::fs::write(&paths[0], &first).expect("write first object");
                std::fs::write(&paths[1], &second).expect("write second object");
                let output = directory.0.join("first.a");
                write_native_archive_objects(&output, &paths).expect("publish archive");
                validate_native_archive_file(&output)
                    .expect("published archive passes cache admission");
                let bytes = std::fs::read(&output).expect("read archive");
                let renamed = [
                    directory.0.join("unrelated-name.o"),
                    directory.0.join("another-name.o"),
                ];
                for (source, destination) in paths.iter().zip(&renamed) {
                    std::fs::rename(source, destination).expect("rename inputs");
                }
                write_native_archive_objects(&directory.0.join("second.a"), &renamed)
                    .expect("publish renamed inputs");
                assert_eq!(
                    bytes,
                    std::fs::read(directory.0.join("second.a")).expect("read replay")
                );
                let archive = object::read::archive::ArchiveFile::parse(bytes.as_slice())
                    .expect("parse archive");
                let members = archive
                    .members()
                    .collect::<Result<Vec<_>, _>>()
                    .expect("read archive members");
                assert_eq!(members.len(), 2);
                for (index, member) in members.iter().enumerate() {
                    assert_eq!(member.name(), format!("molt_{index:08}.o").as_bytes());
                    assert_eq!(member.date(), Some(0));
                    assert_eq!(member.uid(), Some(0));
                    assert_eq!(member.gid(), Some(0));
                    let data = member.data(bytes.as_slice()).expect("read member data");
                    let original = if index == 0 {
                        first.as_slice()
                    } else {
                        second.as_slice()
                    };
                    // Target mangling belongs to the object writer. Archive
                    // transport must preserve input bytes and physical symbols;
                    // Darwin may append member-alignment bytes only.
                    assert!(
                        data.starts_with(original),
                        "{format:?}/{architecture:?} member {index} changed input object bytes"
                    );
                    let input = object::File::parse(original).expect("parse original fixture");
                    let object = object::File::parse(data).expect("parse member object");
                    assert_eq!(object.format(), format);
                    assert_eq!(object.architecture(), architecture);
                    let definitions = |file: &object::File<'_>| {
                        file.symbols()
                            .filter(|symbol| symbol.is_definition())
                            .map(|symbol| {
                                symbol.name_bytes().expect("fixture symbol name").to_vec()
                            })
                            .collect::<Vec<_>>()
                    };
                    let expected = definitions(&input);
                    assert!(!expected.is_empty(), "fixture must contain a definition");
                    assert_eq!(
                        definitions(&object),
                        expected,
                        "{format:?}/{architecture:?} member {index} changed target symbol definitions"
                    );
                }
                write_native_archive_bytes(&output, &first).expect("publish one-member archive");
                validate_native_archive_file(&output)
                    .expect("single-member archive passes cache admission");
                let one = std::fs::read(&output).expect("read one-member archive");
                let archive = object::read::archive::ArchiveFile::parse(one.as_slice())
                    .expect("one member remains an archive");
                assert_eq!(archive.members().count(), 1);
            }
        }
    }

    #[test]
    fn invalid_archive_inputs_preserve_existing_output() {
        let directory = TestDirectory::new();
        let output = directory.0.join("existing.a");
        std::fs::write(&output, b"previous").expect("write existing artifact");
        assert!(write_native_archive_objects(&output, &[]).is_err());
        assert!(write_native_archive_bytes(&output, b"not an object").is_err());
        let paths = [directory.0.join("elf.o"), directory.0.join("coff.o")];
        std::fs::write(
            &paths[0],
            object_bytes(BinaryFormat::Elf, Architecture::X86_64, "first"),
        )
        .expect("write ELF");
        for (format, architecture) in [
            (BinaryFormat::Coff, Architecture::X86_64),
            (BinaryFormat::Elf, Architecture::Aarch64),
        ] {
            std::fs::write(&paths[1], object_bytes(format, architecture, "second"))
                .expect("write conflicting member");
            let error = write_native_archive_objects(&output, &paths)
                .expect_err("mixed target identity must fail");
            assert!(error.to_string().contains("incompatible"));
        }
        assert_eq!(
            std::fs::read(&output).expect("read preserved artifact"),
            b"previous"
        );
        assert_eq!(
            std::fs::read_dir(&directory.0)
                .expect("list directory")
                .count(),
            3,
            "no uncommitted temporary output remains"
        );
    }

    #[test]
    fn emitted_container_variants_are_admitted_before_atomic_publication() {
        let directory = TestDirectory::new();
        let output = directory.0.join("output.a");
        for (format, kind, accepted) in [
            (BinaryFormat::Elf, ArchiveKind::Gnu, true),
            (BinaryFormat::Elf, ArchiveKind::Gnu64, true),
            (BinaryFormat::MachO, ArchiveKind::Bsd, true),
            (BinaryFormat::MachO, ArchiveKind::Darwin, true),
            (BinaryFormat::MachO, ArchiveKind::Darwin64, true),
            (BinaryFormat::Coff, ArchiveKind::Coff, true),
            (BinaryFormat::Coff, ArchiveKind::Gnu, false),
            // Produce the exact container of the upstream >4GiB offset fallback
            // using one small member, not a multi-gigabyte fixture allocation.
            (BinaryFormat::Coff, ArchiveKind::Gnu64, false),
            (BinaryFormat::Coff, ArchiveKind::Bsd, false),
            (BinaryFormat::Elf, ArchiveKind::Coff, false),
            (BinaryFormat::Elf, ArchiveKind::Darwin, false),
            (BinaryFormat::MachO, ArchiveKind::Gnu, false),
        ] {
            let bytes = object_bytes(format, Architecture::X86_64, "member");
            let target = native_archive_target([Ok(bytes.as_slice())]).expect("fixture target");
            let members = [NewArchiveMember::new(
                bytes.as_slice(),
                &DEFAULT_OBJECT_READER,
                "molt_00000000.o".to_string(),
            )];
            std::fs::write(&output, b"previous").expect("seed previous generation");
            let result = publish_validated_native_archive(&output, target, |writer| {
                ar_archive_writer::write_archive_to_stream(writer, &members, kind, false, None)
            });
            assert_eq!(result.is_ok(), accepted, "{format:?}/{kind:?}: {result:?}");
            if accepted {
                validate_native_archive_file(&output).expect("cache shares emitted admission");
                assert!(std::fs::metadata(&output).expect("artifact metadata").len() < 4096);
            } else {
                let error = result.expect_err("wrong target container must fail");
                assert!(error.to_string().contains("container disagrees"));
                assert_eq!(
                    std::fs::read(&output).expect("preserved generation"),
                    b"previous"
                );
            }
            assert_eq!(
                std::fs::read_dir(&directory.0)
                    .expect("list publication root")
                    .count(),
                1,
                "validation failure cannot strand its private temporary"
            );
        }
    }

    #[test]
    fn emitted_target_and_thin_transport_substitutions_preserve_previous_generation() {
        let directory = TestDirectory::new();
        let output = directory.0.join("output.a");
        let original = object_bytes(BinaryFormat::Elf, Architecture::X86_64, "original");
        let target = native_archive_target([Ok(original.as_slice())]).expect("original target");
        for (architecture, thin, expected_error) in [
            (Architecture::Aarch64, false, "differs from input target"),
            (Architecture::X86_64, true, "thin archive"),
        ] {
            let bytes = object_bytes(BinaryFormat::Elf, architecture, "substitute");
            let members = [NewArchiveMember::new(
                bytes.as_slice(),
                &DEFAULT_OBJECT_READER,
                "molt_00000000.o".to_string(),
            )];
            std::fs::write(&output, b"previous").expect("seed previous generation");
            let error = publish_validated_native_archive(&output, target, |writer| {
                ar_archive_writer::write_archive_to_stream(
                    writer,
                    &members,
                    ArchiveKind::Gnu,
                    thin,
                    None,
                )
            })
            .expect_err("emitted target substitution must fail before commit");
            assert!(error.to_string().contains(expected_error), "{error}");
            assert_eq!(
                std::fs::read(&output).expect("preserved generation"),
                b"previous"
            );
        }
        let error = publish_validated_native_archive(&output, target, |writer| {
            writer.write_all(b"not an archive")
        })
        .expect_err("malformed emitted bytes must fail before commit");
        assert!(
            error.to_string().contains("destination unchanged"),
            "{error}"
        );
        assert_eq!(
            std::fs::read(&output).expect("preserved generation"),
            b"previous"
        );
        assert_eq!(
            std::fs::read_dir(&directory.0)
                .expect("list publication root")
                .count(),
            1
        );
    }

    #[test]
    fn coff_member_count_admission_is_exact_without_large_allocations() {
        let bytes = object_bytes(BinaryFormat::Coff, Architecture::X86_64, "member");
        let target = native_archive_target([Ok(bytes.as_slice())]).expect("COFF target");
        assert!(target.validate_member_count(0xfffe).is_ok());
        for count in [0xffff, usize::MAX] {
            let error = target
                .validate_member_count(count)
                .expect_err("COFF index overflow");
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert!(error.to_string().contains("65534"));
        }
    }
}
