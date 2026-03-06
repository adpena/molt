--!strict
-- Molt -> Luau transpiled output (standalone extract for Roblox Studio)
-- Runtime helpers

local function molt_range(start: number, stop: number, step: number?): {number}
	local result = {}
	local s = step or 1
	local i = start
	while (s > 0 and i < stop) or (s < 0 and i > stop) do
		table.insert(result, i)
		i = i + s
	end
	return result
end

local function molt_int(x: any): number
	return math.floor(tonumber(x) or 0)
end

local function molt_floor_div(a: number, b: number): number
	return math.floor(a / b)
end

local function molt_mod(a: number, b: number): number
	return a - math.floor(a / b) * b
end

local bit = bit32 or bit
local math_floor = math.floor

-- Minimal JSON serializer
local molt_json_dumps
do
	local function serialize(val, depth)
		if depth > 50 then return '"[max depth]"' end
		local t = type(val)
		if t == "nil" then return "null"
		elseif t == "boolean" then return val and "true" or "false"
		elseif t == "number" then
			if val ~= val then return "null" end
			if val == math.huge then return "1e308" end
			if val == -math.huge then return "-1e308" end
			if val == math_floor(val) and math.abs(val) < 1e15 then
				return string.format("%d", val)
			end
			return tostring(val)
		elseif t == "string" then
			local escaped = val:gsub('[\\"]', function(c) return "\\" .. c end)
			escaped = escaped:gsub("\n", "\\n"):gsub("\r", "\\r"):gsub("\t", "\\t")
			return '"' .. escaped .. '"'
		elseif t == "table" then
			local is_array = true
			local n = #val
			if n == 0 then
				if next(val) ~= nil then is_array = false end
			else
				for k in pairs(val) do
					if type(k) ~= "number" or k < 1 or k > n or k ~= math_floor(k) then
						is_array = false
						break
					end
				end
			end
			local parts = {}
			if is_array then
				for i = 1, n do
					table.insert(parts, serialize(val[i], depth + 1))
				end
				return "[" .. table.concat(parts, ", ") .. "]"
			else
				for k, v in pairs(val) do
					local ks = type(k) == "string" and k or tostring(k)
					local escaped = ks:gsub('[\\"]', function(c) return "\\" .. c end)
					table.insert(parts, '"' .. escaped .. '": ' .. serialize(v, depth + 1))
				end
				return "{" .. table.concat(parts, ", ") .. "}"
			end
		end
		return '"' .. tostring(val) .. '"'
	end
	molt_json_dumps = function(val)
		return serialize(val, 0)
	end
end

-- Generator functions (transpiled from generator.py)
local function hash_coord(x: number, y: number, z: number, seed: number): number
	local h = bit32.bxor(seed, x * 374761393)
	h = bit32.bxor(h, y * 668265263)
	h = bit32.bxor(h, z * 2246822519)
	h = bit32.bxor(h, bit32.rshift(h, 13)) * 3266489917
	h = bit32.bxor(h, bit32.rshift(h, 16))
	return bit32.band(h, 0xFFFFFFFF) / 4294967295.0
end

local function interpolate(a: number, b: number, w: number): number
	return (b - a) * (3.0 - w * 2.0) * w * w + a
end

local function smooth_noise(x: number, y: number, z: number, seed: number): number
	local ix = math_floor(x)
	local iy = math_floor(y)
	local iz = math_floor(z)
	local fx = x - ix
	local fy = y - iy
	local fz = z - iz
	local v000 = hash_coord(ix, iy, iz, seed)
	local v100 = hash_coord(ix + 1, iy, iz, seed)
	local v010 = hash_coord(ix, iy + 1, iz, seed)
	local v110 = hash_coord(ix + 1, iy + 1, iz, seed)
	local v001 = hash_coord(ix, iy, iz + 1, seed)
	local v101 = hash_coord(ix + 1, iy, iz + 1, seed)
	local v011 = hash_coord(ix, iy + 1, iz + 1, seed)
	local v111 = hash_coord(ix + 1, iy + 1, iz + 1, seed)
	local i1 = interpolate(v000, v100, fx)
	local i2 = interpolate(v010, v110, fx)
	local j1 = interpolate(i1, i2, fy)
	local i3 = interpolate(v001, v101, fx)
	local i4 = interpolate(v011, v111, fx)
	local j2 = interpolate(i3, i4, fy)
	return interpolate(j1, j2, fz)
end

local function generate_procedural_zone(seed: number, base_y: number, depth: number, offset_x: number?, offset_z: number?): string
	local ox = offset_x or 88
	local oz = offset_z or -52
	local platforms = {}
	local crystals = {}
	local anchors = {}
	local step = 8
	local threshold = 0.55

	for _, y in ipairs(molt_range(base_y, base_y + depth, step)) do
		for _, x in ipairs(molt_range(-50, 50, step)) do
			for _, z in ipairs(molt_range(-50, 50, step)) do
				local density = smooth_noise(x * 0.1, y * 0.1, z * 0.1, seed)
				if density > threshold then
					local world_x = ox + x
					local world_z = oz + z
					table.insert(platforms, {
						pos = {world_x, y, world_z},
						size = {step, step, step},
						color = {0.1, 0.12, 0.15}
					})
					local crystal_chance = hash_coord(x, y, z, seed + 1)
					if crystal_chance > 0.95 then
						table.insert(crystals, {
							pos = {world_x, y + step/2 + 2, world_z},
							color = {0.3, 0.8, 0.7},
							size = 1.5 + (hash_coord(x, y, z, seed + 2) * 2.0)
						})
					end
					if crystal_chance < 0.02 then
						table.insert(anchors, {
							pos = {world_x, y + 10, world_z}
						})
					end
				end
			end
		end
	end

	return molt_json_dumps({
		status = "success",
		performance_note = "Generated via Molt Python->Luau transpiler",
		data = {
			id = "procedural_layer_" .. tostring(seed),
			name = "The Endless Fractal",
			yMin = base_y,
			yMax = base_y + depth,
			origin = {ox, base_y + (depth/2), oz},
			platforms = platforms,
			crystals = crystals,
			anchors = anchors,
		}
	})
end

-- Run and print result
local result = generate_procedural_zone(1337, 222, 64)
print("[MOLT_LUAU] Generated zone: " .. string.sub(result, 1, 200) .. "...")
print("[MOLT_LUAU] Total length: " .. tostring(#result))

-- Parse quick stats
local platform_count = 0
for _ in string.gmatch(result, '"pos"') do
	platform_count = platform_count + 1
end
print("[MOLT_LUAU] Approximate object count: " .. tostring(platform_count))
print("[MOLT_LUAU] SUCCESS")
