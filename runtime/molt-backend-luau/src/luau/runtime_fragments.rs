use super::*;

// These are the executable providers. Selection, dependencies and binding
// hoists are derived from their source by runtime_prelude; there is no second
// handwritten export or dependency table.
pub(super) fn fragments() -> Vec<(&'static str, String)> {
    let mut fragments = Vec::new();
    let mut source = String::new();
    source.push_str("local molt_rawequal = rawequal\n");
    source.push_str(
        "local molt_func_attrs: {[any]: {[any]: any}} = setmetatable({}, {__mode = \"k\"})\n",
    );
    source.push_str("local molt_func_self_attr = {}\nlocal function molt_func_attr_get(func: any, name: any): any\n\tlocal attrs = molt_func_attrs[func]\n\tif attrs == nil then return nil end\n\tlocal value = rawget(attrs, name)\n\tif value == molt_func_self_attr then return func end\n\treturn value\nend\nlocal function molt_func_attr_set(func: any, name: any, value: any): nil\n\tlocal attrs = molt_func_attrs[func]\n\tif attrs == nil then attrs = {}; molt_func_attrs[func] = attrs end\n\trawset(attrs, name, if value == func then molt_func_self_attr else value)\n\treturn nil\nend\nlocal function molt_func_attr_del(func: any, name: any): nil\n\tlocal attrs = molt_func_attrs[func]\n\tif attrs ~= nil then rawset(attrs, name, nil) end\n\treturn nil\nend\n");
    source.push_str(
        "local molt_function_metadata: {[any]: any} = setmetatable({}, {__mode = \"k\"})\n",
    );
    source.push_str("local molt_code_slots: {[number]: any} = {}\n");
    source
            .push_str("local molt_sequence_length_key = {}\nlocal molt_sequence_kind_key = {}\nlocal function molt_sequence_len(sequence: {any}): number\n\tlocal packed = rawget(sequence, molt_sequence_length_key)\n\tif type(packed) == \"number\" then return packed end\n\treturn #sequence\nend\nlocal function molt_pack_sequence_kind(kind: string, ...): {any}\n\tlocal sequence = table.pack(...)\n\trawset(sequence, molt_sequence_length_key, sequence.n)\n\trawset(sequence, molt_sequence_kind_key, kind)\n\trawset(sequence, \"n\", nil)\n\treturn sequence\nend\nlocal function molt_pack_list(...): {any} return molt_pack_sequence_kind(\"list\", ...) end\nlocal function molt_pack_tuple(...): {any} return molt_pack_sequence_kind(\"tuple\", ...) end\nlocal function molt_function_register_signature(func: any, arg_names: {any}): nil\n\tmolt_function_metadata[func] = {arg_names = arg_names, posonly = 0, kwonly = molt_pack_tuple(), vararg = nil, varkw = nil, defaults = nil, kwdefaults = nil}\n\treturn nil\nend\n");
    source.push_str("local molt_module_cache: {[string]: any} = {\n\tmath = nil,\n\tjson = nil,\n\ttime = nil,\n\tos = nil,\n}\n\n");
    fragments.push(("base", std::mem::take(&mut source)));
    source.push_str(frame_runtime::FRAME_RUNTIME);
    fragments.push(("frames", std::mem::take(&mut source)));
    source.push_str("local function molt_function_set_builtin(func: any): any\n\tlocal metadata = molt_function_metadata[func]\n\tif metadata == nil then metadata = {}; molt_function_metadata[func] = metadata end\n\tmetadata.is_builtin = true\n\treturn func\nend\n\n");
    fragments.push(("builtin ABI", std::mem::take(&mut source)));
    source.push_str("local molt_binary_metadata: {[any]: any} = setmetatable({}, {__mode = \"k\"})\nlocal function molt_binary_new(kind: string, value: string): any local result = {}; molt_binary_metadata[result] = {kind=kind, value=value}; return result end\n");
    fragments.push(("binary", std::mem::take(&mut source)));
    source.push_str(dict_runtime::DICT_CORE_RUNTIME);
    fragments.push(("dictionary", std::mem::take(&mut source)));
    source.push_str(dict_runtime::EQUALITY_REPR_RUNTIME);
    fragments.push(("equality and repr", std::mem::take(&mut source)));
    source.push_str(dict_runtime::CALLARGS_RUNTIME);
    fragments.push(("call arguments", std::mem::take(&mut source)));
    source.push_str(concat!(
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
    fragments.push(("sys", std::mem::take(&mut source)));
    source.push_str(numeric_error_policy_generated::FLOAT_DIVMOD_LUA);
    fragments.push(("float divmod", std::mem::take(&mut source)));
    source.push_str("local molt_numeric_error_policy = {\n");
    for context in numeric_error_policy_generated::NumericErrorContext::ALL {
        let messages = [12, 13, 14].map(|minor| context.message(3, minor).expect("admitted target"));
        source.push_str(&format!(
            "\t[{:?}] = {{class={:?}, [12]={:?}, [13]={:?}, [14]={:?}}},\n",
            context.wire_key(), context.error_class(), messages[0], messages[1], messages[2]
        ));
    }
    source.push_str(concat!(
        "}\nlocal function molt_numeric_error(context)\n",
        "\tlocal row = molt_numeric_error_policy[context]\n",
        "\tlocal minor = molt_sys_version_info[2]\n",
        "\tif molt_sys_version_info[1] ~= 3 or row == nil or row[minor] == nil then error({__type=\"RuntimeError\", __msg=\"unsupported numeric exception target version\"}) end\n",
        "\terror({__type=row.class, __msg=row[minor]})\nend\n\n",
    ));
    fragments.push(("numeric error policy", std::mem::take(&mut source)));
    // These lifecycle intrinsics already have no-op semantics on this target.
    for stub in [
        "molt_init_sys",
        "molt_runtime_shutdown",
        "molt_runtime_init",
    ] {
        fragments.push((stub, format!("local function {stub}(...) end\n")));
    }
    let helpers: &[(&str, &str)] = &[
        (
            "molt_pack_sequence",
            "local function molt_pack_sequence(...): {any} return molt_pack_list(...) end\n",
        ),
        (
            "molt_range",
            "@native\nlocal function molt_range(start: number, stop: number, step: number?): {number}\n\tlocal s = step or 1\n\tlocal result = molt_pack_sequence_kind(\"range\")\n\tlocal n = 0\n\tlocal i = start\n\twhile (s > 0 and i < stop) or (s < 0 and i > stop) do\n\t\tn += 1\n\t\trawset(result, n, i)\n\t\ti += s\n\tend\n\trawset(result, molt_sequence_length_key, n)\n\treturn result\nend\n",
        ),
        (
            "molt_len",
            "local function molt_len(obj: any): number\n\tif type(obj) == \"string\" then local n = utf8.len(obj); if n == nil then error({__type=\"UnicodeDecodeError\", __msg=\"invalid UTF-8 string\"}) end; return n end\n\tif type(obj) == \"table\" then\n\t\tlocal binary = molt_binary_metadata[obj]; if binary ~= nil then return #binary.value end\n\t\tif molt_dict_is_ordered(obj) then return molt_dict_len(obj) end\n\t\tif molt_dict_view_is(obj) then return molt_dict_view_len(obj) end\n\t\tif molt_set_is(obj) then return molt_set_len(obj) end\n\t\tlocal packed = rawget(obj, molt_sequence_length_key)\n\t\tif type(packed) == \"number\" then return packed end\n\t\terror({__type=\"TypeError\", __msg=\"foreign Luau table has no deterministic Python length\"})\n\tend\n\terror(\"TypeError: object of type '\" .. type(obj) .. \"' has no len()\")\nend\n",
        ),
        (
            "molt_unpack_sequence",
            r#"local function molt_unpack_sequence(obj: any, expected: number, shape: string?): {any}
	local function fail_arity(actual: number): nil
		if actual < expected then
			error({__type="ValueError", __msg="not enough values to unpack (expected " .. tostring(expected) .. ", got " .. tostring(actual) .. ")"})
		end
		error({__type="ValueError", __msg="too many values to unpack (expected " .. tostring(expected) .. ")"})
	end
	local function finish(items: {any}, actual: number): {any}
		if actual ~= expected then fail_arity(actual) end
		rawset(items, molt_sequence_length_key, actual)
		return items
	end
	local function unpack_packed_sequence(sequence: {any}): {any}
		local packed = rawget(sequence, molt_sequence_length_key)
		if type(packed) == "number" then
			if packed ~= math.floor(packed) or packed < 0 then
				error({__type="TypeError", __msg="invalid packed sequence length"})
			end
			if packed ~= expected then fail_arity(packed) end
			local items = table.create(expected)
			for i = 1, expected do rawset(items, i, rawget(sequence, i)) end
			return finish(items, packed)
		end
		-- An ordinary Luau array cannot contain an explicit nil element. Probe
		-- only expected+1 slots; packed arrays above preserve Python None holes.
		local items = table.create(expected)
		local actual = 0
		while actual <= expected do
			local value = rawget(sequence, actual + 1)
			if value == nil then break end
			actual += 1
			if actual <= expected then rawset(items, actual, value) end
		end
		return finish(items, actual)
	end
	local function unpack_mapping_keys(mapping: {any}): {any}
		local actual = molt_dict_len(mapping)
		if actual ~= expected then fail_arity(actual) end
		return finish(molt_dict_view_snapshot(molt_dict_keys(mapping)), actual)
	end
	local function unpack_custom_iterable(iterable: any): {any}
		local items = table.create(expected)
		local actual = 0
		for value in iterable do
			actual += 1
			if actual <= expected then rawset(items, actual, value) end
			if actual > expected then break end
		end
		return finish(items, actual)
	end
	local kind = type(obj)
	if kind == "string" then
		local items = table.create(expected)
		local actual = 0
		for _, codepoint in utf8.codes(obj) do
			actual += 1
			if actual <= expected then rawset(items, actual, utf8.char(codepoint)) end
			if actual > expected then break end
		end
		return finish(items, actual)
	end
	if kind ~= "table" then
		local type_name = if kind == "nil" then "NoneType" else kind
		error({__type="TypeError", __msg="cannot unpack non-iterable " .. type_name .. " object"})
	end
	if shape == "sequence" then return unpack_packed_sequence(obj) end
	if shape == "mapping" then return unpack_mapping_keys(obj) end
	local mt = getmetatable(obj)
	if type(mt) == "table" and type(rawget(mt, "__iter")) == "function" then
		return unpack_custom_iterable(obj)
	end
	if molt_dict_is_ordered(obj) then return unpack_mapping_keys(obj) end
	if type(rawget(obj, molt_sequence_length_key)) == "number" or rawget(obj, 1) ~= nil or next(obj) == nil then
		return unpack_packed_sequence(obj)
	end
	error({__type="TypeError", __msg="unordered foreign Luau table is not a deterministic Python iterable"})
end
"#,
        ),
        (
            "molt_int",
            "local function molt_int(x: any): number\n\tlocal value = tonumber(x)\n\tif value == nil then error({__type=\"ValueError\", __msg=\"invalid literal for int()\"}) end\n\treturn math.floor(value)\nend\n",
        ),
        (
            "molt_float",
            "local function molt_float(x: any): number\n\tlocal value = tonumber(x)\n\tif value == nil then error({__type=\"ValueError\", __msg=\"could not convert string to float\"}) end\n\treturn value\nend\n",
        ),
        (
            "molt_bool",
            "local function molt_bool(x: any): boolean\n\tif x == nil or x == false or x == 0 or x == \"\" then return false end\n\tif type(x) == \"table\" then local binary = molt_binary_metadata[x]; if binary ~= nil then return #binary.value > 0 end; if molt_dict_is_ordered(x) then return molt_dict_len(x) > 0 end; if molt_dict_view_is(x) then return molt_dict_view_len(x) > 0 end; if molt_set_is(x) then return molt_set_len(x) > 0 end; local packed = rawget(x, molt_sequence_length_key); if type(packed) == \"number\" then return packed > 0 end end\n\treturn true\nend\n",
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
            "local function molt_class_lookup(cls: any, attr: any): any\n\tlocal current = cls\n\tlocal seen = {}\n\twhile type(current) == \"table\" and seen[current] ~= true do\n\t\tseen[current] = true\n\t\tlocal raw = rawget(current, attr)\n\t\tif raw ~= nil then return raw end\n\t\tlocal mt = getmetatable(current)\n\t\tif type(mt) ~= \"table\" or type(mt.__index) ~= \"table\" then return nil end\n\t\tcurrent = mt.__index\n\tend\n\treturn nil\nend\n\nlocal function molt_bind_attr(obj: any, owner: any, raw: any): any\n\tif type(raw) == \"table\" then\n\t\tlocal kind = raw.__molt_descriptor_kind\n\t\tif kind == \"staticmethod\" then return raw.__func end\n\t\tif kind == \"classmethod\" then return molt_bound_method_new(raw.__func, owner) end\n\t\tif kind == \"property\" then\n\t\t\tif obj == owner then return raw end\n\t\t\tlocal fget = raw.__get\n\t\t\tif fget == nil then error({__type=\"AttributeError\", __msg=\"unreadable attribute\"}) end\n\t\t\treturn molt_call_checked(fget, obj)\n\t\tend\n\tend\n\tif type(raw) == \"function\" and type(owner) == \"table\" and obj ~= owner then return molt_bound_method_new(raw, obj) end\n\treturn raw\nend\n\nlocal function molt_get_attr(obj: any, attr: any): any\n\tif type(obj) ~= \"table\" then return nil end\n\tif obj.__molt_is_type == true then\n\t\tlocal raw = molt_class_lookup(obj, attr)\n\t\tif raw ~= nil then return molt_bind_attr(obj, obj, raw) end\n\t\treturn nil\n\tend\n\tlocal own = rawget(obj, attr)\n\tif own ~= nil then return own end\n\tlocal cls = getmetatable(obj)\n\tif type(cls) == \"table\" then\n\t\tlocal raw = molt_class_lookup(cls, attr)\n\t\tif raw ~= nil then return molt_bind_attr(obj, cls, raw) end\n\tend\n\treturn obj[attr]\nend\n\nlocal function molt_get_attr_default(obj: any, attr: any, default: any): any\n\tlocal value = molt_get_attr(obj, attr)\n\tif value ~= nil then return value end\n\treturn default\nend\n\nlocal function molt_has_attr(obj: any, attr: any): boolean\n\tlocal ok, value = pcall(function() return molt_get_attr(obj, attr) end)\n\tif ok then return value ~= nil end\n\tif type(value) == \"table\" and value.__type == \"AttributeError\" then return false end\n\terror(value)\nend\n\nlocal function molt_set_attr(obj: any, attr: any, value: any): nil\n\tif type(obj) ~= \"table\" then return nil end\n\tif obj.__molt_is_type ~= true then\n\t\tlocal cls = getmetatable(obj)\n\t\tif type(cls) == \"table\" then\n\t\t\tlocal raw = molt_class_lookup(cls, attr)\n\t\t\tif type(raw) == \"table\" and raw.__molt_descriptor_kind == \"property\" then\n\t\t\t\tlocal fset = raw.__set\n\t\t\t\tif fset == nil then error({__type=\"AttributeError\", __msg=\"can't set attribute\"}) end\n\t\t\t\tmolt_call_checked(fset, obj, value)\n\t\t\t\treturn nil\n\t\t\tend\n\t\tend\n\tend\n\tobj[attr] = value\n\treturn nil\nend\n\nlocal function molt_del_attr(obj: any, attr: any): nil\n\tif type(obj) ~= \"table\" then return nil end\n\tif obj.__molt_is_type ~= true then\n\t\tlocal cls = getmetatable(obj)\n\t\tif type(cls) == \"table\" then\n\t\t\tlocal raw = molt_class_lookup(cls, attr)\n\t\t\tif type(raw) == \"table\" and raw.__molt_descriptor_kind == \"property\" then\n\t\t\t\tlocal fdel = raw.__del\n\t\t\t\tif fdel == nil then error({__type=\"AttributeError\", __msg=\"can't delete attribute\"}) end\n\t\t\t\tmolt_call_checked(fdel, obj)\n\t\t\t\treturn nil\n\t\t\tend\n\t\tend\n\tend\n\tobj[attr] = nil\n\treturn nil\nend\n",
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
            "local function molt_enumerate(t: {any}, start: number?): {{any}}\n\tlocal len = molt_sequence_len(t)\n\tlocal result = molt_pack_sequence_kind(\"iterator\")\n\tlocal s = start or 0\n\tfor i = 1, len do rawset(result, i, molt_pack_tuple(s + i - 1, rawget(t, i))) end\n\trawset(result, molt_sequence_length_key, len)\n\treturn result\nend\n",
        ),
        (
            "molt_zip",
            "local function molt_zip(a: {any}, b: {any}): {{any}}\n\tlocal result = molt_pack_sequence_kind(\"iterator\")\n\tlocal len = math.min(molt_sequence_len(a), molt_sequence_len(b))\n\tfor i = 1, len do rawset(result, i, molt_pack_tuple(rawget(a, i), rawget(b, i))) end\n\trawset(result, molt_sequence_length_key, len)\n\treturn result\nend\n",
        ),
        (
            "molt_sorted",
            "local function molt_sorted(t: {any}): {any}\n\tlocal count = molt_sequence_len(t)\n\tlocal copy = molt_pack_list()\n\tfor index = 1, count do rawset(copy, index, rawget(t, index)) end\n\trawset(copy, molt_sequence_length_key, count)\n\ttable.sort(copy)\n\treturn copy\nend\n",
        ),
        (
            "molt_reversed",
            "@native\nlocal function molt_reversed(t: {any}): {any}\n\tlocal len = molt_sequence_len(t)\n\tlocal result = molt_pack_sequence_kind(\"iterator\")\n\trawset(result, molt_sequence_length_key, len)\n\tfor i = 1, len do\n\t\trawset(result, i, rawget(t, len - i + 1))\n\tend\n\treturn result\nend\n",
        ),
        (
            "molt_sum",
            "@native\nlocal function molt_sum(t: {number}, start: number?): number\n\tlocal len = molt_sequence_len(t)\n\tlocal s = start or 0\n\tfor __i = 1, len do s += t[__i] end\n\treturn s\nend\n",
        ),
        (
            "molt_any",
            "local function molt_any(t: {any}): boolean\n\tlocal len = molt_sequence_len(t)\n\tfor __i = 1, len do\n\t\tif molt_bool(rawget(t, __i)) then return true end\n\tend\n\treturn false\nend\n",
        ),
        (
            "molt_all",
            "local function molt_all(t: {any}): boolean\n\tlocal len = molt_sequence_len(t)\n\tfor __i = 1, len do\n\t\tif not molt_bool(rawget(t, __i)) then return false end\n\tend\n\treturn true\nend\n",
        ),
        (
            "molt_map",
            "@native\nlocal function molt_map(func: (any) -> any, t: {any}): {any}\n\tlocal len = molt_sequence_len(t)\n\tlocal result = molt_pack_sequence_kind(\"iterator\")\n\trawset(result, molt_sequence_length_key, len)\n\tfor i = 1, len do\n\t\trawset(result, i, func(rawget(t, i)))\n\tend\n\treturn result\nend\n",
        ),
        (
            "molt_filter",
            "local function molt_filter(func: ((any) -> boolean)?, t: {any}): {any}\n\tlocal len = molt_sequence_len(t)\n\tlocal result = molt_pack_sequence_kind(\"iterator\")\n\tlocal n = 0\n\tfor __i = 1, len do\n\t\tlocal v = rawget(t, __i)\n\t\tif func then\n\t\t\tif func(v) then n += 1; rawset(result, n, v) end\n\t\telseif molt_bool(v) then\n\t\t\tn += 1; rawset(result, n, v)\n\t\tend\n\tend\n\trawset(result, molt_sequence_length_key, n)\n\treturn result\nend\n",
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
            "molt_get_attr_checked",
            "local function molt_get_attr_checked(obj: any, attr: any): any\n\tlocal value = molt_get_attr(obj, attr)\n\tif value == nil then error({__type=\"AttributeError\", __msg=tostring(attr)}) end\n\treturn value\nend\n",
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
    fragments.extend(
        helpers
            .iter()
            .map(|&(name, source)| (name, source.to_string())),
    );
    source.push_str("local molt_not_implemented = {__molt_not_implemented = true}\n");
    fragments.push(("not implemented", std::mem::take(&mut source)));
    source.push_str(concat!(
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
    source.push_str(concat!(
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
    source.push_str(concat!(
        "local function molt_exception_match(e: any, class_name: string): boolean\n",
        "\tlocal kind = molt_exception_kind(e)\n",
        "\twhile kind do\n",
        "\t\tif kind == class_name then return true end\n",
        "\t\tkind = molt_exception_hierarchy[kind]\n",
        "\tend\n",
        "\treturn false\n",
        "end\n\n",
    ));
    fragments.push(("exception matching", std::mem::take(&mut source)));
    source.push_str(frame_runtime::CALLABLE_FRAME_RUNTIME);
    fragments.push(("callable frames", std::mem::take(&mut source)));
    source.push_str(frame_runtime::CALLABLE_METADATA_RUNTIME);
    fragments.push(("callable metadata", std::mem::take(&mut source)));
    source.push_str(concat!(
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
                "\tlocal context = molt_frame_context()\n",
                "\tlocal active = context.depth > 0\n",
                "\tif active then module = context.globals[context.depth] end\n",
                "\tif type(module) ~= \"table\" then return molt_module_type_error(\"get_global\") end\n",
                "\tlocal present, value = molt_frame_namespace_get(module, name)\n",
                "\tif present then return value end\n",
                "\tlocal builtins = molt_frame_effective_builtins(if active then nil else module)\n",
                "\tlocal builtin_present, builtin_value = molt_frame_namespace_get(builtins, name)\n",
                "\tif builtin_present then return builtin_value end\n",
                "\treturn molt_module_name_error(name)\n",
                "end\n\n",
                "local function molt_module_get_name(module: any, name: any): any\n",
                "\tif type(module) ~= \"table\" then return molt_module_type_error(\"get_name\") end\n",
                "\tif name == \"__dict__\" then return module end\n",
                "\tlocal present, value = molt_frame_namespace_get(module, name)\n",
                "\tif present then return value end\n",
                "\terror({__type = \"AttributeError\", __msg = \"module has no attribute '\" .. tostring(name) .. \"'\"})\n",
                "end\n\n",
                "local function molt_module_del_global(module: any, name: any, missing_ok: boolean): any\n",
                "\tlocal context = molt_frame_context()\n",
                "\tif context.depth > 0 then module = context.globals[context.depth] end\n",
                "\tif type(module) ~= \"table\" then return molt_module_type_error(\"del_global\") end\n",
                "\tif molt_dict_is_ordered(module) and molt_dict_contains(module, name) then\n",
                "\t\tmolt_dict_delete(module, name, false)\n",
                "\t\treturn nil\n",
                "\tend\n",
                "\tif not molt_dict_is_ordered(module) and module[name] ~= nil then\n",
                "\t\tmodule[name] = nil\n",
                "\t\treturn nil\n",
                "\tend\n",
                "\tif missing_ok then return nil end\n",
                "\treturn molt_module_name_error(name)\n",
                "end\n\n",
            ));
    fragments.push(("module namespaces", std::mem::take(&mut source)));
    source.push_str("local math_floor = math.floor\n");
    fragments.push(("math floor", std::mem::take(&mut source)));
    source.push_str("local bit = bit32\n");
    fragments.push(("bit bridge", std::mem::take(&mut source)));
    source.push_str("local molt_missing_sentinel = {}\n");
    fragments.push(("missing sentinel", std::mem::take(&mut source)));
    source.push_str(concat!(
        "local molt_math = {\n",
        "\tfloor = math.floor,\n\tceil = math.ceil,\n\tsqrt = math.sqrt,\n",
        "\tabs = math.abs,\n\tsin = math.sin,\n\tcos = math.cos,\n",
        "\ttan = math.tan,\n\tasin = math.asin,\n\tacos = math.acos,\n",
        "\tatan = math.atan,\n\tatan2 = math.atan2,\n\texp = math.exp,\n",
        "\tlog = math.log,\n\tlog10 = function(x) return math.log(x, 10) end,\n\tpi = math.pi,\n",
        "\te = 2.718281828459045,\n\tinf = math.huge,\n\tnan = 0/0,\n",
        "}\n\n",
    ));
    source.push_str("molt_module_cache[\"math\"] = molt_math\n\n");
    fragments.push(("math module", std::mem::take(&mut source)));
    source.push_str(include_str!("../luau_json_prelude.luau"));
    source.push('\n');
    source.push_str("molt_module_cache[\"json\"] = json\n\n");
    fragments.push(("json module", std::mem::take(&mut source)));
    source.push_str(concat!(
        "local molt_time = {\n",
        "\ttime = os.clock,\n\tperf_counter = os.clock,\n",
        "\tmonotonic = os.clock,\n\tsleep = function(s: number) task.wait(s) end,\n",
        "}\n\n",
    ));
    source.push_str("molt_module_cache[\"time\"] = molt_time\n\n");
    fragments.push(("time module", std::mem::take(&mut source)));
    source.push_str(concat!(
                "local molt_os = {\n",
                "\tgetcwd = function() return \".\" end,\n",
                "\tgetenv = function(_k: string) error({__type=\"RuntimeError\", __msg=\"os.getenv is unavailable in the Luau backend\"}) end,\n",
                "\tpath = { join = function(...) local a = {...} return table.concat(a, \"/\") end,\n",
                "\t\texists = function(_path: string) error({__type=\"RuntimeError\", __msg=\"os.path.exists is unavailable in the Luau backend\"}) end, sep = \"/\" },\n",
                "}\n\n",
            ));
    source.push_str("molt_module_cache[\"os\"] = molt_os\n\n");
    fragments.push(("os module", std::mem::take(&mut source)));
    source.push_str(concat!(
                "local molt_string = {\n",
                "\tformat = string.format,\n",
                "\tjoin = function(sep: string, t: {string}): string\n\t\treturn table.concat(t, sep)\n\tend,\n",
                "\tsplit = function(s: string, sep: string?): {string}\n",
                "\t\tlocal result = molt_pack_list()\n\t\tlocal n = 0\n\t\tlocal pattern = sep and sep or \"%s+\"\n",
                "\t\tif sep then\n\t\t\tif sep == \"\" then error({__type=\"ValueError\", __msg=\"empty separator\"}) end\n\t\t\tlocal pos = 1\n\t\t\twhile pos <= #s do\n",
                "\t\t\t\tlocal i, j = string.find(s, pattern, pos, true)\n",
                "\t\t\t\tif i then\n\t\t\t\t\tn += 1; result[n] = string.sub(s, pos, i - 1)\n",
                "\t\t\t\t\tpos = j + 1\n\t\t\t\telse\n",
                "\t\t\t\t\tn += 1; result[n] = string.sub(s, pos)\n\t\t\t\t\tbreak\n",
                "\t\t\t\tend\n\t\t\tend\n\t\telse\n",
                "\t\t\tfor w in string.gmatch(s, \"%S+\") do\n\t\t\t\tn += 1; result[n] = w\n",
                "\t\t\tend\n\t\tend\n\t\trawset(result, molt_sequence_length_key, n)\n\t\treturn result\n\tend,\n",
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
    fragments.push(("string methods", std::mem::take(&mut source)));
    fragments
}
