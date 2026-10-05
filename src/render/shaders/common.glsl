// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

// What every shader shares: the frame's camera and light, and the way a colour is written
// out (three.js's Neutral tone mapping, then sRGB), so the model looks as it did there.

layout(std140) uniform Frame {
    mat4 uView;
    mat4 uProjection;
    // World to the key light's shadow map, in clip space
    mat4 uLightMatrix;
    vec4 uCamera;
    // Towards the light
    vec4 uLightDirection;
    // Linear RGB, intensity included
    vec4 uLightColor;
    // x: environment intensity, y: the environment's last mip level, z: point size in pixels
    vec4 uParams;
};

const float PI = 3.141592653589793;
const float RECIPROCAL_PI = 0.3183098861837907;

vec3 neutralToneMapping(vec3 color) {
    const float startCompression = 0.8 - 0.04;
    const float desaturation = 0.15;
    float x = min(color.r, min(color.g, color.b));
    float offset = x < 0.08 ? x - 6.25 * x * x : 0.04;
    color -= offset;
    float peak = max(color.r, max(color.g, color.b));
    if (peak < startCompression)
        return color;
    float d = 1.0 - startCompression;
    float newPeak = 1.0 - d * d / (peak + d - startCompression);
    color *= newPeak / peak;
    float g = 1.0 - 1.0 / (desaturation * (peak - newPeak) + 1.0);
    return mix(color, vec3(newPeak), g);
}

vec3 linearToSrgb(vec3 c) {
    c = clamp(c, 0.0, 1.0);
    return mix(pow(c, vec3(0.41666)) * 1.055 - vec3(0.055), c * 12.92, vec3(lessThanEqual(c, vec3(0.0031308))));
}

// Premultiplied: what's drawn is blended over what's behind it with (1, 1 − alpha)
vec4 finalColor(vec3 color, float alpha) {
    return vec4(linearToSrgb(neutralToneMapping(color)) * alpha, alpha);
}
