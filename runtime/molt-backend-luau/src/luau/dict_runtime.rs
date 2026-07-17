/// Canonical Python-dict representation for generated Luau.
///
/// Luau tables deliberately do not promise `pairs()` order and cannot store
/// `nil` as either a key or a value.  The runtime therefore keeps insertion
/// order, values, and size in weak-key side metadata and encodes Python `None`
/// with unreachable private sentinels.  The public table is therefore opaque:
/// raw Luau iteration cannot leak metadata or encoded values. Tombstones make
/// deletion O(1); bounded compaction keeps retained storage O(live entries)
/// amortized without re-hashing keys during internal iteration.
pub(super) const DICT_CORE_RUNTIME: &str = r#"local molt_dict_none_key = {}
local molt_dict_none_value = {}
local molt_dict_metadata = setmetatable({}, {__mode = "k"})
local molt_dict_view_metadata = setmetatable({}, {__mode = "k"})
local molt_dict_iterator_metadata = setmetatable({}, {__mode = "k"})
local molt_callargs_metadata = setmetatable({}, {__mode = "k"})
local molt_set_metadata = setmetatable({}, {__mode = "k"})
local molt_identity_hashes = setmetatable({}, {__mode = "k"})
local molt_identity_hash_next = 0
local molt_set_require: (any) -> any
local molt_set_is: (any) -> boolean
local molt_dict_is_ordered: (any) -> boolean
local molt_dict_view_is: (any) -> boolean
local molt_hashed_index_find: (any, any, number?) -> any

local function molt_hash_mix(hash: number, value: number): number
	return (hash * 33 + value) % 4294967296
end

local function molt_hash_string(seed: number, value: string): number
	-- Hashes select collision buckets only; molt_key_equal remains the decisive
	-- authority, so no string or numeric fingerprint can alias distinct keys.
	local hash = seed
	for index = 1, #value do hash = molt_hash_mix(hash, string.byte(value, index)) end
	return hash
end

local function molt_identity_hash(value: any): number
	local hash = molt_identity_hashes[value]
	if hash == nil then
		molt_identity_hash_next += 1
		hash = molt_hash_mix(2166136261, molt_identity_hash_next)
		molt_identity_hashes[value] = hash
	end
	return hash
end

local function molt_key_has_custom_hash_or_equality(value: any): boolean
	if rawget(value, "__eq__") ~= nil or rawget(value, "__hash__") ~= nil then return true end
	local current = getmetatable(value)
	local seen = {}
	while type(current) == "table" and not seen[current] do
		seen[current] = true
		if rawget(current, "__eq__") ~= nil or rawget(current, "__hash__") ~= nil then return true end
		local next_class = rawget(current, "__index")
		current = if type(next_class) == "table" then getmetatable(next_class) else nil
	end
	return false
end

local function molt_key_hash(value: any, active: any?): number
	if value == nil then return 2654435761 end
	local kind = type(value)
	if kind == "boolean" then value = if value then 1 else 0; kind = "number" end
	if kind == "number" then
		if value ~= value then error({__type="TypeError", __msg="NaN keys are unsupported on the Luau target because Luau numbers do not preserve Python NaN identity"}) end
		if value == 0 then value = 0 end
		return molt_hash_string(2246822519, tostring(value))
	end
	if kind == "string" then return molt_hash_string(3266489917, value) end
	if kind == "function" then return molt_identity_hash(value) end
	if kind ~= "table" then error({__type="TypeError", __msg="unsupported Python key type on the Luau target: " .. kind}) end
	local binary = molt_binary_metadata[value]
	if binary ~= nil then
		if binary.kind ~= "bytes" then error({__type="TypeError", __msg="unhashable type: 'bytearray'"}) end
		return molt_hash_string(668265263, binary.value)
	end
	local sequence_kind = rawget(value, molt_sequence_kind_key)
	if sequence_kind == "tuple" then
		local seen = active or {}
		if seen[value] then error({__type="TypeError", __msg="unhashable recursive tuple"}) end
		seen[value] = true
		local hash = 3432918353
		for index = 1, molt_sequence_len(value) do hash = molt_hash_mix(hash, molt_key_hash(rawget(value, index), seen)) end
		seen[value] = nil
		return molt_hash_mix(hash, molt_sequence_len(value))
	end
	if molt_set_is(value) then
		local metadata = molt_set_require(value)
		if metadata.kind ~= "frozenset" then error({__type="TypeError", __msg="unhashable type: 'set'"}) end
		if metadata.frozen ~= true then error({__type="RuntimeError", __msg="cannot hash frozenset before construction is complete"}) end
		if metadata.cached_hash ~= nil then return metadata.cached_hash end
		local sum = 1927868237
		local xor_hash = 0
		for index = 1, #metadata.order do
			local entry_id = rawget(metadata.order, index)
			if entry_id ~= 0 then
				local item_hash = rawget(metadata.records, (entry_id - 1) * 7 + 2)
				sum = (sum + item_hash) % 4294967296
				xor_hash = bit32.bxor(xor_hash, item_hash)
			end
		end
		metadata.cached_hash = molt_hash_mix(sum, xor_hash)
		metadata.hash_locked = true
		return metadata.cached_hash
	end
	if sequence_kind == "range" then error({__type="TypeError", __msg="range keys require a value-semantic range representation unavailable on the Luau target"}) end
	if sequence_kind ~= nil or molt_dict_is_ordered(value) or molt_dict_view_is(value) then error({__type="TypeError", __msg="unhashable container type"}) end
	if molt_key_has_custom_hash_or_equality(value) then error({__type="TypeError", __msg="custom __hash__/__eq__ keys require boxed Python dispatch unavailable on the Luau target"}) end
	return molt_identity_hash(value)
end

local function molt_dict_public_key(key: any): any
	if key == molt_dict_none_key then return nil end
	return key
end

local function molt_key_storage(key: any): any
	if key == nil then return molt_dict_none_key end
	return key
end

local function molt_key_equal(left: any, right: any): boolean
	if left == right then return true end
	local left_kind = type(left)
	local right_kind = type(right)
	if (left_kind == "boolean" or left_kind == "number") and (right_kind == "boolean" or right_kind == "number") then
		local left_number = if left_kind == "boolean" then (if left then 1 else 0) else left
		local right_number = if right_kind == "boolean" then (if right then 1 else 0) else right
		return left_number == right_number
	end
	if left_kind ~= "table" or right_kind ~= "table" then return false end
	local left_binary = molt_binary_metadata[left]
	local right_binary = molt_binary_metadata[right]
	if left_binary ~= nil or right_binary ~= nil then
		return left_binary ~= nil and right_binary ~= nil and left_binary.kind == right_binary.kind and left_binary.value == right_binary.value
	end
	local left_sequence = rawget(left, molt_sequence_kind_key)
	local right_sequence = rawget(right, molt_sequence_kind_key)
	if left_sequence == "tuple" or right_sequence == "tuple" then
		if left_sequence ~= "tuple" or right_sequence ~= "tuple" then return false end
		local count = molt_sequence_len(left)
		if count ~= molt_sequence_len(right) then return false end
		for index = 1, count do if not molt_key_equal(rawget(left, index), rawget(right, index)) then return false end end
		return true
	end
	local left_is_set = molt_set_is(left)
	local right_is_set = molt_set_is(right)
	if left_is_set or right_is_set then
		if not left_is_set or not right_is_set then return false end
		local left_metadata = molt_set_require(left)
		local right_metadata = molt_set_require(right)
		if left_metadata.kind ~= "frozenset" or right_metadata.kind ~= "frozenset" or left_metadata.size ~= right_metadata.size then return false end
		for index = 1, #left_metadata.order do
			local entry_id = rawget(left_metadata.order, index)
			if entry_id ~= 0 and molt_hashed_index_find(right_metadata, molt_dict_public_key(rawget(left_metadata.records, (entry_id - 1) * 7 + 1)), rawget(left_metadata.records, (entry_id - 1) * 7 + 2)) == 0 then return false end
		end
		return true
	end
	return false
end

local function molt_hashed_index_new(): any
	return {heads = {}, records = {}, free = 0, next_id = 0}
end

molt_hashed_index_find = function(index: any, key: any, known_hash: number?): any
	local hash = known_hash or molt_key_hash(key)
	local entry_id = rawget(index.heads, hash) or 0
	while entry_id ~= 0 do
		local base = (entry_id - 1) * 7
		if molt_key_equal(molt_dict_public_key(rawget(index.records, base + 1)), key) then return entry_id end
		entry_id = rawget(index.records, base + 3) or 0
	end
	return 0
end

local function molt_hashed_index_insert(index: any, key: any, slot: number): any
	local hash = molt_key_hash(key)
	local entry_id = index.free
	if entry_id ~= 0 then index.free = rawget(index.records, (entry_id - 1) * 7 + 6) or 0
	else index.next_id += 1; entry_id = index.next_id end
	local base = (entry_id - 1) * 7
	local head = rawget(index.heads, hash) or 0
	rawset(index.records, base + 1, molt_key_storage(key))
	rawset(index.records, base + 2, hash)
	rawset(index.records, base + 3, head)
	rawset(index.records, base + 4, 0)
	rawset(index.records, base + 5, slot)
	rawset(index.records, base + 6, 0)
	rawset(index.records, base + 7, 0)
	if head ~= 0 then rawset(index.records, (head - 1) * 7 + 4, entry_id) end
	rawset(index.heads, hash, entry_id)
	return entry_id
end

local function molt_hashed_index_delete(index: any, entry_id: number): number
	local base = (entry_id - 1) * 7
	local hash = rawget(index.records, base + 2)
	local previous = rawget(index.records, base + 4) or 0
	local following = rawget(index.records, base + 3) or 0
	if previous == 0 then rawset(index.heads, hash, if following == 0 then nil else following)
	else rawset(index.records, (previous - 1) * 7 + 3, following) end
	if following ~= 0 then rawset(index.records, (following - 1) * 7 + 4, previous) end
	local slot = rawget(index.records, base + 5)
	for offset = 1, 5 do rawset(index.records, base + offset, 0) end
	rawset(index.records, base + 7, 0)
	rawset(index.records, base + 6, index.free); index.free = entry_id
	return slot
end

local function molt_dict_storage_value(value: any): any
	if value == nil then return molt_dict_none_value end
	return value
end

local function molt_dict_public_value(value: any): any
	if value == molt_dict_none_value then return nil end
	return value
end

molt_dict_is_ordered = function(dict: any): boolean
	return type(dict) == "table" and molt_dict_metadata[dict] ~= nil
end

local function molt_dict_require(dict: any): any
	if type(dict) ~= "table" then
		error({__type="TypeError", __msg="dict operation requires a mapping"})
	end
	local metadata = molt_dict_metadata[dict]
	if metadata == nil then
		error({__type="TypeError", __msg="unordered foreign Luau table cannot enter Python dict semantics"})
	end
	return metadata
end

local function molt_dict_new(): {[any]: any}
	local dict = {}
	local metadata = molt_hashed_index_new()
	metadata.size = 0; metadata.version = 0; metadata.order = {}
	molt_dict_metadata[dict] = metadata
	return dict
end

local function molt_order_compact(metadata: any): nil
	local old_order = metadata.order
	if #old_order <= metadata.size * 2 + 32 and metadata.next_id <= metadata.size * 2 + 32 then return nil end
	local old_records = metadata.records
	local heads = {}
	local records = table.create(metadata.size * 7)
	local order = table.create(metadata.size)
	local count = 0
	for index = 1, #old_order do
		local old_entry_id = rawget(old_order, index)
		if old_entry_id ~= 0 then
			count += 1
			local old_base = (old_entry_id - 1) * 7
			local new_base = (count - 1) * 7
			local hash = rawget(old_records, old_base + 2)
			local head = rawget(heads, hash) or 0
			rawset(records, new_base + 1, rawget(old_records, old_base + 1))
			rawset(records, new_base + 2, hash)
			rawset(records, new_base + 3, head)
			rawset(records, new_base + 4, 0)
			rawset(records, new_base + 5, count)
			rawset(records, new_base + 6, 0)
			rawset(records, new_base + 7, rawget(old_records, old_base + 7))
			if head ~= 0 then rawset(records, (head - 1) * 7 + 4, count) end
			rawset(heads, hash, count)
			rawset(order, count, count)
		end
	end
	metadata.heads = heads; metadata.records = records; metadata.order = order
	metadata.free = 0; metadata.next_id = count
	return nil
end

local function molt_dict_len(dict: {[any]: any}): number
	return molt_dict_require(dict).size
end

local function molt_dict_contains(dict: {[any]: any}, key: any): boolean
	return molt_hashed_index_find(molt_dict_require(dict), key) ~= 0
end

local function molt_dict_get(dict: {[any]: any}, key: any, default: any): any
	local metadata = molt_dict_require(dict)
	local entry_id = molt_hashed_index_find(metadata, key)
	if entry_id == 0 then return default end
	return molt_dict_public_value(rawget(metadata.records, (entry_id - 1) * 7 + 7))
end

local function molt_dict_getitem(dict: {[any]: any}, key: any): any
	local metadata = molt_dict_require(dict)
	local entry_id = molt_hashed_index_find(metadata, key)
	if entry_id == 0 then
		error({__type="KeyError", __msg=tostring(key)})
	end
	return molt_dict_public_value(rawget(metadata.records, (entry_id - 1) * 7 + 7))
end

local function molt_dict_set(dict: {[any]: any}, key: any, value: any): nil
	local metadata = molt_dict_require(dict)
	local entry_id = molt_hashed_index_find(metadata, key)
	if entry_id == 0 then
		local slot = #metadata.order + 1
		entry_id = molt_hashed_index_insert(metadata, key, slot)
		rawset(metadata.order, slot, entry_id)
		metadata.size += 1
		metadata.version += 1
	end
	rawset(metadata.records, (entry_id - 1) * 7 + 7, molt_dict_storage_value(value))
	return nil
end

local function molt_dict_delete(dict: {[any]: any}, key: any, missing_ok: boolean?): boolean
	local metadata = molt_dict_require(dict)
	local entry_id = molt_hashed_index_find(metadata, key)
	if entry_id == 0 then
		if missing_ok == true then return false end
		error({__type="KeyError", __msg=tostring(key)})
	end
	local slot = molt_hashed_index_delete(metadata, entry_id)
	rawset(metadata.order, slot, 0)
	metadata.size -= 1
	metadata.version += 1
	molt_order_compact(metadata)
	return true
end

local function molt_dict_clear(dict: {[any]: any}): nil
	local metadata = molt_dict_require(dict)
	if metadata.size ~= 0 then metadata.version += 1 end
	metadata.heads = {}; metadata.records = {}; metadata.free = 0; metadata.next_id = 0
	metadata.order = {}
	metadata.size = 0
	return nil
end

local function molt_dict_setdefault(dict: {[any]: any}, key: any, default: any): any
	if molt_dict_contains(dict, key) then return molt_dict_getitem(dict, key) end
	molt_dict_set(dict, key, default)
	return default
end

local function molt_dict_pop(dict: {[any]: any}, key: any, has_default: boolean, default: any): any
	if not molt_dict_contains(dict, key) then
		if has_default then return default end
		error({__type="KeyError", __msg=tostring(key)})
	end
	local value = molt_dict_getitem(dict, key)
	molt_dict_delete(dict, key, false)
	return value
end

local function molt_dict_scan(dict: {[any]: any}, visit: (any, any) -> ()): nil
	local metadata = molt_dict_require(dict)
	local order = metadata.order
	for index = 1, #order do
		local entry_id = rawget(order, index)
		if entry_id ~= 0 then
			visit(molt_dict_public_key(rawget(metadata.records, (entry_id - 1) * 7 + 1)), molt_dict_public_value(rawget(metadata.records, (entry_id - 1) * 7 + 7)))
		end
	end
	return nil
end

local function molt_dict_view_new(dict: {[any]: any}, kind: string): {any}
	molt_dict_require(dict)
	local view = {}
	molt_dict_view_metadata[view] = {dict = dict, kind = kind}
	return view
end

molt_dict_view_is = function(value: any): boolean
	return type(value) == "table" and molt_dict_view_metadata[value] ~= nil
end

local function molt_dict_view_len(view: {any}): number
	local state = molt_dict_view_metadata[view]
	if state == nil then error({__type="TypeError", __msg="invalid dict view"}) end
	return molt_dict_len(state.dict)
end

local function molt_dict_view_contains(view: {any}, needle: any): boolean
	local state = molt_dict_view_metadata[view]
	if state == nil then error({__type="TypeError", __msg="invalid dict view"}) end
	if state.kind == "keys" then return molt_dict_contains(state.dict, needle) end
	if state.kind == "items" then
		if type(needle) ~= "table" or rawget(needle, molt_sequence_kind_key) ~= "tuple" or molt_sequence_len(needle) ~= 2 then return false end
		local key = rawget(needle, 1)
		return molt_dict_contains(state.dict, key) and molt_equal(molt_dict_getitem(state.dict, key), rawget(needle, 2))
	end
	local found = false
	molt_dict_scan(state.dict, function(_key, value) if molt_equal(value, needle) then found = true end end)
	return found
end

local function molt_dict_keys(dict: {[any]: any}): {any}
	return molt_dict_view_new(dict, "keys")
end

local function molt_dict_values(dict: {[any]: any}): {any}
	return molt_dict_view_new(dict, "values")
end

local function molt_dict_items(dict: {[any]: any}): {any}
	return molt_dict_view_new(dict, "items")
end

local function molt_dict_view_snapshot(view: {any}): {any}
	local state = molt_dict_view_metadata[view]
	if state == nil then error({__type="TypeError", __msg="invalid dict view"}) end
	local result = molt_pack_list()
	local count = 0
	molt_dict_scan(state.dict, function(key, value)
		count += 1
		local item = if state.kind == "keys" then key elseif state.kind == "values" then value else molt_pack_tuple(key, value)
		rawset(result, count, item)
	end)
	rawset(result, molt_sequence_length_key, count)
	return result
end

local function molt_dict_iterator_new(value: any, kind: string?): {any}
	local dict = value
	local selected_kind = kind or "keys"
	local view_state = if molt_dict_view_is(value) then molt_dict_view_metadata[value] else nil
	if view_state ~= nil then dict = view_state.dict; selected_kind = view_state.kind end
	local metadata = molt_dict_require(dict)
	local iterator = {}
	molt_dict_iterator_metadata[iterator] = {dict = dict, kind = selected_kind, index = 0, version = metadata.version}
	return iterator
end

local function molt_dict_iterator_next(iterator: {any}): {any}
	local state = molt_dict_iterator_metadata[iterator]
	if state == nil then error({__type="TypeError", __msg="invalid dict iterator"}) end
	local metadata = molt_dict_require(state.dict)
	if metadata.version ~= state.version then
		error({__type="RuntimeError", __msg="dictionary changed size during iteration"})
	end
	local order = metadata.order
	local index = state.index + 1
	while index <= #order do
		local entry_id = rawget(order, index)
		state.index = index
		if entry_id ~= 0 then
				local key = molt_dict_public_key(rawget(metadata.records, (entry_id - 1) * 7 + 1))
				local value = molt_dict_public_value(rawget(metadata.records, (entry_id - 1) * 7 + 7))
				local item = if state.kind == "keys" then key elseif state.kind == "values" then value else molt_pack_tuple(key, value)
				return molt_pack_tuple(item, false)
		end
		index += 1
	end
	return molt_pack_tuple(nil, true)
end

local function molt_iterator_new(iterable: any): () -> {any}
	if molt_dict_is_ordered(iterable) or molt_dict_view_is(iterable) then
		local state = molt_dict_iterator_new(iterable, "keys")
		return function() return molt_dict_iterator_next(state) end
	end
	if type(iterable) == "table" and rawget(iterable, molt_sequence_kind_key) ~= nil then
		local index = 0
		local count = molt_sequence_len(iterable)
		return function()
			index += 1
			if index <= count then return molt_pack_tuple(rawget(iterable, index), false) end
			return molt_pack_tuple(nil, true)
		end
	end
	if molt_set_is(iterable) then
		local index = 0
		local metadata = molt_set_require(iterable)
		local version = metadata.version
		return function()
			if metadata.version ~= version then error({__type="RuntimeError", __msg="set changed size during iteration"}) end
			while index < #metadata.order do
				index += 1
				local entry_id = rawget(metadata.order, index)
				if entry_id ~= 0 then return molt_pack_tuple(molt_dict_public_key(rawget(metadata.records, (entry_id - 1) * 7 + 1)), false) end
			end
			return molt_pack_tuple(nil, true)
		end
	end
	if type(iterable) == "string" then
		local offset = 1
		return function()
			if offset > #iterable then return molt_pack_tuple(nil, true) end
			local next_offset = utf8.offset(iterable, 2, offset) or (#iterable + 1)
			local value = string.sub(iterable, offset, next_offset - 1)
			offset = next_offset
			return molt_pack_tuple(value, false)
		end
	end
	error({__type="TypeError", __msg="object is not a deterministic Python iterable"})
end

local function molt_dict_update(dict: {[any]: any}, other: {[any]: any}): nil
	molt_dict_require(dict)
	molt_dict_scan(other, function(key, value) molt_dict_set(dict, key, value) end)
	return nil
end

local function molt_dict_update_missing(dict: {[any]: any}, key: any, value: any, missing: any): nil
	if value == missing then molt_dict_delete(dict, key, true) else molt_dict_set(dict, key, value) end
	return nil
end

local function molt_dict_update_kwstar(dict: {[any]: any}, other: {[any]: any}): nil
	molt_dict_require(dict)
	molt_dict_scan(other, function(key, value)
		if type(key) ~= "string" then error({__type="TypeError", __msg="keywords must be strings"}) end
		molt_dict_set(dict, key, value)
	end)
	return nil
end

local function molt_dict_copy(source: {[any]: any}): {[any]: any}
	local result = molt_dict_new()
	molt_dict_update(result, source)
	return result
end

local function molt_dict_from_obj(source: any): {[any]: any}
	if not molt_dict_is_ordered(source) then
		error({__type="TypeError", __msg="dict() requires an ordered Molt mapping on the Luau target"})
	end
	return molt_dict_copy(source)
end

local function molt_dict_popitem(dict: {[any]: any}): {any}
	local metadata = molt_dict_require(dict)
	if metadata.size == 0 then
		error({__type="KeyError", __msg="popitem(): dictionary is empty"})
	end
	local index = #metadata.order
	while index > 0 and rawget(metadata.order, index) == 0 do
		rawset(metadata.order, index, nil)
		index -= 1
	end
	local entry_id = rawget(metadata.order, index)
	local key = molt_dict_public_key(rawget(metadata.records, (entry_id - 1) * 7 + 1))
	local value = molt_dict_public_value(rawget(metadata.records, (entry_id - 1) * 7 + 7))
	molt_dict_delete(dict, key, false)
	return molt_pack_tuple(key, value)
end

local function molt_dict_inc(dict: {[any]: any}, key: any, delta: any): any
	local current = molt_dict_get(dict, key, 0)
	local value = current + delta
	molt_dict_set(dict, key, value)
	return value
end

local function molt_dict_setdefault_empty_list(dict: {[any]: any}, key: any): any
	if molt_dict_contains(dict, key) then return molt_dict_getitem(dict, key) end
	local value = molt_pack_list()
	molt_dict_set(dict, key, value)
	return value
end

molt_set_require = function(value: any): any
	if type(value) ~= "table" or molt_set_metadata[value] == nil then
		error({__type="TypeError", __msg="set operation requires a canonical Molt set"})
	end
	return molt_set_metadata[value]
end

local function molt_set_new(kind: string): {any}
	local value = {}
	local metadata = molt_hashed_index_new()
	metadata.kind = kind; metadata.order = {}; metadata.size = 0; metadata.version = 0
	molt_set_metadata[value] = metadata
	return value
end

molt_set_is = function(value: any): boolean
	return type(value) == "table" and molt_set_metadata[value] ~= nil
end

local function molt_set_contains(set_value: any, value: any): boolean
	return molt_hashed_index_find(molt_set_require(set_value), value) ~= 0
end

local function molt_set_insert(set_value: any, value: any, construction: boolean): nil
	local metadata = molt_set_require(set_value)
	if metadata.kind == "frozenset" then
		if not construction then error({__type="AttributeError", __msg="frozenset is immutable"}) end
		if metadata.frozen == true or metadata.hash_locked == true then
			error({__type="RuntimeError", __msg="cannot mutate frozenset after construction is complete"})
		end
	end
	if molt_hashed_index_find(metadata, value) == 0 then
		local slot = #metadata.order + 1
		local entry_id = molt_hashed_index_insert(metadata, value, slot)
		rawset(metadata.order, slot, entry_id)
		metadata.size += 1
		metadata.version += 1
		metadata.cached_hash = nil
	end
	return nil
end

local function molt_set_add(set_value: any, value: any): nil return molt_set_insert(set_value, value, false) end
local function molt_frozenset_build_add(set_value: any, value: any): nil return molt_set_insert(set_value, value, true) end

local function molt_set_freeze(set_value: any): {any}
	local metadata = molt_set_require(set_value)
	metadata.kind = "frozenset"
	metadata.frozen = true
	metadata.cached_hash = nil
	molt_key_hash(set_value)
	return set_value
end

local function molt_set_len(set_value: any): number return molt_set_require(set_value).size end

local function molt_set_discard(set_value: any, value: any, missing_ok: boolean): boolean
	local metadata = molt_set_require(set_value)
	if metadata.kind == "frozenset" then error({__type="AttributeError", __msg="frozenset is immutable"}) end
	local entry_id = molt_hashed_index_find(metadata, value)
	if entry_id == 0 then
		if missing_ok then return false end
		error({__type="KeyError", __msg=tostring(value)})
	end
	local slot = molt_hashed_index_delete(metadata, entry_id)
	rawset(metadata.order, slot, 0)
	metadata.size -= 1
	metadata.version += 1
	molt_order_compact(metadata)
	return true
end

local function molt_set_clear(set_value: any): nil
	local metadata = molt_set_require(set_value)
	if metadata.kind == "frozenset" then error({__type="AttributeError", __msg="frozenset is immutable"}) end
	if metadata.size ~= 0 then metadata.version += 1 end
	metadata.heads = {}; metadata.records = {}; metadata.free = 0; metadata.next_id = 0
	metadata.order = {}
	metadata.size = 0
	return nil
end

local function molt_set_pop(set_value: any): any
	local metadata = molt_set_require(set_value)
	if metadata.kind == "frozenset" then error({__type="AttributeError", __msg="frozenset is immutable"}) end
	if metadata.size == 0 then error({__type="KeyError", __msg="pop from an empty set"}) end
	local slot = 1
	while rawget(metadata.order, slot) == 0 do slot += 1 end
	local entry_id = rawget(metadata.order, slot)
	local value = molt_dict_public_key(rawget(metadata.records, (entry_id - 1) * 7 + 1))
	molt_set_discard(set_value, value, false)
	return value
end

local function molt_set_scan(set_value: any, visit: (any) -> ()): nil
	local metadata = molt_set_require(set_value)
	for index = 1, #metadata.order do
		local entry_id = rawget(metadata.order, index)
		if entry_id ~= 0 then visit(molt_dict_public_key(rawget(metadata.records, (entry_id - 1) * 7 + 1))) end
	end
	return nil
end

local function molt_set_update(set_value: any, other: any): nil
	if molt_set_is(other) then molt_set_scan(other, function(value) molt_set_add(set_value, value) end); return nil end
	local iterator = molt_iterator_new(other)
	while true do
		local step = iterator()
		if rawget(step, 2) then break end
		molt_set_add(set_value, rawget(step, 1))
	end
	return nil
end

"#;

pub(super) const CALLARGS_RUNTIME: &str = r#"

local function molt_function_init_metadata_packed(func: any, metadata: any, _code: any, bind_kind: any): nil
	if type(func) ~= "function" or type(metadata) ~= "table" or molt_sequence_len(metadata) ~= 11 then
		error({__type="TypeError", __msg="invalid packed function metadata"})
	end
	molt_func_attr_set(func, "__name__", rawget(metadata, 1))
	molt_func_attr_set(func, "__qualname__", rawget(metadata, 2))
	molt_func_attr_set(func, "__module__", rawget(metadata, 3))
	molt_func_attr_set(func, "__defaults__", rawget(metadata, 9))
	molt_func_attr_set(func, "__kwdefaults__", rawget(metadata, 10))
	molt_func_attr_set(func, "__doc__", rawget(metadata, 11))
	molt_func_attr_set(func, "__code__", _code)
	molt_func_attr_set(func, "__molt_bind_kind__", bind_kind)
	molt_function_metadata[func] = {
		arg_names = rawget(metadata, 4),
		posonly = rawget(metadata, 5) or 0,
		kwonly = rawget(metadata, 6) or molt_pack_tuple(),
		vararg = rawget(metadata, 7),
		varkw = rawget(metadata, 8),
		defaults = rawget(metadata, 9),
		kwdefaults = rawget(metadata, 10),
	}
	return nil
end

local function molt_function_set_defaults(func: any, defaults: any, kwdefaults: any): nil
	local metadata = molt_function_metadata[func]
	if metadata == nil then error({__type="TypeError", __msg="expected function metadata"}) end
	metadata.defaults = defaults
	metadata.kwdefaults = kwdefaults
	molt_func_attr_set(func, "__defaults__", defaults)
	molt_func_attr_set(func, "__kwdefaults__", kwdefaults)
	return nil
end

local function molt_callargs_new(): {any}
	local builder = {}
	molt_callargs_metadata[builder] = {pos = molt_pack_list(), kwargs = molt_dict_new()}
	return builder
end

local function molt_callargs_require(builder: {any}): any
	local state = molt_callargs_metadata[builder]
	if state == nil then error({__type="TypeError", __msg="invalid callargs builder"}) end
	return state
end

local function molt_callargs_push_pos(builder: {any}, value: any): nil
	local pos = molt_callargs_require(builder).pos
	local count = molt_sequence_len(pos)
	rawset(pos, count + 1, value)
	rawset(pos, molt_sequence_length_key, count + 1)
	return nil
end

local function molt_callargs_push_kw(builder: {any}, key: any, value: any): nil
	if type(key) ~= "string" then error({__type="TypeError", __msg="keywords must be strings"}) end
	local kwargs = molt_callargs_require(builder).kwargs
	if molt_dict_contains(kwargs, key) then
		error({__type="TypeError", __msg="got multiple values for keyword argument '" .. key .. "'"})
	end
	molt_dict_set(kwargs, key, value)
	return nil
end

local function molt_callargs_expand_star(builder: {any}, iterable: any): nil
	local ok, iterator = pcall(molt_iterator_new, iterable)
	if not ok then error({__type="TypeError", __msg="argument after * must be an iterable, not " .. type(iterable)}) end
	while true do
		local step = iterator()
		if rawget(step, 2) then break end
		molt_callargs_push_pos(builder, rawget(step, 1))
	end
	return nil
end

local function molt_callargs_expand_kwstar(builder: {any}, mapping: any): nil
	if not molt_dict_is_ordered(mapping) then error({__type="TypeError", __msg="argument after ** must be a mapping"}) end
	molt_dict_scan(mapping, function(key, value) molt_callargs_push_kw(builder, key, value) end)
	return nil
end

local function molt_call_bound(func: any, positional: {any}, kwargs: {[any]: any}): any
	if type(func) ~= "function" then error({__type="TypeError", __msg="object is not callable"}) end
	local metadata = molt_function_metadata[func]
	local positional_count = molt_sequence_len(positional)
	if metadata == nil then
		if molt_dict_len(kwargs) ~= 0 then error({__type="TypeError", __msg="callable does not accept keyword arguments"}) end
		return func(table.unpack(positional, 1, positional_count))
	end
	local arg_names = metadata.arg_names or molt_pack_tuple()
	local positional_param_count = molt_sequence_len(arg_names)
	local kwonly = metadata.kwonly or molt_pack_tuple()
	local kwonly_count = molt_sequence_len(kwonly)
	if positional_count == positional_param_count and kwonly_count == 0 and metadata.vararg == nil and metadata.varkw == nil and molt_dict_len(kwargs) == 0 then
		return func(table.unpack(positional, 1, positional_count))
	end
	local values = molt_pack_list()
	local assigned = table.create(positional_param_count + kwonly_count)
	local copy_count = math.min(positional_count, positional_param_count)
	for index = 1, copy_count do rawset(values, index, rawget(positional, index)); assigned[index] = true end
	if positional_count > positional_param_count and metadata.vararg == nil then
		error({__type="TypeError", __msg="takes " .. tostring(positional_param_count) .. " positional arguments but " .. tostring(positional_count) .. " were given"})
	end
	local extra_keywords = molt_dict_new()
	molt_dict_scan(kwargs, function(key, value)
		local found = 0
		for index = 1, positional_param_count do
			if rawget(arg_names, index) == key then found = index; break end
		end
		if found == 0 then
			for index = 1, kwonly_count do
				if rawget(kwonly, index) == key then found = positional_param_count + index; break end
			end
		end
		if found ~= 0 then
			if found <= (metadata.posonly or 0) then
				if metadata.varkw ~= nil then molt_dict_set(extra_keywords, key, value); return end
				error({__type="TypeError", __msg="got some positional-only arguments passed as keyword arguments: '" .. key .. "'"})
			end
			if assigned[found] then error({__type="TypeError", __msg="got multiple values for argument '" .. key .. "'"}) end
			rawset(values, found, value); assigned[found] = true; return
		end
		if metadata.varkw == nil then error({__type="TypeError", __msg="got an unexpected keyword argument '" .. key .. "'"}) end
		molt_dict_set(extra_keywords, key, value)
	end)
	local defaults = metadata.defaults
	local defaults_count = if type(defaults) == "table" then molt_sequence_len(defaults) else 0
	local required_positional = positional_param_count - defaults_count
	for index = 1, positional_param_count do
		if not assigned[index] then
			if index > required_positional then rawset(values, index, rawget(defaults, index - required_positional)); assigned[index] = true
			else error({__type="TypeError", __msg="missing required positional argument: '" .. tostring(rawget(arg_names, index)) .. "'"}) end
		end
	end
	local kwdefaults = metadata.kwdefaults
	for index = 1, kwonly_count do
		local slot = positional_param_count + index
		if not assigned[slot] then
			local key = rawget(kwonly, index)
			if type(kwdefaults) == "table" and molt_dict_is_ordered(kwdefaults) and molt_dict_contains(kwdefaults, key) then
				rawset(values, slot, molt_dict_getitem(kwdefaults, key)); assigned[slot] = true
			else error({__type="TypeError", __msg="missing required keyword-only argument: '" .. tostring(key) .. "'"}) end
		end
	end
	local final_count = positional_param_count + kwonly_count
	if metadata.vararg ~= nil then
		local varargs = molt_pack_tuple()
		local vararg_count = positional_count - positional_param_count
		for index = 1, vararg_count do rawset(varargs, index, rawget(positional, positional_param_count + index)) end
		rawset(varargs, molt_sequence_length_key, math.max(0, vararg_count))
		final_count += 1; rawset(values, final_count, varargs)
	end
	if metadata.varkw ~= nil then final_count += 1; rawset(values, final_count, extra_keywords) end
	rawset(values, molt_sequence_length_key, final_count)
	return func(table.unpack(values, 1, final_count))
end

local function molt_callargs_invoke(func: any, builder: {any}): any
	local state = molt_callargs_require(builder)
	return molt_call_bound(func, state.pos, state.kwargs)
end

molt_call_checked = function(callable: any, ...): any
	local metadata = molt_function_metadata[callable]
	if metadata == nil then return callable(...) end
	if molt_sequence_len(metadata.arg_names or molt_pack_tuple()) == select('#', ...) and molt_sequence_len(metadata.kwonly or molt_pack_tuple()) == 0 and metadata.vararg == nil and metadata.varkw == nil then
		return callable(...)
	end
	local positional = molt_pack_list(...)
	return molt_call_bound(callable, positional, molt_dict_new())
end

local function molt_bound_method_new(func: any, self_value: any): any
	local function bound(...): any return molt_call_checked(func, self_value, ...) end
	local metadata = molt_function_metadata[func]
	if metadata ~= nil then
		local names = molt_pack_tuple()
		local count = math.max(0, molt_sequence_len(metadata.arg_names) - 1)
		for index = 1, count do rawset(names, index, rawget(metadata.arg_names, index + 1)) end
		rawset(names, molt_sequence_length_key, count)
		local defaults = metadata.defaults
		if defaults ~= nil then
			local defaults_count = molt_sequence_len(defaults)
			local keep = math.min(defaults_count, count)
			local bound_defaults = molt_pack_tuple()
			for index = 1, keep do rawset(bound_defaults, index, rawget(defaults, defaults_count - keep + index)) end
			rawset(bound_defaults, molt_sequence_length_key, keep)
			defaults = bound_defaults
		end
		molt_function_metadata[bound] = {
			arg_names = names,
			posonly = math.max(0, (metadata.posonly or 0) - 1),
			kwonly = metadata.kwonly,
			vararg = metadata.vararg,
			varkw = metadata.varkw,
			defaults = defaults,
			kwdefaults = metadata.kwdefaults,
		}
	end
	return bound
end

"#;

pub(super) const EQUALITY_REPR_RUNTIME: &str = r#"

local function molt_setlike_kind(value: any): string?
	if molt_set_is(value) then return "set" end
	if molt_dict_view_is(value) then
		local state = molt_dict_view_metadata[value]
		if state.kind ~= "values" then return "view" end
	end
	return nil
end

local function molt_setlike_len(value: any, kind: string): number
	if kind == "set" then return molt_set_len(value) end
	return molt_dict_view_len(value)
end

local function molt_setlike_contains(value: any, kind: string, needle: any): boolean
	if kind == "set" then return molt_set_contains(value, needle) end
	return molt_dict_view_contains(value, needle)
end

local function molt_setlike_scan(value: any, kind: string, visit: (any) -> ()): nil
	if kind == "set" then return molt_set_scan(value, visit) end
	local snapshot = molt_dict_view_snapshot(value)
	for index = 1, molt_sequence_len(snapshot) do visit(rawget(snapshot, index)) end
	return nil
end

molt_equal = function(left: any, right: any, seen: any?): boolean
	if left == right then return true end
	local left_kind = type(left)
	local right_kind = type(right)
	local left_numeric = left_kind == "number" or left_kind == "boolean"
	local right_numeric = right_kind == "number" or right_kind == "boolean"
	if left_numeric and right_numeric then
		local left_number = if left_kind == "boolean" then (if left then 1 else 0) else left
		local right_number = if right_kind == "boolean" then (if right then 1 else 0) else right
		return left_number == right_number
	end
	if left_kind ~= right_kind then return false end
	if left_kind ~= "table" then return false end
	local left_binary = molt_binary_metadata[left]
	local right_binary = molt_binary_metadata[right]
	if left_binary ~= nil or right_binary ~= nil then
		return left_binary ~= nil and right_binary ~= nil and left_binary.kind == right_binary.kind and left_binary.value == right_binary.value
	end
	local left_setlike = molt_setlike_kind(left)
	local right_setlike = molt_setlike_kind(right)
	if left_setlike ~= nil or right_setlike ~= nil then
		if left_setlike == nil or right_setlike == nil then return false end
		if molt_setlike_len(left, left_setlike) ~= molt_setlike_len(right, right_setlike) then return false end
		local equal = true
		molt_setlike_scan(left, left_setlike, function(value)
			if equal and not molt_setlike_contains(right, right_setlike, value) then equal = false end
		end)
		return equal
	end
	-- Values views deliberately retain CPython's identity-only equality.
	if molt_dict_view_is(left) or molt_dict_view_is(right) then return false end
	local left_is_dict = molt_dict_is_ordered(left)
	local right_is_dict = molt_dict_is_ordered(right)
	if left_is_dict ~= right_is_dict then return false end
	local visited = seen
	if visited == nil then visited = {} end
	local right_seen = visited[left]
	if right_seen ~= nil and right_seen[right] == true then return true end
	if right_seen == nil then right_seen = {}; visited[left] = right_seen end
	right_seen[right] = true
	if left_is_dict then
		if molt_dict_len(left) ~= molt_dict_len(right) then return false end
		local equal = true
		molt_dict_scan(left, function(key, value)
			if not equal or not molt_dict_contains(right, key) then equal = false; return end
			if not molt_equal(value, molt_dict_getitem(right, key), visited) then equal = false end
		end)
		return equal
	end
	local left_sequence_kind = rawget(left, molt_sequence_kind_key)
	local right_sequence_kind = rawget(right, molt_sequence_kind_key)
	-- Arbitrary Luau/object tables have identity semantics. Only canonical
	-- Python sequence representations participate in structural equality.
	if left_sequence_kind == nil or right_sequence_kind == nil then return false end
	if left_sequence_kind ~= right_sequence_kind then return false end
	if left_sequence_kind ~= "list" and left_sequence_kind ~= "tuple" then return false end
	local left_len = molt_sequence_len(left)
	local right_len = molt_sequence_len(right)
	if left_len ~= right_len then return false end
	for index = 1, left_len do
		if not molt_equal(rawget(left, index), rawget(right, index), visited) then return false end
	end
	return true
end

local function molt_repr_string(value: string): string
	local escaped = string.gsub(value, "\\", "\\\\")
	escaped = string.gsub(escaped, "\n", "\\n")
	escaped = string.gsub(escaped, "\r", "\\r")
	escaped = string.gsub(escaped, "\t", "\\t")
	escaped = string.gsub(escaped, "\b", "\\b")
	escaped = string.gsub(escaped, "\f", "\\f")
	escaped = string.gsub(escaped, "'", "\\'")
	return "'" .. escaped .. "'"
end

local function molt_render(x: any, quote_strings: boolean, seen: {[any]: boolean}): string
	if type(x) == "string" then return if quote_strings then molt_repr_string(x) else x end
	if type(x) == "table" then
		local binary = molt_binary_metadata[x]
		if binary ~= nil then return (if binary.kind == "bytes" then "b" else "bytearray(") .. molt_repr_string(binary.value) .. (if binary.kind == "bytes" then "" else ")") end
		local sequence_kind = rawget(x, molt_sequence_kind_key)
		if seen[x] then
			if molt_dict_is_ordered(x) then return "{...}" end
			if molt_set_is(x) then return "set(...)" end
			if sequence_kind == "tuple" then return "(...)" end
			if sequence_kind == "list" then return "[...]" end
			error({__type="TypeError", __msg="foreign Luau table has no Python repr"})
		end
		seen[x] = true
		if molt_dict_view_is(x) then
			local state = molt_dict_view_metadata[x]
			local rendered = molt_render(molt_dict_view_snapshot(x), true, seen)
			seen[x] = nil
			return "dict_" .. state.kind .. "(" .. rendered .. ")"
		end
		if molt_set_is(x) then
			local metadata = molt_set_require(x)
			if metadata.size == 0 then
				seen[x] = nil
				return if metadata.kind == "frozenset" then "frozenset()" else "set()"
			end
			local parts = table.create(metadata.size)
			local count = 0
			molt_set_scan(x, function(value) count += 1; parts[count] = molt_render(value, true, seen) end)
			seen[x] = nil
			local body = "{" .. table.concat(parts, ", ") .. "}"
			return if metadata.kind == "frozenset" then "frozenset(" .. body .. ")" else body
		end
		if molt_dict_is_ordered(x) then
			local parts = table.create(molt_dict_len(x))
			local count = 0
			molt_dict_scan(x, function(key, value)
				count += 1
				parts[count] = molt_render(key, true, seen) .. ": " .. molt_render(value, true, seen)
			end)
			seen[x] = nil
			return "{" .. table.concat(parts, ", ") .. "}"
		end
		if sequence_kind == "list" or sequence_kind == "tuple" then
			local count = molt_sequence_len(x)
			local parts = table.create(count)
			for index = 1, count do parts[index] = molt_render(rawget(x, index), true, seen) end
			seen[x] = nil
			if sequence_kind == "list" then return "[" .. table.concat(parts, ", ") .. "]" end
			if count == 1 then return "(" .. parts[1] .. ",)" end
			return "(" .. table.concat(parts, ", ") .. ")"
		end
		seen[x] = nil
		error({__type="TypeError", __msg="foreign Luau table has no deterministic Python repr"})
	end
	if type(x) == "boolean" then return x and "True" or "False" end
	if x == nil then return "None" end
	return tostring(x)
end

local function molt_str(x: any): string
	return molt_render(x, false, {})
end

local function molt_repr(x: any): string
	return molt_render(x, true, {})
end

"#;
