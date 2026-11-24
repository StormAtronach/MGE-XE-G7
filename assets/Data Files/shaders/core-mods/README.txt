This folder holds core mods: files that change parts of the core shaders of the MGE XE renderer (wgpu).

A core mod is a WGSL file with a name such as "XE Mod Shadow.wgsl", "XE Mod Water.wgsl", "XE Mod Sky.wgsl" or "XE Mod Particles.wgsl". Its first line is:

//! interface: mge-xe-core 1

The file holds only hooks: the functions that the renderer names for that file, with their exact signatures. It can also hold helpers, constants and structs of its own whose names start with "mod_". It cannot hold a binding, an entry point or a variable outside a function. The renderer keeps all other parts of its shaders. "XE Mod Shadow Data.wgsl" holds no code, only a line of values ("//! values: shade = ..., esm_c = ..."). The list of the hooks, their signatures and the functions that each hook can call are in mgeHost64/src/renderer/core_mods.rs of the MGE XE source.

The renderer reads these files one time, when the game starts. If a file breaks a rule, or the graphics device does not accept it, the renderer drops that file and uses its own built-in hooks for it. mgeXE.log names the file and the reason. "XE Mod Shadow" and "XE Mod Shadow Data" are dropped together.

The shadow map has four cascades. Its hook "shadow_esm" works as before. The renderer does not call the hook "shadow_delta" of the two old layers: an "XE Mod Shadow.wgsl" that defines "shadow_delta" is dropped with its "XE Mod Shadow Data.wgsl", and mgeXE.log gives the reason. A later interface (mge-xe-core 2) gives the new form of that hook. An "XE Mod Shadow.wgsl" with only "shadow_esm" loads.

The old Direct3D 9 files do not work here. Do not copy "XE Mod" .fx files from shaders/core into this folder: the renderer does not run an .fx file. mgeXE.log names each such file that has no .wgsl of the same name.

For post shaders (the effects in shaders/XEshaders), see "MGE XE Post Shaders Readme.md" in the Morrowind folder.