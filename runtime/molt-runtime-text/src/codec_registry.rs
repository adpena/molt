//! Canonical text codec identity and alias facts shared by runtime consumers.

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum EncodingKind {
    Utf8,
    Utf8Sig,
    Cp1252,
    Cp437,
    Cp850,
    Cp860,
    Cp862,
    Cp863,
    Cp865,
    Cp866,
    Cp874,
    Cp1250,
    Cp1251,
    Cp1253,
    Cp1254,
    Cp1255,
    Cp1256,
    Cp1257,
    Koi8R,
    Koi8U,
    Iso8859_2,
    Iso8859_3,
    Iso8859_4,
    Iso8859_5,
    Iso8859_6,
    Iso8859_7,
    Iso8859_8,
    Iso8859_10,
    Iso8859_15,
    MacRoman,
    Latin1,
    Ascii,
    UnicodeEscape,
    Utf16,
    Utf16LE,
    Utf16BE,
    Utf32,
    Utf32LE,
    Utf32BE,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextEncodingClass {
    Utf8,
    Ascii,
    SingleByte,
    Utf16,
    Utf32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodecRuntimeClass {
    Utf8,
    Utf8Sig,
    Charmap,
    Latin1,
    Ascii,
    UnicodeEscape,
    Utf16,
    Utf16LE,
    Utf16BE,
    Utf32,
    Utf32LE,
    Utf32BE,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CodecDescriptor {
    pub kind: EncodingKind,
    pub canonical_label: &'static str,
    pub python_module: &'static str,
    pub text_class: Option<TextEncodingClass>,
    pub ordinal_limit: u32,
}

impl EncodingKind {
    pub fn descriptor(self) -> &'static CodecDescriptor {
        &CODEC_DESCRIPTORS[self as usize]
    }

    pub fn name(self) -> &'static str {
        self.descriptor().canonical_label
    }

    pub fn python_module_name(self) -> &'static str {
        self.descriptor().python_module
    }

    pub fn ordinal_limit(self) -> u32 {
        self.descriptor().ordinal_limit
    }

    pub fn text_class(self) -> Option<TextEncodingClass> {
        self.descriptor().text_class
    }

    pub fn runtime_class(self) -> CodecRuntimeClass {
        match self {
            EncodingKind::Utf8 => CodecRuntimeClass::Utf8,
            EncodingKind::Utf8Sig => CodecRuntimeClass::Utf8Sig,
            EncodingKind::Cp1252
            | EncodingKind::Cp437
            | EncodingKind::Cp850
            | EncodingKind::Cp860
            | EncodingKind::Cp862
            | EncodingKind::Cp863
            | EncodingKind::Cp865
            | EncodingKind::Cp866
            | EncodingKind::Cp874
            | EncodingKind::Cp1250
            | EncodingKind::Cp1251
            | EncodingKind::Cp1253
            | EncodingKind::Cp1254
            | EncodingKind::Cp1255
            | EncodingKind::Cp1256
            | EncodingKind::Cp1257
            | EncodingKind::Koi8R
            | EncodingKind::Koi8U
            | EncodingKind::Iso8859_2
            | EncodingKind::Iso8859_3
            | EncodingKind::Iso8859_4
            | EncodingKind::Iso8859_5
            | EncodingKind::Iso8859_6
            | EncodingKind::Iso8859_7
            | EncodingKind::Iso8859_8
            | EncodingKind::Iso8859_10
            | EncodingKind::Iso8859_15
            | EncodingKind::MacRoman => CodecRuntimeClass::Charmap,
            EncodingKind::Latin1 => CodecRuntimeClass::Latin1,
            EncodingKind::Ascii => CodecRuntimeClass::Ascii,
            EncodingKind::UnicodeEscape => CodecRuntimeClass::UnicodeEscape,
            EncodingKind::Utf16 => CodecRuntimeClass::Utf16,
            EncodingKind::Utf16LE => CodecRuntimeClass::Utf16LE,
            EncodingKind::Utf16BE => CodecRuntimeClass::Utf16BE,
            EncodingKind::Utf32 => CodecRuntimeClass::Utf32,
            EncodingKind::Utf32LE => CodecRuntimeClass::Utf32LE,
            EncodingKind::Utf32BE => CodecRuntimeClass::Utf32BE,
        }
    }

    pub fn encode_error_label(self) -> &'static str {
        match self.runtime_class() {
            CodecRuntimeClass::Utf8Sig => "utf-8",
            CodecRuntimeClass::Charmap => "charmap",
            _ => self.name(),
        }
    }
}

const fn descriptor(
    kind: EncodingKind,
    canonical_label: &'static str,
    python_module: &'static str,
    text_class: Option<TextEncodingClass>,
    ordinal_limit: u32,
) -> CodecDescriptor {
    CodecDescriptor {
        kind,
        canonical_label,
        python_module,
        text_class,
        ordinal_limit,
    }
}

pub const CODEC_DESCRIPTORS: &[CodecDescriptor] = &[
    descriptor(
        EncodingKind::Utf8,
        "utf-8",
        "utf_8",
        Some(TextEncodingClass::Utf8),
        u32::MAX,
    ),
    descriptor(
        EncodingKind::Utf8Sig,
        "utf-8-sig",
        "utf_8_sig",
        Some(TextEncodingClass::Utf8),
        u32::MAX,
    ),
    descriptor(
        EncodingKind::Cp1252,
        "cp1252",
        "cp1252",
        Some(TextEncodingClass::SingleByte),
        u32::MAX,
    ),
    descriptor(
        EncodingKind::Cp437,
        "cp437",
        "cp437",
        Some(TextEncodingClass::SingleByte),
        u32::MAX,
    ),
    descriptor(
        EncodingKind::Cp850,
        "cp850",
        "cp850",
        Some(TextEncodingClass::SingleByte),
        u32::MAX,
    ),
    descriptor(
        EncodingKind::Cp860,
        "cp860",
        "cp860",
        Some(TextEncodingClass::SingleByte),
        u32::MAX,
    ),
    descriptor(
        EncodingKind::Cp862,
        "cp862",
        "cp862",
        Some(TextEncodingClass::SingleByte),
        u32::MAX,
    ),
    descriptor(
        EncodingKind::Cp863,
        "cp863",
        "cp863",
        Some(TextEncodingClass::SingleByte),
        u32::MAX,
    ),
    descriptor(
        EncodingKind::Cp865,
        "cp865",
        "cp865",
        Some(TextEncodingClass::SingleByte),
        u32::MAX,
    ),
    descriptor(
        EncodingKind::Cp866,
        "cp866",
        "cp866",
        Some(TextEncodingClass::SingleByte),
        u32::MAX,
    ),
    descriptor(
        EncodingKind::Cp874,
        "cp874",
        "cp874",
        Some(TextEncodingClass::SingleByte),
        u32::MAX,
    ),
    descriptor(
        EncodingKind::Cp1250,
        "cp1250",
        "cp1250",
        Some(TextEncodingClass::SingleByte),
        u32::MAX,
    ),
    descriptor(
        EncodingKind::Cp1251,
        "cp1251",
        "cp1251",
        Some(TextEncodingClass::SingleByte),
        u32::MAX,
    ),
    descriptor(
        EncodingKind::Cp1253,
        "cp1253",
        "cp1253",
        Some(TextEncodingClass::SingleByte),
        u32::MAX,
    ),
    descriptor(
        EncodingKind::Cp1254,
        "cp1254",
        "cp1254",
        Some(TextEncodingClass::SingleByte),
        u32::MAX,
    ),
    descriptor(
        EncodingKind::Cp1255,
        "cp1255",
        "cp1255",
        Some(TextEncodingClass::SingleByte),
        u32::MAX,
    ),
    descriptor(
        EncodingKind::Cp1256,
        "cp1256",
        "cp1256",
        Some(TextEncodingClass::SingleByte),
        u32::MAX,
    ),
    descriptor(
        EncodingKind::Cp1257,
        "cp1257",
        "cp1257",
        Some(TextEncodingClass::SingleByte),
        u32::MAX,
    ),
    descriptor(
        EncodingKind::Koi8R,
        "koi8-r",
        "koi8_r",
        Some(TextEncodingClass::SingleByte),
        u32::MAX,
    ),
    descriptor(
        EncodingKind::Koi8U,
        "koi8-u",
        "koi8_u",
        Some(TextEncodingClass::SingleByte),
        u32::MAX,
    ),
    descriptor(
        EncodingKind::Iso8859_2,
        "iso8859-2",
        "iso8859_2",
        Some(TextEncodingClass::SingleByte),
        u32::MAX,
    ),
    descriptor(
        EncodingKind::Iso8859_3,
        "iso8859-3",
        "iso8859_3",
        Some(TextEncodingClass::SingleByte),
        u32::MAX,
    ),
    descriptor(
        EncodingKind::Iso8859_4,
        "iso8859-4",
        "iso8859_4",
        Some(TextEncodingClass::SingleByte),
        u32::MAX,
    ),
    descriptor(
        EncodingKind::Iso8859_5,
        "iso8859-5",
        "iso8859_5",
        Some(TextEncodingClass::SingleByte),
        u32::MAX,
    ),
    descriptor(
        EncodingKind::Iso8859_6,
        "iso8859-6",
        "iso8859_6",
        Some(TextEncodingClass::SingleByte),
        u32::MAX,
    ),
    descriptor(
        EncodingKind::Iso8859_7,
        "iso8859-7",
        "iso8859_7",
        Some(TextEncodingClass::SingleByte),
        u32::MAX,
    ),
    descriptor(
        EncodingKind::Iso8859_8,
        "iso8859-8",
        "iso8859_8",
        Some(TextEncodingClass::SingleByte),
        u32::MAX,
    ),
    descriptor(
        EncodingKind::Iso8859_10,
        "iso8859-10",
        "iso8859_10",
        Some(TextEncodingClass::SingleByte),
        u32::MAX,
    ),
    descriptor(
        EncodingKind::Iso8859_15,
        "iso8859-15",
        "iso8859_15",
        Some(TextEncodingClass::SingleByte),
        u32::MAX,
    ),
    descriptor(
        EncodingKind::MacRoman,
        "mac-roman",
        "mac_roman",
        Some(TextEncodingClass::SingleByte),
        u32::MAX,
    ),
    descriptor(
        EncodingKind::Latin1,
        "latin-1",
        "latin_1",
        Some(TextEncodingClass::SingleByte),
        256,
    ),
    descriptor(
        EncodingKind::Ascii,
        "ascii",
        "ascii",
        Some(TextEncodingClass::Ascii),
        128,
    ),
    descriptor(
        EncodingKind::UnicodeEscape,
        "unicode-escape",
        "unicode_escape",
        None,
        u32::MAX,
    ),
    descriptor(
        EncodingKind::Utf16,
        "utf-16",
        "utf_16",
        Some(TextEncodingClass::Utf16),
        u32::MAX,
    ),
    descriptor(
        EncodingKind::Utf16LE,
        "utf-16-le",
        "utf_16_le",
        Some(TextEncodingClass::Utf16),
        u32::MAX,
    ),
    descriptor(
        EncodingKind::Utf16BE,
        "utf-16-be",
        "utf_16_be",
        Some(TextEncodingClass::Utf16),
        u32::MAX,
    ),
    descriptor(
        EncodingKind::Utf32,
        "utf-32",
        "utf_32",
        Some(TextEncodingClass::Utf32),
        u32::MAX,
    ),
    descriptor(
        EncodingKind::Utf32LE,
        "utf-32-le",
        "utf_32_le",
        Some(TextEncodingClass::Utf32),
        u32::MAX,
    ),
    descriptor(
        EncodingKind::Utf32BE,
        "utf-32-be",
        "utf_32_be",
        Some(TextEncodingClass::Utf32),
        u32::MAX,
    ),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EncodingAlias {
    pub alias: &'static str,
    pub kind: EncodingKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PythonEncodingAlias {
    pub alias: &'static str,
    pub module: &'static str,
}

pub use crate::codec_aliases_generated::ENCODING_ALIASES;
pub use crate::codec_aliases_generated::PYTHON_ENCODING_ALIASES;

pub fn normalize_encoding(name: &str) -> Option<EncodingKind> {
    let key = encoding_search_key(name);
    ENCODING_ALIASES
        .iter()
        .find(|entry| entry.alias == key)
        .map(|entry| entry.kind)
}

fn encoding_search_key(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut punct = false;
    for byte in name.bytes() {
        match byte {
            b'a'..=b'z' | b'0'..=b'9' | b'.' => {
                if punct && !out.is_empty() {
                    out.push('_');
                }
                out.push(byte as char);
                punct = false;
            }
            b'A'..=b'Z' => {
                if punct && !out.is_empty() {
                    out.push('_');
                }
                out.push((byte + 32) as char);
                punct = false;
            }
            _ => {
                punct = true;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_table_matches_enum_order() {
        assert_eq!(CODEC_DESCRIPTORS.len(), EncodingKind::Utf32BE as usize + 1);
        for (idx, descriptor) in CODEC_DESCRIPTORS.iter().enumerate() {
            assert_eq!(descriptor.kind as usize, idx);
        }
    }

    #[test]
    fn aliases_preserve_direct_label_and_python_module_roles() {
        let kind = normalize_encoding("ISO-8859-2").unwrap();
        assert_eq!(kind.name(), "iso8859-2");
        assert_eq!(kind.python_module_name(), "iso8859_2");

        let utf8 = normalize_encoding("utf_8").unwrap();
        assert_eq!(utf8.name(), "utf-8");
        assert_eq!(utf8.python_module_name(), "utf_8");
    }

    #[test]
    fn generated_aliases_cover_cpython_supported_spellings() {
        assert_eq!(normalize_encoding("1252"), Some(EncodingKind::Cp1252));
        assert_eq!(
            normalize_encoding("windows-1252"),
            Some(EncodingKind::Cp1252)
        );
        assert_eq!(normalize_encoding("l2"), Some(EncodingKind::Iso8859_2));
        assert_eq!(
            normalize_encoding("csisolatin2"),
            Some(EncodingKind::Iso8859_2)
        );
        assert_eq!(normalize_encoding("cskoi8r"), Some(EncodingKind::Koi8R));
        assert_eq!(
            normalize_encoding("macintosh"),
            Some(EncodingKind::MacRoman)
        );
        assert_eq!(normalize_encoding("646"), Some(EncodingKind::Ascii));
        assert_eq!(normalize_encoding("cp367"), Some(EncodingKind::Ascii));
        assert_eq!(normalize_encoding("cp-1252"), None);
    }

    #[test]
    fn text_classes_come_from_descriptors() {
        assert_eq!(
            normalize_encoding("cp1252").unwrap().text_class(),
            Some(TextEncodingClass::SingleByte)
        );
        assert_eq!(
            normalize_encoding("utf-16-le").unwrap().text_class(),
            Some(TextEncodingClass::Utf16)
        );
        assert_eq!(
            normalize_encoding("unicode-escape").unwrap().text_class(),
            None
        );
    }

    #[test]
    fn runtime_classes_own_charmap_and_error_label_facts() {
        let cp1252 = normalize_encoding("cp1252").unwrap();
        assert_eq!(cp1252.runtime_class(), CodecRuntimeClass::Charmap);
        assert_eq!(cp1252.encode_error_label(), "charmap");

        let latin1 = normalize_encoding("latin-1").unwrap();
        assert_eq!(latin1.runtime_class(), CodecRuntimeClass::Latin1);
        assert_eq!(latin1.encode_error_label(), "latin-1");

        let utf8_sig = normalize_encoding("utf-8-sig").unwrap();
        assert_eq!(utf8_sig.runtime_class(), CodecRuntimeClass::Utf8Sig);
        assert_eq!(utf8_sig.encode_error_label(), "utf-8");
    }
}
