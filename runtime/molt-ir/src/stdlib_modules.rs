use std::collections::BTreeSet;

use crate::backend_environment::{self as backend_env, BackendEnv};

const STDLIB_MODULES_ENV: &str = BackendEnv::MOLT_STDLIB_MODULES.name();

pub fn parse_stdlib_modules(raw: &str) -> Result<BTreeSet<String>, String> {
    let parsed: Vec<String> = serde_json::from_str(raw)
        .map_err(|err| format!("{STDLIB_MODULES_ENV} must be a JSON array of strings: {err}"))?;
    let mut out = BTreeSet::new();
    for (index, module) in parsed.into_iter().enumerate() {
        if module.is_empty() {
            return Err(format!("{STDLIB_MODULES_ENV}[{index}] must not be empty"));
        }
        if module.split('.').any(str::is_empty) || module.contains('\0') {
            return Err(format!(
                "{STDLIB_MODULES_ENV}[{index}] is not a dotted module name: {module:?}"
            ));
        }
        if !out.insert(module.clone()) {
            return Err(format!(
                "{STDLIB_MODULES_ENV}[{index}] duplicates module {module:?}"
            ));
        }
    }
    Ok(out)
}

pub fn stdlib_modules_from_env() -> Result<Option<BTreeSet<String>>, String> {
    match backend_env::var(BackendEnv::MOLT_STDLIB_MODULES) {
        Ok(raw) => parse_stdlib_modules(&raw).map(Some),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(err) => Err(format!("{STDLIB_MODULES_ENV} is not valid UTF-8: {err}")),
    }
}

#[cfg(feature = "native-backend")]
pub fn stdlib_modules_from_env_or_panic() -> Option<BTreeSet<String>> {
    stdlib_modules_from_env().unwrap_or_else(|err| panic!("{err}"))
}

/// Whether a function belongs to the application rather than the shared
/// stdlib partition. Ownership is the function record's `module`, set by the
/// frontend that produced it; a symbol is never parsed back into a module. A
/// function with no module is a program entry, which the application owns.
pub fn is_user_owned_function(
    function: &crate::ir::FunctionIR,
    entry_module: &str,
    stdlib_modules: Option<&BTreeSet<String>>,
) -> bool {
    let Some(module) = function.module.as_deref() else {
        return true;
    };
    if module == entry_module || module == "__main__" {
        return true;
    }
    stdlib_modules.is_some_and(|stdlib| !stdlib.contains(module))
}

/// Move `ir.functions` into user-owned and stdlib halves.
///
/// A symbol names one (module, local) pair, so every definition of a name has
/// the same owner and lands on the same side. The stdlib half keeps the first
/// definition of each name: the shared stdlib object defines each symbol
/// exactly once.
pub fn partition_user_owned_functions(
    ir: &mut crate::ir::SimpleIR,
    mut is_user_owned: impl FnMut(&crate::ir::FunctionIR) -> bool,
) -> (Vec<crate::ir::FunctionIR>, Vec<crate::ir::FunctionIR>) {
    use crate::function_table::{FunctionId, project_document, remap_function_ids};
    let user_names: BTreeSet<String> = ir
        .functions
        .iter()
        .filter(|function| is_user_owned(function))
        .map(|function| function.name.clone())
        .collect();
    let mut functions = std::mem::take(&mut ir.functions);
    // A repeated stdlib symbol keeps its first body; its callers move there.
    let mut first: std::collections::BTreeMap<String, FunctionId> = Default::default();
    let mut alias = Vec::with_capacity(functions.len());
    let mut duplicate = vec![false; functions.len()];
    for (index, function) in functions.iter().enumerate() {
        let id = FunctionId::from_index(index);
        if user_names.contains(&function.name) {
            alias.push(Some(id));
            continue;
        }
        let canonical = *first.entry(function.name.clone()).or_insert(id);
        duplicate[index] = canonical != id;
        alias.push(Some(canonical));
    }
    remap_function_ids(&mut functions, &alias);
    let declarations: Vec<crate::ir::FunctionIR> = functions
        .iter()
        .map(|function| {
            function
                .extern_declaration()
                .unwrap_or_else(|error| panic!("invalid partition declaration: {error}"))
        })
        .collect();
    let mut user = Vec::new();
    let mut stdlib = Vec::new();
    for (index, function) in functions.into_iter().enumerate() {
        let id = FunctionId::from_index(index);
        if user_names.contains(&function.name) {
            user.push((id, function));
        } else if !duplicate[index] {
            stdlib.push((id, function));
        }
    }
    // Each side becomes a document of its own: references to the other
    // side are extern rows (HF-847 slice 1, C2).
    let user = project_document(user, |old| declarations[old.index()].clone());
    let stdlib = project_document(stdlib, |old| declarations[old.index()].clone());
    (user, stdlib)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn function(name: &str, module: Option<&str>, params: &[&str]) -> crate::ir::FunctionIR {
        crate::ir::FunctionIR {
            name: name.to_string(),
            module: module.map(str::to_string),
            params: params.iter().map(|param| param.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn partition_user_owned_functions_moves_every_function_once() {
        let mut ir = crate::ir::SimpleIR {
            functions: vec![
                function("py.app.main", Some("app"), &[]),
                function("py.json.dumps", Some("json"), &["first"]),
                function("py.app.main", Some("app"), &["again"]),
                function("py.json.dumps", Some("json"), &["second"]),
                function("py.os.path", Some("os"), &[]),
            ],
            profile: None,
        };

        let (user, stdlib) = partition_user_owned_functions(&mut ir, |function| {
            function.module.as_deref() == Some("app")
        });

        assert!(ir.functions.is_empty());
        let user: Vec<_> = user
            .iter()
            .map(|f| (f.name.as_str(), f.params.clone()))
            .collect();
        assert_eq!(
            user,
            [
                ("py.app.main", vec![]),
                ("py.app.main", vec!["again".to_string()]),
            ]
        );
        let stdlib: Vec<_> = stdlib
            .iter()
            .map(|f| (f.name.as_str(), f.params.clone()))
            .collect();
        assert_eq!(
            stdlib,
            [
                ("py.json.dumps", vec!["first".to_string()]),
                ("py.os.path", vec![]),
            ]
        );
    }

    #[test]
    fn parse_stdlib_modules_accepts_sorted_set_authority() {
        let parsed = parse_stdlib_modules(r#"["sys","json.decoder"]"#).expect("valid modules");

        assert_eq!(
            parsed,
            BTreeSet::from(["json.decoder".to_string(), "sys".to_string()])
        );
    }

    #[test]
    fn parse_stdlib_modules_rejects_malformed_json() {
        let err = parse_stdlib_modules("not-json").expect_err("invalid modules");

        assert!(err.contains("MOLT_STDLIB_MODULES must be a JSON array of strings"));
    }

    #[test]
    fn parse_stdlib_modules_rejects_empty_module() {
        let err = parse_stdlib_modules(r#"["sys",""]"#).expect_err("empty module");

        assert_eq!(err, "MOLT_STDLIB_MODULES[1] must not be empty");
    }

    #[test]
    fn parse_stdlib_modules_rejects_duplicate_module() {
        let err = parse_stdlib_modules(r#"["sys","sys"]"#).expect_err("duplicate module");

        assert_eq!(err, r#"MOLT_STDLIB_MODULES[1] duplicates module "sys""#);
    }

    #[test]
    fn parse_stdlib_modules_rejects_empty_component() {
        let err = parse_stdlib_modules(r#"["json..decoder"]"#).expect_err("bad module");

        assert_eq!(
            err,
            r#"MOLT_STDLIB_MODULES[0] is not a dotted module name: "json..decoder""#
        );
    }

    #[test]
    fn ownership_reads_the_function_module_never_its_symbol() {
        let stdlib = BTreeSet::from(["json".to_string(), "sys".to_string()]);
        for (name, module, expected) in [
            // A symbol that spells a stdlib module still belongs to its record's module.
            ("py.json.encode", Some("app"), true),
            ("py.app.work", Some("json"), false),
            ("molt_host_init", None, true),
            ("pyinit.json", Some("json"), false),
            ("pyinit.app", Some("app"), true),
            ("pyinit.__main__", Some("__main__"), true),
            ("py.jsonish.work", Some("jsonish"), true),
        ] {
            assert_eq!(
                is_user_owned_function(&function(name, module, &[]), "app", Some(&stdlib)),
                expected,
                "{name}"
            );
        }
        assert!(!is_user_owned_function(
            &function("py.jsonish.work", Some("jsonish"), &[]),
            "app",
            None
        ));
    }
}
