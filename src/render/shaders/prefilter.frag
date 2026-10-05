// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

// The environment as a surface of some roughness reflects it: a GGX lobe's worth of it,
// importance sampled, each sample taken from a mip level as wide as its share of the lobe.

out vec4 fragColor;

uniform samplerCube uSource;
uniform float uSourceSize;
uniform float uRoughness;

const int SAMPLES = 512;

vec2 hammersley(int i) {
    uint bits = uint(i);
    bits = (bits << 16u) | (bits >> 16u);
    bits = ((bits & 0x55555555u) << 1u) | ((bits & 0xAAAAAAAAu) >> 1u);
    bits = ((bits & 0x33333333u) << 2u) | ((bits & 0xCCCCCCCCu) >> 2u);
    bits = ((bits & 0x0F0F0F0Fu) << 4u) | ((bits & 0xF0F0F0F0u) >> 4u);
    bits = ((bits & 0x00FF00FFu) << 8u) | ((bits & 0xFF00FF00u) >> 8u);
    return vec2(float(i) / float(SAMPLES), float(bits) * 2.3283064365386963e-10);
}

void main() {
    vec3 N = faceDirection();
    if (uRoughness == 0.0) {
        fragColor = vec4(textureLod(uSource, N, 0.0).rgb, 1.0);
        return;
    }
    float alpha = uRoughness * uRoughness;
    float a2 = alpha * alpha;
    vec3 up = abs(N.z) < 0.999 ? vec3(0.0, 0.0, 1.0) : vec3(1.0, 0.0, 0.0);
    vec3 tangent = normalize(cross(up, N));
    vec3 bitangent = cross(N, tangent);
    float texelAngle = 4.0 * PI / (6.0 * uSourceSize * uSourceSize);

    vec3 sum = vec3(0.0);
    float weight = 0.0;
    for (int i = 0; i < SAMPLES; i++) {
        vec2 xi = hammersley(i);
        float phi = 2.0 * PI * xi.x;
        float cosTheta = sqrt((1.0 - xi.y) / (1.0 + (a2 - 1.0) * xi.y));
        float sinTheta = sqrt(1.0 - cosTheta * cosTheta);
        vec3 H = normalize(tangent * cos(phi) * sinTheta + bitangent * sin(phi) * sinTheta + N * cosTheta);
        vec3 L = 2.0 * dot(N, H) * H - N;
        float dotNL = dot(N, L);
        if (dotNL <= 0.0)
            continue;
        float dotNH = max(dot(N, H), 0.0);
        float d = a2 / (PI * pow(dotNH * dotNH * (a2 - 1.0) + 1.0, 2.0));
        float pdf = d / 4.0 + 1e-4;
        float sampleAngle = 1.0 / (float(SAMPLES) * pdf + 1e-4);
        float lod = max(0.5 * log2(sampleAngle / texelAngle) + 1.0, 0.0);
        sum += textureLod(uSource, L, lod).rgb * dotNL;
        weight += dotNL;
    }
    fragColor = vec4(sum / max(weight, 1e-4), 1.0);
}
