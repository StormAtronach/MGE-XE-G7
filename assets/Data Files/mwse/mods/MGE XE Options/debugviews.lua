--[[
	The debug views of the wgpu renderer (W642): the state and the calls of the page "Debug" of
	the MGE XE window (gui.lua). The player chooses the renderer's debug view, its term, and its
	line sources. The calls are nirender.debug.view and nirender.debug.lines of the plugin
	(the Lua module akulakhan); they send the tunes "debug view ..." and "debug lines ...". The
	helper checks each name and writes the result to its log. The views and the sources are in
	RENDERER.md, "Debug views".

	The page is only on the wgpu renderer (this.available()).

	Nothing of this page is saved. The helper starts each session with no view and no line
	source, and this module starts with the same state, so a restart of the game turns each view
	and each line source off. A debug view that stayed on would look like a fault of the
	renderer. The page shows what it set in this session: a tune from the console or from another
	mod does not show here.
]]--

local this = {}

local hotreload = require("MGE XE Options.hotreload")
local nirender = include("akulakhan")

-- The views of the helper (debug_view.rs, View::NAMES), with "off" first.
this.views = { "off", "lights", "shadow-layers", "shadow-map", "water", "post", "fog" }

-- The views that take a term, with their largest term: the water 1 to 10 (docs/core-mods.md,
-- "The water debug views"), the post chain 1 to 2.
this.termCounts = { water = 10, post = 2 }

-- The line sources of the helper (debug_lines.rs, LineSources::NAMED).
this.lineSources = { "lights", "shadow-boxes", "bounds", "lua" }

-- The state of this session: the view, the term of each view that takes one, and the line
-- sources that are on.
local state = {
	view = "off",
	terms = { water = 1, post = 1 },
	lines = {},
}

-- The page can show: the wgpu renderer draws the game, and the plugin has the debug calls.
function this.available()
	return nirender ~= nil
		and hotreload.wgpu()
		and nirender.debug ~= nil
		and nirender.debug.view ~= nil
		and nirender.debug.lines ~= nil
end

function this.view()
	return state.view
end

function this.term(name)
	return state.terms[name]
end

function this.lineOn(source)
	return state.lines[source] == true
end

-- Sends the view `name` with its term, when it takes one.
local function sendView(name)
	local term = state.terms[name]
	if term then
		nirender.debug.view(name, tostring(term))
	else
		nirender.debug.view(name)
	end
end

-- Sets the next view of this.views, after the last one "off" again.
function this.nextView()
	local next = 1
	for i, name in ipairs(this.views) do
		if name == state.view then
			next = i % #this.views + 1
			break
		end
	end
	state.view = this.views[next]
	sendView(state.view)
end

-- Changes the term of the view `name` by `delta`, from 1 to its largest term. While that view
-- is on, the new term goes to the helper at once.
function this.changeTerm(name, delta)
	local term = math.clamp(state.terms[name] + delta, 1, this.termCounts[name])
	if term == state.terms[name] then
		return
	end
	state.terms[name] = term
	if state.view == name then
		sendView(name)
	end
end

-- Turns the line source `source` on or off.
function this.switchLine(source)
	local on = not this.lineOn(source)
	state.lines[source] = on
	nirender.debug.lines(source, on)
end

return this
