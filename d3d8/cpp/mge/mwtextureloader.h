#pragma once

namespace MWTextureLoader {

// Records whether the active D3D9 runtime supports MGE XE's BC7 format.
// Must be called once device capabilities are known, before textures load.
// The wgpu renderer needs no call: it takes BC7 while it is in use.
void setBC7TextureSupport(bool supported);

// Changes texture creation from D3DPOOL_MANAGED to D3DPOOL_DEFAULT, by loading through a staging texture
// This should reduce process memory footprint by removing managed textures.
// It also makes the Direct3D 9 texture of a BC7 texture (patchDDSReader).
void patchLoadTexture2D();

// Extends NiDDSReader to decode BC7 (DX10 extended header) DDS textures: the reader's two
// vtable slots. For both renderers. A BC7 texture is accepted while the wgpu renderer is in
// use, or when patchLoadTexture2D is installed and the D3D9 runtime supports the format.
void patchDDSReader();

} // namespace MWTextureLoader
