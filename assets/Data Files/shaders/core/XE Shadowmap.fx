
// XE Shadowmap.fx
// MGE XE 0.16.0
// Shadow map rendering, depth only into the shadow atlas

#include "XE Common.fx"
#include "XE Mod Shadow Data.fx"



//------------------------------------------------------------
// Shadow caster rendering

struct ShadowVertOut {
    float4 pos : POSITION;
};

// Distant terrain (TerrainDecl, no texcoords so it cannot alpha test). The mesh is a coarse
// twin of Morrowind's terrain and pokes through roads and floors built on it, which the
// depth compare would read as shadow; sink it so only its silhouette casts.
ShadowVertOut ShadowLandVS(float4 pos : POSITION) {
    ShadowVertOut OUT;

    OUT.pos = mul(pos, world);
    OUT.pos.z -= shadowTerrainSink;
    OUT.pos = mul(OUT.pos, shadowViewProj[0]);
    OUT.pos.z = max(0, OUT.pos.z);
    return OUT;
}

// Colour writes are disabled on every caster pass, depth is the output
float4 ShadowPS(ShadowVertOut IN) : COLOR0 {
    return 0;
}

struct StaticShadowVertOut {
    float4 pos : POSITION;
    float2 texcoords : TEXCOORD0;
    float4 uvBounds : TEXCOORD1;
};

StaticShadowVertOut StaticShadowVS(StatVertIn IN) {
    StaticShadowVertOut OUT;

    OUT.pos = mul(IN.pos, world);
    OUT.pos = mul(OUT.pos, shadowViewProj[0]);

    // Clamp vertices to front plane to avoid clipping and shadow loss
    OUT.pos.z = max(0, OUT.pos.z);

    OUT.texcoords = IN.texcoords;
    OUT.uvBounds = IN.uvBounds;
    return OUT;
}

float4 StaticShadowPS(StaticShadowVertOut IN) : COLOR0 {
    // Sample the static's assigned atlas region if alpha testing is required
    if(hasAlpha) {
        float2 scale = IN.uvBounds.yw - IN.uvBounds.zx;
        float2 dx = ddx(IN.texcoords) * scale;
        float2 dy = ddy(IN.texcoords) * scale;
        float2 atlasUV = IN.uvBounds.zx + frac(IN.texcoords) * scale;
        float a = tex2Dgrad(sampBaseTex, atlasUV, dx, dy).a;
        clip(a - 180.0/255.0);
    }

    return 0;
}

//-----------------------------------------------------------------------------

technique T0 {
    //------------------------------------------------------------
    // Used to render distant land into the shadow map
    Pass P0 {
        ZEnable = true;
        ZWriteEnable = true;
        ZFunc = LessEqual;
        ColorWriteEnable = 0;
        CullMode = CW;
        StencilEnable = false;

        // Push casters back in proportion to their depth slope, which is what
        // grazing sun angles need; the receiver adds a small constant bias
        DepthBias = 0;
        SlopeScaleDepthBias = 2.0;

        VertexShader = compile vs_3_0 ShadowLandVS();
        PixelShader = compile ps_3_0 ShadowPS();
    }
    //------------------------------------------------------------
    // Used to render distant statics into the shadow map
    Pass P1 {
        ZEnable = true;
        ZWriteEnable = true;
        ZFunc = LessEqual;
        ColorWriteEnable = 0;
        CullMode = CW;
        StencilEnable = false;

        // Push casters back in proportion to their depth slope, which is what
        // grazing sun angles need; the receiver adds a small constant bias
        DepthBias = 0;
        SlopeScaleDepthBias = 2.0;

        VertexShader = compile vs_3_0 StaticShadowVS();
        PixelShader = compile ps_3_0 StaticShadowPS();
    }
    //------------------------------------------------------------
}
