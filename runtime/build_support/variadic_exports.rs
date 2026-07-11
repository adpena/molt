use std::fs;
use std::path::Path;

pub fn load_variadic_exports(path: &Path) -> Vec<String> {
    let raw = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    let mut symbols = Vec::new();
    for (index, line) in raw.lines().enumerate() {
        let symbol = line.trim();
        if symbol.is_empty() || symbol.starts_with('#') {
            continue;
        }
        if !is_c_identifier(symbol) {
            panic!(
                "invalid C symbol in {}:{}: {symbol}",
                path.display(),
                index + 1
            );
        }
        if symbols.iter().any(|existing| existing == symbol) {
            panic!("duplicate C symbol in {}: {symbol}", path.display());
        }
        symbols.push(symbol.to_string());
    }
    if symbols.is_empty() {
        panic!("variadic export manifest is empty: {}", path.display());
    }
    symbols
}

fn is_c_identifier(symbol: &str) -> bool {
    let mut chars = symbol.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

#[allow(dead_code)]
pub fn render_rust_anchors(symbols: &[String]) -> String {
    let declarations = symbols
        .iter()
        .map(|symbol| format!("    fn {symbol}();\n"))
        .collect::<String>();
    let anchors = symbols
        .iter()
        .map(|symbol| format!("    {symbol},\n"))
        .collect::<String>();
    format!(
        "unsafe extern \"C\" {{\n{declarations}}}\n\n#[used]\npub(super) static MOLT_CPYTHON_ABI_VARIADIC_EXPORT_ANCHORS: [unsafe extern \"C\" fn(); {}] = [\n{anchors}];\n",
        symbols.len()
    )
}
