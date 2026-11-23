This directory contains the core shader files required for MGE XE rendering.

Core shader replacements can break on MGE XE upgrades.

Do not replace them! Use the core-mods directory.

The wgpu renderer (numidium.exe) uses the .wgsl files here. The first line of each
names the build that it belongs to ("//! core: <build id>"). A file of another build,
or a file that is not here, gives the copy that is built into numidium.exe. An edited
file of this build is used only when every shader that it is part of passes the check;
the log of the helper says which. The next release overwrites the files, and
MGE_XE_CORE_FILES=0 uses the built-in copies only.