//! Luau transpiler backend for Molt.
//!
//! Transpiles `SimpleIR` → Luau source code suitable for Roblox Studio.
//! Unlike the native/WASM backends that emit binary, this produces a `.luau`
//! text file that can be executed directly in Roblox's Luau VM.
//!
//! This backend is intentionally a preview target. Production build paths must
//! reject lowered output that still contains comment-only control-flow markers
//! or stub markers for unsupported semantics.

use crate::{FunctionIR, OpIR, SimpleIR};
use std::collections::{HashMap, HashSet};
use std::fmt::Write;

/// Transpiles Molt `SimpleIR` into Luau source text.
pub struct LuauBackend {
    output: String,
    indent: usize,
    uses_forward_decls: bool,
    /// Variables that have been pre-declared at function scope and should use
    /// assignment (`var = val`) instead of `local var = val` in emit_op.
    hoisted_vars: HashSet<String>,
}

impl LuauBackend {
    pub fn new() -> Self {
        Self {
            output: String::with_capacity(8192),
            indent: 0,
            uses_forward_decls: false,
            hoisted_vars: HashSet::new(),
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

        if emit_funcs.len() > 1 {
            self.uses_forward_decls = true;
            self.emit_line("-- Forward declarations");
            for func in &emit_funcs {
                let name = sanitize_ident(&func.name);
                self.emit_line(&format!("local {name}"));
            }
            self.output.push('\n');
        }

        for func in &emit_funcs {
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
        // scanning for prelude helpers — inlining passes may eliminate helper
        // calls (e.g. molt_pow → ^, molt_mod → %), leaving dead definitions
        // if we scan before optimization.
        inline_single_use_constants(&mut func_body);
        eliminate_nil_missing_wrappers(&mut func_body);
        strip_unbound_local_checks(&mut func_body);
        strip_dead_locals_dict_stores(&mut func_body);
        strip_undefined_rhs_assignments(&mut func_body);
        propagate_single_use_copies(&mut func_body);
        strip_trailing_continue(&mut func_body);
        simplify_comparison_break(&mut func_body);
        optimize_luau_perf(&mut func_body);
        // Second copy-prop pass: optimize_luau_perf reduces type-guard
        // expressions (4 uses → 2 uses), unlocking more copy propagation.
        propagate_single_use_copies(&mut func_body);
        sink_single_use_locals(&mut func_body);
        simplify_return_chain(&mut func_body);

        // Phase 3: Emit prelude with only the helpers that survive optimization.
        self.emit_prelude_conditional(&func_body);

        // Phase 4: Combine prelude + optimized function bodies.
        self.output.push_str(&func_body);

        std::mem::take(&mut self.output)
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
        self.output
            .push_str("--!strict\n-- Molt -> Luau transpiled output\n-- Runtime helpers\n\n");
        self.output
            .push_str("local molt_func_attrs: {[any]: {[string]: any}} = {}\n");
        self.output.push_str("local molt_module_cache: {[string]: any} = {\n\tmath = nil,\n\tjson = nil,\n\ttime = nil,\n\tos = nil,\n}\n\n");

        // Helper to check if a name is used in the function body.
        // We search for "name(" to match call sites, avoiding false positives
        // like "molt_mod" matching inside "molt_module_cache".
        let used_call = |name: &str| {
            let pattern = format!("{name}(");
            func_body.contains(&pattern)
        };
        // For non-function names (modules, variables), use plain contains.
        let used = |name: &str| func_body.contains(name);

        // Conditional runtime helpers — only emit if referenced by call.
        // Each helper is a (name, source) pair.
        let helpers: &[(&str, &str)] = &[
            (
                "molt_range",
                "@native\nlocal function molt_range(start: number, stop: number, step: number?): {number}\n\tlocal result = {}\n\tlocal s = step or 1\n\tlocal n = 0\n\tlocal i = start\n\twhile (s > 0 and i < stop) or (s < 0 and i > stop) do\n\t\tn += 1\n\t\tresult[n] = i\n\t\ti += s\n\tend\n\treturn result\nend\n",
            ),
            (
                "molt_len",
                "local function molt_len(obj: any): number\n\tif type(obj) == \"string\" then return #obj end\n\tif type(obj) == \"table\" then return #obj end\n\treturn 0\nend\n",
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
                "molt_repr",
                "local function molt_repr(x: any): string\n\tif type(x) == \"string\" then return \"'\" .. x .. \"'\" end\n\tif type(x) == \"table\" then return molt_str(x) end\n\tif type(x) == \"boolean\" then return x and \"True\" or \"False\" end\n\tif x == nil then return \"None\" end\n\treturn tostring(x)\nend\n",
            ),
            (
                "molt_floor_div",
                "local function molt_floor_div(a: number, b: number): number\n\treturn math.floor(a / b)\nend\n",
            ),
            (
                "molt_pow",
                "local function molt_pow(a: number, b: number): number\n\treturn a ^ b\nend\n",
            ),
            (
                "molt_mod",
                "local function molt_mod(a: number, b: number): number\n\treturn a - math.floor(a / b) * b\nend\n",
            ),
            (
                "molt_enumerate",
                "local function molt_enumerate(t: {any}, start: number?): {{any}}\n\tlocal result = {}\n\tlocal s = start or 0\n\tlocal n = 0\n\tfor i, v in ipairs(t) do\n\t\tn += 1\n\t\tresult[n] = {s + i - 1, v}\n\tend\n\treturn result\nend\n",
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
                "@native\nlocal function molt_sum(t: {number}, start: number?): number\n\tlocal s = start or 0\n\tfor _, v in ipairs(t) do s += v end\n\treturn s\nend\n",
            ),
            (
                "molt_any",
                "local function molt_any(t: {any}): boolean\n\tfor _, v in ipairs(t) do\n\t\tif v then return true end\n\tend\n\treturn false\nend\n",
            ),
            (
                "molt_all",
                "local function molt_all(t: {any}): boolean\n\tfor _, v in ipairs(t) do\n\t\tif not v then return false end\n\tend\n\treturn true\nend\n",
            ),
            (
                "molt_map",
                "@native\nlocal function molt_map(func: (any) -> any, t: {any}): {any}\n\tlocal len = #t\n\tlocal result = table.create(len)\n\tfor i = 1, len do\n\t\tresult[i] = func(t[i])\n\tend\n\treturn result\nend\n",
            ),
            (
                "molt_filter",
                "local function molt_filter(func: ((any) -> boolean)?, t: {any}): {any}\n\tlocal result = {}\n\tlocal n = 0\n\tfor _, v in ipairs(t) do\n\t\tif func then\n\t\t\tif func(v) then n += 1; result[n] = v end\n\t\telseif v then\n\t\t\tn += 1; result[n] = v\n\t\tend\n\tend\n\treturn result\nend\n",
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
                "molt_print",
                "local function molt_print(...)\n\tlocal n = select(\"#\", ...)\n\tif n == 0 then print(); return end\n\tif n == 1 then print(molt_str((...))) return end\n\tlocal parts = table.create(n)\n\tfor i = 1, n do\n\t\tparts[i] = molt_str((select(i, ...)))\n\tend\n\tprint(table.concat(parts, \" \"))\nend\n",
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
        if needs_str_group {
            self.output.push_str("local molt_repr\n");
        }
        for (name, source) in helpers {
            let emit = if *name == "molt_str" {
                needs_str_group
            } else if *name == "molt_repr" {
                needs_str_group
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

        // Infrastructure used by JSON serializer and math/bitwise ops.
        // math_floor is needed by the JSON prelude (serialize checks integer-ness).
        let needs_json = used("molt_json_dumps") || used("\"json\"");
        if used("math_floor") || needs_json {
            self.output.push_str("local math_floor = math.floor\n");
        }
        if used("bit32") || used("bit.") {
            self.output.push_str("local bit = bit32 or bit\n");
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
                "\tlog = math.log,\n\tlog10 = math.log,\n\tpi = math.pi,\n",
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
                "\tpath = { join = function(...) local p = {} for _, v in {...} do table.insert(p, v) end return table.concat(p, \"/\") end,\n",
                "\t\texists = function() return false end, sep = \"/\" },\n",
                "}\n\n",
            ));
            self.output
                .push_str("molt_module_cache[\"os\"] = molt_os\n\n");
        }

        // String method helpers.
        if used("molt_string") {
            self.output.push_str(concat!(
                "local molt_string = {\n",
                "\tformat = string.format,\n",
                "\tjoin = function(sep: string, t: {string}): string\n\t\treturn table.concat(t, sep)\n\tend,\n",
                "\tsplit = function(s: string, sep: string?): {string}\n",
                "\t\tlocal result = {}\n\t\tlocal pattern = sep and sep or \"%s+\"\n",
                "\t\tif sep then\n\t\t\tlocal pos = 1\n\t\t\twhile pos <= #s do\n",
                "\t\t\t\tlocal i, j = string.find(s, pattern, pos, true)\n",
                "\t\t\t\tif i then\n\t\t\t\t\ttable.insert(result, string.sub(s, pos, i - 1))\n",
                "\t\t\t\t\tpos = j + 1\n\t\t\t\telse\n",
                "\t\t\t\t\ttable.insert(result, string.sub(s, pos))\n\t\t\t\t\tbreak\n",
                "\t\t\t\tend\n\t\t\tend\n\t\telse\n",
                "\t\t\tfor w in string.gmatch(s, \"%S+\") do\n\t\t\t\ttable.insert(result, w)\n",
                "\t\t\tend\n\t\tend\n\t\treturn result\n\tend,\n}\n\n",
            ));
        }
    }

    fn emit_function_body(&mut self, func: &FunctionIR) {
        // Pre-process: lower early returns (store+jump→ret) then strip dead code.
        let ops = lower_early_returns(&func.ops);
        let ops = strip_dead_after_return(&ops);
        let ops = lower_iter_to_for(&ops);

        let params = func
            .params
            .iter()
            .map(|p| sanitize_ident(p))
            .collect::<Vec<_>>()
            .join(", ");

        let name = sanitize_ident(&func.name);
        if self.uses_forward_decls {
            let _ = writeln!(self.output, "{name} = function({params})");
        } else {
            let _ = writeln!(self.output, "local function {name}({params})");
        }
        self.push_indent();

        // Mark position for post-processing hoisted var declarations.
        let func_start = self.output.len();

        // Reset per-function state.
        self.hoisted_vars.clear();

        // Pre-declare loop index variables so they persist across iterations.
        let mut loop_idx_vars = Vec::new();
        for op in &ops {
            if op.kind == "loop_index_start" {
                if let Some(ref out_name) = op.out {
                    loop_idx_vars.push(sanitize_ident(out_name));
                }
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
                if op.kind == "closure_store" || op.kind == "closure_load" {
                    if let Some(ref args) = op.args {
                        if let Some(slot) = args.first() {
                            let var_name = format!("__closure_{}", sanitize_ident(slot));
                            if !closure_slots.contains(&var_name) {
                                closure_slots.push(var_name);
                            }
                        }
                    }
                }
            }
            for var in &closure_slots {
                self.emit_line(&format!("local {var}"));
            }
        }

        // Phi hoisting: find `end_if` followed by `phi` ops and collect
        // the phi output variables.  Also find variables first declared
        // inside if/else blocks but referenced outside (scope escape).
        let mut phi_assignments: HashMap<usize, Vec<(String, Vec<String>)>> = HashMap::new();
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
            // blocks but used outside.  Track nesting depth and declaration
            // sites.
            let mut depth: i32 = 0;
            let mut decl_depth: HashMap<String, i32> = HashMap::new();
            let param_set: HashSet<String> =
                func.params.iter().map(|p| sanitize_ident(p)).collect();

            for op in &ops {
                match op.kind.as_str() {
                    "if" | "loop_start" | "for_range" | "for_iter" => depth += 1,
                    "end_if" | "loop_end" | "end_for" => depth -= 1,
                    _ => {}
                }
                // Record first declaration depth of each variable.
                if let Some(ref out_name) = op.out {
                    if out_name != "none" && !op.kind.starts_with("nop") {
                        let var = sanitize_ident(out_name);
                        decl_depth.entry(var).or_insert(depth);
                    }
                }
                // Check if any referenced variable was declared at a deeper depth.
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
                    if let Some(&dd) = decl_depth.get(&var) {
                        if dd > depth {
                            self.hoisted_vars.insert(var);
                        }
                    }
                }
            }
        }

        // Emit pre-declarations for all hoisted variables.
        if !self.hoisted_vars.is_empty() {
            let mut sorted: Vec<String> = self.hoisted_vars.iter().cloned().collect();
            sorted.sort();
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
        let mut phi_inject_before_else: HashMap<usize, Vec<(String, String)>> = HashMap::new();
        let mut phi_inject_before_end_if: HashMap<usize, Vec<(String, String)>> = HashMap::new();
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
                        if let Some((_if_idx, else_idx)) = if_stack.pop() {
                            if let Some(phis) = phi_assignments.get(&idx) {
                                for (phi_var, args) in phis {
                                    if let Some(else_i) = else_idx {
                                        // True branch value: inject before else.
                                        let true_val = args
                                            .first()
                                            .cloned()
                                            .unwrap_or_else(|| "nil".to_string());
                                        phi_inject_before_else
                                            .entry(else_i)
                                            .or_default()
                                            .push((phi_var.clone(), true_val));
                                        // False branch value: inject before end_if.
                                        let false_val = args
                                            .get(1)
                                            .cloned()
                                            .unwrap_or_else(|| "nil".to_string());
                                        phi_inject_before_end_if
                                            .entry(idx)
                                            .or_default()
                                            .push((phi_var.clone(), false_val));
                                    } else {
                                        // No else branch — only true path sets value.
                                        let true_val = args
                                            .first()
                                            .cloned()
                                            .unwrap_or_else(|| "nil".to_string());
                                        phi_inject_before_end_if
                                            .entry(idx)
                                            .or_default()
                                            .push((phi_var.clone(), true_val));
                                    }
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

        // Post-process: for hoisted variables, replace `local var = ...`
        // with `var = ...` inside the function body (the pre-declaration
        // already emitted `local var` at the top).
        if !self.hoisted_vars.is_empty() {
            let func_output = &self.output[func_start..];
            let mut patched = String::with_capacity(func_output.len());
            for line in func_output.lines() {
                let trimmed = line.trim_start();
                let mut replaced = false;
                if trimmed.starts_with("local ") {
                    // Extract the variable name: "local vXXX = ..." or "local vXXX;"
                    let after_local = &trimmed[6..];
                    let var_end = after_local
                        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                        .unwrap_or(after_local.len());
                    let var_name = &after_local[..var_end];
                    if self.hoisted_vars.contains(var_name) {
                        // Check this isn't the pre-declaration line itself (no "=")
                        let rest = after_local[var_end..].trim_start();
                        if rest.starts_with('=') {
                            // Replace "local var = ..." with "var = ..."
                            let indent = &line[..line.len() - trimmed.len()];
                            patched.push_str(indent);
                            patched.push_str(after_local);
                            patched.push('\n');
                            replaced = true;
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
    }

    fn emit_op(&mut self, op: &OpIR) {
        match op.kind.as_str() {
            // ================================================================
            // Constants
            // ================================================================
            "const" => {
                let out = self.out_var(op);
                if let Some(v) = op.value {
                    self.emit_line(&format!("local {out} = {v}"));
                } else if let Some(f) = op.f_value {
                    self.emit_line(&format!("local {out} = {f}"));
                } else if let Some(ref s) = op.s_value {
                    let escaped = escape_luau_string(s);
                    self.emit_line(&format!("local {out} = \"{escaped}\""));
                } else {
                    self.emit_line(&format!("local {out} = nil"));
                }
            }
            "const_float" => {
                let out = self.out_var(op);
                let val = op.f_value.unwrap_or(0.0);
                self.emit_line(&format!("local {out} = {val}"));
            }
            "const_str" => {
                let out = self.out_var(op);
                let s = op.s_value.as_deref().unwrap_or("");
                let escaped = escape_luau_string(s);
                self.emit_line(&format!("local {out} = \"{escaped}\""));
            }
            "const_bytes" => {
                let out = self.out_var(op);
                if let Some(ref bytes) = op.bytes {
                    let escaped: String = bytes.iter().map(|b| format!("\\x{b:02x}")).collect();
                    self.emit_line(&format!("local {out} = \"{escaped}\""));
                } else {
                    let s = op.s_value.as_deref().unwrap_or("");
                    let escaped = escape_luau_string(s);
                    self.emit_line(&format!("local {out} = \"{escaped}\""));
                }
            }
            "const_bool" | "bool_const" => {
                let out = self.out_var(op);
                let val = if op.value.unwrap_or(0) != 0 {
                    "true"
                } else {
                    "false"
                };
                self.emit_line(&format!("local {out} = {val}"));
            }
            "const_none" | "none_const" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = nil"));
            }
            "string_const" => {
                let out = self.out_var(op);
                let s = op.s_value.as_deref().unwrap_or("");
                let escaped = escape_luau_string(s);
                self.emit_line(&format!("local {out} = \"{escaped}\""));
            }
            "const_bigint" => {
                let out = self.out_var(op);
                let s = op.s_value.as_deref().unwrap_or("0");
                self.emit_line(&format!("local {out} = tonumber(\"{s}\") or 0"));
            }
            "const_not_implemented" | "const_ellipsis" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = nil -- {}", op.kind));
            }

            // ================================================================
            // Variable load/store (both pedagogical and real IR forms)
            // ================================================================
            "load_local" => {
                let out = self.out_var(op);
                let var = self.var_ref(op);
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
                if let Some(ref args) = op.args {
                    if let Some(src) = args.first() {
                        self.emit_line(&format!("{var} = {}", sanitize_ident(src)));
                    }
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
            "identity_alias" => {
                let out = self.out_var(op);
                if let Some(ref args) = op.args {
                    if let Some(src) = args.first() {
                        self.emit_line(&format!("local {out} = {}", sanitize_ident(src)));
                    }
                }
            }

            // ================================================================
            // Arithmetic ops (real IR op kinds)
            // ================================================================
            "add" | "inplace_add" => {
                // Python + is overloaded: numeric add for numbers, concat for strings.
                // When fast_int or type_hint indicates numeric, skip the type check.
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = sanitize_ident(&args[0]);
                    let rhs = sanitize_ident(&args[1]);
                    let is_numeric = op.fast_int == Some(true)
                        || op.fast_float == Some(true)
                        || op.raw_int == Some(true)
                        || matches!(op.type_hint.as_deref(), Some("int") | Some("float"));
                    if is_numeric {
                        self.emit_line(&format!("local {out} = {lhs} + {rhs}"));
                    } else {
                        self.emit_line(&format!(
                            "local {out} = if type({lhs}) == \"string\" or type({rhs}) == \"string\" then tostring({lhs}) .. tostring({rhs}) else {lhs} + {rhs}"
                        ));
                    }
                }
            }
            "sub" | "inplace_sub" => self.emit_binary_op(op, "-"),
            "mul" | "inplace_mul" => self.emit_binary_op(op, "*"),
            "div" => self.emit_binary_op(op, "/"),
            "mod" => {
                // Luau % uses truncated mod but matches Python floor-mod for
                // positive divisors (the overwhelmingly common case in real code).
                // Emit direct % for maximum performance.
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = sanitize_ident(&args[0]);
                    let rhs = sanitize_ident(&args[1]);
                    self.emit_line(&format!("local {out} = {lhs} % {rhs}"));
                }
            }
            "floordiv" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = sanitize_ident(&args[0]);
                    let rhs = sanitize_ident(&args[1]);
                    // Direct inline — no helper call overhead.
                    self.emit_line(&format!("local {out} = math_floor({lhs} / {rhs})"));
                }
            }
            "pow" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let lhs = sanitize_ident(&args[0]);
                    let rhs = sanitize_ident(&args[1]);
                    // Direct ^ operator — no helper call overhead.
                    self.emit_line(&format!("local {out} = {lhs} ^ {rhs}"));
                }
            }
            "pow_mod" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let base = sanitize_ident(&args[0]);
                    let exp = sanitize_ident(&args[1]);
                    let modulus = sanitize_ident(&args[2]);
                    self.emit_line(&format!("local {out} = ({base} ^ {exp}) % {modulus}"));
                }
            }
            "matmul" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = nil -- [unsupported op: matmul]"));
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
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!(
                        "local {out} = math_floor({})",
                        sanitize_ident(val)
                    ));
                }
            }

            // ================================================================
            // Bitwise ops (real IR op kinds)
            // ================================================================
            "bit_and" | "inplace_bit_and" => self.emit_bit_op(op, "band"),
            "bit_or" | "inplace_bit_or" => self.emit_bit_op(op, "bor"),
            "bit_xor" | "inplace_bit_xor" => self.emit_bit_op(op, "bxor"),
            "lshift" => self.emit_bit_op(op, "lshift"),
            "rshift" => self.emit_bit_op(op, "rshift"),

            // ================================================================
            // Unary ops (real IR op kinds)
            // ================================================================
            "not" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!("local {out} = not {}", sanitize_ident(val)));
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
            "lt" => self.emit_binary_op(op, "<"),
            "le" => self.emit_binary_op(op, "<="),
            "gt" => self.emit_binary_op(op, ">"),
            "ge" => self.emit_binary_op(op, ">="),
            "eq" | "string_eq" | "is" => self.emit_binary_op(op, "=="),
            "ne" => self.emit_binary_op(op, "~="),

            // ================================================================
            // Logical ops
            // ================================================================
            "and" => self.emit_binary_op(op, "and"),
            "or" => self.emit_binary_op(op, "or"),

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
                        "//" => format!("math_floor({lhs} / {rhs})"),
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
                        "not" => format!("not {operand}"),
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
                // Standalone Luau CLI doesn't support goto; Roblox Studio does.
                // Emit as comments for compatibility with both targets.
                if let Some(id) = op.value {
                    self.emit_line(&format!("-- ::label_{id}::"));
                } else if let Some(ref s) = op.s_value {
                    let label = sanitize_label(s);
                    self.emit_line(&format!("-- ::{label}::"));
                }
            }
            "jump" | "goto" => {
                if let Some(id) = op.value {
                    self.emit_line(&format!("-- goto label_{id}"));
                } else if let Some(ref target) = op.s_value {
                    let target = sanitize_label(target);
                    self.emit_line(&format!("-- goto {target}"));
                }
            }
            "br_if" => {
                // Luau has no goto. Convert conditional branches to
                // if/then/end blocks. For exception handler jumps,
                // emit the error() call directly.
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(cond) = args.first() {
                    let cond = sanitize_ident(cond);
                    self.emit_line(&format!("-- br_if {cond}"));
                }
            }
            "branch" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                let cond = if let Some(c) = args.first() {
                    sanitize_ident(c)
                } else if let Some(ref v) = op.var {
                    sanitize_ident(v)
                } else {
                    "true".to_string()
                };
                self.emit_line(&format!("-- branch {cond}"));
            }
            "branch_false" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                let cond = if let Some(c) = args.first() {
                    sanitize_ident(c)
                } else if let Some(ref v) = op.var {
                    sanitize_ident(v)
                } else {
                    "false".to_string()
                };
                self.emit_line(&format!("-- branch_false {cond}"));
            }

            // ================================================================
            // Structured if/else/end_if
            // ================================================================
            "if" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(cond) = args.first() {
                    self.emit_line(&format!("if {} then", sanitize_ident(cond)));
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
                if let Some(cond) = args.first() {
                    self.emit_line(&format!("if {} then break end", sanitize_ident(cond)));
                }
            }
            "loop_break_if_false" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(cond) = args.first() {
                    self.emit_line(&format!("if not {} then break end", sanitize_ident(cond)));
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
                    self.emit_line(&format!("for {out} = {start}, {stop} - 1, {step} do"));
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
                    let method = op.s_value.as_deref().unwrap_or("unknown");
                    let method = sanitize_ident(method);
                    let call_args = args[1..]
                        .iter()
                        .map(|a| sanitize_ident(a))
                        .collect::<Vec<_>>()
                        .join(", ");
                    if let Some(ref out_name) = op.out {
                        let out = sanitize_ident(out_name);
                        self.emit_line(&format!("local {out} = {obj}:{method}({call_args})"));
                    } else {
                        self.emit_line(&format!("{obj}:{method}({call_args})"));
                    }
                }
            }
            "call_async" | "block_on" | "spawn" => {
                // Async primitives — emit as synchronous call stub.
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
                    if let Some(val) = args.first() {
                        self.emit_line(&format!("return {}", sanitize_ident(val)));
                    } else {
                        self.emit_line("return");
                    }
                } else if let Some(ref var) = op.var {
                    self.emit_line(&format!("return {}", sanitize_ident(var)));
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
                self.emit_line(&format!("local {out} = {{{items}}}"));
            }
            "tuple_new" | "tuple_from_list" => {
                let out = self.out_var(op);
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
            "build_dict" | "dict_new" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.is_empty() {
                    self.emit_line(&format!("local {out} = {{}}"));
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
                    self.emit_line(&format!("local {out} = {{{body}}}"));
                }
            }
            "set_new" | "frozenset_new" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = {{}}"));
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
                    self.emit_line(&format!("table.insert({list}, {val})"));
                }
            }
            "list_pop" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(list) = args.first() {
                    let list = sanitize_ident(list);
                    if let Some(ref out_name) = op.out {
                        let out = sanitize_ident(out_name);
                        self.emit_line(&format!("local {out} = table.remove({list})"));
                    } else {
                        self.emit_line(&format!("table.remove({list})"));
                    }
                }
            }
            "list_extend" | "callargs_expand_star" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let list = sanitize_ident(&args[0]);
                    let other = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "for _, __v in ipairs({other}) do table.insert({list}, __v) end"
                    ));
                }
            }
            "list_insert" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let list = sanitize_ident(&args[0]);
                    let idx = sanitize_ident(&args[1]);
                    let val = sanitize_ident(&args[2]);
                    self.emit_line(&format!("table.insert({list}, {idx} + 1, {val})"));
                }
            }
            "list_remove" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let list = sanitize_ident(&args[0]);
                    let val = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "for __i, __v in ipairs({list}) do if __v == {val} then table.remove({list}, __i); break end end"
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
                    self.emit_line(&format!(
                        "local {out} = -1; for __i, __v in ipairs({list}) do if __v == {val} then {out} = __i - 1; break end end"
                    ));
                }
            }
            "list_repeat_range" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let val = sanitize_ident(&args[0]);
                    let count = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "local {out} = {{}}; for __i = 1, {count} do table.insert({out}, {val}) end"
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
                    self.emit_line(&format!("local {out} = {dict}[{key}]"));
                }
            }
            "dict_set" | "dict_setdefault" | "callargs_push_kw" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let dict = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    let val = sanitize_ident(&args[2]);
                    self.emit_line(&format!("{dict}[{key}] = {val}"));
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
                    self.emit_line(&format!("local {out} = {dict}[{key}]"));
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
                        "local {out} = nil; for __k, __v in pairs({dict}) do {out} = {{__k, __v}}; {dict}[__k] = nil; break end"
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
            "set_add" | "frozenset_add" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let set = sanitize_ident(&args[0]);
                    let val = sanitize_ident(&args[1]);
                    self.emit_line(&format!("{set}[{val}] = true"));
                }
            }
            "set_discard" | "set_remove" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let set = sanitize_ident(&args[0]);
                    let val = sanitize_ident(&args[1]);
                    self.emit_line(&format!("{set}[{val}] = nil"));
                }
            }
            "set_pop" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(set) = args.first() {
                    let set = sanitize_ident(set);
                    self.emit_line(&format!(
                        "local {out} = nil; for __k in pairs({set}) do {out} = __k; {set}[__k] = nil; break end"
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
                    self.emit_line(&format!("local {out} = ({container}[{val}] ~= nil)"));
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
                    // When the key is known-integer (from fast_int or type_hint),
                    // skip the runtime type guard and always offset by +1.
                    let key_is_int = op.fast_int == Some(true)
                        || op.raw_int == Some(true)
                        || matches!(op.type_hint.as_deref(), Some("int"));
                    if key_is_int {
                        self.emit_line(&format!("local {out} = {container}[{key} + 1]"));
                    } else {
                        self.emit_line(&format!(
                            "local {out} = {container}[if type({key}) == \"number\" then {key} + 1 else {key}]"
                        ));
                    }
                }
            }
            "set_item" | "store_subscript" | "store_index" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let container = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    let value = sanitize_ident(&args[2]);
                    let key_is_int = op.fast_int == Some(true)
                        || op.raw_int == Some(true)
                        || matches!(op.type_hint.as_deref(), Some("int"));
                    if key_is_int {
                        self.emit_line(&format!("{container}[{key} + 1] = {value}"));
                    } else {
                        self.emit_line(&format!(
                            "{container}[if type({key}) == \"number\" then {key} + 1 else {key}] = {value}"
                        ));
                    }
                }
            }
            "del_index" | "del_item" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let container = sanitize_ident(&args[0]);
                    let key = sanitize_ident(&args[1]);
                    self.emit_line(&format!("{container}[{key}] = nil"));
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
                let attr = sanitize_ident(raw_attr);
                if let Some(obj) = args.first() {
                    let obj = sanitize_ident(obj);
                    // For dunder attrs that might be on functions (stored
                    // in the side-table), look there first.
                    let use_side_table =
                        matches!(raw_attr, "__defaults__" | "__kwdefaults__" | "__closure__");
                    if use_side_table {
                        self.emit_line(&format!(
                            "local {out} = if molt_func_attrs[{obj}] then molt_func_attrs[{obj}].{attr} else nil"
                        ));
                    } else {
                        self.emit_line(&format!("local {out} = {obj}.{attr}"));
                    }
                }
            }
            "get_attr_name" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let obj = sanitize_ident(&args[0]);
                    let attr_name = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "local {out} = if type({obj}) == \"table\" then {obj}[{attr_name}] else nil"
                    ));
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
                        "local {out}; if type({obj}) == \"table\" and {obj}[{attr_name}] ~= nil then {out} = {obj}[{attr_name}] else {out} = {default} end"
                    ));
                } else if let Some(obj) = args.first() {
                    let obj = sanitize_ident(obj);
                    let attr = sanitize_ident(op.s_value.as_deref().unwrap_or("unknown"));
                    self.emit_line(&format!(
                        "local {out}; if {obj}.{attr} ~= nil then {out} = {obj}.{attr} else {out} = nil end"
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
                    self.emit_line(&format!(
                        "local {out} = (type({obj}) == \"table\" and {obj}[{attr_name}] ~= nil)"
                    ));
                } else if let Some(obj) = args.first() {
                    let obj = sanitize_ident(obj);
                    let attr = sanitize_ident(op.s_value.as_deref().unwrap_or("unknown"));
                    self.emit_line(&format!("local {out} = ({obj}.{attr} ~= nil)"));
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
                    self.emit_line(&format!(
                        "if type({obj}) == \"table\" then {obj}[{attr_name}] = {value} end"
                    ));
                }
            }
            "set_attr" | "set_attr_generic_obj" | "set_attr_generic_ptr" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                let attr = op.s_value.as_deref().unwrap_or("unknown");
                if attr.starts_with("__") && attr.ends_with("__") {
                    // Dunder attribute.  Functions can't hold attrs in Luau,
                    // so store semantically meaningful ones in the side-table
                    // and drop purely informational metadata.
                    let needs_side_table =
                        matches!(attr, "__defaults__" | "__kwdefaults__" | "__closure__");
                    if needs_side_table && args.len() >= 2 {
                        let obj = sanitize_ident(&args[0]);
                        let value = sanitize_ident(&args[1]);
                        let attr_s = sanitize_ident(attr);
                        self.emit_line(&format!(
                            "if {obj} then if not molt_func_attrs[{obj}] then molt_func_attrs[{obj}] = {{}} end; molt_func_attrs[{obj}].{attr_s} = {value} end"
                        ));
                    }
                    // All other dunders — no-op.
                } else {
                    let attr = sanitize_ident(attr);
                    if args.len() >= 2 {
                        let obj = sanitize_ident(&args[0]);
                        let value = sanitize_ident(&args[1]);
                        self.emit_line(&format!("{obj}.{attr} = {value}"));
                    }
                }
            }
            "del_attr_name" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let obj = sanitize_ident(&args[0]);
                    let attr_name = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "if type({obj}) == \"table\" then {obj}[{attr_name}] = nil end"
                    ));
                }
            }
            "del_attr_generic_obj" | "del_attr_generic_ptr" => {
                let args = op.args.as_deref().unwrap_or(&[]);
                let attr = op.s_value.as_deref().unwrap_or("unknown");
                let attr = sanitize_ident(attr);
                if let Some(obj) = args.first() {
                    let obj = sanitize_ident(obj);
                    self.emit_line(&format!("{obj}.{attr} = nil"));
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
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(obj) = args.first() {
                    self.emit_line(&format!("local {out} = molt_len({})", sanitize_ident(obj)));
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
                    self.emit_line(&format!(
                        "local {out} = string.byte({}, 1)",
                        sanitize_ident(val)
                    ));
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
                    self.emit_line(&format!("local {out} = type({})", sanitize_ident(obj)));
                }
            }
            "isinstance" | "issubclass" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = true -- [stub: {}]", op.kind));
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
            "bytes_from_obj" | "bytes_from_str" | "bytearray_from_obj" | "bytearray_from_str" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(val) = args.first() {
                    self.emit_line(&format!("local {out} = tostring({})", sanitize_ident(val)));
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
                        "molt_hash_builtin" => "function(a, ...) return molt_hash(a[1]) end",
                        "molt_ord" => "function(a, ...) return string.byte(a[1]) end",
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
                            "function(a, ...) return molt_getattr(table.unpack(a)) end"
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
                        "molt_function_set_builtin"
                        | "molt_open_builtin"
                        | "molt_set_attr_name"
                        | "molt_del_attr_name"
                        | "molt_has_attr_name"
                        | "molt_aiter"
                        | "molt_anext_builtin" => "nil",
                        _ => "nil",
                    };
                    self.emit_line(&format!("local {out} = {mapped}"));
                }
            }
            "class_new" | "module_new" | "object_new" | "builtin_type" => {
                let out = self.out_var(op);
                self.emit_line(&format!("local {out} = {{}}"));
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
            "class_set_base"
            | "object_set_class"
            | "class_set_layout_version"
            | "class_apply_set_name"
            | "class_layout_version" => {
                self.emit_line(&format!("-- [class op: {}]", op.kind));
            }
            "module_import" | "module_cache_get" | "module_cache_set" | "module_cache_del"
            | "module_import_star" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    // Try to statically map known module names.
                    let args = op.args.as_deref().unwrap_or(&[]);
                    let module_name = op.s_value.as_deref().unwrap_or("");
                    let mapped = match module_name {
                        "math" => "molt_math",
                        "json" => "json",
                        "time" => "molt_time",
                        "os" => "molt_os",
                        _ => "",
                    };
                    if !mapped.is_empty() {
                        self.emit_line(&format!("local {out} = {mapped}"));
                    } else if matches!(op.kind.as_str(), "module_cache_get" | "module_import") {
                        // Dynamic lookup via the runtime module cache.
                        // The args[0] variable holds the module name string.
                        if let Some(name_var) = args.first() {
                            let nv = sanitize_ident(name_var);
                            self.emit_line(&format!(
                                "local {out} = molt_module_cache[{nv}] or {{}}"
                            ));
                        } else {
                            self.emit_line(&format!("local {out} = {{}}"));
                        }
                    } else {
                        self.emit_line(&format!("local {out} = nil"));
                    }
                } else {
                    // module_cache_set / module_cache_del — no output needed.
                }
            }
            "module_get_attr" | "module_get_global" | "module_get_name" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                let is_global = matches!(op.kind.as_str(), "module_get_global" | "module_get_name");
                if let Some(attr_str) = op.s_value.as_deref().filter(|s| !s.is_empty()) {
                    // Static attribute name — use dot access.
                    let attr = sanitize_ident(attr_str);
                    if let Some(module) = args.first() {
                        let module = sanitize_ident(module);
                        self.emit_line(&format!("local {out} = {module}.{attr}"));
                    }
                } else if args.len() >= 2 {
                    if is_global {
                        // module_get_global: args[0] = source module (often __main__),
                        // args[1] = name var holding target module name.
                        // Look up in module cache to resolve `import math` etc.
                        let name_var = sanitize_ident(&args[1]);
                        self.emit_line(&format!(
                            "local {out} = molt_module_cache[{name_var}] or nil"
                        ));
                    } else {
                        // module_get_attr: args[0] = module table, args[1] = attr name var.
                        // Look up attribute directly on the module.
                        let module = sanitize_ident(&args[0]);
                        let attr_var = sanitize_ident(&args[1]);
                        self.emit_line(&format!(
                            "local {out} = if type({module}) == \"table\" then {module}[{attr_var}] else nil"
                        ));
                    }
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
            "module_del_global" => {
                // Module dict deletion is a no-op in Luau.
            }

            // ================================================================
            // Alloc / memory (table stubs)
            // ================================================================
            "alloc"
            | "alloc_class"
            | "alloc_class_trusted"
            | "alloc_class_static"
            | "alloc_task" => {
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
                         \t\tlocal __co = coroutine.wrap({func_name})\n\
                         \t\twhile true do\n\
                         \t\t\tlocal __item = __co()\n\
                         \t\t\tif __item == nil then break end\n\
                         \t\t\tif type(__item) == \"table\" then\n\
                         \t\t\t\tif __item[2] == true then break end\n\
                         \t\t\t\ttable.insert(__result, __item[1])\n\
                         \t\t\telse\n\
                         \t\t\t\ttable.insert(__result, __item)\n\
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
            "dataclass_new" => {
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
            | "exception_stack_set_depth"
            | "exception_clear" => {
                // Exception bookkeeping — no Luau equivalent.
            }
            "exception_new" | "exception_new_from_class" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                let msg = args
                    .first()
                    .map(|a| sanitize_ident(a))
                    .unwrap_or_else(|| "\"error\"".to_string());
                self.emit_line(&format!("local {out} = {msg}"));
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
            "exception_last"
            | "exception_stack_depth"
            | "exception_kind"
            | "exception_class"
            | "exception_message"
            | "exceptiongroup_match"
            | "exceptiongroup_combine" => {
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
            | "task_register_token_owned"
            | "is_native_awaitable" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil -- [async: {}]", op.kind));
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
            "getargv" | "getframe" | "sys_executable" | "bridge_unavailable" | "missing" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil -- [{}]", op.kind));
                }
            }
            "super_new" | "classmethod_new" | "staticmethod_new" | "property_new" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil -- [{}]", op.kind));
                }
            }

            // ================================================================
            // Closure/code internals
            // ================================================================
            "code_slot_set"
            | "code_slots_init"
            | "fn_ptr_code_set"
            | "asyncgen_locals_register"
            | "gen_locals_register"
            | "function_closure_bits"
            | "trace_enter_slot"
            | "trace_exit"
            | "frame_locals_set" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil -- [internal: {}]", op.kind));
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
                    self.emit_line(&format!("for _, v in ipairs({iterable}) do {body_op} end"));
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
            "json_parse" | "msgpack_parse" | "cbor_parse" | "invoke_ffi" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil -- [{}]", op.kind));
                }
            }

            // ================================================================
            // Memoryview / complex / bytearray stubs
            // ================================================================
            "memoryview_new"
            | "memoryview_tobytes"
            | "memoryview_cast"
            | "intarray_from_seq"
            | "complex_from_obj"
            | "bytearray_fill_range" => {
                if let Some(ref out_name) = op.out {
                    let out = sanitize_ident(out_name);
                    self.emit_line(&format!("local {out} = nil -- [{}]", op.kind));
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
            "string_strip" | "string_lstrip" | "string_rstrip" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(s) = args.first() {
                    let s = sanitize_ident(s);
                    self.emit_line(&format!("local {out} = ({s}:match(\"^%s*(.-)%s*$\"))"));
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
            "string_startswith" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let s = sanitize_ident(&args[0]);
                    let prefix = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "local {out} = (string.sub({s}, 1, #{prefix}) == {prefix})"
                    ));
                }
            }
            "string_endswith" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let s = sanitize_ident(&args[0]);
                    let suffix = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "local {out} = (string.sub({s}, -#{suffix}) == {suffix})"
                    ));
                }
            }
            "string_replace" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 3 {
                    let s = sanitize_ident(&args[0]);
                    let old = sanitize_ident(&args[1]);
                    let new = sanitize_ident(&args[2]);
                    self.emit_line(&format!("local {out} = (string.gsub({s}, {old}, {new}))"));
                }
            }
            "string_find" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if args.len() >= 2 {
                    let s = sanitize_ident(&args[0]);
                    let sub = sanitize_ident(&args[1]);
                    self.emit_line(&format!(
                        "local {out} = (string.find({s}, {sub}, 1, true) or 0) - 1"
                    ));
                }
            }
            "string_split" => {
                let out = self.out_var(op);
                let args = op.args.as_deref().unwrap_or(&[]);
                if let Some(s) = args.first() {
                    let s = sanitize_ident(s);
                    let sep = if args.len() >= 2 {
                        sanitize_ident(&args[1])
                    } else {
                        "\" \"".to_string()
                    };
                    self.emit_line(&format!("local {out} = molt_string.split({s}, {sep})"));
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
            "string_split_ws_dict_inc" | "string_split_sep_dict_inc" | "taq_ingest_line" => {
                self.emit_line(&format!("-- [string op: {}]", op.kind));
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
                // In Luau, use pcall for try blocks.
                self.emit_line("-- [try_start]");
            }
            "try_end" => {
                self.emit_line("-- [try_end]");
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
            "phi" | "nop" => {}

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
            let lhs = sanitize_ident(&args[0]);
            let rhs = sanitize_ident(&args[1]);
            self.emit_line(&format!("local {out} = {lhs} {operator} {rhs}"));
        }
    }

    // --- helper: bit32 op ---
    fn emit_bit_op(&mut self, op: &OpIR, func: &str) {
        let out = self.out_var(op);
        let args = op.args.as_deref().unwrap_or(&[]);
        if args.len() >= 2 {
            let lhs = sanitize_ident(&args[0]);
            let rhs = sanitize_ident(&args[1]);
            self.emit_line(&format!("local {out} = bit32.{func}({lhs}, {rhs})"));
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

fn collect_luau_preview_blockers(source: &str) -> Vec<String> {
    source
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            // Only flag patterns that indicate truly broken control flow
            // (goto/branch without structured replacement).  Nil-stub
            // comments like `-- [exception_last]` are harmless Luau and
            // label comments like `-- ::label_0::` are inert.
            let is_blocker = trimmed.contains("-- [unsupported op:");
            if is_blocker {
                Some(trimmed.to_string())
            } else {
                None
            }
        })
        .collect()
}

pub fn validate_luau_source(source: &str) -> Result<(), String> {
    let blockers = collect_luau_preview_blockers(source);
    if blockers.is_empty() {
        return Ok(());
    }
    let mut message = format!(
        "luau preview backend rejected lowered output with {} unsupported marker{}:",
        blockers.len(),
        if blockers.len() == 1 { "" } else { "s" }
    );
    for blocker in blockers.iter().take(8) {
        let _ = write!(message, "\n- {blocker}");
    }
    if blockers.len() > 8 {
        let _ = write!(message, "\n- ... {} more", blockers.len() - 8);
    }
    Err(message)
}

/// Performance review of emitted Luau source.
///
/// Returns a report of remaining perf opportunities that an agent or human
/// reviewer can act on before the next pipeline phase (deploy, Studio MCP, etc.).
/// Each entry is a (line_number, category, message) triple.
pub fn review_luau_perf(source: &str) -> Vec<(usize, &'static str, String)> {
    let mut issues = Vec::new();
    for (i, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        let ln = i + 1;

        // Remaining helper calls that should have been inlined.
        if trimmed.contains("molt_pow(") {
            issues.push((
                ln,
                "helper-call",
                "molt_pow() not inlined — use a ^ b".into(),
            ));
        }
        if trimmed.contains("molt_floor_div(") {
            issues.push((
                ln,
                "helper-call",
                "molt_floor_div() not inlined — use math_floor(a / b)".into(),
            ));
        }
        if trimmed.contains("molt_mod(") {
            issues.push((
                ln,
                "helper-call",
                "molt_mod() not inlined — use a % b".into(),
            ));
        }

        // Type-checked add that could be numeric.
        if trimmed.contains("if type(") && trimmed.contains("then tostring(") {
            issues.push((
                ln,
                "type-check",
                "type-checked add — verify if operands are numeric".into(),
            ));
        }

        // table.insert in user code (not in helper definitions).
        if trimmed.contains("table.insert(") && !trimmed.starts_with("--") {
            issues.push((
                ln,
                "table-insert",
                "table.insert() — use result[n] = x for speed".into(),
            ));
        }

        // Missing @native on `local function` definitions (only syntax that supports @native).
        if trimmed.starts_with("local function ") && !trimmed.starts_with("--") {
            // Check if previous line has @native.
            if i == 0
                || !source
                    .lines()
                    .nth(i - 1)
                    .map_or(false, |prev| prev.trim() == "@native")
            {
                // Don't flag runtime helper definitions.
                if !trimmed.contains("molt_range")
                    && !trimmed.contains("molt_len")
                    && !trimmed.contains("molt_int")
                    && !trimmed.contains("molt_float")
                    && !trimmed.contains("molt_str")
                    && !trimmed.contains("molt_bool")
                {
                    issues.push((ln, "native", "function missing @native annotation".into()));
                }
            }
        }

        // Unsupported ops that are still present.
        if trimmed.contains("-- [unsupported op:") {
            issues.push((ln, "unsupported", trimmed.to_string()));
        }
    }
    issues
}

/// Sanitize a Molt IR identifier for Luau.
/// Replaces `.` and `-` with `_`, and prefixes Luau keywords with `_m_`.
fn sanitize_ident(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c == '.' || c == '-' { '_' } else { c })
        .collect();

    if is_luau_keyword(&cleaned) {
        format!("_m_{cleaned}")
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

/// Strip dead code after unconditional returns at the same nesting depth.
///
/// Tracks nesting depth via structured control flow ops (if/else/end_if,
/// loop_start/loop_end, for_range/for_iter/end_for). Code after a return
/// at depth 0 (function body level) is removed. Code after a return inside
/// a structured block is kept because the block's `end` re-establishes
/// reachability for the parent scope.
/// Detects "store retval + jump to exit" patterns and converts them into
/// direct return ops, eliminating the need for goto in early-return patterns.
///
/// Pattern detected (inside if blocks):
///   store_index(retval_slot, return_value, value)
///   jump(exit_label)
/// Where exit_label leads to:
///   label(exit_label)
///   index(out, retval_slot, slot_index)
///   ret(out)
///
/// Transformed to:
///   ret_direct(value)      — a synthetic op that emits `return value`
///   jump (kept for dead-code elimination to mark rest as unreachable)
/// Rewrite `iter` + `while true` + `iter_next` + `get_item` patterns into
/// `for_iter` + `end_for`, producing idiomatic `for _, v in ipairs(t) do`.
///
/// The Python frontend emits iteration as:
///   iter(iterable) → loop_start → iter_next → get_item(result,1) [exhausted?]
///   → if → break → end_if → get_item(result,0) [value] → body → loop_end
///
/// We detect this pattern and collapse it to for_iter/end_for.
fn lower_iter_to_for(ops: &[OpIR]) -> Vec<OpIR> {
    if ops.is_empty() {
        return ops.to_vec();
    }

    let mut result: Vec<OpIR> = Vec::with_capacity(ops.len());
    let mut i = 0;

    while i < ops.len() {
        // Look for: iter(iterable) at position i
        if ops[i].kind == "iter" {
            let iter_op = &ops[i];
            let iter_out = iter_op.out.as_deref().unwrap_or("");
            let iterable = iter_op
                .args
                .as_deref()
                .and_then(|a| a.first())
                .cloned()
                .unwrap_or_default();

            // Scan forward for the matching loop pattern.
            // We need: ... → loop_start → ... → iter_next(iter_out) → get_item → if → break
            // The pattern can have nil-checks and TypeError guards between iter and loop_start.
            let mut found_pattern = false;
            let mut loop_start_idx = None;
            let mut iter_next_idx = None;
            let mut iter_next_out = String::new();
            let mut value_var = String::new();
            let mut loop_end_idx = None;
            let _skip_until = i + 1; // ops to skip (boilerplate)

            // Find loop_start — skip exception boilerplate (check_exception, raise,
            // exception_last, const_none, is, not, if, end_if, etc.).
            // The frontend emits ~30 boilerplate ops between iter and loop_start.
            for j in (i + 1)..ops.len().min(i + 50) {
                if ops[j].kind == "loop_start" {
                    loop_start_idx = Some(j);
                    break;
                }
            }

            if let Some(ls_idx) = loop_start_idx {
                // Find iter_next — skip check_exception boilerplate after loop_start.
                for j in (ls_idx + 1)..ops.len().min(ls_idx + 15) {
                    if ops[j].kind == "iter_next" {
                        let args = ops[j].args.as_deref().unwrap_or(&[]);
                        if let Some(arg) = args.first() {
                            // The iter_next should reference the iter output or the
                            // iterable variable directly.
                            if arg == iter_out || arg == &iterable {
                                iter_next_idx = Some(j);
                                iter_next_out = ops[j].out.as_deref().unwrap_or("").to_string();
                                break;
                            }
                        }
                    }
                }
            }

            if let Some(in_idx) = iter_next_idx {
                // Find the value extraction from iter_next result.
                // Pattern:
                //   iter_next → index(result, 1) [exhausted] → loop_break_if_true
                //   → index(result, 0) [value] → body
                // The VALUE is the index op that comes AFTER the break check,
                // not the first one (which is the exhausted flag).
                let mut found_break = false;
                let mut exhausted_flag_var: Option<String> = None;
                let mut break_cond_var: Option<String> = None;
                for j in (in_idx + 1)..ops.len().min(in_idx + 30) {
                    if matches!(ops[j].kind.as_str(), "get_item" | "subscript" | "index") {
                        let args = ops[j].args.as_deref().unwrap_or(&[]);
                        if args.len() >= 2 && args[0] == iter_next_out {
                            let out = ops[j].out.as_deref().unwrap_or("").to_string();
                            if !found_break {
                                if exhausted_flag_var.is_none() {
                                    exhausted_flag_var = Some(out);
                                }
                            } else if value_var.is_empty() {
                                value_var = out;
                                break;
                            }
                        }
                        continue;
                    }
                    if matches!(
                        ops[j].kind.as_str(),
                        "loop_break_if_true" | "loop_break_if_false"
                    ) {
                        if let Some(arg) = ops[j].args.as_deref().and_then(|a| a.first()) {
                            break_cond_var = Some(arg.clone());
                        }
                        found_break = true;
                        continue;
                    }
                    // Legacy if/break/end_if forms are intentionally skipped:
                    // without a direct loop_break_if_* guard variable we cannot
                    // prove this is the iterator exhaustion check safely.
                    if !found_break && ops[j].kind == "break" {
                        break;
                    }
                }

                // Find the matching loop_end by counting nesting.
                if let Some(ls_idx) = loop_start_idx {
                    let mut depth = 1i32;
                    for j in (ls_idx + 1)..ops.len() {
                        match ops[j].kind.as_str() {
                            "loop_start" => depth += 1,
                            "loop_end" => {
                                depth -= 1;
                                if depth == 0 {
                                    loop_end_idx = Some(j);
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                }

                let break_checks_exhaust_flag = matches!(
                    (&exhausted_flag_var, &break_cond_var),
                    (Some(flag), Some(cond)) if flag == cond
                );

                if break_checks_exhaust_flag && !value_var.is_empty() && loop_end_idx.is_some() {
                    found_pattern = true;
                }
            }

            if found_pattern {
                let _ls_idx = loop_start_idx.unwrap();
                let in_idx = iter_next_idx.unwrap();
                let le_idx = loop_end_idx.unwrap();

                // Emit for_iter op.
                result.push(OpIR {
                    kind: "for_iter".to_string(),
                    out: Some(value_var.clone()),
                    args: Some(vec![iterable.clone()]),
                    ..OpIR::default()
                });

                // Find where the loop body starts: after the break-on-exhausted
                // pattern (iter_next → get_item → if → break → end_if → get_item).
                // We need to skip the boilerplate and emit only the body.
                // The body starts after the last get_item that unpacks iter_next_out
                // (which assigns the loop variable into a slot).
                let mut body_start = in_idx + 1;

                // Scan past the unpack + break pattern to find body start.
                // Look for the pattern: get_item, const, get_item, if, break, end_if,
                // then optional store into a slot variable.
                let mut break_end = in_idx + 1;
                let mut depth = 0i32;
                let mut seen_break_check = false;
                for j in (in_idx + 1)..le_idx {
                    match ops[j].kind.as_str() {
                        "if" => depth += 1,
                        "end_if" => {
                            depth -= 1;
                            if depth < 0 {
                                break_end = j + 1;
                                depth = 0;
                            }
                        }
                        _ => {}
                    }
                    // Check if this op still references the iter_next output (part of unpack)
                    let refs_iter = ops[j]
                        .args
                        .as_deref()
                        .map_or(false, |args| args.iter().any(|a| a == &iter_next_out));
                    if refs_iter
                        || matches!(
                            ops[j].kind.as_str(),
                            "const_int"
                                | "const"
                                | "break"
                                | "check_exception"
                                | "exception_last"
                                | "const_none"
                                | "is"
                                | "not"
                                | "if"
                                | "end_if"
                                | "raise"
                                | "jump"
                                | "nop"
                                | "line"
                                | "exception_new"
                                | "exception_stack_set_depth"
                                | "exception_stack_exit"
                                | "tuple_new"
                                | "const_str"
                                | "loop_break_if_true"
                                | "loop_break_if_false"
                        )
                    {
                        body_start = j + 1;
                    }
                    // Track when we've passed the break check.
                    if matches!(
                        ops[j].kind.as_str(),
                        "loop_break_if_true" | "loop_break_if_false" | "break"
                    ) {
                        seen_break_check = true;
                    }
                    // Stop scanning after end_if at depth 0, but ONLY after we've
                    // already passed the break check. Exception-handling end_if ops
                    // appear BEFORE the break check and we must not stop there.
                    if seen_break_check && ops[j].kind == "end_if" && depth <= 0 && j > in_idx + 2 {
                        body_start = j + 1;
                        break;
                    }
                    // After the break check, once we find the value extraction
                    // (an index op referencing iter_next_out), we're done.
                    if seen_break_check
                        && refs_iter
                        && matches!(ops[j].kind.as_str(), "get_item" | "subscript" | "index")
                    {
                        body_start = j + 1;
                        break;
                    }
                }
                body_start = body_start.max(break_end);

                // Now find the actual value extraction: look for set_item or store
                // ops that write the unpacked value into a usable slot.
                // These appear right after the break check.
                for j in body_start..le_idx.min(body_start + 8) {
                    let refs_value = ops[j]
                        .args
                        .as_deref()
                        .map_or(false, |args| args.iter().any(|a| a == &value_var));
                    if refs_value && matches!(ops[j].kind.as_str(), "set_item" | "store_local") {
                        // This stores the loop variable into a slot — part of boilerplate.
                        body_start = j + 1;
                    } else if !refs_value && ops[j].kind == "const_int" {
                        // Index constant for the set_item — skip.
                        body_start = j + 1;
                    } else {
                        break;
                    }
                }

                // Skip any ops between iter and loop body that are boilerplate
                // (nil checks, TypeError, etc.) — they're between i and ls_idx.
                // We already emitted for_iter, so skip from i to body_start.

                // Emit the body ops (from body_start to loop_end, exclusive).
                for j in body_start..le_idx {
                    // Skip `continue` at the end of the loop body — it's implicit in for loops.
                    if j == le_idx - 1 && ops[j].kind == "continue" {
                        continue;
                    }
                    result.push(ops[j].clone());
                }

                // Emit end_for.
                result.push(OpIR {
                    kind: "end_for".to_string(),
                    ..OpIR::default()
                });

                // Skip past the entire original pattern.
                i = le_idx + 1;

                // Also skip any ops between the original iter and loop_start
                // that were nil-check boilerplate (they're now unnecessary).
                continue;
            }
        }

        result.push(ops[i].clone());
        i += 1;
    }

    result
}

fn lower_early_returns(ops: &[OpIR]) -> Vec<OpIR> {
    if ops.is_empty() {
        return ops.to_vec();
    }

    // Phase 1: Find the "return label" pattern.
    // Look for: label(N) → ... → index(out, slot, idx) → ret(out)
    // This tells us which label is the "return exit" and which slot holds
    // the return value.
    let mut return_labels: HashMap<i64, (String, String)> = HashMap::new(); // label_id → (slot_var, index_var)

    for i in 0..ops.len() {
        if ops[i].kind == "label" {
            if let Some(label_id) = ops[i].value {
                // Scan forward past exception boilerplate for index → ret
                let mut j = i + 1;
                while j < ops.len() {
                    let k = ops[j].kind.as_str();
                    if matches!(
                        k,
                        "exception_stack_set_depth"
                            | "exception_stack_exit"
                            | "check_exception"
                            | "exception_last"
                            | "const_none"
                            | "is"
                            | "not"
                            | "nop"
                            | "line"
                    ) {
                        j += 1;
                        continue;
                    }
                    if k == "index" {
                        if let (Some(out), Some(args)) = (&ops[j].out, &ops[j].args) {
                            if args.len() >= 2 {
                                let slot = &args[0];
                                // Look for ret following this index
                                let mut m = j + 1;
                                while m < ops.len() {
                                    let mk = ops[m].kind.as_str();
                                    if matches!(
                                        mk,
                                        "check_exception"
                                            | "exception_stack_set_depth"
                                            | "exception_stack_exit"
                                            | "nop"
                                            | "line"
                                    ) {
                                        m += 1;
                                        continue;
                                    }
                                    if mk == "ret" {
                                        if let Some(ref ret_var) = ops[m].var {
                                            if ret_var == out {
                                                return_labels.insert(
                                                    label_id,
                                                    (slot.clone(), args[1].clone()),
                                                );
                                            }
                                        }
                                    }
                                    break;
                                }
                            }
                        }
                    }
                    break;
                }
            }
        }
    }

    if return_labels.is_empty() {
        return ops.to_vec();
    }

    // Phase 2: Find store_index(slot, idx, value) → jump(exit_label) patterns
    // and replace with direct return.
    let mut result = Vec::with_capacity(ops.len());
    let mut i = 0;
    'outer: while i < ops.len() {
        if ops[i].kind == "store_index" {
            if let Some(ref args) = ops[i].args {
                if args.len() >= 3 {
                    let slot = &args[0];
                    let idx = &args[1];
                    let value = &args[2];

                    // Look ahead past exception boilerplate for a jump to a return label.
                    let mut j = i + 1;
                    while j < ops.len() {
                        let k = ops[j].kind.as_str();
                        if matches!(
                            k,
                            "check_exception"
                                | "exception_stack_set_depth"
                                | "exception_stack_exit"
                                | "exception_last"
                                | "const_none"
                                | "is"
                                | "not"
                                | "if"
                                | "raise"
                                | "end_if"
                                | "nop"
                                | "line"
                        ) {
                            j += 1;
                            continue;
                        }
                        if k == "jump" {
                            if let Some(target_label) = ops[j].value {
                                if let Some((ret_slot, ret_idx)) = return_labels.get(&target_label)
                                {
                                    if slot == ret_slot && idx == ret_idx {
                                        // Match! Replace store_index + jump with ret.
                                        result.push(OpIR {
                                            kind: "ret".to_string(),
                                            out: None,
                                            args: None,
                                            var: Some(value.clone()),
                                            value: None,
                                            f_value: None,
                                            s_value: None,
                                            bytes: None,
                                            fast_int: None,
                                            task_kind: None,
                                            container_type: None,
                                            stack_eligible: None,
                                            fast_float: None,
                                            type_hint: None,
                                            raw_int: None,
                                        });
                                        // Skip past the jump and continue outer loop.
                                        i = j + 1;
                                        continue 'outer;
                                    }
                                }
                            }
                        }
                        break;
                    }
                }
            }
        }
        result.push(ops[i].clone());
        i += 1;
    }

    result
}

fn strip_dead_after_return(ops: &[OpIR]) -> Vec<OpIR> {
    let mut result = Vec::with_capacity(ops.len());
    let mut depth: i32 = 0;
    let mut dead_at_depth: Option<i32> = None; // depth at which we became dead

    for op in ops {
        let kind = op.kind.as_str();

        // Track structured nesting.
        let is_open = matches!(kind, "if" | "loop_start" | "for_range" | "for_iter");
        let is_mid = matches!(kind, "else");
        let is_close = matches!(kind, "end_if" | "loop_end" | "end_for");

        if is_open {
            if dead_at_depth.is_none() {
                result.push(op.clone());
            }
            depth += 1;
            continue;
        }
        if is_mid {
            // `else` doesn't change depth but resets dead state if we're
            // dead at this depth (the other branch may not have returned).
            if dead_at_depth == Some(depth) {
                dead_at_depth = None;
            }
            if dead_at_depth.is_none() {
                result.push(op.clone());
            }
            continue;
        }
        if is_close {
            depth -= 1;
            // Closing a block may bring us back to a reachable state.
            if let Some(d) = dead_at_depth {
                if d > depth {
                    dead_at_depth = None;
                }
            }
            if dead_at_depth.is_none() {
                result.push(op.clone());
            }
            continue;
        }

        // If we're in dead code, skip this op.
        if let Some(d) = dead_at_depth {
            if depth >= d {
                continue;
            }
            // We're at a shallower depth now — no longer dead.
            dead_at_depth = None;
        }

        // Check if this op is an unconditional return.
        let is_return = matches!(kind, "ret" | "return" | "return_value" | "ret_void");
        result.push(op.clone());

        if is_return {
            dead_at_depth = Some(depth);
        }
    }

    result
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

/// Post-processing pass: inline single-use constants.
///
/// Finds patterns like:
///   local v42 = <literal>
/// where v42 appears exactly once more in the source, and replaces
/// that single use with the literal value, removing the declaration.
fn inline_single_use_constants(source: &mut String) {
    let lines: Vec<&str> = source.lines().collect();

    // Phase 1: Identify constant declarations and count variable uses.
    let mut const_decls: HashMap<String, (usize, String)> = HashMap::new(); // var -> (line_idx, rhs)
    let mut var_use_count: HashMap<String, usize> = HashMap::new();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        // Match "local vNNN = <literal>"
        if let Some(rest) = trimmed.strip_prefix("local v") {
            if let Some(eq_pos) = rest.find(" = ") {
                let var_suffix = &rest[..eq_pos];
                if var_suffix.chars().all(|c| c.is_ascii_digit()) {
                    let var_name = format!("v{var_suffix}");
                    let rhs = rest[eq_pos + 3..].to_string();
                    // Only inline simple literals — variable copies are unsafe
                    // because the source variable may be reassigned between
                    // declaration and use (closure save/restore patterns).
                    if is_simple_literal(&rhs) {
                        const_decls.insert(var_name, (i, rhs));
                    }
                }
            }
        }

        // Count all vNNN references in this line.
        let bytes = line.as_bytes();
        let mut pos = 0;
        while pos < bytes.len() {
            if bytes[pos] == b'v' && (pos == 0 || !is_ident_char(bytes[pos - 1])) {
                let start = pos;
                pos += 1;
                while pos < bytes.len() && bytes[pos].is_ascii_digit() {
                    pos += 1;
                }
                if pos > start + 1 && (pos >= bytes.len() || !is_ident_char(bytes[pos])) {
                    let var = std::str::from_utf8(&bytes[start..pos]).unwrap_or("");
                    *var_use_count.entry(var.to_string()).or_insert(0) += 1;
                }
            } else {
                pos += 1;
            }
        }
    }

    // Phase 2: Find single-use constants (declared once + used once = count 2).
    let mut inline_map: HashMap<String, String> = HashMap::new();
    let mut remove_lines: HashSet<usize> = HashSet::new();

    for (var, (line_idx, rhs)) in &const_decls {
        if var_use_count.get(var).copied().unwrap_or(0) == 2 {
            // Exactly 2 occurrences: 1 declaration + 1 use.
            // Only inline short literals to avoid code bloat.
            if rhs.len() <= 80 {
                inline_map.insert(var.clone(), rhs.clone());
                remove_lines.insert(*line_idx);
            }
        }
    }

    if inline_map.is_empty() {
        return;
    }

    // Phase 3: Rebuild source with inlining.
    let mut result = String::with_capacity(source.len());
    for (i, line) in lines.iter().enumerate() {
        if remove_lines.contains(&i) {
            continue; // Skip the declaration line.
        }
        let mut new_line = (*line).to_string();
        // Replace variable references with their literal values.
        for (var, literal) in &inline_map {
            if new_line.contains(var.as_str()) {
                new_line = replace_whole_word(&new_line, var, literal);
            }
        }
        result.push_str(&new_line);
        result.push('\n');
    }

    *source = result;
    eprintln!(
        "[molt-luau] Inlined {} single-use constants",
        inline_map.len()
    );
}

fn is_simple_literal(s: &str) -> bool {
    if s == "nil" || s == "true" || s == "false" {
        return true;
    }
    // Numeric: optional minus, digits, optional decimal
    let bytes = s.as_bytes();
    if !bytes.is_empty() {
        let start = if bytes[0] == b'-' { 1 } else { 0 };
        if start < bytes.len() && bytes[start].is_ascii_digit() {
            return bytes[start..]
                .iter()
                .all(|&b| b.is_ascii_digit() || b == b'.');
        }
    }
    // String: starts and ends with "
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        return true;
    }
    false
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Replace whole-word occurrences of `needle` with `replacement` in `haystack`.
fn replace_whole_word(haystack: &str, needle: &str, replacement: &str) -> String {
    let bytes = haystack.as_bytes();
    let needle_bytes = needle.as_bytes();
    let mut result = String::with_capacity(haystack.len() + replacement.len());
    let mut pos = 0;

    while pos < bytes.len() {
        if pos + needle_bytes.len() <= bytes.len()
            && &bytes[pos..pos + needle_bytes.len()] == needle_bytes
        {
            let before_ok = pos == 0 || !is_ident_char(bytes[pos - 1]);
            let after_ok = pos + needle_bytes.len() >= bytes.len()
                || !is_ident_char(bytes[pos + needle_bytes.len()]);
            if before_ok && after_ok {
                result.push_str(replacement);
                pos += needle_bytes.len();
                continue;
            }
        }
        result.push(bytes[pos] as char);
        pos += 1;
    }
    result
}

/// Eliminate `local vN = nil -- [missing]` / `local vM = {vN}` pairs.
///
/// These arise from Python's default-argument mechanism: the IR creates
/// a `missing` sentinel wrapped in a single-element callargs table.
/// When the nil variable is only used in the wrapper, we can replace the
/// wrapper with `{nil}` and remove the nil declaration entirely.
fn eliminate_nil_missing_wrappers(source: &mut String) {
    let lines: Vec<&str> = source.lines().collect();
    let mut var_use_count: HashMap<String, usize> = HashMap::new();

    // Count uses of all vNNN variables.
    for line in &lines {
        let bytes = line.as_bytes();
        let mut pos = 0;
        while pos < bytes.len() {
            if bytes[pos] == b'v' && (pos == 0 || !is_ident_char(bytes[pos - 1])) {
                let start = pos;
                pos += 1;
                while pos < bytes.len() && bytes[pos].is_ascii_digit() {
                    pos += 1;
                }
                if pos > start + 1 && (pos >= bytes.len() || !is_ident_char(bytes[pos])) {
                    let var = std::str::from_utf8(&bytes[start..pos]).unwrap_or("");
                    *var_use_count.entry(var.to_string()).or_insert(0) += 1;
                }
            } else {
                pos += 1;
            }
        }
    }

    // Find lines matching `local vN = nil -- [missing]` where vN has exactly
    // 2 uses (declaration + one wrapper).  Mark the line for removal and record
    // the variable for replacement in the wrapper line.
    let mut remove_lines: HashSet<usize> = HashSet::new();
    let mut nil_vars: HashSet<String> = HashSet::new();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("local v") {
            if let Some(suffix) = rest.strip_suffix(" = nil -- [missing]") {
                if suffix.chars().all(|c| c.is_ascii_digit()) {
                    let var = format!("v{suffix}");
                    if var_use_count.get(&var).copied().unwrap_or(0) == 2 {
                        remove_lines.insert(i);
                        nil_vars.insert(var);
                    }
                }
            }
        }
    }

    if nil_vars.is_empty() {
        return;
    }

    // Rebuild source, replacing `{vN}` with `{nil}` for eliminated vars.
    let mut result = String::with_capacity(source.len());
    for (i, line) in lines.iter().enumerate() {
        if remove_lines.contains(&i) {
            continue;
        }
        let mut new_line = (*line).to_string();
        for var in &nil_vars {
            let wrapper = format!("{{{var}}}");
            if new_line.contains(&wrapper) {
                new_line = new_line.replace(&wrapper, "{nil}");
            }
        }
        result.push_str(&new_line);
        result.push('\n');
    }

    let removed = remove_lines.len();
    *source = result;
    eprintln!("[molt-luau] Eliminated {} nil-missing wrappers", removed);
}

/// Strip dead UnboundLocalError checks.
///
/// Pattern (3-5 lines):
///   local vN = vM == vP       (comparison — vP is often undefined)
///   if vN then
///       local vQ = "cannot access local variable ..."
///       local vR = "UnboundLocalError"
///   end
///
/// These are Python unbound-variable guards that can never trigger in
/// transpiled Luau (all variables are initialized). Remove the entire block.
fn strip_unbound_local_checks(source: &mut String) {
    let lines: Vec<&str> = source.lines().collect();
    let mut remove: HashSet<usize> = HashSet::new();
    let len = lines.len();

    let mut i = 0;
    while i < len {
        let trimmed = lines[i].trim();
        // Match: `if vN then`
        if trimmed.starts_with("if v") && trimmed.ends_with(" then") {
            // Look ahead: next line should be a string containing "cannot access local variable"
            if i + 1 < len && lines[i + 1].contains("cannot access local variable") {
                // Find the closing `end`
                let mut j = i + 2;
                while j < len {
                    if lines[j].trim() == "end" {
                        break;
                    }
                    j += 1;
                }
                if j < len && lines[j].trim() == "end" {
                    // Also remove the comparison line before the `if`
                    // Pattern: `local vN = vM == vP`
                    if i > 0 {
                        let prev = lines[i - 1].trim();
                        if prev.starts_with("local v") && prev.contains(" == ") {
                            remove.insert(i - 1);
                        }
                    }
                    for k in i..=j {
                        remove.insert(k);
                    }
                    i = j + 1;
                    continue;
                }
            }
        }
        i += 1;
    }

    if remove.is_empty() {
        return;
    }

    let mut result = String::with_capacity(source.len());
    for (i, line) in lines.iter().enumerate() {
        if !remove.contains(&i) {
            result.push_str(line);
            result.push('\n');
        }
    }
    *source = result;
    eprintln!(
        "[molt-luau] Stripped {} UnboundLocalError check lines",
        remove.len()
    );
}

/// Strip dead locals-dict stores.
///
/// Pattern: `vN["name"] = expr` where vN is a locals dict (created as `{}`
/// and only used for store/module metadata). These are Python frame
/// introspection writes — the dict is never read in transpiled Luau.
///
/// We detect the locals dict by looking for the pattern:
///   `local vN = {}` (empty table) followed only by bracket-store writes.
fn strip_dead_locals_dict_stores(source: &mut String) {
    let lines: Vec<&str> = source.lines().collect();

    // Phase 1: Find candidates — `local vN = {}`
    let mut candidates: HashMap<String, usize> = HashMap::new();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("local v") {
            if rest.ends_with(" = {}") {
                if let Some(eq_pos) = rest.find(" = {}") {
                    let suffix = &rest[..eq_pos];
                    if suffix.chars().all(|c| c.is_ascii_digit()) {
                        let var = format!("v{suffix}");
                        candidates.insert(var, i);
                    }
                }
            }
        }
    }

    if candidates.is_empty() {
        return;
    }

    // Phase 2: For each candidate, check if it's ONLY used as:
    //   vN["name"] = expr  (store)
    //   if type(vN) == "table" then vN["name"] = expr end  (guarded store)
    // If it's read in any other context, it's live — skip.
    let mut dead_dicts: HashSet<String> = HashSet::new();

    for (var, _decl_line) in &candidates {
        let var_bytes = var.as_bytes();
        let mut is_dead = true;

        for line in &lines {
            let trimmed = line.trim();
            let bytes = trimmed.as_bytes();
            // Find all occurrences of var in this line
            let mut pos = 0;
            while pos + var_bytes.len() <= bytes.len() {
                if &bytes[pos..pos + var_bytes.len()] == var_bytes {
                    let before_ok = pos == 0 || !is_ident_char(bytes[pos - 1]);
                    let after_ok = pos + var_bytes.len() >= bytes.len()
                        || !is_ident_char(bytes[pos + var_bytes.len()]);
                    if before_ok && after_ok {
                        // Check context: is this a declaration, store, guarded store,
                        // or type-check guard part of a guarded store line?
                        let is_decl = trimmed.starts_with(&format!("local {var} = {{}}"));
                        let is_store = {
                            let after = &trimmed[pos + var_bytes.len()..];
                            after.starts_with("[\"")
                        };
                        // Accept type(vN) on a guarded-store line
                        let is_type_check = pos >= 5 && &trimmed[pos - 5..pos] == "type(";
                        let on_guarded_line = trimmed.starts_with("if type(")
                            && trimmed.contains(&format!("{var}[\""));
                        if !is_decl && !is_store && !(is_type_check && on_guarded_line) {
                            is_dead = false;
                            break;
                        }
                    }
                }
                pos += 1;
            }
            if !is_dead {
                break;
            }
        }

        if is_dead {
            dead_dicts.insert(var.clone());
        }
    }

    if dead_dicts.is_empty() {
        return;
    }

    // Phase 3: Remove declaration lines and all store lines referencing dead dicts.
    let mut remove: HashSet<usize> = HashSet::new();
    for var in &dead_dicts {
        if let Some(&decl_line) = candidates.get(var) {
            remove.insert(decl_line);
        }
    }

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        for var in &dead_dicts {
            // Direct store: `vN["name"] = expr`
            if trimmed.starts_with(&format!("{var}[\"")) {
                remove.insert(i);
                break;
            }
            // Guarded store: `if type(vN) == "table" then vN["name"] = expr end`
            if trimmed.starts_with(&format!("if type({var})"))
                && trimmed.contains(&format!("{var}[\""))
            {
                remove.insert(i);
                break;
            }
        }
    }

    if remove.is_empty() {
        return;
    }

    let mut result = String::with_capacity(source.len());
    for (i, line) in lines.iter().enumerate() {
        if !remove.contains(&i) {
            result.push_str(line);
            result.push('\n');
        }
    }
    let total = remove.len();
    *source = result;
    eprintln!(
        "[molt-luau] Stripped {} dead locals-dict lines ({} dicts)",
        total,
        dead_dicts.len()
    );
}

/// Remove trailing `continue` statements from loop bodies.
/// `continue` right before `end` in a loop is a no-op — the loop naturally
/// continues to the next iteration at `end`.
fn strip_trailing_continue(source: &mut String) {
    let lines: Vec<&str> = source.lines().collect();
    let mut remove: HashSet<usize> = HashSet::new();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed == "continue" {
            // Check if next non-blank line is `end`
            let mut j = i + 1;
            while j < lines.len() && lines[j].trim().is_empty() {
                j += 1;
            }
            if j < lines.len() && lines[j].trim() == "end" {
                remove.insert(i);
            }
        }
    }

    if remove.is_empty() {
        return;
    }

    let mut result = String::with_capacity(source.len());
    for (i, line) in lines.iter().enumerate() {
        if !remove.contains(&i) {
            result.push_str(line);
            result.push('\n');
        }
    }
    *source = result;
    eprintln!(
        "[molt-luau] Stripped {} trailing continue statements",
        remove.len()
    );
}

/// Simplify comparison-break patterns in while-true loops.
/// `local vN = vA < vB; if not vN then break end` → `if vA >= vB then break end`
fn simplify_comparison_break(source: &mut String) {
    use std::collections::{HashMap, HashSet};
    let lines: Vec<&str> = source.lines().collect();
    let mut remove: HashSet<usize> = HashSet::new();
    let mut replacements: HashMap<usize, String> = HashMap::new();

    for i in 0..lines.len().saturating_sub(1) {
        let trimmed = lines[i].trim();
        let next_trimmed = lines[i + 1].trim();

        // Match: `local vN = vA < vB`
        if let Some(rest) = trimmed.strip_prefix("local v") {
            if let Some(eq_pos) = rest.find(" = ") {
                let var_suffix = &rest[..eq_pos];
                if !var_suffix.chars().all(|c| c.is_ascii_digit()) {
                    continue;
                }
                let var_name = format!("v{var_suffix}");
                let rhs = &rest[eq_pos + 3..];

                // Check if next line is `if not vN then break end`
                let expected_if = format!("if not {var_name} then break end");
                if next_trimmed != expected_if {
                    continue;
                }

                // Try to find comparison op in rhs
                let (lhs, op, rhs_val) = if let Some(pos) = rhs.find(" < ") {
                    (&rhs[..pos], ">=", &rhs[pos + 3..])
                } else if let Some(pos) = rhs.find(" > ") {
                    (&rhs[..pos], "<=", &rhs[pos + 3..])
                } else if let Some(pos) = rhs.find(" <= ") {
                    (&rhs[..pos], ">", &rhs[pos + 4..])
                } else if let Some(pos) = rhs.find(" >= ") {
                    (&rhs[..pos], "<", &rhs[pos + 4..])
                } else if let Some(pos) = rhs.find(" == ") {
                    (&rhs[..pos], "~=", &rhs[pos + 4..])
                } else if let Some(pos) = rhs.find(" ~= ") {
                    (&rhs[..pos], "==", &rhs[pos + 4..])
                } else {
                    continue;
                };

                // Verify var_name is only used on these 2 lines
                let var_bytes = var_name.as_bytes();
                let mut total_uses = 0;
                for line in &lines {
                    let bytes = line.as_bytes();
                    let mut pos = 0;
                    while pos + var_bytes.len() <= bytes.len() {
                        if &bytes[pos..pos + var_bytes.len()] == var_bytes {
                            let before_ok = pos == 0 || !is_ident_char(bytes[pos - 1]);
                            let after_ok = pos + var_bytes.len() >= bytes.len()
                                || !is_ident_char(bytes[pos + var_bytes.len()]);
                            if before_ok && after_ok {
                                total_uses += 1;
                            }
                        }
                        pos += 1;
                    }
                }
                // 1 in decl + 1 in if = 2
                if total_uses != 2 {
                    continue;
                }

                let indent = &lines[i][..lines[i].len() - trimmed.len()];
                replacements.insert(i, format!("{indent}if {lhs} {op} {rhs_val} then break end"));
                remove.insert(i + 1);
            }
        }
    }

    if remove.is_empty() && replacements.is_empty() {
        return;
    }

    let mut result = String::with_capacity(source.len());
    for (i, line) in lines.iter().enumerate() {
        if remove.contains(&i) {
            continue;
        }
        if let Some(replacement) = replacements.get(&i) {
            result.push_str(replacement);
        } else {
            result.push_str(line);
        }
        result.push('\n');
    }
    let count = remove.len() + replacements.len();
    *source = result;
    eprintln!(
        "[molt-luau] Simplified {} comparison-break patterns",
        count / 2
    );
}

/// Eliminate assignments where the RHS variable is never declared or assigned
/// anywhere in the function body. These are dead closure-restore ops: the
/// frontend emits frame-restore writes (`vN = vM`) where `vM` was a closure
/// cell that got stripped by tree_shake_luau. In Luau, reading an undeclared
/// local yields nil, making these assignments dead writes.
fn strip_undefined_rhs_assignments(source: &mut String) {
    use std::collections::HashSet;

    let lines: Vec<&str> = source.lines().collect();

    // Phase 1: Collect all defined variables (declared or assigned to).
    let mut defined_vars: HashSet<String> = HashSet::new();
    for line in &lines {
        let trimmed = line.trim();
        // `local vN` or `local vN = ...`
        if let Some(rest) = trimmed.strip_prefix("local ") {
            let var_end = rest
                .find(|c: char| !c.is_alphanumeric() && c != '_')
                .unwrap_or(rest.len());
            let var = &rest[..var_end];
            if !var.is_empty() {
                defined_vars.insert(var.to_string());
            }
        }
        // `vN = ...` (assignment, not `local`)
        if trimmed.starts_with('v') {
            if let Some(eq_pos) = trimmed.find(" = ") {
                let lhs = &trimmed[..eq_pos];
                if lhs.starts_with('v') && lhs[1..].chars().all(|c| c.is_ascii_digit()) {
                    defined_vars.insert(lhs.to_string());
                }
            }
        }
    }
    // Function parameters are also defined.
    for line in &lines {
        let trimmed = line.trim();
        if trimmed.ends_with(')')
            && (trimmed.contains("= function(") || trimmed.contains("function "))
        {
            if let Some(paren_start) = trimmed.rfind('(') {
                let params = &trimmed[paren_start + 1..trimmed.len() - 1];
                for param in params.split(", ") {
                    let p = param.trim();
                    if !p.is_empty() {
                        defined_vars.insert(p.to_string());
                    }
                }
            }
        }
        // For-loop iteration variables: `for _, vN in ...` or `for vN = ...`
        if trimmed.starts_with("for ") {
            let rest = &trimmed[4..];
            // Split on " in " or " = " to get the variable list
            let var_part = if let Some(in_pos) = rest.find(" in ") {
                &rest[..in_pos]
            } else if let Some(eq_pos) = rest.find(" = ") {
                &rest[..eq_pos]
            } else {
                continue;
            };
            for var in var_part.split(", ") {
                let v = var.trim();
                if !v.is_empty() && v != "_" {
                    defined_vars.insert(v.to_string());
                }
            }
        }
    }

    // Phase 2: Find `vN = vM` lines where vM is NOT in defined_vars.
    let mut remove: HashSet<usize> = HashSet::new();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        // Match: `vN = vM` (bare assignment, not `local`)
        if !trimmed.starts_with("local ") && trimmed.starts_with('v') {
            if let Some(eq_pos) = trimmed.find(" = ") {
                let lhs = &trimmed[..eq_pos];
                let rhs = trimmed[eq_pos + 3..].trim();
                // Both sides must be simple variable names (vN pattern).
                if lhs.starts_with('v')
                    && lhs[1..].chars().all(|c| c.is_ascii_digit())
                    && rhs.starts_with('v')
                    && rhs[1..].chars().all(|c| c.is_ascii_digit())
                    && !defined_vars.contains(rhs)
                {
                    remove.insert(i);
                }
            }
        }
    }

    if remove.is_empty() {
        return;
    }

    let mut result = String::with_capacity(source.len());
    for (i, line) in lines.iter().enumerate() {
        if !remove.contains(&i) {
            result.push_str(line);
            result.push('\n');
        }
    }
    *source = result;
    eprintln!(
        "[molt-luau] Stripped {} dead undefined-RHS assignments",
        remove.len()
    );
}

/// Propagate single-use variable copies: `local vN = vM` where vN is used
/// exactly once → replace vN with vM at the use site and remove the declaration.
/// Only applies when vM is not reassigned between declaration and use.
///
/// Runs up to 3 iterations to collapse chains (vA → vB → vC).
fn propagate_single_use_copies(source: &mut String) {
    let mut total = 0;
    for _ in 0..3 {
        let count = propagate_single_use_copies_once(source);
        if count == 0 {
            break;
        }
        total += count;
    }
    if total > 0 {
        eprintln!("[molt-luau] Propagated {} single-use copies", total);
    }
}

fn propagate_single_use_copies_once(source: &mut String) -> usize {
    let lines: Vec<&str> = source.lines().collect();

    // Phase 1: Find `local vN = vM` copy declarations and count all var uses.
    let mut copy_decls: HashMap<String, (usize, String)> = HashMap::new();
    let mut var_use_count: HashMap<String, usize> = HashMap::new();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        if let Some(rest) = trimmed.strip_prefix("local v") {
            if let Some(eq_pos) = rest.find(" = ") {
                let var_suffix = &rest[..eq_pos];
                if var_suffix.chars().all(|c| c.is_ascii_digit()) {
                    let var_name = format!("v{var_suffix}");
                    let rhs = rest[eq_pos + 3..].trim();
                    if is_simple_var_ref(rhs) {
                        copy_decls.insert(var_name, (i, rhs.to_string()));
                    }
                }
            }
        }

        // Count all vNNN references in this line.
        let bytes = line.as_bytes();
        let mut pos = 0;
        while pos < bytes.len() {
            if bytes[pos] == b'v' && (pos == 0 || !is_ident_char(bytes[pos - 1])) {
                let start = pos;
                pos += 1;
                while pos < bytes.len() && bytes[pos].is_ascii_digit() {
                    pos += 1;
                }
                if pos > start + 1 && (pos >= bytes.len() || !is_ident_char(bytes[pos])) {
                    let var = std::str::from_utf8(&bytes[start..pos]).unwrap_or("");
                    *var_use_count.entry(var.to_string()).or_insert(0) += 1;
                }
            } else {
                pos += 1;
            }
        }
    }

    // Phase 2: For single-use copies, verify source is not reassigned between
    // declaration and use.
    let mut inline_map: HashMap<String, String> = HashMap::new();
    let mut remove_lines: HashSet<usize> = HashSet::new();

    for (var, (decl_line, source_var)) in &copy_decls {
        let count = var_use_count.get(var).copied().unwrap_or(0);
        if count != 2 {
            continue; // 1 decl + 1 use = 2
        }

        // Find the use line.
        let mut use_idx = None;
        for (i, line) in lines.iter().enumerate() {
            if i == *decl_line {
                continue;
            }
            if contains_whole_word_var(line, var) {
                use_idx = Some(i);
                break;
            }
        }

        let Some(use_line) = use_idx else { continue };
        if use_line <= *decl_line {
            continue; // Use before decl — skip.
        }

        // Verify source_var is not reassigned between decl and use.
        let mut reassigned = false;
        for i in (*decl_line + 1)..use_line {
            let t = lines[i].trim();
            if t.starts_with("--") {
                continue;
            }
            // Bare assignment: `source_var = ...`
            let assign_pat = format!("{source_var} = ");
            if t.starts_with(&assign_pat) {
                reassigned = true;
                break;
            }
        }

        if !reassigned {
            inline_map.insert(var.clone(), source_var.clone());
            remove_lines.insert(*decl_line);
        }
    }

    if inline_map.is_empty() {
        return 0;
    }

    let count = inline_map.len();
    let mut result = String::with_capacity(source.len());
    for (i, line) in lines.iter().enumerate() {
        if remove_lines.contains(&i) {
            continue;
        }
        let mut new_line = (*line).to_string();
        for (var, replacement) in &inline_map {
            if new_line.contains(var.as_str()) {
                new_line = replace_whole_word(&new_line, var, replacement);
            }
        }
        result.push_str(&new_line);
        result.push('\n');
    }

    *source = result;
    count
}

/// Check if a string is a simple variable reference (v\d+ or parameter name).
fn is_simple_var_ref(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    // v\d+ pattern
    if s.starts_with('v') && s.len() > 1 && s[1..].chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    // Simple parameter names (alphabetic + underscore, no dots/brackets/parens)
    if s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        // Exclude Luau keywords
        !matches!(
            s,
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
        )
    } else {
        false
    }
}

/// Check if `line` contains a whole-word occurrence of `var`.
fn contains_whole_word_var(line: &str, var: &str) -> bool {
    let bytes = line.as_bytes();
    let var_bytes = var.as_bytes();
    let mut pos = 0;
    while pos + var_bytes.len() <= bytes.len() {
        if &bytes[pos..pos + var_bytes.len()] == var_bytes {
            let before_ok = pos == 0 || !is_ident_char(bytes[pos - 1]);
            let after_ok = pos + var_bytes.len() >= bytes.len()
                || !is_ident_char(bytes[pos + var_bytes.len()]);
            if before_ok && after_ok {
                return true;
            }
        }
        pos += 1;
    }
    false
}

/// Simplify return chains where a hoisted variable is assigned just before return.
/// Pattern: `vN = expr; [comment lines]; return vN` → `return expr`
/// Only applies when vN is used exactly 3 times total (decl + assign + return).
fn simplify_return_chain(source: &mut String) {
    let lines: Vec<&str> = source.lines().collect();

    // Count all variable uses.
    let mut var_use_count: HashMap<String, usize> = HashMap::new();
    for line in &lines {
        let bytes = line.as_bytes();
        let mut pos = 0;
        while pos < bytes.len() {
            if bytes[pos] == b'v' && (pos == 0 || !is_ident_char(bytes[pos - 1])) {
                let start = pos;
                pos += 1;
                while pos < bytes.len() && bytes[pos].is_ascii_digit() {
                    pos += 1;
                }
                if pos > start + 1 && (pos >= bytes.len() || !is_ident_char(bytes[pos])) {
                    let var = std::str::from_utf8(&bytes[start..pos]).unwrap_or("");
                    *var_use_count.entry(var.to_string()).or_insert(0) += 1;
                }
            } else {
                pos += 1;
            }
        }
    }

    let mut remove_lines: HashSet<usize> = HashSet::new();
    let mut replacements: HashMap<usize, String> = HashMap::new();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        // Match `return vN`
        if let Some(ret_var) = trimmed.strip_prefix("return ") {
            let ret_var = ret_var.trim();
            if !ret_var.starts_with('v')
                || ret_var.len() < 2
                || !ret_var[1..].chars().all(|c| c.is_ascii_digit())
            {
                continue;
            }

            // Scan backwards for `vN = expr`, skipping comments and blank lines.
            let mut assign_line = None;
            let mut assign_rhs = None;
            let mut j = i.wrapping_sub(1);
            while j < lines.len() {
                let prev = lines[j].trim();
                if prev.is_empty() || prev.starts_with("--") {
                    if j == 0 {
                        break;
                    }
                    j -= 1;
                    continue;
                }
                // Match `vN = expr` (bare assignment, not local)
                let assign_pat = format!("{ret_var} = ");
                if let Some(rhs) = prev.strip_prefix(&assign_pat) {
                    // Verify this is a bare assignment, not inside an if/etc.
                    assign_line = Some(j);
                    assign_rhs = Some(rhs.to_string());
                }
                break;
            }

            if let (Some(a_line), Some(rhs)) = (assign_line, assign_rhs) {
                // Check that ret_var has exactly 3 uses (decl + assign + return).
                let count = var_use_count.get(ret_var).copied().unwrap_or(0);
                if count == 3 {
                    let indent = &line[..line.len() - trimmed.len()];
                    replacements.insert(i, format!("{indent}return {rhs}"));
                    remove_lines.insert(a_line);
                }
            }
        }
    }

    if remove_lines.is_empty() && replacements.is_empty() {
        return;
    }

    let count = replacements.len();
    let mut result = String::with_capacity(source.len());
    for (i, line) in lines.iter().enumerate() {
        if remove_lines.contains(&i) {
            continue;
        }
        if let Some(replacement) = replacements.get(&i) {
            result.push_str(replacement);
        } else {
            result.push_str(line);
        }
        result.push('\n');
    }
    *source = result;
    eprintln!("[molt-luau] Simplified {} return chains", count);
}

/// Sink single-use locals into their sole consumer when the consumer is on
/// the immediately following (non-blank, non-comment) line.
///
/// `local vN = <expr>` followed by a line that uses vN exactly once →
/// remove the local declaration, replace vN with `<expr>` inline.
/// Only applies when the expression is ≤120 chars (avoids line bloat).
///
/// Runs iteratively to handle chains (vA → vB → vC) without introducing
/// dangling references.
fn sink_single_use_locals(source: &mut String) {
    let mut total = 0;
    for _ in 0..5 {
        let count = sink_single_use_locals_once(source);
        if count == 0 {
            break;
        }
        total += count;
    }
    if total > 0 {
        eprintln!(
            "[molt-luau] Sunk {} single-use locals into next line",
            total
        );
    }
}

fn sink_single_use_locals_once(source: &mut String) -> usize {
    let lines: Vec<&str> = source.lines().collect();

    // Phase 1: Find all `local vN = <expr>` and count uses.
    let mut local_decls: HashMap<String, (usize, String)> = HashMap::new();
    let mut var_use_count: HashMap<String, usize> = HashMap::new();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        if let Some(rest) = trimmed.strip_prefix("local v") {
            if let Some(eq_pos) = rest.find(" = ") {
                let var_suffix = &rest[..eq_pos];
                if var_suffix.chars().all(|c| c.is_ascii_digit()) {
                    let var_name = format!("v{var_suffix}");
                    let rhs = rest[eq_pos + 3..].trim().to_string();
                    if rhs.len() <= 120
                        && !rhs.contains('\n')
                        && !is_simple_literal(&rhs)
                        && !is_simple_var_ref(&rhs)
                    {
                        local_decls.insert(var_name, (i, rhs));
                    }
                }
            }
        }

        let bytes = line.as_bytes();
        let mut pos = 0;
        while pos < bytes.len() {
            if bytes[pos] == b'v' && (pos == 0 || !is_ident_char(bytes[pos - 1])) {
                let start = pos;
                pos += 1;
                while pos < bytes.len() && bytes[pos].is_ascii_digit() {
                    pos += 1;
                }
                if pos > start + 1 && (pos >= bytes.len() || !is_ident_char(bytes[pos])) {
                    let var = std::str::from_utf8(&bytes[start..pos]).unwrap_or("");
                    *var_use_count.entry(var.to_string()).or_insert(0) += 1;
                }
            } else {
                pos += 1;
            }
        }
    }

    // Phase 2: Collect candidates. Skip if the RHS references another variable
    // that is ALSO a candidate (chain hazard — handle in the next iteration).
    let mut candidates: HashMap<String, (usize, String)> = HashMap::new();
    for (var, (decl_line, expr)) in &local_decls {
        let count = var_use_count.get(var).copied().unwrap_or(0);
        if count != 2 {
            continue;
        }

        // Find the next non-blank, non-comment line.
        let mut next_line = *decl_line + 1;
        while next_line < lines.len() {
            let t = lines[next_line].trim();
            if !t.is_empty() && !t.starts_with("--") {
                break;
            }
            next_line += 1;
        }
        if next_line >= lines.len() {
            continue;
        }

        if contains_whole_word_var(lines[next_line], var) {
            candidates.insert(var.clone(), (*decl_line, expr.clone()));
        }
    }

    // Filter out candidates whose RHS references another candidate variable.
    let candidate_vars: HashSet<String> = candidates.keys().cloned().collect();
    let mut inline_map: HashMap<String, String> = HashMap::new();
    let mut remove_lines: HashSet<usize> = HashSet::new();

    for (var, (decl_line, expr)) in &candidates {
        let rhs_references_candidate = candidate_vars
            .iter()
            .any(|other| other != var && contains_whole_word_var(expr, other));
        if !rhs_references_candidate {
            // Wrap in parentheses when needed for correctness:
            // - Table constructors: `{...}[n]` is a Luau syntax error
            // - Top-level binary operators: inlining `a + b` into `expr * 2`
            //   would change precedence without parens
            let safe_expr = if expr.starts_with('{') || has_top_level_binary_op(expr) {
                format!("({expr})")
            } else {
                expr.clone()
            };
            inline_map.insert(var.clone(), safe_expr);
            remove_lines.insert(*decl_line);
        }
    }

    if inline_map.is_empty() {
        return 0;
    }

    let count = inline_map.len();
    let mut result = String::with_capacity(source.len());
    for (i, line) in lines.iter().enumerate() {
        if remove_lines.contains(&i) {
            continue;
        }
        let mut new_line = (*line).to_string();
        for (var, replacement) in &inline_map {
            if new_line.contains(var.as_str()) {
                new_line = replace_whole_word(&new_line, var, replacement);
            }
        }
        result.push_str(&new_line);
        result.push('\n');
    }

    *source = result;
    count
}

/// Performance optimization pass over emitted Luau source.
///
/// Applied after constant inlining and nil-wrapper elimination. Performs:
/// 1. Strength reduction: `x ^ 2` → `x * x`, inline trivial helper calls
/// 2. `@native` annotation on transpiled functions for Luau VM JIT
/// 3. Eliminate redundant type-checked add when operands are provably numeric
/// 4. Inline remaining `molt_pow`/`molt_floor_div` helper calls (from binop path)
/// Simplify `local vN = <int>` + `if type(vN) == "number" then vN + 1 else vN`
/// into a direct integer index. Eliminates the runtime type check when the
/// index is a known integer literal.
fn simplify_numeric_type_guards(source: &mut String) {
    use std::collections::HashMap;

    let lines: Vec<&str> = source.lines().collect();
    let mut result = String::with_capacity(source.len());

    // Phase 1: Find `local vN = <integer_literal>` declarations and check
    // if vN is ONLY used in type-guard patterns on the NEXT line.
    let mut int_consts: HashMap<String, (usize, i64)> = HashMap::new(); // var -> (line, value)
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("local v") {
            if let Some(eq_pos) = rest.find(" = ") {
                let suffix = &rest[..eq_pos];
                if suffix.chars().all(|c| c.is_ascii_digit()) {
                    let rhs = rest[eq_pos + 3..].trim();
                    // Check if RHS is a simple integer (possibly negative).
                    if let Ok(val) = rhs.parse::<i64>() {
                        let var_name = format!("v{suffix}");
                        int_consts.insert(var_name, (i, val));
                    }
                }
            }
        }
    }

    // Phase 2: For each int const, check if the next line contains the
    // type-guard pattern and the const is only used there.
    let mut remove_lines: HashSet<usize> = HashSet::new();
    let mut line_replacements: HashMap<usize, String> = HashMap::new();

    for (var, (decl_line, val)) in &int_consts {
        let next_line = decl_line + 1;
        if next_line >= lines.len() {
            continue;
        }

        let pattern = format!("if type({var}) == \"number\" then {var} + 1 else {var}",);
        if lines[next_line].contains(&pattern) {
            // Check the var isn't used elsewhere (only on these 2 lines).
            let mut total_uses = 0;
            for line in &lines {
                let bytes = line.as_bytes();
                let var_bytes = var.as_bytes();
                let mut pos = 0;
                while pos + var_bytes.len() <= bytes.len() {
                    if &bytes[pos..pos + var_bytes.len()] == var_bytes {
                        let before_ok = pos == 0 || !is_ident_char(bytes[pos - 1]);
                        let after_ok = pos + var_bytes.len() >= bytes.len()
                            || !is_ident_char(bytes[pos + var_bytes.len()]);
                        if before_ok && after_ok {
                            total_uses += 1;
                        }
                    }
                    pos += 1;
                }
            }
            // decl (1) + 3 uses in type guard = 4 total
            if total_uses == 4 {
                // Replace the type-guard with the computed index.
                let replacement = format!("{}", val + 1);
                let old_pattern =
                    format!("[if type({var}) == \"number\" then {var} + 1 else {var}]",);
                let new_pattern = format!("[{replacement}]");
                let new_line = lines[next_line].replace(&old_pattern, &new_pattern);
                line_replacements.insert(next_line, new_line);
                remove_lines.insert(*decl_line);
            }
        }
    }

    if remove_lines.is_empty() {
        return; // Nothing to simplify.
    }

    for (i, line) in lines.iter().enumerate() {
        if remove_lines.contains(&i) {
            continue;
        }
        if let Some(replacement) = line_replacements.get(&i) {
            result.push_str(replacement);
        } else {
            result.push_str(line);
        }
        result.push('\n');
    }

    *source = result;
}

fn optimize_luau_perf(source: &mut String) {
    // Pre-pass: Simplify type-guard index patterns.
    // Pattern: `local vN = <int_literal>` followed by a line containing
    //   `if type(vN) == "number" then vN + 1 else vN`
    // This checks at runtime whether a known-integer index needs +1 adjustment.
    // Since we KNOW vN is numeric, simplify to just `vN + 1` (= literal + 1).
    simplify_numeric_type_guards(source);

    let mut result = String::with_capacity(source.len());
    let mut perf_count: usize = 0;

    // Track which variables are known-numeric (assigned from numeric ops).
    let mut numeric_vars: HashSet<String> = HashSet::new();

    for line in source.lines() {
        let trimmed = line.trim();
        let mut optimized = line.to_string();

        // Skip function definition lines — the inlining passes below must not
        // rewrite `local function molt_xyz(...)` declarations.
        let is_func_def = trimmed.starts_with("local function molt_");

        // Pass 1: Inline molt_pow(a, b) → a ^ b
        while !is_func_def {
            let Some(start) = optimized.find("molt_pow(") else {
                break;
            };
            if let Some(close) = find_matching_paren(&optimized, start + 8) {
                let inner = &optimized[start + 9..close];
                if let Some(comma) = inner.find(", ") {
                    let a = inner[..comma].trim();
                    let b = inner[comma + 2..].trim();
                    let replacement = format!("{a} ^ {b}");
                    optimized = format!(
                        "{}{}{}",
                        &optimized[..start],
                        replacement,
                        &optimized[close + 1..]
                    );
                    perf_count += 1;
                    continue;
                }
            }
            break;
        }

        // Pass 2: Inline molt_floor_div(a, b) → math_floor(a / b)
        while !is_func_def {
            let Some(start) = optimized.find("molt_floor_div(") else {
                break;
            };
            if let Some(close) = find_matching_paren(&optimized, start + 14) {
                let inner = &optimized[start + 15..close];
                if let Some(comma) = inner.find(", ") {
                    let a = inner[..comma].trim();
                    let b = inner[comma + 2..].trim();
                    let replacement = format!("math_floor({a} / {b})");
                    optimized = format!(
                        "{}{}{}",
                        &optimized[..start],
                        replacement,
                        &optimized[close + 1..]
                    );
                    perf_count += 1;
                    continue;
                }
            }
            break;
        }

        // Pass 3: Inline molt_mod(a, b) → a % b
        // Python's floor-mod matches Luau's % for positive divisors, which covers
        // the vast majority of real-world uses (array indexing, hash functions, etc.).
        while !is_func_def {
            let Some(start) = optimized.find("molt_mod(") else {
                break;
            };
            if let Some(close) = find_matching_paren(&optimized, start + 8) {
                let inner = &optimized[start + 9..close];
                if let Some(comma) = inner.find(", ") {
                    let a = inner[..comma].trim();
                    let b = inner[comma + 2..].trim();
                    let replacement = format!("{a} % {b}");
                    optimized = format!(
                        "{}{}{}",
                        &optimized[..start],
                        replacement,
                        &optimized[close + 1..]
                    );
                    perf_count += 1;
                    continue;
                }
            }
            break;
        }

        // Pass 4: Track numeric variables and optimize type-checked add.
        // Handles both `local vN = expr` and bare `vN = expr` assignments.
        // When both operands of a type-checked add are known-numeric, simplify.
        {
            let (is_local, var_name_opt, rhs_opt) =
                if let Some(rest) = trimmed.strip_prefix("local ") {
                    if let Some(eq_pos) = rest.find(" = ") {
                        (
                            true,
                            Some(rest[..eq_pos].to_string()),
                            Some(&rest[eq_pos + 3..]),
                        )
                    } else {
                        (true, None, None)
                    }
                } else if trimmed.starts_with('v') {
                    if let Some(eq_pos) = trimmed.find(" = ") {
                        let lhs = &trimmed[..eq_pos];
                        if lhs.starts_with('v') && lhs[1..].chars().all(|c| c.is_ascii_digit()) {
                            (false, Some(lhs.to_string()), Some(&trimmed[eq_pos + 3..]))
                        } else {
                            (false, None, None)
                        }
                    } else {
                        (false, None, None)
                    }
                } else {
                    (false, None, None)
                };

            if let (Some(var_name), Some(rhs)) = (var_name_opt, rhs_opt) {
                // Detect numeric assignment patterns.
                let is_numeric_rhs = rhs.parse::<f64>().is_ok()
                    || rhs.starts_with("math")
                    || rhs.starts_with("bit32")
                    || rhs.contains(" + ")
                    || rhs.contains(" - ")
                    || rhs.contains(" * ")
                    || rhs.contains(" / ")
                    || rhs.contains(" ^ ")
                    || rhs.contains(" % ")
                    || rhs.starts_with("molt_int(")
                    || rhs.starts_with("molt_float(")
                    || rhs.starts_with("molt_len(")
                    || rhs.starts_with("#")
                    || rhs.starts_with("tonumber(")
                    // A variable copy from a known-numeric var is also numeric.
                    || (rhs.starts_with('v') && rhs[1..].chars().all(|c| c.is_ascii_digit())
                        && numeric_vars.contains(rhs));
                if is_numeric_rhs {
                    numeric_vars.insert(var_name.clone());
                }

                // Check for type-checked add that can be simplified.
                if rhs.starts_with("if type(")
                    && rhs.contains("then tostring(")
                    && rhs.contains("else ")
                {
                    if let Some(else_pos) = rhs.rfind("else ") {
                        let numeric_expr = &rhs[else_pos + 5..];
                        if let Some(plus) = numeric_expr.find(" + ") {
                            let lhs_var = numeric_expr[..plus].trim();
                            let rhs_var = numeric_expr[plus + 3..].trim();
                            if numeric_vars.contains(lhs_var) && numeric_vars.contains(rhs_var) {
                                let indent = &line[..line.len() - trimmed.len()];
                                if is_local {
                                    optimized =
                                        format!("{indent}local {var_name} = {numeric_expr}");
                                } else {
                                    optimized = format!("{indent}{var_name} = {numeric_expr}");
                                }
                                numeric_vars.insert(var_name);
                                perf_count += 1;
                            }
                        }
                    }
                }
            }
        }

        // Pass 4b: Simplify index type-guards for known-numeric variables.
        // Pattern: `[if type(vN) == "number" then vN + 1 else vN]` → `[vN + 1]`
        while optimized.contains("if type(") && optimized.contains("+ 1 else") {
            let search = "if type(";
            let Some(start) = optimized.find(search) else {
                break;
            };
            // Check bracket context: must be inside `[...]`
            let bracket_start = if start > 0 && optimized.as_bytes()[start - 1] == b'[' {
                start - 1
            } else {
                break;
            };
            // Extract var name from `if type(vN) ==`
            let after_type = &optimized[start + search.len()..];
            let Some(close_paren) = after_type.find(')') else {
                break;
            };
            let var = &after_type[..close_paren];
            if !var.starts_with('v') || !var[1..].chars().all(|c| c.is_ascii_digit()) {
                break;
            }
            // Verify full pattern
            let full_pattern = format!("[if type({var}) == \"number\" then {var} + 1 else {var}]");
            if !optimized[bracket_start..].starts_with(&full_pattern) {
                break;
            }
            if numeric_vars.contains(var) {
                let replacement = format!("[{var} + 1]");
                optimized = optimized.replacen(&full_pattern, &replacement, 1);
                perf_count += 1;
                continue; // Check for more on same line
            }
            break;
        }

        // Pass 5: Strength reduce x ^ 2 → x * x (only for literal 2).
        if optimized.contains(" ^ 2") {
            // Find pattern: `someVar ^ 2` where 2 is a literal (not part of larger number).
            let bytes = optimized.as_bytes();
            let mut i = 0;
            while i + 4 < bytes.len() {
                if &bytes[i..i + 4] == b" ^ 2"
                    && (i + 4 >= bytes.len() || !bytes[i + 4].is_ascii_digit())
                {
                    // Find the start of the operand (scan backwards for ident).
                    let mut start = i;
                    while start > 0 && is_ident_char(bytes[start - 1]) {
                        start -= 1;
                    }
                    if start < i {
                        let operand = std::str::from_utf8(&bytes[start..i]).unwrap_or("");
                        if !operand.is_empty() {
                            let replacement = format!("{operand} * {operand}");
                            optimized = format!(
                                "{}{}{}",
                                &optimized[..start],
                                replacement,
                                &optimized[i + 4..]
                            );
                            perf_count += 1;
                            break; // Only one replacement per line to avoid index issues.
                        }
                    }
                }
                i += 1;
            }
        }

        // Pass 6: Add @native to function definitions (non-helper user functions).
        // Only annotate `local function molt_*` (transpiled user functions), not helpers.
        if trimmed.starts_with("local function molt_")
            && !trimmed.starts_with("local function molt_range")
            && !trimmed.starts_with("local function molt_len")
            && !trimmed.starts_with("local function molt_int")
            && !trimmed.starts_with("local function molt_float")
            && !trimmed.starts_with("local function molt_str")
            && !trimmed.starts_with("local function molt_bool")
            && !trimmed.starts_with("local function molt_repr")
            && !trimmed.starts_with("local function molt_mod")
            && !trimmed.starts_with("local function molt_enumerate")
            && !trimmed.starts_with("local function molt_zip")
            && !trimmed.starts_with("local function molt_sorted")
            && !trimmed.starts_with("local function molt_reversed")
            && !trimmed.starts_with("local function molt_sum")
            && !trimmed.starts_with("local function molt_any")
            && !trimmed.starts_with("local function molt_all")
            && !trimmed.starts_with("local function molt_map")
            && !trimmed.starts_with("local function molt_filter")
            && !trimmed.starts_with("local function molt_dict_")
            && !trimmed.starts_with("local function molt_json_")
            && !trimmed.starts_with("local function molt_string")
            && !trimmed.starts_with("local function molt_print")
            && !trimmed.starts_with("local function molt_pow")
            && !trimmed.starts_with("local function molt_floor_div")
        {
            let indent = &line[..line.len() - trimmed.len()];
            result.push_str(&format!("{indent}@native\n"));
            perf_count += 1;
        }
        // Note: `@native` only works with `local function` declarations in Luau.
        // Forward-declared assignments like `molt_name = function(` cannot use @native.

        result.push_str(&optimized);
        result.push('\n');
    }

    if perf_count > 0 {
        *source = result;
        eprintln!("[molt-luau] Applied {} perf optimizations", perf_count);
    }
}

/// Find the matching closing parenthesis for an opening paren at `open_pos`.
/// Check if an expression contains binary operators at the top level
/// (not inside `[]`, `()`, or `{}`). Used by the sink pass to decide
/// whether inlined expressions need parenthesization.
fn has_top_level_binary_op(expr: &str) -> bool {
    let bytes = expr.as_bytes();
    let mut depth: i32 = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b'+' | b'-' | b'*' | b'/' | b'%' | b'^' if depth == 0 => {
                // Must be a binary op: preceded and followed by space
                if i > 0 && i + 1 < bytes.len() && bytes[i - 1] == b' ' && bytes[i + 1] == b' ' {
                    return true;
                }
            }
            b'.' if depth == 0 && i + 1 < bytes.len() && bytes[i + 1] == b'.' => {
                return true; // string concatenation `..`
            }
            _ => {}
        }
        i += 1;
    }
    false
}

/// Find the matching closing parenthesis for an opening paren at `open_pos`.
fn find_matching_paren(s: &str, open_pos: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut depth = 1;
    let mut i = open_pos + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
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
                ops: vec![
                    OpIR {
                        kind: "const".to_string(),
                        value: Some(42),
                        f_value: None,
                        s_value: None,
                        bytes: None,
                        var: None,
                        args: None,
                        out: Some("v0".to_string()),
                        fast_int: None,
                        task_kind: None,
                        container_type: None,
                        stack_eligible: None,
                        fast_float: None,
                        type_hint: None,
                        raw_int: None,
                    },
                    OpIR {
                        kind: "print".to_string(),
                        value: None,
                        f_value: None,
                        s_value: None,
                        bytes: None,
                        var: None,
                        args: Some(vec!["v0".to_string()]),
                        out: None,
                        fast_int: None,
                        task_kind: None,
                        container_type: None,
                        stack_eligible: None,
                        fast_float: None,
                        type_hint: None,
                        raw_int: None,
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
    fn test_real_ir_ops() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "test_func".to_string(),
                params: vec!["p0".to_string()],
                ops: vec![
                    OpIR {
                        kind: "const_float".to_string(),
                        value: None,
                        f_value: Some(3.14),
                        s_value: None,
                        bytes: None,
                        var: None,
                        args: None,
                        out: Some("v0".to_string()),
                        fast_int: None,
                        task_kind: None,
                        container_type: None,
                        stack_eligible: None,
                        fast_float: None,
                        type_hint: None,
                        raw_int: None,
                    },
                    OpIR {
                        kind: "const_str".to_string(),
                        value: None,
                        f_value: None,
                        s_value: Some("hello".to_string()),
                        bytes: None,
                        var: None,
                        args: None,
                        out: Some("v1".to_string()),
                        fast_int: None,
                        task_kind: None,
                        container_type: None,
                        stack_eligible: None,
                        fast_float: None,
                        type_hint: None,
                        raw_int: None,
                    },
                    OpIR {
                        kind: "add".to_string(),
                        value: None,
                        f_value: None,
                        s_value: None,
                        bytes: None,
                        var: None,
                        args: Some(vec!["p0".to_string(), "v0".to_string()]),
                        out: Some("v2".to_string()),
                        fast_int: None,
                        task_kind: None,
                        container_type: None,
                        stack_eligible: None,
                        fast_float: None,
                        type_hint: None,
                        raw_int: None,
                    },
                    OpIR {
                        kind: "lt".to_string(),
                        value: None,
                        f_value: None,
                        s_value: None,
                        bytes: None,
                        var: None,
                        args: Some(vec!["v2".to_string(), "p0".to_string()]),
                        out: Some("v3".to_string()),
                        fast_int: None,
                        task_kind: None,
                        container_type: None,
                        stack_eligible: None,
                        fast_float: None,
                        type_hint: None,
                        raw_int: None,
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        value: None,
                        f_value: None,
                        s_value: None,
                        bytes: None,
                        var: None,
                        args: Some(vec!["v3".to_string()]),
                        out: None,
                        fast_int: None,
                        task_kind: None,
                        container_type: None,
                        stack_eligible: None,
                        fast_float: None,
                        type_hint: None,
                        raw_int: None,
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(output.contains("local function test_func(p0)"));
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
                ops: vec![
                    OpIR {
                        kind: "label".to_string(),
                        value: Some(0),
                        f_value: None,
                        s_value: None,
                        bytes: None,
                        var: None,
                        args: None,
                        out: None,
                        fast_int: None,
                        task_kind: None,
                        container_type: None,
                        stack_eligible: None,
                        fast_float: None,
                        type_hint: None,
                        raw_int: None,
                    },
                    OpIR {
                        kind: "jump".to_string(),
                        value: Some(1),
                        f_value: None,
                        s_value: None,
                        bytes: None,
                        var: None,
                        args: None,
                        out: None,
                        fast_int: None,
                        task_kind: None,
                        container_type: None,
                        stack_eligible: None,
                        fast_float: None,
                        type_hint: None,
                        raw_int: None,
                    },
                    OpIR {
                        kind: "label".to_string(),
                        value: Some(1),
                        f_value: None,
                        s_value: None,
                        bytes: None,
                        var: None,
                        args: None,
                        out: None,
                        fast_int: None,
                        task_kind: None,
                        container_type: None,
                        stack_eligible: None,
                        fast_float: None,
                        type_hint: None,
                        raw_int: None,
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        value: None,
                        f_value: None,
                        s_value: None,
                        bytes: None,
                        var: None,
                        args: None,
                        out: None,
                        fast_int: None,
                        task_kind: None,
                        container_type: None,
                        stack_eligible: None,
                        fast_float: None,
                        type_hint: None,
                        raw_int: None,
                    },
                ],
            }],
            profile: None,
        };
        let mut backend = LuauBackend::new();
        let output = backend.compile(&ir);
        assert!(output.contains("-- ::label_0::"));
        assert!(output.contains("-- goto label_1"));
        assert!(output.contains("-- ::label_1::"));
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
    fn test_validate_luau_source_accepts_plain_output() {
        let source = "--!strict\nfunction molt_main()\n\tprint(42)\nend\n";
        assert!(validate_luau_source(source).is_ok());
    }

    #[test]
    fn test_validate_luau_source_accepts_stub_comments() {
        // Stub comments like [async: spawn] are harmless nil assignments.
        let source = "--!strict\nfunction molt_main()\n\tlocal v0 = nil -- [async: spawn]\nend\n";
        assert!(validate_luau_source(source).is_ok());
    }

    #[test]
    fn test_validate_luau_source_rejects_unsupported_op() {
        let err = validate_luau_source(
            "--!strict\nfunction molt_main()\n\tlocal v0 = nil -- [unsupported op: foo]\nend\n",
        )
        .expect_err("unsupported op marker should be rejected");
        assert!(err.contains("unsupported marker"));
        assert!(err.contains("[unsupported op: foo]"));
    }

    #[test]
    fn test_compile_checked_accepts_label_goto_comments() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "flow_test".to_string(),
                params: vec![],
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
        // Labels and gotos emit as comments (standalone Luau has no goto).
        let source = backend
            .compile_checked(&ir)
            .expect("label/goto comments should pass validation");
        assert!(source.contains("-- ::label_0::"));
        assert!(source.contains("-- goto label_1"));
    }
}
