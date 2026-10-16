--[[
	A shader is loaded again when its file changes, so that an author can edit a shader in
	an editor beside the game and see the result at once.

	The files: <name>.fx and, with the wgpu renderer, <name>.wgsl in
	Data Files\shaders\XEshaders. While the setting is on, the mod looks at the change time
	of the files of the loaded shaders two times in a second. The setting is off at the
	start: then no timer runs.

	The result of a load:
	- With the wgpu renderer, the renderer writes it into the file mgeXE-shaders.txt in the
	  folder of the game (the game has no other way to know it). This mod reads the file
	  and shows the error of a shader that does not compile.
	- With Direct3D 9, the error is in mgeXE.log only.
]]--

local this = {}

local lfs = require("lfs")
local nirender = include("akulakhan")

local statusPath = "mgeXE-shaders.txt"
local shaderFolder = "Data Files\\shaders\\XEshaders\\"

this.config = mwse.loadConfig("MGE XE Options", { hotReload = false })

-- The wgpu renderer draws the game.
function this.wgpu()
	return nirender ~= nil and nirender.ownDevice ~= nil and nirender.ownDevice() == true
end

-- The file of the results: a table by the name of the shader, each with `loads` (a number
-- that changes with each load), `ok`, and `text` (the error).
function this.readStatus()
	local shaders = {}
	local file = io.open(statusPath, "r")
	if not file then return shaders end
	local current
	for line in file:lines() do
		local name = line:match("^shader (.+)$")
		if name then
			current = { loads = 0, ok = true, text = "" }
			shaders[name] = current
		elseif current then
			local loads = line:match("^loads (%d+)$")
			if loads then
				current.loads = tonumber(loads)
			elseif line == "error" then
				current.ok = false
			elseif line:sub(1, 2) == "| " then
				current.text = current.text .. line:sub(3) .. "\n"
			end
		end
	end
	file:close()
	return shaders
end

-- A text of the renderer for a message of the game: the font of the game has no signs
-- for the lines that the compiler draws around the code.
local function plain(text, lines)
	local out, count = {}, 0
	for line in text:gmatch("[^\n]+") do
		line = line:gsub("[\128-\255]+", "|"):gsub("%s+$", "")
		if line:find("%S") then
			count = count + 1
			if count <= lines then table.insert(out, line) end
		end
	end
	return table.concat(out, "\n")
end

-- What the last result was, for a window that shows it: `nil` without a result. `unknown`
-- is true when the renderer gave no answer, which is not an error of the shader.
this.last = {}

local function report(name, ok, text, unknown)
	local i18n = this.i18n
	this.last[name] = { ok = ok, unknown = unknown, text = text or "" }
	if unknown then
		mwse.log("[MGE XE Options] no answer from the renderer for the reload of the shader %s", name)
		tes3.messageBox{message = string.format("%s: %s", name, i18n("ShaderNoAnswer"))}
	elseif ok then
		mwse.log("[MGE XE Options] the shader %s was reloaded", name)
		tes3.messageBox{message = string.format("%s: %s", name, i18n("ShaderReloaded"))}
	else
		mwse.log("[MGE XE Options] the shader %s does not compile:\n%s", name, text or "")
		tes3.messageBox{message = string.format("%s: %s\n%s", name, i18n("ShaderError"), plain(text or "", 6))}
	end
	if this.onResult then this.onResult(name) end
end

-- Loads a shader again and reports the result.
function this.reload(shader)
	local i18n = this.i18n
	local name = shader.name
	if not this.wgpu() then
		-- Direct3D 9 compiles at once: true for a shader that was made again.
		if shader:reload() then
			report(name, true)
		else
			report(name, false, i18n("ShaderSeeLog"))
		end
		return
	end
	-- The renderer is another program: its answer comes with a later frame.
	local before = this.readStatus()[name]
	local loadsBefore = before and before.loads or 0
	shader:reload()
	local tries = 0
	timer.start{
		type = timer.real,
		duration = 0.2,
		iterations = 20,
		callback = function(e)
			tries = tries + 1
			local now = this.readStatus()[name]
			if now and now.loads ~= loadsBefore then
				e.timer:cancel()
				report(name, now.ok, now.text)
			elseif tries == 20 then
				report(name, false, nil, true)
			end
		end,
	}
end

-- The files that are watched, by the name of the shader: the shader, its two paths, and
-- the change time of each (`false` for a file that is not there).
local watched = {}
local tick = 0

local function changeTime(path)
	return lfs.attributes(path, "modification") or false
end

-- The list of the loaded shaders is read again every five seconds: a mod can load a shader
-- at any time.
local function refresh()
	local seen = {}
	for _, shader in pairs(mge.shaders.list) do
		local name = shader.name
		seen[name] = true
		if not watched[name] then
			local paths = { shaderFolder .. name .. ".fx", shaderFolder .. name .. ".wgsl" }
			watched[name] = { shader = shader, paths = paths, times = { changeTime(paths[1]), changeTime(paths[2]) } }
		end
	end
	for name in pairs(watched) do
		if not seen[name] then watched[name] = nil end
	end
end

local function check()
	if not this.config.hotReload or not mge.shaders then return end
	tick = tick + 1
	if tick % 10 == 1 then refresh() end
	for _, entry in pairs(watched) do
		local changed = false
		for i = 1, 2 do
			local now = changeTime(entry.paths[i])
			if now ~= entry.times[i] then
				entry.times[i] = now
				-- A file that went away for a moment (an editor that saves with a new file)
				-- counts when it is there again.
				changed = changed or now ~= false
			end
		end
		if changed then this.reload(entry.shader) end
	end
end

-- Starts the watch when the setting is on. The game stops each timer when it loads a
-- save, so the watch starts again then. A watch from before that is still there stops
-- itself at its next look: so a call with the setting off stops the watch.
local started = 0
function this.start()
	started = started + 1
	local mine = started
	if not this.config.hotReload then return end
	for _, entry in pairs(watched) do
		entry.times[1], entry.times[2] = changeTime(entry.paths[1]), changeTime(entry.paths[2])
	end
	timer.start{ type = timer.real, duration = 0.5, iterations = -1, callback = function(e)
		if mine ~= started then
			e.timer:cancel()
			return
		end
		check()
	end }
end

function this.setEnabled(on)
	this.config.hotReload = on
	mwse.saveConfig("MGE XE Options", this.config)
	this.start()
end

return this
