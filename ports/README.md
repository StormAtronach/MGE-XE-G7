# The shader packs of the wgpu renderer

Each folder here is one pack: the `.wgsl` files that port the MGE XE shaders of one mod to the
wgpu renderer. The name of a pack is `<Mod> - wgpu shaders`. Its folders are those of
`Data Files`, so a pack installs as a mod of its own (in Mod Organizer, a mod placed after the
mod that it ports), or by a copy of its folders into `Data Files`.

A pack holds only `.wgsl` files. Direct3D 9 reads the mod's own `.fx` files; the wgpu renderer
reads the `.wgsl` file of the same name:

- a post shader: `shaders/XEshaders/<name>.wgsl` (POST_SHADERS.md of mgeHost64);
- a core mod: `shaders/core-mods/XE Mod <name>.wgsl` (docs/core-mods.md).

| Pack | Its mod |
| --- | --- |
| Enhanced Water Shader - wgpu shaders | Enhanced Water Shader 2.2 Green-Blue by Krokantor, updated for MGE XE 0.13.0 by Hrnchamd |
| Ashfall - wgpu shaders | Ashfall (`ashfall.fx`) |
| Watch the Skies - wgpu shaders | Watch the Skies (its `Sunshafts.fx`) |
| Eternal Shine - wgpu shaders | Eternal Shine |

## The exported packs

The renderer has ports built in, which it finds by the code key of the mod's `.fx` (post.rs
`MOD_FILES`). The test `export_packs` of `mgeHost64/src/renderer/post_export.rs` writes such a
port as a pack file. The file holds the port as the renderer runs it for the mod's real `.fx`
(the `//!` header, then the WGSL with the file's numbers in the lines that end with
`// file`). The tests of that module check each file against its port. The wgpu renderer reads
`<name>.wgsl` before `<name>.fx`, so a number changed in the `.fx` does not reach the wgpu
picture: change it in the `.wgsl`.

The `.fx` copies that each exported pack was made from. All copies of a pack give the same port.

- `Ashfall - wgpu shaders/shaders/XEshaders/ashfall.wgsl`: `ashfall.fx` of
  `mods/Ashfall - A Camping Survival and Needs Mod`, md5 a4719efd5ca35fd57fc90a379ab9926e in
  Morrowind Refreshed, Morrowind100, Morrowind75 - Poison Song, Morrowind75, Morrowind75testing,
  Outlander and Outlander-test; md5 318790efad83358df2940cfa5d287469 in Morrowind Refreshed
  (260602).
- `Watch the Skies - wgpu shaders/shaders/XEshaders/Sunshafts.wgsl`, the port "Sunshafts of
  Watch the Skies": `Sunshafts.fx`, md5 e9784b6a494fba25f6c4476b8328a24d, of
  `mods/Watch the Skies` in Outlander and Outlander-test and of
  `mods/Watch the Skies - Dynamic Weather and Sky Effects` in Morrowind Refreshed (260602). The
  older Watch the Skies of Morrowind Refreshed (md5 a0265743472e0aa94336b530a830c9d0) is
  another shader and needs a pack of its own. `Sunshafts.wgsl` has the name of a stock shader,
  so the gate's game folder does not take it (D640 piece 2, addendum 2): a change of this file
  is not under the gate, and replays check it.
