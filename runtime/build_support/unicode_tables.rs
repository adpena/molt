#![allow(dead_code)]

use std::fs;
use std::path::Path;
use std::process::Command;

struct UnicodeTables {
    version: String,
    digit_ranges: Vec<(u32, u32)>,
    decimal_ranges: Vec<(u32, u32)>,
    numeric_ranges: Vec<(u32, u32)>,
    space_ranges: Vec<(u32, u32)>,
    printable_ranges: Vec<(u32, u32)>,
    titlecase_entries: Vec<(u32, String)>,
}

pub(crate) fn emit_cpython_abi_unicode_tables(out_dir: &Path, build_python: &str) {
    let tables = collect_unicode_tables(build_python, "CPython ABI");
    tables.require_range_tables(build_python, "CPython ABI");
    write_unicode_range_modules(out_dir, &tables);
}

pub(crate) fn emit_runtime_unicode_tables(out_dir: &Path, build_python: &str) {
    let tables = collect_unicode_tables(build_python, "runtime");
    tables.require_range_tables(build_python, "runtime");
    if tables.titlecase_entries.is_empty() {
        panic!(
            "build Python `{build_python}` runtime unicode generation returned no titlecase table"
        );
    }
    write_unicode_range_modules(out_dir, &tables);
    write_unicode_titlecase_module(
        out_dir,
        "unicode_titlecase_map.rs",
        "UNICODE_TITLECASE_VERSION",
        "UNICODE_TITLECASE_MAP",
        &tables.version,
        &tables.titlecase_entries,
    );
}

fn collect_unicode_tables(build_python: &str, consumer: &str) -> UnicodeTables {
    let output = Command::new(build_python)
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            let Some(mut stdin) = child.stdin.take() else {
                panic!("failed to open build Python `{build_python}` stdin for {consumer} unicode tables");
            };
            stdin.write_all(UNICODE_TABLE_SCRIPT.as_bytes())?;
            drop(stdin);
            child.wait_with_output()
        });
    let output = match output {
        Ok(out) => out,
        Err(err) => {
            panic!(
                "failed to run build Python `{build_python}` for {consumer} unicode tables: {err}"
            );
        }
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!(
            "build Python `{build_python}` {consumer} unicode table generation failed: {stderr}"
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines();
    let version = lines.next().unwrap_or_default().trim().to_string();
    if version.is_empty() {
        panic!(
            "build Python `{build_python}` {consumer} unicode table generation returned no version"
        );
    }

    let mut tables = UnicodeTables {
        version,
        digit_ranges: Vec::new(),
        decimal_ranges: Vec::new(),
        numeric_ranges: Vec::new(),
        space_ranges: Vec::new(),
        printable_ranges: Vec::new(),
        titlecase_entries: Vec::new(),
    };
    let mut current_section = "";
    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') && line.len() > 2 {
            current_section = &line[1..line.len() - 1];
            continue;
        }
        if current_section == "titlecase" {
            let Some((code, cps)) = line.split_once(';') else {
                continue;
            };
            let code: u32 = code.parse().expect("invalid unicode titlecase codepoint");
            let mut mapped = String::new();
            for cp in cps.split(',') {
                if cp.is_empty() {
                    continue;
                }
                let value: u32 = cp
                    .parse()
                    .expect("invalid unicode titlecase mapped codepoint");
                let Some(ch) = char::from_u32(value) else {
                    panic!("invalid unicode scalar in titlecase map: {value}");
                };
                mapped.push(ch);
            }
            if mapped.is_empty() {
                panic!("unicode titlecase mapping must not be empty for codepoint {code}");
            }
            tables.titlecase_entries.push((code, mapped));
            continue;
        }
        let Some((lo, hi)) = line.split_once(',') else {
            continue;
        };
        let lo: u32 = lo.parse().expect("invalid unicode range start");
        let hi: u32 = hi.parse().expect("invalid unicode range end");
        match current_section {
            "digit" => tables.digit_ranges.push((lo, hi)),
            "decimal" => tables.decimal_ranges.push((lo, hi)),
            "numeric" => tables.numeric_ranges.push((lo, hi)),
            "space" => tables.space_ranges.push((lo, hi)),
            "printable" => tables.printable_ranges.push((lo, hi)),
            _ => {}
        }
    }
    tables
}

impl UnicodeTables {
    fn require_range_tables(&self, build_python: &str, consumer: &str) {
        if self.digit_ranges.is_empty()
            || self.decimal_ranges.is_empty()
            || self.numeric_ranges.is_empty()
            || self.space_ranges.is_empty()
            || self.printable_ranges.is_empty()
        {
            panic!(
                "build Python `{build_python}` {consumer} unicode generation returned incomplete range tables"
            );
        }
    }
}

fn write_unicode_range_modules(out_dir: &Path, tables: &UnicodeTables) {
    write_unicode_range_module(
        out_dir,
        "unicode_digit_ranges.rs",
        "UNICODE_DIGIT_VERSION",
        "UNICODE_DIGIT_RANGES",
        &tables.version,
        &tables.digit_ranges,
    );
    write_unicode_range_module(
        out_dir,
        "unicode_decimal_ranges.rs",
        "UNICODE_DECIMAL_VERSION",
        "UNICODE_DECIMAL_RANGES",
        &tables.version,
        &tables.decimal_ranges,
    );
    write_unicode_range_module(
        out_dir,
        "unicode_numeric_ranges.rs",
        "UNICODE_NUMERIC_VERSION",
        "UNICODE_NUMERIC_RANGES",
        &tables.version,
        &tables.numeric_ranges,
    );
    write_unicode_range_module(
        out_dir,
        "unicode_space_ranges.rs",
        "UNICODE_SPACE_VERSION",
        "UNICODE_SPACE_RANGES",
        &tables.version,
        &tables.space_ranges,
    );
    write_unicode_range_module(
        out_dir,
        "unicode_printable_ranges.rs",
        "UNICODE_PRINTABLE_VERSION",
        "UNICODE_PRINTABLE_RANGES",
        &tables.version,
        &tables.printable_ranges,
    );
}

fn write_unicode_range_module(
    out_dir: &Path,
    file_name: &str,
    version_symbol: &str,
    ranges_symbol: &str,
    version: &str,
    ranges: &[(u32, u32)],
) {
    let mut out = String::new();
    out.push_str("#[allow(dead_code)]\n");
    out.push_str(&format!(
        "pub(crate) const {version_symbol}: &str = {version:?};\n"
    ));
    out.push_str(&format!(
        "pub(crate) const {ranges_symbol}: &[(u32, u32)] = &[\n"
    ));
    for (lo, hi) in ranges {
        out.push_str(&format!("    ({lo}, {hi}),\n"));
    }
    out.push_str("];\n");
    fs::write(out_dir.join(file_name), out).unwrap_or_else(|err| {
        panic!("failed to write {file_name}: {err}");
    });
}

fn write_unicode_titlecase_module(
    out_dir: &Path,
    file_name: &str,
    version_symbol: &str,
    map_symbol: &str,
    version: &str,
    entries: &[(u32, String)],
) {
    let mut out = String::new();
    out.push_str("#[allow(dead_code)]\n");
    out.push_str(&format!(
        "pub(crate) const {version_symbol}: &str = {version:?};\n"
    ));
    out.push_str(&format!(
        "pub(crate) const {map_symbol}: &[(u32, &str)] = &[\n"
    ));
    for (code, mapped) in entries {
        out.push_str(&format!("    ({code}, {mapped:?}),\n"));
    }
    out.push_str("];\n");
    fs::write(out_dir.join(file_name), out).unwrap_or_else(|err| {
        panic!("failed to write {file_name}: {err}");
    });
}

const UNICODE_TABLE_SCRIPT: &str = r#"
import unicodedata

KEYS = ("digit", "decimal", "numeric", "space", "printable")
ranges = {key: [] for key in KEYS}
active = {key: None for key in KEYS}
titlecase_map = []

def flush_key(key):
    item = active[key]
    if item is not None:
        ranges[key].append(item)
        active[key] = None

for code in range(0x110000):
    ch = chr(code)
    states = {}
    try:
        unicodedata.digit(ch)
        states["digit"] = True
    except ValueError:
        states["digit"] = False
    try:
        unicodedata.decimal(ch)
        states["decimal"] = True
    except ValueError:
        states["decimal"] = False
    try:
        unicodedata.numeric(ch)
        states["numeric"] = True
    except ValueError:
        states["numeric"] = False
    states["space"] = ch.isspace()
    states["printable"] = ch.isprintable()
    title = ch.title()
    upper = ch.upper()
    if title != upper:
        titlecase_cps = ",".join(str(ord(c)) for c in title)
        titlecase_map.append((code, titlecase_cps))

    for key in KEYS:
        is_member = states[key]
        item = active[key]
        if is_member:
            if item is None:
                active[key] = (code, code)
            else:
                lo, hi = item
                if code == hi + 1:
                    active[key] = (lo, code)
                else:
                    ranges[key].append(item)
                    active[key] = (code, code)
        elif item is not None:
            ranges[key].append(item)
            active[key] = None

for key in KEYS:
    flush_key(key)

print(unicodedata.unidata_version)
for key in KEYS:
    print(f"[{key}]")
    for lo, hi in ranges[key]:
        print(f"{lo},{hi}")
print("[titlecase]")
for code, cps in titlecase_map:
    print(f"{code};{cps}")
"#;
