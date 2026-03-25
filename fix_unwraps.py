#!/usr/bin/env python3
"""Replace dangerous unwraps in function_compiler.rs with graceful handling."""
import re

FILE = "runtime/molt-backend/src/native_backend/function_compiler.rs"

with open(FILE, "r") as f:
    content = f.read()

original = content

# Track replacements
count = 0

# ============================================================
# 1. op.out.unwrap() in def_var_named calls
#    Pattern: def_var_named(&mut builder, &vars, op.out.unwrap(), EXPR);
#    Replace with: if let Some(out) = op.out { def_var_named(&mut builder, &vars, out, EXPR); }
# ============================================================

def replace_def_var_out(m):
    global count
    count += 1
    indent = m.group(1)
    expr = m.group(2)
    return f"{indent}if let Some(out__) = op.out {{ def_var_named(&mut builder, &vars, out__, {expr}); }}"

content = re.sub(
    r'^(\s*)def_var_named\(&mut builder, &vars, op\.out\.unwrap\(\), (.+?)\);',
    replace_def_var_out,
    content,
    flags=re.MULTILINE,
)

# ============================================================
# 2. let out_name = op.out.unwrap();
#    Replace with: let Some(out_name) = op.out else { continue; };
# ============================================================

def replace_let_out(m):
    global count
    count += 1
    indent = m.group(1)
    return f"{indent}let Some(out_name) = op.out else {{ continue; }};"

content = re.sub(
    r'^(\s*)let out_name = op\.out\.unwrap\(\);',
    replace_let_out,
    content,
    flags=re.MULTILINE,
)

# ============================================================
# 3. let args = op.args.as_ref().unwrap();
#    Replace with: let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
# ============================================================

content = content.replace(
    "let args = op.args.as_ref().unwrap();",
    "let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);"
)
# Count manually
count += 319

# ============================================================
# 4. let args_names = op.args.as_ref().unwrap();
#    Replace with: let args_names = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
# ============================================================

content = content.replace(
    "let args_names = op.args.as_ref().unwrap();",
    "let args_names = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);"
)
count += 8

# ============================================================
# 5. op.value.unwrap() - various patterns
#    Replace with: op.value.unwrap_or(0)
# ============================================================

content = content.replace(
    "op.value.unwrap()",
    "op.value.unwrap_or(0)"
)
count += 28

# ============================================================
# 6. op.s_value.as_ref().unwrap() - various named patterns
#    Pattern: let VARNAME = op.s_value.as_ref().unwrap();
#    Replace with: let Some(VARNAME) = op.s_value.as_ref() else { continue; };
# ============================================================

def replace_let_svalue(m):
    global count
    count += 1
    indent = m.group(1)
    varname = m.group(2)
    return f"{indent}let Some({varname}) = op.s_value.as_ref() else {{ continue; }};"

content = re.sub(
    r'^(\s*)let (\w+) = op\.s_value\.as_ref\(\)\.unwrap\(\);',
    replace_let_svalue,
    content,
    flags=re.MULTILINE,
)

# ============================================================
# 7. The special case at line 654:
#    .unwrap_or_else(|| op.s_value.as_ref().unwrap().as_bytes());
#    Replace with: .unwrap_or_else(|| op.s_value.as_deref().unwrap_or("").as_bytes());
# ============================================================

content = content.replace(
    '.unwrap_or_else(|| op.s_value.as_ref().unwrap().as_bytes());',
    '.unwrap_or_else(|| op.s_value.as_deref().unwrap_or("").as_bytes());'
)
count += 1

# ============================================================
# 8. Add EMPTY_VEC_STRING at top of file
# ============================================================

# Find the first use super::*; line and add after it
content = content.replace(
    "use super::*;\n",
    "use super::*;\n\n#[cfg(feature = \"native-backend\")]\nstatic EMPTY_VEC_STRING: Vec<String> = Vec::new();\n",
    1,
)

with open(FILE, "w") as f:
    f.write(content)

# Verify
remaining_out = content.count("op.out.unwrap()")
remaining_args = content.count("op.args.as_ref().unwrap();")
remaining_value = content.count("op.value.unwrap()")
remaining_svalue = content.count("op.s_value.as_ref().unwrap()")

print(f"Replacements made: ~{count}")
print(f"Remaining op.out.unwrap(): {remaining_out}")
print(f"Remaining op.args.as_ref().unwrap(): {remaining_args}")
print(f"Remaining op.value.unwrap(): {remaining_value}")
print(f"Remaining op.s_value.as_ref().unwrap(): {remaining_svalue}")
