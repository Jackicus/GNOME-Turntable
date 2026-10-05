// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

// One colour, unlit: wireframes, the grid, points and lines.

in vec3 vWorldPosition;
in vec3 vNormal;
in vec2 vUv0;
in vec2 vUv1;
in vec4 vColor;

out vec4 fragColor;

uniform vec4 uColor;
uniform int uVertexColors;

void main() {
    vec4 color = uVertexColors == 1 ? uColor * vColor : uColor;
    fragColor = finalColor(color.rgb, color.a);
}
