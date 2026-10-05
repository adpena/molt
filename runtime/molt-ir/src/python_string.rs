// This exact dependency-free source is compiled by IR and embedded in standalone
// programs. Canonical UTF-8/surrogatepass is the only storage: ASCII uses one
// byte, scalar Unicode one to four, and each surrogate three. Adjacent surrogate
// halves remain two code points. Rust String is an explicit scalar-text edge.
#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PythonString(Vec<u8>);

impl PythonString {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_code_points(points: &[u32]) -> Self {
        let mut bytes = Vec::new();
        for &point in points {
            assert!(point <= 0x10ffff);
            match point {
                0..=0x7f => bytes.push(point as u8),
                0x80..=0x7ff => {
                    bytes.push(0xc0 | (point >> 6) as u8);
                    bytes.push(0x80 | (point & 0x3f) as u8);
                }
                0x800..=0xffff => {
                    bytes.push(0xe0 | (point >> 12) as u8);
                    bytes.push(0x80 | ((point >> 6) & 0x3f) as u8);
                    bytes.push(0x80 | (point & 0x3f) as u8);
                }
                _ => {
                    bytes.push(0xf0 | (point >> 18) as u8);
                    bytes.push(0x80 | ((point >> 12) & 0x3f) as u8);
                    bytes.push(0x80 | ((point >> 6) & 0x3f) as u8);
                    bytes.push(0x80 | (point & 0x3f) as u8);
                }
            }
        }
        Self(bytes)
    }

    pub fn from_code_point(point: u32) -> Self {
        Self::from_code_points(&[point])
    }

    pub fn from_utf8_surrogatepass(bytes: &[u8]) -> Result<Self, usize> {
        Self::validate_utf8_surrogatepass(bytes)?;
        Ok(Self(bytes.to_vec()))
    }

    pub fn validate_utf8_surrogatepass(bytes: &[u8]) -> Result<(), usize> {
        let mut index = 0;
        while index < bytes.len() {
            index = decode_python_code_point(bytes, index)?.1;
        }
        Ok(())
    }

    pub fn as_surrogatepass_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn code_points(&self) -> impl Iterator<Item = u32> + DoubleEndedIterator + '_ {
        PythonCodePoints { bytes: &self.0 }
    }

    pub fn len(&self) -> usize {
        self.code_points().count()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn concat(&self, other: &Self) -> Self {
        let mut points = self.0.clone();
        points.extend_from_slice(&other.0);
        Self(points)
    }

    pub fn repeat(&self, count: usize) -> Self {
        Self(self.0.repeat(count))
    }

    pub fn contains(&self, other: &Self) -> bool {
        // Canonical nonempty text begins at an ASCII or leading byte and cannot
        // match inside a code point, so byte containment preserves code points.
        other.is_empty()
            || self
                .0
                .windows(other.0.len())
                .any(|window| window == other.0)
    }

    pub fn to_utf8(&self) -> Result<String, usize> {
        std::str::from_utf8(&self.0)
            .map(str::to_owned)
            .map_err(|error| {
                PythonCodePoints {
                    bytes: &self.0[..error.valid_up_to()],
                }
                .count()
            })
    }

    /// Python's UTF-8 backslashreplace edge used by diagnostic stderr output.
    /// Storage remains surrogatepass; encoding never fuses surrogate halves.
    pub fn to_utf8_backslashreplace(&self) -> String {
        let mut output = String::new();
        for point in self.code_points() {
            if let Some(scalar) = char::from_u32(point) {
                output.push(scalar);
            } else {
                use std::fmt::Write;
                write!(&mut output, "\\u{point:04x}").expect("String writes cannot fail");
            }
        }
        output
    }

    pub fn repr(&self) -> String {
        let quote = if self.0.contains(&b'\'') && !self.0.contains(&b'"') {
            '"'
        } else {
            '\''
        };
        let mut output = String::new();
        output.push(quote);
        for point in self.code_points() {
            match point {
                0x09 => output.push_str("\\t"),
                0x0a => output.push_str("\\n"),
                0x0d => output.push_str("\\r"),
                0x5c => output.push_str("\\\\"),
                point if point == u32::from(quote) => {
                    output.push('\\');
                    output.push(quote);
                }
                0..=0x1f | 0x7f..=0x9f => output.push_str(&format!("\\x{point:02x}")),
                0xd800..=0xdfff => output.push_str(&format!("\\u{point:04x}")),
                point => output.push(char::from_u32(point).expect("valid Python code point")),
            }
        }
        output.push(quote);
        output
    }
}

impl From<&str> for PythonString {
    fn from(value: &str) -> Self {
        Self(value.as_bytes().to_vec())
    }
}

impl From<String> for PythonString {
    fn from(value: String) -> Self {
        Self(value.into_bytes())
    }
}

impl PartialEq<str> for PythonString {
    fn eq(&self, other: &str) -> bool {
        self.0 == other.as_bytes()
    }
}

fn decode_python_code_point(bytes: &[u8], index: usize) -> Result<(u32, usize), usize> {
    let first = bytes[index];
    let (width, mut point, minimum) = match first {
        0x00..=0x7f => (1, u32::from(first), 0),
        0xc2..=0xdf => (2, u32::from(first & 0x1f), 0x80),
        0xe0..=0xef => (3, u32::from(first & 0x0f), 0x800),
        0xf0..=0xf4 => (4, u32::from(first & 7), 0x10000),
        _ => return Err(index),
    };
    let Some(sequence) = bytes.get(index..index + width) else {
        return Err(index);
    };
    for &continuation in &sequence[1..] {
        if continuation & 0xc0 != 0x80 {
            return Err(index);
        }
        point = (point << 6) | u32::from(continuation & 0x3f);
    }
    if point < minimum || point > 0x10ffff {
        return Err(index);
    }
    Ok((point, index + width))
}

struct PythonCodePoints<'a> {
    bytes: &'a [u8],
}

impl Iterator for PythonCodePoints<'_> {
    type Item = u32;

    fn next(&mut self) -> Option<u32> {
        if self.bytes.is_empty() {
            return None;
        }
        let (point, next) = decode_python_code_point(self.bytes, 0).expect("validated Python text");
        self.bytes = &self.bytes[next..];
        Some(point)
    }
}

impl DoubleEndedIterator for PythonCodePoints<'_> {
    fn next_back(&mut self) -> Option<u32> {
        let mut start = self.bytes.len().checked_sub(1)?;
        while self.bytes[start] & 0xc0 == 0x80 {
            start -= 1;
        }
        let point = decode_python_code_point(self.bytes, start)
            .expect("validated Python text")
            .0;
        self.bytes = &self.bytes[..start];
        Some(point)
    }
}

impl PartialEq<&str> for PythonString {
    fn eq(&self, other: &&str) -> bool {
        self == *other
    }
}

impl std::fmt::Debug for PythonString {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output.write_str(&self.repr())
    }
}

impl std::fmt::Display for PythonString {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = self.to_utf8().unwrap_or_else(|index| {
            panic!("UnicodeEncodeError: 'utf-8' codec can't encode surrogate at position {index}")
        });
        output.write_str(&text)
    }
}
