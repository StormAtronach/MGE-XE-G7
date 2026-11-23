Ashfall - wgpu shaders

The wgpu renderer of MGE XE G7 reads shaders/XEshaders/ashfall.wgsl of this mod.
The file is the port of ashfall.fx of the mod Ashfall.
Put this mod after Ashfall in the mod list. The Direct3D 9 renderer does not read it.

The wgpu renderer reads ashfall.wgsl before ashfall.fx. A number that you change in the .fx
does not change the wgpu picture: change it in the .wgsl. The lines that end with
"// file" hold the numbers of the .fx.

The export of render-link (mgeHost64/src/renderer/post_export.rs, the test export_packs)
made the file from these copies of ashfall.fx. All of them give the same port:
  Morrowind Refreshed, md5 a4719efd5ca35fd57fc90a379ab9926e:
    mods/Ashfall - A Camping Survival and Needs Mod/shaders/XEshaders/ashfall.fx
  Morrowind Refreshed (260602), md5 318790efad83358df2940cfa5d287469:
    mods/Ashfall - A Camping Survival and Needs Mod/shaders/XEshaders/ashfall.fx
  Morrowind100, md5 a4719efd5ca35fd57fc90a379ab9926e:
    mods/Ashfall - A Camping Survival and Needs Mod/shaders/XEshaders/ashfall.fx
  Morrowind75 - Poison Song, md5 a4719efd5ca35fd57fc90a379ab9926e:
    mods/Ashfall - A Camping Survival and Needs Mod/shaders/XEshaders/ashfall.fx
  Morrowind75, md5 a4719efd5ca35fd57fc90a379ab9926e:
    mods/Ashfall - A Camping Survival and Needs Mod/shaders/XEshaders/ashfall.fx
  Morrowind75testing, md5 a4719efd5ca35fd57fc90a379ab9926e:
    mods/Ashfall - A Camping Survival and Needs Mod/shaders/XEshaders/ashfall.fx
  Outlander, md5 a4719efd5ca35fd57fc90a379ab9926e:
    mods/Ashfall - A Camping Survival and Needs Mod/shaders/XEshaders/ashfall.fx
  Outlander-test, md5 a4719efd5ca35fd57fc90a379ab9926e:
    mods/Ashfall - A Camping Survival and Needs Mod/shaders/XEshaders/ashfall.fx
