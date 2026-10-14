use object::read::{Object, ObjectSection};
use object::{read, write};
use object::{
    Architecture, BinaryFormat, Endianness, RelocationEncoding, RelocationFlags, RelocationKind,
    SymbolFlags, SymbolKind, SymbolScope,
};

#[test]
fn reloc_overflow() {
    let mut object =
        write::Object::new(BinaryFormat::Coff, Architecture::X86_64, Endianness::Little);
    let text = object.section_id(write::StandardSection::Text);
    object.append_section_data(text, &[0; 4], 4);
    let symbol = object.add_symbol(write::Symbol {
        name: b"f".to_vec(),
        value: 0,
        size: 4,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: write::SymbolSection::Section(text),
        flags: SymbolFlags::None,
    });
    for i in 0..0x10000 {
        object
            .add_relocation(
                text,
                write::Relocation {
                    offset: i,
                    symbol,
                    addend: 0,
                    flags: RelocationFlags::Generic {
                        kind: RelocationKind::Absolute,
                        encoding: RelocationEncoding::Generic,
                        size: 64,
                    },
                },
            )
            .unwrap();
    }
    let bytes = object.write().unwrap();

    //std::fs::write(&"reloc_overflow.o", &bytes).unwrap();

    let object = read::File::parse(&*bytes).unwrap();
    assert_eq!(object.format(), BinaryFormat::Coff);
    assert_eq!(object.architecture(), Architecture::X86_64);

    let section = object.sections().next().unwrap();
    assert_eq!(section.name(), Ok(".text"));

    let mut i = 0;
    for (offset, _relocation) in section.relocations() {
        assert_eq!(offset, i);
        i += 1;
    }
    assert_eq!(i, 0x10000);
}

fn arm64_page_fixture() -> (write::Object<'static>, write::SectionId, write::SymbolId) {
    let mut object = write::Object::new(
        BinaryFormat::Coff,
        Architecture::Aarch64,
        Endianness::Little,
    );
    let text = object.section_id(write::StandardSection::Text);
    // adrp x0, target; add x0, x0, :lo12:target
    object.append_section_data(text, &[0x00, 0x00, 0x00, 0x90, 0x00, 0x00, 0x00, 0x91], 4);
    let target = object.add_symbol(write::Symbol {
        name: b"target".to_vec(),
        value: 0,
        size: 0,
        kind: SymbolKind::Data,
        scope: SymbolScope::Linkage,
        weak: false,
        section: write::SymbolSection::Undefined,
        flags: SymbolFlags::None,
    });
    (object, text, target)
}

#[test]
fn arm64_coff_page_addends_preserve_instruction_fields() {
    // The 0x12345 vector is LLVM's MC/AArch64/coff-relocations.s oracle:
    // b0091a20 (ADRP), 910d1400 (ADD). ADRP's encoded offset is in BYTES;
    // PAGEBASE_REL21 adds it to the symbol before page rounding at link time.
    for (addend, adrp, add) in [
        (0, 0x90000000_u32, 0x91000000_u32),
        (1, 0xb0000000, 0x91000400),
        (4095, 0xf0007fe0, 0x913ffc00),
        (4096, 0x90008000, 0x91000000),
        (4097, 0xb0008000, 0x91000400),
        (-1, 0xf0ffffe0, 0x913ffc00),
        (-4096, 0x90ff8000, 0x91000000),
        (-4097, 0xf0ff7fe0, 0x913ffc00),
        (0x12345, 0xb0091a20, 0x910d1400),
        (-1048576, 0x90800000, 0x91000000),
        (1048575, 0xf07fffe0, 0x913ffc00),
    ] {
        for register in [0_u32, 9, 30] {
            let (mut object, text, target) = arm64_page_fixture();
            let data = object.section_mut(text).data_mut();
            data[0..4].copy_from_slice(&(0x90000000 | register).to_le_bytes());
            data[4..8].copy_from_slice(&(0x91000000 | register | (register << 5)).to_le_bytes());
            for (offset, typ) in [
                (0, object::pe::IMAGE_REL_ARM64_PAGEBASE_REL21),
                (4, object::pe::IMAGE_REL_ARM64_PAGEOFFSET_12A),
            ] {
                object
                    .add_relocation(
                        text,
                        write::Relocation {
                            offset,
                            symbol: target,
                            addend,
                            flags: RelocationFlags::Coff { typ },
                        },
                    )
                    .unwrap();
            }
            let bytes = object.write().unwrap();
            let parsed = read::File::parse(bytes.as_slice()).unwrap();
            let section = parsed.section_by_name(".text").unwrap();
            let mut expected = (adrp | register).to_le_bytes().to_vec();
            expected.extend_from_slice(&(add | register | (register << 5)).to_le_bytes());
            assert_eq!(
                section.data().unwrap(),
                expected,
                "addend={addend}, register={register}"
            );
            let relocs: Vec<_> = section
                .relocations()
                .map(|(offset, reloc)| (offset, reloc.flags()))
                .collect();
            assert_eq!(
                relocs,
                vec![
                    (
                        0,
                        RelocationFlags::Coff {
                            typ: object::pe::IMAGE_REL_ARM64_PAGEBASE_REL21
                        }
                    ),
                    (
                        4,
                        RelocationFlags::Coff {
                            typ: object::pe::IMAGE_REL_ARM64_PAGEOFFSET_12A
                        }
                    ),
                ]
            );
        }
    }
}

#[test]
fn arm64_coff_page_addends_reject_range_and_truncated_instructions_atomically() {
    for addend in [i64::MIN, -1048577, 1048576, i64::MAX] {
        let (mut object, text, target) = arm64_page_fixture();
        let before = object.section(text).data().to_vec();
        let error = object
            .add_relocation(
                text,
                write::Relocation {
                    offset: 0,
                    symbol: target,
                    addend,
                    flags: RelocationFlags::Coff {
                        typ: object::pe::IMAGE_REL_ARM64_PAGEBASE_REL21,
                    },
                },
            )
            .unwrap_err();
        assert!(error.to_string().contains("out of range"));
        assert_eq!(object.section(text).data(), before);
        let bytes = object.write().unwrap();
        let parsed = read::File::parse(bytes.as_slice()).unwrap();
        assert_eq!(
            parsed
                .section_by_name(".text")
                .unwrap()
                .relocations()
                .count(),
            0
        );
    }
    for typ in [
        object::pe::IMAGE_REL_ARM64_PAGEBASE_REL21,
        object::pe::IMAGE_REL_ARM64_PAGEOFFSET_12A,
    ] {
        for offset in [5, 8, u64::MAX] {
            for addend in [0, 1, -1] {
                let (mut object, text, target) = arm64_page_fixture();
                let before = object.section(text).data().to_vec();
                let error = object
                    .add_relocation(
                        text,
                        write::Relocation {
                            offset,
                            symbol: target,
                            addend,
                            flags: RelocationFlags::Coff { typ },
                        },
                    )
                    .unwrap_err();
                assert!(error.to_string().contains("offset"));
                assert_eq!(object.section(text).data(), before);
            }
        }
    }
}

#[test]
fn arm64_coff_zero_explicit_addend_keeps_existing_implicit_addend() {
    let (mut object, text, target) = arm64_page_fixture();
    for (offset, instruction, typ) in [
        (
            0,
            0xb0091a20_u32,
            object::pe::IMAGE_REL_ARM64_PAGEBASE_REL21,
        ),
        (
            4,
            0x910d1400_u32,
            object::pe::IMAGE_REL_ARM64_PAGEOFFSET_12A,
        ),
    ] {
        object.section_mut(text).data_mut()[offset..offset + 4]
            .copy_from_slice(&instruction.to_le_bytes());
        object
            .add_relocation(
                text,
                write::Relocation {
                    offset: offset as u64,
                    symbol: target,
                    addend: 0,
                    flags: RelocationFlags::Coff { typ },
                },
            )
            .unwrap();
        assert_eq!(
            &object.section(text).data()[offset..offset + 4],
            &instruction.to_le_bytes()
        );
    }
}
