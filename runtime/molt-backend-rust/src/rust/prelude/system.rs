use super::super::RustBackend;

impl RustBackend {
    pub(super) fn emit_system_runtime_prelude(&mut self, func_body: &str) {
        let used = |name: &str| func_body.contains(name);
        // sys target-version state. The frontend stamps this before user code,
        // and standalone Rust must preserve the same contract as native/WASM.
        let needs_module_cache = used("molt_module_cache_get(")
            || used("molt_module_cache_set(")
            || used("molt_module_cache_del(");
        self.output
            .push_str(include_str!("../python_version_policy_generated.rs"));
        self.output.push_str(
                r#"#[derive(Clone)]
struct MoltSysVersionInfo {
    major: i64,
    minor: i64,
    micro: i64,
    releaselevel: String,
    serial: i64,
}

impl Default for MoltSysVersionInfo {
    fn default() -> Self {
        let (major, minor, micro, releaselevel, serial) = DEFAULT_TARGET_PYTHON_VERSION;
        Self {
            major,
            minor,
            micro,
            releaselevel: releaselevel.to_string(),
            serial,
        }
    }
}

impl MoltSysVersionInfo {
    fn formatted_version(&self) -> String {
        let suffix = match self.releaselevel.as_str() {
            "alpha" => format!("a{}", self.serial),
            "beta" => format!("b{}", self.serial),
            "candidate" => format!("rc{}", self.serial),
            "final" | "" => String::new(),
            other => format!("{other}{}", self.serial),
        };
        format!("{}.{}.{}{} (molt)", self.major, self.minor, self.micro, suffix)
    }

    fn hexversion(&self) -> i64 {
        let release_nibble = match self.releaselevel.as_str() {
            "alpha" => 0xA,
            "beta" => 0xB,
            "candidate" => 0xC,
            "final" => 0xF,
            _ => 0xF,
        };
        ((self.major & 0xFF) << 24)
            | ((self.minor & 0xFF) << 16)
            | ((self.micro & 0xFF) << 8)
            | ((release_nibble & 0xF) << 4)
            | (self.serial & 0xF)
    }
}

fn molt_sys_version_state() -> &'static std::sync::Mutex<MoltSysVersionInfo> {
    static STATE: std::sync::OnceLock<std::sync::Mutex<MoltSysVersionInfo>> =
        std::sync::OnceLock::new();
    STATE.get_or_init(|| std::sync::Mutex::new(MoltSysVersionInfo::default()))
}

fn molt_runtime_target_at_least(major: i64, minor: i64) -> bool {
    let state = molt_sys_version_state().lock().unwrap().clone();
    (state.major, state.minor) >= (major, minor)
}

fn molt_sys_arg_int(args: &[MoltValue], index: usize, default: i64) -> i64 {
    args.get(index).map_or(default, molt_int)
}

fn molt_sys_arg_str(args: &[MoltValue], index: usize, default: &str) -> String {
    args.get(index)
        .map(|value| molt_str(value).to_utf8().unwrap_or_else(|_| panic!("ValueError: sys version metadata contains a surrogate")))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn molt_sys_set_version_info(args: &mut Vec<MoltValue>) -> MoltValue {
    assert_eq!(args.len(), 5, "sys version metadata requires five fields");
    let (major, minor, micro, level, serial) = DEFAULT_TARGET_PYTHON_VERSION;
    let next = MoltSysVersionInfo {
        major: molt_sys_arg_int(args, 0, major),
        minor: molt_sys_arg_int(args, 1, minor),
        micro: molt_sys_arg_int(args, 2, micro),
        releaselevel: molt_sys_arg_str(args, 3, level),
        serial: molt_sys_arg_int(args, 4, serial),
    };
    *molt_sys_version_state().lock().unwrap() = next;
    MoltValue::None
}

fn molt_sys_version_info(_args: &mut Vec<MoltValue>) -> MoltValue {
    let state = molt_sys_version_state().lock().unwrap().clone();
    MoltValue::List(vec![
        MoltValue::Int(state.major),
        MoltValue::Int(state.minor),
        MoltValue::Int(state.micro),
        MoltValue::Str(state.releaselevel.into()),
        MoltValue::Int(state.serial),
    ])
}

fn molt_sys_version(_args: &mut Vec<MoltValue>) -> MoltValue {
    let state = molt_sys_version_state().lock().unwrap().clone();
    MoltValue::Str(state.formatted_version().into())
}

fn molt_sys_hexversion(_args: &mut Vec<MoltValue>) -> MoltValue {
    let state = molt_sys_version_state().lock().unwrap().clone();
    MoltValue::Int(state.hexversion())
}

"#,
        );

        if needs_module_cache {
            self.output.push_str(concat!(
                "fn molt_module_cache() -> &'static std::sync::Mutex<std::collections::BTreeMap<PythonString, MoltValue>> {\n",
                "    static CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::BTreeMap<PythonString, MoltValue>>> = std::sync::OnceLock::new();\n",
                "    CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::BTreeMap::new()))\n",
                "}\n\n",
                "fn molt_module_cache_get(name: &MoltValue) -> MoltValue {\n",
                "    let key = molt_str(name);\n",
                "    molt_module_cache().lock().unwrap().get(&key).cloned().unwrap_or(MoltValue::None)\n",
                "}\n\n",
                "fn molt_module_cache_set(name: &MoltValue, module: MoltValue) -> MoltValue {\n",
                "    let key = molt_str(name);\n",
                "    let mut cache = molt_module_cache().lock().unwrap();\n",
                "    if let Some(existing) = cache.get(&key) {\n",
                "        if !matches!(existing, MoltValue::None) && existing != &module {\n",
                "            return existing.clone();\n",
                "        }\n",
                "    }\n",
                "    cache.insert(key, module);\n",
                "    MoltValue::None\n",
                "}\n\n",
                "fn molt_module_cache_del(name: &MoltValue) -> MoltValue {\n",
                "    let key = molt_str(name);\n",
                "    molt_module_cache().lock().unwrap().remove(&key);\n",
                "    MoltValue::None\n",
                "}\n\n",
            ));
        }
    }
}
