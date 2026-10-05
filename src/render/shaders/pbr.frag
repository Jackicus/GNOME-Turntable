// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

// The model's surfaces: metallic-roughness PBR as three.js's MeshPhysicalMaterial draws it,
// sheen and clear coat included, lit by the key light and the environment. With DEPTH, only
// what the shadow map needs.

in vec3 vWorldPosition;
in vec3 vNormal;
in vec2 vUv0;
in vec2 vUv1;
in vec4 vColor;

out vec4 fragColor;

layout(std140) uniform Material {
    vec4 mBaseColor;
    vec4 mEmissive;
    // Metallic, roughness, normal scale, occlusion strength
    vec4 mFactors;
    // Alpha cutoff (-1 for none), blended (1), unlit (1), double sided (1)
    vec4 mAlpha;
    // Sheen colour and roughness
    vec4 mSheen;
    // Clear coat strength, roughness and normal scale
    vec4 mClearcoat;
    // Specular colour and strength
    vec4 mSpecular;
    // Index of refraction, transmission
    vec4 mExtra;
    // Each map's UV set, or -1 without the map: four to an ivec4, in model::Map's order
    ivec4 mMaps[4];
    // Each map's UV transform, two rows
    vec4 mTransforms[26];
};

const int BASE_COLOR = 0;
const int METALLIC_ROUGHNESS = 1;
const int NORMAL = 2;
const int OCCLUSION = 3;
const int EMISSIVE = 4;
const int SHEEN_COLOR = 5;
const int SHEEN_ROUGHNESS = 6;
const int CLEARCOAT = 7;
const int CLEARCOAT_ROUGHNESS = 8;
const int CLEARCOAT_NORMAL = 9;
const int SPECULAR = 10;
const int SPECULAR_COLOR = 11;
const int TRANSMISSION = 12;

uniform sampler2D uMap0;
uniform sampler2D uMap1;
uniform sampler2D uMap2;
uniform sampler2D uMap3;
uniform sampler2D uMap4;
uniform sampler2D uMap5;
uniform sampler2D uMap6;
uniform sampler2D uMap7;
uniform sampler2D uMap8;
uniform sampler2D uMap9;
uniform sampler2D uMap10;
uniform sampler2D uMap11;
uniform sampler2D uMap12;
uniform samplerCube uEnvironment;
// No normals: shade each facet flat
uniform int uFlat;

bool has(int map) {
    return mMaps[map / 4][map % 4] >= 0;
}

vec2 mapUv(int map) {
    vec3 uv = vec3(mMaps[map / 4][map % 4] == 1 ? vUv1 : vUv0, 1.0);
    return vec2(dot(mTransforms[map * 2].xyz, uv), dot(mTransforms[map * 2 + 1].xyz, uv));
}

float pow2(float x) { return x * x; }
float pow4(float x) { float x2 = x * x; return x2 * x2; }
float max3(vec3 v) { return max(max(v.x, v.y), v.z); }

vec3 F_Schlick(vec3 f0, float f90, float dotVH) {
    float fresnel = exp2((-5.55473 * dotVH - 6.98316) * dotVH);
    return f0 * (1.0 - fresnel) + f90 * fresnel;
}

float V_GGX_SmithCorrelated(float alpha, float dotNL, float dotNV) {
    float a2 = pow2(alpha);
    float gv = dotNL * sqrt(a2 + (1.0 - a2) * pow2(dotNV));
    float gl = dotNV * sqrt(a2 + (1.0 - a2) * pow2(dotNL));
    return 0.5 / max(gv + gl, 1e-6);
}

float D_GGX(float alpha, float dotNH) {
    float a2 = pow2(alpha);
    float denom = pow2(dotNH) * (a2 - 1.0) + 1.0;
    return RECIPROCAL_PI * a2 / pow2(denom);
}

vec3 BRDF_GGX(vec3 L, vec3 V, vec3 N, vec3 f0, float f90, float roughness) {
    float alpha = pow2(roughness);
    vec3 H = normalize(L + V);
    float dotNL = clamp(dot(N, L), 0.0, 1.0);
    float dotNV = clamp(dot(N, V), 0.0, 1.0);
    float dotNH = clamp(dot(N, H), 0.0, 1.0);
    float dotVH = clamp(dot(V, H), 0.0, 1.0);
    return F_Schlick(f0, f90, dotVH) * (V_GGX_SmithCorrelated(alpha, dotNL, dotNV) * D_GGX(alpha, dotNH));
}

// Estevez and Kulla's "Charlie" sheen, with Neubelt and Pettineo's visibility
vec3 BRDF_Sheen(vec3 L, vec3 V, vec3 N, vec3 sheenColor, float sheenRoughness) {
    vec3 H = normalize(L + V);
    float dotNL = clamp(dot(N, L), 0.0, 1.0);
    float dotNV = clamp(dot(N, V), 0.0, 1.0);
    float dotNH = clamp(dot(N, H), 0.0, 1.0);
    float invAlpha = 1.0 / pow2(sheenRoughness);
    float sin2h = max(1.0 - dotNH * dotNH, 0.0078125);
    float D = (2.0 + invAlpha) * pow(sin2h, invAlpha * 0.5) / (2.0 * PI);
    float V_ = clamp(1.0 / (4.0 * (dotNL + dotNV - dotNL * dotNV)), 0.0, 1.0);
    return sheenColor * (D * V_);
}

// The sheen BRDF integrated over the hemisphere, as a curve fit
float IBLSheenBRDF(vec3 N, vec3 V, float roughness) {
    float dotNV = clamp(dot(N, V), 0.0, 1.0);
    float r2 = roughness * roughness;
    float a = roughness < 0.25 ? -339.2 * r2 + 161.4 * roughness - 25.9 : -8.48 * r2 + 14.3 * roughness - 9.95;
    float b = roughness < 0.25 ? 44.0 * r2 - 23.7 * roughness + 3.26 : 1.97 * r2 - 3.27 * roughness + 0.72;
    float DG = exp(a * dotNV + b) + (roughness < 0.25 ? 0.0 : 0.1 * (roughness - 0.25));
    return clamp(DG * RECIPROCAL_PI, 0.0, 1.0);
}

vec2 DFGApprox(vec3 N, vec3 V, float roughness) {
    float dotNV = clamp(dot(N, V), 0.0, 1.0);
    const vec4 c0 = vec4(-1.0, -0.0275, -0.572, 0.022);
    const vec4 c1 = vec4(1.0, 0.0425, 1.04, -0.04);
    vec4 r = roughness * c0 + c1;
    float a004 = min(r.x * r.x, exp2(-9.28 * dotNV)) * r.x + r.y;
    return vec2(-1.04, 1.04) * a004 + r.zw;
}

vec3 environment(vec3 direction, float roughness) {
    return textureLod(uEnvironment, direction, roughness * uParams.y).rgb * uParams.x;
}

vec3 environmentRadiance(vec3 V, vec3 N, float roughness) {
    return environment(normalize(mix(reflect(-V, N), N, pow4(roughness))), roughness);
}

// A normal map's normal, in a tangent frame made from the derivatives as three.js makes it.
// glTF's green points up the image, against increasing v.
vec3 perturb(vec3 normal, vec3 mapN, float scale, vec2 uv, vec3 fdx, vec3 fdy, float faceDirection, bool doubleSided) {
    mapN.xy *= scale;
    vec2 st0 = dFdx(uv);
    vec2 st1 = dFdy(uv);
    vec3 q1perp = cross(fdy, normal);
    vec3 q0perp = cross(normal, fdx);
    vec3 T = q1perp * st0.x + q0perp * st1.x;
    vec3 B = q1perp * st0.y + q0perp * st1.y;
    float det = max(dot(T, T), dot(B, B));
    float s = det == 0.0 ? 0.0 : inversesqrt(det);
    if (doubleSided) {
        T *= faceDirection;
        B *= faceDirection;
    }
    return normalize(mat3(T * s, -B * s, normal) * mapN);
}

void main() {
    vec4 base = mBaseColor * vColor;
#ifdef DEPTH
    // Only a cutout changes the shadow's shape
    if (mAlpha.x < 0.0) {
        fragColor = vec4(0.0);
        return;
    }
#endif
    if (has(BASE_COLOR))
        base *= texture(uMap0, mapUv(BASE_COLOR));
    if (base.a < mAlpha.x)
        discard;
#ifdef DEPTH
    fragColor = vec4(0.0);
#else
    float alpha = mAlpha.y > 0.5 ? base.a : 1.0;
    if (mAlpha.z > 0.5) {
        fragColor = finalColor(base.rgb, alpha);
        return;
    }

    vec3 fdx = dFdx(vWorldPosition);
    vec3 fdy = dFdy(vWorldPosition);
    float faceDirection = gl_FrontFacing ? 1.0 : -1.0;
    bool doubleSided = mAlpha.w > 0.5;
    vec3 normal;
    if (uFlat == 1) {
        normal = normalize(cross(fdx, fdy));
    } else {
        normal = normalize(vNormal);
        if (doubleSided)
            normal *= faceDirection;
    }
    vec3 geometryNormal = normal;
    if (has(NORMAL)) {
        vec2 uv = mapUv(NORMAL);
        normal = perturb(normal, texture(uMap2, uv).xyz * 2.0 - 1.0, mFactors.z, uv, fdx, fdy, faceDirection, doubleSided);
    }

    float metallic = mFactors.x;
    float roughness = mFactors.y;
    if (has(METALLIC_ROUGHNESS)) {
        vec4 mr = texture(uMap1, mapUv(METALLIC_ROUGHNESS));
        roughness *= mr.g;
        metallic *= mr.b;
    }
    vec3 dxy = max(abs(dFdx(geometryNormal)), abs(dFdy(geometryNormal)));
    float geometryRoughness = max3(dxy);
    roughness = min(max(roughness, 0.0525) + geometryRoughness, 1.0);
    vec3 diffuseColor = base.rgb * (1.0 - metallic);

    // The specular colour from the index of refraction, tinted and scaled by KHR_materials_specular
    float specularIntensity = mSpecular.w;
    if (has(SPECULAR))
        specularIntensity *= texture(uMap10, mapUv(SPECULAR)).a;
    vec3 specularTint = mSpecular.rgb;
    if (has(SPECULAR_COLOR))
        specularTint *= texture(uMap11, mapUv(SPECULAR_COLOR)).rgb;
    float ior = mExtra.x;
    vec3 specularColor = mix(min(pow2((ior - 1.0) / (ior + 1.0)) * specularTint, vec3(1.0)) * specularIntensity, base.rgb, metallic);
    float specularF90 = mix(specularIntensity, 1.0, metallic);

    vec3 V = normalize(uCamera.xyz - vWorldPosition);
    float dotNV = clamp(dot(normal, V), 0.0, 1.0);
    vec3 L = uLightDirection.xyz;
    vec3 iblIrradiance = PI * environment(normal, 1.0);

    // The key light
    vec3 irradiance = clamp(dot(normal, L), 0.0, 1.0) * uLightColor.rgb;
    vec3 directDiffuse = irradiance * diffuseColor * RECIPROCAL_PI;
    vec3 directSpecular = irradiance * BRDF_GGX(L, V, normal, specularColor, specularF90, roughness);

    // The environment, with multiple scattering
    vec3 radiance = environmentRadiance(V, normal, roughness);
    vec2 fab = DFGApprox(normal, V, roughness);
    vec3 FssEss = specularColor * fab.x + specularF90 * fab.y;
    float Ems = 1.0 - (fab.x + fab.y);
    vec3 Favg = specularColor + (1.0 - specularColor) * 0.047619;
    vec3 Fms = FssEss * Favg / (1.0 - Ems * Favg);
    vec3 multiScattering = Fms * Ems;
    vec3 totalScattering = FssEss + multiScattering;
    vec3 diffuse = diffuseColor * (1.0 - max3(totalScattering));
    vec3 cosineWeightedIrradiance = iblIrradiance * RECIPROCAL_PI;
    vec3 indirectSpecular = radiance * FssEss + multiScattering * cosineWeightedIrradiance;
    vec3 indirectDiffuse = diffuse * cosineWeightedIrradiance;

    if (has(OCCLUSION)) {
        float ao = (texture(uMap3, mapUv(OCCLUSION)).r - 1.0) * mFactors.w + 1.0;
        indirectDiffuse *= ao;
        indirectSpecular *= clamp(pow(dotNV + ao, exp2(-16.0 * roughness - 1.0)) - 1.0 + ao, 0.0, 1.0);
    }

    // Light passing through, as through glass: the diffuse part gives way to what's behind
    float transmission = mExtra.y;
    if (has(TRANSMISSION))
        transmission *= texture(uMap12, mapUv(TRANSMISSION)).r;
    vec3 diffuseTotal = (directDiffuse + indirectDiffuse) * (1.0 - transmission);

    vec3 emissive = mEmissive.rgb;
    if (has(EMISSIVE))
        emissive *= texture(uMap4, mapUv(EMISSIVE)).rgb;
    vec3 color = diffuseTotal + directSpecular + indirectSpecular + emissive;

    vec3 sheenColor = mSheen.rgb;
    if (has(SHEEN_COLOR))
        sheenColor *= texture(uMap5, mapUv(SHEEN_COLOR)).rgb;
    if (max3(sheenColor) > 0.0) {
        float sheenRoughness = mSheen.a;
        if (has(SHEEN_ROUGHNESS))
            sheenRoughness *= texture(uMap6, mapUv(SHEEN_ROUGHNESS)).a;
        sheenRoughness = clamp(sheenRoughness, 0.07, 1.0);
        vec3 sheen = irradiance * BRDF_Sheen(L, V, normal, sheenColor, sheenRoughness)
                   + iblIrradiance * sheenColor * IBLSheenBRDF(normal, V, sheenRoughness);
        color = color * (1.0 - 0.157 * max3(sheenColor)) + sheen;
    }

    float clearcoat = mClearcoat.x;
    if (has(CLEARCOAT))
        clearcoat *= texture(uMap7, mapUv(CLEARCOAT)).r;
    if (clearcoat > 0.0) {
        float clearcoatRoughness = mClearcoat.y;
        if (has(CLEARCOAT_ROUGHNESS))
            clearcoatRoughness *= texture(uMap8, mapUv(CLEARCOAT_ROUGHNESS)).g;
        clearcoatRoughness = min(max(clearcoatRoughness, 0.0525) + geometryRoughness, 1.0);
        vec3 clearcoatNormal = geometryNormal;
        if (has(CLEARCOAT_NORMAL)) {
            vec2 uv = mapUv(CLEARCOAT_NORMAL);
            clearcoatNormal = perturb(geometryNormal, texture(uMap9, uv).xyz * 2.0 - 1.0, mClearcoat.z, uv, fdx, fdy, faceDirection, doubleSided);
        }
        vec3 ccIrradiance = clamp(dot(clearcoatNormal, L), 0.0, 1.0) * uLightColor.rgb;
        vec3 ccDirect = ccIrradiance * BRDF_GGX(L, V, clearcoatNormal, vec3(0.04), 1.0, clearcoatRoughness);
        vec2 ccFab = DFGApprox(clearcoatNormal, V, clearcoatRoughness);
        vec3 ccIndirect = environmentRadiance(V, clearcoatNormal, clearcoatRoughness) * (0.04 * ccFab.x + ccFab.y);
        vec3 Fcc = F_Schlick(vec3(0.04), 1.0, clamp(dot(clearcoatNormal, V), 0.0, 1.0));
        color = color * (1.0 - clearcoat * Fcc) + (ccDirect + ccIndirect) * clearcoat;
    }

    if (transmission > 0.0) {
        // Premultiplied: the reflections stay whole, the background shows through the rest
        float coverage = alpha * (1.0 - transmission * (1.0 - max3(totalScattering)));
        fragColor = vec4(finalColor(color, 1.0).rgb * alpha, coverage);
    } else {
        fragColor = finalColor(color, alpha);
    }
#endif
}
