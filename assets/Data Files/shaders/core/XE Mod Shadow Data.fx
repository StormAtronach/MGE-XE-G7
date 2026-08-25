
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

// Luminance reduction parameter for shadowed areas; higher is lighter. Percentage-closer
// shadows reach full strength close to the caster where the old exponential map faded,
// so this sits above the ESM-era 0.4.
static const float shade = 0.6;

// Shade colouration, how much each channel is affected by shadow. A strong blue cast on a
// fully shadowed slope reads as fog, so this is kept close to neutral.
static const float3 shadecolor = float3(1.0, 0.985, 0.93);

// Constant depth comparison bias in atlas depth units; the light range spans 16384 world
// units, so 1.5e-3 is about 24 units. Casters are distant-land copies of the receivers and
// sit a few units off them, so this has to exceed that LOD error or every static shadows
// itself. Slope-dependent bias is on the caster side (SlopeScaleDepthBias in XE Shadowmap.fx).
static const float shadowBias = 1.5e-3;

// Receiver offset along its normal, in shadow texels of the sampled cascade, scaled from
// 0.5x facing the sun to 1.5x at grazing angles
static const float shadowNormalOffset = 1.5;

// Filter tap spread in texels; each tap is a hardware 2x2 bilinear compare
static const float shadowFilterRadius = 1.0;

// World units the distant terrain caster is lowered by, so its coarse mesh stays below
// roads and floors built on it instead of shadowing them. Costs that much shadow at the
// foot of every slope.
static const float shadowTerrainSink = 24.0;
