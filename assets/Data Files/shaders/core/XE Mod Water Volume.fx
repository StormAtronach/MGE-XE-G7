
// XE Mod Water Volume.fx
// Surfaces of water volumes: meshes the game submits, shaded like the water plane.
// Uses the vertex shader, samplers and helpers of XE Mod Water.fx.

// A volume can sit at any height and its surface can face any way, so the planar reflection
// of the main water does not apply to it. The sky is reflected from its analytic colour, and
// what is on screen is reflected by marching the reflected ray through the depth frame.
// The shading follows the normal of the mesh: the ripples are tilted to it, and the reflection
// and the Fresnel term are taken from it.

// A mod can replace the pixel shader of a surface with one of its own: see WaterShade below
// and docs/water-shaders.md.

// The base texture of the mesh, for the water shader of a mod
sampler sampMesh0 = sampler_state { texture = <texMesh0>; minfilter = linear; magfilter = linear; mipfilter = linear; addressu = wrap; addressv = wrap; };

// The look of a surface beyond its colour, from waterVolumeFlow, waterVolumeMix and the vertex.
struct WaterSurfaceLook
{
    // Drift of the ripples in world units per second, speed and size of the ripples
    float2 drift;
    float speed;
    float scale;
    float glow;
    // 1 is water; lower shows what is behind the surface
    float opacity;
    // The vertex colour, or 1
    float3 tint;
    // The surface reflects what is on screen; otherwise the sky only
    bool reflectsScene;
    // The surface is drawn from the distant land, farther away than the game draws
    bool distant;
    // Free values p0 to p3 of the look, for the water shader of a mod
    float4 p0, p1, p2, p3;
};

// The look of the surface that is drawn. The drift is given in the axes of the mesh and
// turned into the world.
WaterSurfaceLook surfaceLook(float4 vertexColour)
{
    WaterSurfaceLook look;
    look.drift = mul(float4(waterVolumeFlow.xy, 0, 0), world).xy;
    look.speed = waterVolumeFlow.z;
    look.scale = waterVolumeFlow.w;
    look.glow = waterVolumeMix.x;
    float tintFromVertex = fmod(waterVolumeMix.z, 2);
    float opacityFromVertex = floor(waterVolumeMix.z / 2);
    look.opacity = waterVolumeMix.y * lerp(1, vertexColour.a, opacityFromVertex);
    look.tint = lerp(1, vertexColour.rgb, tintFromVertex);
    look.reflectsScene = fmod(waterVolumeMix.w, 2) > 0.5;
    look.distant = waterVolumeMix.w > 1.5;
    look.p0 = waterVolumeParams[0];
    look.p1 = waterVolumeParams[1];
    look.p2 = waterVolumeParams[2];
    look.p3 = waterVolumeParams[3];
    return look;
}

// The ripples of the water plane, with a drift, a speed and a size of their own. The drift
// moves the ripple coordinates; it is wrapped so that it keeps its precision over hours.
float3 surfaceRipples(float2 texcoord1, float2 texcoord2, float dist, float2 vertXY, WaterSurfaceLook look)
{
    float t = 0.4 * time * look.speed;
    float2 moved = look.drift * time;
    float3 w1 = float3(texcoord1 / look.scale + frac(moved / 3900), t);
    float3 w2 = float3(texcoord2 / look.scale + frac(moved / 527), t);

    float2 far_normal = tex3D(sampWater3d, w1).rg;
    float2 close_normal = tex3D(sampWater3d, w2).rg;

#ifdef DYNAMIC_RIPPLES
    close_normal.rg += tex2Dlod(sampRain, float4(texcoord2, 0, 0)).ba;
    close_normal.rg += tex2Dlod(sampWave, float4((vertXY - rippleOrigin) / waveTexWorldSize, 0, 0)).ba;
#endif

    float2 normal_R = 2 * lerp(close_normal, far_normal, saturate(dist / 8000)) - 1;
    return normalize(float3(normal_R, 1));
}

// Turns a direction by the turn that takes straight up to the given direction.
float3 tiltTo(float3 up, float3 v)
{
    float3 axis = float3(-up.y, up.x, 0);
    return v * up.z + cross(axis, v) + axis * (dot(axis, v) / max(1 + up.z, 1e-3));
}

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

        // Only a surface near the ray counts; otherwise the ray went behind something that
        // is nearer to the eye. The halvings leave a thirty-second of the step, so a surface
        // that the ray meets is no farther in front of it than that. A wider limit takes a
        // thing in the foreground for a hit at some pixels and not at others.
        float3 hit = reflectionSample(clipOrigin, clipDir, after);
        if(hit.z < stepLength / 16 + 16)
        {
            // Fade out towards the screen edge, where the ray would leave the frame
            float2 edge = min(hit.xy, 1 - hit.xy);
            result.rgb = tex2Dlod(sampRefract, float4(hit.xy, 0, 0)).rgb;
            result.a = saturate(12 * min(edge.x, edge.y));
        }
    }

    return result;
}

// The shading of a surface point before the look finishes it. The water shader of a mod
// gets it from shadeWaterVolume, changes the colour, and gives it to finishWaterVolume.
struct WaterShade
{
    // The colour of the water at this point
    float3 colour;
    // From the eye to the point, and how far that is
    float3 eyeVec;
    float dist;
    // Fog at the point: rgb what the fog adds, a how much of the surface is left
    float4 fog;
    // The ripple normal as on a level surface, and the way the surface faces
    float3 ripple;
    float3 face;
    // How far the view ray goes through the water to what is behind the surface, and how
    // deep that is under the surface. Both are 4000 or more where the water is deep.
    float rayDepth;
    float waterDepth;
};

// A surface either reflects what is on screen or the sky only; that is a value of the look.
// A distant surface reflects what is on screen out to waterVolumeReflectRange.
// A range of zero turns the reflection of what is on screen off, near and far.
// tint is the colour of the water of this surface: the emissive colour of its material.
// facing is the normal of the mesh. A mesh without normals faces up.
// look is the rest of what the mesh and its mod say about the surface.
WaterShade shadeWaterVolume(in WaterVertOut IN, float3 facing, float3 tint, WaterSurfaceLook look)
{
    // A distant surface is left out nearer than the handoff depth, where the game draws the
    // surface itself. The depth is zero for a surface near the player, and for a distant mesh
    // whose reference the game has not loaded.
    clip(IN.screenpos.w - waterVolumeHandoff);

    bool reflectsScene = look.reflectsScene;
    bool distant = look.distant;

    // Calculate eye vector
    float3 EyeVec = IN.pos.xyz - eyePos.xyz;
    float dist = length(EyeVec);
    EyeVec /= dist;

    // Define fog
    float4 fog = fogColourWater(EyeVec, dist);
    float3 depthColor = fogApply(waterDepthBase(depthBaseColor, tint) * look.tint, fog);

    // The way the surface faces, turned to the side of the eye
    float3 face = dot(facing, facing) > 0.25 ? normalize(facing) : float3(0, 0, 1);
    face = dot(face, EyeVec) > 0 ? -face : face;

    // Calculate water normal: the ripples of a level surface, tilted to the surface
    float3 ripple = surfaceRipples(IN.texcoords.xy, IN.texcoords.zw, dist, IN.pos.xy, look);
    float3 normal = tiltTo(face, ripple);

    // Refraction pixel distortion factor, wind strength increases distortion
    float2 reffactor = (windFactor * dist + 0.1) * ripple.xy;

    // Distort refraction dependent on depth
    float4 newscrpos = IN.screenpos + float4(reffactor.yx, 0, 0);
    float depth = max(0, tex2Dproj(sampDepth, newscrpos).r - IN.screenpos.w);

    // Refraction
    float3 refracted = depthColor;
    float shorefactor = 0;
    float rayDepth = depth;

    // Avoid sampling deep water
    if(depth < 4000)
    {
        // Sample refraction texture
        newscrpos = IN.screenpos + saturate(depth / 100) * float4(reffactor.yx, 0, 0);
        refracted = tex2Dproj(sampRefract, newscrpos).rgb;

        // Get distorted depth
        depth = max(0, tex2Dproj(sampDepth, newscrpos).r - IN.screenpos.w);
        depth /= dot(EyeVec, float3(view[0][2], view[1][2], view[2][2]));
        refracted *= waterTransmission(tint, depth) * look.tint;
        rayDepth = depth;

        // Small scale shoreline animation
        depth += 300 * (0.95 - ripple.z);

        float depthscale = saturate(exp(-depth / 800));
        shorefactor = pow(depthscale, 90);

        // Make transition between actual refraction image and depth color depending on water depth
        refracted = lerp(depthColor, refracted, 0.8 * depthscale + 0.2 * shorefactor);
    }

    // Smooth out high frequencies at a distance
    float calm = (1 + dot(EyeVec, face)) * (1 - saturate(1 / (dist / 1000 + 1)));
    float3 adjustnormal = lerp(0.1 * face, normal, pow(saturate(1.05 * fog.a), 2));
    adjustnormal = lerp(adjustnormal, face, calm);

    // Reflect the sky. The ripples tilt the reflected direction only a little, to keep it calm,
    // and less far away and at a flat angle, where a ripple is smaller than a pixel and the
    // reflection would break up into dots.
    // A reflected direction that points into the surface is turned back out of it.
    float3 reflectdir = reflect(EyeVec, normalize(lerp(face, normal, 0.35 * (1 - calm))));
    reflectdir -= 2 * min(0, dot(reflectdir, face)) * face;
    reflectdir = normalize(reflectdir);
    float3 reflected = fogColourSky(reflectdir).rgb;

    // Reflect what is on screen over the sky
    if(reflectsScene)
    {
        // Far away the march is left out; it fades out over the last quarter of a cell.
        float sceneWeight = distant ? saturate((waterVolumeReflectRange - dist) / 2048) : step(1, waterVolumeReflectRange);
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

    WaterShade shade;
    shade.colour = result;
    shade.eyeVec = EyeVec;
    shade.dist = dist;
    shade.fog = fog;
    shade.ripple = ripple;
    shade.face = face;
    shade.rayDepth = rayDepth;
    shade.waterDepth = rayDepth * abs(dot(EyeVec, face));
    return shade;
}

// The last steps of the shading, which belong to the look: glow and opacity.
float4 finishWaterVolume(in WaterVertOut IN, WaterShade shade, float3 tint, WaterSurfaceLook look)
{
    float3 result = shade.colour;

    // Light of the surface's own, in the colour of the water. It does not follow the daylight,
    // so a surface of the usual colour glows in a water blue of its own.
    float3 glowColour = dot(tint, 1) > 0 ? tint : float3(0.25, 0.5, 0.6);
    result += look.glow * glowColour * shade.fog.a;

    // A surface that is not fully water shows what is behind it
    float3 behind = tex2Dproj(sampRefract, IN.screenpos).rgb;
    result = lerp(behind, result, look.opacity);

    return float4(result, 1);
}

// A vertex of a surface, with the normal of the mesh in world space.
struct WaterVolumeVertOut
{
    WaterVertOut water;
    // The colour of the water: the emissive colour of the material
    float3 tint : TEXCOORD4;
    float3 facing : TEXCOORD5;
    // The vertex colour of the mesh; 1 for a surface from the distant land
    float4 color : COLOR0;
    // The first texture coordinates of the mesh
    float2 uv : TEXCOORD6;
};

WaterVolumeVertOut WaterVolumeVS(in float4 pos : POSITION, in float3 normal : NORMAL, in float4 color : COLOR0, in float2 uv : TEXCOORD0)
{
    WaterVolumeVertOut OUT;
    OUT.water = WaterVS(pos);
    OUT.tint = waterVolumeTint;
    OUT.facing = mul(float4(normal, 0), world).xyz;
    OUT.color = color;
    OUT.uv = uv;
    return OUT;
}

// The pixel shader of a surface near the player is a water shader: the standard one in
// XE Mod Water Standard.fx, or the one of a mod.

//------------------------------------------------------------
// The same water among the distant statics, farther away than the game draws.

// It gives the pixel shader the same vertex as a surface near the player, so one water
// shader draws both.
// A distant static keeps a palette index in pos.w. The generator writes the colour of the
// water into the vertex colour of a water subset.
WaterVolumeVertOut WaterVolumeDistantVS(in StatVertIn IN)
{
    WaterVolumeVertOut OUT;
    OUT.water = WaterVS(float4(IN.pos.xyz, 1));
    OUT.tint = IN.color.rgb;
    OUT.facing = mul(float4(2 * IN.normal.xyz - 1, 0), world).xyz;
    OUT.color = 1;
    OUT.uv = IN.texcoords;
    return OUT;
}
