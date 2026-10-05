// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

// Every mesh, line and point: morphed and skinned when the model is, to world space.

uniform mat4 uModel;
uniform mat3 uNormalMatrix;

in vec3 aPosition;
in vec3 aNormal;
in vec2 aUv0;
in vec2 aUv1;
in vec4 aColor;
in uvec4 aJoints;
in vec4 aWeights;

out vec3 vWorldPosition;
out vec3 vNormal;
out vec2 vUv0;
out vec2 vUv1;
out vec4 vColor;

#ifdef MORPH
// Each target's offsets, vertex after vertex: position, then normal if uMorphStride is 2
uniform highp sampler2D uMorphs;
uniform int uMorphCount;
uniform int uMorphStride;
uniform int uVertexCount;
uniform float uMorphWeights[64];

vec3 morphOffset(int index) {
    return texelFetch(uMorphs, ivec2(index % 4096, index / 4096), 0).xyz;
}
#endif

#ifdef SKIN
// A joint's matrix in each row of four texels
uniform highp sampler2D uJoints;

mat4 joint(uint index) {
    int y = int(index);
    return mat4(texelFetch(uJoints, ivec2(0, y), 0), texelFetch(uJoints, ivec2(1, y), 0),
                texelFetch(uJoints, ivec2(2, y), 0), texelFetch(uJoints, ivec2(3, y), 0));
}
#endif

void main() {
    vec3 position = aPosition;
    vec3 normal = aNormal;
#ifdef MORPH
    for (int i = 0; i < uMorphCount; i++) {
        float weight = uMorphWeights[i];
        if (weight == 0.0)
            continue;
        int base = (i * uVertexCount + gl_VertexID) * uMorphStride;
        position += weight * morphOffset(base);
        if (uMorphStride == 2)
            normal += weight * morphOffset(base + 1);
    }
#endif
#ifdef SKIN
    // The joints carry the mesh to the world: its own node doesn't
    mat4 skin = aWeights.x * joint(aJoints.x) + aWeights.y * joint(aJoints.y) +
                aWeights.z * joint(aJoints.z) + aWeights.w * joint(aJoints.w);
    vec4 world = skin * vec4(position, 1.0);
    vNormal = mat3(skin) * normal;
#else
    vec4 world = uModel * vec4(position, 1.0);
    vNormal = uNormalMatrix * normal;
#endif
    vWorldPosition = world.xyz;
    vUv0 = aUv0;
    vUv1 = aUv1;
    vColor = aColor;
    gl_Position = uProjection * uView * world;
    gl_PointSize = uParams.z;
}
