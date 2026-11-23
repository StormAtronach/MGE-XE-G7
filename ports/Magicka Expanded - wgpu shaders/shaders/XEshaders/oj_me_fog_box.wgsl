//! interface: mge-xe-wgsl 1
//! passes: draw
//! category: scene
//! variables: fogCenters vec3[2], fogRadi vec3[2], fogColors vec3[2], fogDensities f32[2]

const NUM_FOG_VOLUMES: i32 = 2;

fn toWorld(tex: vec2f) -> vec3f {
    var v = vec3f(mview(0, 2), mview(1, 2), mview(2, 2));
    v += (1.0 / mproj(0, 0) * (2.0 * tex.x - 1.0)) * vec3f(mview(0, 0), mview(1, 0), mview(2, 0));
    v += (-1.0 / mproj(1, 1) * (2.0 * tex.y - 1.0)) * vec3f(mview(0, 1), mview(1, 1), mview(2, 1));
    return v;
}

// -------------------------------------------------------------- //
//
// https://www.shadertoy.com/view/Ml3GR8
//

fn boxDensity(wpos: vec3f, wdir: vec3f, p: vec3f, r: vec3f, dbuffer: f32) -> f32 {
    let d = wdir;
    var o = wpos - p;

    // ray-box intersection in box space
    let m = 1.0 / d;
    let n = m * o;
    let k = abs(m) * r;
    let ta = -n - k;
    let tb = -n + k;
    var tN = max(max(ta.x, ta.y), ta.z);
    var tF = min(min(tb.x, tb.y), tb.z);
    if tN > tF || tF < 0.0 {
        return 0.0;
    }

    // not visible (behind camera or behind dbuffer)
    if tF < 0.0 || tN > dbuffer {
        return 0.0;
    }

    // clip integration segment from camera to dbuffer
    tN = max(tN, 0.0);
    tF = min(tF, dbuffer);

    // move ray to the intersection point
    o += tN * d;
    tF = tF - tN;
    tN = 0.0;

    // density calculation. density is of the form
    //
    // d(x,y,z) = [1-(x/rx)^2] * [1-(y/ry)^2] * [1-(z/rz)^2];
    //
    // this can be analytically integrable (it's a degree 6 polynomial):

    let a = 1.0 - (o * o) / (r * r);
    let b = -2.0 * (o * d) / (r * r);
    let c = -(d * d) / (r * r);

    let t1 = tF;
    let t2 = t1 * t1;
    let t3 = t2 * t1;
    let t4 = t2 * t2;
    let t5 = t2 * t3;
    let t6 = t3 * t3;
    let t7 = t3 * t4;

    let f = (t1 / 1.0) * (a.x * a.y * a.z)
        + (t2 / 2.0) * (a.x * a.y * b.z + a.x * b.y * a.z + b.x * a.y * a.z)
        + (t3 / 3.0) * (a.x * a.y * c.z + a.x * b.y * b.z + a.x * c.y * a.z + b.x * a.y * b.z + b.x * b.y * a.z + c.x * a.y * a.z)
        + (t4 / 4.0) * (a.x * b.y * c.z + a.x * c.y * b.z + b.x * a.y * c.z + b.x * b.y * b.z + b.x * c.y * a.z + c.x * a.y * b.z + c.x * b.y * a.z)
        + (t5 / 5.0) * (a.x * c.y * c.z + b.x * b.y * c.z + b.x * c.y * b.z + c.x * a.y * c.z + c.x * b.y * b.z + c.x * c.y * a.z)
        + (t6 / 6.0) * (b.x * c.y * c.z + c.x * b.y * c.z + c.x * c.y * b.z)
        + (t7 / 7.0) * (c.x * c.y * c.z);

    return f;
}

// -------------------------------------------------------------- //

// Ordered dithering matrix
const DITHERING = array<f32, 16>(
    0.001176, 0.001961, -0.001176, -0.001699,
    -0.000654, -0.000915, 0.000392, 0.000131,
    -0.000131, -0.001961, 0.000654, 0.000915,
    0.001699, 0.001438, -0.000392, -0.001438,
);

@fragment fn draw(in: Quad) -> @location(0) vec4f {
    let tex = in.tex;
    var color = textureSampleLevel(lastshader, point_clamp, tex, 0.0).rgb;
    // The file reads the depth with a linear filter at the centre of a pixel: the value of
    // that pixel.
    let depth = textureSampleLevel(depthframe, point_clamp, tex, 0.0).r;

    let pos = u.eyepos.xyz;
    let dir = toWorld(tex);

    // gamma -> linear
    color = pow(color, vec3f(2.2));

    // draw fog volumes
    for (var i = 0; i < NUM_FOG_VOLUMES; i++) {
        let center = fogCenters(i);
        let radius = fogRadi(i);
        let density = boxDensity(pos, dir, center, radius, depth);
        if density > 0.0 {
            // apply lua config
            let fogScalar = 1.0 / sqrt(dot(radius, radius));
            let amount = density * fogScalar * fogDensities(i);

            // do the fog stuff
            let fogColor = fogColors(i);
            color = mix(fogColor * fogColor, color, exp(-0.5 * amount));
        }
    }

    // linear -> gamma
    color = pow(color, vec3f(1.0 / 2.2));

    // dithering!
    // VPOS of the file is the pixel: DITHERING[vpos.x % 4][vpos.y % 4].
    let dithering = DITHERING[(i32(in.position.x) % 4) * 4 + i32(in.position.y) % 4];
    color += dithering;

    return vec4f(color, 1.0);
}
