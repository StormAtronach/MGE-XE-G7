
// XE Mod Water Volume.fx
// Surfaces of water volumes: meshes the game submits, shaded like the water plane.
// Uses the vertex shader, samplers and helpers of XE Mod Water.fx.

// A volume can sit at any height and can slope, so the planar reflection of the main water
// does not apply to it. The sky is reflected from its analytic colour, and what is on screen
// is reflected by marching the reflected ray through the depth frame.

// Steps of the reflection march. Each is 1.25 times as long as the one before, starting at
// 12 units, so 24 steps reach about 10000 units.
static const int volumeReflectionSteps = 24;

// Halvings of the step the ray went behind something in, to find the place where it did.
static const int volumeReflectionRefinements = 5;

// How far along a ray it leaves the screen. The ray is given in clip space.
float reflectionReach(float4 origin, float4 dir)
{
    // The ray is on screen while each of these is positive
    float4 room = float4(origin.w - origin.x, origin.w + origin.x, origin.w - origin.y, origin.w + origin.y);
    float4 closing = float4(dir.x - dir.w, -dir.x - dir.w, dir.y - dir.w, -dir.y - dir.w);
    float4 reach = closing > 0 ? room / closing : 1e9;
    return min(min(reach.x, reach.y), min(reach.z, reach.w));
}

// The ray at one distance along it: where it is on screen, and in z how far it is behind what
// the depth frame has there. The depth frame holds view depth; the sky is cleared to a huge
// value, so the ray is never behind it.
float3 reflectionSample(float4 origin, float4 dir, float travelled)
{
    float4 clip = origin + dir * travelled;
    float2 uv = 0.5 * (1 + rcpRes) + float2(0.5, -0.5) * clip.xy / clip.w;
    return float3(uv, clip.w - tex2Dlod(sampDepth, float4(uv, 0, 0)).r);
}

// Looks for the first thing on screen that the ray from origin along dir passes behind.
// Returns its colour from the frame behind the water, and in alpha how much to trust it.
float4 reflectScene(float3 origin, float3 dir)
{
    float4 clipOrigin = mul(mul(float4(origin, 1), view), proj);
    float4 clipDir = mul(mul(float4(dir, 0), view), proj);

    // The march stops where the ray leaves the screen
    float reach = 0.999 * reflectionReach(clipOrigin, clipDir);

    float stepLength = 12;
    float before = 0;
    float after = 0;
    bool behind = false;

    for(int i = 0; i < volumeReflectionSteps; i++)
    {
        after = min(before + stepLength, reach);
        if(reflectionSample(clipOrigin, clipDir, after).z > 0)
        {
            behind = true;
            break;
        }
        if(after >= reach)
            break;

        before = after;
        stepLength *= 1.25;
    }

    float4 result = 0;
    if(behind)
    {
        // The ray went behind something between before and after. Find where, so that the
        // reflection does not jump from one step to the next.
        for(int j = 0; j < volumeReflectionRefinements; j++)
        {
            float middle = 0.5 * (before + after);
            if(reflectionSample(clipOrigin, clipDir, middle).z > 0)
                after = middle;
            else
                before = middle;
        }

        // Only a surface near the ray counts; otherwise the ray went behind something
        float3 hit = reflectionSample(clipOrigin, clipDir, after);
        if(hit.z < 2 * stepLength + 8)
        {
            // Fade out towards the screen edge, where the ray would leave the frame
            float2 edge = min(hit.xy, 1 - hit.xy);
            result.rgb = tex2Dlod(sampRefract, float4(hit.xy, 0, 0)).rgb;
            result.a = saturate(12 * min(edge.x, edge.y));
        }
    }

    return result;
}

// reflectsScene and distant are fixed per pass. A surface either reflects what is on screen or
// the sky only. A distant surface reflects what is on screen out to waterVolumeReflectRange.
// tint is the colour of the water of this surface: the emissive colour of its material.
float4 waterVolumeColour(in WaterVertOut IN, bool reflectsScene, bool distant, float3 tint)
{
    // Calculate eye vector
    float3 EyeVec = IN.pos.xyz - eyePos.xyz;
    float dist = length(EyeVec);
    EyeVec /= dist;

    // Define fog
    float4 fog = fogColourWater(EyeVec, dist);
    float3 depthColor = fogApply(waterDepthBase(depthBaseColor, tint), fog);

    // Calculate water normal
    float3 normal = getFinalWaterNormal(IN.texcoords.xy, IN.texcoords.zw, dist, IN.pos.xy);

    // Refraction pixel distortion factor, wind strength increases distortion
    float2 reffactor = (windFactor * dist + 0.1) * normal.xy;

    // Distort refraction dependent on depth
    float4 newscrpos = IN.screenpos + float4(reffactor.yx, 0, 0);
    float depth = max(0, tex2Dproj(sampDepth, newscrpos).r - IN.screenpos.w);

    // Refraction
    float3 refracted = depthColor;
    float shorefactor = 0;

    // Avoid sampling deep water
    if(depth < 4000)
    {
        // Sample refraction texture
        newscrpos = IN.screenpos + saturate(depth / 100) * float4(reffactor.yx, 0, 0);
        refracted = tex2Dproj(sampRefract, newscrpos).rgb;

        // Get distorted depth
        depth = max(0, tex2Dproj(sampDepth, newscrpos).r - IN.screenpos.w);
        depth /= dot(EyeVec, float3(view[0][2], view[1][2], view[2][2]));
        refracted *= waterTransmission(tint, depth);

        // Small scale shoreline animation
        depth += 300 * (0.95 - normal.z);

        float depthscale = saturate(exp(-depth / 800));
        shorefactor = pow(depthscale, 90);

        // Make transition between actual refraction image and depth color depending on water depth
        refracted = lerp(depthColor, refracted, 0.8 * depthscale + 0.2 * shorefactor);
    }

    // Smooth out high frequencies at a distance
    float3 adjustnormal = lerp(float3(0, 0, 0.1), normal, pow(saturate(1.05 * fog.a), 2));
    adjustnormal = lerp(adjustnormal, float3(0, 0, 1.0), (1 + EyeVec.z) * (1 - saturate(1 / (dist / 1000 + 1))));

    // Reflect the sky. The ripples tilt the reflected direction only a little, to keep it calm.
    float3 reflectdir = reflect(EyeVec, normalize(lerp(float3(0, 0, 1), normal, 0.35)));
    reflectdir.z = abs(reflectdir.z);
    reflectdir = normalize(reflectdir);
    float3 reflected = fogColourSky(reflectdir).rgb;

    // Reflect what is on screen over the sky
    if(reflectsScene)
    {
        // Far away the march is left out; it fades out over the last quarter of a cell.
        float sceneWeight = distant ? saturate((waterVolumeReflectRange - dist) / 2048) : 1;
        if(sceneWeight > 0)
        {
            float4 scene = reflectScene(IN.pos.xyz, reflectdir);
            reflected = lerp(reflected, scene.rgb, scene.a * sceneWeight);
        }
    }

    // Fade reflection into an inscatter dominated horizon
    reflected = lerp(fog.rgb, reflected, fog.a);

    // Fresnel equation determines reflection/refraction
    float fresnel = dot(-EyeVec, adjustnormal);
    fresnel = 0.02 + pow(saturate(0.9988 - 0.28 * fresnel), 16);
    float3 result = lerp(refracted, reflected, fresnel);

    // Specular lighting, as for the water plane
    float vdotr = dot(-EyeVec, reflect(-sunPos, normal));
    vdotr = saturate(1.0025 * vdotr);
    float3 spec = sunColAdjusted * (pow(vdotr, 170) + 0.07 * pow(vdotr, 4));
    result += spec * fog.a;

    // Smooth transition at shore line
    result = lerp(result, refracted, shorefactor * fog.a);

    return float4(result, 1);
}

float4 WaterVolumePS(in WaterVertOut IN, uniform bool reflectsScene): COLOR0
{
    return waterVolumeColour(IN, reflectsScene, false, waterVolumeTint);
}

//------------------------------------------------------------
// The same water among the distant statics, farther away than the game draws.

struct WaterVolumeDistantVertOut
{
    WaterVertOut water;
    float3 tint : TEXCOORD4;
};

// A distant static keeps a palette index in pos.w. The generator writes the colour of the
// water into the vertex colour of a water subset.
WaterVolumeDistantVertOut WaterVolumeDistantVS(in StatVertIn IN)
{
    WaterVolumeDistantVertOut OUT;
    OUT.water = WaterVS(float4(IN.pos.xyz, 1));
    OUT.tint = IN.color.rgb;
    return OUT;
}

float4 WaterVolumeDistantPS(in WaterVolumeDistantVertOut IN, uniform bool reflectsScene): COLOR0
{
    // Nearer than this the game draws the surface itself
    clip(IN.water.screenpos.w - nearViewRange);
    return waterVolumeColour(IN.water, reflectsScene, true, IN.tint);
}
