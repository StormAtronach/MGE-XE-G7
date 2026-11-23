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
| Joy of Painting - wgpu shaders | Joy of Painting (its 23 `jop_*.fx` shaders) |
| Magicka Expanded - wgpu shaders | Magicka Expanded (`oj_me_fog_box.fx`) |
| Skoomaesthesia - wgpu shaders | Skoomaesthesia (`skoomaesthesia.fx`) |
| MM - Enhanced Invisibility - wgpu shaders | MM - Enhanced Invisibility (`Invisibility.fx`) |
| Heat Haze - wgpu shaders | Heat Haze - A MGE XE Shader Mod (`heathaze.fx`) |
| Vapourmist - wgpu shaders | Vapourmist (`tew_fogbox.fx`) |

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
- `Joy of Painting - wgpu shaders/shaders/XEshaders/jop_*.wgsl`, 23 files: the `jop_*.fx` of
  `mods/Joy of Painting` in Morrowind Refreshed, Morrowind Refreshed (260602), Morrowind75 -
  Poison Song, Morrowind75testing, Outlander and Outlander-test (138 copies; each file has one
  md5 in all of them). The parts that the `.fx` files include (`jop_common.fx`, `jop_gaussian.fx`
  and the colour lookup) are written into each file that uses them. The older Joy of Painting of
  Morrowind100 and Morrowind75 is dropped (D654): its `jop_dof.fx` has `priorityAdjust = 67`, this
  pack's `jop_dof.wgsl` has 2, so with this pack `jop_dof` takes another place in the chain there.
- `Magicka Expanded - wgpu shaders/shaders/XEshaders/oj_me_fog_box.wgsl`: `oj_me_fog_box.fx`,
  md5 4d98da2b68af03585773483f5c92c53c, of `mods/Magicka Expanded` in all 8 installs.
- `Skoomaesthesia - wgpu shaders/shaders/XEshaders/skoomaesthesia.wgsl`: `skoomaesthesia.fx`,
  md5 0cf040ab916b7d50e613c4a98c9a2244, of `mods/Skoomaesthesia` in all 8 installs.
- `MM - Enhanced Invisibility - wgpu shaders/shaders/XEshaders/Invisibility.wgsl`:
  `Invisibility.fx`, md5 144556a6e16db5931ef74b597b4486a6, of `mods/MM - Enhanced Invisibility`
  in all installs but Morrowind75.
- `Heat Haze - wgpu shaders/shaders/XEshaders/heathaze.wgsl`: `heathaze.fx`, md5
  107c674f342c078c631c81eaf03023e4, of `mods/Heat Haze - A MGE XE Shader Mod` in Morrowind
  Refreshed, Morrowind Refreshed (260602), Outlander and Outlander-test, and of our
  `mods/Shader port tests` in Morrowind75testing.
- `Vapourmist - wgpu shaders/shaders/XEshaders/tew_fogbox.wgsl`: `tew_fogbox.fx`, md5
  2348d7e9db2854c4bc823fbad66ea1c6, of `mods/Vapourmist` in Outlander and Outlander-test and of
  `mods/Vapourmist - A Cloud and Mist Mod` in Morrowind Refreshed (260602).
