
// XE Mod Shadow Data.fx
// MGE XE 0.16.0
// Shared constants



//------------------------------------------------------------
// Engine parameters

// Number of cascades that are rendered. Do not change this in a core mod: it must match
// DistantLand::kShadowCascades, which sizes the atlas and the per-cascade matrices, and
// nothing checks that the two agree.
static const int shadowCascades = 2;

// Shadow atlas texture scaling factor
static const float shadowCascadeSize = 1. / shadowCascades;

// Cascade half-widths in world units. Must match shadowNearRadius and shadowFarRadius in
// rendershadow.cpp; only used to size the receiver normal offset in world units.
static const float shadowCascadeRadius[2] = { 1000.0, 4000.0 };

//------------------------------------------------------------
// Shadow parameters

// Luminance reduction parameter for shadowed areas, recommend values in [0.25, 0.5]
static const float shade = 0.4;

// Shade colouration, how much each channel is affected by shadow
static const float3 shadecolor = float3(1.0, 0.97, 0.81);

// Depth comparison bias in atlas depth units; the light range spans 16384 world units
static const float shadowBias = 2.5e-4;

// Receiver offset along its normal, in shadow texels of the sampled cascade
static const float shadowNormalOffset = 1.5;

// Filter tap spread in texels; each tap is a hardware 2x2 bilinear compare
static const float shadowFilterRadius = 1.0;
