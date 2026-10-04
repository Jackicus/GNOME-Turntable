// SPDX-License-Identifier: GPL-2.0-or-later
// SPDX-FileCopyrightText: 2026 Jack Tully

// The renderer: one three.js scene in the window's WebKit view. The window calls
// turntable.command(name, args) (viewer.js) and hears back through the "turntable" script
// message handler. The page is transparent, so the window's own background, light or dark,
// shows behind the model. It draws only while something moves: an idle model costs nothing.

import * as THREE from 'three';
import {OrbitControls} from 'three/addons/controls/OrbitControls.js';
import {RoomEnvironment} from 'three/addons/environments/RoomEnvironment.js';

const post = message => window.webkit?.messageHandlers?.turntable?.postMessage(message);

const SPIN_SPEED = 4;            // OrbitControls units: a turn every 15 seconds
const KEY_STEP = Math.PI / 12;   // an arrow key turns the model 15°
const START_DIRECTION = new THREE.Vector3().setFromSphericalCoords(
    1, THREE.MathUtils.degToRad(70), THREE.MathUtils.degToRad(30));

const renderer = new THREE.WebGLRenderer({antialias: true, alpha: true});
renderer.setPixelRatio(window.devicePixelRatio);
renderer.setSize(window.innerWidth, window.innerHeight);
renderer.setClearColor(0x000000, 0);
renderer.toneMapping = THREE.NeutralToneMapping;
renderer.shadowMap.enabled = true;
renderer.shadowMap.type = THREE.PCFShadowMap;
// The shadow only changes when the model or the light does, not when the camera moves: it's
// drawn again only when something sets needsUpdate.
renderer.shadowMap.autoUpdate = false;
document.body.append(renderer.domElement);

const scene = new THREE.Scene();
const camera = new THREE.PerspectiveCamera(35, window.innerWidth / window.innerHeight, 0.01, 100);
const controls = new OrbitControls(camera, renderer.domElement);
controls.enableDamping = true;
controls.autoRotateSpeed = SPIN_SPEED;
controls.addEventListener('change', invalidate);

const pmrem = new THREE.PMREMGenerator(renderer);

// The light that casts the floor shadow; each lighting preset sets its colour and direction.
const keyLight = new THREE.DirectionalLight(0xffffff, 1);
keyLight.castShadow = true;
keyLight.shadow.mapSize.set(2048, 2048);
keyLight.shadow.bias = -0.0005;
keyLight.shadow.radius = 4;
scene.add(keyLight, keyLight.target);

const floor = new THREE.Mesh(new THREE.PlaneGeometry(1, 1),
    new THREE.ShadowMaterial({opacity: 0.2, depthWrite: false}));
floor.rotation.x = -Math.PI / 2;
floor.receiveShadow = true;
scene.add(floor);

const wireMaterial = new THREE.MeshBasicMaterial({
    color: 0x3584e4, wireframe: true, transparent: true, depthWrite: false,
});

const neutralMaterial = () => new THREE.MeshStandardMaterial({
    color: 0xdeddda, roughness: 0.55, metalness: 0.05,
});

const state = {
    model: null,       // the model, in a pivot that stands it on the floor at the origin
    meshes: [],        // [mesh, its own material]: the wireframe passes swap the material
    radius: 1,         // of the model's bounding sphere
    height: 1,
    mixer: null,
    actions: [],
    action: null,      // the clip playing or paused
    playing: false,
    lastReport: 0,
    display: 'shaded',
    lighting: null,
    showGrid: false,
    grid: null,
    dark: false,
};

// Lighting presets: an environment for reflections and soft light, and one key light for the
// shadow. The environments are made here, not loaded, so the app ships no image files.
const LIGHTING = {
    studio: {
        environment: () => new RoomEnvironment(),
        intensity: 1,
        key: {color: 0xffffff, intensity: 1.5, direction: [0.5, 1, 0.6]},
        shadow: 0.2,
    },
    soft: {
        environment: () => gradientEnvironment(0xffffff, 0xd0d0d0, 0x9a9996),
        intensity: 1,
        key: {color: 0xffffff, intensity: 0.4, direction: [0, 1, 0.2]},
        shadow: 0.1,
    },
    sunlight: {
        environment: () => gradientEnvironment(0x99c1f1, 0xf6f5f4, 0x865e3c),
        intensity: 0.7,
        key: {color: 0xfff1d6, intensity: 3, direction: [-0.6, 1, 0.5]},
        shadow: 0.35,
    },
    dramatic: {
        environment: () => new RoomEnvironment(),
        intensity: 0.12,
        key: {color: 0xffffff, intensity: 4, direction: [1, 1.2, -0.4]},
        shadow: 0.3,
    },
};
const environments = new Map();

// A sky above, a horizon and the ground below, on a sphere around the model.
function gradientEnvironment(sky, horizon, ground) {
    const geometry = new THREE.SphereGeometry(1, 32, 16);
    const colors = [];
    const [top, middle, bottom] = [sky, horizon, ground].map(c => new THREE.Color(c));
    const position = geometry.attributes.position;
    for (let i = 0; i < position.count; i++) {
        const y = position.getY(i);
        const c = y >= 0 ? middle.clone().lerp(top, y) : middle.clone().lerp(bottom, -y);
        colors.push(c.r, c.g, c.b);
    }
    geometry.setAttribute('color', new THREE.Float32BufferAttribute(colors, 3));
    const environment = new THREE.Scene();
    environment.add(new THREE.Mesh(geometry,
        new THREE.MeshBasicMaterial({vertexColors: true, side: THREE.BackSide})));
    return environment;
}

function setLighting(name) {
    const preset = LIGHTING[name] ?? LIGHTING.studio;
    if (!environments.has(name)) {
        const source = preset.environment();
        environments.set(name, pmrem.fromScene(source, 0.04).texture);
        source.traverse(o => {
            o.geometry?.dispose();
            o.material?.dispose();
        });
    }
    scene.environment = environments.get(name);
    scene.environmentIntensity = preset.intensity;
    keyLight.color.set(preset.key.color);
    keyLight.intensity = preset.key.intensity;
    state.lighting = preset;
    placeLights();
    invalidate();
}

// Scales the key light, its shadow, the floor and the grid to the model.
function placeLights() {
    const {radius, height} = state;
    const preset = state.lighting ?? LIGHTING.studio;
    const target = new THREE.Vector3(0, height / 2, 0);
    keyLight.target.position.copy(target);
    keyLight.position.copy(target).addScaledVector(
        new THREE.Vector3(...preset.key.direction).normalize(), radius * 4);
    const shadow = keyLight.shadow.camera;
    shadow.left = shadow.bottom = -radius * 2.5;
    shadow.right = shadow.top = radius * 2.5;
    shadow.near = radius * 0.5;
    shadow.far = radius * 8;
    shadow.updateProjectionMatrix();
    floor.scale.setScalar(radius * 12);
    floor.position.y = -radius * 0.002;
    floor.material.opacity = preset.shadow * (state.dark ? 1.3 : 1);
    renderer.shadowMap.needsUpdate = true;
    updateGrid();
}

function updateGrid() {
    if (state.grid) {
        scene.remove(state.grid);
        state.grid.geometry.dispose();
        state.grid.material.dispose();
        state.grid = null;
    }
    if (!state.showGrid || !state.model)
        return;
    // A round number of cells about a tenth of the model's size wide
    const cell = 10 ** Math.floor(Math.log10(state.radius / 2));
    const divisions = Math.ceil(state.radius * 3 / cell) * 2;
    const grid = new THREE.GridHelper(divisions * cell, divisions);
    grid.material.vertexColors = false;
    grid.material.transparent = true;
    grid.material.depthWrite = false;
    grid.material.color.set(state.dark ? 0xffffff : 0x000000);
    grid.material.opacity = state.dark ? 0.14 : 0.12;
    scene.add(grid);
    state.grid = grid;
}

// Model loading. Each loader is imported the first time it's needed, so startup parses only
// three.js itself. Every loader resolves to {object, animations?}.

const LOADERS = {
    async gltf(url) {
        const [{GLTFLoader}, {DRACOLoader}, {MeshoptDecoder}] = await Promise.all([
            import('three/addons/loaders/GLTFLoader.js'),
            import('three/addons/loaders/DRACOLoader.js'),
            import('three/addons/libs/meshopt_decoder.module.js'),
        ]);
        const draco = new DRACOLoader();
        draco.setDecoderPath(new URL('./three/addons/libs/draco/gltf/', import.meta.url).href);
        const loader = new GLTFLoader().setDRACOLoader(draco).setMeshoptDecoder(MeshoptDecoder);
        try {
            const gltf = await loader.loadAsync(url);
            return {object: gltf.scene, animations: gltf.animations};
        } finally {
            draco.dispose();
        }
    },

    async obj(url) {
        const [{OBJLoader}, {MTLLoader}] = await Promise.all([
            import('three/addons/loaders/OBJLoader.js'),
            import('three/addons/loaders/MTLLoader.js'),
        ]);
        const response = await fetch(url);
        if (!response.ok)
            throw new Error(`${response.status} ${url}`);
        const text = await response.text();
        const loader = new OBJLoader();
        const library = /^mtllib\s+(.+?)\s*$/m.exec(text)?.[1];
        let hasMaterials = false;
        if (library) {
            try {
                const materials = await new MTLLoader().loadAsync(new URL(library, url).href);
                materials.preload();
                loader.setMaterials(materials);
                hasMaterials = true;
            } catch (e) {
                post({type: 'log', text: `No materials from ${library}: ${e}`});
            }
        }
        const object = loader.parse(text);
        if (!hasMaterials)
            useNeutralMaterial(object);
        return {object};
    },

    async stl(url) {
        const {STLLoader} = await import('three/addons/loaders/STLLoader.js');
        const geometry = await new STLLoader().loadAsync(url);
        const material = neutralMaterial();
        material.vertexColors = Boolean(geometry.hasColors);
        const mesh = new THREE.Mesh(geometry, material);
        mesh.rotation.x = -Math.PI / 2;  // STL is Z-up
        return {object: wrap(mesh)};
    },

    async ply(url) {
        const {PLYLoader} = await import('three/addons/loaders/PLYLoader.js');
        const geometry = await new PLYLoader().loadAsync(url);
        const colors = geometry.hasAttribute('color');
        if (!geometry.index) {
            // No faces: a point cloud
            const material = new THREE.PointsMaterial({
                size: 2, sizeAttenuation: false, vertexColors: colors,
                color: colors ? 0xffffff : 0x77767b,
            });
            return {object: wrap(new THREE.Points(geometry, material))};
        }
        if (!geometry.hasAttribute('normal'))
            geometry.computeVertexNormals();
        const material = neutralMaterial();
        material.vertexColors = colors;
        return {object: wrap(new THREE.Mesh(geometry, material))};
    },

    async fbx(url) {
        const {FBXLoader} = await import('three/addons/loaders/FBXLoader.js');
        const object = await new FBXLoader().loadAsync(url);
        return {object, animations: object.animations};
    },

    async '3mf'(url) {
        const {ThreeMFLoader} = await import('three/addons/loaders/3MFLoader.js');
        const object = await new ThreeMFLoader().loadAsync(url);
        object.rotation.x = -Math.PI / 2;  // 3MF is Z-up
        return {object: wrap(object)};
    },
};

function wrap(object) {
    const group = new THREE.Group();
    group.add(object);
    return group;
}

function useNeutralMaterial(object) {
    object.traverse(o => {
        if (!o.isMesh)
            return;
        const material = neutralMaterial();
        material.vertexColors = o.geometry.hasAttribute('color');
        o.material = material;
    });
}

function clear() {
    stopAnimation();
    if (!state.model)
        return;
    scene.remove(state.model);
    state.model.traverse(o => {
        o.geometry?.dispose();
        for (const material of [o.material ?? []].flat()) {
            for (const value of Object.values(material)) {
                if (value?.isTexture)
                    value.dispose();
            }
            material.dispose();
        }
    });
    state.model = null;
    state.meshes = [];
    renderer.shadowMap.needsUpdate = true;
    updateGrid();
    invalidate();
}

async function load({url, format}) {
    let result;
    try {
        result = await LOADERS[format](url);
    } catch (e) {
        clear();
        return {error: 'failed', detail: String(e?.stack ?? e)};
    }
    clear();
    const model = result.object;
    model.updateMatrixWorld(true);
    const box = new THREE.Box3().setFromObject(model);
    if (box.isEmpty() || !Number.isFinite(box.min.x)) {
        return {error: 'empty', detail: 'Nothing to show'};
    }
    const size = box.getSize(new THREE.Vector3());
    const center = box.getCenter(new THREE.Vector3());
    // Stand the model on the floor, at the origin
    model.position.sub(new THREE.Vector3(center.x, box.min.y, center.z));
    model.traverse(o => {
        if (o.isMesh) {
            state.meshes.push([o, o.material]);
            o.castShadow = true;
            // Keeps the shaded surface just behind its own wireframe
            for (const material of [o.material].flat()) {
                material.polygonOffset = true;
                material.polygonOffsetFactor = 1;
                material.polygonOffsetUnits = 1;
            }
        }
    });
    const pivot = wrap(model);
    scene.add(pivot);
    state.model = pivot;
    state.radius = Math.max(size.length() / 2, 1e-6);
    state.height = size.y;
    placeLights();
    resetView();
    const animations = result.animations ?? [];
    if (animations.length)
        startAnimation(pivot, animations);
    return measure(pivot, animations, size);
}

function measure(model, animations, size) {
    let vertices = 0, triangles = 0, points = 0, meshes = 0;
    const materials = new Set();
    model.traverse(o => {
        const position = o.geometry?.attributes.position;
        if (!position)
            return;
        const instances = o.isInstancedMesh ? o.count : 1;
        if (o.isMesh) {
            meshes += instances;
            vertices += position.count * instances;
            const indices = o.geometry.index ? o.geometry.index.count : position.count;
            triangles += Math.floor(indices / 3) * instances;
            for (const material of [o.material].flat())
                materials.add(material);
        } else if (o.isPoints) {
            points += position.count;
        }
    });
    return {
        vertices, triangles, points, meshes,
        materials: materials.size,
        size: size.toArray(),
        animations: animations.map((a, i) => ({name: a.name || `${i + 1}`, duration: a.duration})),
    };
}

// Camera

function resetView() {
    const {radius, height} = state;
    const target = new THREE.Vector3(0, height / 2, 0);
    const vertical = THREE.MathUtils.degToRad(camera.fov);
    const horizontal = 2 * Math.atan(Math.tan(vertical / 2) * camera.aspect);
    const distance = radius / Math.sin(Math.min(vertical, horizontal) / 2) * 1.3;
    camera.near = radius / 100;
    camera.far = distance + radius * 50;
    camera.updateProjectionMatrix();
    camera.position.copy(target).addScaledVector(START_DIRECTION, distance);
    controls.target.copy(target);
    controls.minDistance = radius * 0.2;
    controls.maxDistance = distance * 8;
    controls.update();
    invalidate();
}

const offset = new THREE.Vector3();
const spherical = new THREE.Spherical();

function orbit(left, up) {
    offset.copy(camera.position).sub(controls.target);
    spherical.setFromVector3(offset);
    spherical.theta += left;
    spherical.phi = THREE.MathUtils.clamp(spherical.phi + up, 0.01, Math.PI - 0.01);
    offset.setFromSpherical(spherical);
    camera.position.copy(controls.target).add(offset);
    controls.update();
}

function dolly(scale) {
    offset.copy(camera.position).sub(controls.target);
    const distance = THREE.MathUtils.clamp(offset.length() * scale,
        controls.minDistance, controls.maxDistance);
    camera.position.copy(controls.target).add(offset.setLength(distance));
    controls.update();
}

// Animation

function startAnimation(model, clips) {
    state.mixer = new THREE.AnimationMixer(model);
    state.actions = clips.map(clip => state.mixer.clipAction(clip));
    state.playing = true;
    selectClip(0);
}

function stopAnimation() {
    if (!state.mixer)
        return;
    state.mixer.stopAllAction();
    state.mixer.uncacheRoot(state.mixer.getRoot());
    state.mixer = null;
    state.actions = [];
    state.action = null;
    state.playing = false;
}

function selectClip(index) {
    if (!state.actions[index])
        return null;
    state.action?.stop();
    state.action = state.actions[index];
    state.action.reset().play();
    state.action.paused = !state.playing;
    state.mixer.update(0);
    renderer.shadowMap.needsUpdate = true;
    reportAnimation();
    invalidate();
    return {duration: state.action.getClip().duration};
}

function setPlaying(playing) {
    if (!state.action)
        return;
    state.playing = playing;
    state.action.paused = !playing;
    reportAnimation();
    invalidate();
}

function seek(time) {
    if (!state.action)
        return;
    state.action.time = time;
    state.mixer.update(0);
    renderer.shadowMap.needsUpdate = true;
    reportAnimation();
    invalidate();
}

function reportAnimation() {
    state.lastReport = performance.now();
    post({type: 'animation', playing: state.playing, time: state.action?.time ?? 0});
}

// Drawing

let frameRequested = false;
let lastFrame = null;
let lastChange = 0;

function invalidate() {
    if (frameRequested)
        return;
    frameRequested = true;
    requestAnimationFrame(frame);
}

function frame(now) {
    frameRequested = false;
    const delta = lastFrame === null ? 1 / 60 : Math.min((now - lastFrame) / 1000, 0.1);
    lastFrame = now;
    // The damping decays with time, not frames (0.05 a frame at 60 Hz), so a flick coasts the
    // same at any refresh rate
    controls.dampingFactor = 1 - 0.95 ** (delta * 60);
    let moving = controls.update(delta);
    if (state.playing && state.action) {
        state.mixer.update(delta);
        renderer.shadowMap.needsUpdate = true;
        if (now - state.lastReport > 100)
            reportAnimation();
        moving = true;
    }
    draw();
    // The controls report a change only above a small threshold, which a slowing coast falls
    // under a while before it ends: frames go on for a moment after the last reported one,
    // so the coast finishes instead of stopping short.
    if (moving || controls.autoRotate)
        lastChange = now;
    if (now - lastChange < 250)
        invalidate();
    else
        lastFrame = null;
}

// The wireframe is the meshes drawn again with wireMaterial: alone, or over the shaded model
// (the polygon offset set in load() keeps the surface just behind its lines). Points keep their
// own material, which is the only one that draws them.
function draw() {
    const wire = state.display !== 'shaded' && state.model;
    if (!wire || state.display === 'shaded-wireframe')
        renderer.render(scene, camera);
    if (!wire)
        return;
    const overlay = state.display === 'shaded-wireframe';
    const hidden = [floor, state.grid].filter(o => o && (overlay || o === floor));
    if (overlay)
        state.model.traverse(o => o.isPoints && hidden.push(o));
    hidden.forEach(o => (o.visible = false));
    state.meshes.forEach(([mesh]) => (mesh.material = wireMaterial));
    wireMaterial.opacity = overlay ? 0.45 : 0.9;
    renderer.autoClear = !overlay;
    renderer.render(scene, camera);
    renderer.autoClear = true;
    state.meshes.forEach(([mesh, material]) => (mesh.material = material));
    hidden.forEach(o => (o.visible = true));
}

new ResizeObserver(() => {
    renderer.setPixelRatio(window.devicePixelRatio);
    renderer.setSize(window.innerWidth, window.innerHeight);
    camera.aspect = window.innerWidth / Math.max(window.innerHeight, 1);
    camera.updateProjectionMatrix();
    invalidate();
}).observe(document.body);

// Keys the renderer handles itself while it has the focus; the window's shortcuts (with Ctrl)
// reach the window first. R and Space belong to the window's actions, so they go back to it.
window.addEventListener('keydown', event => {
    if (event.ctrlKey || event.altKey || event.metaKey || !state.model)
        return;
    switch (event.key) {
    case 'ArrowLeft': orbit(KEY_STEP, 0); break;
    case 'ArrowRight': orbit(-KEY_STEP, 0); break;
    case 'ArrowUp': orbit(0, KEY_STEP); break;
    case 'ArrowDown': orbit(0, -KEY_STEP); break;
    case '+': case '=': dolly(0.8); break;
    case '-': case '_': dolly(1.25); break;
    case 'r': case 'R': post({type: 'action', name: 'toggle-spin'}); break;
    case ' ': post({type: 'action', name: 'toggle-playback'}); break;
    default: return;
    }
    event.preventDefault();
});

const commands = {
    load,
    clear,
    resetView,
    snapshot() {
        draw();
        return renderer.domElement.toDataURL('image/png');
    },
    theme({dark, accent}) {
        state.dark = dark;
        wireMaterial.color.set(accent);
        placeLights();
        invalidate();
    },
    lighting: ({name}) => setLighting(name),
    display({mode}) {
        state.display = mode;
        invalidate();
    },
    grid({visible}) {
        state.showGrid = visible;
        updateGrid();
        invalidate();
    },
    spin({enabled}) {
        controls.autoRotate = enabled;
        invalidate();
    },
    play: () => setPlaying(true),
    pause: () => setPlaying(false),
    seek: ({time}) => seek(time),
    clip: ({index}) => selectClip(index),
};

window.turntable = {
    command(name, args) {
        const command = commands[name];
        if (!command)
            throw new Error(`No command ${name}`);
        return command(args ?? {}) ?? null;
    },
};

setLighting('studio');
post({type: 'ready'});
