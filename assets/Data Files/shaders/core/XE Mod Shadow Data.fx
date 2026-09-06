
// XE Mod Shadow Data.fx
// MGE XE 0.16.0
// Shared constants



//------------------------------------------------------------
// Engine parameters

// Number of cascades that are rendered. Do not change this in a core mod: it must match
// DistantLand::kShadowCascades, which sizes the atlas and the per-cascade matrices, and
// nothing checks that the two agree.
static const int shadowCascades = 4;

// Shadow atlas texture scaling factor
static const float shadowCascadeSize = 1. / shadowCascades;

//------------------------------------------------------------
// Shadow parameters

// Luminance reduction parameter for shadowed areas; higher is lighter. Percentage-closer
// shadows reach full strength close to the caster where the old exponential map faded,
// so this sits above the ESM-era 0.4.
static const float shade = 0.6;

// Shade colouration, how much each channel is affected by shadow. A strong blue cast on a
// fully shadowed slope reads as fog, so this is kept close to neutral.
static const float3 shadecolor = float3(1.0, 0.985, 0.93);

// Constant depth comparison bias in world units. Casters are distant-land copies of the
// receivers and sit a few units off them, so this has to exceed that LOD error or every
// static shadows itself. Slope-dependent bias is on the caster side (SlopeScaleDepthBias
// in XE Shadowmap.fx).
static const float shadowBias = 24.0;

// Extra comparison bias of one texel of the sampled cascade, in world units per unit of
// texel size, so the coarse far cascades get the tolerance their rasterisation error needs
static const float shadowBiasTexels = 1.0;

// Receiver offset along its normal, in shadow texels of the sampled cascade, scaled from
// 0.5x facing the sun to 1.5x at grazing angles
static const float shadowNormalOffset = 1.5;

// Cap on that offset in world units, so the coarse far cascades do not push receivers a
// quarter of a cell toward the sun
static const float shadowNormalOffsetMax = 16.0;

// Spacing of the 3x3 filter taps in cascade 0 texels; each tap is a hardware 2x2 bilinear
// compare, so the penumbra is about 3 * radius + 1 texels wide (cascade 0 texels are ~1 world
// unit). Other cascades use the same spacing in world units, so the penumbra width does not
// jump at a cascade boundary
static const float shadowFilterRadius = 2.0;

// First cascade sampled with one compare instead of the 3x3 grid: from here the grid falls
// inside a quarter texel
static const int shadowSingleTapCascade = 2;

// World units the distant terrain caster is lowered by, so its coarse mesh stays below
// roads and floors built on it instead of shadowing them. Costs that much shadow at the
// foot of every slope.
static const float shadowTerrainSink = 24.0;

// Sun elevation band in degrees over which shadows fade out toward the horizon. The top
// must match the fit clamp in rendershadow.cpp (shadowMinElevation): below it the constant
// bias detaches shadows from their casters and ground texels stretch along the light
static const float2 shadowElevationFade = float2(5.0, 10.0);
