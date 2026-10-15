# The wgpu renderer

With `[runtime] renderer_wgpu = true` (the default) MGE XE makes no Direct3D device. The game's
draws go as records through shared memory to `mgeHost64.exe --live`, which draws them with
wgpu (Vulkan or Direct3D 12) together with everything MGE XE adds. This page is the
architecture of that renderer: the processes, the frame, every pipeline with its bind groups,
and the conventions that a change must keep. The player's view is in
[`RENDERER.md`](../../RENDERER.md).

The page cites functions and types, not line numbers. Two repositories take part:

- this repository: `d3d8.dll` (namespace `Deviceless`) and the helper `mgeHost64`
  (`mgeHost64/src/renderer.rs` and `mgeHost64/src/renderer/`);
- `nirender-plugin`, a sibling checkout: the MWSE plugin `nirender.dll` (32-bit) and the crate
  `nirender_wgpu` (`host/`, 64-bit), which `mgeHost64` links by path.

## 1. Processes

```text
Morrowind.exe (32-bit)                                 mgeHost64.exe --live (64-bit)
+------------------------------------------+          +--------------------------------------+
| game code -- Direct3D 8 calls -->        |          | nirender_wgpu (crate)                |
|   nirender.dll (MWSE plugin)             |  shared  |   device, surface, window            |
|     ports of the renderer functions      |  memory  |   replay of the game's draws         |
|     records of each frame  ------------------ring---->  trait Extension, SceneView        |
| d3d8.dll (MGE XE, Deviceless)            |          |   overlay (F10), capture, errors     |
|   weather, settings, mge API, vis groups |          |                                      |
|     extension records 1-8  ------------------------>  DistantLand (impl Extension)        |
| MWSE.dll + Lua mods: mge.*, nirender.*   |          |   land, statics, grass, shadows,     |
+------------------------------------------+          |   water, volumes, lights, post       |
                                                      +--------------------------------------+
```

- `d3d8.dll` loads `Data Files\MWSE\lib\nirender.dll` and hands the game the plugin's own
  `IDirect3D8`. The plugin replaces the engine's renderer functions, writes each frame as
  records into the ring, and starts the helper.
- `Deviceless::present` (`d3d8/cpp/mge/mged3d8device.cpp`) adds MGE XE's own state as extension
  records. `DistantLand::init` stops early without a device, so the distant land of
  `d3d8.dll` stays unloaded; the helper reads the data files itself (see
  [distantland-data.md](distantland-data.md)).
- The helper draws in absolute f32 world space. The game's draws are camera-relative through
  the f64 origin of the scene record.

### Extension records (game to helper)

| Id | Record | Kept | Sent |
| --- | --- | --- | --- |
| 1 | environment (fog, sun, weather, scattering, camera flags) | no | every Present |
| 2 | shader state (chain, enabled flags, variables) | no | on a change |
| 3 | picture request (`mge.saveScreenshot`, the screenshot macro) | no | at Present |
| 4 | settings (`MGEFlags`, distances, water, lighting mode) | no | on a change |
| 5 | tune text (`nirender.tune`) | no | from Lua |
| 6 | water looks | yes | on a change |
| 7 | water masks (dry spaces) | yes | on a change |
| 8 | vis groups | yes | on a cell change or a load |
| 9 | status text (`StatusOverlay::setStatus`): warning, ms to show, text | no | at Present after a new text |
| 10 | the place of a wait of a game load (`distant_land.wait_on_load`): version, serial, position, interior name | no | at the resolve hook of a load |

A kept record (bit `0x80000000`) stays in the stream, so a capture file holds it. Use kept
records for state. The next free id is 11. The ids are constants at the top of `renderer.rs`
and in `Deviceless`; each side names the other.

The environment flags (record 1): 1 exponential fog, 2 under water, 4 an exterior or an
interior like one, 16 in a water volume, 32 in a dry space, 64 in a menu, 128 a mouse click
in this frame, 256 a game was loaded (in one record only; the helper latches it in `data()`,
because a loading screen has no world frame).

### Messages (helper to game)

The helper's `Extension::messages` go at the end of each live frame into the shared memory
`<ring>.messages` (a ring of number, length, bytes; 64 KB; the plugin makes it). The plugin
reads it at each frame end and calls `MGE_StreamMessage(id, data, length)` of `d3d8.dll`
(`Deviceless::message`). A replay drops the messages.

| Id | Message | Sent |
| --- | --- | --- |
| 1 | distant land: version 1, reason 1 loaded / 2 arrived, the serial of the wait it answers (0 none) | `LOADED` once; `ARRIVED` after a game load or a jump of more than 2 cells, when `Statics::settled` (or after 10 s) |

The wait of a game load (`Deviceless::waitForDistantLand`): the game sends record 10 and
draws its loading bar until `ARRIVED` with that serial comes back (or 15 s). The frames of
the loading bar have no world: the crate calls `Extension::after_frame(scene)` at the end of
each live frame, and there the helper finishes its loads and streams the statics near the
place (`Statics::prefetch`; those arrivals show with no fade-in).

`d3d8.dll` queues the reasons (`distantlandready.h`, also filled by the Direct3D 9 path); the
Lua of MGE XE (`MGE XE Options/events.lua`) takes them through the FFI export
`MGE_DistantLandReadyNext` and triggers the MWSE event `distantLandReady { reason }`.

## 2. Device

`Renderer::new` (`nirender_wgpu`, `host/src/lib.rs`):

- Instance flags from the environment (`with_env`: `WGPU_VALIDATION`, `WGPU_DEBUG`, ...).
  Adapter: high performance, compatible with the surface. Backend: `[runtime]
  renderer_backend` (`auto`, `vulkan`, `dx12`).
- Features: `TEXTURE_COMPRESSION_BC` always. Only when the adapter has them:
  `POLYGON_MODE_LINE` (wireframe pipelines), `TIMESTAMP_QUERY` and
  `TIMESTAMP_QUERY_INSIDE_ENCODERS` (GPU times), `INDIRECT_FIRST_INSTANCE` and
  `MULTI_DRAW_INDIRECT_COUNT` (GPU culling of the distant statics; without them the CPU
  chooses the places).
- Limits: the wgpu defaults, except `max_texture_dimension_2d` = min(adapter, 16384),
  `max_sampled_textures_per_shader_stage` = min(adapter, 32), `max_buffer_size` = the
  adapter's. So there are 4 bind groups and 8 storage buffers per stage; the statics cull uses
  all 8 in one stage.
- Formats: depth `Depth24PlusStencil8` everywhere (stencil bit `0x02` counts the dry spaces).
  The game draws into an off-screen back buffer (`Bgra8Unorm`, else the first non-sRGB
  format of the surface; `Rgba8Unorm` in a replay), copied to the surface at the end of the
  frame. No MSAA anywhere.
- Surface: `AutoNoVsync`, frame latency 2.

## 3. The frame

### 3.1 Command buffers and submits

One frame is one submit: `[deferred buffers of the extension, in defer order..., main
encoder]`. An extension never submits. For GPU work before the frame it records its own
encoder and calls `scene.commands.defer(encoder.finish())` (`FrameCommands`); the host sends
those buffers first. `queue.write_buffer` is staged until that submit, so it lands before
every buffer of the frame: a buffer that two deferred encoders read must not get two values
in one frame.

Other submits happen only on demand: a game read of pixels or a copy into a render texture
mid-frame (a flush), picture files, the growth of the statics pools, a statics report, and
`submit_now` for the encoders timed with `MGE_XE_GPU_TIME=1` (replay only).

### 3.2 Order

Recording order in the main encoder; the deferred buffers run before all of it.

```mermaid
flowchart TD
    subgraph deferred["Deferred command buffers (run first, same submit)"]
        R["ripples: rain and player waves (Rgba16Float ping-pong)"]
        C["distant statics, cull: compute per set (main, mirror, 2 shadow layers)"]
        L["distant lights, cull: compute"]
        S["shadow map: land + statics into R16Float atlas, then 2 blur passes"]
        W["water reflection: land, statics (mirror set), sky replay; mip levels"]
    end
    subgraph main["Main encoder"]
        P0["game pass: sky draws (back buffer + D24S8)"]
        P1["before_world: distant land, statics set 0, glows"]
        P2["copies: picture before world, depth; game pass reopened, depth cleared"]
        P3["game world draws (game draw shader + extension frame group)"]
        P4["class 10 replaced: MGE water plane (dry-space stencil)"]
        P5["after_world: grass"]
        P6["blend pass: distant land under the game's picture"]
        P7["draw_held: sea caustics, far water, water volumes"]
        P8["translucent and first-person draws"]
        P9["post: HDR, depth, the shader chain, blit (own passes)"]
        P10["UI draws"]
        P11["copy to surface, overlay pass, present"]
    end
    R --> C --> L --> S --> W --> P0
    P0 --> P1 --> P2 --> P3 --> P4 --> P5 --> P6 --> P7 --> P8 --> P9 --> P10 --> P11
    S -. shadow atlas .-> P1
    S -. shadow atlas .-> P3
    W -. reflection .-> P4
    C -. indirect draws .-> P1
    C -. indirect draws .-> W
    C -. indirect draws .-> S
    L -. glow list .-> P1
    P2 -. depth copy .-> P7
```

| Step | Extension hook | Pass | Condition |
| --- | --- | --- | --- |
| frame values, ripples | `frame` (first world draw) | deferred | ripples on and game time moved |
| statics cull | `before_world` -> `Statics::prepare` | deferred compute | GPU culling |
| lights cull | `before_world` -> `DistantLights::prepare` | deferred compute | night, exterior, `lights.data` loaded |
| shadow map | `before_world` | deferred ("shadow map", "shadow blur") | shadows on, distant cell |
| water reflection | `before_world` | deferred ("water reflection", levels) | water in view |
| land, statics, glows | `before_world` | game pass | distant cell |
| water plane | `draw_class` (class 10) | game pass | water cell |
| volumes in place | `draw_held_in_place` | game pass | a surface with its own stencil |
| grass | `after_world` | game pass | distant cell |
| blend | `blend` | own pass, colour only | `before_world` drew |
| caustics, far water, volumes | `draw_held` | game pass | held shapes or far water |
| post | `post` | own passes | chain not empty |

The host opens a new game pass after every copy, target change or extension pass, and calls
`BoundGroups::forget` there, so groups 0 to 2 of the game draws are set again.

## 4. Pipelines and bind groups

Group 0 changes least and the last group changes most, within each pipeline. No pipeline has
an empty group, an automatic layout or immediate (push) constants. "Rate" is how often the
bound group object changes. V = vertex, F = fragment, C = compute.

### 4.1 Host: the game's draws (`nirender_wgpu`)

**"game draw"**: one pipeline per `PipelineKey` (vertex format, topology, blend, depth,
stencil, cull, fill, colour mask), one layout "draw" for all. WGSL: `host/src/shader.wgsl`
plus the extension's `draw_shader` text (its `draw.` becomes `draws[record].` when the
shader is made). Vertex buffers: slot 0 the game's vertices (a chunk of the vertex pool:
the vertices of all meshes in buffers of 64 MB; the draw names its base vertex), slot 1 a
"vertex defaults" buffer (instance step, zeros, as long as the instance buffer) for
attributes a vertex lacks, slot 2 the instances (80 bytes: the world matrix at locations
9-12 and the index of the draw's record at 13). Indices `Uint16` in chunks of the index
pool (16 MB); the draw names its first index. Since 2026-10-16 (plan 0.5 entries TD1, TD3).

| Group | Layout | Bindings | Rate |
| --- | --- | --- | --- |
| 0 | extension frame | 0 uniform `FrameParameters` (512 B), 1 texture (shadow atlas or white), 2 sampler, 3 uniform cluster grid, 4-5 storage cluster table and indexes, 6 storage lights of the frame, 7 storage bone palette (V), 8 storage records of the draws (`array<Draw>`, VF) | frame |
| 1 | draw texture arrays (an adapter with binding arrays) | 0 `binding_array<texture_2d>` of the frame's textures (4096 at most), 1 `binding_array<sampler>` (64) (F); a stage reads by the slots in its record | pass (a variant for a pass into a rendered texture of the game) |
| 1, 2 | draw samplers, draw textures (without binding arrays, or `MGE_XE_ARRAYS=0`) | 0-5 sampler, 0-5 texture per stage (F) | material |

The groups are set only when they change (`BoundGroups`), as are the pipeline, the stencil
reference and the buffers of slot 0 and the index buffer; slots 1 and 2 once a pass. A draw
is `draw_indexed(first index.., base vertex, first instance..)`: its record is read by
`draws[instance.record]` in both stages. A copy of a draw (clipped at a water plane, replayed
into a reflection) takes a record and instances from the end of the room of the frame.

**"screen draw"**: film frames and screen-space draws. Group 0: texture, sampler (F); a new
group per draw. Depth test always, no depth write.

**overlay**: made by `egui_wgpu` inside the crate.

### 4.2 Helper: the view group, distant land and statics

**The view group** (`renderer/view.rs`, `ViewBinding`) is group 0 of each helper pipeline
that draws into the world: the land, the statics, the glows, the grass, the water, the dry
spaces, the caustics and the water volumes. One layout, and one group object for each view of
the frame, each with its own uniform buffer:

| View | Number | Projection | Pipelines |
| --- | --- | --- | --- |
| main | 0 | the land's (far plane at the draw distance) | land, statics set 0, glows |
| mirror | 1 | the mirror of the water | land and statics in the reflection |
| shadow 0, shadow 1 | 2, 3 | the layers of the shadow map | land shadow, statics shadow |
| game | 4 | the game's | grass, water plane, dry spaces, caustics, water volumes |

| Binding | Holds | Stage |
| --- | --- | --- |
| 0 | uniform `View` (`u`): view projection, eye, fog colour, fog (exponential start and divisor, linear start and end), flags, sun direction, sun colour, ambient, the scattering (sun position, sky, outscatter, inscatter, sun altitude) | VF |
| 1-4 | the baked light of the distant lights (`LightBinding`): uniform, texture of the tiles, texture `Sint` index, sampler | VF |

The number of a view is the number of a statics set. The groups are made again when the
baked light changes (`LightBinding::generation`). `before_world` writes the main, mirror and
shadow views, and the game view at its start, in each frame with a world. The values of one
pipeline only are in its group 1: `eye.w` of the view is 0 (the view range of a handoff, the
time and the water level are values of a pipeline).

**"distant land"** (fill, lines, mirror fill, mirror lines; `make_gpu` in `renderer.rs`).
Vertices: 20 bytes (`Float32x3`, `Unorm8x4`, `Unorm8x4`), indices `Uint32`.

| Group | Layout | Bindings | Rate |
| --- | --- | --- | --- |
| 0 | view | as above | view (main or mirror) |
| 1 | distant land | 0 uniform `Land` (VF), 1-5 textures atlas, material, flags, albedo, patterns, 6 sampler (F) | fixed |

**"distant land shadow"**: the view group only (one per shadow layer). R16Float target.

**"distant statics"** (back/front cull x fill/lines; `make_gpu` in `renderer/statics.rs`).
Vertices: 20 bytes (`Float16x4` position and palette number, `Unorm8x4` normal, `Unorm8x4`
colour, `Float16x2` uv); instance 80 bytes (locations 4-8); indices `Uint16`. Draws:
`multi_draw_indexed_indirect_count` per texture, or the CPU path.

| Group | Layout | Bindings | Rate |
| --- | --- | --- | --- |
| 0 | view | as above | set (the view of the same number) |
| 1 | distant statics | 0 uniform `Set` (range, handoff, fade) (VF), 1 storage palettes (V), 2 sampler (F), 3 storage arrivals (V) | set (main, mirror, shadow 0, shadow 1) |
| 2 | distant statics, textures | 0 texture (F) | texture |

The fade-in (2026-10-15): binding 3 has the arrival time of each subset (real time of the
helper; written in `take_arrivals`), the instance data has the subset number in `subset.w`,
and the uniforms have `fade` (now, fade time from the tunable `statics.fade`; 0 in a
replay). The statics shader multiplies the handoff dissolve by the share that is in.

**"distant statics shadow"**: the same pipeline layout, with the view of its shadow layer.
R16Float target. Its casters fade in with the same threshold.

**"grass"** (fill, lines): group 0 the game view; group 1 "grass" (uniform `Grass`, sampler,
the shadow atlas and its clamp sampler; frame); group 2 the statics texture layout (texture
run). The grass reads the view projection, the eye, the sun direction and the linear fog
(`u.fog.zw`) from the view. Its own uniform has the end of the grass, the fog colour of the
near cells, its sun colour and ambient light (also in a frame without a sun direction, where
the view has none), the wind, the player, the two world-to-layer matrices of the shadow map
and the looks. The atlas is a 1 x 1 texture until the session has a shadow map. Instance 64
bytes (locations 4-7; 7.x is the share of the subset that is in, computed on the CPU).

**"distant statics, cull"** (compute, `renderer/statics/cull.rs`; entries `cull` and
`compact`, workgroup 256). Group 0, one per set: 0 uniform, 1-5 storage read (places,
subsets, vis groups, world list, buckets), 6 storage flags, 7 storage indirect draws, 8
storage counts (atomics). `cull` runs over the places, `compact` writes the draws of each
texture in a fixed order.

**"distant lights, cull"** (compute, `renderer/distant_lights.rs`): group 0: uniform,
sources, glow list, indirect arguments (atomics). **"distant lights, glows"**: group 0 the
main view; group 1: uniform `Glow` (range, glow, screen) (VF), glow list (V);
`draw_indirect`, no vertex buffers, additive, no depth write.

### 4.3 Helper: shadows

**"shadow blur"** and **"shadow debug"** (`renderer/shadow.rs`) share one layout: 0
uniform (VF), 1 texture, 2 sampler (F). Blur: 2 groups, one per pass, R16Float. Debug: with
`MGE_XE_SHADOW_DEBUG`.

### 4.4 Helper: water

**"water"** (fill, lines, cut; `renderer/water.rs`): group 0 the game view; group 1 the
shared water layout, a new group each frame (`gpu::frame_group`). The water reads the
camera, the exponential fog (`u.fog.xy`), the far fog colour and the scattering from the
view.

| Binding | Holds | Stage |
| --- | --- | --- |
| 0 | uniform `Water` (`water`): view matrix, plane, time, sun, sky colour, debug and underwater, projection, size, waves, mirror, land | VF |
| 1, 2 | reflection texture, clamp sampler | F |
| 3, 4 | waves (3D texture), wave sampler | VF |
| 5 | copy of the picture | F |
| 6 | copy of the depth (depth texture) | F |
| 7, 8 | rain and player ripples | F |
| 9 | depth of the reflection | F |
| 10 | depth before the world, or a 1x1 "no land" depth | F |
| 11-16 | the light of the frame for the water volumes: uniform `lighting`, shadow atlas, storage lights, uniform clusters, storage cluster table and indexes; stand-ins in the group of the plane | F |

The light of the frame is in the water group, not in the view group: the crate gives the
lights of the frame to an extension only in `draw_held`.

**"water reflection level"**: group 0 texture, sampler; one group per mip level.

**"water dry spaces"** (zero, count plane, count behind; `renderer/water/dry.rs`): the game
view and the water layout as groups 0 and 1; stencil writes only, no colour.

**"water volume"** and **"water volume, far"** (`renderer/water/volume.rs`): one pipeline per
key; a mod water shader adds its own module.

| Group | Layout | Bindings | Rate |
| --- | --- | --- | --- |
| 0 | view | the game view | frame |
| 1 | water | as above, with the light of the frame | call |
| 2 | water volume mesh | 0-1 textures of the mesh, 2 sampler (F) | mesh (mod shaders: one group per texture pair a frame) |
| 3 | water volume | 0 uniform `Surface`, dynamic offset (VF) | draw (offset) |

**"caustics"** (plain, cut): group 0 the game view (the camera, the sun direction, the linear
fog); group 1: uniform `Caustics` (`c`), depth, waves, sampler (F); a new group each frame.
Multiply blend.

**"ripples"** (step, player): group 0: texture, sampler, uniform with a dynamic offset; one
group per ping-pong texture.

### 4.5 Helper: blend and post

**"blend"**: group 0: uniform, picture (non-filterable), depth (F); a new group each frame.
Alpha blending, colour only.

**post chain** (`renderer/post.rs`): one pipeline per shader pass, one pipeline layout
"post shaders" for all, three groups:

| Group | Bindings | Rate |
| --- | --- | --- |
| 0 frame | 0 uniform of the variables (`u`), 1 linear depth `depthframe` (non-filterable), 2 waves `watertexture` (3D), 3-8 the six samplers | frame (one group) |
| 1 shader | 0 uniform `modv`, 1-10 the shader's own textures `tex1` to `tex10` | shader (made at load, kept with the shader) |
| 2 pass | 0 uniform pass information, 1 `lastshader`, 2 `lastpass`, 3-10 `pass0` to `pass7` | pass |

**"post depth"** (R32Float), **"post blit"** and **"post hdr"** (one `blit_layout`), and
**"post debug"** (`MGE_XE_POST_DEBUG`) are full-screen passes with one group each.

## 5. Conventions for a change

- **One submit a frame.** Defer pre-frame GPU work; `submit_now` only where the result is
  read at once.
- **Bind groups from least to most changing**: group 0 the frame, the last group the draw.
  Use a dynamic offset into one pooled uniform buffer for per-draw values.
- **Uploads with `queue.write_buffer`**; no buffer made per frame. A bind group made each
  frame goes through `gpu::frame_group`, so the marks count it.
- **A label on every GPU object**, and a debug group around each part drawn into the game's
  pass, so a capture tool and a validation message name it.
- **Errors**: `gpu::watch` logs uncaptured errors (`gpu error N (kind, adapter ...)`) and
  the program goes on; the broken draw or pass does nothing. A lost device is logged, a
  message box comes up, and the host drains the stream. Post shaders of mods compile inside
  an error scope. After a shader or layout change, look for `gpu error` in the log.
- **Group 0 is the view group** (`view.rs`) for a helper pipeline that draws into the
  world: put a value that each pipeline of a view shares into `View`, and a value of one
  pipeline into its group 1. A uniform struct in Rust gets a test of its size against its
  WGSL struct.
- **WGSL** lives in Rust string constants, composed with `format!` (for example
  `view::wgsl()` starts each shader of the view group with `View` and the baked light).
  Mod shaders are `.wgsl` files: post shaders (see
  [`mgeHost64/POST_SHADERS.md`](../../mgeHost64/POST_SHADERS.md)) and water shaders (see
  [`water-shaders.md`](../water-shaders.md)).
- **Features**: ask only for what the adapter has, and keep a fallback.
- **Switches**: an environment switch `MGE_XE_<NAME>` for an A/B and a fallback; GPU times
  with `MGE_XE_GPU_TIME=1`.

## 6. Review of the bind group layouts (2026-10-15, closed)

The review found six points that a reader who knows the usual layout would ask about. None
was a cost: the bind groups made each frame cost 0.067-0.085 ms (18-21 groups). The owner
chose to change them for the reader (plan item D1). Each step was equal to the pixel in a
replay A/B of 32 captures against the build before (baseline, water volumes with a mod
water shader, grass by day and at night), with no GPU errors.

1. **Group 0 meant something else in each pipeline**: the baked light (land, statics), grass
   values, the glow view, water, caustics. Now group 0 is the view group (section 4.2) in each
   helper pipeline that draws into the world (MGE c80148c, 341176c, b97394f, 3b04a57,
   223cbb1). The game's draws keep the crate's own group 0 (the extension frame): the helper
   draws with the projection of the land, so the two cannot share one group object, and a
   group 0 shared with the crate would change the interface of both repos.
2. **One layout at two group numbers**: the "distant land" layout was group 1 of the land
   and group 0 of the land shadow; the statics texture layout was group 2 of the statics and
   group 1 of the grass. Now the land shadow has the view group only, and the texture layout
   is group 2 in both.
3. **Bindings that a shader did not read**: the statics shadow pipeline had the baked-light
   group in its layout; the land shadow group had the five land textures. Now both shadow
   pipelines read their view from group 0.
4. **Per-frame data in the per-draw group**: group 3 of the game draws held the lights of the
   frame and the bone palette. They are now bindings 6 and 7 of the crate's group 0, and
   group 3 is the per-draw slot only (nirender-plugin 086533b).
5. **Post: one group of 31 bindings, made for each pass**. Now three groups: frame, shader
   (kept with the compiled shader) and pass (MGE 17897f3). The names in the post shaders did
   not change.
6. **Water volumes**: the light group was set again for each surface, and a mod water shader
   got a new mesh group for each draw. The light is set once for each call (MGE dd84691) and
   is now part of the water group (223cbb1); the mesh groups of mod shaders are kept for a
   frame by texture pair (e257386).
