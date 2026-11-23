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
| Eternal Shine - wgpu shaders | Eternal Shine |
