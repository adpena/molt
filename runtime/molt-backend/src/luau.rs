//! Luau transpiler backend for Molt.
//!
//! Transpiles `SimpleIR` → Luau source code suitable for Roblox Studio.
//! Unlike the native/WASM backends that emit binary, this produces a `.luau`
//! text file that can be executed directly in Roblox's Luau VM.
//!
//! This backend is intentionally a preview target. Production build paths must
//! reject lowered output that still contains comment-only control-flow markers
//! or stub markers for unsupported semantics.

use crate::repr::{ContainerKind, ScalarKind};
use crate::representation_plan::ScalarRepresentationPlan;
use crate::{FunctionIR, OpIR, SimpleIR};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

#[path = "luau_backend/ir_rewrites.rs"]
mod ir_rewrites;
use ir_rewrites::{
    hoist_exception_edge_block_arg_stores, lower_early_returns, lower_iter_to_for,
    lower_try_to_pcall, strip_dead_after_return,
};

#[path = "luau_backend/source_postprocess.rs"]
mod source_postprocess;
use source_postprocess::optimize_luau_source;

#[path = "luau_backend/source_checks.rs"]
mod source_checks;
pub use source_checks::{review_luau_perf, validate_luau_source};

/// Transpiles Molt `SimpleIR` into Luau source text.
pub struct LuauBackend {
    output: String,
    /// Current indentation level (number of tabs).
    indent: usize,
    uses_forward_decls: bool,
    /// Variables that have been pre-declared at function scope and should use
    /// assignment (`var = val`) instead of `local var = val` in emit_op.
    hoisted_vars: BTreeSet<String>,
    /// Variables produced by `tuple_new` / `tuple_from_list` ops.  When one of
    /// these is returned we emit `return table.unpack(v)` so the caller
    /// receives multiple values instead of a single table.
    tuple_vars: BTreeSet<String>,
    /// Backend-neutral scalar representation facts for the function currently
    /// being emitted.
    scalar_plan: ScalarRepresentationPlan,
    /// Stack of pcall counter values for nested try/except blocks.
    try_depth_counter: Vec<u32>,
    /// Monotonically increasing counter for generating unique pcall variable names.
    pcall_counter: u32,
    /// True when we are inside a pcall body (between pcall_wrap_begin and
    /// pcall_wrap_end). exception_last should return nil in this zone.
    inside_pcall_body: bool,
    /// Variables known to hold non-negative integer constants.  Populated from
    /// `const` / `const_int` ops with `value >= 0`.  Used to skip the negative
    /// index ternary in get_item / set_item / del_item / string index paths.
    nonneg_consts: BTreeSet<String>,
    /// Counter of local declarations at function scope level 1 (inside the
    /// function body but not inside nested blocks).  Used to insert `do...end`
    /// scope blocks when nearing Luau's 200 local register limit.
    scope_local_count: u32,
    /// The indent level at which the function body sits (normally 1).
    /// Used to determine when we're at the top scope for local counting.
    func_body_indent: u32,
    /// True when we've opened a `do` block for local spilling.
    in_spill_do_block: bool,
    /// True when the current function needs local spilling (>190 ops with output).
    needs_local_spill: bool,
}

impl Default for LuauBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl LuauBackend {
    pub fn new() -> Self {
        Self {
            output: String::with_capacity(8192),
            indent: 0,
            uses_forward_decls: false,
            hoisted_vars: BTreeSet::new(),
            tuple_vars: BTreeSet::new(),
            scalar_plan: ScalarRepresentationPlan::default(),
            try_depth_counter: Vec::new(),
            pcall_counter: 0,
            inside_pcall_body: false,
            nonneg_consts: BTreeSet::new(),
            scope_local_count: 0,
            func_body_indent: 1,
            in_spill_do_block: false,
            needs_local_spill: false,
        }
    }

    /// Compile the given IR to a Luau source string.
    pub fn compile(&mut self, ir: &SimpleIR) -> String {
        // Phase 1: Emit all function bodies to a temporary buffer so we can
        // scan which runtime helpers are actually referenced.
        let emit_funcs: Vec<&FunctionIR> = ir
            .functions
            .iter()
            .filter(|f| !f.name.contains("__annotate__"))
            .collect();

        let mut func_output = String::with_capacity(8192);
        std::mem::swap(&mut self.output, &mut func_output);

        // Collect the set of function names that will be defined in this
        // compilation unit.  Any identifier referenced but NOT in this set
        // needs a forward declaration (or it will be an undeclared global in
        // Luau, causing a parse error).
        let defined_names: std::collections::HashSet<String> =
            emit_funcs.iter().map(|f| sanitize_ident(&f.name)).collect();

        // Scan all ops for identifiers that reference module-chunk
        // initializer functions not present in the function list.
        // These come from:
        //   - call_internal: s_value is the callee name
        //   - load_local: var field may hold a function reference
        //     (e.g. `builtins.__require_importlib_util_module`)
        //   - call_func: args[0] is the callee
        let mut extra_decls_set = std::collections::HashSet::new();
        let mut extra_forward_decls: Vec<String> = Vec::new();
        for func in &emit_funcs {
            for op in &func.ops {
                let mut check_ident = |raw: &str| {
                    let ident = sanitize_ident(raw);
                    // Skip temp vars (v0, v123, etc.), internal names, and
                    // runtime helpers — they are already declared elsewhere.
                    let is_temp_var =
                        ident.starts_with('v') && ident[1..].chars().all(|c| c.is_ascii_digit());
                    if !is_temp_var
                        && !defined_names.contains(&ident)
                        && !extra_decls_set.contains(&ident)
                        && !ident.starts_with("__")
                        && !ident.starts_with("molt_")
                    {
                        extra_decls_set.insert(ident.clone());
                        extra_forward_decls.push(ident);
                    }
                };
                // call_internal: s_value is the callee name
                if op.kind == "call_internal"
                    && let Some(ref s_val) = op.s_value
                {
                    check_ident(s_val);
                }
                // func_new / func_new_closure / code_new: s_value is the
                // function name emitted as a bare identifier reference.
                if matches!(
                    op.kind.as_str(),
                    "func_new" | "func_new_closure" | "code_new"
                ) && let Some(ref s_val) = op.s_value
                {
                    check_ident(s_val);
                }
                // load_local: var field may hold a function reference
                if op.kind == "load_local"
                    && let Some(ref var) = op.var
                {
                    check_ident(var);
                }
                // call_func: args[0] is the callee
                if op.kind == "call_func"
                    && let Some(ref args) = op.args
                    && let Some(callee) = args.first()
                {
                    check_ident(callee);
                }
            }
        }

        if emit_funcs.len() > 1 || !extra_forward_decls.is_empty() {
            let total_decls = emit_funcs.len() + extra_forward_decls.len();
            if total_decls <= 180 {
                // Small enough: use local forward declarations.
                self.uses_forward_decls = true;
                self.emit_line("-- Forward declarations");
                for func in &emit_funcs {
                    let name = sanitize_ident(&func.name);
                    self.emit_line(&format!("local {name}"));
                }
                for name in &extra_forward_decls {
                    self.emit_line(&format!("local {name}"));
                }
            } else {
                // Too many functions for local declarations.  Skip forward
                // declarations entirely — functions will be assigned as globals.
                // This avoids the 200 local register limit.
                self.uses_forward_decls = true;
            }
            self.output.push('\n');
        }

        for func in &emit_funcs {
            eprintln!("Luau: emitting function {}", func.name);
            self.emit_function_body(func);
            self.output.push('\n');
        }

        self.emit_line("-- Entry point");
        self.emit_line("if molt_main then");
        self.push_indent();
        self.emit_line("molt_main()");
        self.pop_indent();
        self.emit_line("end");

        let mut func_body = std::mem::take(&mut self.output);
        self.output = func_output;

        // Phase 2: Run text-level optimizations on the function body BEFORE
        // scanning for prelude helpers. Inlining passes may eliminate helper
        // calls, leaving dead definitions if we scan before optimization.
        optimize_luau_source(&mut func_body);

        // Phase 3: Emit prelude with only the helpers that survive optimization.
        self.emit_prelude_conditional(&func_body);

        // Phase 4: Combine prelude + optimized function bodies.
        self.output.push_str(&func_body);

        std::mem::take(&mut self.output)
    }

    /// Compile via the IR pipeline with validation and performance review.
    ///
    /// This path is intentionally fail-closed: preview/IR-pipeline builds must
    /// not emit unchecked Luau when validation discovers unsupported semantics.
    pub fn compile_via_ir(&mut self, ir: &SimpleIR) -> Result<String, String> {
        self.compile_checked(ir)
    }

    /// Compile the given IR and reject preview-blocker markers that would
    /// otherwise silently emit syntactically valid but semantically incomplete
    /// Luau.
    pub fn compile_checked(&mut self, ir: &SimpleIR) -> Result<String, String> {
        let source = self.compile(ir);
        validate_luau_source(&source)?;

        // Performance review — report remaining opportunities to stderr.
        let perf_issues = review_luau_perf(&source);
        if !perf_issues.is_empty() {
            eprintln!(
                "[molt-luau] Performance review ({} issue{}):",
                perf_issues.len(),
                if perf_issues.len() == 1 { "" } else { "s" }
            );
            for (ln, cat, msg) in perf_issues.iter().take(20) {
                eprintln!("  L{ln} [{cat}] {msg}");
            }
            if perf_issues.len() > 20 {
                eprintln!("  ... {} more", perf_issues.len() - 20);
            }
        } else {
            eprintln!("[molt-luau] Performance review: clean — no issues found");
        }

        Ok(source)
    }

    fn emit_prelude_conditional(&mut self, func_body: &str) {
        // Always-emitted header.
        self.output.push_str(
            "--!native\n--!strict\n-- Molt -> Luau transpiled output\n-- Runtime helpers\n\n",
        );
        self.output
            .push_str("local molt_func_attrs: {[any]: {[string]: any}} = {}\n");
        self.output.push_str("local molt_module_cache: {[string]: any} = {\n\tmath = nil,\n\tjson = nil,\n\ttime = nil,\n\tos = nil,\n}\n\n");

        let needs_luau_module_import = func_body.contains("molt_luau_import_module(");
        let needs_sys_bootstrap = func_body.contains("molt_sys_set_version_info(")
            || func_body.contains("molt_sys_ensure_module(")
            || needs_luau_module_import;
        if needs_sys_bootstrap {
            self.output.push_str(concat!(
                "local molt_sys_version_info = {3, 12, 0, \"final\", 0}\n",
                "local molt_sys_version = \"3.12.0 (molt)\"\n",
                "local molt_sys_hexversion = 0x030c00f0\n\n",
                "local function molt_sys_release_nibble(releaselevel)\n",
                "\tif releaselevel == \"alpha\" then return 0xA end\n",
                "\tif releaselevel == \"beta\" then return 0xB end\n",
                "\tif releaselevel == \"candidate\" then return 0xC end\n",
                "\treturn 0xF\n",
                "end\n\n",
                "local function molt_sys_format_version(major, minor, micro, releaselevel, serial)\n",
                "\tlocal suffix = \"\"\n",
                "\tif releaselevel == \"alpha\" then suffix = \"a\" .. tostring(serial) end\n",
                "\tif releaselevel == \"beta\" then suffix = \"b\" .. tostring(serial) end\n",
                "\tif releaselevel == \"candidate\" then suffix = \"rc\" .. tostring(serial) end\n",
                "\tif releaselevel ~= \"final\" and releaselevel ~= \"\" and suffix == \"\" then suffix = tostring(releaselevel) .. tostring(serial) end\n",
                "\treturn tostring(major) .. \".\" .. tostring(minor) .. \".\" .. tostring(micro) .. suffix .. \" (molt)\"\n",
                "end\n\n",
                "local function molt_sys_compute_hexversion(major, minor, micro, releaselevel, serial)\n",
                "\treturn major * 0x1000000 + minor * 0x10000 + micro * 0x100 + molt_sys_release_nibble(releaselevel) * 0x10 + serial\n",
                "end\n\n",
                "local function molt_sys_seed_module()\n",
                "\tlocal sys_module = {\n",
                "\t\tversion_info = molt_sys_version_info,\n",
                "\t\tversion = molt_sys_version,\n",
                "\t\thexversion = molt_sys_hexversion,\n",
                "\t}\n",
                "\tmolt_module_cache[\"sys\"] = sys_module\n",
                "\treturn sys_module\n",
                "end\n\n",
                "local function molt_sys_ensure_module()\n",
                "\tlocal sys_module = molt_module_cache[\"sys\"]\n",
                "\tif sys_module == nil then\n",
                "\t\treturn molt_sys_seed_module()\n",
                "\tend\n",
                "\treturn sys_module\n",
                "end\n\n",
                "local function molt_sys_set_version_info(major, minor, micro, releaselevel, serial, version)\n",
                "\tmajor = major or 3\n",
                "\tminor = minor or 12\n",
                "\tmicro = micro or 0\n",
                "\treleaselevel = releaselevel or \"final\"\n",
                "\tserial = serial or 0\n",
                "\tif version == nil or version == \"\" then\n",
                "\t\tversion = molt_sys_format_version(major, minor, micro, releaselevel, serial)\n",
                "\tend\n",
                "\tmolt_sys_version_info = {major, minor, micro, releaselevel, serial}\n",
                "\tmolt_sys_version = version\n",
                "\tmolt_sys_hexversion = molt_sys_compute_hexversion(major, minor, micro, releaselevel, serial)\n",
                "\tmolt_sys_seed_module()\n",
                "\treturn nil\n",
                "end\n\n",
            ));
        }

        if needs_luau_module_import {
            self.output.push_str(concat!(
                "local function molt_luau_import_module(name)\n",
                "\tif name == \"sys\" then\n",
                "\t\treturn molt_sys_ensure_module()\n",
                "\tend\n",
                "\tlocal module = molt_module_cache[name]\n",
                "\tif module ~= nil then\n",
                "\t\treturn module\n",
                "\tend\n",
                "\terror(\"unsupported module import in Luau backend: \" .. tostring(name))\n",
                "end\n\n",
            ));
        }

        // Runtime intrinsic stubs — bootstrap functions from the native
        // runtime that are no-ops in Luau transpiled output.
        for stub in &[
            "molt_init_sys",
            "molt_runtime_shutdown",
            "molt_runtime_init",
        ] {
            if func_body.contains(stub) {
                self.output
                    .push_str(&format!("local function {stub}(...) end\n"));
            }
        }

        // Helper to check if a name is used in the function body.
        // We search for "name(" to match call sites, avoiding false positives
        // like "molt_mod" matching inside "molt_module_cache".
        let used_call = |name: &str| {
            let pattern = format!("{name}(");
            func_body.contains(&pattern)
        };
        // For non-function names (modules, variables), use plain contains.
        let used = |name: &str| func_body.contains(name);

        if used_call("molt_module_get_global")
            || used_call("molt_module_get_name")
            || used_call("molt_module_del_global")
        {
            self.output.push_str(concat!(
                "local function molt_module_type_error(action: string): any\n",
                "\terror({__type = \"TypeError\", __msg = \"module \" .. action .. \" expects module\"})\n",
                "end\n\n",
                "local function molt_module_name_error(name: any): any\n",
                "\tlocal name_s = tostring(name)\n",
                "\tif name_s == \"exec\" or name_s == \"eval\" then\n",
                "\t\terror({__type = \"RuntimeError\", __msg = \"MOLT_COMPAT_ERROR: \" .. name_s .. \"() is unsupported in compiled Molt binaries; dynamic code execution is outside the verified subset. Use static modules or pre-generated code paths instead.\"})\n",
                "\tend\n",
                "\terror({__type = \"NameError\", __msg = \"name '\" .. name_s .. \"' is not defined\"})\n",
                "end\n\n",
                "local function molt_module_get_global(module: any, name: any): any\n",
                "\tif type(module) ~= \"table\" then return molt_module_type_error(\"get_global\") end\n",
                "\tlocal value = module[name]\n",
                "\tif value ~= nil then return value end\n",
                "\tlocal builtins = molt_module_cache[\"builtins\"]\n",
                "\tif type(builtins) == \"table\" then\n",
                "\t\tlocal builtin_value = builtins[name]\n",
                "\t\tif builtin_value ~= nil then return builtin_value end\n",
                "\tend\n",
                "\treturn molt_module_name_error(name)\n",
                "end\n\n",
                "local function molt_module_get_name(module: any, name: any): any\n",
                "\tif type(module) ~= \"table\" then return molt_module_type_error(\"get_name\") end\n",
                "\tlocal value = module[name]\n",
                "\tif value ~= nil then return value end\n",
                "\terror({__type = \"AttributeError\", __msg = \"module has no attribute '\" .. tostring(name) .. \"'\"})\n",
                "end\n\n",
                "local function molt_module_del_global(module: any, name: any, missing_ok: boolean): any\n",
                "\tif type(module) ~= \"table\" then return molt_module_type_error(\"del_global\") end\n",
                "\tif module[name] ~= nil then\n",
                "\t\tmodule[name] = nil\n",
                "\t\treturn nil\n",
                "\tend\n",
                "\tif missing_ok then return nil end\n",
                "\treturn molt_module_name_error(name)\n",
                "end\n\n",
            ));
        }

        // Conditional runtime helpers — only emit if referenced by call.
        // Each helper is a (name, source) pair.
        let helpers: &[(&str, &str)] = &[
            (
                "molt_range",
                "@native\nlocal function molt_range(start: number, stop: number, step: number?): {number}\n\tlocal s = step or 1\n\tlocal result = table.create(math.max(0, math.ceil((stop - start) / s)))\n\tlocal n = 0\n\tlocal i = start\n\twhile (s > 0 and i < stop) or (s < 0 and i > stop) do\n\t\tn += 1\n\t\tresult[n] = i\n\t\ti += s\n\tend\n\treturn result\nend\n",
            ),
            (
                "molt_len",
                "local function molt_len(obj: any): number\n\tif type(obj) == \"string\" then return #obj end\n\tif type(obj) == \"table\" then\n\t\tlocal n = #obj\n\t\tif n > 0 or next(obj) == nil then return n end\n\t\tlocal c = 0; for _ in pairs(obj) do c += 1 end; return c\n\tend\n\terror(\"TypeError: object of type '\" .. type(obj) .. \"' has no len()\")\nend\n",
            ),
            (
                "molt_int",
                "local function molt_int(x: any): number\n\treturn math.floor(tonumber(x) or 0)\nend\n",
            ),
            (
                "molt_float",
                "local function molt_float(x: any): number\n\treturn tonumber(x) or 0.0\nend\n",
            ),
            (
                "molt_str",
                "local function molt_str(x: any): string\n\tif type(x) == \"table\" then\n\t\tlocal n = #x\n\t\tif n > 0 or next(x) == nil then\n\t\t\tlocal parts = table.create(n)\n\t\t\tfor i = 1, n do parts[i] = molt_str(x[i]) end\n\t\t\treturn \"[\" .. table.concat(parts, \", \") .. \"]\"\n\t\telse\n\t\t\tlocal parts = {}\n\t\t\tlocal m = 0\n\t\t\tfor k, v in pairs(x) do m += 1; parts[m] = molt_repr(k) .. \": \" .. molt_str(v) end\n\t\t\treturn \"{\" .. table.concat(parts, \", \") .. \"}\"\n\t\tend\n\tend\n\tif type(x) == \"boolean\" then return x and \"True\" or \"False\" end\n\tif x == nil then return \"None\" end\n\treturn tostring(x)\nend\n",
            ),
            (
                "molt_bool",
                "local function molt_bool(x: any): boolean\n\tif x == nil or x == false or x == 0 or x == \"\" then return false end\n\tif type(x) == \"table\" and next(x) == nil then return false end\n\treturn true\nend\n",
            ),
            (
                "molt_builtin_type",
                "local function molt_builtin_type(tag: any): {[string]: any}\n\tif type(tag) ~= \"number\" then error({__type=\"TypeError\", __msg=\"builtin type tag must be int\"}) end\n\tlocal name = nil\n\tif tag == 1 then name = \"int\"\n\telseif tag == 2 then name = \"float\"\n\telseif tag == 3 then name = \"bool\"\n\telseif tag == 5 then name = \"str\"\n\telseif tag == 6 then name = \"bytes\"\n\telseif tag == 7 then name = \"bytearray\"\n\telseif tag == 8 then name = \"list\"\n\telseif tag == 9 then name = \"tuple\"\n\telseif tag == 10 then name = \"dict\"\n\telseif tag == 11 then name = \"range\"\n\telseif tag == 12 then name = \"slice\"\n\telseif tag == 15 then name = \"memoryview\"\n\telseif tag == 17 then name = \"set\"\n\telseif tag == 18 then name = \"frozenset\"\n\telseif tag == 100 then name = \"object\"\n\telseif tag == 101 then name = \"type\"\n\telseif tag == 102 then name = \"BaseException\"\n\telseif tag == 103 then name = \"Exception\"\n\telseif tag == 226 then name = \"classmethod\"\n\telseif tag == 227 then name = \"staticmethod\"\n\telseif tag == 228 then name = \"property\"\n\telseif tag == 229 then name = \"super\"\n\telse error({__type=\"TypeError\", __msg=\"unknown builtin type tag\"}) end\n\treturn {__name__ = name, __molt_builtin_type_tag = tag, __molt_is_type = true}\nend\n",
            ),
            (
                "molt_type_of",
                "local function molt_type_of(x: any): {[string]: any}\n\tif type(x) == \"table\" and x.__type then return {__name__ = x.__type, __molt_is_type = true} end\n\tif type(x) == \"table\" then\n\t\tif x.__molt_is_type then return molt_builtin_type(101) end\n\t\tlocal mt = getmetatable(x)\n\t\tif type(mt) == \"table\" and mt.__molt_is_type then return mt end\n\tend\n\tlocal t = type(x)\n\tif t == \"nil\" then return {__name__ = \"NoneType\", __molt_is_type = true} end\n\tif t == \"number\" then return molt_builtin_type(1) end\n\tif t == \"string\" then return molt_builtin_type(5) end\n\tif t == \"boolean\" then return molt_builtin_type(3) end\n\tif t == \"function\" then return {__name__ = \"function\", __molt_is_type = true} end\n\treturn {__name__ = t, __molt_is_type = true}\nend\n",
            ),
            (
                "molt_issubclass",
                "local function molt_issubclass(sub: any, classinfo: any): boolean\n\tif type(classinfo) == \"table\" and classinfo.__molt_is_type ~= true then\n\t\tfor i = 1, #classinfo do\n\t\t\tif molt_issubclass(sub, classinfo[i]) then return true end\n\t\tend\n\t\treturn false\n\tend\n\tif type(sub) ~= \"table\" or sub.__molt_is_type ~= true then error({__type=\"TypeError\", __msg=\"issubclass() arg 1 must be a class\"}) end\n\tif type(classinfo) ~= \"table\" or classinfo.__molt_is_type ~= true then error({__type=\"TypeError\", __msg=\"issubclass() arg 2 must be a class or tuple of classes\"}) end\n\tlocal class_tag = classinfo.__molt_builtin_type_tag\n\tlocal sub_tag = sub.__molt_builtin_type_tag\n\tif class_tag == 100 then return true end\n\tif sub == classinfo then return true end\n\tif sub_tag ~= nil and class_tag ~= nil then\n\t\tif sub_tag == class_tag then return true end\n\t\tif sub_tag == 3 and class_tag == 1 then return true end\n\t\tif sub_tag == 103 and class_tag == 102 then return true end\n\t\treturn false\n\tend\n\tlocal current = sub\n\tlocal seen = {}\n\twhile type(current) == \"table\" and current.__molt_is_type == true do\n\t\tif current == classinfo then return true end\n\t\tif seen[current] then return false end\n\t\tseen[current] = true\n\t\tlocal mt = getmetatable(current)\n\t\tif type(mt) ~= \"table\" or type(mt.__index) ~= \"table\" then return false end\n\t\tcurrent = mt.__index\n\tend\n\treturn false\nend\n",
            ),
            (
                "molt_isinstance",
                "local function molt_isinstance(obj: any, classinfo: any): boolean\n\tif type(classinfo) == \"table\" and classinfo.__molt_is_type ~= true then\n\t\tfor i = 1, #classinfo do\n\t\t\tif molt_isinstance(obj, classinfo[i]) then return true end\n\t\tend\n\t\treturn false\n\tend\n\treturn molt_issubclass(molt_type_of(obj), classinfo)\nend\n",
            ),
            (
                "molt_get_attr",
                "local function molt_class_lookup(cls: any, attr: any): any\n\tlocal current = cls\n\tlocal seen = {}\n\twhile type(current) == \"table\" and seen[current] ~= true do\n\t\tseen[current] = true\n\t\tlocal raw = rawget(current, attr)\n\t\tif raw ~= nil then return raw end\n\t\tlocal mt = getmetatable(current)\n\t\tif type(mt) ~= \"table\" or type(mt.__index) ~= \"table\" then return nil end\n\t\tcurrent = mt.__index\n\tend\n\treturn nil\nend\n\nlocal function molt_bind_attr(obj: any, owner: any, raw: any): any\n\tif type(raw) == \"table\" then\n\t\tlocal kind = raw.__molt_descriptor_kind\n\t\tif kind == \"staticmethod\" then return raw.__func end\n\t\tif kind == \"classmethod\" then\n\t\t\tlocal func = raw.__func\n\t\t\treturn function(...) return func(owner, ...) end\n\t\tend\n\t\tif kind == \"property\" then\n\t\t\tif obj == owner then return raw end\n\t\t\tlocal fget = raw.__get\n\t\t\tif fget == nil then error({__type=\"AttributeError\", __msg=\"unreadable attribute\"}) end\n\t\t\treturn fget(obj)\n\t\tend\n\tend\n\tif type(raw) == \"function\" and type(owner) == \"table\" and obj ~= owner then\n\t\treturn function(...) return raw(obj, ...) end\n\tend\n\treturn raw\nend\n\nlocal function molt_get_attr(obj: any, attr: any): any\n\tif type(obj) ~= \"table\" then return nil end\n\tif obj.__molt_is_type == true then\n\t\tlocal raw = molt_class_lookup(obj, attr)\n\t\tif raw ~= nil then return molt_bind_attr(obj, obj, raw) end\n\t\treturn nil\n\tend\n\tlocal own = rawget(obj, attr)\n\tif own ~= nil then return own end\n\tlocal cls = getmetatable(obj)\n\tif type(cls) == \"table\" then\n\t\tlocal raw = molt_class_lookup(cls, attr)\n\t\tif raw ~= nil then return molt_bind_attr(obj, cls, raw) end\n\tend\n\treturn obj[attr]\nend\n\nlocal function molt_get_attr_default(obj: any, attr: any, default: any): any\n\tlocal value = molt_get_attr(obj, attr)\n\tif value ~= nil then return value end\n\treturn default\nend\n\nlocal function molt_has_attr(obj: any, attr: any): boolean\n\tlocal ok, value = pcall(function() return molt_get_attr(obj, attr) end)\n\tif ok then return value ~= nil end\n\tif type(value) == \"table\" and value.__type == \"AttributeError\" then return false end\n\terror(value)\nend\n\nlocal function molt_set_attr(obj: any, attr: any, value: any): nil\n\tif type(obj) ~= \"table\" then return nil end\n\tif obj.__molt_is_type ~= true then\n\t\tlocal cls = getmetatable(obj)\n\t\tif type(cls) == \"table\" then\n\t\t\tlocal raw = molt_class_lookup(cls, attr)\n\t\t\tif type(raw) == \"table\" and raw.__molt_descriptor_kind == \"property\" then\n\t\t\t\tlocal fset = raw.__set\n\t\t\t\tif fset == nil then error({__type=\"AttributeError\", __msg=\"can't set attribute\"}) end\n\t\t\t\tfset(obj, value)\n\t\t\t\treturn nil\n\t\t\tend\n\t\tend\n\tend\n\tobj[attr] = value\n\treturn nil\nend\n\nlocal function molt_del_attr(obj: any, attr: any): nil\n\tif type(obj) ~= \"table\" then return nil end\n\tif obj.__molt_is_type ~= true then\n\t\tlocal cls = getmetatable(obj)\n\t\tif type(cls) == \"table\" then\n\t\t\tlocal raw = molt_class_lookup(cls, attr)\n\t\t\tif type(raw) == \"table\" and raw.__molt_descriptor_kind == \"property\" then\n\t\t\t\tlocal fdel = raw.__del\n\t\t\t\tif fdel == nil then error({__type=\"AttributeError\", __msg=\"can't delete attribute\"}) end\n\t\t\t\tfdel(obj)\n\t\t\t\treturn nil\n\t\t\tend\n\t\tend\n\tend\n\tobj[attr] = nil\n\treturn nil\nend\n",
            ),
            (
                "molt_class_apply_set_name",
                "local function molt_class_apply_set_name(cls: any): nil\n\tif type(cls) ~= \"table\" or cls.__molt_is_type ~= true then return nil end\n\tlocal entries = {}\n\tlocal count = 0\n\tfor name, value in pairs(cls) do\n\t\tif name ~= \"__index\" and name ~= \"__molt_is_type\" and (type(name) ~= \"string\" or string.sub(name, 1, 7) ~= \"__molt_\") then\n\t\t\tcount += 1\n\t\t\tentries[count] = {name, value}\n\t\tend\n\tend\n\tfor i = 1, count do\n\t\tlocal entry = entries[i]\n\t\tlocal name = entry[1]\n\t\tlocal value = entry[2]\n\t\tlocal hook = molt_get_attr(value, \"__set_name__\")\n\t\tif hook ~= nil then hook(cls, name) end\n\tend\n\treturn nil\nend\n",
            ),
            (
                "molt_matmul",
                "local function molt_matmul_impl(a: any, b: any, op: string): any\n\tlocal lhs = molt_get_attr(a, \"__matmul__\")\n\tif lhs ~= nil then\n\t\tlocal result = lhs(b)\n\t\tif result ~= molt_not_implemented then return result end\n\tend\n\tlocal rhs = molt_get_attr(b, \"__rmatmul__\")\n\tif rhs ~= nil then\n\t\tlocal result = rhs(a)\n\t\tif result ~= molt_not_implemented then return result end\n\tend\n\terror({__type=\"TypeError\", __msg=\"unsupported operand type(s) for \" .. op .. \": '\" .. type(a) .. \"' and '\" .. type(b) .. \"'\"})\nend\n\nlocal function molt_matmul(a: any, b: any): any\n\treturn molt_matmul_impl(a, b, \"@\")\nend\n",
            ),
            (
                "molt_inplace_matmul",
                "local function molt_inplace_matmul(a: any, b: any): any\n\tlocal lhs = molt_get_attr(a, \"__imatmul__\")\n\tif lhs ~= nil then\n\t\tlocal result = lhs(b)\n\t\tif result ~= molt_not_implemented then return result end\n\tend\n\treturn molt_matmul_impl(a, b, \"@=\")\nend\n",
            ),
            (
                "molt_guard_type",
                "local function molt_guard_type(val: any, expected: any): any\n\tif type(expected) ~= \"number\" then error({__type=\"TypeError\", __msg=\"guard type tag must be int\"}) end\n\treturn val\nend\n",
            ),
            (
                "molt_repr",
                "local function molt_repr(x: any): string\n\tif type(x) == \"string\" then return \"'\" .. x .. \"'\" end\n\tif type(x) == \"table\" then return molt_str(x) end\n\tif type(x) == \"boolean\" then return x and \"True\" or \"False\" end\n\tif x == nil then return \"None\" end\n\treturn tostring(x)\nend\n",
            ),
            (
                "molt_floor_div",
                "local function molt_floor_div(a: number, b: number): number\n\treturn a // b\nend\n",
            ),
            (
                "molt_pow",
                "local function molt_pow(a: number, b: number): number\n\treturn a ^ b\nend\n",
            ),
            (
                "molt_mod",
                "local function molt_mod(a: number, b: number): number\n\treturn a % b\nend\n",
            ),
            (
                "molt_enumerate",
                "local function molt_enumerate(t: {any}, start: number?): {{any}}\n\tlocal len = #t\n\tlocal result = table.create(len)\n\tlocal s = start or 0\n\tfor i = 1, len do\n\t\tresult[i] = {s + i - 1, t[i]}\n\tend\n\treturn result\nend\n",
            ),
            (
                "molt_zip",
                "local function molt_zip(a: {any}, b: {any}): {{any}}\n\tlocal result = {}\n\tlocal len = math.min(#a, #b)\n\tfor i = 1, len do\n\t\tresult[i] = {a[i], b[i]}\n\tend\n\treturn result\nend\n",
            ),
            (
                "molt_sorted",
                "local function molt_sorted(t: {any}): {any}\n\tlocal copy = table.clone(t)\n\ttable.sort(copy)\n\treturn copy\nend\n",
            ),
            (
                "molt_reversed",
                "@native\nlocal function molt_reversed(t: {any}): {any}\n\tlocal len = #t\n\tlocal result = table.create(len)\n\tfor i = 1, len do\n\t\tresult[i] = t[len - i + 1]\n\tend\n\treturn result\nend\n",
            ),
            (
                "molt_sum",
                "@native\nlocal function molt_sum(t: {number}, start: number?): number\n\tlocal s = start or 0\n\tfor __i = 1, #t do s += t[__i] end\n\treturn s\nend\n",
            ),
            (
                "molt_any",
                "local function molt_any(t: {any}): boolean\n\tfor __i = 1, #t do\n\t\tif molt_bool(t[__i]) then return true end\n\tend\n\treturn false\nend\n",
            ),
            (
                "molt_all",
                "local function molt_all(t: {any}): boolean\n\tfor __i = 1, #t do\n\t\tif not molt_bool(t[__i]) then return false end\n\tend\n\treturn true\nend\n",
            ),
            (
                "molt_map",
                "@native\nlocal function molt_map(func: (any) -> any, t: {any}): {any}\n\tlocal len = #t\n\tlocal result = table.create(len)\n\tfor i = 1, len do\n\t\tresult[i] = func(t[i])\n\tend\n\treturn result\nend\n",
            ),
            (
                "molt_filter",
                "local function molt_filter(func: ((any) -> boolean)?, t: {any}): {any}\n\tlocal result = {}\n\tlocal n = 0\n\tfor __i = 1, #t do\n\t\tlocal v = t[__i]\n\t\tif func then\n\t\t\tif func(v) then n += 1; result[n] = v end\n\t\telseif molt_bool(v) then\n\t\t\tn += 1; result[n] = v\n\t\tend\n\tend\n\treturn result\nend\n",
            ),
            (
                "molt_dict_keys",
                "local function molt_dict_keys(d: {[any]: any}): {any}\n\tlocal result = {}\n\tlocal n = 0\n\tfor k in pairs(d) do n += 1; result[n] = k end\n\treturn result\nend\n",
            ),
            (
                "molt_dict_values",
                "local function molt_dict_values(d: {[any]: any}): {any}\n\tlocal result = {}\n\tlocal n = 0\n\tfor _, v in pairs(d) do n += 1; result[n] = v end\n\treturn result\nend\n",
            ),
            (
                "molt_dict_items",
                "local function molt_dict_items(d: {[any]: any}): {{any}}\n\tlocal result = {}\n\tlocal n = 0\n\tfor k, v in pairs(d) do n += 1; result[n] = {k, v} end\n\treturn result\nend\n",
            ),
            (
                "molt_string_split_ws_dict_inc",
                "local function molt_string_split_ws_dict_inc(line: any, dict: any, delta: any): {any}\n\tif type(line) ~= \"string\" then error({__type=\"TypeError\", __msg=\"split expects str\"}) end\n\tif type(dict) ~= \"table\" then error({__type=\"TypeError\", __msg=\"dict increment expects dict\"}) end\n\tlocal last: any = nil\n\tlocal had_any = false\n\tfor token in string.gmatch(line, \"%S+\") do\n\t\tlocal current = dict[token]\n\t\tif current == nil then current = 0 end\n\t\tdict[token] = current + delta\n\t\tlast = token\n\t\thad_any = true\n\tend\n\treturn {last, had_any}\nend\n",
            ),
            (
                "molt_string_split_sep_dict_inc",
                "local function molt_string_split_sep_dict_inc(line: any, sep: any, dict: any, delta: any): {any}\n\tif type(line) ~= \"string\" then error({__type=\"TypeError\", __msg=\"split expects str\"}) end\n\tif type(sep) ~= \"string\" then error({__type=\"TypeError\", __msg=\"must be str or None\"}) end\n\tif type(dict) ~= \"table\" then error({__type=\"TypeError\", __msg=\"dict increment expects dict\"}) end\n\tif sep == \"\" then error({__type=\"ValueError\", __msg=\"empty separator\"}) end\n\tlocal last: any = nil\n\tlocal had_any = false\n\tlocal pos = 1\n\twhile true do\n\t\tlocal i, j = string.find(line, sep, pos, true)\n\t\tlocal token\n\t\tif i then\n\t\t\ttoken = string.sub(line, pos, i - 1)\n\t\t\tpos = j + 1\n\t\telse\n\t\t\ttoken = string.sub(line, pos)\n\t\tend\n\t\tlocal current = dict[token]\n\t\tif current == nil then current = 0 end\n\t\tdict[token] = current + delta\n\t\tlast = token\n\t\thad_any = true\n\t\tif not i then break end\n\tend\n\treturn {last, had_any}\nend\n",
            ),
            (
                "molt_taq_ingest_line",
                "local function molt_taq_parse_i64_field(field: string): number\n\tlocal trimmed = string.match(field, \"^%s*(.-)%s*$\")\n\tif trimmed == nil or trimmed == \"\" then error({__type=\"ValueError\", __msg=\"invalid literal for int() with base 10: ''\"}) end\n\tif string.match(trimmed, \"^[+-]?%d+$\") == nil then error({__type=\"ValueError\", __msg=\"invalid literal for int() with base 10: '\" .. trimmed .. \"'\"}) end\n\treturn tonumber(trimmed) :: number\nend\n\nlocal function molt_taq_div_euclid(a: number, b: number): number\n\tlocal q = if a >= 0 then math.floor(a / b) else math.ceil(a / b)\n\tlocal r = a - q * b\n\tif r < 0 then\n\t\tif b > 0 then q -= 1 else q += 1 end\n\tend\n\treturn q\nend\n\nlocal function molt_taq_ingest_line(dict: any, line: any, bucket_size: any): boolean\n\tif type(dict) ~= \"table\" then error({__type=\"TypeError\", __msg=\"TAQ ingest expects dict\"}) end\n\tif type(line) ~= \"string\" then error({__type=\"TypeError\", __msg=\"TAQ ingest expects str\"}) end\n\tif type(bucket_size) ~= \"number\" then error({__type=\"TypeError\", __msg=\"TAQ ingest expects integer bucket size\"}) end\n\tif bucket_size == 0 then error({__type=\"ZeroDivisionError\", __msg=\"integer division or modulo by zero\"}) end\n\tlocal fields = {}\n\tlocal field_count = 0\n\tlocal pos = 1\n\twhile true do\n\t\tlocal i, j = string.find(line, \"|\", pos, true)\n\t\tfield_count += 1\n\t\tif i then\n\t\t\tfields[field_count] = string.sub(line, pos, i - 1)\n\t\t\tpos = j + 1\n\t\telse\n\t\t\tfields[field_count] = string.sub(line, pos)\n\t\t\tbreak\n\t\tend\n\tend\n\tlocal ts_field = fields[1]\n\tlocal sym_field = fields[3]\n\tlocal vol_field = fields[5]\n\tif ts_field == nil or sym_field == nil or vol_field == nil then error({__type=\"IndexError\", __msg=\"list index out of range\"}) end\n\tif ts_field == \"END\" or vol_field == \"ENDP\" then return false end\n\tlocal timestamp = molt_taq_parse_i64_field(ts_field)\n\tlocal volume = molt_taq_parse_i64_field(vol_field)\n\tlocal series = dict[sym_field]\n\tif series == nil then\n\t\tseries = {}\n\t\tdict[sym_field] = series\n\tend\n\tif type(series) ~= \"table\" then error({__type=\"TypeError\", __msg=\"TAQ ingest bucket must be list\"}) end\n\tseries[#series + 1] = {molt_taq_div_euclid(timestamp, bucket_size), volume}\n\treturn true\nend\n",
            ),
            (
                "molt_print",
                "local function molt_print(...)\n\tlocal n = select(\"#\", ...)\n\tif n == 0 then print(); return end\n\tif n == 1 then print(molt_str((...))) return end\n\tlocal parts = table.create(n)\n\tfor i = 1, n do\n\t\tparts[i] = molt_str((select(i, ...)))\n\tend\n\tprint(table.concat(parts, \" \"))\nend\n",
            ),
            (
                "molt_str_codepoint_len",
                "local function molt_str_codepoint_len(s: string): number\n\tlocal len = utf8.len(s)\n\tif len == nil then error({__type=\"UnicodeDecodeError\", __msg=\"invalid UTF-8 string\"}) end\n\treturn len\nend\n",
            ),
            (
                "molt_str_byte_offset",
                "local function molt_str_byte_offset(s: string, idx: number): number\n\tlocal offset = utf8.offset(s, idx)\n\tif offset == nil then error({__type=\"IndexError\", __msg=\"string index out of range\"}) end\n\treturn offset\nend\n",
            ),
            (
                "molt_ord",
                "local function molt_ord(ch: any): number\n\tif type(ch) ~= \"string\" then error({__type=\"TypeError\", __msg=\"ord() expected string of length 1, but \" .. type(ch) .. \" found\"}) end\n\tlocal len = molt_str_codepoint_len(ch)\n\tif len ~= 1 then error({__type=\"TypeError\", __msg=\"ord() expected a character, but string of length \" .. tostring(len) .. \" found\"}) end\n\tlocal code = utf8.codepoint(ch, 1)\n\tif code == nil then error({__type=\"UnicodeDecodeError\", __msg=\"invalid UTF-8 string\"}) end\n\treturn code\nend\n",
            ),
            (
                // CheckedAdd's Luau lowering (docs/design/foundation/
                // 15_luau-checkedadd-plan.md): Luau numbers are f64 — `+`
                // never wraps i64, it rounds — so the i64-overflow flag is
                // ALWAYS false here and the overflow_peel slow path is
                // correctly dead. The sum is the same f64 addition the bare
                // `add` op already emits, so peeled and un-peeled Luau output
                // are identical (precision above 2^53 is the pre-existing
                // Luau number-model bound, not a peel regression).
                "molt_checked_i64_add",
                "@native\nlocal function molt_checked_i64_add(a: number, b: number): (number, boolean)\n\treturn a + b, false\nend\n",
            ),
            (
                // CheckedMul's Luau lowering. Luau numbers are f64, so a
                // structural `return a * b, false` would be a SILENT WRONG
                // ANSWER: an integer product whose magnitude reaches 2^53
                // loses mantissa bits, yet the bare `mul` would report it as
                // exact and the overflow_peel slow (boxed BigInt) loop would
                // never run. Instead the flag is CONSERVATIVE: it is `true`
                // whenever exactness cannot be proven, forcing the sound boxed
                // slow loop. Soundness: if `|a*b| < 2^53` the IEEE product of
                // two integer-valued operands is computed EXACTLY (the exact
                // integer is representable and multiply is correctly rounded),
                // so flag=false is safe; once the (already-rounded) product
                // reaches 2^53 in magnitude precision may have been lost, so
                // flag=true conservatively re-routes to BigInt. (Products of
                // magnitude exactly 2^53 take the boxed path too — sound, just
                // mildly pessimistic; the boxed path yields the same value.)
                "molt_checked_i64_mul",
                "@native\nlocal function molt_checked_i64_mul(a: number, b: number): (number, boolean)\n\tlocal p = a * b\n\tif p >= 9007199254740992 or p <= -9007199254740992 then\n\t\treturn p, true\n\tend\n\treturn p, false\nend\n",
            ),
            (
                "molt_ord_at",
                "local function molt_ord_at(obj: any, key: any): number\n\tif type(obj) ~= \"string\" then\n\t\tif type(obj) == \"table\" then\n\t\t\tlocal table_key = key\n\t\t\tif type(key) == \"boolean\" then\n\t\t\t\ttable_key = if key then 2 else 1\n\t\t\telseif type(key) == \"number\" then\n\t\t\t\ttable_key = if key >= 0 then key + 1 else #obj + key + 1\n\t\t\tend\n\t\t\treturn molt_ord(obj[table_key])\n\t\tend\n\t\terror({__type=\"TypeError\", __msg=\"'\" .. type(obj) .. \"' object is not subscriptable\"})\n\tend\n\tlocal key_num = key\n\tif type(key) == \"boolean\" then key_num = if key then 1 else 0 end\n\tif type(key_num) ~= \"number\" then error({__type=\"TypeError\", __msg=\"string indices must be integers, not '\" .. type(key_num) .. \"'\"}) end\n\tlocal len = molt_str_codepoint_len(obj)\n\tlocal idx = if key_num >= 0 then key_num + 1 else len + key_num + 1\n\tif idx < 1 or idx > len then error({__type=\"IndexError\", __msg=\"string index out of range\"}) end\n\tlocal byte_idx = molt_str_byte_offset(obj, idx)\n\tlocal code = utf8.codepoint(obj, byte_idx)\n\tif code == nil then error({__type=\"UnicodeDecodeError\", __msg=\"invalid UTF-8 string\"}) end\n\treturn code\nend\n",
            ),
        ];

        // Dependency: molt_str ↔ molt_repr are mutually recursive for table
        // serialization. molt_print depends on molt_str. If any is used, emit all linked.
        // Luau `local function` is NOT hoisted, so we need a forward declaration for
        // molt_repr before molt_str (which calls molt_repr in its body).
        let needs_print = used_call("molt_print");
        let needs_str = used_call("molt_str") || needs_print;
        let needs_repr = used_call("molt_repr");
        let needs_str_group = needs_str || needs_repr;
        let needs_ord_group = used_call("molt_ord")
            || used_call("molt_ord_at")
            || used_call("molt_str_codepoint_len")
            || used_call("molt_str_byte_offset");
        let needs_builtin_type = used_call("molt_builtin_type")
            || used_call("molt_type_of")
            || used_call("molt_isinstance");
        let needs_type_of = used_call("molt_type_of") || used_call("molt_isinstance");
        let needs_issubclass = used_call("molt_issubclass") || used_call("molt_isinstance");
        let needs_matmul_group = used_call("molt_matmul") || used_call("molt_inplace_matmul");
        let needs_not_implemented = used("molt_not_implemented") || needs_matmul_group;
        let needs_get_attr = used_call("molt_get_attr")
            || used_call("molt_get_attr_default")
            || used_call("molt_has_attr")
            || used_call("molt_set_attr")
            || used_call("molt_del_attr")
            || used_call("molt_class_apply_set_name")
            || needs_matmul_group;
        if needs_str_group {
            self.output.push_str("local molt_repr\n");
        }
        if needs_not_implemented {
            self.output
                .push_str("local molt_not_implemented = {__molt_not_implemented = true}\n");
        }
        for (name, source) in helpers {
            let emit = if matches!(*name, "molt_str" | "molt_repr") {
                needs_str_group
            } else if matches!(
                *name,
                "molt_str_codepoint_len" | "molt_str_byte_offset" | "molt_ord" | "molt_ord_at"
            ) {
                needs_ord_group
            } else if *name == "molt_builtin_type" {
                needs_builtin_type
            } else if *name == "molt_type_of" {
                needs_type_of
            } else if *name == "molt_issubclass" {
                needs_issubclass
            } else if *name == "molt_isinstance" {
                used_call("molt_isinstance")
            } else if *name == "molt_get_attr" {
                needs_get_attr
            } else if *name == "molt_matmul" {
                needs_matmul_group
            } else if *name == "molt_inplace_matmul" {
                used_call("molt_inplace_matmul")
            } else if *name == "molt_print" {
                needs_print
            } else {
                used_call(name)
            };
            if emit {
                // molt_repr uses assignment form since it was forward-declared.
                if *name == "molt_repr" && needs_str_group {
                    let assigned =
                        source.replace("local function molt_repr(", "molt_repr = function(");
                    self.output.push_str(&assigned);
                } else {
                    self.output.push_str(source);
                }
                self.output.push('\n');
            }
        }

        // Exception handling helpers — only emit if pcall-based try/except is used.
        if used_call("molt_exception_kind") || used_call("molt_exception_match") {
            self.output.push_str(concat!(
                "local molt_exception_hierarchy = {\n",
                "\tZeroDivisionError = \"ArithmeticError\",\n",
                "\tOverflowError = \"ArithmeticError\",\n",
                "\tFloatingPointError = \"ArithmeticError\",\n",
                "\tArithmeticError = \"Exception\",\n",
                "\tValueError = \"Exception\",\n",
                "\tTypeError = \"Exception\",\n",
                "\tKeyError = \"LookupError\",\n",
                "\tIndexError = \"LookupError\",\n",
                "\tLookupError = \"Exception\",\n",
                "\tAttributeError = \"Exception\",\n",
                "\tNameError = \"Exception\",\n",
                "\tRuntimeError = \"Exception\",\n",
                "\tNotImplementedError = \"RuntimeError\",\n",
                "\tRecursionError = \"RuntimeError\",\n",
                "\tStopIteration = \"Exception\",\n",
                "\tFileNotFoundError = \"OSError\",\n",
                "\tPermissionError = \"OSError\",\n",
                "\tOSError = \"Exception\",\n",
                "\tIOError = \"OSError\",\n",
                "\tImportError = \"Exception\",\n",
                "\tModuleNotFoundError = \"ImportError\",\n",
                "\tStopAsyncIteration = \"Exception\",\n",
                "\tAssertionError = \"Exception\",\n",
                "\tUnicodeError = \"ValueError\",\n",
                "\tUnicodeDecodeError = \"UnicodeError\",\n",
                "\tUnicodeEncodeError = \"UnicodeError\",\n",
                "\tConnectionError = \"OSError\",\n",
                "\tBrokenPipeError = \"ConnectionError\",\n",
                "\tConnectionRefusedError = \"ConnectionError\",\n",
                "\tConnectionResetError = \"ConnectionError\",\n",
                "\tConnectionAbortedError = \"ConnectionError\",\n",
                "\tTimeoutError = \"OSError\",\n",
                "\tChildProcessError = \"OSError\",\n",
                "\tProcessLookupError = \"OSError\",\n",
                "\tBlockingIOError = \"OSError\",\n",
                "\tInterruptedError = \"OSError\",\n",
                "\tIsADirectoryError = \"OSError\",\n",
                "\tNotADirectoryError = \"OSError\",\n",
                "\tFileExistsError = \"OSError\",\n",
                "\tEOFError = \"Exception\",\n",
                "\tUnboundLocalError = \"NameError\",\n",
                "\tSyntaxError = \"Exception\",\n",
                "\tIndentationError = \"SyntaxError\",\n",
                "\tSystemExit = \"BaseException\",\n",
                "\tKeyboardInterrupt = \"BaseException\",\n",
                "\tGeneratorExit = \"BaseException\",\n",
                "\tException = \"BaseException\",\n",
                "\tBaseException = nil,\n",
                "}\n\n",
            ));
            self.output.push_str(concat!(
                "local function molt_exception_kind(e: any): string\n",
                "\tif type(e) == \"table\" and e.__type then return e.__type end\n",
                "\tif type(e) == \"string\" then\n",
                "\t\tif string.find(e, \"attempt to perform arithmetic\") or string.find(e, \"divide by zero\") or string.find(e, \"division by zero\") then return \"ZeroDivisionError\" end\n",
                "\t\tif string.find(e, \"attempt to index\") or string.find(e, \"is not a valid member\") then return \"AttributeError\" end\n",
                "\t\tif string.find(e, \"invalid argument\") or string.find(e, \"expected\") then return \"TypeError\" end\n",
                "\t\treturn \"Exception\"\n",
                "\tend\n",
                "\treturn \"Exception\"\n",
                "end\n\n",
            ));
            self.output.push_str(concat!(
                "local function molt_exception_match(e: any, class_name: string): boolean\n",
                "\tlocal kind = molt_exception_kind(e)\n",
                "\twhile kind do\n",
                "\t\tif kind == class_name then return true end\n",
                "\t\tkind = molt_exception_hierarchy[kind]\n",
                "\tend\n",
                "\treturn false\n",
                "end\n\n",
            ));
        }

        // Infrastructure used by JSON serializer and math/bitwise ops.
        // math_floor is needed by the JSON prelude (serialize checks integer-ness).
        let needs_json = used("molt_json_dumps") || used("\"json\"");
        if used("math_floor") || needs_json {
            self.output.push_str("local math_floor = math.floor\n");
        }
        if used("bit32") || used("bit.") {
            self.output.push_str("local bit = bit32 or bit\n");
        }
        if used("molt_missing_sentinel") {
            self.output.push_str("local molt_missing_sentinel = {}\n");
        }
        self.output.push('\n');

        // Math module bridge — emit if any function references math module.
        // Detection: molt_math (static path) or module_cache["math"] (dynamic path)
        // or .floor/.sqrt/.ceil etc. via module attribute access.
        if used("molt_math") || used("\"math\"") {
            self.output.push_str(concat!(
                "local molt_math = {\n",
                "\tfloor = math.floor,\n\tceil = math.ceil,\n\tsqrt = math.sqrt,\n",
                "\tabs = math.abs,\n\tsin = math.sin,\n\tcos = math.cos,\n",
                "\ttan = math.tan,\n\tasin = math.asin,\n\tacos = math.acos,\n",
                "\tatan = math.atan,\n\tatan2 = math.atan2,\n\texp = math.exp,\n",
                "\tlog = math.log,\n\tlog10 = function(x) return math.log(x, 10) end,\n\tpi = math.pi,\n",
                "\te = 2.718281828459045,\n\tinf = math.huge,\n\tnan = 0/0,\n",
                "}\n\n",
            ));
            self.output
                .push_str("molt_module_cache[\"math\"] = molt_math\n\n");
        }

        // JSON serializer — emit if any function references json module.
        if used("molt_json_dumps") || used("\"json\"") {
            self.output.push_str(include_str!("luau_json_prelude.luau"));
            self.output.push('\n');
            self.output
                .push_str("molt_module_cache[\"json\"] = json\n\n");
        }

        // Time module bridge — emit if any function references time module.
        if used("molt_time") || used("\"time\"") {
            self.output.push_str(concat!(
                "local molt_time = {\n",
                "\ttime = os.clock,\n\tperf_counter = os.clock,\n",
                "\tmonotonic = os.clock,\n\tsleep = function(s: number) task.wait(s) end,\n",
                "}\n\n",
            ));
            self.output
                .push_str("molt_module_cache[\"time\"] = molt_time\n\n");
        }

        // OS module bridge — emit if any function references os module.
        if used("molt_os") || used("\"os\"") {
            self.output.push_str(concat!(
                "local molt_os = {\n",
                "\tgetcwd = function() return \".\" end,\n",
                "\tgetenv = function(k: string) return nil end,\n",
                "\tpath = { join = function(...) local a = {...} return table.concat(a, \"/\") end,\n",
                "\t\texists = function() return false end, sep = \"/\" },\n",
                "}\n\n",
            ));
            self.output
                .push_str("molt_module_cache[\"os\"] = molt_os\n\n");
        }

        // String method helpers.
        if used("molt_string.") {
            self.output.push_str(concat!(
                "local molt_string = {\n",
                "\tformat = string.format,\n",
                "\tjoin = function(sep: string, t: {string}): string\n\t\treturn table.concat(t, sep)\n\tend,\n",
                "\tsplit = function(s: string, sep: string?): {string}\n",
                "\t\tlocal result = {}\n\t\tlocal n = 0\n\t\tlocal pattern = sep and sep or \"%s+\"\n",
                "\t\tif sep then\n\t\t\tif sep == \"\" then error({__type=\"ValueError\", __msg=\"empty separator\"}) end\n\t\t\tlocal pos = 1\n\t\t\twhile pos <= #s do\n",
                "\t\t\t\tlocal i, j = string.find(s, pattern, pos, true)\n",
                "\t\t\t\tif i then\n\t\t\t\t\tn += 1; result[n] = string.sub(s, pos, i - 1)\n",
                "\t\t\t\t\tpos = j + 1\n\t\t\t\telse\n",
                "\t\t\t\t\tn += 1; result[n] = string.sub(s, pos)\n\t\t\t\t\tbreak\n",
                "\t\t\t\tend\n\t\t\tend\n\t\telse\n",
                "\t\t\tfor w in string.gmatch(s, \"%S+\") do\n\t\t\t\tn += 1; result[n] = w\n",
                "\t\t\tend\n\t\tend\n\t\treturn result\n\tend,\n",
                "\tsplit_validate = function(s: string, sep: string): nil\n",
                "\t\tif type(s) ~= \"string\" then error({__type=\"TypeError\", __msg=\"descriptor 'split' for 'str' objects doesn't apply to a '\" .. type(s) .. \"' object\"}) end\n",
                "\t\tif type(sep) ~= \"string\" then error({__type=\"TypeError\", __msg=\"must be str or None, not \" .. type(sep)}) end\n",
                "\t\tif sep == \"\" then error({__type=\"ValueError\", __msg=\"empty separator\"}) end\n\t\treturn nil\n\tend,\n",
                "\tsplit_field = function(s: string, sep: string, idx: number): string\n",
                "\t\tmolt_string.split_validate(s, sep)\n",
                "\t\tif idx < 0 then error({__type=\"IndexError\", __msg=\"list index out of range\"}) end\n",
                "\t\tlocal pos = 1\n\t\tlocal field = 0\n\t\twhile true do\n",
                "\t\t\tlocal i, j = string.find(s, sep, pos, true)\n",
                "\t\t\tif i then\n\t\t\t\tif field == idx then return string.sub(s, pos, i - 1) end\n\t\t\t\tpos = j + 1\n\t\t\t\tfield += 1\n\t\t\telse\n\t\t\t\tif field == idx then return string.sub(s, pos) end\n\t\t\t\tbreak\n\t\t\tend\n\t\tend\n",
                "\t\terror({__type=\"IndexError\", __msg=\"list index out of range\"})\n\tend,\n",
                "\tsplit_field_len = function(s: string, sep: string, idx: number): number\n",
                "\t\treturn string.len(molt_string.split_field(s, sep, idx))\n\tend,\n",
                "\tsplit_field_eq = function(s: string, sep: string, idx: number, expected: string): boolean\n",
                "\t\treturn molt_string.split_field(s, sep, idx) == expected\n\tend,\n}\n\n",
            ));
        }
    }

    fn emit_function_body(&mut self, func: &FunctionIR) {
        // Pre-process: lower early returns (store+jump→ret) then strip dead code.
        let ops = lower_early_returns(&func.ops);
        let ops = strip_dead_after_return(&ops);
        let ops = lower_iter_to_for(&ops);
        let ops = hoist_exception_edge_block_arg_stores(&ops);
        let (ops, pcall_escaped_vars) = lower_try_to_pcall(&ops);
        let scalar_func = FunctionIR {
            name: func.name.clone(),
            params: func.params.clone(),
            ops: ops.clone(),
            param_types: func.param_types.clone(),
            source_file: func.source_file.clone(),
            is_extern: func.is_extern,
        };
        self.scalar_plan = ScalarRepresentationPlan::for_function_ir(&scalar_func);

        // Build typed parameter list.  When `param_types` carries per-param
        // type hints from the frontend we emit Luau type annotations so the
        // native JIT can skip runtime type guards.
        let typed_params: Vec<String> = func
            .params
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let ident = sanitize_ident(p);
                let luau_ty = func
                    .param_types
                    .as_ref()
                    .and_then(|pts| pts.get(i))
                    .map(|t| python_type_to_luau(t))
                    .unwrap_or("any");
                format!("{ident}: {luau_ty}")
            })
            .collect();
        let params = typed_params.join(", ");

        let name = sanitize_ident(&func.name);
        if self.uses_forward_decls {
            // Forward-declared assignment form — @native is not supported on
            // bare `name = function(` in Luau, so we skip the attribute here.
            let _ = writeln!(self.output, "{name} = function({params})");
        } else {
            // Emit @native attribute to enable Luau's native codegen.  This is
            // zero-cost when the JIT is off and enables specialisation when it
            // is on.  Type-annotated parameters further allow the JIT to skip
            // runtime type guards.
            self.output.push_str("@native\n");
            let _ = writeln!(self.output, "local function {name}({params})");
        }
        self.push_indent();

        // Mark position for post-processing hoisted var declarations.
        let func_start = self.output.len();

        // Reset per-function state.
        self.hoisted_vars.clear();
        self.tuple_vars.clear();
        self.try_depth_counter.clear();
        self.pcall_counter = 0;
        self.inside_pcall_body = false;
        self.nonneg_consts.clear();
        self.scope_local_count = 0;
        self.func_body_indent = self.indent as u32;
        self.in_spill_do_block = false;
        // Pre-count ops that will produce `local` declarations.
        // If > 190, enable local-spilling `do...end` blocks.
        let local_producing_ops = ops
            .iter()
            .filter(|op| op.out.is_some() && op.out.as_deref() != Some("none"))
            .count();
        self.needs_local_spill = local_producing_ops > 190;

        // Pre-declare loop index variables so they persist across iterations.
        let mut loop_idx_vars = Vec::new();
        for op in &ops {
            if op.kind == "loop_index_start"
                && let Some(ref out_name) = op.out
            {
                loop_idx_vars.push(sanitize_ident(out_name));
            }
        }
        if !loop_idx_vars.is_empty() {
            for var in &loop_idx_vars {
                self.emit_line(&format!("local {var}"));
            }
        }

        // Pre-declare closure slot variables used by closure_store/closure_load.
        // These are generator/coroutine state variables that must persist across
        // loop iterations and function calls.
        {
            let mut closure_slots: Vec<String> = Vec::new();
            for op in &ops {
                if (op.kind == "closure_store" || op.kind == "closure_load")
                    && let Some(ref args) = op.args
                    && let Some(slot) = args.first()
                {
                    let var_name = format!("__closure_{}", sanitize_ident(slot));
                    if !closure_slots.contains(&var_name) {
                        closure_slots.push(var_name);
                    }
                }
            }
            for var in &closure_slots {
                self.emit_line(&format!("local {var}"));
            }
        }

        // Pre-scan: collect variables defined by const/const_int with non-negative values.
        for op in &ops {
            match op.kind.as_str() {
                "const" | "const_int" => {
                    if let (Some(out_name), Some(v)) = (&op.out, op.value)
                        && v >= 0
                    {
                        self.nonneg_consts.insert(out_name.clone());
                    }
                }
                _ => {}
            }
        }

        // Phi hoisting: find `end_if` followed by `phi` ops and collect
        // the phi output variables.  Also find variables first declared
        // inside if/else blocks but referenced outside (scope escape).
        let mut phi_assignments: BTreeMap<usize, Vec<(String, Vec<String>)>> = BTreeMap::new();
        {
            // Pass 1: find phi ops that follow end_if and record their
            // output vars plus branch values.
            let mut i = 0;
            while i < ops.len() {
                if ops[i].kind == "end_if" {
                    // Scan forward for consecutive phi ops.
                    let end_if_idx = i;
                    let mut j = i + 1;
                    while j < ops.len() && ops[j].kind == "phi" {
                        if let Some(ref out_name) = ops[j].out {
                            let phi_var = sanitize_ident(out_name);
                            let args: Vec<String> = ops[j]
                                .args
                                .as_deref()
                                .unwrap_or(&[])
                                .iter()
                                .map(|a| sanitize_ident(a))
                                .collect();
                            phi_assignments
                                .entry(end_if_idx)
                                .or_default()
                                .push((phi_var.clone(), args));
                            self.hoisted_vars.insert(phi_var);
                        }
                        j += 1;
                    }
                }
                i += 1;
            }

            // Pass 2: find variables first declared inside if/else/loop
            // blocks but used outside, OR declared in one block and used
            // in a different block at the same depth (e.g., two sequential
            // while loops). Track (depth, block_id) pairs.
            let mut depth: i32 = 0;
            let mut block_id: u32 = 0;
            let mut decl_scope: BTreeMap<String, (i32, u32)> = BTreeMap::new();
            let param_set: BTreeSet<String> =
                func.params.iter().map(|p| sanitize_ident(p)).collect();

            for op in &ops {
                match op.kind.as_str() {
                    "if" | "loop_start" | "for_range" | "for_iter" | "pcall_wrap_begin" => {
                        depth += 1;
                        block_id += 1;
                    }
                    "else" => {
                        // else starts a new block at the same depth
                        block_id += 1;
                    }
                    "end_if" | "loop_end" | "end_for" | "pcall_wrap_end" => {
                        depth -= 1;
                        block_id += 1;
                    }
                    _ => {}
                }
                // Record first declaration site of each variable.
                if let Some(ref out_name) = op.out
                    && out_name != "none"
                    && !op.kind.starts_with("nop")
                {
                    let var = sanitize_ident(out_name);
                    decl_scope.entry(var).or_insert((depth, block_id));
                }
                // Check if any referenced variable was declared at a deeper
                // depth OR in a different block at the same depth.
                let refs: Vec<&str> = op
                    .args
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(|s| s.as_str())
                    .chain(op.var.as_deref())
                    .collect();
                for r in refs {
                    let var = sanitize_ident(r);
                    if param_set.contains(&var) {
                        continue;
                    }
                    if let Some(&(dd, db)) = decl_scope.get(&var) {
                        // Hoist if: declared deeper, OR declared at same
                        // depth but in a different block (different loop/if).
                        if dd > depth || (dd > 0 && dd == depth && db != block_id) {
                            self.hoisted_vars.insert(var);
                        }
                    }
                }
            }
        }

        // Add pcall-escaped variables to hoisted set so they use assignment
        // form instead of `local` inside the pcall closure.
        for escaped_var in &pcall_escaped_vars {
            self.hoisted_vars.insert(sanitize_ident(escaped_var));
        }
        // TIR store_var/load_var represent named storage slots that must remain
        // visible across structured control-flow edges in the emitted function.
        for op in &ops {
            if op.kind == "store_var"
                && let Some(name) = op.var.as_deref().or(op.out.as_deref())
            {
                self.hoisted_vars.insert(sanitize_ident(name));
            }
        }

        // Emit pre-declarations for hoisted variables.  Cap at 150 to stay
        // within Luau's ~200 local register limit.  Variables beyond the cap
        // are removed from hoisted_vars so they get `local` declarations
        // inline (which Luau handles as new inner-scope bindings).
        if !self.hoisted_vars.is_empty() {
            let mut sorted: Vec<String> = self.hoisted_vars.iter().cloned().collect();
            sorted.sort();
            let cap = 150;
            if sorted.len() > cap {
                let overflow: Vec<String> = sorted.drain(cap..).collect();
                for var in &overflow {
                    self.hoisted_vars.remove(var);
                }
            }
            for var in &sorted {
                self.emit_line(&format!("local {var}"));
            }
        }

        // Build a map: for each if block, record the phi assignments to
        // inject into true/false branches.  We need to find the matching
        // if/else/end_if structure for each phi group.
        // Strategy: walk ops, track if/else/end_if nesting, and for each
        // end_if that has phi_assignments, record the injection points.
        //
        // For a pattern: if(idx_a) ... else(idx_b) ... end_if(idx_c) phi
        // We inject:
        //   - at end of true branch (just before else): phi_var = args[0]
        //   - at end of false branch (just before end_if): phi_var = args[1]
        //
        // We track: for each end_if index with phis, find the matching
        // if and else indices.
        let mut phi_inject_before_else: BTreeMap<usize, Vec<(String, String)>> = BTreeMap::new();
        let mut phi_inject_before_end_if: BTreeMap<usize, Vec<(String, String)>> = BTreeMap::new();
        // For if-without-else + phi, we need to synthesize an else branch.
        // Track: end_if_idx → Vec<(phi_var, false_val)>
        let mut phi_synthesize_else: BTreeMap<usize, Vec<(String, String)>> = BTreeMap::new();
        if !phi_assignments.is_empty() {
            // Walk ops to find if/else/end_if triples.
            let mut if_stack: Vec<(usize, Option<usize>)> = Vec::new(); // (if_idx, else_idx)
            for (idx, op) in ops.iter().enumerate() {
                match op.kind.as_str() {
                    "if" => {
                        if_stack.push((idx, None));
                    }
                    "else" => {
                        if let Some(last) = if_stack.last_mut() {
                            last.1 = Some(idx);
                        }
                    }
                    "end_if" => {
                        if let Some((if_idx, else_idx)) = if_stack.pop()
                            && let Some(phis) = phi_assignments.get(&idx)
                        {
                            for (phi_var, args) in phis {
                                if let Some(else_i) = else_idx {
                                    // True branch value: inject before else.
                                    let true_val =
                                        args.first().cloned().unwrap_or_else(|| "nil".to_string());
                                    phi_inject_before_else
                                        .entry(else_i)
                                        .or_default()
                                        .push((phi_var.clone(), true_val));
                                    // False branch value: inject before end_if.
                                    let false_val =
                                        args.get(1).cloned().unwrap_or_else(|| "nil".to_string());
                                    phi_inject_before_end_if
                                        .entry(idx)
                                        .or_default()
                                        .push((phi_var.clone(), false_val));
                                } else {
                                    // No else branch — this is the `and` short-circuit
                                    // pattern.  The true branch sets the phi from
                                    // args[0].  When false, the phi should get the
                                    // if-condition variable (the LHS of `and`).
                                    let true_val =
                                        args.first().cloned().unwrap_or_else(|| "nil".to_string());
                                    phi_inject_before_end_if
                                        .entry(idx)
                                        .or_default()
                                        .push((phi_var.clone(), true_val));
                                    // Extract the condition variable from the `if` op.
                                    let cond_var = ops[if_idx]
                                        .args
                                        .as_deref()
                                        .and_then(|a| a.first())
                                        .map(|s| sanitize_ident(s))
                                        .unwrap_or_else(|| "nil".to_string());
                                    phi_synthesize_else
                                        .entry(idx)
                                        .or_default()
                                        .push((phi_var.clone(), cond_var));
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        // Emit ops with phi injection and loop_start handling.
        let mut i = 0;
        while i < ops.len() {
            // Inject phi true-branch assignments before else.
            if let Some(injects) = phi_inject_before_else.get(&i) {
                for (var, val) in injects {
                    self.emit_line(&format!("{var} = {val}"));
                }
            }
            // Inject phi false-branch assignments before end_if.
            if let Some(injects) = phi_inject_before_end_if.get(&i) {
                for (var, val) in injects {
                    self.emit_line(&format!("{var} = {val}"));
                }
            }
            // Synthesize else branch for if-without-else + phi (and pattern).
            // This assigns the condition variable when the if body was skipped.
            if ops[i].kind == "end_if"
                && let Some(synth) = phi_synthesize_else.get(&i)
            {
                self.pop_indent();
                self.emit_line("else");
                self.push_indent();
                for (var, cond_val) in synth {
                    self.emit_line(&format!("{var} = {cond_val}"));
                }
            }

            if ops[i].kind == "loop_start"
                && i + 1 < ops.len()
                && ops[i + 1].kind == "loop_index_start"
            {
                let idx_op = &ops[i + 1];
                if let Some(ref out_name) = idx_op.out {
                    let out = sanitize_ident(out_name);
                    let args = idx_op.args.as_deref().unwrap_or(&[]);
                    if let Some(start_val) = args.first() {
                        let start = sanitize_ident(start_val);
                        self.emit_line(&format!("{out} = {start}"));
                    } else {
                        self.emit_line(&format!("{out} = 0"));
                    }
                }
                self.emit_op(&ops[i]);
                i += 2;
            } else {
                self.emit_op(&ops[i]);
                i += 1;
            }
        }

        self.pop_indent();
        self.emit_line("end");

        // Post-process: (1) for hoisted variables, replace `local var = ...`
        // with `var = ...` (the pre-declaration already emitted `local var`),
        // and (2) deduplicate any remaining `local` declarations — if a
        // variable was already declared with `local` earlier in the function,
        // subsequent `local var = ...` lines become plain `var = ...`.
        {
            let func_output = &self.output[func_start..];
            let mut patched = String::with_capacity(func_output.len());
            let mut seen_locals: BTreeSet<String> = BTreeSet::new();
            // Seed with function parameters — they are implicitly declared.
            for p in &func.params {
                seen_locals.insert(sanitize_ident(p));
            }
            for line in func_output.lines() {
                let trimmed = line.trim_start();
                let mut replaced = false;
                if let Some(after_local) = trimmed.strip_prefix("local ") {
                    // Extract the variable name: "local vXXX = ..." or "local vXXX"
                    // Skip "local function ..." lines — those are function defs.
                    if !after_local.starts_with("function ") {
                        let var_end = after_local
                            .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                            .unwrap_or(after_local.len());
                        let var_name = &after_local[..var_end];
                        if !var_name.is_empty() {
                            let rest = after_local[var_end..].trim_start();
                            if rest.starts_with('=') {
                                // This is `local var = ...` — check for hoisted or duplicate.
                                if self.hoisted_vars.contains(var_name)
                                    || !seen_locals.insert(var_name.to_string())
                                {
                                    // Already declared — strip `local `.
                                    let indent = &line[..line.len() - trimmed.len()];
                                    patched.push_str(indent);
                                    patched.push_str(after_local);
                                    patched.push('\n');
                                    replaced = true;
                                } else {
                                    // First declaration — keep `local`.
                                    // (already inserted into seen_locals above)
                                }
                            } else if rest.is_empty() || rest.starts_with("--") {
                                // Bare `local var` pre-declaration.
                                seen_locals.insert(var_name.to_string());
                            }
                        }
                    }
                }
                if !replaced {
                    patched.push_str(line);
                    patched.push('\n');
                }
            }
            self.output.truncate(func_start);
            self.output.push_str(&patched);
        }

        self.hoisted_vars.clear();
        self.tuple_vars.clear();
    }

    fn emit_op(&mut self, op: &OpIR) {
        match op.kind.as_str() {
            // ================================================================
            // Constants
            // ================================================================
            "const" => {
                let out = self.out_var(op);
                if let Some(v) = op.value {
                    self.emit_line(&format!("local {out}: number = {v}"));
                } else if let Some(f) = op.f_value {
                    self.emit_line(&format!("local {out}: number = {f}"));
                } else if let Some(ref s) = op.s_value {
                    let escaped = escape_luau_string(s);
                    self.emit_line(&format!("local {out}: string = \"{escaped}\""));
                } else {
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "const_float" => {
                let out = self.out_var(op);
                let val = op.f_value.unwrap_or(0.0);
                self.emit_line(&format!("local {out}: number = {val}"));
            }
            "const_int" => {
                let out = self.out_var(op);
                let val = op.value.unwrap_or(0);
                self.emit_line(&format!("local {out}: number = {val}"));
            }
            "const_str" => {
                let out = self.out_var(op);
                let s = op.s_value.as_deref().unwrap_or("");
                let escaped = escape_luau_string(s);
                self.emit_line(&format!("local {out}: string = \"{escaped}\""));
            }
            "const_bytes" => {
                let out = self.out_var(op);
                if let Some(ref bytes) = op.bytes {
                    let escaped: String = bytes.iter().map(|b| format!("\\x{b:02x}")).collect();
                    self.emit_line(&format!("local {out}: string = \"{escaped}\""));
                } else {
                    let s = op.s_value.as_deref().unwrap_or("");
                    let escaped = escape_luau_string(s);
                    self.emit_line(&format!("local {out}: string = \"{escaped}\""));
                }
            }
            "const_bool" | "bool_const" => {
                let out = self.out_var(op);
                let val = if op.value.unwrap_or(0) != 0 {
                    "true"
                } else {
                    "false"
                };
                self.emit_line(&format!("local {out}: boolean = {val}"));
            }
            "const_none" | "none_const" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = nil"));
            }
            "string_const" => {
                let out = self.out_var(op);
                let s = op.s_value.as_deref().unwrap_or("");
                let escaped = escape_luau_string(s);
                self.emit_line(&format!("local {out}: string = \"{escaped}\""));
            }
            "const_bigint" => {
                let out = self.out_var(op);
                let s = op.s_value.as_deref().unwrap_or("0");
                self.emit_line(&format!("local {out} = tonumber(\"{s}\") or 0"));
            }
            "const_not_implemented" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = molt_not_implemented"));
            }
            "const_ellipsis" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = nil -- {}", op.kind));
            }
            "missing" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = molt_missing_sentinel"));
            }

            // ================================================================
            // Variable load/store (both pedagogical and real IR forms)
            // ================================================================
            "load_local" => {
                let out = self.out_var(op);
                let var = self.var_ref(op);
                self.emit_line(&format!("local {out} = {var}"));
            }
            "load_var" | "copy_var" => {
                let out = self.out_var(op);
                let var = op
                    .var
                    .as_deref()
                    .or_else(|| {
                        op.args
                            .as_deref()
                            .and_then(|args| args.first().map(String::as_str))
                    })
                    .map(sanitize_ident)
                    .unwrap_or_else(|| "_".to_string());
                self.emit_line(&format!("local {out} = {var}"));
            }
            "load" | "guarded_load" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(obj) = args.first() {
                    // Field offsets are byte offsets in 8-byte MoltValue slots.
                    let slot = (op.value.unwrap_or(0) / 8) + 1;
                    let obj = sanitize_ident(obj);
                    self.emit_line(&format!("local {out} = {obj}[{slot}]"));
                } else {
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "guard_type" | "guard_tag" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let value = sanitize_ident(&args[0]);
                    let expected = sanitize_ident(&args[1]);
                    if let Some(ref out_name) = op.out
                        && out_name != "none"
                    {
                        let out = sanitize_ident(out_name);
                        self.emit_line(&format!(
                            "local {out} = molt_guard_type({value}, {expected})"
                        ));
                    } else {
                        self.emit_line(&format!("molt_guard_type({value}, {expected})"));
                    }
                }
            }
            "closure_load" => {
                // closure_load: args[0] = slot name, out = destination var
                // Reads from closure slot (stored via closure_store).
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(slot) = args.first() {
                    let slot = sanitize_ident(slot);
                    self.emit_line(&format!("local {out} = __closure_{slot}"));
                } else {
                    let var = self.var_ref(op);
                    self.emit_line(&format!("local {out} = {var}"));
                }
            }
            "store_local" => {
                let var = self.var_ref(op);
                if let Some(ref args) = op.args
                    && let Some(src) = args.first()
                {
                    // Propagate tuple tracking: if the source is a known
                    // tuple variable, the destination inherits that status.
                    if self.tuple_vars.contains(src)
                        && let Some(ref var_name) = op.var
                    {
                        self.tuple_vars.insert(var_name.clone());
                    }
                    self.emit_line(&format!("{var} = {}", sanitize_ident(src)));
                }
            }
            "store_var" => {
                let var = op
                    .var
                    .as_deref()
                    .or(op.out.as_deref())
                    .map(sanitize_ident)
                    .unwrap_or_else(|| "_".to_string());
                if let Some(ref args) = op.args
                    && let Some(src) = args.first()
                {
                    if self.tuple_vars.contains(src)
                        && let Some(ref var_name) = op.var
                    {
                        self.tuple_vars.insert(var_name.clone());
                    }
                    self.emit_line(&format!("{var} = {}", sanitize_ident(src)));
                }
            }
            "store" | "store_init" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    // Field offsets are byte offsets in 8-byte MoltValue slots.
                    let slot = (op.value.unwrap_or(0) / 8) + 1;
                    let obj = sanitize_ident(&args[0]);
                    let value = sanitize_ident(&args[1]);
                    self.emit_line(&format!("{obj}[{slot}] = {value}"));
                }
            }
            "closure_store" => {
                // closure_store: args[0] = slot name, args[1] = value
                // Stores value into a closure slot variable.
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let slot = sanitize_ident(&args[0]);
                    let value = sanitize_ident(&args[1]);
                    self.emit_line(&format!("__closure_{slot} = {value}"));
                }
            }
            "identity_alias" | "binding_alias" => {
                let out = self.out_var(op);
                if let Some(ref args) = op.args
                    && let Some(src) = args.first()
                {
                    // Propagate tuple tracking through aliases.
                    if self.tuple_vars.contains(src)
                        && let Some(ref out_name) = op.out
                    {
                        self.tuple_vars.insert(out_name.clone());
                    }
                    self.emit_line(&format!("local {out} = {}", sanitize_ident(src)));
                }
            }

            // ================================================================
            // Arithmetic ops (real IR op kinds)
            // ================================================================
            "add" | "inplace_add" => {
                // Python + is overloaded: numeric add for numbers, concat for strings.
                // Only producer-derived operand facts may skip the type check:
                // the current op's result-side type_hint is passive metadata.
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = sanitize_ident(&args[0]);
                    let rhs = sanitize_ident(&args[1]);
                    let lhs_num = self.numeric_operand_expr(&args[0]);
                    let rhs_num = self.numeric_operand_expr(&args[1]);
                    let is_numeric = self.scalar_plan.op_prefers_integer_runtime_lane(op)
                        || matches!(self.scalar_plan.op_scalar_lane(op), Some(ScalarKind::Float));
                    if is_numeric {
                        self.emit_line(&format!("local {out}: number = {lhs_num} + {rhs_num}"));
                    } else {
                        self.emit_line(&format!(
                            "local {out} = if type({lhs}) == \"string\" or type({rhs}) == \"string\" then tostring({lhs}) .. tostring({rhs}) else {lhs_num} + {rhs_num}"
                        ));
                    }
                }
            }
            "sub" | "inplace_sub" => self.emit_binary_op(op, "-"),
            "mul" | "inplace_mul" => self.emit_binary_op(op, "*"),
            "div" => {
                // Luau 1/0 = inf (IEEE 754), Python raises ZeroDivisionError.
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = self.numeric_operand_expr(&args[0]);
                    let rhs = sanitize_ident(&args[1]);
                    let rhs_num = self.numeric_operand_expr(&args[1]);
                    self.emit_line(&format!(
                        "if {rhs} == 0 then error({{__type=\"ZeroDivisionError\", __msg=\"division by zero\"}}) end"
                    ));
                    self.emit_line(&format!("local {out}: number = {lhs} / {rhs_num}"));
                }
            }
            "mod" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = self.numeric_operand_expr(&args[0]);
                    let rhs = sanitize_ident(&args[1]);
                    let rhs_num = self.numeric_operand_expr(&args[1]);
                    self.emit_line(&format!(
                        "if {rhs} == 0 then error({{__type=\"ZeroDivisionError\", __msg=\"integer modulo by zero\"}}) end"
                    ));
                    self.emit_line(&format!("local {out}: number = {lhs} % {rhs_num}"));
                }
            }
            "floordiv" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = self.numeric_operand_expr(&args[0]);
                    let rhs = sanitize_ident(&args[1]);
                    let rhs_num = self.numeric_operand_expr(&args[1]);
                    self.emit_line(&format!(
                        "if {rhs} == 0 then error({{__type=\"ZeroDivisionError\", __msg=\"integer division or modulo by zero\"}}) end"
                    ));
                    self.emit_line(&format!("local {out}: number = {lhs} // {rhs_num}"));
                }
            }
            "pow" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = self.numeric_operand_expr(&args[0]);
                    let rhs = self.numeric_operand_expr(&args[1]);
                    // Direct ^ operator — no helper call overhead.
                    self.emit_line(&format!("local {out}: number = {lhs} ^ {rhs}"));
                }
            }
            "pow_mod" => {
                // Python pow(base, exp, mod) uses modular exponentiation —
                // computing base^exp directly overflows for large exponents.
                // Emit a loop-based modular exponentiation (square-and-multiply).
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let base = sanitize_ident(&args[0]);
                    let exp = sanitize_ident(&args[1]);
                    let modulus = sanitize_ident(&args[2]);
                    self.emit_line(&format!(
                        "local {out}; do local __b, __e, __m = {base} % {modulus}, {exp}, {modulus}; \
                         local __r = 1; while __e > 0 do \
                         if __e % 2 == 1 then __r = (__r * __b) % __m end; \
                         __b = (__b * __b) % __m; __e = __e // 2 end; \
                         {out} = __r end"
                    ));
                }
            }
            "matmul" | "inplace_matmul" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = sanitize_ident(&args[0]);
                    let rhs = sanitize_ident(&args[1]);
                    let helper = if op.kind == "inplace_matmul" {
                        "molt_inplace_matmul"
                    } else {
                        "molt_matmul"
                    };
                    self.emit_line(&format!("local {out} = {helper}({lhs}, {rhs})"));
                }
            }
            "round" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = math.round({})",
                        sanitize_ident(val)
                    ));
                }
            }
            "trunc" => {
                // Python math.trunc: truncates toward zero.
                // math.floor truncates toward negative infinity — wrong for negatives.
                // math.modf returns (integer_part, fractional_part).
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    let v = sanitize_ident(val);
                    self.emit_line(&format!(
                        "local {out} = if {v} >= 0 then math_floor({v}) else math.ceil({v})"
                    ));
                }
            }

            // ================================================================
            // Bitwise ops (real IR op kinds)
            // ================================================================
            "bit_and" | "inplace_bit_and" => self.emit_bit_op(op, "band"),
            "bit_or" | "inplace_bit_or" => self.emit_bit_op(op, "bor"),
            "bit_xor" | "inplace_bit_xor" => self.emit_bit_op(op, "bxor"),
            "lshift" | "shl" => self.emit_bit_op(op, "lshift"),
            "rshift" | "shr" => self.emit_bit_op(op, "rshift"),

            // ================================================================
            // Unary ops (real IR op kinds)
            // ================================================================
            "not" => {
                // Python `not x` uses Python truthiness (0, "", [], {} are falsy).
                // Luau `not x` only treats nil/false as falsy.
                // Use molt_bool for Python-compatible truthiness when type is unknown.
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    let v = sanitize_ident(val);
                    let is_bool = self.is_known_bool_value(val);
                    if is_bool {
                        self.emit_line(&format!("local {out}: boolean = not {v}"));
                    } else {
                        let truthy = self.guard_truthiness(val);
                        self.emit_line(&format!("local {out}: boolean = not {truthy}"));
                    }
                }
            }
            "invert" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = bit32.bnot({})",
                        sanitize_ident(val)
                    ));
                }
            }
            "abs" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!("local {out} = math.abs({})", sanitize_ident(val)));
                }
            }

            // ================================================================
            // Comparison ops (real IR op kinds)
            // ================================================================
            "lt" | "le" | "gt" | "ge" | "eq" | "string_eq" | "ne" => {
                let operator = match op.kind.as_str() {
                    "lt" => "<",
                    "le" => "<=",
                    "gt" => ">",
                    "ge" => ">=",
                    "eq" | "string_eq" => "==",
                    "ne" => "~=",
                    _ => unreachable!(),
                };
                self.emit_binary_op(op, operator);
            }
            "is" => {
                // Python `is` checks identity, not equality.  For `x is None`
                // this maps correctly to `x == nil` (fine since nil is a
                // singleton).  For non-None operands, `==` checks value
                // equality which differs, but there's no Luau equivalent for
                // identity.  This is an accepted semantic gap.
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = sanitize_ident(&args[0]);
                    let rhs = sanitize_ident(&args[1]);
                    self.emit_line(&format!("local {out}: boolean = ({lhs} == {rhs})"));
                }
            }

            // ================================================================
            // Logical ops — Python truthiness differs from Luau.
            // Python treats 0, "", [], {} as falsy; Luau only nil/false.
            // When operands are known-boolean (from comparisons), use native
            // and/or.  Otherwise use molt_bool() to get Python semantics.
            // ================================================================
            "and" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let a = sanitize_ident(&args[0]);
                    let b = sanitize_ident(&args[1]);
                    if self.is_known_bool_value(&args[0]) && self.is_known_bool_value(&args[1]) {
                        self.emit_line(&format!("local {out} = {a} and {b}"));
                    } else {
                        // Python `a and b`: if a is falsy return a, else return b
                        let truthy = self.guard_truthiness(&args[0]);
                        self.emit_line(&format!("local {out} = if {truthy} then {b} else {a}"));
                    }
                }
            }
            "or" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let a = sanitize_ident(&args[0]);
                    let b = sanitize_ident(&args[1]);
                    if self.is_known_bool_value(&args[0]) && self.is_known_bool_value(&args[1]) {
                        self.emit_line(&format!("local {out} = {a} or {b}"));
                    } else {
                        // Python `a or b`: if a is truthy return a, else return b
                        let truthy = self.guard_truthiness(&args[0]);
                        self.emit_line(&format!("local {out} = if {truthy} then {a} else {b}"));
                    }
                }
            }

            // ================================================================
            // Pedagogical composite ops (binop/compare/unary_op with s_value)
            // ================================================================
            "binop" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = sanitize_ident(&args[0]);
                    let rhs = sanitize_ident(&args[1]);
                    let op_str = op.s_value.as_deref().unwrap_or("+");
                    let expr = match op_str {
                        "+" | "-" | "*" | "/" | "%" => format!("{lhs} {op_str} {rhs}"),
                        "//" => format!("{lhs} // {rhs}"),
                        "**" => format!("{lhs} ^ {rhs}"),
                        "&" => format!("bit32.band({lhs}, {rhs})"),
                        "|" => format!("bit32.bor({lhs}, {rhs})"),
                        "^" => format!("bit32.bxor({lhs}, {rhs})"),
                        "<<" => format!("bit32.lshift({lhs}, {rhs})"),
                        ">>" => format!("bit32.rshift({lhs}, {rhs})"),
                        _ => format!("{lhs} {op_str} {rhs}"),
                    };
                    self.emit_line(&format!("local {out} = {expr}"));
                }
            }
            "compare" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = sanitize_ident(&args[0]);
                    let rhs = sanitize_ident(&args[1]);
                    let cmp = op.s_value.as_deref().unwrap_or("==");
                    let luau_cmp = match cmp {
                        "!=" | "<>" => "~=",
                        other => other,
                    };
                    self.emit_line(&format!("local {out} = {lhs} {luau_cmp} {rhs}"));
                }
            }
            "unary_op" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(operand) = args.first() {
                    let operand = sanitize_ident(operand);
                    let uop = op.s_value.as_deref().unwrap_or("-");
                    let expr = match uop {
                        "-" => format!("-{operand}"),
                        "not" => {
                            let truthy = self.guard_truthiness(args.first().unwrap());
                            format!("not {truthy}")
                        }
                        "~" => format!("bit32.bnot({operand})"),
                        _ => format!("-{operand}"),
                    };
                    self.emit_line(&format!("local {out} = {expr}"));
                }
            }

            // ================================================================
            // Control flow: labels and jumps
            // ================================================================
            "label" | "state_label" => {
                // Emit real Luau labels — Roblox Studio supports goto/labels.
                if let Some(id) = op.value {
                    self.emit_line(&format!("::label_{id}::"));
                } else if let Some(ref s) = op.s_value {
                    let label = sanitize_label(s);
                    self.emit_line(&format!("::{label}::"));
                }
            }
            "jump" | "goto" => {
                if let Some(id) = op.value {
                    self.emit_line(&format!("goto label_{id}"));
                } else if let Some(ref target) = op.s_value {
                    let target = sanitize_label(target);
                    self.emit_line(&format!("goto {target}"));
                }
            }
            "pcall_failure_jump" => {
                let pcall_id = op
                    .s_value
                    .as_deref()
                    .and_then(|s| s.parse::<u32>().ok())
                    .unwrap_or(0);
                if let Some(id) = op.value {
                    self.emit_line(&format!("if not __ok_{pcall_id} then goto label_{id} end"));
                } else if let Some(ref target) = op.s_value {
                    let target = sanitize_label(target);
                    self.emit_line(&format!("if not __ok_{pcall_id} then goto {target} end"));
                }
            }
            "br_if" => {
                // Emit real conditional goto with Python truthiness guard.
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(cond_raw) = args.first() {
                    let cond = self.guard_truthiness(cond_raw);
                    if let Some(id) = op.value {
                        self.emit_line(&format!("if {cond} then goto label_{id} end"));
                    } else if let Some(ref target) = op.s_value {
                        let target = sanitize_label(target);
                        self.emit_line(&format!("if {cond} then goto {target} end"));
                    } else {
                        let cond_ident = sanitize_ident(cond_raw);
                        self.emit_line(&format!(
                            "error(\"[unsupported op: br_if {cond_ident} missing target label]\")"
                        ));
                    }
                }
            }
            "branch" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                let cond_raw = args.first().map(|s| s.as_str()).or(op.var.as_deref());
                let cond = if let Some(raw) = cond_raw {
                    self.guard_truthiness(raw)
                } else {
                    "true".to_string()
                };
                if let Some(id) = op.value {
                    self.emit_line(&format!("if {cond} then goto label_{id} end"));
                } else if let Some(ref target) = op.s_value {
                    let target = sanitize_label(target);
                    self.emit_line(&format!("if {cond} then goto {target} end"));
                } else {
                    self.emit_line(&format!(
                        "error(\"[unsupported op: branch {cond} missing target label]\")"
                    ));
                }
            }
            "branch_false" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                let cond_raw = args.first().map(|s| s.as_str()).or(op.var.as_deref());
                let cond = if let Some(raw) = cond_raw {
                    self.guard_truthiness(raw)
                } else {
                    "false".to_string()
                };
                let not_cond = if cond.starts_with("molt_bool(") {
                    format!("not ({cond})")
                } else {
                    format!("not {cond}")
                };
                if let Some(id) = op.value {
                    self.emit_line(&format!("if {not_cond} then goto label_{id} end"));
                } else if let Some(ref target) = op.s_value {
                    let target = sanitize_label(target);
                    self.emit_line(&format!("if {not_cond} then goto {target} end"));
                } else {
                    self.emit_line(&format!(
                        "error(\"[unsupported op: branch_false {cond} missing target label]\")"
                    ));
                }
            }

            // ================================================================
            // Structured if/else/end_if
            // ================================================================
            "if" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(cond) = args.first() {
                    let cond_ident = sanitize_ident(cond);
                    // Python truthiness: 0, "", [], {} are falsy but Luau
                    // treats them as truthy. Known boolean producers and
                    // literal booleans can be used directly. Otherwise wrap in
                    // molt_bool().
                    let is_bool = self.is_known_bool_value(cond);
                    if is_bool {
                        self.emit_line(&format!("if {cond_ident} then"));
                    } else {
                        self.emit_line(&format!("if molt_bool({cond_ident}) then"));
                    }
                    self.push_indent();
                }
            }
            "else" => {
                self.pop_indent();
                self.emit_line("else");
                self.push_indent();
            }
            "end_if" => {
                self.pop_indent();
                self.emit_line("end");
            }

            // ================================================================
            // Loops
            // ================================================================
            "loop_start" => {
                // Check if the next op is loop_index_start — if so, its
                // initialization is handled via the pending_loop_index
                // mechanism to ensure it runs before the while opens.
                self.emit_line("while true do");
                self.push_indent();
            }
            "loop_index_start" => {
                // No-op here — initialization is emitted before the while
                // by the loop_start handler via pending_loop_index.
                // The pre-declared variable is set before loop entry.
            }
            "loop_end" => {
                self.pop_indent();
                self.emit_line("end");
            }
            "loop_break" => {
                self.emit_line("break");
            }
            "loop_break_if_true" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(cond_raw) = args.first() {
                    let cond = self.guard_truthiness(cond_raw);
                    self.emit_line(&format!("if {cond} then break end"));
                }
            }
            "loop_break_if_exception" => {
                // Value-less exception-flag break.  On native/WASM the producer
                // returns a None sentinel on a mid-iteration raise and this op
                // breaks the consumption loop so it cannot spin forever.  In the
                // Luau backend, Python exceptions are raised via Lua `error()`,
                // which unwinds the call stack immediately out of the iterator
                // closure call (`iter_var()` in the `iter_next` lowering) — the
                // loop body after `iter_next` never executes, so there is no
                // sentinel-driven spin to break.  The op is therefore a no-op
                // here: the unwinding `error()` already exited the loop and is
                // caught by the enclosing `pcall` for the active try/except.
            }
            "loop_break_if_false" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(cond_raw) = args.first() {
                    let cond = self.guard_truthiness(cond_raw);
                    // Use parens only when cond is a molt_bool() call (compound expr).
                    // Plain idents don't need parens and must not have them
                    // (optimization passes pattern-match `if not vN then`).
                    if cond.starts_with("molt_bool(") {
                        self.emit_line(&format!("if not ({cond}) then break end"));
                    } else {
                        self.emit_line(&format!("if not {cond} then break end"));
                    }
                }
            }
            "loop_continue" => {
                self.emit_line("continue");
            }
            "loop_index_next" => {
                // Update loop counter: out = args[0].
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    let args = op.args.as_deref().unwrap_or(&[]);
                    if let Some(new_val) = args.first() {
                        let val = sanitize_ident(new_val);
                        self.emit_line(&format!("{out} = {val}"));
                    }
                }
            }
            "loop_carry_init" | "loop_carry_update" => {
                // Internal loop bookkeeping — skip.
            }
            "for_range" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let start = sanitize_ident(&args[0]);
                    let stop = sanitize_ident(&args[1]);
                    let step = if args.len() >= 3 {
                        sanitize_ident(&args[2])
                    } else {
                        "1".to_string()
                    };
                    // Python range is exclusive of stop; Luau for-loop is inclusive.
                    // For positive step: limit = stop - 1
                    // For negative step: limit = stop + 1
                    // When step is a variable, emit a ternary at runtime.
                    let limit_expr = if step == "1" {
                        format!("{stop} - 1")
                    } else if step == "-1" {
                        format!("{stop} + 1")
                    } else if let Ok(n) = step.parse::<i64>() {
                        if n > 0 {
                            format!("{stop} - 1")
                        } else {
                            format!("{stop} + 1")
                        }
                    } else {
                        format!("if {step} > 0 then {stop} - 1 else {stop} + 1")
                    };
                    self.emit_line(&format!("for {out} = {start}, {limit_expr}, {step} do"));
                    self.push_indent();
                }
            }
            "for_iter" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(iterable) = args.first() {
                    let iterable = sanitize_ident(iterable);
                    self.emit_line(&format!("for _, {out} in ipairs({iterable}) do"));
                    self.push_indent();
                }
            }
            "end_for" => {
                self.pop_indent();
                self.emit_line("end");
            }

            // ================================================================
            // Function calls
            // ================================================================
            "call" | "call_guarded" => {
                let out = self.out_var(op);
                // First check for s_value function name (pedagogical IR form).
                if let Some(ref func_name) = op.s_value {
                    let func_name = sanitize_ident(func_name);
                    let call_args = op
                        .args
                        .as_deref()
                        .unwrap_or(&[])
                        .iter()
                        .map(|a| sanitize_ident(a))
                        .collect::<Vec<_>>()
                        .join(", ");
                    // Map Python builtins to Luau equivalents.
                    let mapped = match func_name.as_str() {
                        "len" | "molt_len" => format!("molt_len({call_args})"),
                        "int" | "molt_int" => format!("molt_int({call_args})"),
                        "float" | "molt_float" => format!("molt_float({call_args})"),
                        "str" | "molt_str" => format!("molt_str({call_args})"),
                        "bool" | "molt_bool" => format!("molt_bool({call_args})"),
                        "range" | "molt_range" => format!("molt_range({call_args})"),
                        "abs" => format!("math.abs({call_args})"),
                        "min" => format!("math.min({call_args})"),
                        "max" => format!("math.max({call_args})"),
                        "round" => format!("math.round({call_args})"),
                        "enumerate" | "molt_enumerate" => format!("molt_enumerate({call_args})"),
                        "zip" | "molt_zip" => format!("molt_zip({call_args})"),
                        "sorted" | "molt_sorted" => format!("molt_sorted({call_args})"),
                        "reversed" | "molt_reversed" => format!("molt_reversed({call_args})"),
                        "sum" | "molt_sum" => format!("molt_sum({call_args})"),
                        "any" | "molt_any" => format!("molt_any({call_args})"),
                        "all" | "molt_all" => format!("molt_all({call_args})"),
                        "map" | "molt_map" => format!("molt_map({call_args})"),
                        "filter" | "molt_filter" => format!("molt_filter({call_args})"),
                        "print" => format!("molt_print({call_args})"),
                        _ => format!("{func_name}({call_args})"),
                    };
                    self.emit_line(&format!("local {out} = {mapped}"));
                } else {
                    // Real IR form: args[0] is the callable, rest are arguments.
                    let args = op.args.as_deref().unwrap_or(&[]);
                    if !args.is_empty() {
                        let func_ref = sanitize_ident(&args[0]);
                        let call_args = args[1..]
                            .iter()
                            .map(|a| sanitize_ident(a))
                            .collect::<Vec<_>>()
                            .join(", ");
                        if op.out.is_some() {
                            self.emit_line(&format!("local {out} = {func_ref}({call_args})"));
                        } else {
                            self.emit_line(&format!("{func_ref}({call_args})"));
                        }
                    }
                }
            }
            "call_internal" => {
                if let Some(ref s_val) = op.s_value {
                    let func_name = sanitize_ident(s_val);
                    let call_args = op
                        .args
                        .as_deref()
                        .unwrap_or(&[])
                        .iter()
                        .map(|a| sanitize_ident(a))
                        .collect::<Vec<_>>()
                        .join(", ");
                    if let Some(ref out_name) = op.out {
                        let out = sanitize_ident(out_name);
                        self.emit_line(&format!("local {out} = {func_name}({call_args})"));
                    } else {
                        self.emit_line(&format!("{func_name}({call_args})"));
                    }
                }
            }
            "call_func" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if !args.is_empty() {
                    let func_ref = sanitize_ident(&args[0]);
                    let call_args = args[1..]
                        .iter()
                        .map(|a| sanitize_ident(a))
                        .collect::<Vec<_>>()
                        .join(", ");
                    if let Some(ref out_name) = op.out {
                        let out = sanitize_ident(out_name);
                        self.emit_line(&format!(
                            "local {out} = if {func_ref} then {func_ref}({call_args}) else nil"
                        ));
                    } else {
                        self.emit_line(&format!("if {func_ref} then {func_ref}({call_args}) end"));
                    }
                }
            }
            "call_method" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if !args.is_empty() {
                    let obj = sanitize_ident(&args[0]);
                    let method_name = op.s_value.as_deref().unwrap_or("unknown");
                    let method = sanitize_ident(method_name);
                    let call_args = args[1..]
                        .iter()
                        .map(|a| sanitize_ident(a))
                        .collect::<Vec<_>>()
                        .join(", ");
                    // When the receiver is a known list, emit direct table
                    // operations instead of method calls (Luau tables don't
                    // have Python list methods).
                    let obj_is_list = self.plan_knows_list(&args[0]);
                    if obj_is_list {
                        match method_name {
                            "append" => {
                                if let Some(val) = args.get(1) {
                                    self.emit_line(&format!(
                                        "{obj}[#{obj} + 1] = {}",
                                        sanitize_ident(val)
                                    ));
                                }
                                // append returns None in Python; skip output.
                            }
                            "pop" => {
                                let idx = args.get(1).map(|s| sanitize_ident(s));
                                if let Some(ref out_name) = op.out {
                                    let out = sanitize_ident(out_name);
                                    self.emit_list_pop(&obj, idx.as_deref(), Some(&out));
                                } else {
                                    self.emit_list_pop(&obj, idx.as_deref(), None);
                                }
                            }
                            "insert" => {
                                if args.len() >= 3 {
                                    let idx = sanitize_ident(&args[1]);
                                    let val = sanitize_ident(&args[2]);
                                    self.emit_list_insert(&obj, &idx, &val);
                                }
                            }
                            "remove" => {
                                if let Some(val) = args.get(1) {
                                    let val = sanitize_ident(val);
                                    // Guard: table.find returns nil when not found,
                                    // and table.remove(t, nil) silently removes the
                                    // LAST element. Must check and raise ValueError.
                                    self.emit_line(&format!(
                                        "do local __idx = table.find({obj}, {val}); if __idx then table.remove({obj}, __idx) else error(\"ValueError: list.remove(x): x not in list\") end end"
                                    ));
                                }
                            }
                            "sort" => {
                                self.emit_line(&format!("table.sort({obj})"));
                            }
                            "reverse" => {
                                // In-place reverse via swap loop.
                                self.emit_line(&format!(
                                    "for __i = 1, math.floor(#{obj} / 2) do {obj}[__i], {obj}[#{obj} - __i + 1] = {obj}[#{obj} - __i + 1], {obj}[__i] end"
                                ));
                            }
                            "clear" => {
                                self.emit_line(&format!("table.clear({obj})"));
                            }
                            "copy" => {
                                if let Some(ref out_name) = op.out {
                                    let out = sanitize_ident(out_name);
                                    self.emit_line(&format!("local {out} = table.clone({obj})"));
                                }
                            }
                            "extend" => {
                                if let Some(other) = args.get(1) {
                                    let other = sanitize_ident(other);
                                    self.emit_line(&format!(
                                        "table.move({other}, 1, #{other}, #{obj} + 1, {obj})"
                                    ));
                                }
                            }
                            _ => {
                                // Fall through to generic method call for count/index/etc.
                                if let Some(ref out_name) = op.out {
                                    let out = sanitize_ident(out_name);
                                    self.emit_line(&format!(
                                        "local {out} = {obj}:{method}({call_args})"
                                    ));
                                } else {
                                    self.emit_line(&format!("{obj}:{method}({call_args})"));
                                }
                            }
                        }
                    } else if let Some(ref out_name) = op.out {
                        let out = sanitize_ident(out_name);
                        let escaped = escape_luau_string(method_name);
                        self.emit_line(&format!(
                            "local {out}; do local __method = molt_get_attr({obj}, \"{escaped}\"); {out} = if __method then __method({call_args}) else nil end end"
                        ));
                    } else {
                        let escaped = escape_luau_string(method_name);
                        self.emit_line(&format!(
                            "do local __method = molt_get_attr({obj}, \"{escaped}\"); if __method then __method({call_args}) end end"
                        ));
                    }
                }
            }
            "call_async" => {
                // Luau has no Molt scheduler object model here, but CALL_ASYNC
                // already carries the concrete poll target in s_value. Execute
                // that target directly for the admitted synchronous subset.
                let args = op.args.as_deref().unwrap_or(&[]);
                let func_ref = sanitize_ident(
                    op.s_value
                        .as_deref()
                        .expect("call_async expects poll target in s_value"),
                );
                let call_args = args
                    .iter()
                    .map(|a| sanitize_ident(a))
                    .collect::<Vec<_>>()
                    .join(", ");
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = {func_ref}({call_args})"));
                } else {
                    self.emit_line(&format!("{func_ref}({call_args})"));
                }
            }
            "block_on" | "spawn" => {
                // Scheduler ownership is not represented in checked Luau yet.
                let args = op.args.as_deref().unwrap_or(&[]);
                if !args.is_empty() {
                    let func_ref = sanitize_ident(&args[0]);
                    let call_args = args[1..]
                        .iter()
                        .map(|a| sanitize_ident(a))
                        .collect::<Vec<_>>()
                        .join(", ");
                    if let Some(ref out_name) = op.out {
                        let out = sanitize_ident(out_name);
                        self.emit_line(&format!(
                            "local {out} = {func_ref}({call_args}) -- [async: {}]",
                            op.kind
                        ));
                    } else {
                        self.emit_line(&format!("{func_ref}({call_args}) -- [async: {}]", op.kind));
                    }
                } else if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil -- [async: {}]", op.kind));
                }
            }

            // ================================================================
            // Return
            // ================================================================
            "ret" | "return" | "return_value" => {
                if let Some(ref args) = op.args {
                    if args.len() == 1 {
                        let val = sanitize_ident(&args[0]);
                        if self.tuple_vars.contains(&args[0]) {
                            self.emit_line(&format!("return table.unpack({val})"));
                        } else {
                            self.emit_line(&format!("return {val}"));
                        }
                    } else if args.len() > 1 {
                        let vals: Vec<String> = args.iter().map(|a| sanitize_ident(a)).collect();
                        self.emit_line(&format!("return {}", vals.join(", ")));
                    } else {
                        self.emit_line("return");
                    }
                } else if let Some(ref var) = op.var {
                    let val = sanitize_ident(var);
                    if self.tuple_vars.contains(var) {
                        self.emit_line(&format!("return table.unpack({val})"));
                    } else {
                        self.emit_line(&format!("return {val}"));
                    }
                } else {
                    self.emit_line("return");
                }
            }
            "ret_void" => {
                self.emit_line("return");
            }

            // ================================================================
            // Collection construction
            // ================================================================
            "build_list" | "list_new" | "callargs_new" => {
                let out = self.out_var(op);
                let items = op
                    .args
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(|a| sanitize_ident(a))
                    .collect::<Vec<_>>()
                    .join(", ");
                self.emit_line(&format!("local {out}: {{any}} = {{{items}}}"));
            }
            "list_fill_new" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                let count = args
                    .first()
                    .map(|a| sanitize_ident(a))
                    .unwrap_or_else(|| "0".to_string());
                let fill = args
                    .get(1)
                    .map(|a| sanitize_ident(a))
                    .unwrap_or_else(|| "nil".to_string());
                self.emit_line(&format!("local {out}: {{any}} = {{}}"));
                self.emit_line(&format!("for __i = 1, math.max(0, {count}) do"));
                self.indent += 1;
                self.emit_line(&format!("{out}[__i] = {fill}"));
                self.indent -= 1;
                self.emit_line("end");
            }
            "tuple_new" | "tuple_from_list" => {
                let out = self.out_var(op);
                // Track this variable so return sites can unpack it.
                if let Some(ref out_name) = op.out {
                    self.tuple_vars.insert(out_name.clone());
                }
                let items = op
                    .args
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(|a| sanitize_ident(a))
                    .collect::<Vec<_>>()
                    .join(", ");
                self.emit_line(&format!("local {out} = {{{items}}}"));
            }
            "unpack_sequence" => {
                // Destructure a tuple/list into individual variables.
                // args[0] = source container, args[1..] = output variable names.
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let src = sanitize_ident(&args[0]);
                    for (i, out_name) in args[1..].iter().enumerate() {
                        let out = sanitize_ident(out_name);
                        self.emit_line(&format!("local {out} = {src}[{}]", i + 1));
                    }
                }
            }
            "build_dict" | "dict_new" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.is_empty() {
                    self.emit_line(&format!("local {out}: {{[any]: any}} = {{}}"));
                } else {
                    // args are key-value pairs: [k1, v1, k2, v2, ...]
                    let mut entries = Vec::new();
                    for pair in args.chunks(2) {
                        if pair.len() == 2 {
                            let key = sanitize_ident(&pair[0]);
                            let val = sanitize_ident(&pair[1]);
                            entries.push(format!("[{key}] = {val}"));
                        }
                    }
                    let body = entries.join(", ");
                    self.emit_line(&format!("local {out}: {{[any]: any}} = {{{body}}}"));
                }
            }
            "set_new" | "frozenset_new" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.is_empty() {
                    self.emit_line(&format!("local {out} = {{}}"));
                } else {
                    // Sets are tables with value→true entries for O(1) lookup.
                    let entries = args
                        .iter()
                        .map(|a| format!("[{}] = true", sanitize_ident(a)))
                        .collect::<Vec<_>>()
                        .join(", ");
                    self.emit_line(&format!("local {out} = {{{entries}}}"));
                }
            }
            "range_new" | "list_from_range" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                match args.len() {
                    1 => {
                        let stop = sanitize_ident(&args[0]);
                        self.emit_line(&format!("local {out} = molt_range(0, {stop})"));
                    }
                    2 => {
                        let start = sanitize_ident(&args[0]);
                        let stop = sanitize_ident(&args[1]);
                        self.emit_line(&format!("local {out} = molt_range({start}, {stop})"));
                    }
                    _ => {
                        let start = sanitize_ident(&args[0]);
                        let stop = sanitize_ident(&args[1]);
                        let step = sanitize_ident(&args[2]);
                        self.emit_line(&format!(
                            "local {out} = molt_range({start}, {stop}, {step})"
                        ));
                    }
                }
            }

            // ================================================================
            // List operations
            // ================================================================
            "list_append" | "callargs_push_pos" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let list = sanitize_ident(&args[0]);
                    let val = sanitize_ident(&args[1]);
                    // rawset bypasses metamethods — safe for plain list tables
                    // and avoids __newindex overhead in Luau's native codegen.
                    self.emit_line(&format!("rawset({list}, #{list} + 1, {val})"));
                }
            }
            "list_pop" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(list) = args.first() {
                    let list = sanitize_ident(list);
                    let idx = args.get(1).map(|s| sanitize_ident(s));
                    if let Some(ref out_name) = op.out {
                        let out = sanitize_ident(out_name);
                        self.emit_list_pop(&list, idx.as_deref(), Some(&out));
                    } else {
                        self.emit_list_pop(&list, idx.as_deref(), None);
                    }
                }
            }
            "list_extend" | "callargs_expand_star" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let list = sanitize_ident(&args[0]);
                    let other = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "table.move({other}, 1, #{other}, #{list} + 1, {list})"
                    ));
                }
            }
            "list_insert" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let list = sanitize_ident(&args[0]);
                    let idx = sanitize_ident(&args[1]);
                    let val = sanitize_ident(&args[2]);
                    self.emit_list_insert(&list, &idx, &val);
                }
            }
            "list_remove" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let list = sanitize_ident(&args[0]);
                    let val = sanitize_ident(&args[1]);
                    // Use numeric for-loop with a found flag. Python list.remove(x)
                    // raises ValueError when x is not in the list.
                    self.emit_line(&format!(
                        "do local __found = false; for __i = 1, #{list} do if {list}[__i] == {val} then table.remove({list}, __i); __found = true; break end end; if not __found then error(\"ValueError: list.remove(x): x not in list\") end end"
                    ));
                }
            }
            "list_clear" | "dict_clear" | "set_clear" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(tbl) = args.first() {
                    self.emit_line(&format!("table.clear({})", sanitize_ident(tbl)));
                }
            }
            "list_copy" | "dict_copy" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(src) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = table.clone({})",
                        sanitize_ident(src)
                    ));
                }
            }
            "list_reverse" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(list) = args.first() {
                    let list = sanitize_ident(list);
                    self.emit_line(&format!(
                        "do local __n = #{list}; for __i = 1, math_floor(__n / 2) do {list}[__i], {list}[__n - __i + 1] = {list}[__n - __i + 1], {list}[__i] end end"
                    ));
                }
            }
            "list_count" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let list = sanitize_ident(&args[0]);
                    let val = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "local {out} = 0; for _, __v in ipairs({list}) do if __v == {val} then {out} = {out} + 1 end end"
                    ));
                }
            }
            "list_index" | "list_index_range" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let list = sanitize_ident(&args[0]);
                    let val = sanitize_ident(&args[1]);
                    if args.len() >= 4 {
                        let start = sanitize_ident(&args[2]);
                        let stop = sanitize_ident(&args[3]);
                        self.emit_line(&format!(
                            "local {out} = -1; do local __n = #{list}; local __start = if {start} < 0 then __n + {start} else {start}; if __start < 0 then __start = 0 end; if __start > __n then __start = __n end; local __stop = if {stop} < 0 then __n + {stop} else {stop}; if __stop < 0 then __stop = 0 end; if __stop > __n then __stop = __n end; local __found = false; for __i = __start + 1, __stop do if {list}[__i] == {val} then {out} = __i - 1; __found = true; break end end; if not __found then error(\"ValueError: \" .. tostring({val}) .. \" is not in list\") end end"
                        ));
                    } else {
                        self.emit_line(&format!(
                            "local {out} = -1; do local __found = false; for __i, __v in ipairs({list}) do if __v == {val} then {out} = __i - 1; __found = true; break end end; if not __found then error(\"ValueError: \" .. tostring({val}) .. \" is not in list\") end end"
                        ));
                    }
                }
            }
            "list_repeat_range" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let val = sanitize_ident(&args[0]);
                    let count = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "local {out} = table.create(math.max(0, {count}), {val})"
                    ));
                } else {
                    self.emit_line(&format!("local {out} = {{}}"));
                }
            }

            // ================================================================
            // Dict operations
            // ================================================================
            "dict_get" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let dict = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    if args.len() >= 3 {
                        // dict.get(key, default) — return default when missing.
                        let default = sanitize_ident(&args[2]);
                        self.emit_line(&format!(
                            "local {out} = if {dict}[{key}] ~= nil then {dict}[{key}] else {default}"
                        ));
                    } else {
                        self.emit_line(&format!("local {out} = {dict}[{key}]"));
                    }
                }
            }
            "dict_set" | "callargs_push_kw" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let dict = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    let val = sanitize_ident(&args[2]);
                    self.emit_line(&format!("{dict}[{key}] = {val}"));
                }
            }
            "dict_setdefault" => {
                // Python dict.setdefault(k, v) only sets if key is absent.
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let dict = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    let val = sanitize_ident(&args[2]);
                    if let Some(ref out_name) = op.out {
                        let out = sanitize_ident(out_name);
                        self.emit_line(&format!(
                            "if {dict}[{key}] == nil then {dict}[{key}] = {val} end; local {out} = {dict}[{key}]"
                        ));
                    } else {
                        self.emit_line(&format!(
                            "if {dict}[{key}] == nil then {dict}[{key}] = {val} end"
                        ));
                    }
                }
            }
            "dict_setdefault_empty_list" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let dict = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "if {dict}[{key}] == nil then {dict}[{key}] = {{}} end"
                    ));
                }
            }
            "dict_pop" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let dict = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    if args.len() >= 3 {
                        // dict.pop(key, default) — return default if missing.
                        let default = sanitize_ident(&args[2]);
                        self.emit_line(&format!(
                            "local {out} = if {dict}[{key}] ~= nil then {dict}[{key}] else {default}"
                        ));
                    } else {
                        // dict.pop(key) — raise KeyError if missing.
                        self.emit_line(&format!(
                            "if {dict}[{key}] == nil then error(\"KeyError: \" .. tostring({key})) end"
                        ));
                        self.emit_line(&format!("local {out} = {dict}[{key}]"));
                    }
                    self.emit_line(&format!("{dict}[{key}] = nil"));
                }
            }
            "dict_update"
            | "dict_update_missing"
            | "callargs_expand_kwstar"
            | "dict_update_kwstar" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let dict = sanitize_ident(&args[0]);
                    let other = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "for __k, __v in pairs({other}) do {dict}[__k] = __v end"
                    ));
                }
            }
            "dict_popitem" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(dict) = args.first() {
                    let dict = sanitize_ident(dict);
                    self.emit_line(&format!(
                        "local {out} = nil; for __k, __v in pairs({dict}) do {out} = {{__k, __v}}; {dict}[__k] = nil; break end; if {out} == nil then error({{__type=\"KeyError\", __msg=\"popitem(): dictionary is empty\"}}) end"
                    ));
                }
            }
            "dict_inc" | "dict_str_int_inc" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let dict = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    let inc = sanitize_ident(&args[2]);
                    self.emit_line(&format!("{dict}[{key}] = ({dict}[{key}] or 0) + {inc}"));
                }
            }
            "dict_from_obj" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(src) = args.first() {
                    let src = sanitize_ident(src);
                    self.emit_line(&format!(
                        "local {out} = {{}}; for __k, __v in pairs({src}) do {out}[__k] = __v end"
                    ));
                }
            }

            // ================================================================
            // Set operations
            // ================================================================
            // `set_add_probe` is `set_add` for the temporary set realized when
            // probing the operand of intersection/issubset; the Luau lane uses
            // Lua-table keys (any value hashable) so it is identical to set_add.
            "set_add" | "set_add_probe" | "frozenset_add" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let set = sanitize_ident(&args[0]);
                    let val = sanitize_ident(&args[1]);
                    self.emit_line(&format!("{set}[{val}] = true"));
                }
            }
            "set_discard" => {
                // discard is silent if element is absent.
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let set = sanitize_ident(&args[0]);
                    let val = sanitize_ident(&args[1]);
                    self.emit_line(&format!("{set}[{val}] = nil"));
                }
            }
            "set_remove" => {
                // remove raises KeyError if element is absent.
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let set = sanitize_ident(&args[0]);
                    let val = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "if {set}[{val}] == nil then error(\"KeyError: \" .. tostring({val})) end; {set}[{val}] = nil"
                    ));
                }
            }
            "set_pop" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(set) = args.first() {
                    let set = sanitize_ident(set);
                    self.emit_line(&format!(
                        "local {out} = nil; for __k in pairs({set}) do {out} = __k; {set}[__k] = nil; break end; if {out} == nil then error(\"KeyError: pop from an empty set\") end"
                    ));
                }
            }
            "set_update" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let set = sanitize_ident(&args[0]);
                    let other = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "for __k in pairs({other}) do {set}[__k] = true end"
                    ));
                }
            }
            "contains" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let container = sanitize_ident(&args[0]);
                    let val = sanitize_ident(&args[1]);
                    let container_kind = self.scalar_plan.name_container_kind(&args[0]);
                    let is_dict = matches!(
                        container_kind,
                        Some(ContainerKind::Dict | ContainerKind::Set)
                    );
                    let is_list = matches!(container_kind, Some(ContainerKind::List));
                    if is_dict {
                        // Dict/set: key lookup.
                        self.emit_line(&format!("local {out} = ({container}[{val}] ~= nil)"));
                    } else if is_list {
                        // List: value search via table.find.
                        self.emit_line(&format!(
                            "local {out} = (table.find({container}, {val}) ~= nil)"
                        ));
                    } else {
                        // Unknown container: string→find, table→check both
                        // array values AND hash keys for correctness.
                        self.emit_line(&format!(
                            "local {out} = if type({container}) == \"string\" then \
                             (string.find({container}, {val}, 1, true) ~= nil) \
                             elseif type({container}) == \"table\" then \
                             (table.find({container}, {val}) ~= nil or {container}[{val}] ~= nil) \
                             else false"
                        ));
                    }
                }
            }

            // ================================================================
            // Indexing / subscript
            // ================================================================
            "get_item" | "subscript" | "index" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let container = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);

                    let container_kind = self.scalar_plan.name_container_kind(&args[0]);
                    let container_is_str = container_kind == Some(ContainerKind::Str);

                    // Fast-path: when the key is a known non-negative constant,
                    // skip the negative-index ternary entirely.
                    let key_is_scalar_int = self.scalar_plan.name_is_integer_family(&args[1]);
                    let key_known_nonneg = self.nonneg_consts.contains(&args[1])
                        || (key_is_scalar_int && op.value.is_some_and(|v| v >= 0));

                    if container_is_str {
                        // Luau does not support string[index]; use string.sub.
                        // Python uses 0-based indexing, Luau uses 1-based.
                        let idx_var = format!("__idx_{out}");
                        if key_known_nonneg {
                            self.emit_line(&format!("local {idx_var} = {key} + 1"));
                        } else {
                            // Handle negative indexing for strings too.
                            self.emit_line(&format!(
                                "local {idx_var} = if {key} >= 0 then {key} + 1 else #{container} + {key} + 1"
                            ));
                        }
                        self.emit_index_bounds_guard(
                            &idx_var,
                            &container,
                            "string index out of range",
                        );
                        let byte_idx_var = format!("__byte_idx_{out}");
                        let next_byte_idx_var = format!("__next_byte_idx_{out}");
                        self.emit_line(&format!(
                            "local {byte_idx_var}: number = molt_str_byte_offset({container}, {idx_var})"
                        ));
                        self.emit_line(&format!(
                            "local {next_byte_idx_var} = utf8.offset({container}, {idx_var} + 1)"
                        ));
                        self.emit_line(&format!(
                            "local {out} = string.sub({container}, {byte_idx_var}, if {next_byte_idx_var} == nil then #{container} else {next_byte_idx_var} - 1)"
                        ));
                    } else {
                        // If the container is a known list, the key is
                        // integer-indexed. Nested-list output identity must come
                        // from `ScalarRepresentationPlan`, not copied transport
                        // hints.
                        let container_is_list = matches!(container_kind, Some(ContainerKind::List));
                        let key_is_int = key_is_scalar_int || container_is_list;
                        if container_is_list {
                            let idx_var = format!("__idx_{out}");
                            if key_known_nonneg {
                                self.emit_line(&format!("local {idx_var}: number = {key} + 1"));
                            } else {
                                self.emit_line(&format!(
                                    "local {idx_var}: number = if {key} >= 0 then {key} + 1 else #{container} + {key} + 1"
                                ));
                            }
                            self.emit_index_bounds_guard(
                                &idx_var,
                                &container,
                                "list index out of range",
                            );
                            // rawget bypasses metamethods — safe for plain list
                            // tables and faster in Luau's native codegen path.
                            self.emit_line(&format!(
                                "local {out} = rawget({container}, {idx_var})"
                            ));
                        } else if key_known_nonneg {
                            // Known non-negative: skip negative index ternary.
                            self.emit_line(&format!("local {out} = {container}[{key} + 1]"));
                        } else if key_is_int {
                            // Handle negative indexing: Python a[-1] = last element.
                            self.emit_line(&format!(
                                "local {out} = {container}[if {key} >= 0 then {key} + 1 else #{container} + {key} + 1]"
                            ));
                        } else {
                            self.emit_line(&format!(
                                "local {out} = {container}[if type({key}) == \"number\" then (if {key} >= 0 then {key} + 1 else #{container} + {key} + 1) else {key}]"
                            ));
                        }
                    }
                }
            }
            "set_item" | "store_subscript" | "store_index" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let container = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    let value = sanitize_ident(&args[2]);
                    let key_is_int = self.scalar_plan.name_is_integer_family(&args[1]);
                    let key_known_nonneg = self.nonneg_consts.contains(&args[1])
                        || (key_is_int && op.value.is_some_and(|v| v >= 0));
                    let known_list_like = matches!(
                        self.scalar_plan.name_container_kind(&args[0]),
                        Some(ContainerKind::List)
                    );
                    if known_list_like {
                        let idx_expr = if key_known_nonneg {
                            format!("{key} + 1")
                        } else {
                            format!("if {key} >= 0 then {key} + 1 else #{container} + {key} + 1")
                        };
                        // rawset bypasses metamethods — safe for plain list tables.
                        self.emit_line(&format!(
                            "do local __idx: number = {idx_expr}; if __idx < 1 or __idx > #{container} then error({{__type=\"IndexError\", __msg=\"list assignment index out of range\"}}) end; rawset({container}, __idx, {value}) end"
                        ));
                    } else if key_known_nonneg {
                        self.emit_line(&format!("{container}[{key} + 1] = {value}"));
                    } else if key_is_int {
                        self.emit_line(&format!(
                            "{container}[if {key} >= 0 then {key} + 1 else #{container} + {key} + 1] = {value}"
                        ));
                    } else {
                        self.emit_line(&format!(
                            "{container}[if type({key}) == \"number\" then (if {key} >= 0 then {key} + 1 else #{container} + {key} + 1) else {key}] = {value}"
                        ));
                    }
                }
            }
            "del_index" | "del_item" => {
                // Python del lst[i] removes the element and shifts remaining.
                // Setting to nil creates a hole that breaks # and ipairs.
                // For integer keys (list deletion), use table.remove with +1 offset.
                // For string keys (dict deletion), nil assignment is correct.
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let container = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    let key_is_int = self.scalar_plan.name_is_integer_family(&args[1]);
                    let key_known_nonneg = self.nonneg_consts.contains(&args[1])
                        || (key_is_int && op.value.is_some_and(|v| v >= 0));
                    let known_list_like = matches!(
                        self.scalar_plan.name_container_kind(&args[0]),
                        Some(ContainerKind::List)
                    );
                    if known_list_like {
                        let idx_expr = if key_known_nonneg {
                            format!("{key} + 1")
                        } else {
                            format!("if {key} >= 0 then {key} + 1 else #{container} + {key} + 1")
                        };
                        self.emit_line(&format!(
                            "do local __idx = {idx_expr}; if __idx < 1 or __idx > #{container} then error({{__type=\"IndexError\", __msg=\"list deletion index out of range\"}}) end; table.remove({container}, __idx) end"
                        ));
                    } else if key_known_nonneg {
                        self.emit_line(&format!("table.remove({container}, {key} + 1)"));
                    } else if key_is_int {
                        self.emit_line(&format!(
                            "table.remove({container}, if {key} >= 0 then {key} + 1 else #{container} + {key} + 1)"
                        ));
                    } else {
                        self.emit_line(&format!(
                            "if type({key}) == \"number\" then table.remove({container}, if {key} >= 0 then {key} + 1 else #{container} + {key} + 1) else {container}[{key}] = nil end"
                        ));
                    }
                }
            }

            // ================================================================
            // Attribute access
            // ================================================================
            "get_attr"
            | "get_attr_generic_obj"
            | "get_attr_generic_ptr"
            | "get_attr_special_obj" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                let raw_attr = op.s_value.as_deref().unwrap_or("unknown");
                if let Some(obj) = args.first() {
                    let raw_obj = obj.as_str();
                    let obj = sanitize_ident(raw_obj);
                    let obj_is_str = self.plan_knows_string(raw_obj);
                    if obj_is_str && raw_attr == "removeprefix" {
                        self.emit_line(&format!(
                            "local {out} = function(__args) local __prefix = __args[1]; if __prefix ~= \"\" and string.sub({obj}, 1, #__prefix) == __prefix then return string.sub({obj}, #__prefix + 1) end; return {obj} end"
                        ));
                    } else if obj_is_str && raw_attr == "removesuffix" {
                        self.emit_line(&format!(
                            "local {out} = function(__args) local __suffix = __args[1]; if __suffix ~= \"\" and string.sub({obj}, -#__suffix) == __suffix then return string.sub({obj}, 1, #{obj} - #__suffix) end; return {obj} end"
                        ));
                    } else if obj_is_str
                        && matches!(
                            raw_attr,
                            "isalpha"
                                | "isdigit"
                                | "isalnum"
                                | "isspace"
                                | "isupper"
                                | "islower"
                                | "isidentifier"
                                | "isprintable"
                                | "isdecimal"
                                | "isnumeric"
                                | "istitle"
                        )
                    {
                        self.emit_string_predicate_attr(&out, &obj, raw_attr);
                    } else if raw_attr.starts_with("__") && raw_attr.ends_with("__") {
                        let escaped = escape_luau_string(raw_attr);
                        self.emit_line(&format!(
                            "local {out} = if type({obj}) == \"function\" and molt_func_attrs[{obj}] ~= nil then molt_func_attrs[{obj}][\"{escaped}\"] else molt_get_attr({obj}, \"{escaped}\")"
                        ));
                    } else {
                        let escaped = escape_luau_string(raw_attr);
                        self.emit_line(&format!(
                            "local {out} = molt_get_attr({obj}, \"{escaped}\")"
                        ));
                    }
                }
            }
            "get_attr_name" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let obj = sanitize_ident(&args[0]);
                    let attr_name = sanitize_ident(&args[1]);
                    self.emit_line(&format!("local {out} = molt_get_attr({obj}, {attr_name})"));
                } else {
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "get_attr_name_default" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let obj = sanitize_ident(&args[0]);
                    let attr_name = sanitize_ident(&args[1]);
                    let default = if args.len() >= 3 {
                        sanitize_ident(&args[2])
                    } else {
                        "nil".to_string()
                    };
                    self.emit_line(&format!(
                        "local {out} = molt_get_attr_default({obj}, {attr_name}, {default})"
                    ));
                } else if let Some(obj) = args.first() {
                    let obj = sanitize_ident(obj);
                    let attr = escape_luau_string(op.s_value.as_deref().unwrap_or("unknown"));
                    self.emit_line(&format!(
                        "local {out} = molt_get_attr_default({obj}, \"{attr}\", nil)"
                    ));
                } else {
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "has_attr_name" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let obj = sanitize_ident(&args[0]);
                    let attr_name = sanitize_ident(&args[1]);
                    self.emit_line(&format!("local {out} = molt_has_attr({obj}, {attr_name})"));
                } else if let Some(obj) = args.first() {
                    let obj = sanitize_ident(obj);
                    let attr = escape_luau_string(op.s_value.as_deref().unwrap_or("unknown"));
                    self.emit_line(&format!("local {out} = molt_has_attr({obj}, \"{attr}\")"));
                } else {
                    self.emit_line(&format!("local {out} = false"));
                }
            }
            "set_attr_name" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let obj = sanitize_ident(&args[0]);
                    let attr_name = sanitize_ident(&args[1]);
                    let value = sanitize_ident(&args[2]);
                    self.emit_line(&format!("molt_set_attr({obj}, {attr_name}, {value})"));
                }
            }
            "set_attr" | "set_attr_generic_obj" | "set_attr_generic_ptr" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                let attr = op.s_value.as_deref().unwrap_or("unknown");
                let escaped = escape_luau_string(attr);
                if attr.starts_with("__") && attr.ends_with("__") {
                    // Functions cannot hold attrs in Luau; table-backed
                    // classes and objects use the normal attribute authority.
                    if args.len() >= 2 {
                        let obj = sanitize_ident(&args[0]);
                        let value = sanitize_ident(&args[1]);
                        self.emit_line(&format!(
                            "if type({obj}) == \"function\" then if molt_func_attrs[{obj}] == nil then molt_func_attrs[{obj}] = {{}} end; molt_func_attrs[{obj}][\"{escaped}\"] = {value} else molt_set_attr({obj}, \"{escaped}\", {value}) end"
                        ));
                    }
                } else {
                    if args.len() >= 2 {
                        let obj = sanitize_ident(&args[0]);
                        let value = sanitize_ident(&args[1]);
                        self.emit_line(&format!("molt_set_attr({obj}, \"{escaped}\", {value})"));
                    }
                }
            }
            "del_attr_name" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let obj = sanitize_ident(&args[0]);
                    let attr_name = sanitize_ident(&args[1]);
                    self.emit_line(&format!("molt_del_attr({obj}, {attr_name})"));
                }
            }
            "del_attr_generic_obj" | "del_attr_generic_ptr" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                let attr = op.s_value.as_deref().unwrap_or("unknown");
                let attr = escape_luau_string(attr);
                if let Some(obj) = args.first() {
                    let obj = sanitize_ident(obj);
                    self.emit_line(&format!("molt_del_attr({obj}, \"{attr}\")"));
                }
            }

            // ================================================================
            // Guarded field access
            // ================================================================
            "guarded_field_get" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(obj) = args.first() {
                    let obj = sanitize_ident(obj);
                    if let Some(attr) = op.s_value.as_deref() {
                        let attr = sanitize_ident(attr);
                        self.emit_line(&format!("local {out} = {obj}.{attr}"));
                    } else if args.len() >= 2 {
                        let key = sanitize_ident(&args[1]);
                        self.emit_line(&format!("local {out} = {obj}[{key}]"));
                    }
                }
            }
            "guarded_field_set" | "guarded_field_init" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let obj = sanitize_ident(&args[0]);
                    if let Some(attr) = op.s_value.as_deref() {
                        let attr = sanitize_ident(attr);
                        let val = sanitize_ident(&args[1]);
                        self.emit_line(&format!("{obj}.{attr} = {val}"));
                    } else if args.len() >= 3 {
                        let key = sanitize_ident(&args[1]);
                        let val = sanitize_ident(&args[2]);
                        self.emit_line(&format!("{obj}[{key}] = {val}"));
                    }
                }
            }

            // ================================================================
            // Len / type introspection
            // ================================================================
            "len" => {
                // # operator (LOP_LENGTH) is a single opcode — 2-3x faster than
                // molt_len() function call. Use # directly when type is known;
                // fall back to molt_len() for unknown types (handles 0 for non-table/string).
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(obj) = args.first() {
                    let obj_s = sanitize_ident(obj);
                    let is_known_lennable = matches!(
                        self.scalar_plan.name_container_kind(obj),
                        Some(ContainerKind::List | ContainerKind::Tuple | ContainerKind::Str)
                    );
                    if is_known_lennable {
                        self.emit_line(&format!("local {out} = #{obj_s}"));
                    } else {
                        self.emit_line(&format!("local {out} = molt_len({obj_s})"));
                    }
                }
            }
            "id" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(obj) = args.first() {
                    self.emit_line(&format!("local {out} = tostring({})", sanitize_ident(obj)));
                }
            }
            "ord" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!("local {out} = molt_ord({})", sanitize_ident(val)));
                }
            }
            "ord_at" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let container = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    self.emit_line(&format!("local {out} = molt_ord_at({container}, {key})"));
                }
            }
            "chr" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = string.char({})",
                        sanitize_ident(val)
                    ));
                }
            }
            "type_of" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(obj) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = molt_type_of({})",
                        sanitize_ident(obj)
                    ));
                }
            }
            "isinstance" | "issubclass" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let obj = sanitize_ident(&args[0]);
                    let cls = sanitize_ident(&args[1]);
                    let helper = if op.kind == "isinstance" {
                        "molt_isinstance"
                    } else {
                        "molt_issubclass"
                    };
                    self.emit_line(&format!("local {out} = {helper}({obj}, {cls})"));
                } else {
                    self.emit_line(&format!("local {out} = molt_isinstance(nil, nil)"));
                }
            }
            "exception_match_builtin" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(exc) = args.first() {
                    let class_name = op.s_value.as_deref().unwrap_or("Exception");
                    self.emit_line(&format!(
                        "local {out} = molt_exception_match({}, \"{class_name}\")",
                        sanitize_ident(exc)
                    ));
                } else {
                    self.emit_line(&format!("local {out} = false"));
                }
            }

            // ================================================================
            // Type casting
            // ================================================================
            "int_from_obj" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!("local {out} = molt_int({})", sanitize_ident(val)));
                }
            }
            "int_from_str_of_obj" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let val = sanitize_ident(&args[0]);
                    let base = sanitize_ident(&args[1]);
                    let has_base = sanitize_ident(&args[2]);
                    self.emit_line(&format!(
                        "local {out} = if molt_bool({has_base}) then math.floor(tonumber(molt_str({val}), molt_int({base})) or 0) else molt_int(molt_str({val}))"
                    ));
                }
            }
            "float_from_obj" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = molt_float({})",
                        sanitize_ident(val)
                    ));
                }
            }
            "str_from_obj" | "ascii_from_obj" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!("local {out} = molt_str({})", sanitize_ident(val)));
                }
            }
            "repr_from_obj" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!("local {out} = molt_repr({})", sanitize_ident(val)));
                }
            }
            "bytes_from_obj" | "bytes_from_str" | "bytearray_from_str" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!("local {out} = tostring({})", sanitize_ident(val)));
                }
            }
            "bytearray_from_obj" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    let val = sanitize_ident(val);
                    self.emit_line(&format!(
                        "local {out} = if type({val}) == \"number\" then string.rep(string.char(0), math.max(0, {val})) else tostring({val})"
                    ));
                }
            }

            // ================================================================
            // Print
            // ================================================================
            "print" => {
                let call_args = op
                    .args
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(|a| sanitize_ident(a))
                    .collect::<Vec<_>>()
                    .join(", ");
                self.emit_line(&format!("molt_print({call_args})"));
            }
            "print_newline" => {
                self.emit_line("molt_print()");
            }

            // ================================================================
            // Function/class/module objects
            // ================================================================
            "func_new" | "func_new_closure" | "code_new" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    let name = op
                        .s_value
                        .as_deref()
                        .map(sanitize_ident)
                        .unwrap_or_else(|| "nil".to_string());
                    self.emit_line(&format!("local {out} = {name}"));
                }
            }
            "builtin_func" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    let s_val = op.s_value.as_deref().unwrap_or("");
                    // Map known Python builtins to Luau function references.
                    // The call_func IR op passes (args_tuple, kwargs, varkw),
                    // so we wrap Luau functions in a closure that unpacks the
                    // positional args tuple for the correct calling convention.
                    let mapped = match s_val {
                        "molt_max_builtin" => {
                            "function(a, ...) return math.max(table.unpack(a)) end"
                        }
                        "molt_min_builtin" => {
                            "function(a, ...) return math.min(table.unpack(a)) end"
                        }
                        "molt_abs_builtin" => "function(a, ...) return math.abs(a[1]) end",
                        "molt_round_builtin" => "function(a, ...) return math.round(a[1]) end",
                        "molt_print_builtin" => {
                            "function(a, ...) return molt_print(table.unpack(a)) end"
                        }
                        "molt_len" => "function(a, ...) return molt_len(a[1]) end",
                        "molt_int_builtin" | "molt_int" => {
                            "function(a, ...) return molt_int(a[1]) end"
                        }
                        "molt_float_builtin" | "molt_float" => {
                            "function(a, ...) return molt_float(a[1]) end"
                        }
                        "molt_str_builtin" | "molt_str" => {
                            "function(a, ...) return molt_str(a[1]) end"
                        }
                        "molt_bool_builtin" | "molt_bool" => {
                            "function(a, ...) return molt_bool(a[1]) end"
                        }
                        "molt_sum_builtin" => "function(a, ...) return molt_sum(a[1]) end",
                        "molt_any_builtin" => "function(a, ...) return molt_any(a[1]) end",
                        "molt_all_builtin" => "function(a, ...) return molt_all(a[1]) end",
                        "molt_sorted_builtin" => {
                            "function(a, ...) return molt_sorted(table.unpack(a)) end"
                        }
                        "molt_reversed_builtin" => {
                            "function(a, ...) return molt_reversed(a[1]) end"
                        }
                        "molt_enumerate_builtin" => {
                            "function(a, ...) return molt_enumerate(table.unpack(a)) end"
                        }
                        "molt_zip_builtin" => {
                            "function(a, ...) return molt_zip(table.unpack(a)) end"
                        }
                        "molt_isinstance" => {
                            "function(a, ...) return molt_isinstance(a[1], a[2]) end"
                        }
                        "molt_issubclass" => {
                            "function(a, ...) return molt_issubclass(a[1], a[2]) end"
                        }
                        "molt_classmethod_new" => {
                            "function(a, ...) return {__molt_descriptor_kind=\"classmethod\", __func=a[1]} end"
                        }
                        "molt_staticmethod_new" => {
                            "function(a, ...) return {__molt_descriptor_kind=\"staticmethod\", __func=a[1]} end"
                        }
                        "molt_property_new" => {
                            "function(a, ...) return {__molt_descriptor_kind=\"property\", __get=a[1], __set=a[2], __del=a[3]} end"
                        }
                        "molt_hash_builtin" => "function(a, ...) return molt_hash(a[1]) end",
                        "molt_ord" => "function(a, ...) return molt_ord(a[1]) end",
                        "molt_chr" => "function(a, ...) return string.char(a[1]) end",
                        "molt_repr_builtin" => "function(a, ...) return molt_repr(a[1]) end",
                        "molt_id" => "function(a, ...) return molt_id(a[1]) end",
                        "molt_callable_builtin" => {
                            "function(a, ...) return molt_callable(a[1]) end"
                        }
                        "molt_iter_checked" => "function(a, ...) return molt_iter(a[1]) end",
                        "molt_next_builtin" => {
                            "function(a, ...) return molt_next(table.unpack(a)) end"
                        }
                        "molt_getattr_builtin" => {
                            "function(a, ...) local value = molt_get_attr(a[1], a[2]); if value ~= nil then return value end; if a[3] ~= nil then return a[3] end; error({__type=\"AttributeError\", __msg=tostring(a[2])}) end"
                        }
                        "molt_set_attr_name" => {
                            "function(a, ...) return molt_set_attr(a[1], a[2], a[3]) end"
                        }
                        "molt_del_attr_name" => {
                            "function(a, ...) return molt_del_attr(a[1], a[2]) end"
                        }
                        "molt_has_attr_name" => {
                            "function(a, ...) return molt_has_attr(a[1], a[2]) end"
                        }
                        "molt_divmod_builtin" => {
                            "function(a, ...) return molt_divmod(a[1], a[2]) end"
                        }
                        "molt_hex_builtin" => "function(a, ...) return molt_hex(a[1]) end",
                        "molt_oct_builtin" => "function(a, ...) return molt_oct(a[1]) end",
                        "molt_bin_builtin" => "function(a, ...) return molt_bin(a[1]) end",
                        "molt_ascii_from_obj" => "function(a, ...) return molt_ascii(a[1]) end",
                        "molt_format_builtin" => {
                            "function(a, ...) return molt_format(table.unpack(a)) end"
                        }
                        "molt_dir_builtin" => "function(a, ...) return molt_dir(a[1]) end",
                        "molt_vars_builtin" => "function(a, ...) return molt_vars(a[1]) end",
                        // Runtime intrinsics that have no Luau equivalent.
                        "molt_function_init_metadata_packed"
                        | "molt_function_set_builtin"
                        | "molt_function_set_defaults"
                        | "molt_open_builtin"
                        | "molt_aiter"
                        | "molt_anext_builtin" => "nil",
                        _ => "nil",
                    };
                    self.emit_line(&format!("local {out} = {mapped}"));
                }
            }
            "class_new" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = {{__molt_is_type = true}}"));
                // Self-referential __index enables Luau's inline caching
                self.emit_line(&format!("{out}.__index = {out}"));
            }
            "module_new" | "object_new" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = {{}}"));
            }
            "builtin_type" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                let tag = args
                    .first()
                    .map(|value| sanitize_ident(value))
                    .unwrap_or_else(|| "nil".to_string());
                self.emit_line(&format!("local {out} = molt_builtin_type({tag})"));
            }
            "bound_method_new" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    // IR contract: args[0] = function, args[1] = self.
                    let method = sanitize_ident(&args[0]);
                    let obj = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "local {out} = function(...) local __m = {method}; if __m then return __m({obj}, ...) end; return nil end"
                    ));
                } else {
                    self.emit_line(&format!("local {out} = nil -- bound_method missing args"));
                }
            }
            "class_set_base" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let class = sanitize_ident(&args[0]);
                    let base = sanitize_ident(&args[1]);
                    self.emit_line(&format!("setmetatable({class}, {{__index = {base}}})"));
                }
            }
            "object_set_class" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let obj = sanitize_ident(&args[0]);
                    let class = sanitize_ident(&args[1]);
                    self.emit_line(&format!("setmetatable({obj}, {class})"));
                }
            }
            "class_layout_version" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    let args = op.args.as_deref().unwrap_or(&[]);
                    if let Some(class) = args.first() {
                        let class = sanitize_ident(class);
                        self.emit_line(&format!(
                            "local {out} = if type({class}) == \"table\" and type({class}.__molt_layout_version) == \"number\" then {class}.__molt_layout_version else 0"
                        ));
                    } else {
                        self.emit_line(&format!("local {out} = 0"));
                    }
                }
            }
            "class_set_layout_version" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let class = sanitize_ident(&args[0]);
                    let version = sanitize_ident(&args[1]);
                    self.emit_line("do");
                    self.push_indent();
                    self.emit_line(&format!("local __cls = {class}"));
                    self.emit_line(&format!("local __version = {version}"));
                    self.emit_line("if type(__version) ~= \"number\" or __version < 0 then");
                    self.push_indent();
                    self.emit_line(
                        "error({__type=\"TypeError\", __msg=\"layout version must be int\"})",
                    );
                    self.pop_indent();
                    self.emit_line("end");
                    self.emit_line("if type(__cls) == \"table\" then");
                    self.push_indent();
                    self.emit_line("__cls.__molt_layout_version = __version");
                    self.pop_indent();
                    self.emit_line("end");
                    self.pop_indent();
                    self.emit_line("end");
                }
                if let Some(ref out_name) = op.out
                    && out_name != "none"
                {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "class_merge_layout" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let class = sanitize_ident(&args[0]);
                    let offsets = sanitize_ident(&args[1]);
                    let size = sanitize_ident(&args[2]);
                    self.emit_line("do");
                    self.push_indent();
                    self.emit_line(&format!("local __cls = {class}"));
                    self.emit_line(&format!("local __offsets = {offsets}"));
                    self.emit_line(&format!("local __size = {size}"));
                    self.emit_line("if type(__cls) ~= \"table\" then");
                    self.push_indent();
                    self.emit_line(
                        "error({__type=\"TypeError\", __msg=\"class layout merge expects type\"})",
                    );
                    self.pop_indent();
                    self.emit_line("end");
                    self.emit_line("if type(__size) ~= \"number\" or __size < 0 then");
                    self.push_indent();
                    self.emit_line(
                        "error({__type=\"TypeError\", __msg=\"__molt_layout_size__ must be int\"})",
                    );
                    self.pop_indent();
                    self.emit_line("end");
                    self.emit_line("local __merged = __cls.__molt_field_offsets__");
                    self.emit_line("if __offsets ~= nil then");
                    self.push_indent();
                    self.emit_line("if type(__offsets) ~= \"table\" then");
                    self.push_indent();
                    self.emit_line(
                        "error({__type=\"TypeError\", __msg=\"__molt_field_offsets__ must be dict or None\"})",
                    );
                    self.pop_indent();
                    self.emit_line("end");
                    self.emit_line("if type(__merged) ~= \"table\" then");
                    self.push_indent();
                    self.emit_line("__merged = {}");
                    self.emit_line("__cls.__molt_field_offsets__ = __merged");
                    self.pop_indent();
                    self.emit_line("end");
                    self.emit_line("for __k, __v in pairs(__offsets) do");
                    self.push_indent();
                    self.emit_line(
                        "if type(__k) == \"string\" and type(__v) == \"number\" and __v >= 0 and __merged[__k] == nil then",
                    );
                    self.push_indent();
                    self.emit_line("__merged[__k] = __v");
                    self.pop_indent();
                    self.emit_line("end");
                    self.pop_indent();
                    self.emit_line("end");
                    self.pop_indent();
                    self.emit_line("elseif __merged ~= nil and type(__merged) ~= \"table\" then");
                    self.push_indent();
                    self.emit_line(
                        "error({__type=\"TypeError\", __msg=\"__molt_field_offsets__ must be dict or None\"})",
                    );
                    self.pop_indent();
                    self.emit_line("end");
                    self.emit_line("local __layout_size = __cls.__molt_layout_size__");
                    self.emit_line(
                        "if type(__layout_size) ~= \"number\" or __layout_size < 0 then",
                    );
                    self.push_indent();
                    self.emit_line("__layout_size = 0");
                    self.pop_indent();
                    self.emit_line("end");
                    self.emit_line("if __size > __layout_size then");
                    self.push_indent();
                    self.emit_line("__layout_size = __size");
                    self.pop_indent();
                    self.emit_line("end");
                    self.emit_line("if type(__merged) == \"table\" then");
                    self.push_indent();
                    self.emit_line("for _, __offset in pairs(__merged) do");
                    self.push_indent();
                    self.emit_line("if type(__offset) == \"number\" and __offset >= 0 then");
                    self.push_indent();
                    self.emit_line("local __end = __offset + 16");
                    self.emit_line("if __end > __layout_size then");
                    self.push_indent();
                    self.emit_line("__layout_size = __end");
                    self.pop_indent();
                    self.emit_line("end");
                    self.pop_indent();
                    self.emit_line("end");
                    self.pop_indent();
                    self.emit_line("end");
                    self.pop_indent();
                    self.emit_line("end");
                    self.emit_line("if __layout_size == 0 then");
                    self.push_indent();
                    self.emit_line("__layout_size = 8");
                    self.pop_indent();
                    self.emit_line("end");
                    self.emit_line("__cls.__molt_layout_size__ = __layout_size");
                    self.pop_indent();
                    self.emit_line("end");
                }
                if let Some(ref out_name) = op.out
                    && out_name != "none"
                {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "class_apply_set_name" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(class) = args.first() {
                    let class = sanitize_ident(class);
                    self.emit_line(&format!("molt_class_apply_set_name({class})"));
                }
                if let Some(ref out_name) = op.out
                    && out_name != "none"
                {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "module_import" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    let args = op.args.as_deref().unwrap_or(&[]);
                    let module_name = op.s_value.as_deref().unwrap_or("");
                    let mapped = match module_name {
                        "math" => "molt_math",
                        "json" => "json",
                        "time" => "molt_time",
                        "os" => "molt_os",
                        "sys" => "molt_sys_ensure_module()",
                        _ => "",
                    };
                    if !mapped.is_empty() {
                        self.emit_line(&format!("local {out} = {mapped}"));
                    } else if let Some(name_var) = args.first() {
                        let nv = sanitize_ident(name_var);
                        self.emit_line(&format!("local {out} = molt_luau_import_module({nv})"));
                    } else {
                        self.emit_line(&format!("local {out} = nil"));
                    }
                }
            }
            "module_cache_get" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    let args = op.args.as_deref().unwrap_or(&[]);
                    if let Some(name_var) = args.first() {
                        let nv = sanitize_ident(name_var);
                        self.emit_line(&format!("local {out} = molt_module_cache[{nv}]"));
                    } else {
                        self.emit_line(&format!("local {out} = nil"));
                    }
                }
            }
            "module_cache_set" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let name = sanitize_ident(&args[0]);
                    let module = sanitize_ident(&args[1]);
                    self.emit_line(&format!("molt_module_cache[{name}] = {module}"));
                }
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "module_cache_del" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(name_var) = args.first() {
                    let name = sanitize_ident(name_var);
                    self.emit_line(&format!("molt_module_cache[{name}] = nil"));
                }
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "module_import_star" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "module_get_global" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let module = sanitize_ident(&args[0]);
                    let name_var = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "local {out} = molt_module_get_global({module}, {name_var})"
                    ));
                } else {
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "module_get_name" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let module = sanitize_ident(&args[0]);
                    let name_var = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "local {out} = molt_module_get_name({module}, {name_var})"
                    ));
                } else {
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "module_get_attr" | "module_import_from" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(attr_str) = op.s_value.as_deref().filter(|s| !s.is_empty()) {
                    // Static attribute name — use dot access.
                    let attr = sanitize_ident(attr_str);
                    if let Some(module) = args.first() {
                        let module = sanitize_ident(module);
                        self.emit_line(&format!("local {out} = {module}.{attr}"));
                    }
                } else if args.len() >= 2 {
                    // module_get_attr: args[0] = module table, args[1] = attr name var.
                    // Look up attribute directly on the module.
                    let module = sanitize_ident(&args[0]);
                    let attr_var = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "local {out} = if type({module}) == \"table\" then {module}[{attr_var}] else nil"
                    ));
                } else if let Some(module) = args.first() {
                    let module = sanitize_ident(module);
                    self.emit_line(&format!("local {out} = {module}"));
                }
            }
            "module_set_attr" => {
                // Store user-visible variables into the module cache so they can
                // be accessed by name later (e.g., `nums = [1,2,3]`).
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let module = sanitize_ident(&args[0]);
                    let attr_name = sanitize_ident(&args[1]);
                    let value = sanitize_ident(&args[2]);
                    // Only emit for non-dunder attributes (user variables).
                    // Dunder metadata writes are unnecessary in Luau.
                    self.emit_line(&format!(
                        "if type({module}) == \"table\" then {module}[{attr_name}] = {value} end"
                    ));
                }
            }
            "module_del_global" | "module_del_global_if_present" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let module = sanitize_ident(&args[0]);
                    let name = sanitize_ident(&args[1]);
                    let missing_ok = op.kind == "module_del_global_if_present";
                    if let Some(ref out_name) = op.out
                        && out_name != "none"
                    {
                        let out = sanitize_ident(out_name);
                        self.emit_line(&format!(
                            "local {out} = molt_module_del_global({module}, {name}, {missing_ok})"
                        ));
                    } else {
                        self.emit_line(&format!(
                            "molt_module_del_global({module}, {name}, {missing_ok})"
                        ));
                    }
                }
            }

            // ================================================================
            // Alloc / memory (table stubs)
            // ================================================================
            "alloc_class" | "alloc_class_trusted" | "alloc_class_static" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(class_var) = args.first() {
                    let class_ref = sanitize_ident(class_var);
                    self.emit_line(&format!("local {out} = setmetatable({{}}, {class_ref})"));
                } else if let Some(ref class_name) = op.s_value {
                    let class_ref = sanitize_ident(class_name);
                    self.emit_line(&format!("local {out} = setmetatable({{}}, {class_ref})"));
                } else {
                    self.emit_line(&format!("local {out} = {{}}"));
                }
            }
            "alloc" | "alloc_task" => {
                let out = self.out_var(op);
                // If this is a genexpr/listcomp task, create a coroutine-based
                // iterator that eagerly collects all yielded values into a list.
                let task_func = op.s_value.as_deref().unwrap_or("");
                if task_func.contains("genexpr") || task_func.contains("listcomp") {
                    // Create a list by running the generator to completion.
                    // The genexpr function uses state_yield to produce values
                    // as {value, false} tuples and returns {nil, true} when done.
                    let func_name = sanitize_ident(task_func);
                    self.emit_line(&format!(
                        "local {out} = (function()\n\
                         \t\tlocal __result = {{}}\n\
                         \t\tlocal __n = 0\n\
                         \t\tlocal __co = coroutine.wrap({func_name})\n\
                         \t\twhile true do\n\
                         \t\t\tlocal __item = __co()\n\
                         \t\t\tif __item == nil then break end\n\
                         \t\t\tif type(__item) == \"table\" then\n\
                         \t\t\t\tif __item[2] == true then break end\n\
                         \t\t\t\t__n += 1; __result[__n] = __item[1]\n\
                         \t\t\telse\n\
                         \t\t\t\t__n += 1; __result[__n] = __item\n\
                         \t\t\tend\n\
                         \t\tend\n\
                         \t\treturn __result\n\
                         \tend)()"
                    ));
                } else {
                    self.emit_line(&format!("local {out} = {{}}"));
                }
            }

            // ================================================================
            // Dataclass
            // ================================================================
            "dataclass_new" | "dataclass_new_values" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = {{}}"));
            }
            "dataclass_get" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(obj) = args.first() {
                    let obj = sanitize_ident(obj);
                    if let Some(attr) = op.s_value.as_deref() {
                        self.emit_line(&format!("local {out} = {obj}.{}", sanitize_ident(attr)));
                    } else if args.len() >= 2 {
                        let idx = sanitize_ident(&args[1]);
                        self.emit_line(&format!("local {out} = {obj}[{idx}]"));
                    }
                }
            }
            "dataclass_set" | "dataclass_set_class" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let obj = sanitize_ident(&args[0]);
                    if let Some(attr) = op.s_value.as_deref() {
                        let val = sanitize_ident(&args[1]);
                        self.emit_line(&format!("{obj}.{} = {val}", sanitize_ident(attr)));
                    }
                }
            }

            // ================================================================
            // Exception handling (stubs)
            // ================================================================
            "exception_push"
            | "exception_pop"
            | "exception_stack_clear"
            | "exception_stack_enter"
            | "exception_stack_exit"
            | "exception_set_last"
            | "exception_set_value"
            | "exception_set_cause"
            | "exception_context_set"
            | "exception_stack_set_depth" => {
                // Exception bookkeeping — no Luau equivalent.
            }
            "exception_clear" => {
                // Clear the pcall error so subsequent exception_last returns nil.
                let explicit_pcall = op.value.and_then(|n| u32::try_from(n).ok());
                if let Some(n) = explicit_pcall.or_else(|| {
                    (!self.inside_pcall_body)
                        .then(|| self.try_depth_counter.last().copied())
                        .flatten()
                }) {
                    self.emit_line(&format!("__err_{n} = nil"));
                }
            }
            "drop_inserted"
            | "exception_region_drops_inserted"
            | "inc_ref"
            | "dec_ref"
            | "release" => {
                // Shared TIR DropInsertion artifacts are consumed explicitly.
                // Luau is GC-managed, so RC barriers and drop-fact markers are
                // semantic no-ops here rather than unsupported transport.
            }
            "exception_new" | "exception_new_builtin" | "exception_new_from_class" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                let class_name = op.s_value.as_deref().unwrap_or("Exception");
                let msg = args
                    .first()
                    .map(|a| sanitize_ident(a))
                    .unwrap_or_else(|| "\"error\"".to_string());
                self.emit_line(&format!(
                    "local {out} = {{__type = \"{class_name}\", __msg = {msg}}}"
                ));
            }
            "exception_new_builtin_empty" => {
                let out = self.out_var(op);
                let class_name = op.s_value.as_deref().unwrap_or("Exception");
                self.emit_line(&format!(
                    "local {out} = {{__type = \"{class_name}\", __msg = \"\"}}"
                ));
            }
            "exception_new_builtin_one" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                let class_name = op.s_value.as_deref().unwrap_or("Exception");
                let msg = args
                    .first()
                    .map(|a| sanitize_ident(a))
                    .unwrap_or_else(|| "\"\"".to_string());
                self.emit_line(&format!(
                    "local {out} = {{__type = \"{class_name}\", __msg = {msg}}}"
                ));
            }
            "raise" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!("error({})", sanitize_ident(val)));
                } else {
                    self.emit_line("error(\"raised\")");
                }
            }
            "check_exception" => {
                // Suppress — exception handler jumps are no-ops in Luau.
            }
            "exception_last" | "exception_last_pending" | "exception_finally_pending_observer" => {
                let out = self.out_var(op);
                if let Some(n) = op.value.and_then(|n| u32::try_from(n).ok()) {
                    self.emit_line(&format!("local {out} = __err_{n}"));
                } else if !self.inside_pcall_body {
                    if let Some(&n) = self.try_depth_counter.last() {
                        // After pcall — read the captured error.
                        self.emit_line(&format!("local {out} = __err_{n}"));
                    } else {
                        self.emit_line(&format!("local {out} = nil -- [exception_last]"));
                    }
                } else {
                    // Inside pcall body — no exception yet, return nil.
                    self.emit_line(&format!("local {out} = nil -- [exception_last]"));
                }
            }
            "exception_kind" => {
                // exception_kind extracts the type from an exception object.
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(exc_var) = args.first() {
                    let exc = sanitize_ident(exc_var);
                    self.emit_line(&format!("local {out} = molt_exception_kind({exc})"));
                } else {
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "exception_class" => {
                // exception_class returns the class name for isinstance matching.
                // args[0] is already the class name string — pass it through.
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(class_var) = args.first() {
                    let cls = sanitize_ident(class_var);
                    self.emit_line(&format!("local {out} = {cls}"));
                } else {
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "exception_message" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(exc_var) = args.first() {
                    let exc = sanitize_ident(exc_var);
                    self.emit_line(&format!(
                        "local {out} = (type({exc}) == \"table\" and {exc}.__msg or tostring({exc}))"
                    ));
                } else {
                    self.emit_line(&format!("local {out} = nil -- [exception_message]"));
                }
            }
            "exception_stack_depth" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = 0"));
            }
            "exceptiongroup_match" | "exceptiongroup_combine" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = nil -- [{}]", op.kind));
            }

            // ================================================================
            // Context manager stubs
            // ================================================================
            "context_null" | "context_enter" | "context_exit" | "context_closing"
            | "context_unwind" | "context_unwind_to" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil -- [context: {}]", op.kind));
                }
            }
            "context_depth" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = 0"));
            }

            // ================================================================
            // Async/generator stubs
            // ================================================================
            "state_yield" => {
                // Generator yield: yield the value to the coroutine consumer.
                // args[0] is the value (typically a {result, false} tuple).
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!("coroutine.yield({})", sanitize_ident(val)));
                }
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil -- [async: state_yield]"));
                }
            }
            "state_switch"
            | "state_transition"
            | "chan_new"
            | "chan_drop"
            | "chan_send_yield"
            | "chan_recv_yield"
            | "cancel_token_new"
            | "cancel_token_clone"
            | "cancel_token_drop"
            | "cancel_token_cancel"
            | "cancel_token_is_cancelled"
            | "cancel_token_set_current"
            | "cancel_token_get_current"
            | "cancelled"
            | "cancel_current"
            | "future_cancel"
            | "future_cancel_msg"
            | "future_cancel_clear"
            | "promise_new"
            | "promise_set_result"
            | "promise_set_exception"
            | "thread_submit"
            | "task_register_token_owned" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil -- [async: {}]", op.kind));
                }
            }
            "is_native_awaitable" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = false"));
                }
            }

            // ================================================================
            // File I/O stubs
            // ================================================================
            "file_open" | "file_read" | "file_write" | "file_close" | "file_flush" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil -- [file: {}]", op.kind));
                }
            }

            // ================================================================
            // Misc intrinsics
            // ================================================================
            "getargv" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = {{}}"));
                }
            }
            "sys_executable" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = \"\""));
                }
            }
            "getframe" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "bridge_unavailable" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                let msg = args
                    .first()
                    .map(|arg| {
                        format!(
                            "\"Molt bridge unavailable: \" .. tostring({})",
                            sanitize_ident(arg)
                        )
                    })
                    .unwrap_or_else(|| "\"Molt bridge unavailable\"".to_string());
                let diagnostic = format!("{{__type=\"RuntimeError\", __msg={msg}}}");
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out}: any = error({diagnostic})"));
                } else {
                    self.emit_line(&format!("error({diagnostic})"));
                }
            }
            "super_new" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let class = sanitize_ident(&args[0]);
                    self.emit_line(&format!(
                        "local {out} = setmetatable({{}}, {{__index = getmetatable({class}).__index}})"
                    ));
                } else {
                    self.emit_line(&format!("local {out} = {{}}"));
                }
            }
            "classmethod_new" | "staticmethod_new" | "property_new" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    let args = op.args.as_deref().unwrap_or(&[]);
                    match op.kind.as_str() {
                        "classmethod_new" => {
                            let func = args
                                .first()
                                .map(|arg| sanitize_ident(arg))
                                .unwrap_or_else(|| "nil".to_string());
                            self.emit_line(&format!(
                                "local {out} = {{__molt_descriptor_kind=\"classmethod\", __func={func}}}"
                            ));
                        }
                        "staticmethod_new" => {
                            let func = args
                                .first()
                                .map(|arg| sanitize_ident(arg))
                                .unwrap_or_else(|| "nil".to_string());
                            self.emit_line(&format!(
                                "local {out} = {{__molt_descriptor_kind=\"staticmethod\", __func={func}}}"
                            ));
                        }
                        "property_new" => {
                            let get = args
                                .first()
                                .map(|arg| sanitize_ident(arg))
                                .unwrap_or_else(|| "nil".to_string());
                            let set = args
                                .get(1)
                                .map(|arg| sanitize_ident(arg))
                                .unwrap_or_else(|| "nil".to_string());
                            let del = args
                                .get(2)
                                .map(|arg| sanitize_ident(arg))
                                .unwrap_or_else(|| "nil".to_string());
                            self.emit_line(&format!(
                                "local {out} = {{__molt_descriptor_kind=\"property\", __get={get}, __set={set}, __del={del}}}"
                            ));
                        }
                        _ => unreachable!(),
                    }
                }
            }

            // ================================================================
            // Closure/code internals
            // ================================================================
            "fn_ptr_code_set"
            | "asyncgen_locals_register"
            | "gen_locals_register"
            | "function_closure_bits" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil -- [internal: {}]", op.kind));
                }
            }
            "code_slot_set" | "code_slots_init" | "frame_locals_set" => {
                if let Some(ref out_name) = op.out
                    && out_name != "none"
                {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "trace_enter_slot" | "trace_exit" => {
                if let Some(ref out_name) = op.out
                    && out_name != "none"
                {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil"));
                }
            }

            // ================================================================
            // Line info (debug)
            // ================================================================
            "line" => {
                // Skip line markers in production output — they add
                // ~3% to file size with no runtime benefit.
                // Uncomment for debugging: self.emit_line(&format!("-- line {val}"));
            }

            // ================================================================
            // Raw int bridge (no-op in Luau — values are already unboxed)
            // ================================================================
            "unbox_to_raw_int" | "box_from_raw_int" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!("local {out} = {}", sanitize_ident(val)));
                }
            }

            // ================================================================
            // Vectorized reduction ops (emit stubs)
            // ================================================================
            kind if kind.starts_with("vec_sum_")
                || kind.starts_with("vec_prod_")
                || kind.starts_with("vec_min_")
                || kind.starts_with("vec_max_") =>
            {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                // Vectorized reduction: args[0] = iterable.
                // Emit as a Luau loop that computes the reduction, returning
                // a tuple-like table {result, false} on success.
                // The subsequent get_item(out, 0) and get_item(out, 1) unpack it.
                if let Some(iterable) = args.first() {
                    let iterable = sanitize_ident(iterable);
                    let (init, body_op) = if kind.starts_with("vec_sum_") {
                        ("0", "acc = acc + v")
                    } else if kind.starts_with("vec_prod_") {
                        ("1", "acc = acc * v")
                    } else if kind.starts_with("vec_min_") {
                        ("math.huge", "if v < acc then acc = v end")
                    } else {
                        ("-math.huge", "if v > acc then acc = v end")
                    };
                    self.emit_line(&format!("local {out}; do -- [vectorized: {kind}]"));
                    self.push_indent();
                    self.emit_line(&format!(
                        "if type({iterable}) == \"table\" and #({iterable}) > 0 then"
                    ));
                    self.push_indent();
                    self.emit_line(&format!("local acc = {init}"));
                    self.emit_line(&format!(
                        "for __vi = 1, #{iterable} do local v = {iterable}[__vi]; {body_op} end"
                    ));
                    self.emit_line(&format!("{out} = {{acc, false}}"));
                    self.pop_indent();
                    self.emit_line("else");
                    self.push_indent();
                    self.emit_line(&format!("{out} = {{nil, true}}"));
                    self.pop_indent();
                    self.emit_line("end");
                    self.pop_indent();
                    self.emit_line("end");
                } else {
                    // No iterable arg — emit nil tuple (will fall through to loop).
                    self.emit_line(&format!(
                        "local {out} = {{nil, true}} -- [vectorized: {kind}]"
                    ));
                }
            }

            // ================================================================
            // Serialization stubs
            // ================================================================
            "json_parse" | "msgpack_parse" | "cbor_parse" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil -- [{}]", op.kind));
                }
            }
            "invoke_ffi" => {
                let diagnostic =
                    "{__type=\"RuntimeError\", __msg=\"Luau target does not support FFI\"}";
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out}: any = error({diagnostic})"));
                } else {
                    self.emit_line(&format!("error({diagnostic})"));
                }
            }

            // ================================================================
            // Memoryview / complex / bytearray stubs
            // ================================================================
            "memoryview_new" | "memoryview_tobytes" | "memoryview_cast" | "complex_from_obj" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil -- [{}]", op.kind));
                }
            }
            "intarray_from_seq" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(seq) = args.first() {
                    let seq = sanitize_ident(seq);
                    self.emit_line(&format!("local {out}"));
                    self.emit_line("do");
                    self.push_indent();
                    self.emit_line(&format!("local __seq = {seq}"));
                    self.emit_line("if type(__seq) == \"table\" then");
                    self.push_indent();
                    self.emit_line("local __arr = {}");
                    self.emit_line("local __ok = true");
                    self.emit_line("for __i = 1, #__seq do");
                    self.push_indent();
                    self.emit_line("local __v = __seq[__i]");
                    self.emit_line("if type(__v) == \"number\" and math.floor(__v) == __v then");
                    self.push_indent();
                    self.emit_line("__arr[__i] = __v");
                    self.pop_indent();
                    self.emit_line("else");
                    self.push_indent();
                    self.emit_line("__ok = false");
                    self.emit_line("break");
                    self.pop_indent();
                    self.emit_line("end");
                    self.pop_indent();
                    self.emit_line("end");
                    self.emit_line(&format!("{out} = if __ok then __arr else nil"));
                    self.pop_indent();
                    self.emit_line("else");
                    self.push_indent();
                    self.emit_line(&format!("{out} = nil"));
                    self.pop_indent();
                    self.emit_line("end");
                    self.pop_indent();
                    self.emit_line("end");
                } else {
                    self.emit_line(&format!("local {out} = nil"));
                }
            }

            "bytearray_fill_range" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 4 {
                    let bytearray = sanitize_ident(&args[0]);
                    let start = sanitize_ident(&args[1]);
                    let stop = sanitize_ident(&args[2]);
                    let value = sanitize_ident(&args[3]);
                    self.emit_line(&format!(
                        "do local __ba = {bytearray}; local __start = {start}; local __stop = {stop}; local __byte = {value}; if __byte < 0 or __byte > 255 then error({{__type=\"ValueError\", __msg=\"byte must be in range(0, 256)\"}}) end; if __start < 0 or __stop < __start or __stop > #__ba then error({{__type=\"IndexError\", __msg=\"bytearray fill range out of range\"}}) end; {bytearray} = string.sub(__ba, 1, __start) .. string.rep(string.char(__byte), __stop - __start) .. string.sub(__ba, __stop + 1) end"
                    ));
                }
            }

            // ================================================================
            // String ops
            // ================================================================
            "string_join" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let sep = sanitize_ident(&args[0]);
                    let list = sanitize_ident(&args[1]);
                    self.emit_line(&format!("local {out} = table.concat({list}, {sep})"));
                }
            }
            "string_format" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if !args.is_empty() {
                    let fmt_str = sanitize_ident(&args[0]);
                    let fmt_args = args[1..]
                        .iter()
                        .map(|a| sanitize_ident(a))
                        .collect::<Vec<_>>()
                        .join(", ");
                    if fmt_args.is_empty() {
                        self.emit_line(&format!("local {out} = {fmt_str}"));
                    } else {
                        self.emit_line(&format!(
                            "local {out} = string.format({fmt_str}, {fmt_args})"
                        ));
                    }
                }
            }
            "string_strip" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(s) = args.first() {
                    let s = sanitize_ident(s);
                    self.emit_line(&format!("local {out} = ({s}:match(\"^%s*(.-)%s*$\"))"));
                }
            }
            "string_lstrip" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(s) = args.first() {
                    let s = sanitize_ident(s);
                    self.emit_line(&format!("local {out} = ({s}:match(\"^%s*(.+)\") or \"\")"));
                }
            }
            "string_rstrip" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(s) = args.first() {
                    let s = sanitize_ident(s);
                    self.emit_line(&format!("local {out} = ({s}:match(\"^(.-)%s*$\"))"));
                }
            }
            "string_upper" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(s) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = string.upper({})",
                        sanitize_ident(s)
                    ));
                }
            }
            "string_lower" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(s) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = string.lower({})",
                        sanitize_ident(s)
                    ));
                }
            }
            "string_startswith" | "string_startswith_slice" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let s = sanitize_ident(&args[0]);
                    let prefix = sanitize_ident(&args[1]);
                    let prefix_is_tuple = self.tuple_vars.contains(&args[1]);
                    if prefix_is_tuple && args.len() >= 4 {
                        let start = sanitize_ident(&args[2]);
                        let end = sanitize_ident(&args[3]);
                        self.emit_line(&format!(
                            "local {out}; do local __n = #{s}; local __start_raw = if {start} < 0 then __n + {start} else {start}; local __start = __start_raw; if __start < 0 then __start = 0 end; if __start > __n then __start = __n end; local __end = if {end} < 0 then __n + {end} else {end}; if __end < __start then __end = __start end; if __end > __n then __end = __n end; local __slice = string.sub({s}, __start + 1, __end); {out} = false; for __i = 1, #{prefix} do local __cand = {prefix}[__i]; if type(__cand) ~= \"string\" then error({{__type=\"TypeError\", __msg=\"tuple for startswith must only contain str\"}}) end; if __cand == \"\" then if __start_raw <= __n and __start <= __end then {out} = true; break end elseif string.sub(__slice, 1, #__cand) == __cand then {out} = true; break end end end"
                        ));
                    } else if prefix_is_tuple {
                        self.emit_line(&format!(
                            "local {out} = false; for __i = 1, #{prefix} do local __cand = {prefix}[__i]; if type(__cand) ~= \"string\" then error({{__type=\"TypeError\", __msg=\"tuple for startswith must only contain str\"}}) end; if __cand == \"\" or string.sub({s}, 1, #__cand) == __cand then {out} = true; break end end"
                        ));
                    } else if args.len() >= 4 {
                        let start = sanitize_ident(&args[2]);
                        let end = sanitize_ident(&args[3]);
                        self.emit_line(&format!(
                            "local {out}; do local __n = #{s}; local __start_raw = if {start} < 0 then __n + {start} else {start}; local __start = __start_raw; if __start < 0 then __start = 0 end; if __start > __n then __start = __n end; local __end = if {end} < 0 then __n + {end} else {end}; if __end < __start then __end = __start end; if __end > __n then __end = __n end; local __slice = string.sub({s}, __start + 1, __end); {out} = if {prefix} == \"\" then (__start_raw <= __n and __start <= __end) else (string.sub(__slice, 1, #{prefix}) == {prefix}) end"
                        ));
                    } else {
                        self.emit_line(&format!(
                            "local {out} = ({prefix} == \"\" or string.sub({s}, 1, #{prefix}) == {prefix})"
                        ));
                    }
                }
            }
            "string_endswith" | "string_endswith_slice" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let s = sanitize_ident(&args[0]);
                    let suffix = sanitize_ident(&args[1]);
                    let suffix_is_tuple = self.tuple_vars.contains(&args[1]);
                    if suffix_is_tuple && args.len() >= 4 {
                        let start = sanitize_ident(&args[2]);
                        let end = sanitize_ident(&args[3]);
                        self.emit_line(&format!(
                            "local {out}; do local __n = #{s}; local __start_raw = if {start} < 0 then __n + {start} else {start}; local __start = __start_raw; if __start < 0 then __start = 0 end; if __start > __n then __start = __n end; local __end = if {end} < 0 then __n + {end} else {end}; if __end < __start then __end = __start end; if __end > __n then __end = __n end; local __slice = string.sub({s}, __start + 1, __end); {out} = false; for __i = 1, #{suffix} do local __cand = {suffix}[__i]; if type(__cand) ~= \"string\" then error({{__type=\"TypeError\", __msg=\"tuple for endswith must only contain str\"}}) end; if __cand == \"\" then if __start_raw <= __n and __start <= __end then {out} = true; break end elseif string.sub(__slice, -#__cand) == __cand then {out} = true; break end end end"
                        ));
                    } else if suffix_is_tuple {
                        self.emit_line(&format!(
                            "local {out} = false; for __i = 1, #{suffix} do local __cand = {suffix}[__i]; if type(__cand) ~= \"string\" then error({{__type=\"TypeError\", __msg=\"tuple for endswith must only contain str\"}}) end; if __cand == \"\" or string.sub({s}, -#__cand) == __cand then {out} = true; break end end"
                        ));
                    } else if args.len() >= 4 {
                        let start = sanitize_ident(&args[2]);
                        let end = sanitize_ident(&args[3]);
                        self.emit_line(&format!(
                            "local {out}; do local __n = #{s}; local __start_raw = if {start} < 0 then __n + {start} else {start}; local __start = __start_raw; if __start < 0 then __start = 0 end; if __start > __n then __start = __n end; local __end = if {end} < 0 then __n + {end} else {end}; if __end < __start then __end = __start end; if __end > __n then __end = __n end; local __slice = string.sub({s}, __start + 1, __end); {out} = if {suffix} == \"\" then (__start_raw <= __n and __start <= __end) else (string.sub(__slice, -#{suffix}) == {suffix}) end"
                        ));
                    } else {
                        self.emit_line(&format!(
                            "local {out} = ({suffix} == \"\" or string.sub({s}, -#{suffix}) == {suffix})"
                        ));
                    }
                }
            }
            "string_replace" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let s = sanitize_ident(&args[0]);
                    let old = sanitize_ident(&args[1]);
                    let new_val = sanitize_ident(&args[2]);
                    // Escape Lua pattern magic characters in search string so gsub
                    // does literal matching. Also escape % in replacement string
                    // since gsub interprets %0, %1, etc. as capture references.
                    if args.len() >= 4 {
                        let count = sanitize_ident(&args[3]);
                        self.emit_line(&format!(
                            "local {out}; do local __pattern = {old}:gsub(\"[%(%)%.%%%+%-%*%?%[%]%^%$]\", \"%%%0\"); local __replacement = ({new_val}):gsub(\"%%\", \"%%%%\"); if {count} >= 0 then {out} = (string.gsub({s}, __pattern, __replacement, {count})) else {out} = (string.gsub({s}, __pattern, __replacement)) end end"
                        ));
                    } else {
                        self.emit_line(&format!(
                            "local {out} = (string.gsub({s}, \
                             {old}:gsub(\"[%(%)%.%%%+%-%*%?%[%]%^%$]\", \"%%%0\"), \
                             ({new_val}):gsub(\"%%\", \"%%%%\")))"
                        ));
                    }
                }
            }
            "string_find" | "string_find_slice" | "string_index" | "string_index_slice" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let s = sanitize_ident(&args[0]);
                    let sub = sanitize_ident(&args[1]);
                    let needs_error = op.kind.contains("index");
                    let error_guard = if needs_error {
                        format!(
                            "; if {out} == -1 then error({{__type=\"ValueError\", __msg=\"substring not found\"}}) end"
                        )
                    } else {
                        String::new()
                    };
                    if args.len() >= 4 {
                        let start = sanitize_ident(&args[2]);
                        let end = sanitize_ident(&args[3]);
                        self.emit_line(&format!(
                            "local {out}; do local __n = #{s}; local __start_raw = if {start} < 0 then __n + {start} else {start}; local __start = __start_raw; if __start < 0 then __start = 0 end; if __start > __n then __start = __n end; local __end = if {end} < 0 then __n + {end} else {end}; if __end < __start then __end = __start end; if __end > __n then __end = __n end; if {sub} == \"\" then {out} = if __start_raw <= __n and __start <= __end then __start else -1 else local __found = string.find({s}, {sub}, __start + 1, true); if __found and __found <= __end then {out} = __found - 1 else {out} = -1 end end{error_guard} end"
                        ));
                    } else {
                        self.emit_line(&format!(
                            "local {out} = (string.find({s}, {sub}, 1, true) or 0) - 1{error_guard}"
                        ));
                    }
                }
            }
            "string_rfind" | "string_rfind_slice" | "string_rindex" | "string_rindex_slice" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let s = sanitize_ident(&args[0]);
                    let sub = sanitize_ident(&args[1]);
                    let needs_error = op.kind.contains("rindex");
                    let error_guard = if needs_error {
                        format!(
                            "; if {out} == -1 then error({{__type=\"ValueError\", __msg=\"substring not found\"}}) end"
                        )
                    } else {
                        String::new()
                    };
                    let bounds = if args.len() >= 4 {
                        let start = sanitize_ident(&args[2]);
                        let end = sanitize_ident(&args[3]);
                        format!(
                            "local __n = #{s}; local __start_raw = if {start} < 0 then __n + {start} else {start}; local __start = __start_raw; if __start < 0 then __start = 0 end; if __start > __n then __start = __n end; local __end = if {end} < 0 then __n + {end} else {end}; if __end < __start then __end = __start end; if __end > __n then __end = __n end;"
                        )
                    } else {
                        format!(
                            "local __n = #{s}; local __start_raw = 0; local __start = 0; local __end = __n;"
                        )
                    };
                    self.emit_line(&format!(
                        "local {out}; do {bounds} if {sub} == \"\" then {out} = if __start_raw <= __n and __start <= __end then __end else -1 else local __last = -1; local __pos = __start + 1; while true do local __found = string.find({s}, {sub}, __pos, true); if not __found or __found > __end then break end; __last = __found - 1; __pos = __found + 1 end; {out} = __last end{error_guard} end"
                    ));
                }
            }
            "string_count" | "string_count_slice" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let s = sanitize_ident(&args[0]);
                    let sub = sanitize_ident(&args[1]);
                    let source_expr = if args.len() >= 4 {
                        let start = sanitize_ident(&args[2]);
                        let end = sanitize_ident(&args[3]);
                        format!(
                            "do local __n = #{s}; local __start = if {start} < 0 then __n + {start} else {start}; if __start < 0 then __start = 0 end; if __start > __n then __start = __n end; local __end = if {end} < 0 then __n + {end} else {end}; if __end < __start then __end = __start end; if __end > __n then __end = __n end; local __src = string.sub({s}, __start + 1, __end);"
                        )
                    } else {
                        format!("do local __src = {s};")
                    };
                    self.emit_line(&format!(
                        "local {out}; {source_expr} local __sub = {sub}; if __sub == \"\" then {out} = #__src + 1 else local __count = 0; local __pos = 1; while true do local __i, __j = string.find(__src, __sub, __pos, true); if not __i then break end; __count += 1; __pos = __j + 1 end; {out} = __count end end"
                    ));
                }
            }
            "string_partition" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let s = sanitize_ident(&args[0]);
                    let sep = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "local {out}; do if {sep} == \"\" then error({{__type=\"ValueError\", __msg=\"empty separator\"}}) end; local __i, __j = string.find({s}, {sep}, 1, true); if __i then {out} = {{string.sub({s}, 1, __i - 1), {sep}, string.sub({s}, __j + 1)}} else {out} = {{{s}, \"\", \"\"}} end end"
                    ));
                    if let Some(ref out_name) = op.out {
                        self.tuple_vars.insert(out_name.clone());
                    }
                }
            }
            "string_rpartition" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let s = sanitize_ident(&args[0]);
                    let sep = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "local {out}; do if {sep} == \"\" then error({{__type=\"ValueError\", __msg=\"empty separator\"}}) end; local __last_i, __last_j = nil, nil; local __pos = 1; while true do local __i, __j = string.find({s}, {sep}, __pos, true); if not __i then break end; __last_i, __last_j = __i, __j; __pos = __i + 1 end; if __last_i then {out} = {{string.sub({s}, 1, __last_i - 1), {sep}, string.sub({s}, __last_j + 1)}} else {out} = {{\"\", \"\", {s}}} end end"
                    ));
                    if let Some(ref out_name) = op.out {
                        self.tuple_vars.insert(out_name.clone());
                    }
                }
            }
            "string_splitlines" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(s) = args.first() {
                    let s = sanitize_ident(s);
                    let keep = args
                        .get(1)
                        .map(|arg| sanitize_ident(arg))
                        .unwrap_or_else(|| "false".to_string());
                    self.emit_line(&format!(
                        "local {out}; do local __keep = {keep}; local __lines = {{}}; local __n = 0; local __line_start = 1; local __i = 1; while __i <= #{s} do local __c = string.sub({s}, __i, __i); if __c == \"\\n\" or __c == \"\\r\" then local __line_end = __i - 1; local __next = __i + 1; if __c == \"\\r\" and __next <= #{s} and string.sub({s}, __next, __next) == \"\\n\" then __next += 1 end; __n += 1; if __keep then __lines[__n] = string.sub({s}, __line_start, __next - 1) else __lines[__n] = string.sub({s}, __line_start, __line_end) end; __line_start = __next; __i = __next else __i += 1 end end; if __line_start <= #{s} then __n += 1; __lines[__n] = string.sub({s}, __line_start) end; {out} = __lines end"
                    ));
                }
            }
            "string_split" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(s) = args.first() {
                    let s = sanitize_ident(s);
                    if args.len() >= 2 {
                        let sep = sanitize_ident(&args[1]);
                        self.emit_line(&format!("local {out} = molt_string.split({s}, {sep})"));
                    } else {
                        // Python str.split() with no args splits on any
                        // whitespace and strips leading/trailing.  The
                        // molt_string.split helper handles sep==nil correctly
                        // using %s+ pattern matching.
                        self.emit_line(&format!("local {out} = molt_string.split({s})"));
                    }
                }
            }
            "string_split_validate" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let s = sanitize_ident(&args[0]);
                    let sep = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "local {out} = molt_string.split_validate({s}, {sep})"
                    ));
                }
            }
            "string_split_field" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let s = sanitize_ident(&args[0]);
                    let sep = sanitize_ident(&args[1]);
                    let idx = sanitize_ident(&args[2]);
                    self.emit_line(&format!(
                        "local {out} = molt_string.split_field({s}, {sep}, {idx})"
                    ));
                }
            }
            "string_split_field_len" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let s = sanitize_ident(&args[0]);
                    let sep = sanitize_ident(&args[1]);
                    let idx = sanitize_ident(&args[2]);
                    self.emit_line(&format!(
                        "local {out} = molt_string.split_field_len({s}, {sep}, {idx})"
                    ));
                }
            }
            "string_split_field_eq" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 4 {
                    let s = sanitize_ident(&args[0]);
                    let sep = sanitize_ident(&args[1]);
                    let idx = sanitize_ident(&args[2]);
                    let expected = sanitize_ident(&args[3]);
                    self.emit_line(&format!(
                        "local {out} = molt_string.split_field_eq({s}, {sep}, {idx}, {expected})"
                    ));
                }
            }
            "string_concat" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let a = sanitize_ident(&args[0]);
                    let b = sanitize_ident(&args[1]);
                    self.emit_line(&format!("local {out} = {a} .. {b}"));
                }
            }
            "string_repeat" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let s = sanitize_ident(&args[0]);
                    let n = sanitize_ident(&args[1]);
                    self.emit_line(&format!("local {out} = string.rep({s}, {n})"));
                }
            }
            "string_split_ws_dict_inc" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let line = sanitize_ident(&args[0]);
                    let dict = sanitize_ident(&args[1]);
                    let delta = sanitize_ident(&args[2]);
                    self.emit_line(&format!(
                        "local {out} = molt_string_split_ws_dict_inc({line}, {dict}, {delta})"
                    ));
                    if let Some(ref out_name) = op.out {
                        self.tuple_vars.insert(out_name.clone());
                    }
                }
            }
            "string_split_sep_dict_inc" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 4 {
                    let line = sanitize_ident(&args[0]);
                    let sep = sanitize_ident(&args[1]);
                    let dict = sanitize_ident(&args[2]);
                    let delta = sanitize_ident(&args[3]);
                    self.emit_line(&format!(
                        "local {out} = molt_string_split_sep_dict_inc({line}, {sep}, {dict}, {delta})"
                    ));
                    if let Some(ref out_name) = op.out {
                        self.tuple_vars.insert(out_name.clone());
                    }
                }
            }
            "taq_ingest_line" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let dict = sanitize_ident(&args[0]);
                    let line = sanitize_ident(&args[1]);
                    let bucket_size = sanitize_ident(&args[2]);
                    self.emit_line(&format!(
                        "local {out} = molt_taq_ingest_line({dict}, {line}, {bucket_size})"
                    ));
                }
            }

            // ================================================================
            // Iterator ops
            // ================================================================
            "iter" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(iterable) = args.first() {
                    let it = sanitize_ident(iterable);
                    // Create a stateful iterator closure over the table.
                    // Each call returns {value, nil} or {nil, true} when exhausted.
                    self.emit_line(&format!(
                        "local {out}; do local _t = {it}; local _i = 0; \
                         {out} = function() _i = _i + 1; \
                         if _i <= #_t then return {{_t[_i], nil}} \
                         else return {{nil, true}} end; end; end"
                    ));
                }
            }
            "iter_next" => {
                // Call the stateful iterator closure created by the `iter` op.
                // Returns {value, nil} or {nil, true} when exhausted.
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(iter_var) = args.first() {
                    let iter_var = sanitize_ident(iter_var);
                    self.emit_line(&format!("local {out} = {iter_var}()"));
                }
            }
            "iter_next_unboxed" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(iter_var) = args.first() {
                    let iter_var = sanitize_ident(iter_var);
                    let done_out = op.out.as_deref().map(sanitize_ident);
                    let value_out = op.var.as_deref().map(sanitize_ident);
                    let tmp_seed = done_out
                        .as_deref()
                        .or(value_out.as_deref())
                        .unwrap_or("iter");
                    let tmp = format!("__next_{tmp_seed}");
                    self.emit_line(&format!("local {tmp} = {iter_var}()"));
                    if let Some(done) = done_out {
                        self.emit_line(&format!("local {done} = {tmp}[2]"));
                    }
                    if let Some(value) = value_out {
                        self.emit_line(&format!("local {value} = {tmp}[1]"));
                    }
                }
            }

            "checked_add" => {
                // 2-result op: op.var = wrapping sum (results[0]), op.out =
                // overflow flag (results[1]) — the IterNextUnboxed transport
                // convention. Luau supports multi-return destructuring, so no
                // tmp table is needed (unlike iter_next_unboxed's table
                // unpack). The helper returns (a + b, false): f64 addition
                // never overflows i64, so the flag is constant-false and the
                // peel's slow loop is dead on this target by design.
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = sanitize_ident(&args[0]);
                    let rhs = sanitize_ident(&args[1]);
                    let flag_out = op.out.as_deref().map(sanitize_ident);
                    let sum_out = op.var.as_deref().map(sanitize_ident);
                    match (sum_out, flag_out) {
                        (Some(sum), Some(flag)) => {
                            self.emit_line(&format!(
                                "local {sum}: number, {flag}: boolean = molt_checked_i64_add({lhs}, {rhs})"
                            ));
                        }
                        (Some(sum), None) => {
                            self.emit_line(&format!(
                                "local {sum}: number = molt_checked_i64_add({lhs}, {rhs})"
                            ));
                        }
                        (None, Some(flag)) => {
                            self.emit_line(&format!(
                                "local _, {flag}: boolean = molt_checked_i64_add({lhs}, {rhs})"
                            ));
                        }
                        (None, None) => {}
                    }
                }
            }

            "checked_mul" => {
                // 2-result op: op.var = wrapping product (results[0]), op.out =
                // overflow/inexactness flag (results[1]) — the IterNextUnboxed
                // transport convention. The helper returns (a * b, flag) where
                // flag is CONSERVATIVE: true whenever the f64 product may have
                // lost precision (|product| >= 2^53), forcing the boxed BigInt
                // slow loop. A structural (a * b, false) here would be a SILENT
                // WRONG ANSWER above 2^53 — see molt_checked_i64_mul.
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = sanitize_ident(&args[0]);
                    let rhs = sanitize_ident(&args[1]);
                    let flag_out = op.out.as_deref().map(sanitize_ident);
                    let product_out = op.var.as_deref().map(sanitize_ident);
                    match (product_out, flag_out) {
                        (Some(product), Some(flag)) => {
                            self.emit_line(&format!(
                                "local {product}: number, {flag}: boolean = molt_checked_i64_mul({lhs}, {rhs})"
                            ));
                        }
                        (Some(product), None) => {
                            self.emit_line(&format!(
                                "local {product}: number = molt_checked_i64_mul({lhs}, {rhs})"
                            ));
                        }
                        (None, Some(flag)) => {
                            self.emit_line(&format!(
                                "local _, {flag}: boolean = molt_checked_i64_mul({lhs}, {rhs})"
                            ));
                        }
                        (None, None) => {}
                    }
                }
            }

            // ================================================================
            // Indirect / bound calls
            // ================================================================
            "call_indirect" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if !args.is_empty() {
                    let func_ref = sanitize_ident(&args[0]);
                    let call_args = args[1..]
                        .iter()
                        .map(|a| sanitize_ident(a))
                        .collect::<Vec<_>>()
                        .join(", ");
                    if let Some(ref out_name) = op.out {
                        let out = sanitize_ident(out_name);
                        self.emit_line(&format!("local {out} = {func_ref}({call_args})"));
                    } else {
                        self.emit_line(&format!("{func_ref}({call_args})"));
                    }
                }
            }
            "call_bind" => {
                // In Molt IR, call_bind is a function CALL whose second arg is
                // always a callargs tuple (built via callargs_new + callargs_push_pos).
                // We must unpack the tuple so individual args are spread:
                //   func(table.unpack(args_tuple))  instead of  func(args_tuple)
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let func = sanitize_ident(&args[0]);
                    let args_tuple = sanitize_ident(&args[1]);
                    if let Some(ref out_name) = op.out {
                        let out = sanitize_ident(out_name);
                        self.emit_line(&format!(
                            "local {out} = if {func} then {func}(table.unpack({args_tuple})) else nil"
                        ));
                    } else {
                        self.emit_line(&format!(
                            "if {func} then {func}(table.unpack({args_tuple})) end"
                        ));
                    }
                } else if let Some(func) = args.first() {
                    self.emit_line(&format!("local {out} = {}", sanitize_ident(func)));
                }
            }
            "is_callable" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = (type({}) == \"function\")",
                        sanitize_ident(val)
                    ));
                }
            }

            // ================================================================
            // Try/except blocks
            // ================================================================
            "try_start" => {
                // Handled by lower_try_to_pcall — should not reach here.
                self.emit_line("-- [try_start]");
            }
            "try_end" => {
                // Handled by lower_try_to_pcall — should not reach here.
                self.emit_line("-- [try_end]");
            }
            "pcall_wrap_begin" => {
                let n = op.value.unwrap_or(0) as u32;
                self.emit_line(&format!("local __ok_{n}, __err_{n}"));
                self.emit_line(&format!("__ok_{n}, __err_{n} = pcall(function()"));
                self.push_indent();
                self.try_depth_counter.push(n);
                self.inside_pcall_body = true;
            }
            "pcall_wrap_end" => {
                self.pop_indent();
                self.emit_line("end)");
                self.inside_pcall_body = false;
                // Do NOT pop try_depth_counter here — the handler code
                // after pcall_wrap_end needs to reference __err_N via
                // exception_last.
            }
            // ================================================================
            // Slice
            // ================================================================
            "slice" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let obj = sanitize_ident(&args[0]);
                    let start = sanitize_ident(&args[1]);
                    let stop = sanitize_ident(&args[2]);
                    self.emit_line(&format!(
                        "local {out} = {{table.unpack({obj}, {start} + 1, {stop})}}"
                    ));
                } else if args.len() >= 2 {
                    let obj = sanitize_ident(&args[0]);
                    let start = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "local {out} = {{table.unpack({obj}, {start} + 1)}}"
                    ));
                }
            }

            // ================================================================
            // Enumerate op (distinct from call to enumerate builtin)
            // ================================================================
            "enumerate" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(iterable) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = molt_enumerate({})",
                        sanitize_ident(iterable)
                    ));
                }
            }

            // ================================================================
            // Dict key/value/items ops
            // ================================================================
            "dict_keys" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(d) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = molt_dict_keys({})",
                        sanitize_ident(d)
                    ));
                }
            }
            "dict_values" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(d) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = molt_dict_values({})",
                        sanitize_ident(d)
                    ));
                }
            }
            "dict_items" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(d) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = molt_dict_items({})",
                        sanitize_ident(d)
                    ));
                }
            }

            // ================================================================
            // Phi nodes (SSA merge — no-op in sequential Luau) / nop
            // ================================================================
            "phi" => {}
            "nop" => {}
            "pcall_handler_end" => {
                // Pop pcall counter at the end of the handler dispatch zone.
                if !self.try_depth_counter.is_empty() {
                    self.try_depth_counter.pop();
                }
            }

            // ================================================================
            // Default: unsupported op
            // ================================================================
            _ => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!(
                        "local {out} = nil -- [unsupported op: {}]",
                        op.kind
                    ));
                } else {
                    self.emit_line(&format!("-- [unsupported op: {}]", op.kind));
                }
            }
        }
    }

    // --- helper: binary op ---
    fn emit_binary_op(&mut self, op: &OpIR, operator: &str) {
        let out = self.out_var(op);
        let args = op.args.as_deref().unwrap_or(&[]);
        if args.len() >= 2 {
            let arithmetic = matches!(operator, "+" | "-" | "*" | "/" | "%" | "//" | "^");
            let lhs = if arithmetic {
                self.numeric_operand_expr(&args[0])
            } else {
                sanitize_ident(&args[0])
            };
            let rhs = if arithmetic {
                self.numeric_operand_expr(&args[1])
            } else {
                sanitize_ident(&args[1])
            };
            // Parenthesize comparison/boolean results to prevent precedence
            // issues when the sink pass inlines into `not` expressions.
            // Without parens: `not a == b` → `(not a) == b` (wrong).
            // With parens: `not (a == b)` (correct).
            let is_cmp = matches!(operator, "==" | "~=" | "<" | "<=" | ">" | ">=");
            let is_logical = matches!(operator, "and" | "or");
            // Type annotation: arithmetic → number, comparisons → boolean.
            let ty_ann = if arithmetic {
                ": number"
            } else if is_cmp {
                ": boolean"
            } else {
                ""
            };
            if is_cmp || is_logical {
                self.emit_line(&format!("local {out}{ty_ann} = ({lhs} {operator} {rhs})"));
            } else {
                self.emit_line(&format!("local {out}{ty_ann} = {lhs} {operator} {rhs}"));
            }
        }
    }

    // --- helper: bit32 op ---
    fn emit_bit_op(&mut self, op: &OpIR, func: &str) {
        let out = self.out_var(op);
        let args = op.args.as_deref().unwrap_or(&[]);
        if args.len() >= 2 {
            let lhs = sanitize_ident(&args[0]);
            let rhs = sanitize_ident(&args[1]);
            self.emit_line(&format!("local {out}: number = bit32.{func}({lhs}, {rhs})"));
        }
    }

    // --- helpers ---

    fn out_var(&self, op: &OpIR) -> String {
        op.out
            .as_deref()
            .map(sanitize_ident)
            .unwrap_or_else(|| "_".to_string())
    }

    fn var_ref(&self, op: &OpIR) -> String {
        op.var
            .as_deref()
            .map(sanitize_ident)
            .unwrap_or_else(|| "_".to_string())
    }

    fn numeric_operand_expr(&self, raw_name: &str) -> String {
        let ident = sanitize_ident(raw_name);
        if self.scalar_plan.name_scalar_kind(raw_name) == Some(ScalarKind::Bool) {
            format!("(if {ident} then 1 else 0)")
        } else {
            ident
        }
    }

    fn plan_knows_string(&self, raw_name: &str) -> bool {
        self.scalar_plan.name_scalar_kind(raw_name) == Some(ScalarKind::Str)
            || self.scalar_plan.name_container_kind(raw_name) == Some(ContainerKind::Str)
    }

    fn plan_knows_list(&self, raw_name: &str) -> bool {
        self.scalar_plan.name_container_kind(raw_name) == Some(ContainerKind::List)
    }

    fn emit_index_bounds_guard(&mut self, idx: &str, container: &str, message: &str) {
        self.emit_line(&format!(
            "if {idx} < 1 or {idx} > #{container} then error({{__type=\"IndexError\", __msg=\"{message}\"}}) end"
        ));
    }

    fn emit_list_insert(&mut self, list: &str, idx: &str, val: &str) {
        self.emit_line(&format!(
            "do local __idx = if {idx} >= 0 then {idx} + 1 else #{list} + {idx} + 1; if __idx < 1 then __idx = 1 end; if __idx > #{list} + 1 then __idx = #{list} + 1 end; if __idx == #{list} + 1 then {list}[#{list} + 1] = {val} else table.insert({list}, __idx, {val}) end end"
        ));
    }

    fn emit_list_pop(&mut self, list: &str, idx: Option<&str>, out: Option<&str>) {
        match (idx, out) {
            (Some(idx), Some(out)) => self.emit_line(&format!(
                "local {out}; do local __idx = if {idx} >= 0 then {idx} + 1 else #{list} + {idx} + 1; if __idx < 1 or __idx > #{list} then error({{__type=\"IndexError\", __msg=\"pop index out of range\"}}) end; {out} = table.remove({list}, __idx) end"
            )),
            (Some(idx), None) => self.emit_line(&format!(
                "do local __idx = if {idx} >= 0 then {idx} + 1 else #{list} + {idx} + 1; if __idx < 1 or __idx > #{list} then error({{__type=\"IndexError\", __msg=\"pop index out of range\"}}) end; table.remove({list}, __idx) end"
            )),
            (None, Some(out)) => self.emit_line(&format!(
                "local {out}; if #{list} == 0 then error({{__type=\"IndexError\", __msg=\"pop from empty list\"}}) end; {out} = table.remove({list})"
            )),
            (None, None) => self.emit_line(&format!(
                "if #{list} == 0 then error({{__type=\"IndexError\", __msg=\"pop from empty list\"}}) end; table.remove({list})"
            )),
        }
    }

    fn emit_string_predicate_attr(&mut self, out: &str, obj: &str, method: &str) {
        let predicate = match method {
            "isalpha" => "__is_alpha and not __is_digit",
            "isdigit" => "__is_digit",
            "isalnum" => "__is_alpha or __is_digit",
            "isspace" => "__is_space",
            "isupper" => "not __is_lower",
            "islower" => "not __is_upper",
            "isidentifier" => "(__is_alpha or __is_digit or __b == 95)",
            "isprintable" => "(__b >= 32 and __b <= 126)",
            "isdecimal" | "isnumeric" => "__is_digit",
            "istitle" => "true",
            _ => "false",
        };
        let prefix = match method {
            "isidentifier" => {
                "local __first = string.byte(__s, 1); local __first_ok = ((__first >= 65 and __first <= 90) or (__first >= 97 and __first <= 122) or __first == 95);"
            }
            "istitle" => "local __prev_uncased = true;",
            _ => "",
        };
        let suffix = match method {
            "isupper" | "islower" => " and __has_cased",
            "isidentifier" => " and __first_ok",
            "istitle" => " and __has_cased",
            _ => "",
        };
        let title_update = if method == "istitle" {
            " if __is_alpha then if __prev_uncased then if not __is_upper then __ok = false; break end else if not __is_lower then __ok = false; break end end; __prev_uncased = false else __prev_uncased = true end"
        } else {
            ""
        };
        self.emit_line(&format!(
            "local {out} = function(__args) local __s = {obj}; local __ok = (#__s > 0); local __has_cased = false; {prefix} for __i = 1, #__s do local __b = string.byte(__s, __i); local __is_upper = (__b >= 65 and __b <= 90); local __is_lower = (__b >= 97 and __b <= 122); local __is_alpha = (__is_upper or __is_lower); local __is_digit = (__b >= 48 and __b <= 57); local __is_space = (__b == 32 or __b == 9 or __b == 10 or __b == 11 or __b == 12 or __b == 13); if __is_alpha then __has_cased = true end; if not ({predicate}) then __ok = false; break end{title_update} end; return __ok{suffix} end"
        ));
    }

    /// Wrap a condition identifier in `molt_bool()` if it's not a known boolean.
    /// Returns the identifier as-is for booleans, or `molt_bool(ident)` otherwise.
    fn guard_truthiness(&self, raw_name: &str) -> String {
        let ident = sanitize_ident(raw_name);
        match self.scalar_plan.name_scalar_kind(raw_name) {
            Some(ScalarKind::Bool) => ident,
            // Strength-reduce: type-specific truthiness checks avoid
            // the multi-branch molt_bool() function call overhead.
            Some(ScalarKind::Int | ScalarKind::Float) => format!("({ident} ~= 0)"),
            Some(ScalarKind::Str) => format!("({ident} ~= \"\")"),
            Some(ScalarKind::NoneValue) => "false".to_string(),
            None => self
                .container_truthiness(raw_name, &ident)
                .unwrap_or_else(|| match ident.as_str() {
                    "true" | "false" => ident,
                    _ => format!("molt_bool({ident})"),
                }),
        }
    }

    fn container_truthiness(&self, raw_name: &str, ident: &str) -> Option<String> {
        match self.scalar_plan.name_container_kind(raw_name) {
            Some(ContainerKind::List | ContainerKind::Tuple | ContainerKind::Str) => {
                Some(format!("(#{ident} > 0)"))
            }
            Some(ContainerKind::Dict | ContainerKind::Set) => {
                Some(format!("(next({ident}) ~= nil)"))
            }
            None => None,
        }
    }

    fn is_known_bool_value(&self, raw_name: &str) -> bool {
        matches!(raw_name, "true" | "false")
            || self.scalar_plan.name_scalar_kind(raw_name) == Some(ScalarKind::Bool)
    }

    fn emit_line(&mut self, line: &str) {
        for _ in 0..self.indent {
            self.output.push('\t');
        }
        self.output.push_str(line);
        self.output.push('\n');
    }

    fn push_indent(&mut self) {
        self.indent += 1;
    }

    fn pop_indent(&mut self) {
        if self.indent > 0 {
            self.indent -= 1;
        }
    }
}

/// Map a Python/Molt type hint string to a Luau type annotation.
///
/// Returns a `&'static str` for the common primitive cases and falls back
/// to `"any"` for anything the Luau type system cannot express directly.
fn python_type_to_luau(hint: &str) -> &'static str {
    match hint {
        "int" | "Int" => "number",
        "float" | "Float" => "number",
        "str" | "Str" | "string" => "string",
        "bool" | "Bool" | "boolean" => "boolean",
        "None" | "NoneType" => "nil",
        "list" | "List" => "{any}",
        "dict" | "Dict" => "{[any]: any}",
        s if s.starts_with("list[") || s.starts_with("List[") => "{any}",
        s if s.starts_with("dict[") || s.starts_with("Dict[") => "{[any]: any}",
        _ => "any",
    }
}

/// Sanitize a Molt IR identifier for Luau.
/// Replaces `.` and `-` with `_`, and prefixes Luau keywords with `_m_`.
fn sanitize_ident(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();

    if cleaned.is_empty() {
        return "_empty".to_string();
    }
    if is_luau_keyword(&cleaned) {
        format!("_m_{cleaned}")
    } else if cleaned.starts_with(|c: char| c.is_ascii_digit()) {
        format!("_{cleaned}")
    } else {
        cleaned
    }
}

fn sanitize_label(label: &str) -> String {
    label
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn is_luau_keyword(word: &str) -> bool {
    matches!(
        word,
        "and"
            | "break"
            | "do"
            | "else"
            | "elseif"
            | "end"
            | "false"
            | "for"
            | "function"
            | "if"
            | "in"
            | "local"
            | "nil"
            | "not"
            | "or"
            | "repeat"
            | "return"
            | "then"
            | "true"
            | "until"
            | "while"
            | "continue"
            | "type"
            | "export"
    )
}

fn escape_luau_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_ident() {
        assert_eq!(sanitize_ident("foo"), "foo");
        assert_eq!(sanitize_ident("my.attr"), "my_attr");
        assert_eq!(sanitize_ident("and"), "_m_and");
        assert_eq!(sanitize_ident("v0"), "v0");
    }

    #[test]
    fn test_escape_luau_string() {
        assert_eq!(escape_luau_string("hello"), "hello");
        assert_eq!(escape_luau_string("say \"hi\""), "say \\\"hi\\\"");
        assert_eq!(escape_luau_string("a\nb"), "a\\nb");
    }

    #[test]
    fn test_empty_ir() {
        let ir = SimpleIR {
            functions: vec![],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(output.contains("--!strict"));
        assert!(output.contains("molt_main"));
    }

    #[test]
    fn test_simple_function() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "const".to_string(),
                        value: Some(42),
                        out: Some("v0".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "print".to_string(),
                        args: Some(vec!["v0".to_string()]),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(output.contains("function molt_main()"));
        // v0 is a single-use constant inlined into the print call.
        assert!(output.contains("print(42)"));
    }

    #[test]
    fn test_int_from_str_of_obj_preserves_base_operand() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec![
                    "value".to_string(),
                    "base".to_string(),
                    "has_base".to_string(),
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "int_from_str_of_obj".to_string(),
                        args: Some(vec![
                            "value".to_string(),
                            "base".to_string(),
                            "has_base".to_string(),
                        ]),
                        out: Some("out".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        var: Some("out".to_string()),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(output.contains("molt_bool(has_base)"));
        assert!(output.contains("tonumber(molt_str(value), molt_int(base))"));
    }

    #[test]
    fn test_real_ir_ops() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "test_func".to_string(),
                params: vec!["p0".to_string()],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "const_float".to_string(),
                        f_value: Some(std::f64::consts::PI),
                        out: Some("v0".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const_str".to_string(),
                        s_value: Some("hello".to_string()),
                        out: Some("v1".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "add".to_string(),
                        args: Some(vec!["p0".to_string(), "v0".to_string()]),
                        out: Some("v2".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "lt".to_string(),
                        args: Some(vec!["v2".to_string(), "p0".to_string()]),
                        out: Some("v3".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["v3".to_string()]),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(output.contains("local function test_func(p0: any)"));
        // v0 (3.14) is single-use, inlined into the add expression.
        // add emits a type-aware string/number ternary.
        assert!(
            output.contains("p0 + 3.14") || output.contains("3.14"),
            "Expected 3.14 inlined somewhere, got:\n{output}"
        );
        // After sink pass, v2 is inlined into the lt expression.
        assert!(
            output.contains("v2 < p0") || output.contains("< p0"),
            "Expected lt comparison with p0, got:\n{output}"
        );
        assert!(output.contains("return"));
    }

    #[test]
    fn test_control_flow() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "flow_test".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "label".to_string(),
                        value: Some(0),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "jump".to_string(),
                        value: Some(1),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "label".to_string(),
                        value: Some(1),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        // The dead goto/label stripping pass removes:
        //   - label_0 (orphaned: no goto targets it)
        //   - goto label_1 + label_1 (dead: goto jumps to immediately next label)
        // This is correct — the optimiser eliminates redundant control flow.
        // Verify they are NOT emitted as comments (the old Bug 4 regression).
        assert!(
            !output.contains("-- ::label_0::"),
            "labels must not be comments"
        );
        assert!(!output.contains("-- goto"), "gotos must not be comments");
        // The function still compiles and returns.
        assert!(output.contains("return"));
    }

    #[test]
    fn test_lower_iter_to_for_requires_exhaustion_break_condition() {
        let ops = vec![
            OpIR {
                kind: "iter".to_string(),
                out: Some("v_it".to_string()),
                args: Some(vec!["v_src".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_start".to_string(),
                ..OpIR::default()
            },
            OpIR {
                kind: "iter_next".to_string(),
                out: Some("v_next".to_string()),
                args: Some(vec!["v_it".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "index".to_string(),
                out: Some("v_exhausted".to_string()),
                args: Some(vec!["v_next".to_string(), "v_idx1".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_break_if_true".to_string(),
                args: Some(vec!["v_other_cond".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "index".to_string(),
                out: Some("v_value".to_string()),
                args: Some(vec!["v_next".to_string(), "v_idx0".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_local".to_string(),
                args: Some(vec!["v_sink".to_string(), "v_value".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_end".to_string(),
                ..OpIR::default()
            },
        ];

        let lowered = lower_iter_to_for(&ops);
        assert!(
            lowered.iter().any(|op| op.kind == "iter"),
            "iter op should be preserved when break guard is unrelated"
        );
        assert!(
            !lowered.iter().any(|op| op.kind == "for_iter"),
            "unsafe iterator rewrite should not fire"
        );
    }

    #[test]
    fn test_compile_checked_materializes_sys_target_version_module() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "const".to_string(),
                        value: Some(3),
                        out: Some("major".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const".to_string(),
                        value: Some(14),
                        out: Some("minor".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const".to_string(),
                        value: Some(0),
                        out: Some("micro".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const_str".to_string(),
                        s_value: Some("final".to_string()),
                        out: Some("releaselevel".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const".to_string(),
                        value: Some(0),
                        out: Some("serial".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const_str".to_string(),
                        s_value: Some("3.14.0 (molt)".to_string()),
                        out: Some("version".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "call_internal".to_string(),
                        s_value: Some("molt_sys_set_version_info".to_string()),
                        args: Some(vec![
                            "major".to_string(),
                            "minor".to_string(),
                            "micro".to_string(),
                            "releaselevel".to_string(),
                            "serial".to_string(),
                            "version".to_string(),
                        ]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const_str".to_string(),
                        s_value: Some("sys".to_string()),
                        out: Some("sys_name".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "module_import".to_string(),
                        args: Some(vec!["sys_name".to_string()]),
                        out: Some("sys_module".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "module_get_attr".to_string(),
                        args: Some(vec!["sys_module".to_string()]),
                        s_value: Some("version_info".to_string()),
                        out: Some("version_info".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "module_get_attr".to_string(),
                        args: Some(vec!["sys_module".to_string()]),
                        s_value: Some("hexversion".to_string()),
                        out: Some("hexversion".to_string()),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };

        let source = LuauBackend::new()
            .compile_checked(&ir)
            .expect("sys target-version bootstrap must be supported");
        assert!(!source.contains("local function molt_sys_set_version_info(...) end"));
        assert!(source.contains("local function molt_sys_set_version_info("));
        assert!(source.contains("molt_module_cache[\"sys\"] ="));
        assert!(source.contains("version_info = molt_sys_version_info"));
        assert!(source.contains("version = molt_sys_version"));
        assert!(source.contains("hexversion = molt_sys_hexversion"));
        assert!(!source.contains("(molt_module_cache[sys_name] or {})"));
        assert!(source.contains("local sys_module = molt_luau_import_module(sys_name)"));
    }

    #[test]
    fn test_compile_checked_accepts_label_goto_comments() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "flow_test".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "label".to_string(),
                        value: Some(0),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "jump".to_string(),
                        value: Some(1),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        // Labels and gotos emit as real Luau control flow, then the dead
        // goto/label stripping pass removes unreachable ones.  The key
        // correctness property is that they are NOT emitted as comments.
        let source = backend
            .compile_checked(&ir)
            .expect("label/goto source should pass validation");
        assert!(
            !source.contains("-- ::label_0::"),
            "labels must not be comments"
        );
        assert!(!source.contains("-- goto"), "gotos must not be comments");
    }

    #[test]
    fn test_compile_checked_lowers_store_var_and_load_var() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "slot_test".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "const_int".to_string(),
                        out: Some("v0".to_string()),
                        value: Some(42),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "store_var".to_string(),
                        var: Some("slot".to_string()),
                        args: Some(vec!["v0".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "load_var".to_string(),
                        out: Some("v1".to_string()),
                        var: Some("slot".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["v1".to_string()]),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let source = backend
            .compile_checked(&ir)
            .expect("store_var/load_var should lower without stub markers");
        assert!(source.contains("\tlocal slot\n"));
        assert!(source.contains("\tslot = "));
        assert!(source.contains("return slot") || source.contains("local v1 = slot"));
        assert!(!source.contains("[unsupported op: store_var]"));
        assert!(!source.contains("[unsupported op: load_var]"));
    }

    #[test]
    fn test_compile_checked_lowers_missing_singleton() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "missing_singleton_test".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "missing".to_string(),
                        out: Some("first".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "missing".to_string(),
                        out: Some("second".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "is".to_string(),
                        args: Some(vec!["first".to_string(), "second".to_string()]),
                        out: Some("same".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["same".to_string()]),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let source = backend
            .compile_checked(&ir)
            .expect("missing sentinel should lower without stub markers");

        assert!(source.contains("local molt_missing_sentinel = {}"));
        assert!(source.contains("local first = molt_missing_sentinel"));
        assert!(source.contains("local second = molt_missing_sentinel"));
        assert!(!source.contains("-- [missing]"));
    }

    #[test]
    fn test_compile_checked_lowers_luau_process_target_facts() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "process_target_facts_test".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "getargv".to_string(),
                        out: Some("argv".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "sys_executable".to_string(),
                        out: Some("executable".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const".to_string(),
                        out: Some("depth".to_string()),
                        value: Some(0),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "getframe".to_string(),
                        out: Some("frame".to_string()),
                        args: Some(vec!["depth".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "tuple_new".to_string(),
                        args: Some(vec![
                            "argv".to_string(),
                            "executable".to_string(),
                            "frame".to_string(),
                        ]),
                        out: Some("facts".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["facts".to_string()]),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let source = backend
            .compile_checked(&ir)
            .expect("process target facts should lower without stub markers");

        assert!(source.contains("local argv = {}"));
        assert!(source.contains("local executable = \"\""));
        assert!(source.contains("local frame = nil"));
        assert!(!source.contains("-- [getargv]"));
        assert!(!source.contains("-- [sys_executable]"));
        assert!(!source.contains("-- [getframe]"));
    }

    #[test]
    fn test_compile_checked_lowers_trace_markers_as_luau_noops() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "trace_marker_test".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "trace_enter_slot".to_string(),
                        value: Some(7),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "trace_exit".to_string(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const".to_string(),
                        out: Some("ok".to_string()),
                        value: Some(1),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["ok".to_string()]),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let source = backend
            .compile_checked(&ir)
            .expect("trace markers should lower as Luau no-ops");

        assert!(
            source.contains("trace_marker_test"),
            "compiled trace marker function should be emitted, got:\n{source}"
        );
        assert!(
            !source.contains("[internal: trace_enter_slot]")
                && !source.contains("[internal: trace_exit]")
                && !source.contains("[unsupported op: trace_enter_slot]")
                && !source.contains("[unsupported op: trace_exit]"),
            "trace markers must not leave semantic stub markers, got:\n{source}"
        );
    }

    #[test]
    fn test_compile_checked_lowers_loop_exception_break_as_luau_noop() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "loop_exception_break_test".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "loop_start".to_string(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "loop_break_if_exception".to_string(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "loop_break".to_string(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "loop_end".to_string(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const".to_string(),
                        out: Some("ok".to_string()),
                        value: Some(1),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["ok".to_string()]),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let source = backend
            .compile_checked(&ir)
            .expect("exception-break markers should lower as Luau no-ops");

        assert!(
            source.contains("loop_exception_break_test"),
            "compiled loop exception-break function should be emitted, got:\n{source}"
        );
        assert!(
            !source.contains("[loop_break_if_exception]")
                && !source.contains("[unsupported op: loop_break_if_exception]"),
            "loop exception-break markers must not leave semantic stub markers, got:\n{source}"
        );
    }

    #[test]
    fn test_compile_checked_lowers_code_and_frame_metadata_as_luau_noops() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "code_frame_metadata_test".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "code_slots_init".to_string(),
                        value: Some(2),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const_none".to_string(),
                        out: Some("code".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "code_slot_set".to_string(),
                        value: Some(1),
                        args: Some(vec!["code".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const_none".to_string(),
                        out: Some("locals".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "frame_locals_set".to_string(),
                        args: Some(vec!["locals".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const".to_string(),
                        out: Some("ok".to_string()),
                        value: Some(1),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["ok".to_string()]),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let source = backend
            .compile_checked(&ir)
            .expect("code/frame metadata should lower as Luau no-ops");

        assert!(
            source.contains("code_frame_metadata_test"),
            "compiled code/frame metadata function should be emitted, got:\n{source}"
        );
        assert!(
            !source.contains("[internal: code_slots_init]")
                && !source.contains("[internal: code_slot_set]")
                && !source.contains("[internal: frame_locals_set]")
                && !source.contains("[unsupported op: code_slots_init]")
                && !source.contains("[unsupported op: code_slot_set]")
                && !source.contains("[unsupported op: frame_locals_set]"),
            "code/frame metadata must not leave semantic stub markers, got:\n{source}"
        );
    }

    #[test]
    fn test_compile_checked_accepts_shared_drop_artifacts_as_gc_noops() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "drop_artifact_test".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "drop_inserted".to_string(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "exception_region_drops_inserted".to_string(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const_str".to_string(),
                        out: Some("v0".to_string()),
                        s_value: Some("owned".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "inc_ref".to_string(),
                        args: Some(vec!["v0".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "dec_ref".to_string(),
                        args: Some(vec!["v0".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "release".to_string(),
                        args: Some(vec!["v0".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["v0".to_string()]),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let source = backend
            .compile_checked(&ir)
            .expect("shared drop artifacts should be explicit Luau GC no-ops");
        assert!(!source.contains("[unsupported op: drop_inserted]"));
        assert!(!source.contains("[unsupported op: exception_region_drops_inserted]"));
        assert!(!source.contains("[unsupported op: inc_ref]"));
        assert!(!source.contains("[unsupported op: dec_ref]"));
        assert!(!source.contains("[unsupported op: release]"));
    }

    #[test]
    fn test_compile_checked_lowers_shared_guard_tag_fact() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "guard_tag_test".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "const".to_string(),
                        value: Some(7),
                        out: Some("value".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const".to_string(),
                        value: Some(1),
                        out: Some("int_tag".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "guard_tag".to_string(),
                        args: Some(vec!["value".to_string(), "int_tag".to_string()]),
                        out: Some("none".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["value".to_string()]),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let source = backend
            .compile_checked(&ir)
            .expect("guard_tag should lower to the shared Luau guard helper");
        assert!(source.contains("local function molt_guard_type"));
        assert!(source.contains("molt_guard_type(value, int_tag)"));
        assert!(!source.contains("[unsupported op: guard_tag]"));
    }

    #[test]
    fn test_compile_checked_lowers_exception_stack_depth_to_value() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "exception_depth_test".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "exception_stack_depth".to_string(),
                        out: Some("v0".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "exception_stack_set_depth".to_string(),
                        args: Some(vec!["v0".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let source = backend
            .compile_checked(&ir)
            .expect("exception stack depth bookkeeping should lower");
        assert!(source.contains("\tlocal v0 = 0\n"));
        assert!(!source.contains("[exception_stack_depth]"));
    }

    #[test]
    fn test_compile_checked_lowers_iter_next_unboxed() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "iter_unboxed_test".to_string(),
                params: vec!["xs".to_string()],
                param_types: Some(vec!["list[int]".to_string()]),
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "iter".to_string(),
                        out: Some("it".to_string()),
                        args: Some(vec!["xs".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "iter_next_unboxed".to_string(),
                        args: Some(vec!["it".to_string()]),
                        var: Some("value".to_string()),
                        out: Some("done".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["value".to_string()]),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let source = backend
            .compile_checked(&ir)
            .expect("iter_next_unboxed should lower without stub markers");
        assert!(source.contains("local __next_done = it()"));
        assert!(source.contains("local done = __next_done[2]"));
        assert!(source.contains("local value = __next_done[1]"));
        assert!(!source.contains("[unsupported op: iter_next_unboxed]"));
    }

    #[test]
    fn test_compile_checked_lowers_checked_add_helper() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "checked_add_test".to_string(),
                params: vec!["a".to_string(), "b".to_string()],
                param_types: Some(vec!["int".to_string(), "int".to_string()]),
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "checked_add".to_string(),
                        args: Some(vec!["a".to_string(), "b".to_string()]),
                        var: Some("sum".to_string()),
                        out: Some("overflow".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["sum".to_string()]),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let source = backend
            .compile_checked(&ir)
            .expect("checked_add should lower without stub markers");

        assert!(source.contains("local function molt_checked_i64_add"));
        assert!(source.contains("return a + b, false"));
        assert!(
            source.contains("local sum: number, overflow: boolean = molt_checked_i64_add(a, b)")
        );
        assert!(!source.contains("[unsupported op: checked_add]"));
    }

    #[test]
    fn test_compile_checked_lowers_checked_mul_helper() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "checked_mul_test".to_string(),
                params: vec!["a".to_string(), "b".to_string()],
                param_types: Some(vec!["int".to_string(), "int".to_string()]),
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "checked_mul".to_string(),
                        args: Some(vec!["a".to_string(), "b".to_string()]),
                        var: Some("product".to_string()),
                        out: Some("overflow".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["product".to_string()]),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let source = backend
            .compile_checked(&ir)
            .expect("checked_mul should lower without stub markers");

        assert!(source.contains("local function molt_checked_i64_mul"));
        assert!(source.contains("if p >= 9007199254740992 or p <= -9007199254740992"));
        assert!(
            source.contains(
                "local product: number, overflow: boolean = molt_checked_i64_mul(a, b)"
            )
        );
        assert!(!source.contains("[unsupported op: checked_mul]"));
    }

    #[test]
    fn test_compile_checked_lowers_intarray_from_seq_dense_integer_table() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "intarray_from_seq_test".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "const".to_string(),
                        out: Some("one".to_string()),
                        value: Some(1),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const".to_string(),
                        out: Some("two".to_string()),
                        value: Some(2),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "list_new".to_string(),
                        out: Some("seq".to_string()),
                        args: Some(vec!["one".to_string(), "two".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "intarray_from_seq".to_string(),
                        out: Some("arr".to_string()),
                        args: Some(vec!["seq".to_string()]),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let source = backend
            .compile_checked(&ir)
            .expect("intarray_from_seq should lower to a dense Luau integer table");
        assert!(
            source.contains("local arr\n")
                && source.contains("\tdo\n")
                && source.contains("local __seq = seq")
                && source.contains("local __arr = {}")
                && source.contains("math.floor(__v) == __v")
                && source.contains("arr = if __ok then __arr else nil")
                && source.contains("arr = nil"),
            "intarray_from_seq should copy integer tables and fail closed, got:\n{source}"
        );
        assert!(
            !source.contains("[intarray_from_seq]")
                && !source.contains("[unsupported op: intarray_from_seq]"),
            "intarray_from_seq must not leave checked-output markers, got:\n{source}"
        );
    }

    #[test]
    fn test_compile_checked_lowers_fused_dict_kernels() {
        let ir = SimpleIR {
            functions: vec![
                FunctionIR {
                    name: "split_ws_dict_inc_test".to_string(),
                    params: vec!["line".to_string(), "dict".to_string(), "delta".to_string()],
                    param_types: Some(vec![
                        "str".to_string(),
                        "dict".to_string(),
                        "int".to_string(),
                    ]),
                    source_file: None,
                    is_extern: false,
                    ops: vec![
                        OpIR {
                            kind: "string_split_ws_dict_inc".to_string(),
                            args: Some(vec![
                                "line".to_string(),
                                "dict".to_string(),
                                "delta".to_string(),
                            ]),
                            out: Some("ws_result".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "ret".to_string(),
                            args: Some(vec!["ws_result".to_string()]),
                            ..OpIR::default()
                        },
                    ],
                },
                FunctionIR {
                    name: "split_sep_dict_inc_test".to_string(),
                    params: vec![
                        "line".to_string(),
                        "sep".to_string(),
                        "dict".to_string(),
                        "delta".to_string(),
                    ],
                    param_types: Some(vec![
                        "str".to_string(),
                        "str".to_string(),
                        "dict".to_string(),
                        "int".to_string(),
                    ]),
                    source_file: None,
                    is_extern: false,
                    ops: vec![
                        OpIR {
                            kind: "string_split_sep_dict_inc".to_string(),
                            args: Some(vec![
                                "line".to_string(),
                                "sep".to_string(),
                                "dict".to_string(),
                                "delta".to_string(),
                            ]),
                            out: Some("sep_result".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "ret".to_string(),
                            args: Some(vec!["sep_result".to_string()]),
                            ..OpIR::default()
                        },
                    ],
                },
                FunctionIR {
                    name: "taq_ingest_line_test".to_string(),
                    params: vec![
                        "dict".to_string(),
                        "line".to_string(),
                        "bucket_size".to_string(),
                    ],
                    param_types: Some(vec![
                        "dict".to_string(),
                        "str".to_string(),
                        "int".to_string(),
                    ]),
                    source_file: None,
                    is_extern: false,
                    ops: vec![
                        OpIR {
                            kind: "taq_ingest_line".to_string(),
                            args: Some(vec![
                                "dict".to_string(),
                                "line".to_string(),
                                "bucket_size".to_string(),
                            ]),
                            out: Some("ingested".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "ret".to_string(),
                            args: Some(vec!["ingested".to_string()]),
                            ..OpIR::default()
                        },
                    ],
                },
            ],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let source = backend
            .compile_checked(&ir)
            .expect("fused dict kernels should lower without unsupported markers");

        assert!(source.contains("local function molt_string_split_ws_dict_inc"));
        assert!(source.contains("local function molt_string_split_sep_dict_inc"));
        assert!(source.contains("local function molt_taq_ingest_line"));
        assert!(!source.contains("local molt_string = {"));
        assert!(
            source.contains("local ws_result = molt_string_split_ws_dict_inc(line, dict, delta)")
        );
        assert!(
            source.contains(
                "local sep_result = molt_string_split_sep_dict_inc(line, sep, dict, delta)"
            )
        );
        assert!(source.contains("local ingested = molt_taq_ingest_line(dict, line, bucket_size)"));
        assert!(source.contains(
            "series[#series + 1] = {molt_taq_div_euclid(timestamp, bucket_size), volume}"
        ));
        assert!(!source.contains("[unsupported op: string_split_ws_dict_inc]"));
        assert!(!source.contains("[unsupported op: string_split_sep_dict_inc]"));
        assert!(!source.contains("[unsupported op: taq_ingest_line]"));
    }

    #[test]
    fn test_compile_checked_lowers_labeled_branch_ops() {
        let branch_function = |name: &str, kind: &str, label: i64, flag_value: i64| FunctionIR {
            name: name.to_string(),
            params: Vec::new(),
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "const_bool".to_string(),
                    value: Some(flag_value),
                    out: Some("flag".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: kind.to_string(),
                    value: Some(label),
                    args: Some(vec!["flag".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    value: Some(0),
                    out: Some("zero".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["zero".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "label".to_string(),
                    value: Some(label),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    value: Some(1),
                    out: Some("one".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["one".to_string()]),
                    ..OpIR::default()
                },
            ],
        };
        let ir = SimpleIR {
            functions: vec![
                branch_function("br_if_test", "br_if", 7, 1),
                branch_function("branch_test", "branch", 8, 1),
                branch_function("branch_false_test", "branch_false", 9, 0),
            ],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let source = backend
            .compile_checked(&ir)
            .expect("labeled branch ops should lower without unsupported markers");

        assert!(source.contains("br_if_test = function()"));
        assert!(source.contains("branch_test = function()"));
        assert!(source.contains("branch_false_test = function()"));
        assert!(!source.contains("[unsupported op: br_if"));
        assert!(!source.contains("[unsupported op: branch "));
        assert!(!source.contains("[unsupported op: branch_false"));
    }

    #[test]
    fn test_compile_via_ir_rejects_unsupported_output() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "unsupported_test".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![OpIR {
                    kind: "unknown_luau_op".to_string(),
                    out: Some("v0".to_string()),
                    args: Some(vec!["v1".to_string(), "v2".to_string()]),
                    ..OpIR::default()
                }],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let err = backend
            .compile_via_ir(&ir)
            .expect_err("preview/IR path must reject unsupported output");
        assert!(err.contains("unsupported marker"));
        assert!(err.contains("[unsupported op: unknown_luau_op]"));
    }

    #[test]
    fn test_compile_checked_lowers_matmul_dunder_dispatch() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "matmul_test".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![OpIR {
                    kind: "matmul".to_string(),
                    out: Some("v0".to_string()),
                    args: Some(vec!["v1".to_string(), "v2".to_string()]),
                    ..OpIR::default()
                }],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let source = backend
            .compile_checked(&ir)
            .expect("matmul should lower through Luau dunder helper");
        assert!(
            source.contains("local function molt_matmul")
                && source.contains("local v0 = molt_matmul(v1, v2)")
                && source.contains("molt_get_attr(a, \"__matmul__\")")
                && source.contains("molt_get_attr(b, \"__rmatmul__\")"),
            "matmul should share Luau descriptor lookup authority, got:\n{source}"
        );
        assert!(
            !source.contains("[unsupported op: matmul]"),
            "matmul must not leave checked-output markers, got:\n{source}"
        );
    }

    #[test]
    fn test_compile_checked_lowers_matmul_not_implemented_reflection() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "matmul_not_implemented_test".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "const_not_implemented".to_string(),
                        out: Some("not_impl".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "matmul".to_string(),
                        out: Some("v0".to_string()),
                        args: Some(vec!["lhs".to_string(), "rhs".to_string()]),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let source = backend
            .compile_checked(&ir)
            .expect("NotImplemented-aware matmul should lower without markers");
        assert!(
            source.contains("local molt_not_implemented = {__molt_not_implemented = true}")
                && source.contains("local not_impl = molt_not_implemented")
                && source.contains("if result ~= molt_not_implemented then return result end"),
            "matmul should use a concrete NotImplemented sentinel, got:\n{source}"
        );
        assert!(
            !source.contains("[unsupported op: matmul]"),
            "matmul NotImplemented path must not leave checked-output markers, got:\n{source}"
        );
    }

    #[test]
    fn test_compile_checked_lowers_inplace_matmul_dunder_dispatch() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "inplace_matmul_test".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![OpIR {
                    kind: "inplace_matmul".to_string(),
                    out: Some("v0".to_string()),
                    args: Some(vec!["lhs".to_string(), "rhs".to_string()]),
                    ..OpIR::default()
                }],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let source = backend
            .compile_checked(&ir)
            .expect("inplace matmul should lower through Luau dunder helper");
        assert!(
            source.contains("local function molt_inplace_matmul")
                && source.contains("local v0 = molt_inplace_matmul(lhs, rhs)")
                && source.contains("molt_get_attr(a, \"__imatmul__\")")
                && source.contains("return molt_matmul_impl(a, b, \"@=\")"),
            "inplace matmul should try __imatmul__ before binary fallback, got:\n{source}"
        );
        assert!(
            !source.contains("[unsupported op: inplace_matmul]"),
            "inplace matmul must not leave checked-output markers, got:\n{source}"
        );
    }

    #[test]
    fn test_compile_checked_rejects_async_marker() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "async_test".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![OpIR {
                    kind: "spawn".to_string(),
                    out: Some("v0".to_string()),
                    ..OpIR::default()
                }],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let err = backend
            .compile_checked(&ir)
            .expect_err("compile_checked must reject async stub markers");
        assert!(
            err.contains("semantic stub marker"),
            "error should mention semantic stub marker, got: {err}"
        );
    }

    #[test]
    fn test_compile_checked_lowers_call_async_poll_target_directly() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "call_async_test".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "const".to_string(),
                        out: Some("payload".to_string()),
                        value: Some(5),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "call_async".to_string(),
                        s_value: Some("poll_target".to_string()),
                        args: Some(vec!["payload".to_string()]),
                        out: Some("awaited".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["awaited".to_string()]),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let source = backend
            .compile_checked(&ir)
            .expect("call_async with a known poll target should lower directly");

        assert!(
            source.contains("local awaited = poll_target(payload)"),
            "call_async should invoke the s_value poll target directly, got:\n{source}"
        );
        assert!(
            !source.contains("[async: call_async]")
                && !source.contains("[unsupported op: call_async]"),
            "call_async must not leave async stub markers, got:\n{source}"
        );
    }

    #[test]
    fn test_compile_checked_lowers_is_native_awaitable_target_fact() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "native_awaitable_test".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "object_new".to_string(),
                        out: Some("awaitable".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "is_native_awaitable".to_string(),
                        out: Some("is_native".to_string()),
                        args: Some(vec!["awaitable".to_string()]),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let source = backend
            .compile_checked(&ir)
            .expect("is_native_awaitable should lower as a Luau target fact");
        assert!(
            source.contains("local is_native = false"),
            "Luau has no native Molt poll-function objects, got:\n{source}"
        );
        assert!(
            !source.contains("[async: is_native_awaitable]")
                && !source.contains("[unsupported op: is_native_awaitable]"),
            "is_native_awaitable must not lower through async stubs, got:\n{source}"
        );
    }

    #[test]
    fn test_compile_checked_rejects_file_marker() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "file_test".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![OpIR {
                    kind: "file_open".to_string(),
                    out: Some("v0".to_string()),
                    ..OpIR::default()
                }],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let err = backend
            .compile_checked(&ir)
            .expect_err("compile_checked must reject file stub markers");
        assert!(
            err.contains("semantic stub marker"),
            "error should mention semantic stub marker, got: {err}"
        );
    }

    #[test]
    fn test_compile_checked_rejects_context_marker() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "context_test".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![OpIR {
                    kind: "context_enter".to_string(),
                    out: Some("v0".to_string()),
                    ..OpIR::default()
                }],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let err = backend
            .compile_checked(&ir)
            .expect_err("compile_checked must reject context stub markers");
        assert!(
            err.contains("semantic stub marker"),
            "error should mention semantic stub marker, got: {err}"
        );
    }

    #[test]
    fn test_compile_checked_lowers_type_check_helpers() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "type_check_test".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "const".to_string(),
                        out: Some("int_tag".to_string()),
                        value: Some(1),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "builtin_type".to_string(),
                        out: Some("int_cls".to_string()),
                        args: Some(vec!["int_tag".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const".to_string(),
                        out: Some("bool_tag".to_string()),
                        value: Some(3),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "builtin_type".to_string(),
                        out: Some("bool_cls".to_string()),
                        args: Some(vec!["bool_tag".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const_bool".to_string(),
                        out: Some("flag".to_string()),
                        value: Some(1),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "isinstance".to_string(),
                        out: Some("is_int".to_string()),
                        args: Some(vec!["flag".to_string(), "int_cls".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "issubclass".to_string(),
                        out: Some("bool_is_int".to_string()),
                        args: Some(vec!["bool_cls".to_string(), "int_cls".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "class_new".to_string(),
                        out: Some("base_cls".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "class_new".to_string(),
                        out: Some("derived_cls".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "class_set_base".to_string(),
                        args: Some(vec!["derived_cls".to_string(), "base_cls".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "object_new".to_string(),
                        out: Some("obj".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "object_set_class".to_string(),
                        args: Some(vec!["obj".to_string(), "derived_cls".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "isinstance".to_string(),
                        out: Some("obj_is_base".to_string()),
                        args: Some(vec!["obj".to_string(), "base_cls".to_string()]),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let source = backend
            .compile_checked(&ir)
            .expect("type-check ops should lower through Luau helper authority");
        assert!(
            source.contains("local function molt_builtin_type")
                && source.contains("local function molt_issubclass")
                && source.contains("local function molt_isinstance"),
            "type-check helper authority should be emitted, got:\n{source}"
        );
        assert!(
            source.contains("local int_cls = molt_builtin_type(int_tag)")
                && source.contains("local is_int = molt_isinstance(flag, int_cls)")
                && source.contains("local bool_is_int = molt_issubclass(bool_cls, int_cls)")
                && source.contains("local base_cls = {__molt_is_type = true}")
                && source.contains("local obj_is_base = molt_isinstance(obj, base_cls)"),
            "type-check ops should use named builtin/class metadata, got:\n{source}"
        );
        assert!(
            !source.contains("[stub: isinstance]")
                && !source.contains("[unsupported op: isinstance]")
                && !source.contains("[unsupported op: issubclass]")
                && !source.contains("[unsupported op: builtin_type]"),
            "type-check ops must not leave checked-output markers, got:\n{source}"
        );
    }

    #[test]
    fn test_compile_checked_lowers_descriptor_attribute_authority() {
        let ir = SimpleIR {
            functions: vec![
                FunctionIR {
                    name: "descriptor_attribute_test".to_string(),
                    params: vec![],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                    ops: vec![
                        OpIR {
                            kind: "func_new".to_string(),
                            out: Some("method_func".to_string()),
                            s_value: Some("descriptor_method".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "func_new".to_string(),
                            out: Some("class_func".to_string()),
                            s_value: Some("descriptor_class".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "func_new".to_string(),
                            out: Some("static_func".to_string()),
                            s_value: Some("descriptor_static".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "func_new".to_string(),
                            out: Some("get_func".to_string()),
                            s_value: Some("descriptor_get".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "func_new".to_string(),
                            out: Some("set_func".to_string()),
                            s_value: Some("descriptor_set".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "func_new".to_string(),
                            out: Some("del_func".to_string()),
                            s_value: Some("descriptor_del".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "class_new".to_string(),
                            out: Some("cls".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "object_new".to_string(),
                            out: Some("obj".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "object_set_class".to_string(),
                            args: Some(vec!["obj".to_string(), "cls".to_string()]),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "classmethod_new".to_string(),
                            out: Some("cm_desc".to_string()),
                            args: Some(vec!["class_func".to_string()]),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "staticmethod_new".to_string(),
                            out: Some("sm_desc".to_string()),
                            args: Some(vec!["static_func".to_string()]),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "property_new".to_string(),
                            out: Some("prop_desc".to_string()),
                            args: Some(vec![
                                "get_func".to_string(),
                                "set_func".to_string(),
                                "del_func".to_string(),
                            ]),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "set_attr_generic_obj".to_string(),
                            args: Some(vec!["cls".to_string(), "method_func".to_string()]),
                            s_value: Some("method".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "set_attr_generic_obj".to_string(),
                            args: Some(vec!["cls".to_string(), "cm_desc".to_string()]),
                            s_value: Some("cm".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "set_attr_generic_obj".to_string(),
                            args: Some(vec!["cls".to_string(), "sm_desc".to_string()]),
                            s_value: Some("sm".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "set_attr_generic_obj".to_string(),
                            args: Some(vec!["cls".to_string(), "prop_desc".to_string()]),
                            s_value: Some("value".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "get_attr_generic_obj".to_string(),
                            out: Some("cm_bound".to_string()),
                            args: Some(vec!["obj".to_string()]),
                            s_value: Some("cm".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "get_attr_generic_obj".to_string(),
                            out: Some("sm_func".to_string()),
                            args: Some(vec!["cls".to_string()]),
                            s_value: Some("sm".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "get_attr_generic_obj".to_string(),
                            out: Some("prop_value".to_string()),
                            args: Some(vec!["obj".to_string()]),
                            s_value: Some("value".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "const_str".to_string(),
                            out: Some("value_name".to_string()),
                            s_value: Some("value".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "has_attr_name".to_string(),
                            out: Some("has_value".to_string()),
                            args: Some(vec!["obj".to_string(), "value_name".to_string()]),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "const".to_string(),
                            out: Some("new_value".to_string()),
                            value: Some(7),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "set_attr_generic_obj".to_string(),
                            args: Some(vec!["obj".to_string(), "new_value".to_string()]),
                            s_value: Some("value".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "del_attr_generic_obj".to_string(),
                            args: Some(vec!["obj".to_string()]),
                            s_value: Some("value".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "call_method".to_string(),
                            out: Some("method_result".to_string()),
                            args: Some(vec!["obj".to_string()]),
                            s_value: Some("method".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "builtin_func".to_string(),
                            out: Some("getattr_builtin".to_string()),
                            s_value: Some("molt_getattr_builtin".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "builtin_func".to_string(),
                            out: Some("setattr_builtin".to_string()),
                            s_value: Some("molt_set_attr_name".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "builtin_func".to_string(),
                            out: Some("delattr_builtin".to_string()),
                            s_value: Some("molt_del_attr_name".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "builtin_func".to_string(),
                            out: Some("hasattr_builtin".to_string()),
                            s_value: Some("molt_has_attr_name".to_string()),
                            ..OpIR::default()
                        },
                    ],
                },
                FunctionIR {
                    name: "descriptor_method".to_string(),
                    params: vec!["self".to_string()],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                    ops: vec![OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    }],
                },
                FunctionIR {
                    name: "descriptor_class".to_string(),
                    params: vec!["cls".to_string()],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                    ops: vec![OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    }],
                },
                FunctionIR {
                    name: "descriptor_static".to_string(),
                    params: vec![],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                    ops: vec![OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    }],
                },
                FunctionIR {
                    name: "descriptor_get".to_string(),
                    params: vec!["self".to_string()],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                    ops: vec![OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    }],
                },
                FunctionIR {
                    name: "descriptor_set".to_string(),
                    params: vec!["self".to_string(), "value".to_string()],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                    ops: vec![OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    }],
                },
                FunctionIR {
                    name: "descriptor_del".to_string(),
                    params: vec!["self".to_string()],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                    ops: vec![OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    }],
                },
            ],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let source = backend
            .compile_checked(&ir)
            .expect("descriptor ops should lower through Luau attribute authority");
        assert!(
            source.contains("local function molt_get_attr")
                && source.contains("local function molt_has_attr")
                && source.contains("local function molt_set_attr")
                && source.contains("local function molt_del_attr"),
            "descriptor-aware attribute helpers should be emitted, got:\n{source}"
        );
        assert!(
            source.contains(
                "local cm_desc = {__molt_descriptor_kind=\"classmethod\", __func=class_func}"
            ) && source.contains(
                "local sm_desc = {__molt_descriptor_kind=\"staticmethod\", __func=static_func}"
            ) && source.contains(
                "local prop_desc = {__molt_descriptor_kind=\"property\", __get=get_func, __set=set_func, __del=del_func}"
            ),
            "descriptor constructors should use one table shape, got:\n{source}"
        );
        assert!(
            source.contains("molt_set_attr(cls, \"cm\", cm_desc)")
                && source.contains("local cm_bound = molt_get_attr(obj, \"cm\")")
                && source.contains("local sm_func = molt_get_attr(cls, \"sm\")")
                && source.contains("local prop_value = molt_get_attr(obj, \"value\")")
                && source.contains("local has_value = molt_has_attr(obj, value_name)")
                && source.contains("molt_set_attr(obj, \"value\", new_value)")
                && source.contains("molt_del_attr(obj, \"value\")")
                && source.contains(
                    "local method_result; do local __method = molt_get_attr(obj, \"method\");"
                ),
            "attribute get/set/delete and method call should route through descriptor authority, got:\n{source}"
        );
        assert!(
            source.contains("local getattr_builtin = function(a, ...)")
                && source.contains("local value = molt_get_attr(a[1], a[2])")
                && source.contains("local setattr_builtin = function(a, ...) return molt_set_attr(a[1], a[2], a[3]) end")
                && source.contains("local delattr_builtin = function(a, ...) return molt_del_attr(a[1], a[2]) end")
                && source.contains("local hasattr_builtin = function(a, ...) return molt_has_attr(a[1], a[2]) end")
                && !source.contains("molt_getattr(table.unpack(a))"),
            "attribute builtins should route through descriptor helpers, got:\n{source}"
        );
        assert!(
            !source.contains("[classmethod_new]")
                && !source.contains("[staticmethod_new]")
                && !source.contains("[property_new]")
                && !source.contains("[unsupported op: classmethod_new]")
                && !source.contains("[unsupported op: staticmethod_new]")
                && !source.contains("[unsupported op: property_new]"),
            "descriptor ops must not leave checked-output markers, got:\n{source}"
        );
    }

    #[test]
    fn test_compile_checked_lowers_class_apply_set_name_authority() {
        let ir = SimpleIR {
            functions: vec![
                FunctionIR {
                    name: "class_apply_set_name_test".to_string(),
                    params: vec![],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                    ops: vec![
                        OpIR {
                            kind: "func_new".to_string(),
                            out: Some("set_name_func".to_string()),
                            s_value: Some("descriptor_set_name".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "class_new".to_string(),
                            out: Some("descriptor_cls".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "set_attr_generic_obj".to_string(),
                            args: Some(vec![
                                "descriptor_cls".to_string(),
                                "set_name_func".to_string(),
                            ]),
                            s_value: Some("__set_name__".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "object_new".to_string(),
                            out: Some("descriptor".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "object_set_class".to_string(),
                            args: Some(vec![
                                "descriptor".to_string(),
                                "descriptor_cls".to_string(),
                            ]),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "class_new".to_string(),
                            out: Some("owner_cls".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "set_attr_generic_obj".to_string(),
                            args: Some(vec!["owner_cls".to_string(), "descriptor".to_string()]),
                            s_value: Some("field".to_string()),
                            ..OpIR::default()
                        },
                        OpIR {
                            kind: "class_apply_set_name".to_string(),
                            args: Some(vec!["owner_cls".to_string()]),
                            out: Some("none".to_string()),
                            ..OpIR::default()
                        },
                    ],
                },
                FunctionIR {
                    name: "descriptor_set_name".to_string(),
                    params: vec!["self".to_string(), "owner".to_string(), "name".to_string()],
                    param_types: None,
                    source_file: None,
                    is_extern: false,
                    ops: vec![OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    }],
                },
            ],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let source = backend
            .compile_checked(&ir)
            .expect("class_apply_set_name should lower through descriptor authority");
        assert!(
            source.contains("local function molt_class_apply_set_name")
                && source.contains("local entries = {}")
                && source.contains("local hook = molt_get_attr(value, \"__set_name__\")")
                && source.contains("if hook ~= nil then hook(cls, name) end"),
            "class_apply_set_name helper should snapshot and dispatch hooks, got:\n{source}"
        );
        assert!(
            source.contains("molt_set_attr(descriptor_cls, \"__set_name__\", set_name_func)")
                && source.contains("molt_set_attr(owner_cls, \"field\", descriptor)")
                && source.contains("molt_class_apply_set_name(owner_cls)"),
            "dunder class attrs and apply op should share attribute authority, got:\n{source}"
        );
        assert!(
            !source.contains("[class op: class_apply_set_name]")
                && !source.contains("[unsupported op: class_apply_set_name]")
                && !source.contains("All other dunders"),
            "class_apply_set_name must not leave stale stub/no-op markers, got:\n{source}"
        );
    }

    #[test]
    fn test_compile_checked_rejects_internal_marker() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "internal_test".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![OpIR {
                    kind: "function_closure_bits".to_string(),
                    out: Some("v0".to_string()),
                    ..OpIR::default()
                }],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let err = backend
            .compile_checked(&ir)
            .expect_err("compile_checked must reject internal stub markers");
        assert!(
            err.contains("semantic stub marker"),
            "error should mention semantic stub marker, got: {err}"
        );
    }

    #[test]
    fn test_compile_checked_lowers_bridge_unavailable_to_runtime_error() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "bridge_unavailable_test".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "const_str".to_string(),
                        out: Some("message".to_string()),
                        s_value: Some("dynamic bridge disabled".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "bridge_unavailable".to_string(),
                        out: Some("v0".to_string()),
                        args: Some(vec!["message".to_string()]),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let source = backend
            .compile_checked(&ir)
            .expect("bridge_unavailable must lower to a checked runtime error");
        assert!(
            source.contains("local v0: any = error({__type=\"RuntimeError\""),
            "bridge_unavailable should be a terminal RuntimeError expression, got:\n{source}"
        );
        assert!(
            source.contains("Molt bridge unavailable: "),
            "diagnostic should match runtime bridge-unavailable prefix, got:\n{source}"
        );
        assert!(
            !source.contains("[bridge_unavailable]"),
            "bridge_unavailable must not leave a semantic stub marker, got:\n{source}"
        );
    }

    #[test]
    fn test_compile_checked_lowers_invoke_ffi_to_luau_capability_error() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "invoke_ffi_capability_test".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![OpIR {
                    kind: "invoke_ffi".to_string(),
                    out: Some("v0".to_string()),
                    ..OpIR::default()
                }],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let source = backend
            .compile_checked(&ir)
            .expect("invoke_ffi should lower to a Luau target capability error");
        assert!(
            source.contains(
                "local v0: any = error({__type=\"RuntimeError\", __msg=\"Luau target does not support FFI\"})"
            ),
            "invoke_ffi should be an explicit target capability error, got:\n{source}"
        );
        assert!(
            !source.contains("[invoke_ffi]") && !source.contains("[unsupported op: invoke_ffi]"),
            "invoke_ffi must not leave semantic stub markers, got:\n{source}"
        );
    }

    #[test]
    fn test_compile_checked_lowers_object_set_class_metatable() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "object_set_class_test".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "object_new".to_string(),
                        out: Some("obj".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "class_new".to_string(),
                        out: Some("cls".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "object_set_class".to_string(),
                        args: Some(vec!["obj".to_string(), "cls".to_string()]),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let source = backend
            .compile_checked(&ir)
            .expect("object_set_class must lower to Luau metatable assignment");
        assert!(
            source.contains("setmetatable(obj, cls)"),
            "object_set_class should bind the object to its class metatable, got:\n{source}"
        );
        assert!(
            !source.contains("[class op: object_set_class]"),
            "object_set_class must not be reported as a class-op marker, got:\n{source}"
        );
    }

    #[test]
    fn test_compile_checked_lowers_class_layout_metadata() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "class_layout_metadata_test".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "class_new".to_string(),
                        out: Some("cls".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "class_layout_version".to_string(),
                        out: Some("version_before".to_string()),
                        args: Some(vec!["cls".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const_str".to_string(),
                        out: Some("field_name".to_string()),
                        s_value: Some("field".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const".to_string(),
                        out: Some("field_offset".to_string()),
                        value: Some(0),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "dict_new".to_string(),
                        out: Some("offsets".to_string()),
                        args: Some(vec!["field_name".to_string(), "field_offset".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const".to_string(),
                        out: Some("layout_size".to_string()),
                        value: Some(24),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "class_merge_layout".to_string(),
                        args: Some(vec![
                            "cls".to_string(),
                            "offsets".to_string(),
                            "layout_size".to_string(),
                        ]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const".to_string(),
                        out: Some("layout_version".to_string()),
                        value: Some(7),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "class_set_layout_version".to_string(),
                        args: Some(vec!["cls".to_string(), "layout_version".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "class_layout_version".to_string(),
                        out: Some("version_after".to_string()),
                        args: Some(vec!["cls".to_string()]),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let source = backend
            .compile_checked(&ir)
            .expect("class layout metadata ops must lower to Luau class-table metadata");
        assert!(
            source.contains("local version_before = if type(cls) == \"table\""),
            "class_layout_version should read class-table layout metadata, got:\n{source}"
        );
        assert!(
            source.contains("__cls.__molt_field_offsets__ = __merged")
                && source.contains("__cls.__molt_layout_size__ = __layout_size"),
            "class_merge_layout should maintain field offsets and layout size, got:\n{source}"
        );
        assert!(
            source.contains("__cls.__molt_layout_version = __version"),
            "class_set_layout_version should write layout version metadata, got:\n{source}"
        );
        assert!(
            !source.contains("[class op: class_layout_version]")
                && !source.contains("[class op: class_set_layout_version]")
                && !source.contains("[class op: class_merge_layout]"),
            "layout metadata ops must not leave class-op markers, got:\n{source}"
        );
    }

    #[test]
    fn test_default_luau_dispatch_uses_checked_path() {
        // Verify that both compile_via_ir and compile_checked reject the same unknown ops.
        // This ensures no fail-open path exists.
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "dispatch_test".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![OpIR {
                    kind: "unknown_luau_op".to_string(),
                    out: Some("v0".to_string()),
                    args: Some(vec!["v1".to_string(), "v2".to_string()]),
                    ..OpIR::default()
                }],
            }],
            profile: None,
        };
        let mut backend_ir = LuauBackend::new();
        let mut backend_checked = LuauBackend::new();
        let err_ir = backend_ir
            .compile_via_ir(&ir)
            .expect_err("compile_via_ir must reject unknown ops");
        let err_checked = backend_checked
            .compile_checked(&ir)
            .expect_err("compile_checked must reject unknown ops");
        assert_eq!(
            err_ir, err_checked,
            "compile_via_ir and compile_checked must produce identical errors"
        );
    }

    #[test]
    fn test_luau_repr_authority_typed_list_call_method_dispatch() {
        // Structured TIR facts, not legacy transport hints, authorize direct
        // list-method lowering for Luau tables.
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "append_to".to_string(),
                params: vec!["xs".to_string(), "v".to_string()],
                param_types: Some(vec!["list[int]".to_string(), "int".to_string()]),
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "call_method".to_string(),
                        s_value: Some("append".to_string()),
                        args: Some(vec!["xs".to_string(), "v".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        // Must use direct table insertion, not xs:append(v).
        assert!(
            output.contains("xs[#xs + 1] = v"),
            "Expected table insert for list param, got:\n{output}"
        );
        assert!(
            !output.contains("xs:append"),
            "Must NOT emit method call for list.append(), got:\n{output}"
        );
    }

    #[test]
    fn test_bool_arithmetic_coerces_bool_operands() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "bool_arithmetic".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "const_bool".to_string(),
                        value: Some(1),
                        out: Some("v0".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const_bool".to_string(),
                        value: Some(0),
                        out: Some("v1".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "add".to_string(),
                        args: Some(vec!["v0".to_string(), "v1".to_string()]),
                        out: Some("v2".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "sub".to_string(),
                        args: Some(vec!["v0".to_string(), "v1".to_string()]),
                        out: Some("v3".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "mul".to_string(),
                        args: Some(vec!["v0".to_string(), "v1".to_string()]),
                        out: Some("v4".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(
            output.contains("then 1 else 0"),
            "bool operands must be numerically coerced in arithmetic, got:\n{output}"
        );
        assert!(
            !output.contains("true + false"),
            "bool addition must not emit raw Luau boolean arithmetic, got:\n{output}"
        );
    }

    #[test]
    fn test_result_type_hint_does_not_prove_luau_not_operand_bool() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "truthy_not".to_string(),
                params: vec!["x".to_string()],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "not".to_string(),
                        args: Some(vec!["x".to_string()]),
                        out: Some("v0".to_string()),
                        type_hint: Some("bool".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["v0".to_string()]),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(
            output.contains("not molt_bool(x)"),
            "result-side type_hint=bool must not bypass Python truthiness for not, got:\n{output}"
        );
        assert!(
            !output.contains("not x"),
            "unknown operands must not use raw Luau boolean not, got:\n{output}"
        );
    }

    #[test]
    fn test_result_type_hint_does_not_prove_luau_and_or_operands_bool() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "truthy_and_or".to_string(),
                params: vec!["a".to_string(), "b".to_string()],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "and".to_string(),
                        args: Some(vec!["a".to_string(), "b".to_string()]),
                        out: Some("v0".to_string()),
                        type_hint: Some("bool".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "or".to_string(),
                        args: Some(vec!["a".to_string(), "b".to_string()]),
                        out: Some("v1".to_string()),
                        type_hint: Some("bool".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["v1".to_string()]),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(
            output.contains("if molt_bool(a) then b else a"),
            "and must preserve Python value-returning truthiness for unknown operands, got:\n{output}"
        );
        assert!(
            output.contains("if molt_bool(a) then a else b"),
            "or must preserve Python value-returning truthiness for unknown operands, got:\n{output}"
        );
        assert!(
            !output.contains("local v0 = a and b") && !output.contains("local v1 = a or b"),
            "result-side type_hint=bool must not select native Luau and/or, got:\n{output}"
        );
    }

    #[test]
    fn test_result_type_hint_does_not_force_luau_numeric_add() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "hinted_add".to_string(),
                params: vec!["a".to_string(), "b".to_string()],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "add".to_string(),
                        args: Some(vec!["a".to_string(), "b".to_string()]),
                        out: Some("v0".to_string()),
                        type_hint: Some("int".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["v0".to_string()]),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(
            output.contains("if type(a) == \"string\" or type(b) == \"string\""),
            "unknown add operands must keep Python string-concat guard, got:\n{output}"
        );
        assert!(
            !output.contains("local v0: number ="),
            "result-side type_hint=int must not force numeric add lowering, got:\n{output}"
        );
    }

    #[test]
    fn test_transport_hints_do_not_force_luau_numeric_add() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "transport_hinted_add".to_string(),
                params: vec!["a".to_string(), "b".to_string()],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "add".to_string(),
                        args: Some(vec!["a".to_string(), "b".to_string()]),
                        out: Some("v0".to_string()),
                        fast_int: Some(true),
                        fast_float: Some(true),
                        type_hint: Some("int".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["v0".to_string()]),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(
            output.contains("if type(a) == \"string\" or type(b) == \"string\""),
            "transport hints must not bypass unknown add guard, got:\n{output}"
        );
        assert!(
            !output.contains("local v0: number ="),
            "transport hints must not select numeric add lowering, got:\n{output}"
        );
    }

    #[test]
    fn test_type_hint_int_does_not_force_luau_integer_index() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "hinted_index".to_string(),
                params: vec!["xs".to_string(), "key".to_string()],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "get_item".to_string(),
                        args: Some(vec!["xs".to_string(), "key".to_string()]),
                        out: Some("v0".to_string()),
                        type_hint: Some("int".to_string()),
                        fast_int: Some(true),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["v0".to_string()]),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(
            output.contains("if type(key) == \"number\""),
            "unknown key must keep dynamic key normalization, got:\n{output}"
        );
        assert!(
            !output.contains("xs[if key >= 0 then key + 1"),
            "transport hints must not select integer-only indexing, got:\n{output}"
        );
    }

    #[test]
    fn test_container_transport_hints_do_not_force_luau_list_dispatch() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "hinted_container_index".to_string(),
                params: vec!["xs".to_string(), "key".to_string(), "value".to_string()],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "get_item".to_string(),
                        args: Some(vec!["xs".to_string(), "key".to_string()]),
                        out: Some("v0".to_string()),
                        type_hint: Some("list".to_string()),
                        container_type: Some("list".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "set_item".to_string(),
                        args: Some(vec![
                            "xs".to_string(),
                            "key".to_string(),
                            "value".to_string(),
                        ]),
                        type_hint: Some("list".to_string()),
                        container_type: Some("list".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["v0".to_string()]),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);

        assert!(
            output.contains("if type(key) == \"number\""),
            "unknown container must keep dynamic key normalization, got:\n{output}"
        );
        assert!(
            !output.contains("rawget(xs") && !output.contains("rawset(xs"),
            "transport hints must not select raw list dispatch, got:\n{output}"
        );
        assert!(
            !output.contains("list index out of range")
                && !output.contains("list assignment index out of range"),
            "transport hints must not select list bounds-guard path, got:\n{output}"
        );
    }

    #[test]
    fn test_len_transport_hint_does_not_force_luau_raw_length() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "hinted_len".to_string(),
                params: vec!["xs".to_string()],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "len".to_string(),
                        args: Some(vec!["xs".to_string()]),
                        out: Some("n".to_string()),
                        type_hint: Some("list".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["n".to_string()]),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);

        assert!(
            output.contains("local n = molt_len(xs)"),
            "unknown len operand must stay on runtime len, got:\n{output}"
        );
        assert!(
            !output.contains("local n = #xs"),
            "result-side type_hint must not select raw Luau length, got:\n{output}"
        );
    }

    #[test]
    fn test_len_uses_tir_container_fact_for_luau_raw_length() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "typed_len".to_string(),
                params: vec!["xs".to_string()],
                param_types: Some(vec!["list[int]".to_string()]),
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "len".to_string(),
                        args: Some(vec!["xs".to_string()]),
                        out: Some("n".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["n".to_string()]),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);

        assert!(
            output.contains("local n = #xs"),
            "typed list len should use raw Luau length, got:\n{output}"
        );
        assert!(
            !output.contains("local n = molt_len(xs)"),
            "typed list len should not call runtime len, got:\n{output}"
        );
    }

    #[test]
    fn test_typed_list_truthiness_uses_luau_raw_length_for_not() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "typed_list_not".to_string(),
                params: vec!["xs".to_string()],
                param_types: Some(vec!["list[int]".to_string()]),
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "not".to_string(),
                        args: Some(vec!["xs".to_string()]),
                        out: Some("empty".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["empty".to_string()]),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);

        assert!(
            output.contains("local empty: boolean = not (#xs > 0)"),
            "typed list truthiness should use raw Luau length for not, got:\n{output}"
        );
        assert!(
            !output.contains("not molt_bool(xs)"),
            "typed list truthiness should not call runtime bool for not, got:\n{output}"
        );
    }

    #[test]
    fn test_typed_dict_truthiness_uses_luau_next_for_or() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "typed_dict_or".to_string(),
                params: vec!["d".to_string(), "fallback".to_string()],
                param_types: Some(vec![
                    "dict[str, int]".to_string(),
                    "dict[str, int]".to_string(),
                ]),
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "or".to_string(),
                        args: Some(vec!["d".to_string(), "fallback".to_string()]),
                        out: Some("selected".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["selected".to_string()]),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);

        assert!(
            output.contains("local selected = if (next(d) ~= nil) then d else fallback"),
            "typed dict truthiness should use raw Luau next() for or, got:\n{output}"
        );
        assert!(
            !output.contains("molt_bool(d)"),
            "typed dict truthiness should not call runtime bool for or, got:\n{output}"
        );
    }

    #[test]
    fn test_list_and_string_get_item_emit_index_error_guards() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "index_guards".to_string(),
                params: vec!["xs".to_string(), "s".to_string(), "i".to_string()],
                param_types: Some(vec![
                    "list[int]".to_string(),
                    "str".to_string(),
                    "int".to_string(),
                ]),
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "get_item".to_string(),
                        args: Some(vec!["xs".to_string(), "i".to_string()]),
                        out: Some("v0".to_string()),
                        type_hint: Some("list".to_string()),
                        fast_int: Some(true),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "get_item".to_string(),
                        args: Some(vec!["s".to_string(), "i".to_string()]),
                        out: Some("v1".to_string()),
                        type_hint: Some("str".to_string()),
                        fast_int: Some(true),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(
            output.contains("__type=\"IndexError\""),
            "list/string indexing must guard out-of-range accesses, got:\n{output}"
        );
        assert!(
            output.contains("list index out of range")
                && output.contains("string index out of range"),
            "expected list and string IndexError messages, got:\n{output}"
        );
    }

    #[test]
    fn test_string_get_item_uses_utf8_codepoint_offsets() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "string_index".to_string(),
                params: vec!["s".to_string(), "i".to_string()],
                param_types: Some(vec!["str".to_string(), "int".to_string()]),
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "get_item".to_string(),
                        args: Some(vec!["s".to_string(), "i".to_string()]),
                        out: Some("v0".to_string()),
                        type_hint: Some("str".to_string()),
                        fast_int: Some(true),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["v0".to_string()]),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(
            output.contains("molt_str_byte_offset(s, __idx_v0)")
                && output.contains("utf8.offset(s, __idx_v0 + 1)"),
            "string indexing must translate codepoint index to byte offsets, got:\n{output}"
        );
        assert!(
            !output.contains("string.sub(s, __idx_v0, __idx_v0)"),
            "string indexing must not fall back to byte-indexed substring extraction, got:\n{output}"
        );
    }

    #[test]
    fn test_ord_at_emits_utf8_codepoint_helper() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "ord_at_unicode".to_string(),
                params: vec!["s".to_string(), "i".to_string()],
                param_types: Some(vec!["str".to_string(), "int".to_string()]),
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "ord_at".to_string(),
                        args: Some(vec!["s".to_string(), "i".to_string()]),
                        out: Some("v0".to_string()),
                        type_hint: Some("int".to_string()),
                        fast_int: Some(true),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["v0".to_string()]),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(
            output.contains("local function molt_ord_at")
                && output.contains("molt_ord_at(s, i)")
                && output.contains("utf8.codepoint(obj, byte_idx)")
                && output.contains("molt_str_codepoint_len(obj)"),
            "ord_at must use the shared UTF-8 codepoint helper path, got:\n{output}"
        );
    }

    #[test]
    fn test_list_set_and_delete_emit_index_error_guards() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "mutation_index_guards".to_string(),
                params: vec!["xs".to_string(), "i".to_string(), "v".to_string()],
                param_types: Some(vec![
                    "list[int]".to_string(),
                    "int".to_string(),
                    "int".to_string(),
                ]),
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "set_item".to_string(),
                        args: Some(vec!["xs".to_string(), "i".to_string(), "v".to_string()]),
                        type_hint: Some("list".to_string()),
                        fast_int: Some(true),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "del_item".to_string(),
                        args: Some(vec!["xs".to_string(), "i".to_string()]),
                        type_hint: Some("list".to_string()),
                        fast_int: Some(true),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(
            output.contains("list assignment index out of range")
                && output.contains("list deletion index out of range"),
            "list set/delete must guard out-of-range accesses, got:\n{output}"
        );
    }

    #[test]
    fn test_list_pop_and_index_emit_python_error_guards() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "list_method_guards".to_string(),
                params: vec!["xs".to_string(), "i".to_string(), "needle".to_string()],
                param_types: Some(vec![
                    "list[int]".to_string(),
                    "int".to_string(),
                    "int".to_string(),
                ]),
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "list_pop".to_string(),
                        args: Some(vec!["xs".to_string()]),
                        out: Some("v0".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "list_pop".to_string(),
                        args: Some(vec!["xs".to_string(), "i".to_string()]),
                        out: Some("v1".to_string()),
                        fast_int: Some(true),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "list_index".to_string(),
                        args: Some(vec!["xs".to_string(), "needle".to_string()]),
                        out: Some("v2".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(
            output.contains("pop from empty list")
                && output.contains("pop index out of range")
                && output.contains("is not in list"),
            "list pop/index must emit Python error guards, got:\n{output}"
        );
    }

    #[test]
    fn test_call_method_list_pop_uses_python_error_guards() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "list_call_method_pop_guards".to_string(),
                params: vec!["xs".to_string(), "i".to_string()],
                param_types: Some(vec!["list[int]".to_string(), "int".to_string()]),
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "call_method".to_string(),
                        s_value: Some("pop".to_string()),
                        args: Some(vec!["xs".to_string()]),
                        out: Some("v0".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "call_method".to_string(),
                        s_value: Some("pop".to_string()),
                        args: Some(vec!["xs".to_string(), "i".to_string()]),
                        out: Some("v1".to_string()),
                        fast_int: Some(true),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(
            output.contains("pop from empty list") && output.contains("pop index out of range"),
            "list method pop must share direct list_pop Python guards, got:\n{output}"
        );
    }

    #[test]
    fn test_list_index_range_honors_start_stop_bounds() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "list_index_range_bounds".to_string(),
                params: vec![
                    "xs".to_string(),
                    "needle".to_string(),
                    "start".to_string(),
                    "stop".to_string(),
                ],
                param_types: Some(vec![
                    "list[int]".to_string(),
                    "int".to_string(),
                    "int".to_string(),
                    "int".to_string(),
                ]),
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "list_index_range".to_string(),
                        args: Some(vec![
                            "xs".to_string(),
                            "needle".to_string(),
                            "start".to_string(),
                            "stop".to_string(),
                        ]),
                        out: Some("v0".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(
            output.contains("__start")
                && output.contains("__stop")
                && output.contains("__n + start")
                && output.contains("__n + stop")
                && output.contains("for __i = __start + 1, __stop do"),
            "list.index(value, start, stop) must honor range bounds, got:\n{output}"
        );
    }

    #[test]
    fn test_dict_popitem_emits_empty_dict_key_error_guard() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "dict_popitem_guard".to_string(),
                params: vec!["d".to_string()],
                param_types: Some(vec!["dict[str, int]".to_string()]),
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "dict_popitem".to_string(),
                        args: Some(vec!["d".to_string()]),
                        out: Some("v0".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(
            output.contains("__type=\"KeyError\"")
                && output.contains("popitem(): dictionary is empty"),
            "dict.popitem must guard empty dictionaries, got:\n{output}"
        );
    }

    #[test]
    fn test_list_insert_clamps_python_index_bounds() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "list_insert_clamps".to_string(),
                params: vec!["xs".to_string(), "i".to_string(), "v".to_string()],
                param_types: Some(vec![
                    "list[int]".to_string(),
                    "int".to_string(),
                    "int".to_string(),
                ]),
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "list_insert".to_string(),
                        args: Some(vec!["xs".to_string(), "i".to_string(), "v".to_string()]),
                        fast_int: Some(true),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(
            output.contains("__idx < 1")
                && output.contains("__idx = 1")
                && output.contains("__idx > #xs + 1")
                && output.contains("xs[#xs + 1] = v"),
            "list.insert must clamp Python indices before mutation, got:\n{output}"
        );
    }

    #[test]
    fn test_list_extend_uses_table_move_fast_path() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "list_extend_fast_path".to_string(),
                params: vec!["dst".to_string(), "src".to_string()],
                param_types: Some(vec!["list[int]".to_string(), "list[int]".to_string()]),
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "list_extend".to_string(),
                        args: Some(vec!["dst".to_string(), "src".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(
            output.contains("table.move(src, 1, #src, #dst + 1, dst)")
                && !output.contains("for __i = 1, #src"),
            "list_extend must use Luau table.move fast path, got:\n{output}"
        );
    }

    #[test]
    fn test_list_repeat_clamps_negative_count_to_empty() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "list_repeat_clamps".to_string(),
                params: vec!["value".to_string(), "count".to_string()],
                param_types: Some(vec!["int".to_string(), "int".to_string()]),
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "list_repeat_range".to_string(),
                        args: Some(vec!["value".to_string(), "count".to_string()]),
                        out: Some("v0".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(
            output.contains("math.max(0, count)"),
            "list repetition must clamp negative counts to empty list, got:\n{output}"
        );
    }

    #[test]
    fn test_string_startswith_endswith_honor_start_end_bounds() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "string_prefix_suffix_bounds".to_string(),
                params: vec![
                    "s".to_string(),
                    "prefix".to_string(),
                    "suffix".to_string(),
                    "start".to_string(),
                    "end_idx".to_string(),
                ],
                param_types: Some(vec![
                    "str".to_string(),
                    "str".to_string(),
                    "str".to_string(),
                    "int".to_string(),
                    "int".to_string(),
                ]),
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "string_startswith".to_string(),
                        args: Some(vec![
                            "s".to_string(),
                            "prefix".to_string(),
                            "start".to_string(),
                            "end_idx".to_string(),
                        ]),
                        out: Some("v0".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "string_endswith".to_string(),
                        args: Some(vec![
                            "s".to_string(),
                            "suffix".to_string(),
                            "start".to_string(),
                            "end_idx".to_string(),
                        ]),
                        out: Some("v1".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(
            output.contains("__start")
                && output.contains("__end")
                && output.contains("string.sub(s, __start + 1, __end)"),
            "startswith/endswith must normalize start/end bounds, got:\n{output}"
        );
    }

    #[test]
    fn test_string_slice_opcode_aliases_use_range_lowering() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "string_slice_opcode_aliases".to_string(),
                params: vec![
                    "s".to_string(),
                    "needle".to_string(),
                    "start".to_string(),
                    "end_idx".to_string(),
                ],
                param_types: Some(vec![
                    "str".to_string(),
                    "str".to_string(),
                    "int".to_string(),
                    "int".to_string(),
                ]),
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "string_find_slice".to_string(),
                        args: Some(vec![
                            "s".to_string(),
                            "needle".to_string(),
                            "start".to_string(),
                            "end_idx".to_string(),
                        ]),
                        out: Some("v0".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "string_startswith_slice".to_string(),
                        args: Some(vec![
                            "s".to_string(),
                            "needle".to_string(),
                            "start".to_string(),
                            "end_idx".to_string(),
                        ]),
                        out: Some("v1".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "string_endswith_slice".to_string(),
                        args: Some(vec![
                            "s".to_string(),
                            "needle".to_string(),
                            "start".to_string(),
                            "end_idx".to_string(),
                        ]),
                        out: Some("v2".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(
            output.contains("__start_raw")
                && !output.contains("[unsupported op: string_find_slice]")
                && !output.contains("[unsupported op: string_startswith_slice]")
                && !output.contains("[unsupported op: string_endswith_slice]"),
            "slice op aliases must use range-aware string lowering, got:\n{output}"
        );
    }

    #[test]
    fn test_string_find_honors_start_end_bounds() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "string_find_bounds".to_string(),
                params: vec![
                    "s".to_string(),
                    "needle".to_string(),
                    "start".to_string(),
                    "end_idx".to_string(),
                ],
                param_types: Some(vec![
                    "str".to_string(),
                    "str".to_string(),
                    "int".to_string(),
                    "int".to_string(),
                ]),
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "string_find".to_string(),
                        args: Some(vec![
                            "s".to_string(),
                            "needle".to_string(),
                            "start".to_string(),
                            "end_idx".to_string(),
                        ]),
                        out: Some("v0".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(
            output.contains("__found")
                && output.contains("__start")
                && output.contains("__end")
                && output.contains("if __found and __found <= __end then"),
            "string.find must honor normalized start/end bounds, got:\n{output}"
        );
    }

    #[test]
    fn test_string_startswith_endswith_tuple_prefixes_lower() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "string_tuple_affixes".to_string(),
                params: vec!["s".to_string()],
                param_types: Some(vec!["str".to_string()]),
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "const_str".to_string(),
                        s_value: Some("ba".to_string()),
                        out: Some("v0".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const_str".to_string(),
                        s_value: Some("na".to_string()),
                        out: Some("v1".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "tuple_new".to_string(),
                        args: Some(vec!["v0".to_string(), "v1".to_string()]),
                        out: Some("t0".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "string_startswith".to_string(),
                        args: Some(vec!["s".to_string(), "t0".to_string()]),
                        out: Some("v2".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "string_endswith".to_string(),
                        args: Some(vec!["s".to_string(), "t0".to_string()]),
                        out: Some("v3".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(
            output.contains("for __i = 1, #t0 do")
                && output.contains("type(__cand) ~= \"string\"")
                && !output.contains("[unsupported op: string_startswith]")
                && !output.contains("[unsupported op: string_endswith]"),
            "tuple affix args must lower to candidate loop with type guard, got:\n{output}"
        );
    }

    #[test]
    fn test_string_rfind_honors_start_end_bounds() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "string_rfind_bounds".to_string(),
                params: vec![
                    "s".to_string(),
                    "needle".to_string(),
                    "start".to_string(),
                    "end_idx".to_string(),
                ],
                param_types: Some(vec![
                    "str".to_string(),
                    "str".to_string(),
                    "int".to_string(),
                    "int".to_string(),
                ]),
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "string_rfind_slice".to_string(),
                        args: Some(vec![
                            "s".to_string(),
                            "needle".to_string(),
                            "start".to_string(),
                            "end_idx".to_string(),
                        ]),
                        out: Some("v0".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(
            output.contains("__last")
                && output.contains("__found")
                && !output.contains("[unsupported op: string_rfind_slice]"),
            "string_rfind_slice must lower to bounded reverse find, got:\n{output}"
        );
    }

    #[test]
    fn test_string_index_rindex_raise_value_error_when_missing() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "string_index_rindex_errors".to_string(),
                params: vec![
                    "s".to_string(),
                    "needle".to_string(),
                    "start".to_string(),
                    "end_idx".to_string(),
                ],
                param_types: Some(vec![
                    "str".to_string(),
                    "str".to_string(),
                    "int".to_string(),
                    "int".to_string(),
                ]),
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "string_index_slice".to_string(),
                        args: Some(vec![
                            "s".to_string(),
                            "needle".to_string(),
                            "start".to_string(),
                            "end_idx".to_string(),
                        ]),
                        out: Some("v0".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "string_rindex_slice".to_string(),
                        args: Some(vec![
                            "s".to_string(),
                            "needle".to_string(),
                            "start".to_string(),
                            "end_idx".to_string(),
                        ]),
                        out: Some("v1".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(
            output.contains("__type=\"ValueError\"")
                && output.contains("substring not found")
                && !output.contains("[unsupported op: string_index_slice]")
                && !output.contains("[unsupported op: string_rindex_slice]"),
            "string index/rindex must raise ValueError when missing, got:\n{output}"
        );
    }

    #[test]
    fn test_string_partition_and_rpartition_lower_to_tuple_tables() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "string_partition_ops".to_string(),
                params: vec!["s".to_string(), "sep".to_string()],
                param_types: Some(vec!["str".to_string(), "str".to_string()]),
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "string_partition".to_string(),
                        args: Some(vec!["s".to_string(), "sep".to_string()]),
                        out: Some("v0".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "string_rpartition".to_string(),
                        args: Some(vec!["s".to_string(), "sep".to_string()]),
                        out: Some("v1".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(
            output.contains("empty separator")
                && output.contains("{s, \"\", \"\"}")
                && output.contains("{\"\", \"\", s}")
                && output.contains("string_partition")
                && !output.contains("[unsupported op: string_partition]"),
            "string partition/rpartition must lower to Python tuple tables, got:\n{output}"
        );
    }

    #[test]
    fn test_string_removeprefix_suffix_get_attr_indirect_path() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "string_remove_affix".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "const_str".to_string(),
                        s_value: Some("foobar".to_string()),
                        out: Some("s".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "get_attr_generic_obj".to_string(),
                        args: Some(vec!["s".to_string()]),
                        s_value: Some("removeprefix".to_string()),
                        out: Some("m0".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const_str".to_string(),
                        s_value: Some("foo".to_string()),
                        out: Some("p".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "callargs_new".to_string(),
                        out: Some("a0".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "callargs_push_pos".to_string(),
                        args: Some(vec!["a0".to_string(), "p".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "call_indirect".to_string(),
                        args: Some(vec!["m0".to_string(), "a0".to_string()]),
                        out: Some("v0".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "get_attr_generic_obj".to_string(),
                        args: Some(vec!["s".to_string()]),
                        s_value: Some("removesuffix".to_string()),
                        out: Some("m1".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const_str".to_string(),
                        s_value: Some("bar".to_string()),
                        out: Some("q".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "callargs_new".to_string(),
                        out: Some("a1".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "callargs_push_pos".to_string(),
                        args: Some(vec!["a1".to_string(), "q".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "call_indirect".to_string(),
                        args: Some(vec!["m1".to_string(), "a1".to_string()]),
                        out: Some("v1".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(
            output.contains("function(__args)")
                && output.contains("string.sub(s, 1, #__prefix)")
                && output.contains("string.sub(s, -#__suffix)")
                && !output.contains("s.removeprefix")
                && !output.contains("s.removesuffix"),
            "string remove-prefix/suffix method attrs must lower to callable closures, got:\n{output}"
        );
    }

    #[test]
    fn test_luau_repr_authority_typed_string_get_attr_dispatch() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "typed_string_remove_prefix_attr".to_string(),
                params: vec!["s".to_string()],
                param_types: Some(vec!["str".to_string()]),
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "get_attr_generic_obj".to_string(),
                        args: Some(vec!["s".to_string()]),
                        s_value: Some("removeprefix".to_string()),
                        out: Some("method".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);

        assert!(
            output.contains("function(__args)")
                && output.contains("string.sub(s, 1, #__prefix)")
                && !output.contains("s.removeprefix"),
            "typed str facts should authorize string removeprefix closure lowering, got:\n{output}"
        );
    }

    #[test]
    fn test_string_ascii_predicate_get_attr_indirect_path() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "string_predicate_attrs".to_string(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "const_str".to_string(),
                        s_value: Some("Abc123".to_string()),
                        out: Some("s".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "get_attr_generic_obj".to_string(),
                        args: Some(vec!["s".to_string()]),
                        s_value: Some("isalnum".to_string()),
                        out: Some("m0".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "callargs_new".to_string(),
                        out: Some("a0".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "call_indirect".to_string(),
                        args: Some(vec!["m0".to_string(), "a0".to_string()]),
                        out: Some("v0".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "get_attr_generic_obj".to_string(),
                        args: Some(vec!["s".to_string()]),
                        s_value: Some("isidentifier".to_string()),
                        out: Some("m1".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "callargs_new".to_string(),
                        out: Some("a1".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "call_indirect".to_string(),
                        args: Some(vec!["m1".to_string(), "a1".to_string()]),
                        out: Some("v1".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "get_attr_generic_obj".to_string(),
                        args: Some(vec!["s".to_string()]),
                        s_value: Some("istitle".to_string()),
                        out: Some("m2".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "callargs_new".to_string(),
                        out: Some("a2".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "call_indirect".to_string(),
                        args: Some(vec!["m2".to_string(), "a2".to_string()]),
                        out: Some("v2".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(
            output.contains("function(__args)")
                && output.contains("__has_cased")
                && output.contains("__first_ok")
                && output.contains("__prev_uncased")
                && output.contains("string.byte(__s, __i)")
                && !output.contains("s.isalnum"),
            "string predicate attrs must lower to ASCII-fast closures, got:\n{output}"
        );
    }

    #[test]
    fn test_string_splitlines_lowers_with_keepends_flag() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "string_splitlines_op".to_string(),
                params: vec!["s".to_string(), "keep".to_string()],
                param_types: Some(vec!["str".to_string(), "bool".to_string()]),
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "string_splitlines".to_string(),
                        args: Some(vec!["s".to_string(), "keep".to_string()]),
                        out: Some("v0".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(
            output.contains("__keep")
                && output.contains("\\r")
                && output.contains("\\n")
                && output.contains("__next += 1")
                && output.contains("__line_start"),
            "string_splitlines must lower with CR/LF handling and keepends flag, got:\n{output}"
        );
    }

    #[test]
    fn test_string_empty_needle_edge_cases_are_explicit() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "string_empty_needle_edges".to_string(),
                params: vec![
                    "s".to_string(),
                    "needle".to_string(),
                    "start".to_string(),
                    "end_idx".to_string(),
                ],
                param_types: Some(vec![
                    "str".to_string(),
                    "str".to_string(),
                    "int".to_string(),
                    "int".to_string(),
                ]),
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "string_find".to_string(),
                        args: Some(vec![
                            "s".to_string(),
                            "needle".to_string(),
                            "start".to_string(),
                            "end_idx".to_string(),
                        ]),
                        out: Some("v0".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "string_startswith".to_string(),
                        args: Some(vec![
                            "s".to_string(),
                            "needle".to_string(),
                            "start".to_string(),
                            "end_idx".to_string(),
                        ]),
                        out: Some("v1".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "string_endswith".to_string(),
                        args: Some(vec![
                            "s".to_string(),
                            "needle".to_string(),
                            "start".to_string(),
                            "end_idx".to_string(),
                        ]),
                        out: Some("v2".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(
            output.contains("needle == \"\"")
                && output.contains("__start_raw")
                && output.contains("__start_raw <= __n"),
            "empty substring cases must be explicit and Python-shaped, got:\n{output}"
        );
    }

    #[test]
    fn test_string_split_rejects_empty_separator() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "string_split_empty_sep".to_string(),
                params: vec!["s".to_string(), "sep".to_string()],
                param_types: Some(vec!["str".to_string(), "str".to_string()]),
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "string_split".to_string(),
                        args: Some(vec!["s".to_string(), "sep".to_string()]),
                        out: Some("v0".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(
            output.contains("__type=\"ValueError\"") && output.contains("empty separator"),
            "str.split must reject empty separator instead of looping, got:\n{output}"
        );
    }

    #[test]
    fn test_string_replace_honors_count_argument() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "string_replace_count".to_string(),
                params: vec![
                    "s".to_string(),
                    "old".to_string(),
                    "new_value".to_string(),
                    "count".to_string(),
                ],
                param_types: Some(vec![
                    "str".to_string(),
                    "str".to_string(),
                    "str".to_string(),
                    "int".to_string(),
                ]),
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "string_replace".to_string(),
                        args: Some(vec![
                            "s".to_string(),
                            "old".to_string(),
                            "new_value".to_string(),
                            "count".to_string(),
                        ]),
                        out: Some("v0".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(
            output.contains("if count >= 0 then")
                && output.contains("__pattern")
                && output.contains("__replacement"),
            "str.replace(old, new, count) must pass bounded count to gsub, got:\n{output}"
        );
    }

    #[test]
    fn test_string_count_and_count_slice_lower_to_nonoverlap_loop() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "string_count_ops".to_string(),
                params: vec![
                    "s".to_string(),
                    "needle".to_string(),
                    "start".to_string(),
                    "end_idx".to_string(),
                ],
                param_types: Some(vec![
                    "str".to_string(),
                    "str".to_string(),
                    "int".to_string(),
                    "int".to_string(),
                ]),
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "string_count".to_string(),
                        args: Some(vec!["s".to_string(), "needle".to_string()]),
                        out: Some("v0".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "string_count_slice".to_string(),
                        args: Some(vec![
                            "s".to_string(),
                            "needle".to_string(),
                            "start".to_string(),
                            "end_idx".to_string(),
                        ]),
                        out: Some("v1".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(
            output.contains("__sub == \"\"")
                && output.contains("__count += 1")
                && output.contains("__pos = __j + 1"),
            "string_count ops must use Python non-overlapping count loop, got:\n{output}"
        );
    }

    #[test]
    fn test_lower_try_to_pcall_basic() {
        let ops = vec![
            OpIR {
                kind: "try_start".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_int".into(),
                value: Some(1),
                out: Some("v0".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "try_end".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "exception_last".into(),
                out: Some("v1".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "try_end".into(),
                ..OpIR::default()
            },
        ];
        let (lowered, _) = lower_try_to_pcall(&ops);
        assert!(lowered.iter().any(|op| op.kind == "pcall_wrap_begin"));
        assert!(lowered.iter().any(|op| op.kind == "pcall_wrap_end"));
        assert!(!lowered.iter().any(|op| op.kind == "try_start"));
    }

    #[test]
    fn test_lower_try_to_pcall_targets_protected_exception_handler() {
        let ops = vec![
            OpIR {
                kind: "try_start".into(),
                value: Some(5),
                ..OpIR::default()
            },
            OpIR {
                kind: "exception_new_builtin_one".into(),
                args: Some(vec!["arg".into()]),
                out: Some("exc".into()),
                s_value: Some("ValueError".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "raise".into(),
                args: Some(vec!["exc".into()]),
                out: Some("none".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "jump".into(),
                value: Some(2),
                ..OpIR::default()
            },
            OpIR {
                kind: "try_end".into(),
                value: Some(5),
                ..OpIR::default()
            },
            OpIR {
                kind: "jump".into(),
                value: Some(3),
                ..OpIR::default()
            },
            OpIR {
                kind: "label".into(),
                value: Some(2),
                ..OpIR::default()
            },
            OpIR {
                kind: "exception_last_pending".into(),
                out: Some("caught".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "try_end".into(),
                ..OpIR::default()
            },
        ];
        let (lowered, _) = lower_try_to_pcall(&ops);
        let failure = lowered
            .iter()
            .find(|op| op.kind == "pcall_failure_jump")
            .expect("pcall lowering should emit a failure jump");
        assert_eq!(failure.value, Some(2));
        let handler_last = lowered
            .iter()
            .find(|op| op.kind == "exception_last_pending")
            .expect("handler should keep exception_last_pending");
        assert_eq!(handler_last.value, Some(0));
        let begin_idx = lowered
            .iter()
            .position(|op| op.kind == "pcall_wrap_begin")
            .expect("pcall begin should be emitted");
        let end_idx = lowered
            .iter()
            .position(|op| op.kind == "pcall_wrap_end")
            .expect("pcall end should be emitted");
        assert!(
            !lowered[begin_idx..end_idx]
                .iter()
                .any(|op| op.kind == "jump" && op.value == Some(2)),
            "raise's static handler jump must be consumed by pcall_failure_jump: {lowered:?}"
        );
    }

    fn luau_tir_roundtrip_function(mut func: FunctionIR) -> FunctionIR {
        if func.ops.iter().any(|op| op.kind == "phi") {
            crate::rewrite_phi_to_store_load(&mut func.ops);
        }
        let mut tir_func = crate::tir::lower_from_simple::lower_to_tir(&func);
        crate::tir::type_refine::refine_types(&mut tir_func);
        let target_info = crate::tir::target_info::TargetInfo::luau_release_fast();
        let _stats = crate::tir::passes::run_pipeline(&mut tir_func, &target_info);
        let _drop_changed =
            crate::tir::drop_phase::finalize_function_drops(&mut tir_func, &target_info);
        crate::tir::type_refine::refine_types(&mut tir_func);
        func.ops = crate::tir::lower_to_simple::lower_to_simple_ir(&tir_func);
        func
    }

    #[test]
    fn test_luau_tir_roundtrip_raise_catch_closes_pcall_before_handler() {
        let func: FunctionIR = serde_json::from_str(
            r#"{"name":"__main____raise_catch","ops":[{"kind":"trace_enter_slot","value":1},{"kind":"exception_stack_enter","out":"v107"},{"kind":"exception_stack_depth","out":"v108"},{"kind":"missing","out":"v109"},{"args":["v109"],"kind":"store_var","var":"caught"},{"kind":"check_exception","value":3},{"kind":"missing","out":"v110"},{"args":["v110"],"kind":"store_var","var":"i"},{"kind":"check_exception","value":3},{"args":["n"],"col_offset":4,"end_col_offset":14,"kind":"store_var","var":"n"},{"col_offset":4,"end_col_offset":14,"kind":"line","value":36},{"kind":"check_exception","value":3},{"kind":"const","out":"v111","value":0},{"args":["v111"],"col_offset":4,"end_col_offset":23,"kind":"store_var","var":"caught"},{"col_offset":4,"end_col_offset":23,"kind":"line","value":37},{"kind":"check_exception","value":3},{"kind":"const","out":"v112","value":0},{"kind":"const","out":"v113","value":1},{"args":["v112","n","v113"],"kind":"range_new","out":"v114"},{"kind":"check_exception","value":3},{"kind":"const","out":"v115","value":0},{"kind":"const","out":"v116","value":1},{"args":["v114"],"kind":"len","out":"v117"},{"kind":"check_exception","value":3},{"kind":"loop_start"},{"args":["v115"],"kind":"loop_index_start","out":"v118"},{"args":["v118","v117"],"fast_int":true,"kind":"lt","out":"v119"},{"kind":"check_exception","value":3},{"args":["v119"],"kind":"loop_break_if_false","type_hint":"bool"},{"args":["v114","v118"],"kind":"index","out":"v120"},{"kind":"check_exception","value":3},{"args":["v120"],"col_offset":8,"end_col_offset":23,"kind":"store_var","var":"i"},{"col_offset":8,"end_col_offset":23,"kind":"line","value":38},{"kind":"check_exception","value":3},{"kind":"exception_push","out":"none"},{"col_offset":12,"end_col_offset":31,"kind":"try_start","value":4},{"col_offset":12,"end_col_offset":31,"kind":"line","value":39},{"kind":"load_var","out":"v121","var":"i"},{"kind":"check_exception","value":4},{"args":["v121"],"kind":"exception_new_builtin_one","out":"v122","s_value":"ValueError","value":5},{"args":["v122"],"kind":"raise","out":"none"},{"kind":"jump","value":4},{"kind":"try_end","value":4},{"kind":"jump","value":6},{"kind":"label","value":4},{"kind":"exception_last_pending","out":"v123"},{"kind":"exception_clear","out":"none"},{"args":["v123"],"kind":"exception_match_builtin","out":"v124","s_value":"ValueError","value":5},{"args":["v124"],"kind":"if","type_hint":"bool"},{"kind":"exception_clear","out":"none"},{"args":["v123"],"col_offset":12,"end_col_offset":23,"kind":"exception_context_set","out":"none"},{"col_offset":12,"end_col_offset":23,"kind":"line","value":41},{"kind":"load_var","out":"v125","var":"caught"},{"kind":"const","out":"v126","value":1},{"args":["v125","v126"],"fast_int":true,"kind":"inplace_add","out":"v127"},{"args":["v127"],"kind":"store_var","var":"caught"},{"kind":"const_none","out":"v128"},{"args":["v128"],"kind":"exception_context_set","out":"none"},{"kind":"else"},{"args":["v123"],"kind":"raise","out":"none"},{"kind":"end_if"},{"kind":"jump","value":7},{"kind":"label","value":6},{"kind":"exception_pop","out":"none"},{"kind":"jump","value":8},{"kind":"label","value":7},{"kind":"exception_pop","out":"none"},{"kind":"check_exception","value":3},{"kind":"label","value":8},{"kind":"check_exception","value":3},{"args":["v118","v116"],"fast_int":true,"kind":"add","out":"v129"},{"kind":"check_exception","value":3},{"args":["v129"],"kind":"loop_index_next","out":"v118"},{"kind":"loop_continue"},{"col_offset":4,"end_col_offset":17,"kind":"loop_end"},{"col_offset":4,"end_col_offset":17,"kind":"line","value":42},{"kind":"load_var","out":"v130","var":"caught"},{"kind":"check_exception","value":3},{"args":["v108"],"kind":"exception_stack_set_depth","out":"none"},{"kind":"check_exception","value":3},{"args":["v108"],"kind":"exception_stack_set_depth","out":"none"},{"args":["v107"],"kind":"exception_stack_exit","out":"none"},{"kind":"trace_exit"},{"kind":"trace_exit"},{"kind":"ret","var":"v130"},{"kind":"label","value":3},{"args":["v108"],"kind":"exception_stack_set_depth","out":"none"},{"args":["v107"],"kind":"exception_stack_exit","out":"none"},{"kind":"trace_exit"},{"kind":"trace_exit"},{"kind":"ret_void"}],"param_types":["i64"],"params":["n"]}"#,
        )
        .expect("raise_catch frontend fixture should deserialize");
        let func = luau_tir_roundtrip_function(func);
        let mut backend = LuauBackend::new();
        let source = backend
            .compile_checked(&SimpleIR {
                functions: vec![func],
                profile: None,
            })
            .expect("TIR-roundtripped raise/catch should lower to Luau");
        let pcall_start = source
            .find("pcall(function()")
            .expect("pcall wrapper should be emitted");
        let after_pcall = &source[pcall_start..];
        let pcall_end = after_pcall.find("end)").unwrap_or_else(|| {
            panic!("pcall wrapper must close before handler dispatch:\n{source}")
        });
        let failure_dispatch = after_pcall
            .find("__err_0")
            .expect("handler dispatch should consume the pcall error value");
        assert!(
            pcall_end < failure_dispatch,
            "handler dispatch must remain outside the protected pcall body:\n{source}"
        );
    }

    #[test]
    fn test_compile_checked_structures_raise_catch_pcall_boundary() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "raise_catch_boundary_test".into(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "exception_push".into(),
                        out: Some("none".into()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "try_start".into(),
                        value: Some(5),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "exception_new_builtin_one".into(),
                        args: Some(vec!["arg".into()]),
                        out: Some("exc".into()),
                        s_value: Some("ValueError".into()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "raise".into(),
                        args: Some(vec!["exc".into()]),
                        out: Some("none".into()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "jump".into(),
                        value: Some(2),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "try_end".into(),
                        value: Some(5),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "jump".into(),
                        value: Some(3),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "label".into(),
                        value: Some(2),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "exception_last_pending".into(),
                        out: Some("caught".into()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "exception_clear".into(),
                        out: Some("none".into()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "exception_match_builtin".into(),
                        args: Some(vec!["caught".into()]),
                        out: Some("matched".into()),
                        s_value: Some("ValueError".into()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "if".into(),
                        args: Some(vec!["matched".into()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const".into(),
                        value: Some(1),
                        out: Some("handled".into()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "else".into(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "raise".into(),
                        args: Some(vec!["caught".into()]),
                        out: Some("none".into()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "end_if".into(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "try_end".into(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".into(),
                        var: Some("handled".into()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "label".into(),
                        value: Some(3),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "exception_pop".into(),
                        out: Some("none".into()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".into(),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let source = backend
            .compile_checked(&ir)
            .expect("raise/catch pcall boundary should lower to valid Luau");
        let pcall_start = source
            .find("pcall(function()")
            .expect("pcall wrapper should be emitted");
        let pcall_end = source[pcall_start..]
            .find("end)")
            .map(|offset| pcall_start + offset)
            .expect("pcall wrapper should be closed before handler dispatch");
        let handler_read = source
            .find("caught = __err_0")
            .expect("handler should read the pcall error value");
        assert!(
            pcall_start < pcall_end && pcall_end < handler_read,
            "handler must be outside pcall body, got:\n{source}"
        );
    }

    #[test]
    fn test_lower_try_to_pcall_escape_detection() {
        let ops = vec![
            OpIR {
                kind: "try_start".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_int".into(),
                value: Some(42),
                out: Some("v0".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "try_end".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "call_function".into(),
                args: Some(vec!["print".into(), "v0".into()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "try_end".into(),
                ..OpIR::default()
            },
        ];
        let (_, escaped) = lower_try_to_pcall(&ops);
        assert!(
            escaped.contains("v0"),
            "v0 should escape pcall scope: {:?}",
            escaped
        );
    }

    #[test]
    fn test_luau_exception_region_block_args_hoist_before_protected_op() {
        let ops = vec![
            OpIR {
                kind: "call".into(),
                out: Some("v_call".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".into(),
                var: Some("_bb1_arg0".into()),
                args: Some(vec!["module_obj".into()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".into(),
                value: Some(5),
                ..OpIR::default()
            },
        ];

        let hoisted = hoist_exception_edge_block_arg_stores(&ops);

        assert_eq!(hoisted[0].kind, "store_var");
        assert_eq!(hoisted[1].kind, "call");
        assert_eq!(hoisted[2].kind, "check_exception");
    }

    #[test]
    fn test_luau_exception_region_block_args_hoist_before_raise_edge() {
        let ops = vec![
            OpIR {
                kind: "raise".into(),
                args: Some(vec!["exc".into()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".into(),
                var: Some("_bb5_arg0".into()),
                args: Some(vec!["caught".into()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".into(),
                var: Some("_bb5_arg1".into()),
                args: Some(vec!["limit".into()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "jump".into(),
                value: Some(5),
                ..OpIR::default()
            },
        ];

        let hoisted = hoist_exception_edge_block_arg_stores(&ops);

        assert_eq!(hoisted[0].kind, "store_var");
        assert_eq!(hoisted[0].var.as_deref(), Some("_bb5_arg0"));
        assert_eq!(hoisted[1].kind, "store_var");
        assert_eq!(hoisted[1].var.as_deref(), Some("_bb5_arg1"));
        assert_eq!(hoisted[2].kind, "raise");
        assert_eq!(hoisted[3].kind, "jump");
    }

    #[test]
    fn test_luau_exception_region_block_args_do_not_hoist_dependent_result() {
        let ops = vec![
            OpIR {
                kind: "call".into(),
                out: Some("v_call".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".into(),
                var: Some("_bb1_arg0".into()),
                args: Some(vec!["v_call".into()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".into(),
                value: Some(5),
                ..OpIR::default()
            },
        ];

        let hoisted = hoist_exception_edge_block_arg_stores(&ops);

        assert_eq!(hoisted[0].kind, "call");
        assert_eq!(hoisted[1].kind, "store_var");
        assert_eq!(hoisted[2].kind, "check_exception");
    }

    #[test]
    fn test_luau_exception_region_module_global_ops_use_module_dict_helpers() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "module_global_test".into(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "const_str".into(),
                        out: Some("name".into()),
                        s_value: Some("exc".into()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "dict_new".into(),
                        out: Some("module".into()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "module_get_global".into(),
                        args: Some(vec!["module".into(), "name".into()]),
                        out: Some("value".into()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "module_del_global_if_present".into(),
                        args: Some(vec!["module".into(), "name".into()]),
                        out: Some("none".into()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".into(),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);

        assert!(
            output.contains("molt_module_get_global(module, name)"),
            "module_get_global must read the supplied module dict:\n{output}"
        );
        assert!(
            output.contains("molt_module_del_global(module, name, true)"),
            "module_del_global_if_present must delete from the supplied module dict:\n{output}"
        );
        assert!(
            !output.contains("local value = molt_module_cache[name]"),
            "module_get_global must not read import cache directly:\n{output}"
        );
    }

    #[test]
    fn test_luau_exception_region_type_of_uses_python_descriptor_helper() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "type_descriptor_test".into(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "exception_new_builtin_empty".into(),
                        out: Some("exc".into()),
                        s_value: Some("NameError".into()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "type_of".into(),
                        args: Some(vec!["exc".into()]),
                        out: Some("typ".into()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "get_attr_generic_obj".into(),
                        args: Some(vec!["typ".into()]),
                        out: Some("name".into()),
                        s_value: Some("__name__".into()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".into(),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);

        assert!(
            output.contains("local typ = molt_type_of(exc)")
                && output.contains("if type(x) == \"table\" and x.__type then"),
            "type_of must preserve Python exception class identity:\n{output}"
        );
    }

    #[test]
    fn test_pcall_try_except_compile() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "try_except_test".into(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "try_start".into(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const_int".into(),
                        value: Some(1),
                        out: Some("v0".into()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const_int".into(),
                        value: Some(0),
                        out: Some("v1".into()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "binary_op".into(),
                        s_value: Some("/".into()),
                        args: Some(vec!["v0".into(), "v1".into()]),
                        out: Some("v2".into()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "try_end".into(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "exception_last".into(),
                        out: Some("v3".into()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const_int".into(),
                        value: Some(42),
                        out: Some("v4".into()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "try_end".into(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "call_function".into(),
                        s_value: Some("print".into()),
                        args: Some(vec!["print".into(), "v4".into()]),
                        out: Some("v5".into()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".into(),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(
            output.contains("pcall(function()"),
            "Expected pcall wrapper, got:\n{output}"
        );
        assert!(
            output.contains("__ok_0") && output.contains("__err_0"),
            "Expected __ok_0/__err_0, got:\n{output}"
        );
        assert!(
            !output.contains("= nil -- [exception_last]"),
            "exception_last should NOT emit nil inside pcall, got:\n{output}"
        );
    }

    #[test]
    fn test_no_duplicate_local_declarations() {
        // When the same variable name appears as `out` in multiple ops,
        // only the first should emit `local`.  Subsequent uses should be
        // plain assignment to avoid Luau syntax errors.
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "dup_local_test".into(),
                params: vec![],
                param_types: None,
                source_file: None,
                is_extern: false,
                ops: vec![
                    // First definition of v0 — should get `local v0 = 1`
                    OpIR {
                        kind: "const_int".into(),
                        value: Some(1),
                        out: Some("v0".into()),
                        ..OpIR::default()
                    },
                    // Second definition of v0 — must NOT emit `local` again
                    OpIR {
                        kind: "const_int".into(),
                        value: Some(2),
                        out: Some("v0".into()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "call_function".into(),
                        s_value: Some("print".into()),
                        args: Some(vec!["print".into(), "v0".into()]),
                        out: Some("v1".into()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".into(),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        // Count occurrences of `local v0` — should be exactly 1.
        let local_v0_count = output.matches("local v0").count();
        assert_eq!(
            local_v0_count, 1,
            "Expected exactly 1 `local v0`, found {local_v0_count} in:\n{output}"
        );
    }

    #[test]
    fn test_lower_try_to_pcall_nested() {
        let ops = vec![
            OpIR {
                kind: "try_start".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "try_start".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "const_int".into(),
                value: Some(1),
                out: Some("v0".into()),
                ..OpIR::default()
            },
            OpIR {
                kind: "try_end".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "try_end".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "try_end".into(),
                ..OpIR::default()
            },
            OpIR {
                kind: "try_end".into(),
                ..OpIR::default()
            },
        ];
        let (lowered, _) = lower_try_to_pcall(&ops);
        let begin_count = lowered
            .iter()
            .filter(|op| op.kind == "pcall_wrap_begin")
            .count();
        let end_count = lowered
            .iter()
            .filter(|op| op.kind == "pcall_wrap_end")
            .count();
        assert_eq!(begin_count, 2, "should have 2 pcall_wrap_begin");
        assert_eq!(end_count, 2, "should have 2 pcall_wrap_end");
    }
}
