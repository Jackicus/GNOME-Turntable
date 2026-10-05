// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

// The light around the model, before it's blurred for rough surfaces. The room is three.js's
// RoomEnvironment, traced rather than rendered: a white room with a few boxes in it, lit by a
// point light, and bright panels on its walls. The gradient is a sky, a horizon and the ground.

out vec4 fragColor;

// 0: the room, 1: the gradient
uniform int uKind;
uniform vec3 uSky;
uniform vec3 uHorizon;
uniform vec3 uGround;

// Where a ray first meets a box turned about y, or -1. `inside` takes where it leaves
float hitBox(vec3 origin, vec3 direction, vec3 center, vec3 size, float angle, bool inside, out vec3 normal) {
    float c = cos(angle), s = sin(angle);
    vec3 o = origin - center;
    vec3 lo = vec3(c * o.x - s * o.z, o.y, s * o.x + c * o.z);
    vec3 ld = vec3(c * direction.x - s * direction.z, direction.y, s * direction.x + c * direction.z);
    vec3 inv = 1.0 / ld;
    vec3 t0 = (-size / 2.0 - lo) * inv;
    vec3 t1 = (size / 2.0 - lo) * inv;
    vec3 tmin = min(t0, t1);
    vec3 tmax = max(t0, t1);
    float near = max(max(tmin.x, tmin.y), tmin.z);
    float far = min(min(tmax.x, tmax.y), tmax.z);
    if (near > far || far <= 0.0)
        return -1.0;
    float t = inside ? far : near;
    if (t <= 0.0)
        return -1.0;
    vec3 n;
    if (inside)
        n = far == tmax.x ? vec3(-sign(ld.x), 0.0, 0.0) : far == tmax.y ? vec3(0.0, -sign(ld.y), 0.0) : vec3(0.0, 0.0, -sign(ld.z));
    else
        n = near == tmin.x ? vec3(-sign(ld.x), 0.0, 0.0) : near == tmin.y ? vec3(0.0, -sign(ld.y), 0.0) : vec3(0.0, 0.0, -sign(ld.z));
    normal = vec3(c * n.x + s * n.z, n.y, -s * n.x + c * n.z);
    return t;
}

const vec3 LIGHT = vec3(0.418, 16.199, 0.300);

// White and rough, lit only by the point light (900 cd, reaching 28 units)
vec3 shade(vec3 p, vec3 n) {
    vec3 l = LIGHT - p;
    float d = length(l);
    float falloff = 1.0 / max(d * d, 0.01) * pow(clamp(1.0 - pow(d / 28.0, 4.0), 0.0, 1.0), 2.0);
    return vec3(900.0 * falloff * max(dot(n, l / d), 0.0) * RECIPROCAL_PI);
}

vec3 room(vec3 direction) {
    // The room's camera stands 3.5 above the scene's origin
    vec3 origin = vec3(0.0, 3.5, 0.0);
    vec3 normal;
    float best = hitBox(origin, direction, vec3(-0.757, 13.219, 0.717), vec3(31.713, 28.305, 28.591), 0.0, true, normal);
    vec3 color = best > 0.0 ? shade(origin + direction * best, normal) : vec3(0.0);

    vec3 boxes[18] = vec3[](
        vec3(-10.906, 2.009, 1.846), vec3(2.328, 7.905, 4.651), vec3(-0.195, 0.0, 0.0),
        vec3(-5.607, -0.754, -0.758), vec3(1.970, 1.534, 3.955), vec3(0.994, 0.0, 0.0),
        vec3(6.167, 0.857, 7.803), vec3(3.927, 6.285, 3.687), vec3(0.561, 0.0, 0.0),
        vec3(-2.017, 0.018, 6.124), vec3(2.002, 4.566, 2.064), vec3(0.333, 0.0, 0.0),
        vec3(2.291, -0.756, -2.621), vec3(1.546, 1.552, 1.496), vec3(-0.286, 0.0, 0.0),
        vec3(-2.193, -0.369, -5.547), vec3(3.875, 3.487, 2.986), vec3(0.516, 0.0, 0.0));
    for (int i = 0; i < 6; i++) {
        float t = hitBox(origin, direction, boxes[i * 3], boxes[i * 3 + 1], boxes[i * 3 + 2].x, false, normal);
        if (t > 0.0 && (best < 0.0 || t < best)) {
            best = t;
            color = shade(origin + direction * t, normal);
        }
    }

    // The panels: centre, size, brightness
    vec4 lights[12] = vec4[](
        vec4(-16.116, 14.37, 8.208, 50.0), vec4(0.1, 2.428, 2.739, 0.0),
        vec4(-16.109, 18.021, -8.207, 50.0), vec4(0.1, 2.425, 2.751, 0.0),
        vec4(14.904, 12.198, -1.832, 17.0), vec4(0.15, 4.265, 6.331, 0.0),
        vec4(-0.462, 8.89, 14.520, 43.0), vec4(4.38, 5.441, 0.088, 0.0),
        vec4(3.235, 11.486, -12.541, 20.0), vec4(2.5, 2.0, 0.1, 0.0),
        vec4(0.0, 20.0, 0.0, 100.0), vec4(1.0, 0.1, 1.0, 0.0));
    for (int i = 0; i < 6; i++) {
        float t = hitBox(origin, direction, lights[i * 2].xyz, lights[i * 2 + 1].xyz, 0.0, false, normal);
        if (t > 0.0 && (best < 0.0 || t < best)) {
            best = t;
            color = vec3(lights[i * 2].w);
        }
    }
    return color;
}

vec3 gradient(vec3 direction) {
    float y = direction.y;
    return y >= 0.0 ? mix(uHorizon, uSky, y) : mix(uHorizon, uGround, -y);
}

void main() {
    vec3 direction = faceDirection();
    fragColor = vec4(uKind == 0 ? room(direction) : gradient(direction), 1.0);
}
