Watch the Skies - wgpu shaders

The wgpu renderer of MGE XE G7 reads shaders/XEshaders/Sunshafts.wgsl of this mod.
The file is the port of Sunshafts.fx of the mod Watch the Skies.
Put this mod after Watch the Skies in the mod list. The Direct3D 9 renderer does not read it.

The wgpu renderer reads Sunshafts.wgsl before Sunshafts.fx. A number that you change in the .fx
does not change the wgpu picture: change it in the .wgsl. The lines that end with
"// file" hold the numbers of the .fx.

The export of render-link (mgeHost64/src/renderer/post_export.rs, the test export_packs)
made the file from these copies of Sunshafts.fx. All of them give the same port:
  Morrowind Refreshed (260602), md5 e9784b6a494fba25f6c4476b8328a24d:
    mods/Watch the Skies - Dynamic Weather and Sky Effects/shaders/XEshaders/Sunshafts.fx
  Outlander, md5 e9784b6a494fba25f6c4476b8328a24d:
    mods/Watch the Skies/shaders/XEshaders/Sunshafts.fx
  Outlander-test, md5 e9784b6a494fba25f6c4476b8328a24d:
    mods/Watch the Skies/shaders/XEshaders/Sunshafts.fx
