//! Dynamic obligations emitted once by the frontend descriptor projection.
//! This boundary does not execute IR or infer a second range lattice.
use super::*;

pub(super) struct AdmittedBindings<'py> {
    _pins: Vec<OwnedRuntimeValue<'py>>,
}

pub(super) fn admit<'py>(
    py: &'py PyToken,
    callable: u64,
    descriptor_bits: u64,
    desc: &mut RuntimeKernelDescriptor,
    args: &mut BTreeMap<String, RuntimeKernelArg<'py>>,
    grid: i64,
    threads: i64,
) -> Result<AdmittedBindings<'py>, u64> {
    let fail = |message: &str| raise_exception::<u64>(py, "RuntimeError", message);
    let mut pins = Vec::new();
    // All user-capable argument extraction has completed. Refresh through the
    // default field owner before checking callable/binding dependencies.
    for argument in args.values_mut() {
        if let RuntimeKernelArg::Buffer(buffer) = argument {
            refresh_buffer_fields(py, buffer, &mut pins)?;
        }
    }
    for argument in args.values() {
        if let RuntimeKernelArg::Buffer(buffer) = argument {
            admit_buffer_methods(py, desc, buffer, &mut pins)?;
        }
    }

    if !descriptor_is_current(callable, descriptor_bits, desc.code_slot) {
        return Err(fail(
            "GPU descriptor no longer describes the current callable code/shape",
        ));
    }
    let total = grid
        .checked_mul(threads)
        .filter(|v| *v > 0 && *v <= i64::from(i32::MAX))
        .ok_or_else(|| fail("GPU geometry exceeds the shader index representation"))?;
    let mut queries = BTreeMap::new();
    for binding in &desc.query_bindings {
        let mut path = binding.path.iter();
        let root = path
            .next()
            .ok_or_else(|| fail("GPU query has an empty binding path"))?;
        let mut selected =
            unsafe { OwnedRuntimeValue::from_owned_bits(py, kernel_global(callable, root)) };
        if exception_pending(py) {
            return Err(MoltObject::none().bits());
        }
        for name in path {
            let next = unsafe {
                OwnedRuntimeValue::from_owned_bits(py, module_binding(selected.bits(), name))
            };
            if exception_pending(py) {
                return Err(MoltObject::none().bits());
            }
            pins.push(selected);
            selected = next;
        }
        let kind = [
            "gpu_thread_id",
            "gpu_block_id",
            "gpu_block_dim",
            "gpu_grid_dim",
            "gpu_barrier",
        ]
        .into_iter()
        .find(|kind| intrinsic_matches(selected.bits(), &format!("molt_{kind}")))
        .ok_or_else(|| fail("GPU query binding is not a canonical native primitive"))?;
        if kind == "gpu_barrier" && binding.conditional {
            return Err(fail(
                "GPU collective requires unconditional workgroup participation",
            ));
        }
        if kind == "gpu_barrier" && desc.ops.iter().any(|op| op.args.contains(&binding.out)) {
            return Err(fail("GPU barrier has no numeric result"));
        }
        pins.push(selected);
        if queries.insert(binding.out.clone(), kind).is_some() {
            return Err(fail("duplicate GPU query output"));
        }
    }
    if field(&desc.numeric, "kind").map_err(|message| fail(&message))? != "strict_integral_i32" {
        return Err(fail("unknown GPU numeric certificate"));
    }
    if desc
        .numeric
        .get("single_thread")
        .and_then(JsonValue::as_bool)
        .ok_or_else(|| fail("GPU certificate lacks memory effect shape"))?
        && total != 1
    {
        return Err(fail("GPU memory coordinates require one logical thread"));
    }
    if total != 1 {
        for query in array(&desc.numeric, "memory_queries").map_err(|message| fail(&message))? {
            if query.as_str().and_then(|name| queries.get(name)).copied() != Some("gpu_thread_id") {
                return Err(fail(
                    "GPU memory effects are not proved independent across threads",
                ));
            }
        }
    }
    desc.query_magnitude = 0;
    for kind in queries.values() {
        let maximum = match *kind {
            "gpu_thread_id" => total - 1,
            "gpu_block_id" => grid - 1,
            "gpu_block_dim" => threads,
            "gpu_grid_dim" => grid,
            "gpu_barrier" => 0,
            _ => unreachable!(),
        };
        desc.query_magnitude = desc.query_magnitude.max(maximum as u64);
    }
    // All callback-capable argument extraction is complete. The following
    // checks, source rendering and device dispatch contain no Python callbacks.
    for requirement in &desc.requirements {
        validate_requirement(requirement, args, &queries, total, grid, threads)
            .map_err(|message| fail(&message))?;
    }
    for op in &mut desc.ops {
        if op.kind == "gpu_query" {
            let out = op
                .out
                .as_ref()
                .ok_or_else(|| fail("GPU query has no result"))?;
            op.kind = queries
                .get(out)
                .ok_or_else(|| fail("GPU query lacks a binding obligation"))?
                .to_string();
        }
        if op.kind == "const" && op.value.is_none_or(|v| i32::try_from(v).is_err()) {
            return Err(fail("GPU constant exceeds shader integer representation"));
        }
    }
    Ok(AdmittedBindings { _pins: pins })
}

fn field<'a>(value: &'a JsonValue, name: &str) -> Result<&'a str, String> {
    value
        .get(name)
        .and_then(JsonValue::as_str)
        .ok_or_else(|| format!("GPU obligation lacks {name}"))
}
fn array<'a>(value: &'a JsonValue, name: &str) -> Result<&'a [JsonValue], String> {
    value
        .get(name)
        .and_then(JsonValue::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("GPU obligation lacks {name}"))
}

fn coordinate(
    value: &JsonValue,
    args: &BTreeMap<String, RuntimeKernelArg<'_>>,
    queries: &BTreeMap<String, &str>,
    total: i64,
    grid: i64,
    threads: i64,
) -> Result<(i64, i64), String> {
    let exact = match field(value, "kind")? {
        "constant" => value
            .get("value")
            .and_then(JsonValue::as_i64)
            .ok_or("invalid GPU constant")?,
        "scalar" => match args.get(field(value, "name")?) {
            Some(RuntimeKernelArg::Int(value)) => *value,
            _ => return Err("GPU index/guard scalar must be an integer".into()),
        },
        "query" => {
            return match queries.get(field(value, "name")?).copied() {
                Some("gpu_thread_id") => Ok((0, total - 1)),
                Some("gpu_block_id") => Ok((0, grid - 1)),
                Some("gpu_block_dim") => Ok((threads, threads)),
                Some("gpu_grid_dim") => Ok((grid, grid)),
                _ => Err("GPU index/guard requires a geometry query".into()),
            };
        }
        _ => return Err("unsupported GPU index coordinate".into()),
    };
    i32::try_from(exact).map_err(|_| "GPU scalar exceeds shader integer representation")?;
    Ok((exact, exact))
}

fn validate_requirement(
    value: &JsonValue,
    args: &BTreeMap<String, RuntimeKernelArg<'_>>,
    queries: &BTreeMap<String, &str>,
    total: i64,
    grid: i64,
    threads: i64,
) -> Result<(), String> {
    match field(value, "kind")? {
        "bounds" => {
            let name = field(value, "buffer")?;
            let Some(RuntimeKernelArg::Buffer(buffer)) = args.get(name) else {
                return Err("indexed argument is not a buffer".into());
            };
            let index = value
                .get("index")
                .ok_or("GPU bounds obligation lacks an index")?;
            let (lower, mut upper) = coordinate(index, args, queries, total, grid, threads)?;
            for limit in array(value, "upper_limits")? {
                let (minimum, maximum) = coordinate(limit, args, queries, total, grid, threads)?;
                // Guard refinement uses only one launch-invariant bound. A
                // varying RHS needs compiler range facts, not a runtime guess.
                if minimum != maximum {
                    return Err("GPU guard bound must be launch-invariant".into());
                }
                upper = upper.min(maximum - 1);
            }
            if upper >= lower
                && (lower < 0 || usize::try_from(upper).ok().is_none_or(|v| v >= buffer.size))
            {
                return Err(format!("GPU bounds are unproved for buffer {name}"));
            }
        }
        _ => return Err("unknown GPU admission obligation".into()),
    }
    Ok(())
}

fn owned_lookup<'py>(py: &'py PyToken, bits: u64) -> Result<OwnedRuntimeValue<'py>, u64> {
    let value = unsafe { OwnedRuntimeValue::from_owned_bits(py, bits) };
    if exception_pending(py) {
        Err(MoltObject::none().bits())
    } else {
        Ok(value)
    }
}

fn refresh_buffer_fields<'py>(
    py: &'py PyToken,
    buffer: &mut RuntimeKernelBufferArg<'py>,
    pins: &mut Vec<OwnedRuntimeValue<'py>>,
) -> Result<(), u64> {
    let object = MoltObject::from_ptr(buffer.object_ptr).bits();
    let data = owned_lookup(py, default_field(object, "_data"))?;
    let format = owned_lookup(py, default_field(object, "_format_char"))?;
    let size = owned_lookup(py, default_field(object, "_size"))?;
    let itemsize = owned_lookup(py, default_field(object, "_itemsize"))?;
    let element = owned_lookup(py, default_field(object, "_element_type"))?;
    let fail = |message: &str| raise_exception::<u64>(py, "RuntimeError", message);
    let format_ptr = obj_from_bits(format.bits())
        .as_ptr()
        .ok_or_else(|| fail("GPU buffer format must be an exact string"))?;
    if !builtin_matches(unsafe { object_class_bits(format_ptr) }, "str") {
        return Err(fail("GPU buffer format must be an exact string"));
    }
    let format_text = string_obj_to_owned(obj_from_bits(format.bits()))
        .ok_or_else(|| fail("GPU buffer format must be an exact string"))?;
    let scalar = scalar_format_from_text(&format_text)
        .ok_or_else(|| fail("GPU buffer format is outside the hardware numeric capability"))?;
    let size_object = obj_from_bits(size.bits());
    if size_object.as_int().is_none()
        && !size_object.is_bool()
        && !size_object
            .as_ptr()
            .is_some_and(|ptr| builtin_matches(unsafe { object_class_bits(ptr) }, "int"))
    {
        return Err(fail(
            "GPU buffer size needs exact integer comparison semantics",
        ));
    }
    let size = parse_usize_arg(py, size.bits(), "_size")?;
    if obj_from_bits(itemsize.bits()).as_int() != Some(scalar.itemsize() as i64) {
        return Err(fail(
            "GPU buffer item size disagrees with its actual format",
        ));
    }
    let data_ptr = obj_from_bits(data.bits())
        .as_ptr()
        .ok_or_else(|| fail("GPU buffer storage must be exact bytes or bytearray"))?;
    let data_class = unsafe { object_class_bits(data_ptr) };
    if !builtin_matches(data_class, "bytes") && !builtin_matches(data_class, "bytearray") {
        return Err(fail("GPU buffer storage must be exact bytes or bytearray"));
    }
    // Retain the public setter conversion branch. The strict numeric admission
    // below additionally proves that its int/float conversion is exact.
    buffer.float_elements = builtin_matches(element.bits(), "float");
    // A prior extracted owner can have been detached by a later callback.
    // Retain it through dispatch; dropping it during final admission could run
    // a destructor and invalidate already-checked siblings.
    pins.push(std::mem::replace(&mut buffer.data, data));
    buffer.original_format = format_text;
    buffer.size = size;
    Ok(())
}

fn require_body<'py>(
    py: &'py PyToken,
    desc: &RuntimeKernelDescriptor,
    role: &str,
    bits: u64,
) -> Result<OwnedRuntimeValue<'py>, u64> {
    let value = owned_lookup(py, bits)?;
    if !desc
        .python_bodies
        .get(role)
        .is_some_and(|body| compiled_body_matches(value.bits(), body))
    {
        return Err(raise_exception::<u64>(
            py,
            "RuntimeError",
            &format!("GPU buffer dependency {role} no longer has its compiler-admitted body"),
        ));
    }
    Ok(value)
}

fn require_builtin<'py>(
    py: &'py PyToken,
    function: u64,
    name: &str,
) -> Result<OwnedRuntimeValue<'py>, u64> {
    let value = owned_lookup(py, function_binding(function, name))?;
    if !builtin_matches(value.bits(), name) {
        return Err(raise_exception::<u64>(
            py,
            "RuntimeError",
            &format!("GPU buffer builtin dependency {name} was rebound"),
        ));
    }
    Ok(value)
}

fn admit_buffer_methods<'py>(
    py: &'py PyToken,
    desc: &RuntimeKernelDescriptor,
    buffer: &RuntimeKernelBufferArg<'_>,
    pins: &mut Vec<OwnedRuntimeValue<'py>>,
) -> Result<(), u64> {
    let object = MoltObject::from_ptr(buffer.object_ptr).bits();
    let get = require_body(py, desc, "buffer_get", class_binding(object, "__getitem__"))?;
    let set = require_body(py, desc, "buffer_set", class_binding(object, "__setitem__"))?;
    for name in ["isinstance", "bytes", "bytearray", "float", "int"] {
        pins.push(require_builtin(py, set.bits(), name)?);
    }
    for (method, role, export) in [
        (get.bits(), "struct_unpack_from", "unpack_from"),
        (set.bits(), "struct_pack_into", "pack_into"),
    ] {
        let module = owned_lookup(py, function_binding(method, "struct"))?;
        let body = require_body(py, desc, role, module_binding(module.bits(), export))?;
        let normalize = require_body(
            py,
            desc,
            "struct_normalize_format",
            function_binding(body.bits(), "_normalize_format"),
        )?;
        pins.push(require_builtin(py, normalize.bits(), "isinstance")?);
        pins.push(require_builtin(py, normalize.bits(), "str")?);
        let dependencies: &[(&str, &str)] = if role == "struct_pack_into" {
            &[
                ("_MOLT_STRUCT_PACK", "molt_struct_pack"),
                ("_MOLT_STRUCT_PACK_INTO", "molt_struct_pack_into"),
            ]
        } else {
            &[("_MOLT_STRUCT_UNPACK_FROM", "molt_struct_unpack_from")]
        };
        for &(binding, symbol) in dependencies {
            let intrinsic = owned_lookup(py, function_binding(body.bits(), binding))?;
            if !intrinsic_matches(intrinsic.bits(), symbol) {
                return Err(raise_exception::<u64>(
                    py,
                    "RuntimeError",
                    "GPU buffer struct primitive was rebound",
                ));
            }
            pins.push(intrinsic);
        }
        pins.extend([module, body, normalize]);
    }
    pins.extend([get, set]);
    Ok(())
}
