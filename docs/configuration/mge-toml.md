# MGE XE TOML configuration reference

MGE XE stores its settings in `mgeXE.toml` at the Morrowind root. The active
schema is `schema_version = 3`. The executable components never read, migrate,
overwrite, or delete the obsolete `MGE3\MGE.ini`.

The machine-readable authority is `mge-config`: typed structures live in
`src/schema.rs`, validation in `src/validation.rs`, and the commented
defaults in `src/default.toml`. Targeted saves preserve comments and valid
values; mge-config retains unknown root tables for other components.

## Load and write rules

- Missing known keys resolve to their schema defaults without a warning.
- Unknown keys, wrong types, unknown enum spellings, out-of-range values, and
  schema-version mismatches produce warnings. Valid sibling values still load;
  invalid values use defaults, and the loader discards malformed collection
  elements individually where it can.
- Saving a parseable document writes current version numbers, removes unknown
  keys inside owned tables, and removes or defaults values reported during load.
  Saves preserve completely unknown root tables.
- Malformed UTF-8 or TOML syntax uses embedded defaults and disables TOML writes
  until a successful explicit reload. Independent `Morrowind.ini` and registry
  writes remain available.
- Saves compare an exact-byte revision and use a unique adjacent temporary file
  plus an atomic Windows move.
- The GUI refuses revision conflicts. The runtime reloads the latest valid
  document and replays only its owned non-`DONT_SAVE` scalar values.
- First-run creation never replaces a file that appeared after the process
  loaded its defaults.

## Component ownership

`mge-config` owns the persistent schema, defaults, validation,
diagnostics, parsing, and document mutation. Each executable consumes it
directly:

- `MGEXEgui.exe` uses the typed Rust API and targeted document saves.
- `numidium.exe` reads the document itself through the same crate. On Direct3D 9 it
  uses the settings of startup generation and host-side culling. As the wgpu renderer
  (`runtime.renderer_wgpu`) it also reads the distant land, water, fog, shadow, lighting
  and shader settings when it starts. In the game, changes reach it as records of the
  stream from `d3d8.dll` (settings and shader state), not from the file.
- `d3d8.dll` calls a narrow Rust C ABI, then applies scalar values to
  the live C++ `Configuration` object through the binding table in
  `d3d8/cpp/mge/inidata.h`.

The C++ object remains the runtime authority because render code and the MWSE
API read or mutate it directly. `inidata.h` maps TOML paths to C++
storage; it is not a second schema or default table. Structured macros,
triggers, remaps, and the shader chain live in their Rust models, which render
them into the runtime's legacy buffers only at the boundary.

The DLL performs its read-only load during process initialization. It never
creates or saves a file from `DllMain`; first-run creation and later writes use
explicit save paths. The host is not a configuration service. On Direct3D 9 it
remains optional when its distant-land responsibilities are not needed; the wgpu
renderer needs it, because it draws the picture.

## Enum spellings

| Path | Accepted values |
| --- | --- |
| `graphics.anti_aliasing` | `none`, `x2`, `x4`, `x8` |
| `graphics.z_buffer_format` | `d24s8`, `d24x8`, `d32`, `d16`, `d16l`, `d32fl`, `intz`, `rawz`, `df16`, `df24` |
| `graphics.vsync` | `immediate`, `one`, `two`, `three`, `four` |
| `graphics.anisotropy` | `off`, `x2`, `x4`, `x8`, `x16` |
| `render.fog_mode` | `depth_pixel`, `depth_vertex`, `range_vertex` |
| `render.screenshot_format` | `bmp`, `jpeg`, `dds`, `png`, `tga` |
| `render.screenshot_suffix` | `timestamp`, `ordinal`, `character_ordinal`, `character_game_time_ordinal` |
| `render.window_align_x`, `render.window_align_y` | `left`, `center`, `right` |
| `distant_land.per_pixel_mode` | `always`, `interiors_only` |
| `input.macros[].type` | `unused`, `console1`, `console2`, `hammer1`, `hammer2`, `unhammer`, `alternate_hammer1`, `alternate_hammer2`, `alternate_unhammer`, `press1`, `press2`, `unpress`, `begin_timer`, `end_timer`, `graphics` |

## Complete cutover inventory

The following inventory is deliberately expressed as old section/key to new
TOML path. An explicit name set plus a field pattern covers the repeated
weather rows, so all 50 generated bindings remain reviewable without copying
the same mapping ten times.

### Runtime and graphics

| Obsolete INI entry | TOML path |
| --- | --- |
| `[Misc] MGE Disabled` | `runtime.disabled` |
| `[Misc] Internal MWSE Disabled` | `runtime.mwse_disabled` |
| `[Misc] Only Proxy D3D8To9` | `runtime.proxy_only` |
| (new, no INI key) | `runtime.renderer_wgpu` |
| (new, no INI key) | `runtime.renderer_probe` |
| (new, no INI key) | `runtime.renderer_backend` |
| (new, no INI key) | `runtime.renderer_texture_slots` |
| (new, no INI key) | `runtime.renderer_frame_latency` |
| `[Misc] Skip Intro Movies` | `runtime.skip_intro` |
| `[Misc] Use Menu Background Caching` | `runtime.menu_caching` |
| `[Misc] Customize 3rd Person Camera` | `runtime.custom_camera` |
| `[Misc] Initial 3rd Person Camera X` | `runtime.camera_x` |
| `[Misc] Initial 3rd Person Camera Y` | `runtime.camera_y` |
| `[Misc] Initial 3rd Person Camera Z` | `runtime.camera_z` |
| `[Misc] Crosshair Autohide` | `runtime.crosshair_autohide` |
| `[Global Graphics] Antialiasing Level` | `graphics.anti_aliasing` |
| `[Global Graphics] Z-Buffer Format` | `graphics.z_buffer_format` |
| `[Global Graphics] VWait` | `graphics.vsync` |
| `[Global Graphics] Refresh Rate` | `graphics.refresh_rate` |
| `[Global Graphics] Borderless Window` | `graphics.borderless` |
| `[Render State] Anisotropic Filtering Level` | `graphics.anisotropy` |
| `[Render State] Transparency Antialiasing` | `graphics.transparency_antialiasing` |
| `[Render State] Horizontal Screen FOV` | `render.fov` |
| `[Render State] Fog Mode` | `render.fog_mode` |
| `[Render State] Hardware Shader` | `render.enable_shaders` |
| `[Render State] HDR Reaction Time` | `render.hdr_reaction_time` |
| `[Render State] MGE FPS Counter` | `render.fps_counter` |
| `[Render State] MGE Messages` | `render.messages` |
| `[Render State] MGE Messages Timeout` | `render.message_timeout_ms` |
| `[Render State] Screenshot Format` | `render.screenshot_format` |
| `[Render State] Screenshot Output Directory` | `render.screenshot_directory` |
| `[Render State] Screenshot Name Prefix` | `render.screenshot_name` |
| `[Render State] Screenshot Name Suffix` | `render.screenshot_suffix` |
| `[Render State] UI Scaling` | `render.ui_scale` |
| `[Render State] Window Align X` | `render.window_align_x` |
| `[Render State] Window Align Y` | `render.window_align_y` |
| (new, no INI key) | `render.camera_relative` |

`render.indexed_skinning` has no obsolete INI predecessor. It enables the
indexed bone-palette skinning path: MGE XE rebuilds Morrowind's skin partitions
around an eight-entry palette so each skinned mesh draws in fewer partitions.
It defaults to `false` and is read once at process start, so changing it takes
effect only after a full game restart. It is never applied to partitions that
have already been built.

The feature also needs the matching custom DXVK `d3d9.dll`, which supplies the
eight-entry fixed-function matrix palette. MGE XE checks for it at runtime and
stays on the stock skinning path when it, or any of the engine patches, is
unavailable. Set `render.indexed_skinning = true` to opt in; leaving it at the
default `false` is the documented rollback and is safe with the custom DXVK
still installed.

### Distant land

| Obsolete `[Distant Land]` key | TOML path |
| --- | --- |
| `Distant Land` | `distant_land.enabled` |
| `Automatic Distant Land Rebuild` | `distant_land.automatic_rebuild` |
| `Distant Statics` | `distant_land.statics` |
| `Use Distant Water Without Distant Land` | `distant_land.water_without_land` |
| `Render Grass` | `distant_land.render_grass` |
| `Draw Distance` | `distant_land.draw_distance` |
| `Near Statics End` | `distant_land.near_static_end` |
| `Far Statics End` | `distant_land.far_static_end` |
| `Very Far Statics End` | `distant_land.very_far_static_end` |
| `Far Static Min Size` | `distant_land.far_static_min_size` |
| `Very Far Static Min Size` | `distant_land.very_far_static_min_size` |
| `Water Reflects Land` | `distant_land.water.reflect_land` |
| `Water Reflects Near Statics` | `distant_land.water.reflect_near_statics` |
| `Water Reflects Interiors` | `distant_land.water.reflect_interiors` |
| `Enable Sky Reflections` | `distant_land.water.reflect_sky` |
| `Dynamic Ripples` | `distant_land.water.dynamic_ripples` |
| `Blur Water Reflections` | `distant_land.water.blur_reflections` |
| `Water Wave Height` | `distant_land.water.wave_height` |
| `Water Caustics Intensity` | `distant_land.water.caustics_intensity` |
| `Use Exponential Fog` | `distant_land.fog.exponential` |
| `Use Atmosphere Scattering` | `distant_land.fog.atmosphere_scattering` |
| `Above Water Fog Start` | `distant_land.fog.above_water_start` |
| `Above Water Fog End` | `distant_land.fog.above_water_end` |
| `Below Water Fog Start` | `distant_land.fog.below_water_start` |
| `Below Water Fog End` | `distant_land.fog.below_water_end` |
| `Interior Fog Start` | `distant_land.fog.interior_start` |
| `Interior Fog End` | `distant_land.fog.interior_end` |
| `Sun Shadows` | `distant_land.shadows.enabled` |
| `Sun Shadow Map Resolution` | `distant_land.shadows.map_resolution` |
| `Per Pixel Shader` | `distant_land.per_pixel_lighting` |
| `Per Pixel Shader Flags` | `distant_land.per_pixel_mode` |
| `Terrain Horizon Culling` | `distant_land.horizon.culling` |
| `Horizon Height Bias` | `distant_land.horizon.height_bias` |
| `Horizon Object Bias` | `distant_land.horizon.object_bias` |
| `Horizon Near Exclude` | `distant_land.horizon.near_exclude` |
| `Horizon Ring Step` | `distant_land.horizon.ring_step` |
| `Horizon Max Range` | `distant_land.horizon.max_range` |
| `Horizon Azimuth Bins` | `distant_land.horizon.azimuth_bins` |
| `Horizon Sample Spacing` | `distant_land.horizon.sample_spacing` |
| `Horizon Adaptive Gate` | `distant_land.horizon.adaptive_gate` |
| `Horizon Hierarchical March` | `distant_land.horizon.hierarchical_march` |
| (new, no INI key) | `distant_land.native_depth_capture` |
| (new, no INI key) | `distant_land.native_ppl_packets` |
| (new, no INI key) | `distant_land.expanded_light_limit` |
| (new, no INI key) | `distant_land.per_pixel_light_fade` (default `true` since 2026-10-16) |
| (new, no INI key) | `distant_land.per_pixel_light_fade_radius` (default `1.75`, the radius multiplier of OpenMW) |
| (new, no INI key) | `distant_land.wait_on_load` |

`src/default.toml` describes each of these new keys, and `render.camera_relative` too.

Six more keys have no obsolete INI predecessor and no C++ binding: only the wgpu renderer
reads them.

- `distant_land.grass.soft_end`, `distant_land.grass.vertex_color_sway` and
  `distant_land.grass.wrap_lighting` (default `false` each): the looks of the grass shader
  of Remiros' Groundcover. The grass sinks into the ground near the end of its range at a
  distance that a noise moves; the red of the vertex colour weights the sway (and chooses
  the parts that get the wrap light); the sun light wraps past the side of a blade, and
  0.4 of it comes through from the back. The shader is
  `mgeHost64/src/renderer/statics/grass.wgsl`.

- `runtime.renderer_frame_latency` (default `2`, the range `1` to `3`): the frames of the wgpu
  renderer that may wait for the graphics card before the window shows them. With `1` the
  mouse has the least lag when the graphics card is the slow part (the mouse is read when the
  game's frame starts, so each frame that waits adds a frame of lag), but the renderer and the
  card take turns: in a test with an RTX 5070 the frame rate fell by a third (237.7 against
  343.4 frames a second in Balmora). `2` keeps both busy. Read by numidium.exe when it starts. The F10 panel
  shows how long the renderer waits for a picture of the window: a long wait means the card
  is behind. For a test, `MGE_XE_FRAME_LATENCY` overrides it.

- `graphics.vsync` on the wgpu renderer: with `immediate` (the default) the window shows a
  frame when the game gives one, and the frame rate is not capped (the present mode
  `AutoNoVsync`: immediate, mailbox or FIFO, the first that the graphics card has). With
  `one`, `two`, `three` or `four` the window waits for the next refresh of the screen (the
  present mode `Fifo`), so the frame rate is capped at the refresh rate; the wgpu renderer has
  no interval of 2 or more refreshes, so the four values are the same. Read by numidium.exe
  when it starts; the in-game menu of MGE XE does not change it. For a test, `MGE_XE_PRESENT`
  (`immediate`, `mailbox`, `fifo`, `auto`) sets the present mode over the key;
  nirender-host.log names the present mode in use.

- `runtime.renderer_texture_slots` (default `0`): the room for the textures of a frame of
  the wgpu renderer (its texture arrays). `0` lets the renderer choose: the room of the
  graphics card, at most 16384. A number from 256 to 1048576 is the most slots (at most the
  card's room); another value counts as `0`. The textures of the objects the renderer keeps
  and the textures of the frame's other draws share the room; 256 slots stay for the latter.
  A slot is freed when no object uses its texture, and free slots at the end of the room
  go. A texture past a full room draws white, and `nirender-host.log` says so. The
  environment variable `MGE_XE_ARRAY_CAP` overrides it, for tests. Read when numidium.exe
  starts.
- `distant_land.per_pixel_original_lights` (default `false`): the wgpu renderer lights each
  pixel with each light of the game in its own colour and with its whole falloff. `true`
  decodes the lights as MGE XE on Direct3D 9 does.
- `distant_land.clustered_lights` (default `false`): a pixel takes its lights from the
  clusters of the view (tiles of the picture by slices of depth), with the mask of the
  lights of its draw. The picture is the same either way; `false` lights each pixel from
  the list of its draw. The environment variable `MGE_XE_CLUSTERS` (`0` or `1`) overrides
  it, for tests.
- `distant_land.water.blur_contact_hardening` (default `true`): with
  `distant_land.water.blur_reflections`, a reflection is sharp where the thing meets the
  water and its blur grows with distance from the water. `false` gives the even blur of
  Direct3D 9.

The host-only horizon setting with no obsolete C++ binding is
`distant_land.horizon.rebuild_eye_threshold`.

`distant_land.statics_fallback_cap_mb` has no obsolete INI predecessor, and only the wgpu
renderer reads it. The wgpu renderer keeps the merged distant statics on the GPU under a
limit: what the memory budget of the GPU leaves after the memory in use and a margin (as
MGE XE on Direct3D 9 does with the DXVK of the fork), lowered when other programs take
memory. It reads the budget from `VK_EXT_memory_budget`, so it has it on the Vulkan backend
with a driver that has the extension, also under Wine and Proton. Without a budget (the
Direct3D 12 backend, or a driver without the extension), this setting is the limit in
megabytes: `1024` by default, the range `0` to `65536`, and `0` is no limit. When the limit
is full, a merged static farther from the camera goes from the GPU for a nearer one, and a
far one can show late.

`distant_land.statics_residency` and `distant_land.statics_resident_mb` have no obsolete INI
predecessor, and only the wgpu renderer reads them. They say which merged distant statics
stay on the GPU under that limit:

- `"auto"` (the default): a merged static comes to the GPU when the camera is within the
  draw distance and one cell, and stays. When the merged statics pass the high mark
  (`statics_resident_mb`), the ones the camera wanted least recently go, down to 85 % of the
  mark; a static wanted in the last minute or near the camera stays. A return to a place
  seen before finds its statics there.
- `"near"`: a merged static goes when the camera is farther than the draw distance and two
  cells (the rule before 2026-10-26). It uses the least memory and loads the most.
- `"all"`: all merged statics of the world space go to the GPU at the start, when they fit
  under the high mark; else as `"auto"`.

`statics_resident_mb` is the high mark in megabytes: `0` (the default) is half of the limit
above as it is at the start of the session, the range `0` to `65536`. The mark never goes over
the limit as it is now: when other programs take memory, the limit and then the mark go down.
The limit moves only for a change of more than 64 MB or 1 % of the budget, whichever is
larger. An interior cell with distant statics is a world space of its
own: the statics of another world space count as the farthest, so a visit to an interior
takes none off the GPU unless the high mark is passed.

`distant_land.water.volume_reflection_cells` has no obsolete INI predecessor. It is how far
from the camera, in cells, a water volume among the distant statics reflects what is on screen;
farther away it reflects the sky only. A water volume that the game draws itself, near the
camera, reflects what is on screen for every value but `0`. The default is `8.0`, the range
`0` to `64`, and `0` turns the screen reflection of all water volumes off, near ones too:
they reflect the sky only.

Three settings put light on water volumes. They have no obsolete INI predecessor. Each is
`true` by default. The water shading has no code for one that is off, so an option that is
off costs nothing. A change takes effect when the game starts.

- `distant_land.water.volume_shadows`: the shadows of distant land fall on the surface. In a
  shadow the water has no glint of the sun and its depth is a little darker. It needs
  `distant_land.shadows.enabled` and a cell with weather.
- `distant_land.water.volume_point_lights`: the lamps and fires of the game glint in the
  surface of a water volume near the player, in the hue of the light. A surface takes up to
  four lights, those that the game lights its mesh with, and the glint falls off with
  distance as the light of the game does: a lamp must be near the water to show.
- `distant_land.water.volume_caustics`: the light pattern of ripples on what is under the
  surface, with the strength of `distant_land.water.caustics_intensity`, in a cell with
  weather, on surfaces that face up. As for the water of the cell, the pattern fades with
  the distance of the camera.

`distant_land.grass.interior_wind` has no obsolete INI predecessor either. It
is the constant wind applied to grass placed in interior cells, which have no
weather to drive the per-weather wind factors. It uses the same units as those
factors and is clamped to `0.0..1.0`; `0.0` leaves interior grass with only the
shader's faint idle shimmer. It reaches only cells without weather: an
interior flagged to behave like an exterior, such as a Mournhold district, has
weather and keeps the per-weather wind factors, sun shadows and fog like any
exterior cell. The GUI edits it as the Interior row of the Distant Land Weather
Settings window, where the fog columns are disabled because interiors have no
weather fog to scale. Interior grass comes from the generator's
`grass_plugins` list, whose interior placements are baked into `usage.data`
like exterior ones.

### Weather and lighting patterns

For each weather name in `clear`, `cloudy`, `foggy`, `overcast`, `rain`,
`thunderstorm`, `ashstorm`, `blight`, `snow`, and `blizzard`:

| Obsolete entry pattern | TOML path pattern |
| --- | --- |
| `[Distant Land Weather] <Weather> Wind Ratio` | `distant_land.weather.<weather>.wind` |
| `[Distant Land Weather] <Weather> Fog Ratio` | `distant_land.weather.<weather>.fog_ratio` |
| `[Distant Land Weather] <Weather> Fog Offset` | `distant_land.weather.<weather>.fog_offset` |
| `[Per Pixel Lighting] <Weather> Sun Brightness` | `lighting.weather.<weather>.sun` |
| `[Per Pixel Lighting] <Weather> Ambient Brightness` | `lighting.weather.<weather>.ambient` |

### Structured and GUI-owned data

| Obsolete INI representation | TOML representation |
| --- | --- |
| Bare `[Shader Chain]` lines | `shaders.chain = ["Shader A", "Shader B"]` |
| `[Macros] M<index>=...` plus `[MacrosDesc]` | `[[input.macros]]` with `index`, `type`, and type-specific fields |
| `[InputTriggers] T<index>=...` | `[[input.triggers]]` with `index`, `active`, `interval_ms`, and `keys` |
| `[InputRemap] R<source>=<target>` | `[input.remap]` |
| `[Render State] Match FOV To Aspect Ratio` | `gui.match_fov_to_aspect_ratio` |
| `[Distant Land] Auto Distances` | `gui.auto_distances` |
| `[Distant Land] Auto Distances Choice` | `gui.auto_distance_mode` |
| `[Distant Land] Exponential Distance Multiplier` | `gui.exponential_distance_multiplier` |
| `[Main] GUI Language` + `[Main] Language Autodetection` | `gui.language = "auto"` or an embedded locale code; see [GUI localization](../gui/localization.md) |

Trigger intervals are milliseconds in both TOML and the runtime. This removes
the old GUI/runtime factor-of-1000 mismatch.

## Runtime ownership

The following bound values retain the legacy `DONT_SAVE` policy because the GUI,
registry, or structured Rust model owns them:

- startup gates: `runtime.disabled`, `runtime.mwse_disabled`,
  `runtime.proxy_only`, `runtime.renderer_wgpu`, and `runtime.renderer_probe`;
- graphics/display selection fields;
- screenshot format/path/name/suffix;
- `distant_land.water_without_land`;
- shader chain and all structured input collections.

`d3d8/crates/config-contract-test` exports the real C++ binding table and verifies all
140 rows against the Rust schema on the i686 target, including uniqueness,
storage widths, buffer capacities, `DONT_SAVE`, and default values.

## `Morrowind.ini`

`Morrowind.ini` remains game-owned and is not part of this TOML schema.
`MGEXEgui/src/morrowind_profile.rs` uses `GetPrivateProfileStringA` and
`WritePrivateProfileStringA` against the engine's exact
`.\\Morrowind.ini` path for the following scalar entries:

- `[General]`: `Max FPS`, `Screen Shot Enable`, `DontThreadLoad`,
  `AllowYesToAll`, `High Detail Shadows`, `Show FPS`, `Disable Audio`,
  `Subtitles`, and `ShowHitFader`.
- `[LightAttenuation]`: `UseConstant`, `ConstantValue`, `UseLinear`,
  `LinearValue`, `UseQuadratic`, and `QuadraticValue`.

`DontThreadLoad` remains the inverse of the GUI's thread-loading option. Reads
honor attenuation enable defaults false/true/false; saves intentionally preserve
the established behavior of enabling all three attenuation terms.
