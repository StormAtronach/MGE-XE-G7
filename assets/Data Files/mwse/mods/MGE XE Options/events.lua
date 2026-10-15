--[[
	The event distantLandReady of MGE XE, for mods:

		event.register("distantLandReady", function(e)
			-- e.reason: "loaded" (the distant land of the session is loaded) or "arrived"
			-- (after a game load or a jump of the camera of more than 2 cells, the distant
			-- statics around the camera are there, or the wait of 10 s ended).
		end)

	The renderer gives each reason to d3d8.dll (the Direct3D 9 path itself, the wgpu renderer
	through a message of its helper). This file takes them through LuaJIT's FFI, as gui.lua
	does for the horizon parameters, and triggers the event. A build of MGE XE without
	MGE_DistantLandReadyNext gives no event.
]]--

local ffi = require("ffi")

pcall(ffi.cdef, [[
	int MGE_DistantLandReadyNext(void);
]])

local d3d8 = nil
do
	local ok, lib = pcall(ffi.load, "d3d8")
	if ok and pcall(function() return lib.MGE_DistantLandReadyNext end) then
		d3d8 = lib
	end
end

local reasons = { [1] = "loaded", [2] = "arrived" }

local function poll()
	while true do
		local reason = d3d8.MGE_DistantLandReadyNext()
		if reason == 0 then
			break
		end
		event.trigger("distantLandReady", { reason = reasons[reason] or tostring(reason) })
	end
end

if d3d8 then
	event.register(tes3.event.enterFrame, poll)
end
