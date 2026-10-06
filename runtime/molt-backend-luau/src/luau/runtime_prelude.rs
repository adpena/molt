//! Executable helper providers, dependency closure, and lexical binding custody.
//!
//! Provider source owns its bindings and references. Whole code identifiers use
//! the same literal/comment masking as source validation; diagnostic strings
//! never select helpers. Runtime fragments use column-zero module bindings and
//! indented private scopes, so bindings can be hoisted without rewriting bodies.

use super::*;
use std::sync::OnceLock;

// Luau permits 200 simultaneously active locals. Reserve the existing two
// entry-point exception-guard cells in every chunk budget calculation.
pub(super) const CHUNK_LOCAL_LIMIT: usize = 198;

pub(super) struct Prelude {
    pub source: String,
    /// Exported cells still live when guest declarations begin. Provider-private
    /// cells have left lexical scope, even when retained by exported closures.
    pub local_count: usize,
}

struct Fragment {
    name: &'static str,
    source: String,
    bindings: BTreeSet<String>,
    references: BTreeSet<String>,
}

pub(super) struct RuntimeLibrary {
    fragments: Vec<Fragment>,
    providers: BTreeMap<String, usize>,
}

fn identifier(text: &str) -> &str {
    let end = text
        .bytes()
        .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        .count();
    &text[..end]
}

fn code_identifiers(source: &str) -> BTreeSet<String> {
    source
        .lines()
        .flat_map(|line| {
            let code = source_checks::luau_line_code(line);
            let mut names = Vec::new();
            let mut offset = 0;
            while offset < code.len() {
                let byte = code.as_bytes()[offset];
                if !byte.is_ascii_alphabetic() && byte != b'_' {
                    offset += 1;
                    continue;
                }
                let name = identifier(&code[offset..]);
                let before = code[..offset].trim_end();
                let after = code[offset + name.len()..].trim_start();
                // Member names and assignment/table keys are not references.
                if !before.ends_with('.')
                    && !before.ends_with(':')
                    && (!after.starts_with('=') || after.starts_with("=="))
                {
                    names.push(name.to_string());
                }
                offset += name.len();
            }
            names
        })
        .collect()
}

fn module_binding(code: &str) -> Result<Option<&str>, &'static str> {
    if code.starts_with("function ") {
        return Err("must declare module functions with `local function` or a bare assignment");
    }
    let function = code.strip_prefix("local function ");
    let local = code.strip_prefix("local ");
    let rest = function.or(local).unwrap_or(code);
    let name = identifier(rest);
    let tail = rest[name.len()..].trim_start();
    if local.is_some() {
        if !name.starts_with(|ch: char| ch.is_ascii_alphabetic() || ch == '_') {
            return Err("has an invalid module binding name");
        }
        if function.is_some() {
            if !tail.starts_with('(') {
                return Err("must declare an unqualified module function");
            }
        } else if tail.starts_with(',') {
            return Err("must declare one module binding per statement");
        } else if !tail.is_empty() && !tail.starts_with(':') && !tail.starts_with('=') {
            return Err("has an unsupported module binding declaration");
        }
        return Ok(Some(name));
    }
    if name.is_empty() {
        return Ok(None);
    }
    // Bare assignments are source-owned definitions as well, including the
    // mutually recursive equality/call providers and scalar initialization.
    // Hoist every such cell; an unfamiliar definition must never become global.
    if tail.starts_with('=') && !tail.starts_with("==") {
        return Ok(Some(name));
    }
    if tail.starts_with(',') && tail.contains('=') {
        return Err("must assign one module binding per statement");
    }
    if ["+=", "-=", "*=", "/=", "//=", "%=", "^=", "..="]
        .iter()
        .any(|operator| tail.starts_with(*operator))
    {
        return Err("must initialize module bindings with a plain assignment");
    }
    Ok(None)
}

fn has_definition(source: &str, name: &str) -> bool {
    source.lines().any(|line| {
        let code = source_checks::luau_line_code(line);
        let code = code.as_str();
        if let Some(rest) = code.strip_prefix("local function ") {
            return identifier(rest) == name;
        }
        let (local, code) = match code.strip_prefix("local ") {
            Some(code) => (true, code),
            None => (false, code),
        };
        if identifier(code) != name {
            return false;
        }
        let tail = code[name.len()..].trim_start();
        if local && tail.starts_with(':') {
            // Provider value annotations contain no expression-level equals.
            tail.contains('=')
        } else {
            tail.starts_with('=') && !tail.starts_with("==")
        }
    })
}

fn emit_fragment(source: &str, output: &mut String) {
    for line in source.lines() {
        let code = source_checks::luau_line_code(line);
        if code.starts_with("local function ") {
            // Every binding has one hoisted local, including recursive groups.
            // Preserve @native and all function/argument annotations.
            output.push('\t');
            output.push_str(&line["local ".len()..]);
        } else if let Some(rest) = code.strip_prefix("local ") {
            let name = identifier(rest);
            if let Some(equal) = code.find('=') {
                output.push('\t');
                output.push_str(name);
                output.push(' ');
                output.push_str(&line[equal..]);
            } else {
                // A typed forward declaration is supplied by the shared hoist.
                continue;
            }
        } else {
            output.push('\t');
            output.push_str(line);
        }
        output.push('\n');
    }
    output.push('\n');
}

impl RuntimeLibrary {
    pub(super) fn new(sources: Vec<(&'static str, String)>) -> Result<Self, String> {
        let mut fragments = Vec::new();
        let mut providers = BTreeMap::new();
        for (name, source) in sources {
            let mut bindings = BTreeSet::new();
            let mut declarations = BTreeSet::new();
            for line in source.lines() {
                let code = source_checks::luau_line_code(line);
                if let Some(binding) = module_binding(&code)
                    .map_err(|reason| format!("runtime fragment `{name}` {reason}"))?
                {
                    if code.starts_with("local ") && !declarations.insert(binding.to_string()) {
                        return Err(format!(
                            "runtime fragment `{name}` redeclares module binding `{binding}`"
                        ));
                    }
                    if binding.is_empty() || !has_definition(&source, binding) {
                        return Err(format!(
                            "runtime fragment `{name}` has an unimplemented binding `{binding}`"
                        ));
                    }
                    bindings.insert(binding.to_string());
                }
            }
            for binding in &bindings {
                if providers.insert(binding.clone(), fragments.len()).is_some() {
                    return Err(format!("duplicate Luau runtime provider `{binding}`"));
                }
            }
            let references = code_identifiers(&source);
            fragments.push(Fragment {
                name,
                source,
                bindings,
                references,
            });
        }
        Ok(Self {
            fragments,
            providers,
        })
    }

    fn close(&self, roots: BTreeSet<String>) -> Result<BTreeSet<usize>, String> {
        let mut pending: Vec<(String, &str)> = roots
            .into_iter()
            .map(|name| (name, "requested source"))
            .collect();
        let mut selected = BTreeSet::new();
        while let Some((name, consumer)) = pending.pop() {
            let Some(&index) = self.providers.get(&name) else {
                return Err(format!(
                    "missing executable Luau runtime provider `{name}` required by {consumer}"
                ));
            };
            if !selected.insert(index) {
                continue;
            }
            let fragment = &self.fragments[index];
            pending.extend(
                fragment
                    .references
                    .iter()
                    .filter(|name| name.starts_with("molt_") || self.providers.contains_key(*name))
                    .map(|name| (name.clone(), fragment.name)),
            );
        }
        Ok(selected)
    }

    pub(super) fn validate_adapter(&self, adapter: &str) -> Result<(), String> {
        let roots = code_identifiers(adapter)
            .into_iter()
            .filter(|name| name.starts_with("molt_") || self.providers.contains_key(name))
            .collect();
        self.close(roots).map(|_| ())
    }

    pub(super) fn emit(&self, body: &str, publish_builtins: bool) -> Result<Prelude, String> {
        let publication = if publish_builtins {
            op_calls::builtin_namespace_publication(self)
        } else {
            String::new()
        };
        let mut roots: BTreeSet<_> = code_identifiers(body)
            .into_iter()
            .filter(|name| self.providers.contains_key(name))
            .collect();
        // Base storage/signature metadata is used by emitted function prologues.
        roots.insert("molt_rawequal".to_string());
        roots.extend(
            code_identifiers(&publication)
                .into_iter()
                .filter(|name| self.providers.contains_key(name)),
        );
        // Existing standard-module bridges retain their literal import roots.
        // Python namespace bootstrap is independently selected from IR semantics.
        for (module, provider) in [
            ("math", "molt_math"),
            ("json", "molt_json_dumps"),
            ("time", "molt_time"),
            ("os", "molt_os"),
        ] {
            if body.contains(&format!("\"{module}\"")) {
                roots.insert(provider.to_string());
            }
        }
        let selected = self.close(roots.clone())?;
        let mut exports = roots;
        for &index in &selected {
            exports.extend(self.fragments[index].references.iter().filter_map(|name| {
                self.providers
                    .get(name)
                    .filter(|&&provider| provider != index)
                    .map(|_| name.clone())
            }));
        }
        // Exports stay live throughout initialization. Each provider's private
        // cells coexist only with those exports, not with other providers or
        // later guest declarations. Escaping closures retain the same cells.
        for &index in &selected {
            let fragment = &self.fragments[index];
            let private_count = fragment.bindings.difference(&exports).count();
            let active_count = exports.len() + private_count;
            if active_count > CHUNK_LOCAL_LIMIT {
                return Err(format!(
                    "Luau runtime provider `{}` needs {active_count} simultaneously active locals ({} exports and {private_count} private), exceeding the {CHUNK_LOCAL_LIMIT} available before the entry-point guard",
                    fragment.name,
                    exports.len()
                ));
            }
        }
        let mut source = String::from(
            "--!native\n--!strict\n-- Molt -> Luau transpiled output\n-- Runtime helpers\n\n",
        );
        for binding in &exports {
            let _ = writeln!(source, "local {binding}: any");
        }
        source.push('\n');
        for &index in &selected {
            let fragment = &self.fragments[index];
            let _ = writeln!(source, "do -- Runtime provider: {}", fragment.name);
            for binding in fragment.bindings.difference(&exports) {
                let _ = writeln!(source, "\tlocal {binding}: any");
            }
            emit_fragment(&fragment.source, &mut source);
            source.push_str("end\n\n");
        }
        source.push_str(&publication);
        Ok(Prelude {
            source,
            local_count: exports.len(),
        })
    }
}

pub(super) fn library() -> Result<&'static RuntimeLibrary, String> {
    static LIBRARY: OnceLock<Result<RuntimeLibrary, String>> = OnceLock::new();
    LIBRARY
        .get_or_init(|| RuntimeLibrary::new(runtime_fragments::fragments()))
        .as_ref()
        .map_err(|reason| reason.clone())
}

pub(super) fn needs_builtin_namespace(ir: &SimpleIR) -> bool {
    use molt_ir::tir::op_kinds_generated::SimpleIrRuntimeRequirements as Requirements;
    let namespace_semantics = Requirements::OBJECT_MODEL
        .union(Requirements::FALLIBLE_PROTOCOL)
        .union(Requirements::EXECUTION_FRAME)
        .union(Requirements::IMPORT_PROTOCOL);
    ir.functions
        .iter()
        .flat_map(|function| &function.ops)
        .any(|op| {
            op.runtime_requirements()
                .is_some_and(|requirements| requirements.bits() & namespace_semantics.bits() != 0)
        })
}

pub(super) fn validate_ir_adapters(ir: &SimpleIR) -> Result<(), String> {
    let library = library()?;
    for function in &ir.functions {
        for (index, op) in function.ops.iter().enumerate() {
            if op.kind != "builtin_func" || op_calls::is_public_builtin_acquisition(op) {
                continue;
            }
            if let Some(adapter) = op
                .s_value
                .as_deref()
                .and_then(op_calls::builtin_runtime_adapter)
            {
                library.validate_adapter(adapter).map_err(|reason| format!(
                    "luau target rejected before source generation: {}:op#{index} explicit runtime constructor `{}`: {reason}",
                    function.name, op.s_value.as_deref().unwrap_or_default(),
                ))?;
            }
        }
    }
    Ok(())
}
