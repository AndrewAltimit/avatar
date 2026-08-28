/* viewer.js — 3D avatar preview for the Analyzer page (vanilla ES module, no build step).
 *
 * Renders the `SceneView` the wasm bundle (crates/web-analyzer) exposes — see the
 * wasm <-> site contract: `manifest()` JSON plus flat typed arrays per mesh
 * (`positions / normals / uvs / indices / triangle_materials / skin_indices /
 * skin_weights`) and `texture(material)` bytes. three.js (pinned 0.170.0) is
 * loaded lazily from jsDelivr the first time a viewer is created; nothing here
 * touches the wasm module itself beyond those accessors.
 *
 * API
 * ---
 *   import { createViewer } from './viewer.js';
 *   const v = createViewer(containerEl, {
 *     onBoneSelect(index|null, bone|null) {},  // a joint was clicked (index into manifest.bones)
 *     onError(err) {},                          // three.js failed to load / a load() failed
 *     onReady() {},                             // three.js loaded, canvas mounted
 *   });
 *
 *   await v.ready;                          // resolves once three.js is up (rejects on CDN failure)
 *   await v.load(sceneView, manifest);      // build the scene (manifest = JSON.parse(sceneView.manifest()));
 *                                           // pass `manifest` as an object or the raw JSON string.
 *                                           // Frees the previous scene's GPU resources first.
 *   v.setMode('shaded'|'wireframe'|'uv'|'weights'|'normals');
 *   v.setOverlay('skeleton'|'grid'|'labels'|'autorotate', bool);
 *   v.highlightBone(index|null);            // select a joint from outside (e.g. the bone tree)
 *   v.resetView();                          // re-frame the camera from the bounds
 *   await v.applyTextures(library);         // library.find(relative, absolute, materialName) -> { name, blob, via? } | null:
 *                                           // resolve external (non-embedded) textures from dropped files
 *   v.textureStatus() -> [{ index, name, path, kind: embedded|file|missing|none|error, source, width, height, alpha, thumb }]
 *   v.onTextures = status => {};            // fires whenever texture decoding settles
 *   v.screenshot() -> 'data:image/png;base64,…'
 *   v.getState()   -> { mode, overlays, selectedBone, stats }
 *   v.dispose();                            // tear down everything (canvas, GPU, listeners)
 *
 * The container gets `class="av-viewer"`; the toolbar, tooltip and stats readout are
 * rendered inside it. All styling lives in css/viewer.css (link it on the page).
 * The mesh data is uploaded straight from the wasm typed arrays into BufferGeometry —
 * no per-vertex JS objects — so 300k-triangle avatars stay cheap.
 */

const THREE_VERSION = '0.170.0';
const CDN = `https://cdn.jsdelivr.net/npm/three@${THREE_VERSION}`;

const MODES = ['shaded', 'wireframe', 'uv', 'weights', 'normals'];
const MODE_LABELS = { shaded: 'Shaded', wireframe: 'Wireframe', uv: 'UV check', weights: 'Bone weights', normals: 'Normals' };
const OVERLAYS = ['skeleton', 'grid', 'labels', 'autorotate'];
const OVERLAY_LABELS = { skeleton: 'Skeleton', grid: 'Grid', labels: 'Labels', autorotate: 'Auto-rotate' };

// Palette for meshes that carry no material (one colour per mesh, cycling).
const MESH_PALETTE = [0x8fb3d9, 0xd9a58f, 0x9fd98f, 0xd98fcf, 0xd9d08f, 0x8fd9d4, 0xb08fd9, 0xd98f8f];

// Humanoid bone → colour family: spine chain, head, left side, right side.
const SPINE = new Set(['Hips', 'Spine', 'Chest', 'UpperChest', 'Neck']);
const HEAD = new Set(['Head', 'Jaw', 'LeftEye', 'RightEye']);
const BONE_COLORS = { spine: 0xf2c14e, head: 0xf28e4e, left: 0x58c4b3, right: 0xd96c9a, unmapped: 0x7d858f, other: 0xb0b8c0 };

function boneFamily(bone) {
  const h = bone.humanoid;
  if (!h) return bone.bone_like ? 'unmapped' : 'other';
  if (SPINE.has(h)) return 'spine';
  if (HEAD.has(h)) return 'head';
  if (h.startsWith('Left')) return 'left';
  if (h.startsWith('Right')) return 'right';
  return 'spine';
}

// --- three.js loading -----------------------------------------------------------

let threePromise = null;

function ensureImportMap() {
  if (document.querySelector('script[type="importmap"]')) return;
  const s = document.createElement('script');
  s.type = 'importmap';
  s.textContent = JSON.stringify({
    imports: {
      three: `${CDN}/build/three.module.js`,
      'three/addons/': `${CDN}/examples/jsm/`,
    },
  });
  document.head.appendChild(s);
}

/** Load three + OrbitControls once. Tries the import map (bare specifiers) first; if the
 *  browser refuses a late import map, falls back to jsDelivr's `/+esm` endpoints, which
 *  rewrite `three` imports so both modules still share one three instance. */
function loadThree() {
  if (threePromise) return threePromise;
  threePromise = (async () => {
    try {
      ensureImportMap();
      const [THREE, orbit] = await Promise.all([
        import('three'),
        import('three/addons/controls/OrbitControls.js'),
      ]);
      return { THREE, OrbitControls: orbit.OrbitControls, viaMap: true };
    } catch (e) {
      const [THREE, orbit] = await Promise.all([
        import(`${CDN}/+esm`),
        import(`${CDN}/examples/jsm/controls/OrbitControls.js/+esm`),
      ]);
      return { THREE, OrbitControls: orbit.OrbitControls, viaMap: false };
    }
  })();
  threePromise.catch(() => { threePromise = null; });
  return threePromise;
}

let tgaPromise = null;
function loadTgaLoader(viaMap) {
  if (!tgaPromise) {
    tgaPromise = import(viaMap ? 'three/addons/loaders/TGALoader.js' : `${CDN}/examples/jsm/loaders/TGALoader.js/+esm`)
      .then(m => m.TGALoader);
  }
  return tgaPromise;
}

// --- helpers ------------------------------------------------------------------------

function cssVar(name, fallback) {
  const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return v || fallback;
}

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text != null) e.textContent = text;
  return e;
}

function fmt(n) { return Number(n || 0).toLocaleString(); }

function makeCheckerTexture(THREE) {
  const size = 512, cells = 16, c = document.createElement('canvas');
  c.width = c.height = size;
  const ctx = c.getContext('2d'), cs = size / cells;
  for (let y = 0; y < cells; y++) {
    for (let x = 0; x < cells; x++) {
      const hue = ((x * 7 + y * 13) % cells) / cells;
      ctx.fillStyle = (x + y) % 2 ? `hsl(${hue * 360} 55% 42%)` : `hsl(${hue * 360} 45% 78%)`;
      ctx.fillRect(x * cs, y * cs, cs, cs);
    }
  }
  ctx.strokeStyle = 'rgba(0,0,0,0.35)';
  ctx.lineWidth = 2;
  for (let i = 0; i <= cells; i++) {
    ctx.beginPath(); ctx.moveTo(i * cs, 0); ctx.lineTo(i * cs, size); ctx.stroke();
    ctx.beginPath(); ctx.moveTo(0, i * cs); ctx.lineTo(size, i * cs); ctx.stroke();
  }
  ctx.fillStyle = '#000';
  ctx.font = 'bold 28px sans-serif';
  ctx.textAlign = 'center';
  ctx.fillText('UV', size * 0.5, size * 0.5 + 10);
  const t = new THREE.CanvasTexture(c);
  t.wrapS = t.wrapT = THREE.RepeatWrapping;
  t.colorSpace = THREE.SRGBColorSpace;
  t.anisotropy = 4;
  return t;
}

function makeLabelSprite(THREE, text, color) {
  const pad = 8, font = '600 26px system-ui, sans-serif';
  const c = document.createElement('canvas');
  const ctx = c.getContext('2d');
  ctx.font = font;
  const w = Math.ceil(ctx.measureText(text).width) + pad * 2, hgt = 40;
  c.width = w; c.height = hgt;
  ctx.font = font;
  ctx.fillStyle = 'rgba(10,14,21,0.8)';
  ctx.fillRect(0, 0, w, hgt);
  ctx.fillStyle = color;
  ctx.textBaseline = 'middle';
  ctx.fillText(text, pad, hgt / 2);
  const tex = new THREE.CanvasTexture(c);
  tex.colorSpace = THREE.SRGBColorSpace;
  const mat = new THREE.SpriteMaterial({ map: tex, depthTest: false, transparent: true, sizeAttenuation: true });
  const sp = new THREE.Sprite(mat);
  sp.userData.aspect = w / hgt;
  sp.renderOrder = 1001;
  return sp;
}

// blue → cyan → green → yellow → red
function heat(w, out, o) {
  const t = Math.max(0, Math.min(1, w));
  let r, g, b;
  if (t < 0.25) { r = 0; g = t / 0.25; b = 1; }
  else if (t < 0.5) { r = 0; g = 1; b = 1 - (t - 0.25) / 0.25; }
  else if (t < 0.75) { r = (t - 0.5) / 0.25; g = 1; b = 0; }
  else { r = 1; g = 1 - (t - 0.75) / 0.25; b = 0; }
  out[o] = r; out[o + 1] = g; out[o + 2] = b;
}

function hashColor(i, out, o) {
  const hue = ((i * 0.618033988749895) % 1) * 6;
  const c = 0.75, x = c * (1 - Math.abs((hue % 2) - 1)), m = 0.2;
  let r = 0, g = 0, b = 0;
  const k = Math.floor(hue);
  if (k === 0) { r = c; g = x; } else if (k === 1) { r = x; g = c; } else if (k === 2) { g = c; b = x; }
  else if (k === 3) { g = x; b = c; } else if (k === 4) { r = x; b = c; } else { r = c; b = x; }
  out[o] = r + m; out[o + 1] = g + m; out[o + 2] = b + m;
}

// --- the viewer ---------------------------------------------------------------------

export function createViewer(container, opts = {}) {
  if (!container) throw new Error('createViewer: container element required');
  container.classList.add('av-viewer');
  container.innerHTML = '';

  const dom = {
    stage: el('div', 'av-stage'),
    toolbar: el('div', 'av-toolbar'),
    tooltip: el('div', 'av-tooltip'),
    stats: el('div', 'av-stats'),
    message: el('div', 'av-message'),
  };
  dom.tooltip.hidden = true;
  dom.message.hidden = true;
  container.append(dom.stage, dom.toolbar, dom.stats, dom.tooltip, dom.message);

  const state = {
    mode: 'shaded',
    overlays: { skeleton: true, grid: true, labels: false, autorotate: false },
    selectedBone: null,
    hoverBone: null,
    stats: null,
    disposed: false,
    loaded: false,
    library: (opts && opts.textureLibrary) || null,
  };

  let THREE = null, OrbitControls = null, viaMap = true;
  let renderer, scene, camera, controls, raycaster, resizeObs, rafId = 0, loopRunning = false;
  let sharedMats = null;     // mode materials shared by every mesh: wireframe / uv / normals / weights
  let checker = null;
  const buttons = {};

  // Per-load resources.
  let world = null;         // { group, meshes:[{mesh, meshIndex, shadedMat}], geoms, textures, mats, blobUrls,
                            //   skeleton:{lines, joints, labels, positions, boneCount}, grid, bounds, manifest, skin:[] }

  function showMessage(text, isError) {
    dom.message.textContent = text;
    dom.message.classList.toggle('is-error', !!isError);
    dom.message.hidden = !text;
  }

  // --- toolbar -------------------------------------------------------------------
  function buildToolbar() {
    const modeGroup = el('div', 'av-tb-group');
    for (const m of MODES) {
      const b = el('button', 'av-tb-btn', MODE_LABELS[m]);
      b.type = 'button';
      b.dataset.mode = m;
      b.addEventListener('click', () => api.setMode(m));
      modeGroup.append(b);
      buttons['mode:' + m] = b;
    }
    const ovGroup = el('div', 'av-tb-group');
    for (const o of OVERLAYS) {
      const b = el('button', 'av-tb-btn av-tb-toggle', OVERLAY_LABELS[o]);
      b.type = 'button';
      b.dataset.overlay = o;
      b.setAttribute('aria-pressed', String(state.overlays[o]));
      b.addEventListener('click', () => api.setOverlay(o, !state.overlays[o]));
      ovGroup.append(b);
      buttons['ov:' + o] = b;
    }
    const actGroup = el('div', 'av-tb-group');
    const reset = el('button', 'av-tb-btn', 'Reset view');
    reset.type = 'button';
    reset.addEventListener('click', () => api.resetView());
    const shot = el('button', 'av-tb-btn', 'Screenshot');
    shot.type = 'button';
    shot.addEventListener('click', () => {
      const url = api.screenshot();
      if (!url) return;
      const a = document.createElement('a');
      a.href = url; a.download = 'avatar.png';
      a.click();
    });
    actGroup.append(reset, shot);
    dom.toolbar.append(modeGroup, ovGroup, actGroup);
    syncToolbar();
  }

  function syncToolbar() {
    for (const m of MODES) buttons['mode:' + m]?.classList.toggle('is-active', state.mode === m);
    for (const o of OVERLAYS) {
      const b = buttons['ov:' + o];
      if (!b) continue;
      b.classList.toggle('is-active', !!state.overlays[o]);
      b.setAttribute('aria-pressed', String(!!state.overlays[o]));
    }
    const hasBones = !!(world && world.skeleton);
    for (const o of ['skeleton', 'labels']) if (buttons['ov:' + o]) buttons['ov:' + o].disabled = !hasBones;
    if (buttons['mode:weights']) buttons['mode:weights'].disabled = !(world && world.skin.some(Boolean));
  }

  // --- three bootstrap -----------------------------------------------------------
  const ready = (async () => {
    buildToolbar();
    showMessage('Loading 3D viewer…');
    try {
      ({ THREE, OrbitControls, viaMap } = await loadThree());
    } catch (e) {
      showMessage(`3D viewer unavailable: could not load three.js from ${CDN} (${e && e.message ? e.message : e}).`, true);
      if (opts.onError) opts.onError(e);
      throw e;
    }
    if (state.disposed) return;

    renderer = new THREE.WebGLRenderer({ antialias: true, alpha: false, preserveDrawingBuffer: true });
    renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));
    renderer.outputColorSpace = THREE.SRGBColorSpace;
    renderer.domElement.className = 'av-canvas';
    dom.stage.append(renderer.domElement);

    scene = new THREE.Scene();
    applyTheme();

    camera = new THREE.PerspectiveCamera(40, 1, 0.01, 1000);
    camera.position.set(0, 1, 3);

    controls = new OrbitControls(camera, renderer.domElement);
    controls.enableDamping = true;
    controls.dampingFactor = 0.08;
    controls.autoRotateSpeed = 1.5;
    controls.screenSpacePanning = true;

    scene.add(new THREE.HemisphereLight(0xffffff, 0x3a4048, 1.6));
    const key = new THREE.DirectionalLight(0xffffff, 1.4);
    key.position.set(1.5, 3, 2.5);
    scene.add(key);
    const fill = new THREE.DirectionalLight(0xbfd8ff, 0.5);
    fill.position.set(-2, 1, -2);
    scene.add(fill);

    raycaster = new THREE.Raycaster();
    raycaster.params.Line = { threshold: 0 };

    checker = makeCheckerTexture(THREE);
    sharedMats = {
      wireframe: new THREE.MeshBasicMaterial({ wireframe: true, color: 0x9fd6cc }),
      uv: new THREE.MeshLambertMaterial({ map: checker, side: THREE.DoubleSide }),
      normals: new THREE.MeshNormalMaterial({ side: THREE.DoubleSide }),
      weights: new THREE.MeshLambertMaterial({ vertexColors: true, side: THREE.DoubleSide }),
    };

    resizeObs = new ResizeObserver(() => resize());
    resizeObs.observe(container);
    resize();

    renderer.domElement.addEventListener('pointermove', onPointerMove);
    renderer.domElement.addEventListener('pointerleave', () => setHover(null));
    renderer.domElement.addEventListener('click', onClick);
    document.addEventListener('visibilitychange', onVisibility);

    showMessage('Drop an FBX to preview it here.');
    updateStats();
    startLoop();
    if (opts.onReady) opts.onReady();
  })();
  ready.catch(() => {});

  function applyTheme() {
    if (!scene) return;
    scene.background = new THREE.Color(cssVar('--bg-code', '#0a0e15'));
  }

  function resize() {
    if (!renderer) return;
    const w = Math.max(1, container.clientWidth), h = Math.max(1, dom.stage.clientHeight || container.clientHeight);
    renderer.setSize(w, h, false);
    camera.aspect = w / h;
    camera.updateProjectionMatrix();
  }

  function onVisibility() {
    if (document.hidden) stopLoop(); else startLoop();
  }

  function startLoop() {
    if (loopRunning || state.disposed) return;
    loopRunning = true;
    const tick = () => {
      if (!loopRunning) return;
      rafId = requestAnimationFrame(tick);
      controls.update();
      renderer.render(scene, camera);
    };
    rafId = requestAnimationFrame(tick);
  }
  function stopLoop() {
    loopRunning = false;
    if (rafId) cancelAnimationFrame(rafId);
    rafId = 0;
  }

  // --- scene construction --------------------------------------------------------
  function freeWorld() {
    if (!world) return;
    world.dead = true;   // in-flight texture decodes for it must not touch disposed materials
    stopHover();
    scene.remove(world.group);
    for (const g of world.geoms) g.dispose();
    for (const m of world.mats) m.dispose();
    for (const t of world.textures) t.dispose();
    for (const u of world.blobUrls) URL.revokeObjectURL(u);
    if (world.skeleton) {
      world.skeleton.lines.geometry.dispose(); world.skeleton.lines.material.dispose();
      world.skeleton.joints.geometry.dispose(); world.skeleton.joints.material.dispose();
      for (const l of world.skeleton.labels) { l.material.map.dispose(); l.material.dispose(); }
    }
    if (world.grid) { world.grid.geometry.dispose(); world.grid.material.dispose(); }
    world = null;
    state.loaded = false;
    state.selectedBone = null;
  }

  function stopHover() { setHover(null); }

  function boundsOf(manifest, positionsList) {
    const b = manifest && manifest.bounds;
    const ok = b && Array.isArray(b.min) && Array.isArray(b.max) && b.min.every(Number.isFinite) && b.max.every(Number.isFinite)
      && (b.max[0] > b.min[0] || b.max[1] > b.min[1] || b.max[2] > b.min[2]);
    if (ok) return { min: b.min.slice(), max: b.max.slice() };
    const min = [Infinity, Infinity, Infinity], max = [-Infinity, -Infinity, -Infinity];
    for (const p of positionsList) {
      for (let i = 0; i + 2 < p.length; i += 3) {
        for (let k = 0; k < 3; k++) { const v = p[i + k]; if (v < min[k]) min[k] = v; if (v > max[k]) max[k] = v; }
      }
    }
    if (!Number.isFinite(min[0])) return { min: [-1, -1, -1], max: [1, 1, 1] };
    return { min, max };
  }

  // --- textures ------------------------------------------------------------------
  // A material's texture comes from one of two places: bytes embedded in the FBX
  // (`sceneView.texture(i)`) or a file the page handed us through a texture
  // library (`library.find(relative, absolute)` → { name, blob } | null) — MMD /
  // Blender exports reference their images by relative path and ship them next
  // to the .fbx, so the page lets the user drop those files in after the fact.

  const IMAGE_MIME = { png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', bmp: 'image/bmp', gif: 'image/gif', webp: 'image/webp', tga: 'image/x-tga' };

  function extOf(name) { const m = /\.([a-z0-9]+)$/i.exec(name || ''); return m ? m[1].toLowerCase() : ''; }

  function texturePath(tinfo) { return (tinfo && (tinfo.relative || tinfo.absolute)) || ''; }

  // Decode image bytes into a three texture; also report alpha use + a thumbnail.
  async function decodeTexture(bytes, mime, name, w) {
    const isTga = mime === 'image/x-tga' || mime === 'image/tga' || extOf(name) === 'tga';
    const url = URL.createObjectURL(new Blob([bytes], { type: isTga ? 'application/octet-stream' : (mime || 'application/octet-stream') }));
    w.blobUrls.push(url);
    let tex;
    if (isTga) {
      const TGALoader = await loadTgaLoader(viaMap);
      tex = await new TGALoader().loadAsync(url);
    } else {
      tex = await new THREE.TextureLoader().loadAsync(url);
    }
    tex.colorSpace = THREE.SRGBColorSpace;
    tex.wrapS = tex.wrapT = THREE.RepeatWrapping;
    tex.anisotropy = Math.min(8, renderer.capabilities.getMaxAnisotropy());
    w.textures.push(tex);
    const meta = inspectImage(tex.image);
    return { tex, ...meta };
  }

  // Sample the decoded image on a small canvas: does it use alpha, and a thumbnail data URL.
  function inspectImage(image) {
    const out = { width: image && image.width || 0, height: image && image.height || 0, alpha: false, thumb: null };
    try {
      const size = 96;
      const c = document.createElement('canvas');
      c.width = c.height = size;
      const g = c.getContext('2d');
      if (image && image.data && image.width && image.height) {
        // TGALoader → { data: RGBA bytes, width, height } (rows bottom-up per TGA, flipped by the loader).
        const src = document.createElement('canvas');
        src.width = image.width; src.height = image.height;
        const id = new ImageData(new Uint8ClampedArray(image.data.buffer, image.data.byteOffset, image.width * image.height * 4), image.width, image.height);
        src.getContext('2d').putImageData(id, 0, 0);
        g.drawImage(src, 0, 0, size, size);
      } else if (image && image.width) {
        g.drawImage(image, 0, 0, size, size);
      } else {
        return out;
      }
      const px = g.getImageData(0, 0, size, size).data;
      for (let i = 3; i < px.length; i += 4) { if (px[i] < 250) { out.alpha = true; break; } }
      out.thumb = c.toDataURL('image/png');
    } catch (e) { /* tainted canvas or decode quirk: no thumbnail */ }
    return out;
  }

  // Resolve where a material's texture bytes come from. Returns null if nowhere (yet).
  function resolveTextureSource(sceneView, matIndex, tinfo, matName) {
    if (!tinfo) return null;
    if (tinfo.embedded) {
      let bytes = null;
      try { bytes = sceneView.texture(matIndex); } catch (e) { bytes = null; }
      if (bytes && bytes.length) return { kind: 'embedded', bytes, mime: tinfo.mime, name: texturePath(tinfo), source: 'embedded in the FBX' };
    }
    const lib = state.library;
    if (lib && typeof lib.find === 'function') {
      const hit = lib.find(tinfo.relative || null, tinfo.absolute || null, matName || null);
      if (hit && hit.blob) return { kind: 'file', blob: hit.blob, mime: hit.blob.type || IMAGE_MIME[extOf(hit.name)] || null, name: hit.name, source: hit.via ? `${hit.name} (via ${hit.via})` : hit.name };
    }
    return null;
  }

  // `w` is the scene the entry belongs to — during the initial build it is not yet `world`.
  async function attachTexture(entry, w) {
    // entry: { index, info, mat, kind, ... } from w.materials
    if (!w || w.dead || !entry || entry.loading || entry.kind === 'embedded' || entry.kind === 'file' || !entry.info.texture) return false;
    const src = resolveTextureSource(entry.sceneView, entry.index, entry.info.texture, entry.info.name);
    if (!src) { entry.kind = 'missing'; return false; }
    entry.loading = true;
    try {
      const bytes = src.bytes || new Uint8Array(await src.blob.arrayBuffer());
      if (state.disposed || w.dead) return false;
      const r = await decodeTexture(bytes, src.mime, src.name, w);
      if (state.disposed || w.dead) return false;
      const m = entry.mat;
      m.map = r.tex;
      if (r.alpha) { m.transparent = true; m.alphaTest = 0.02; m.depthWrite = true; }
      m.needsUpdate = true;
      Object.assign(entry, { kind: src.kind, source: src.source, width: r.width, height: r.height, alpha: r.alpha, thumb: r.thumb, bytes: bytes.length, error: null });
      return true;
    } catch (e) {
      console.warn('viewer: texture for material', entry.index, 'failed to decode:', e);
      entry.kind = 'error'; entry.error = String(e && e.message ? e.message : e); entry.source = src.source;
      return false;
    } finally { entry.loading = false; }
  }

  function textureStatus() {
    if (!world) return [];
    return world.materials.map(e => ({
      index: e.index, name: e.info.name || `material ${e.index}`, path: texturePath(e.info.texture),
      kind: e.kind, source: e.source || null, width: e.width || 0, height: e.height || 0,
      alpha: !!e.alpha, thumb: e.thumb || null, bytes: e.bytes || 0, error: e.error || null,
    }));
  }

  function notifyTextures() {
    if (state.disposed || !world) return;
    const st = textureStatus();
    if (typeof api.onTextures === 'function') { try { api.onTextures(st); } catch (e) { console.warn(e); } }
    if (opts.onTextures) { try { opts.onTextures(st); } catch (e) { console.warn(e); } }
  }

  async function buildMeshes(sceneView, manifest, w) {
    const meshes = manifest.meshes || [];
    const materials = manifest.materials || [];
    const matCache = new Map();       // material index → MeshStandardMaterial
    const texJobs = [];

    const materialFor = (mi, fallbackColor) => {
      if (mi == null || !materials[mi]) {
        const m = new THREE.MeshStandardMaterial({ color: fallbackColor, roughness: 0.85, metalness: 0, side: THREE.DoubleSide });
        w.mats.push(m);
        return m;
      }
      if (matCache.has(mi)) return matCache.get(mi);
      const info = materials[mi];
      const m = new THREE.MeshStandardMaterial({ roughness: 0.85, metalness: 0, side: THREE.DoubleSide });
      const dc = info.diffuse_color;
      if (Array.isArray(dc) && dc.length >= 3) {
        m.color.setRGB(dc[0], dc[1], dc[2], THREE.SRGBColorSpace);
        if (dc.length > 3 && dc[3] < 0.999) { m.transparent = true; m.opacity = dc[3]; }
      }
      m.name = info.name || `material ${mi}`;
      matCache.set(mi, m);
      w.mats.push(m);
      return m;
    };
    // One status entry per manifest material (also the ones no mesh uses), attached lazily.
    w.materials = materials.map((info, mi) => ({
      index: mi, info, sceneView, mat: materialFor(mi, MESH_PALETTE[mi % MESH_PALETTE.length]),
      kind: info.texture ? 'missing' : 'none', source: null, loading: false,
    }));

    const positionsList = [];
    let totalVerts = 0, totalTris = 0, groupCount = 0;
    const usedMaterials = new Set();

    for (const info of meshes) {
      const mi = info.index;
      let pos, nrm, uv, idx, triMat, sIdx, sWgt;
      try {
        pos = sceneView.positions(mi);
        nrm = sceneView.normals(mi);
        uv = sceneView.uvs(mi);
        idx = sceneView.indices(mi);
        triMat = sceneView.triangle_materials(mi);
        sIdx = sceneView.skin_indices(mi);
        sWgt = sceneView.skin_weights(mi);
      } catch (e) {
        console.warn('viewer: mesh', mi, 'skipped:', e);
        continue;
      }
      const nv = Math.floor(pos.length / 3);
      if (nv === 0 || idx.length < 3) continue;
      positionsList.push(pos);
      const hasSkin = !!(info.skinned && sIdx.length >= nv * 4 && sWgt.length >= nv * 4);
      w.skin[mi] = hasSkin ? { idx: sIdx, wgt: sWgt, count: nv } : null;

      const posAttr = new THREE.BufferAttribute(pos, 3);
      const nrmAttr = nrm.length >= nv * 3 ? new THREE.BufferAttribute(nrm, 3) : null;
      const uvAttr = uv.length >= nv * 2 ? new THREE.BufferAttribute(uv, 2) : null;
      const colorArr = new Float32Array(nv * 3);
      const colorAttr = new THREE.BufferAttribute(colorArr, 3);

      const ntris = Math.floor(idx.length / 3);
      totalVerts += nv;
      totalTris += ntris;

      // Split the index by material slot.
      const slots = Array.isArray(info.material_slots) ? info.material_slots : [];
      const perSlot = new Map();
      if (triMat.length >= ntris && slots.length > 1) {
        const counts = new Map();
        for (let t = 0; t < ntris; t++) { const s = triMat[t]; counts.set(s, (counts.get(s) || 0) + 1); }
        for (const [s, c] of counts) perSlot.set(s, { arr: new Uint32Array(c * 3), n: 0 });
        for (let t = 0; t < ntris; t++) {
          const b = perSlot.get(triMat[t]);
          b.arr[b.n++] = idx[t * 3]; b.arr[b.n++] = idx[t * 3 + 1]; b.arr[b.n++] = idx[t * 3 + 2];
        }
      } else {
        perSlot.set(0, { arr: idx.length % 3 === 0 ? idx : idx.subarray(0, ntris * 3), n: ntris * 3 });
      }

      for (const [slot, b] of perSlot) {
        const g = new THREE.BufferGeometry();
        g.setAttribute('position', posAttr);
        if (nrmAttr) g.setAttribute('normal', nrmAttr);
        if (uvAttr) g.setAttribute('uv', uvAttr);
        g.setAttribute('color', colorAttr);
        g.setIndex(new THREE.BufferAttribute(b.arr, 1));
        if (!nrmAttr) g.computeVertexNormals();
        w.geoms.push(g);
        const matIndex = slots.length ? slots[Math.min(slot, slots.length - 1)] : null;
        if (matIndex != null && materials[matIndex]) usedMaterials.add(matIndex);
        const mat = materialFor(matIndex, MESH_PALETTE[mi % MESH_PALETTE.length]);
        const mesh = new THREE.Mesh(g, mat);
        mesh.name = `${info.name || 'mesh ' + mi}${perSlot.size > 1 ? ' [slot ' + slot + ']' : ''}`;
        mesh.userData = { meshIndex: mi, slot, matIndex };
        mesh.frustumCulled = true;
        w.group.add(mesh);
        w.meshes.push({ mesh, meshIndex: mi, shadedMat: mat, colorAttr, colorArr, nv });
        groupCount++;
      }
    }

    w.bounds = boundsOf(manifest, positionsList);
    state.stats = {
      vertices: totalVerts, triangles: totalTris, meshes: meshes.length,
      groups: groupCount, materials: usedMaterials.size || materials.length,
      bones: (manifest.bones || []).length,
    };
    // Textures decode in the background; the scene is usable immediately.
    texJobs.push(...w.materials.map(e => attachTexture(e, w)));
    Promise.all(texJobs).then(() => { if (world === w) notifyTextures(); }).catch(() => {});
  }

  function buildSkeleton(manifest, w) {
    const bones = (manifest.bones || []).filter(b => b && Array.isArray(b.position));
    if (!bones.length) return;
    const byIndex = new Map(bones.map(b => [b.index, b]));
    const anyBoneLike = bones.some(b => b.bone_like || b.humanoid);
    if (!anyBoneLike) return; // a static prop: no skeleton to draw

    const size = Math.max(w.bounds.max[0] - w.bounds.min[0], w.bounds.max[1] - w.bounds.min[1], w.bounds.max[2] - w.bounds.min[2]) || 1;
    const jointR = size * 0.009;

    // Lines parent → child, only along bone-like / humanoid nodes.
    const drawn = bones.filter(b => b.bone_like || b.humanoid);
    const lp = [], lc = [];
    const col = new THREE.Color();
    for (const b of drawn) {
      const p = b.parent != null ? byIndex.get(b.parent) : null;
      if (!p || !(p.bone_like || p.humanoid)) continue;
      col.set(BONE_COLORS[boneFamily(b)]);
      lp.push(p.position[0], p.position[1], p.position[2], b.position[0], b.position[1], b.position[2]);
      lc.push(col.r, col.g, col.b, col.r, col.g, col.b);
    }
    const lg = new THREE.BufferGeometry();
    lg.setAttribute('position', new THREE.Float32BufferAttribute(lp, 3));
    lg.setAttribute('color', new THREE.Float32BufferAttribute(lc, 3));
    const lines = new THREE.LineSegments(lg, new THREE.LineBasicMaterial({ vertexColors: true, depthTest: false, transparent: true, opacity: 0.95 }));
    lines.renderOrder = 999;

    // Joints as one instanced sphere mesh (raycastable via instanceId).
    const jg = new THREE.SphereGeometry(1, 10, 8);
    const jm = new THREE.MeshBasicMaterial({ depthTest: false, transparent: true, opacity: 0.95 });
    const joints = new THREE.InstancedMesh(jg, jm, drawn.length);
    joints.renderOrder = 1000;
    const m4 = new THREE.Matrix4();
    const jointBones = [];
    drawn.forEach((b, i) => {
      m4.makeScale(jointR, jointR, jointR).setPosition(b.position[0], b.position[1], b.position[2]);
      joints.setMatrixAt(i, m4);
      joints.setColorAt(i, col.set(BONE_COLORS[boneFamily(b)]));
      jointBones.push(b);
    });
    joints.instanceMatrix.needsUpdate = true;
    if (joints.instanceColor) joints.instanceColor.needsUpdate = true;

    // Labels for humanoid bones only.
    const labels = [];
    const labelH = size * 0.04;
    for (const b of drawn) {
      if (!b.humanoid) continue;
      const sp = makeLabelSprite(THREE, b.humanoid, '#' + col.set(BONE_COLORS[boneFamily(b)]).getHexString());
      sp.position.set(b.position[0] + jointR * 2, b.position[1] + jointR * 2, b.position[2]);
      sp.scale.set(labelH * sp.userData.aspect, labelH, 1);
      sp.visible = false;
      labels.push(sp);
    }
    const labelGroup = new THREE.Group();
    labelGroup.add(...labels);

    const skelGroup = new THREE.Group();
    skelGroup.add(lines, joints, labelGroup);
    w.group.add(skelGroup);
    w.skeleton = { group: skelGroup, lines, joints, jointBones, labels, labelGroup, jointR, bones, byIndex };
  }

  function buildGrid(w) {
    const b = w.bounds;
    const sx = b.max[0] - b.min[0], sz = b.max[2] - b.min[2], sy = b.max[1] - b.min[1];
    const extent = Math.max(sx, sz, sy * 0.6, 0.1) * 2.2;
    const div = 20;
    const grid = new THREE.GridHelper(extent, div, new THREE.Color(cssVar('--border-hi', '#2a323e')), new THREE.Color(cssVar('--border', '#1f2630')));
    grid.material.transparent = true;
    grid.material.opacity = 0.7;
    grid.material.depthWrite = false;
    grid.position.set((b.min[0] + b.max[0]) / 2, b.min[1], (b.min[2] + b.max[2]) / 2);
    w.group.add(grid);
    w.grid = grid;
  }

  function frameCamera() {
    if (!world) return;
    const b = world.bounds;
    const cx = (b.min[0] + b.max[0]) / 2, cy = (b.min[1] + b.max[1]) / 2, cz = (b.min[2] + b.max[2]) / 2;
    const sy = b.max[1] - b.min[1], sx = b.max[0] - b.min[0], sz = b.max[2] - b.min[2];
    const radius = Math.max(0.5 * Math.hypot(sx, sy, sz), 1e-3);
    const fov = camera.fov * Math.PI / 180;
    const aspect = camera.aspect || 1;
    const hfov = 2 * Math.atan(Math.tan(fov / 2) * aspect);
    const dist = radius / Math.sin(Math.min(fov, hfov) / 2) * 1.05;
    // Front-facing (+Z after upright), slight elevation.
    camera.position.set(cx + dist * 0.12, cy + dist * 0.22, cz + dist * 0.97);
    camera.near = Math.max(dist / 500, 1e-4);
    camera.far = dist * 50;
    camera.updateProjectionMatrix();
    controls.target.set(cx, cy, cz);
    controls.minDistance = radius * 0.05;
    controls.maxDistance = dist * 10;
    controls.update();
  }

  // --- modes / overlays ----------------------------------------------------------
  function applyMode() {
    if (!world) return;
    for (const m of world.meshes) {
      let mat = m.shadedMat;
      if (state.mode === 'wireframe') mat = sharedMats.wireframe;
      else if (state.mode === 'uv') mat = sharedMats.uv;
      else if (state.mode === 'normals') mat = sharedMats.normals;
      else if (state.mode === 'weights') mat = world.skin[m.meshIndex] ? sharedMats.weights : m.shadedMat;
      m.mesh.material = mat;
    }
    if (state.mode === 'weights') recolorWeights();
  }

  function recolorWeights() {
    if (!world) return;
    const sel = state.selectedBone;
    for (const m of world.meshes) {
      const skin = world.skin[m.meshIndex];
      if (!skin) continue;
      const { idx, wgt } = skin;
      const out = m.colorArr;
      const nv = m.nv;
      if (sel == null) {
        for (let v = 0; v < nv; v++) {
          // dominant influence = top-4 sorted by weight → slot 0
          hashColor(idx[v * 4] + 1, out, v * 3);
        }
      } else {
        for (let v = 0; v < nv; v++) {
          let w = 0;
          const o = v * 4;
          if (idx[o] === sel) w += wgt[o];
          if (idx[o + 1] === sel) w += wgt[o + 1];
          if (idx[o + 2] === sel) w += wgt[o + 2];
          if (idx[o + 3] === sel) w += wgt[o + 3];
          if (w <= 0) { out[v * 3] = 0.16; out[v * 3 + 1] = 0.16; out[v * 3 + 2] = 0.22; }
          else heat(w, out, v * 3);
        }
      }
      m.colorAttr.needsUpdate = true;
    }
  }

  function applyOverlays() {
    if (world) {
      if (world.skeleton) {
        world.skeleton.group.visible = !!state.overlays.skeleton;
        const showLabels = !!(state.overlays.skeleton && state.overlays.labels);
        for (const l of world.skeleton.labels) l.visible = showLabels;
      }
      if (world.grid) world.grid.visible = !!state.overlays.grid;
    }
    if (controls) controls.autoRotate = !!state.overlays.autorotate;
    syncToolbar();
  }

  function applyJointHighlight() {
    if (!world || !world.skeleton) return;
    const sk = world.skeleton;
    const m4 = new THREE.Matrix4();
    const col = new THREE.Color();
    sk.jointBones.forEach((b, i) => {
      const isSel = b.index === state.selectedBone, isHover = b.index === state.hoverBone;
      const r = sk.jointR * (isSel ? 2.2 : isHover ? 1.6 : 1);
      m4.makeScale(r, r, r).setPosition(b.position[0], b.position[1], b.position[2]);
      sk.joints.setMatrixAt(i, m4);
      sk.joints.setColorAt(i, col.set(isSel ? 0xffffff : isHover ? 0xfff2b0 : BONE_COLORS[boneFamily(b)]));
    });
    sk.joints.instanceMatrix.needsUpdate = true;
    if (sk.joints.instanceColor) sk.joints.instanceColor.needsUpdate = true;
  }

  function updateStats() {
    const s = state.stats;
    dom.stats.innerHTML = '';
    if (!s) { dom.stats.hidden = true; return; }
    dom.stats.hidden = false;
    const row = (label, value) => {
      const d = el('span', 'av-stat');
      d.append(el('b', null, value), ' ', label);
      return d;
    };
    dom.stats.append(row('verts', fmt(s.vertices)), row('tris', fmt(s.triangles)), row('meshes', fmt(s.meshes)), row('materials', fmt(s.materials)));
    if (s.bones) dom.stats.append(row('bones', fmt(s.bones)));
    const b = state.selectedBone != null && world && world.skeleton ? world.skeleton.byIndex.get(state.selectedBone) : null;
    if (b) {
      const d = el('span', 'av-stat av-stat-bone');
      d.append('bone: ', el('b', null, b.name || `#${b.index}`), b.humanoid ? ` (${b.humanoid})` : '');
      dom.stats.append(d);
    } else if (state.selectedBone != null) {
      dom.stats.append(el('span', 'av-stat av-stat-bone', `bone: #${state.selectedBone}`));
    }
  }

  // --- picking -------------------------------------------------------------------
  const pointer = { x: 0, y: 0, pending: false, cx: 0, cy: 0 };

  function pickJoint(clientX, clientY) {
    if (!world || !world.skeleton || !state.overlays.skeleton) return null;
    const rect = renderer.domElement.getBoundingClientRect();
    pointer.x = ((clientX - rect.left) / rect.width) * 2 - 1;
    pointer.y = -((clientY - rect.top) / rect.height) * 2 + 1;
    raycaster.setFromCamera(pointer, camera);
    // Generous hit radius: temporarily test against enlarged spheres by using a threshold on distance to ray.
    const sk = world.skeleton;
    const ray = raycaster.ray;
    let best = null, bestD = Infinity;
    const p = new THREE.Vector3();
    const camPos = camera.position;
    for (const b of sk.jointBones) {
      p.set(b.position[0], b.position[1], b.position[2]);
      const distAlong = p.clone().sub(ray.origin).dot(ray.direction);
      if (distAlong <= 0) continue;
      const dToRay = ray.distanceToPoint(p);
      // pick radius scales with distance so joints stay clickable when zoomed out
      const pickR = Math.max(sk.jointR * 2.5, distAlong * 0.012);
      if (dToRay < pickR) {
        const d = p.distanceTo(camPos);
        if (d < bestD) { bestD = d; best = b; }
      }
    }
    return best;
  }

  function onPointerMove(ev) {
    pointer.cx = ev.clientX; pointer.cy = ev.clientY;
    if (pointer.pending) return;
    pointer.pending = true;
    requestAnimationFrame(() => {
      pointer.pending = false;
      if (state.disposed) return;
      const b = pickJoint(pointer.cx, pointer.cy);
      setHover(b, pointer.cx, pointer.cy);
    });
  }

  function setHover(bone, cx, cy) {
    const idx = bone ? bone.index : null;
    if (idx !== state.hoverBone) {
      state.hoverBone = idx;
      applyJointHighlight();
      if (renderer) renderer.domElement.style.cursor = bone ? 'pointer' : '';
    }
    if (!bone) { dom.tooltip.hidden = true; return; }
    const rect = container.getBoundingClientRect();
    dom.tooltip.innerHTML = '';
    dom.tooltip.append(el('b', null, bone.name || `#${bone.index}`));
    dom.tooltip.append(el('span', 'av-tooltip-slot', bone.humanoid ? bone.humanoid : bone.bone_like ? 'unmapped bone' : 'node'));
    if (bone.influenced_vertices) dom.tooltip.append(el('span', 'av-tooltip-dim', `${fmt(bone.influenced_vertices)} verts`));
    dom.tooltip.hidden = false;
    dom.tooltip.style.left = `${cx - rect.left + 12}px`;
    dom.tooltip.style.top = `${cy - rect.top + 12}px`;
  }

  function onClick(ev) {
    const b = pickJoint(ev.clientX, ev.clientY);
    const idx = b ? b.index : null;
    if (idx === null && state.selectedBone === null) return;
    selectBone(idx === state.selectedBone ? null : idx, true);
  }

  function selectBone(index, emit) {
    state.selectedBone = index == null ? null : Number(index);
    applyJointHighlight();
    if (state.mode === 'weights') recolorWeights();
    updateStats();
    if (emit && opts.onBoneSelect) {
      const bone = world && world.skeleton ? world.skeleton.byIndex.get(state.selectedBone) || null : null;
      opts.onBoneSelect(state.selectedBone, bone);
    }
  }

  // --- public API ----------------------------------------------------------------
  const api = {
    ready,
    get element() { return container; },

    async load(sceneView, manifest, loadOpts) {
      await ready;
      if (state.disposed) return;
      if (loadOpts && loadOpts.textureLibrary !== undefined) state.library = loadOpts.textureLibrary;
      if (typeof manifest === 'string') manifest = JSON.parse(manifest);
      if (!manifest && sceneView && typeof sceneView.manifest === 'function') manifest = JSON.parse(sceneView.manifest());
      freeWorld();
      showMessage('');
      const w = { group: new THREE.Group(), meshes: [], geoms: [], mats: [], materials: [], textures: [], blobUrls: [], skin: [], skeleton: null, grid: null, bounds: null, manifest };
      try {
        await buildMeshes(sceneView, manifest, w);
        buildSkeleton(manifest, w);
        buildGrid(w);
      } catch (e) {
        showMessage(`Could not build the preview: ${e && e.message ? e.message : e}`, true);
        if (opts.onError) opts.onError(e);
        throw e;
      }
      world = w;
      scene.add(w.group);
      state.loaded = true;
      if (!w.meshes.length) showMessage('This file has no triangle geometry to draw.', false);
      applyMode();
      applyOverlays();
      applyJointHighlight();
      updateStats();
      frameCamera();
      startLoop();
      return state.stats;
    },

    setMode(mode) {
      if (!MODES.includes(mode)) throw new Error(`viewer.setMode: unknown mode "${mode}"`);
      state.mode = mode;
      applyMode();
      syncToolbar();
    },

    setOverlay(name, on) {
      if (!OVERLAYS.includes(name)) throw new Error(`viewer.setOverlay: unknown overlay "${name}"`);
      state.overlays[name] = !!on;
      applyOverlays();
    },

    highlightBone(index) {
      selectBone(index, false);
    },

    resetView() { frameCamera(); },

    /** Supply (or replace) the texture library and (re)try every unresolved material.
     *  Resolves to the texture status list once decoding settles. */
    async applyTextures(library) {
      if (library !== undefined) state.library = library;
      if (!world) return [];
      const w = world;
      await Promise.all(w.materials.map(e => attachTexture(e, w)));
      if (world === w) notifyTextures();
      return textureStatus();
    },

    /** Per-material texture status: { index, name, path, kind: embedded|file|missing|none|error, source, width, height, alpha, thumb, bytes, error }. */
    textureStatus() { return textureStatus(); },

    /** Set by the host page: called with the status list whenever textures settle. */
    onTextures: null,

    screenshot() {
      if (!renderer) return null;
      renderer.render(scene, camera);
      return renderer.domElement.toDataURL('image/png');
    },

    getState() {
      return { mode: state.mode, overlays: { ...state.overlays }, selectedBone: state.selectedBone, stats: state.stats ? { ...state.stats } : null, loaded: state.loaded };
    },

    dispose() {
      if (state.disposed) return;
      state.disposed = true;
      stopLoop();
      freeWorld();
      document.removeEventListener('visibilitychange', onVisibility);
      if (resizeObs) resizeObs.disconnect();
      if (controls) controls.dispose();
      if (sharedMats) for (const m of Object.values(sharedMats)) m.dispose();
      if (checker) checker.dispose();
      if (renderer) { renderer.dispose(); renderer.domElement.remove(); }
      container.innerHTML = '';
      container.classList.remove('av-viewer');
    },
  };
  return api;
}

export default createViewer;
