use super::*;

#[derive(Debug, Default, PartialEq, Eq)]
enum CompilationCache {
    #[default]
    Disabled,
    Default,
    Config(PathBuf),
}

#[derive(Debug)]
struct EngineOptions {
    max_stack: usize,
    async_stack: usize,
    cache: CompilationCache,
    serial: bool,
    fast: bool,
    deterministic: bool,
}

impl EngineOptions {
    fn from_lookup(lookup: impl Fn(&str) -> Option<std::ffi::OsString>) -> Result<Self> {
        if lookup("MOLT_WASM_PRECOMPILED_WRITE").is_some() {
            bail!("MOLT_WASM_PRECOMPILED_WRITE is retired; use molt-wasm-host --precompile");
        }
        let flag = |name: &str| -> Result<Option<bool>> {
            match lookup(name) {
                None => Ok(None),
                Some(value) if value == "0" => Ok(Some(false)),
                Some(value) if value == "1" => Ok(Some(true)),
                Some(_) => bail!("{name} must be 0 or 1"),
            }
        };
        let max_stack = match lookup("MOLT_WASM_MAX_STACK") {
            None => 8 * 1024 * 1024,
            Some(value) => value
                .to_str()
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|value| *value > 0)
                .context("MOLT_WASM_MAX_STACK must be a positive byte count fitting usize")?,
        };
        let async_stack = max_stack
            .checked_add(128 * 1024)
            .context("MOLT_WASM_MAX_STACK overflows async stack headroom")?;
        let cache = match (flag("MOLT_WASM_CACHE")?, lookup("MOLT_WASM_CACHE_CONFIG")) {
            (Some(false), _) => CompilationCache::Disabled,
            (_, Some(path)) if path.is_empty() => bail!("MOLT_WASM_CACHE_CONFIG must not be empty"),
            (_, Some(path)) => CompilationCache::Config(PathBuf::from(path)),
            (Some(true), None) => CompilationCache::Default,
            (None, None) => CompilationCache::Disabled,
        };
        Ok(Self {
            max_stack,
            async_stack,
            cache,
            serial: flag("MOLT_WASM_COMPILE_SERIAL")?.unwrap_or(false),
            fast: flag("MOLT_WASM_COMPILE_FAST")?.unwrap_or(false),
            deterministic: flag("MOLT_DETERMINISTIC")?.unwrap_or(false),
        })
    }

    fn build_config(&self) -> Result<Config> {
        let mut config = Config::new();
        // Wasmtime 43+ requires async_stack_size >= max_wasm_stack unconditionally.
        // Bump async_stack_size to accommodate, adding headroom for host-side frames.
        config.async_stack_size(self.async_stack);
        config.max_wasm_stack(self.max_stack);
        match &self.cache {
            CompilationCache::Disabled => {}
            CompilationCache::Default => {
                config.cache(Some(Cache::from_file(None)?));
            }
            CompilationCache::Config(path) => {
                config.cache(Some(Cache::from_file(Some(path))?));
            }
        }
        // Compiler scheduling is a resource policy, not a guest semantic policy.
        // Wasmtime collects parallel results in input order, including errors.
        config.parallel_compilation(!self.serial);
        if self.fast {
            config.cranelift_opt_level(OptLevel::None);
        }
        if self.deterministic {
            config.cranelift_nan_canonicalization(true);
            config.relaxed_simd_deterministic(true);
        }
        config.wasm_function_references(true);
        config.wasm_gc(true);
        Ok(config)
    }
}

pub(super) fn build_engine() -> Result<Engine> {
    let options = EngineOptions::from_lookup(|name| env::var_os(name))?;
    log::debug!("wasmtime engine options: {options:?}");
    Ok(Engine::new(&options.build_config()?)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(values: &[(&str, &str)]) -> Result<EngineOptions> {
        EngineOptions::from_lookup(|name| {
            values
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).into())
        })
    }

    #[test]
    fn engine_option_family_validates_values_and_stack_headroom() {
        let default = options(&[]).unwrap();
        assert_eq!(default.max_stack, 8 * 1024 * 1024);
        assert_eq!(default.async_stack, default.max_stack + 128 * 1024);
        assert_eq!(default.cache, CompilationCache::Disabled);
        assert!(!default.serial && !default.fast && !default.deterministic);
        assert!(
            options(&[("MOLT_WASM_PRECOMPILED_WRITE", "1")])
                .unwrap_err()
                .to_string()
                .contains("--precompile")
        );
        for name in [
            "MOLT_WASM_COMPILE_SERIAL",
            "MOLT_WASM_COMPILE_FAST",
            "MOLT_DETERMINISTIC",
            "MOLT_WASM_CACHE",
        ] {
            for value in ["", "true", "2"] {
                assert!(
                    options(&[(name, value)])
                        .unwrap_err()
                        .to_string()
                        .contains(name)
                );
            }
            assert!(options(&[(name, "0")]).is_ok());
            assert!(options(&[(name, "1")]).is_ok());
        }
        for value in ["0", "", "-1", "NaN", &usize::MAX.to_string()] {
            assert!(
                options(&[("MOLT_WASM_MAX_STACK", value)])
                    .unwrap_err()
                    .to_string()
                    .contains("MOLT_WASM_MAX_STACK")
            );
        }
        let custom = options(&[("MOLT_WASM_MAX_STACK", "1024")]).unwrap();
        assert_eq!(
            (custom.max_stack, custom.async_stack),
            (1024, 1024 + 128 * 1024)
        );
        assert_eq!(
            options(&[("MOLT_WASM_CACHE", "1")]).unwrap().cache,
            CompilationCache::Default
        );
        assert_eq!(
            options(&[("MOLT_WASM_CACHE_CONFIG", "cache.toml")])
                .unwrap()
                .cache,
            CompilationCache::Config("cache.toml".into())
        );
        assert!(options(&[("MOLT_WASM_CACHE_CONFIG", "")]).is_err());
        assert_eq!(
            options(&[
                ("MOLT_WASM_CACHE", "0"),
                ("MOLT_WASM_CACHE_CONFIG", "unused.toml")
            ])
            .unwrap()
            .cache,
            CompilationCache::Disabled
        );
        let deterministic = options(&[("MOLT_DETERMINISTIC", "1")]).unwrap();
        assert!(deterministic.deterministic);
        assert!(!deterministic.serial);
    }

    const NUMERIC_DETERMINISM: &str = r#"(module
        (func (export "nan") (param f64) (result i64)
            (i64.reinterpret_f64 (f64.add (local.get 0) (f64.const 1))))
        (func (export "swizzle") (param i32) (result i32)
            (i8x16.extract_lane_u 0
                (i8x16.relaxed_swizzle
                    (v128.const i8x16 42 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15)
                    (i8x16.splat (local.get 0))))))"#;

    fn assert_numeric_results(engine: &Engine, module: &Module) {
        let mut store = Store::new(engine, ());
        let instance = Instance::new(&mut store, module, &[]).unwrap();
        let nan = instance
            .get_typed_func::<f64, i64>(&mut store, "nan")
            .unwrap();
        assert_eq!(
            nan.call(&mut store, f64::from_bits(0x7ff1_2345_6789_abcd))
                .unwrap() as u64,
            0x7ff8_0000_0000_0000
        );
        let swizzle = instance
            .get_typed_func::<i32, i32>(&mut store, "swizzle")
            .unwrap();
        for (index, expected) in [(0, 42), (15, 15), (16, 0), (128, 0)] {
            assert_eq!(swizzle.call(&mut store, index).unwrap(), expected);
        }
    }

    #[test]
    fn deterministic_numeric_artifacts_and_execution_are_schedule_independent() {
        for fast in ["0", "1"] {
            let parallel = options(&[
                ("MOLT_DETERMINISTIC", "1"),
                ("MOLT_WASM_COMPILE_FAST", fast),
            ])
            .unwrap();
            let serial = options(&[
                ("MOLT_DETERMINISTIC", "1"),
                ("MOLT_WASM_COMPILE_FAST", fast),
                ("MOLT_WASM_COMPILE_SERIAL", "1"),
            ])
            .unwrap();
            let parallel = Engine::new(&parallel.build_config().unwrap()).unwrap();
            let serial = Engine::new(&serial.build_config().unwrap()).unwrap();
            let parallel_module = Module::new(&parallel, NUMERIC_DETERMINISM).unwrap();
            let serial_module = Module::new(&serial, NUMERIC_DETERMINISM).unwrap();
            let parallel_bytes = parallel_module.serialize().unwrap();
            let serial_bytes = serial_module.serialize().unwrap();
            assert_eq!(
                sha256_hex(&parallel_bytes),
                sha256_hex(&serial_bytes),
                "compiler scheduling changed artifact bytes"
            );
            assert_numeric_results(&parallel, &parallel_module);
            assert_numeric_results(&serial, &serial_module);
            // Both artifacts were just compiled by this process with equivalent
            // production configurations, so these are trusted native-code bytes.
            let cross_parallel = unsafe { Module::deserialize(&parallel, &serial_bytes) }.unwrap();
            let cross_serial = unsafe { Module::deserialize(&serial, &parallel_bytes) }.unwrap();
            assert_numeric_results(&parallel, &cross_parallel);
            assert_numeric_results(&serial, &cross_serial);
        }
    }

    #[test]
    fn old_relaxed_simd_artifacts_require_recompilation_for_deterministic_mode() {
        let options = options(&[("MOLT_DETERMINISTIC", "1")]).unwrap();
        let mut old_config = options.build_config().unwrap();
        old_config.relaxed_simd_deterministic(false);
        let old_engine = Engine::new(&old_config).unwrap();
        let old_bytes = Module::new(&old_engine, NUMERIC_DETERMINISM)
            .unwrap()
            .serialize()
            .unwrap();
        let new_engine = Engine::new(&options.build_config().unwrap()).unwrap();
        // Trusted bytes from this process, deliberately bound to an incompatible
        // engine. Never retry using a weaker semantic configuration.
        let error = unsafe { Module::deserialize(&new_engine, &old_bytes) }.unwrap_err();
        assert!(format!("{error:#}").contains("relaxed"));
    }
}
