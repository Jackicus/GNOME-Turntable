// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

// The direction a pixel of a cube map face looks in, by GL's cube map layout.

uniform int uFace;
uniform float uSize;

vec3 faceDirection() {
    vec2 st = gl_FragCoord.xy / uSize * 2.0 - 1.0;
    float s = st.x, t = st.y;
    if (uFace == 0) return normalize(vec3(1.0, -t, -s));
    if (uFace == 1) return normalize(vec3(-1.0, -t, s));
    if (uFace == 2) return normalize(vec3(s, 1.0, t));
    if (uFace == 3) return normalize(vec3(s, -1.0, -t));
    if (uFace == 4) return normalize(vec3(s, -t, 1.0));
    return normalize(vec3(-s, -t, -1.0));
}
