# The shader packs of the wgpu renderer

Each folder here is one pack: the `.wgsl` files that port the MGE XE shaders of one mod to the
wgpu renderer. The name of a pack is `<Mod> - wgpu shaders`. Its folders are those of
`Data Files`, so a pack installs as a mod of its own (in Mod Organizer, a mod placed after the
mod that it ports), or by a copy of its folders into `Data Files`.

A pack holds only `.wgsl` files. Direct3D 9 reads the mod's own `.fx` files; the wgpu renderer
reads the `.wgsl` file of the same name:

- a post shader: `shaders/XEshaders/<name>.wgsl` (POST_SHADERS.md of mgeHost64);
- a core mod: `shaders/core-mods/XE Mod <name>.wgsl` (docs/core-mods.md);
- a water look: `shaders/water/<name>.wgsl`, the name of the look line's `shader=`.

| Pack | Its mod |
| --- | --- |
| Enhanced Water Shader - wgpu shaders | Enhanced Water Shader 2.2 Green-Blue by Krokantor, updated for MGE XE 0.13.0 by Hrnchamd |
| Ashfall - wgpu shaders | Ashfall (`ashfall.fx`) |
| Watch the Skies - wgpu shaders | Watch the Skies (its `Sunshafts.fx` and `XE Mod Sky.fx`) |
| Watch the Skies (older, Morrowind Refreshed) - wgpu shaders | the older Watch the Skies of Morrowind Refreshed (its `Sunshafts.fx` and `XE Mod Sky.fx`) |
| Joy of Painting - wgpu shaders | Joy of Painting (its 23 `jop_*.fx` shaders) |
| Magicka Expanded - wgpu shaders | Magicka Expanded (`oj_me_fog_box.fx`) |
| Skoomaesthesia - wgpu shaders | Skoomaesthesia (`skoomaesthesia.fx`) |
| MM - Enhanced Invisibility - wgpu shaders | MM - Enhanced Invisibility (`Invisibility.fx`) |
| Heat Haze - wgpu shaders | Heat Haze - A MGE XE Shader Mod (`heathaze.fx`) |
| Vapourmist - wgpu shaders | Vapourmist (`tew_fogbox.fx`) |
| Eternal Shine - wgpu shaders | Eternal Shine (`Eternal Shine.fx`; the lights of the frame and the weather of the game, W622, in place of its Lua) |
| Ultra Bloom - wgpu shaders | Ultra Bloom, the current version (`Ultra Bloom.fx`, `XERO-lut.fx`); no install has its `Ultra Bloom.fx` |
| STEP Shaders Compilation - wgpu shaders | STEP Shaders Compilation (its 6 shaders) |
| Morrowind Graphics Guide Shaders - wgpu shaders | Morrowind Graphics Guide Shaders (its 5 shaders that are not stock) |
| Outlander Configuration - wgpu shaders | Outlander Configuration (its 3 shaders) |
| Kogoruhn Expanded - wgpu shaders | Kogoruhn Expanded (`Disorient.fx`) |
| Inspect It - wgpu shaders | Inspect It (`InspectIt/Bokeh.fx`) |
| Modern Lockpicking (MWSE) - wgpu shaders | Modern Lockpicking (MWSE) (`modern-lockpicking/Bokeh.fx`) |
| Pixel Shader Style Water for MGE XE - wgpu shaders | Pixel Shader Style Water for MGE XE 1.0 by Hrnchamd (its `XE Mod Water.fx`, md5 7f40c5546d36b84cbdc83b2830def8ff, in Morrowind Refreshed and Morrowind Refreshed (260602)) |
| CRT Shader - wgpu shaders | CRT Shader (`CRT.fx`, md5 6a8b5ea4e1f7bc11074aa8e4752abed7, and `CRT Zoomed Out.fx`, md5 0be439714569ad6176827e796595ae15, in Outlander) |
| Darker Nights - wgpu shaders | `Darker Nights.fx`, md5 85d20186c83457f61cc449dcfa1544d3: a file of MGE XE v0.20.0 that MGE XE removed (d2ed258), in `mods/MGE XE G7 Fork` of Morrowind75 - Poison Song and in the game folder of Morrowind Refreshed (260602). Written by hand from the `.fx` |
| True Water - Odai - wgpu shaders | `tw_foam.fx`, md5 10c758a2be9a6e447316c655798409eb, of the True Water showcase `True Water - Odai`. Written by hand from the `.fx` |
| True Water - Flow Map - wgpu shaders | `tw_flowmap.fx`, md5 5439e388e2823c125485b09eebc912c4, of the True Water showcase `True Water - Flow Map`. Written by hand from the `.fx` |

Two packs give `shaders/core-mods/XE Mod Water.wgsl`: Enhanced Water Shader and Pixel Shader
Style Water. Install one water pack at a time, the one of the water mod that is on. The test
`each_water_file_of_the_packs_is_used` of `core_mods.rs` checks each water file of the packs.

Three packs give `shaders/XEshaders/Sunshafts.wgsl`: Watch the Skies, the older Watch the Skies
and STEP Shaders Compilation. Two packs give `shaders/core-mods/XE Mod Sky.wgsl`: Watch the Skies
and the older Watch the Skies. Install one pack of a mod at a time, the pack of the version of
the mod that is on.

## The written ports

A core mod has no built-in port, and some post shaders of mods have none: their pack files are
written by hand from the mod's `.fx`.

- `Watch the Skies - wgpu shaders/shaders/core-mods/XE Mod Sky.wgsl`: the `clouds_color` hook,
  from `CloudsPS` of `XE Mod Sky.fx`, md5 9ec82f14916051527a12be3577689c53, of
  `mods/Watch the Skies` in Outlander and Outlander-test and of
  `mods/Watch the Skies - Dynamic Weather and Sky Effects` in Morrowind Refreshed (260602). The
  `.fx` changes only `CloudsPS`, so the file has only that hook.
- `Pixel Shader Style Water for MGE XE - wgpu shaders/shaders/core-mods/XE Mod Water.wgsl`: the
  four water hooks, from `XE Mod Water.fx`, md5 7f40c5546d36b84cbdc83b2830def8ff, of
  `mods/Pixel Shader Style Water for MGE XE` in Morrowind Refreshed and Morrowind Refreshed
  (260602).
- `CRT Shader - wgpu shaders/shaders/XEshaders/CRT.wgsl` and `CRT Zoomed Out.wgsl`: from
  `CRT.fx` (md5 6a8b5ea4e1f7bc11074aa8e4752abed7) and `CRT Zoomed Out.fx` (md5
  0be439714569ad6176827e796595ae15) of `mods/CRT Shader` in Outlander. The two files differ only
  in their switches and the zoom of the curve; the switches are constants in the `.wgsl`.
- `Watch the Skies (older, Morrowind Refreshed) - wgpu shaders/shaders/XEshaders/Sunshafts.wgsl`:
  from `Sunshafts.fx`, md5 a0265743472e0aa94336b530a830c9d0, of
  `mods/Watch the Skies - Dynamic Weather and Sky Effects` in Morrowind Refreshed. It is the stock
  Sunshafts with other numbers, two of them in expressions, so the stock port does not take them:
  the file is the stock port with all the numbers of the `.fx`. The test
  `the_older_watch_the_skies_sunshafts_is_the_stock_port_with_its_numbers` of `post_export.rs`
  checks it.
- `Watch the Skies (older, Morrowind Refreshed) - wgpu shaders/shaders/core-mods/XE Mod Sky.wgsl`:
  from `XE Mod Sky.fx`, md5 56b31f168d28ae2453e019552a53308e, of the same mod in Morrowind
  Refreshed. It is the current Watch the Skies' `XE Mod Sky.fx` with 9 other numbers: the file is
  the port of the pack "Watch the Skies - wgpu shaders" with these numbers.

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
  another shader, with a pack of its own (above). `Sunshafts.wgsl` has the name of a stock shader,
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
- `Ultra Bloom - wgpu shaders/shaders/XEshaders/`, 2 files:
  - `Ultra Bloom.wgsl`: the port with its own numbers. No install has the `Ultra Bloom.fx` of the
    current version (the source of the port), so the file has no copy to take numbers from. The
    `Ultra Bloom.fx` of Morrowind100 is another version (md5 9685c45d1a98097cc6522601c6093555),
    dropped by the owner's D653: this pack is not for Morrowind100.
  - `XERO-lut.wgsl`: `XERO-lut.fx`, md5 726f9a2c9fead8de1a3eea9be3d0d4e8, of `mods/Ultra Bloom`
    in Morrowind100.
- `STEP Shaders Compilation - wgpu shaders/shaders/XEshaders/`, 6 files, from
  `mods/STEP Shaders Compilation` in Outlander and Outlander-test:
  - `Eye Adaptation (HDR).wgsl`, the port "Eye Adaptation (HDR) of STEP": md5
    b5d03bec06e7d3ad756130468c77183c;
  - `gammavtdt.wgsl`: md5 f0fe6580bfe5fb0f490a3b71c371b85d;
  - `SSAO HQ.wgsl`, the port "SSAO HQ of STEP": md5 538190720e77be4cfbef2f6f03dd00d3;
  - `Sunshafts.wgsl`, the port "Sunshafts of STEP": md5 31bbf78034d43613d6728d9d3a4309fb;
  - `alternatebloom.wgsl`, the port "Bloom Soft without the depth pass": md5
    3d5b58e5eafd043623a83f7f14b78716, also in our `mods/Shader port tests` in Morrowind75testing;
  - `ColorMood.wgsl`: md5 e19e7adafcb743c1a091be6293b9a6e5, also in our `mods/Shader port tests`
    in Morrowind75testing.

  `Eye Adaptation (HDR)`, `SSAO HQ` and `Sunshafts` have the names of stock shaders, so the gate's
  game folder does not take them (D640 piece 2, addendum 2): a change of these files is not under
  the gate, and replays check it.
- `Morrowind Graphics Guide Shaders - wgpu shaders/shaders/XEshaders/`, 5 files, from
  `mods/Morrowind Graphics Guide Shaders` in Outlander and Outlander-test, the game folder of
  Outlander (`Stock Game Folder/Data Files`) and our `mods/Shader port tests` in
  Morrowind75testing, one md5 for each file:
  - `Bloom Soft Sky.wgsl`, the port "Bloom Soft": md5 e9b8120b91b4b83412fdb359443e6f6d;
  - `deband_fogaware.wgsl`: md5 5b914e2f5b4bfa3ba3daa8dc31a43a25;
  - `film.wgsl`: md5 c76214bec9b9ec38fa80d217020678ad;
  - `MGG darker interiors.wgsl`: md5 a7876199c71dcc9a5210dee3fad7e77b;
  - `MGG SSAO.wgsl`: md5 0f90eb91deabf2e703db4a32c4f1d4f2.

  The mod's `Bloom Fine.fx` and `Depth of Field.fx` are stock shaders: MGE XE's own files port
  them.
- `Outlander Configuration - wgpu shaders/shaders/XEshaders/`, 3 files, from
  `mods/Outlander Configuration` in Outlander and Outlander-test and the game folder of Outlander:
  - `Cinematic Bokeh DoF.wgsl`: md5 80f933316ba8a644933ee612b0bc0211;
  - `Lens Effects.wgsl`: md5 3f91ae394d1c154ce1fc9f75a86c7cab, also in our
    `mods/Shader port tests` in Morrowind75testing;
  - `Special Process.wgsl`: md5 4686414c63cd611dbf44ebd310a65e20, also in our
    `mods/Shader port tests` in Morrowind75testing.
- `Kogoruhn Expanded - wgpu shaders/shaders/XEshaders/Disorient.wgsl`: `Disorient.fx`, md5
  ec6e7c421b12365fd1f157fd5b076aa6, of `mods/Kogoruhn Expanded` in Outlander and
  Outlander-test and of our `mods/Shader port tests` in Morrowind75testing.
- `Inspect It - wgpu shaders/shaders/XEshaders/InspectIt/Bokeh.wgsl`: `InspectIt/Bokeh.fx`, md5
  706d003ffb27afbcd126826c0b34724b, of `mods/Inspect It` in Outlander and Outlander-test and of
  our `mods/Shader port tests` in Morrowind75testing.
- `Modern Lockpicking (MWSE) - wgpu shaders/shaders/XEshaders/modern-lockpicking/Bokeh.wgsl`:
  `modern-lockpicking/Bokeh.fx` of `mods/Modern Lockpicking (MWSE)` in Morrowind Refreshed
  (260602). It is the file of Inspect It (the same md5), loaded by another name, so the pack file
  is the same as Inspect It's. The renderer has no built-in port for this name, so without this
  pack the shader does not run on wgpu.
