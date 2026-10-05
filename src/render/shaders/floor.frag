// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

// The floor catches the model's shadow and nothing else: it's black where the shadow falls,
// see-through everywhere. Soft-edged, as three.js's PCF shadow with a radius of 4.

in vec3 vWorldPosition;
in vec3 vNormal;
in vec2 vUv0;
in vec2 vUv1;
in vec4 vColor;

out vec4 fragColor;

uniform highp sampler2DShadow uShadowMap;
uniform float uOpacity;

const float BIAS = -0.0005;
const float RADIUS = 4.0;

float lit(vec2 uv, float depth) {
    return texture(uShadowMap, vec3(uv, depth));
}

void main() {
    vec4 clip = uLightMatrix * vec4(vWorldPosition, 1.0);
    vec3 p = clip.xyz / clip.w * 0.5 + 0.5;
    float light = 1.0;
    if (all(greaterThanEqual(p, vec3(0.0))) && all(lessThanEqual(p, vec3(1.0)))) {
        float z = p.z + BIAS;
        vec2 texel = RADIUS / vec2(textureSize(uShadowMap, 0));
        float dx0 = -texel.x, dy0 = -texel.y, dx1 = texel.x, dy1 = texel.y;
        float dx2 = dx0 / 2.0, dy2 = dy0 / 2.0, dx3 = dx1 / 2.0, dy3 = dy1 / 2.0;
        vec2 uv = p.xy;
        light = (
            lit(uv + vec2(dx0, dy0), z) + lit(uv + vec2(0.0, dy0), z) + lit(uv + vec2(dx1, dy0), z) +
            lit(uv + vec2(dx2, dy2), z) + lit(uv + vec2(0.0, dy2), z) + lit(uv + vec2(dx3, dy2), z) +
            lit(uv + vec2(dx0, 0.0), z) + lit(uv + vec2(dx2, 0.0), z) + lit(uv, z) +
            lit(uv + vec2(dx3, 0.0), z) + lit(uv + vec2(dx1, 0.0), z) +
            lit(uv + vec2(dx2, dy3), z) + lit(uv + vec2(0.0, dy3), z) + lit(uv + vec2(dx3, dy3), z) +
            lit(uv + vec2(dx0, dy1), z) + lit(uv + vec2(0.0, dy1), z) + lit(uv + vec2(dx1, dy1), z)
        ) * (1.0 / 17.0);
    }
    fragColor = finalColor(vec3(0.0), uOpacity * (1.0 - light));
}
