//! Executable helper providers, dependency closure, and lexical binding custody.
//!
//! Provider source owns its exports and references. Whole code identifiers use
//! the same literal/comment masking as source validation; diagnostic strings
//! never select helpers. Runtime fragments use column-zero module bindings and
//! indented private scopes, so bindings can be hoisted without rewriting bodies.

use super::*;
use std::sync::OnceLock;

pub(super) struct Prelude {
    pub source: String,
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
                    && !(after.starts_with('=') && !after.starts_with("=="))
                {
                    names.push(name.to_string());
                }
                offset += name.len();
            }
            names
        })
        .collect()
}

fn module_binding(code: &str) -> Option<&str> {
    if let Some(rest) = code.strip_prefix("local function ") {
        return Some(identifier(rest));
    }
    if let Some(rest) = code.strip_prefix("local ") {
        return Some(identifier(rest));
    }
    // Existing mutually recursive fragments assign a prior local declaration.
    // These definitions are providers too; forward declarations alone are not.
    let name = identifier(code);
    (name.starts_with("molt_") && code[name.len()..].trim_start().starts_with("= function"))
        .then_some(name)
}

fn has_definition(source: &str, name: &str) -> bool {
    source.lines().any(|line| {
        let code = source_checks::luau_line_code(line);
        let code = code.trim_start();
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
            // Every provider has one hoisted local, including recursive groups.
            // Preserve @native and all function/argument annotations.
            output.push_str(&line["local ".len()..]);
        } else if let Some(rest) = code.strip_prefix("local ") {
            let name = identifier(rest);
            if let Some(equal) = code.find('=') {
                output.push_str(name);
                output.push(' ');
                output.push_str(&line[equal..]);
            } else {
                // A typed forward declaration is supplied by the shared hoist.
                continue;
            }
        } else {
            output.push_str(line);
        }
        output.push('\n');
    }
    output.push('\n');
}

impl RuntimeLibrary {
    fn new(sources: Vec<(&'static str, String)>) -> Result<Self, String> {
        let mut fragments = Vec::new();
        let mut providers = BTreeMap::new();
        for (name, source) in sources {
            let mut bindings = BTreeSet::new();
            for line in source.lines() {
                let code = source_checks::luau_line_code(line);
                if let Some(binding) = module_binding(&code) {
                    let tail = code.strip_prefix("local ").unwrap_or(&code);
                    if tail
                        .strip_prefix(binding)
                        .is_some_and(|rest| rest.trim_start().starts_with(','))
                    {
                        return Err(format!(
                            "runtime fragment `{name}` must declare one module binding per statement"
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
        let selected = self.close(roots)?;
        let bindings: BTreeSet<_> = selected
            .iter()
            .flat_map(|&index| &self.fragments[index].bindings)
            .collect();
        // The entry-point exception guard can add two locals. Guest function
        // forward declarations share this same chunk budget in emit_source.
        if bindings.len() > 198 {
            return Err(format!(
                "Luau runtime needs {} chunk locals, exceeding the 198 available before the entry-point guard",
                bindings.len()
            ));
        }
        let mut source = String::from(
            "--!native\n--!strict\n-- Molt -> Luau transpiled output\n-- Runtime helpers\n\n",
        );
        for binding in &bindings {
            let _ = writeln!(source, "local {binding}: any");
        }
        source.push('\n');
        for &index in &selected {
            emit_fragment(&self.fragments[index].source, &mut source);
        }
        source.push_str(&publication);
        Ok(Prelude {
            source,
            local_count: bindings.len(),
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
