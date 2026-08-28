/* Analyzer page: drag-and-drop FBX inspection, fully client-side.
 *
 * Loads the wasm bundle wasm-pack builds from crates/web-analyzer (the same
 * diagnose graph the CLI uses: avatar-fbx + avatar-armature + avatar-stats)
 * and renders its JSON report as a tabbed inspector, plus a 3D preview via
 * the sibling viewer module (js/viewer.js, three.js). The dropped file is
 * read into memory and handed to wasm — it never leaves the browser.
 *
 * wasm surface used (see crates/web-analyzer):
 *   analyze_fbx(bytes, name) -> JSON report string
 *   sample_fbx()             -> Uint8Array (the testkit synthetic humanoid)
 *   SceneView.load(bytes)    -> { manifest(), positions(i), …, texture(i), free() }
 * Viewer surface used (js/viewer.js):
 *   createViewer(el) -> { load(sceneView, manifest), highlightBone(i|null),
 *                         onBoneSelect, setMode, setOverlay, screenshot(), dispose() }
 * The report renders even if the viewer module or SceneView fails.
 *
 * The bundle is a build artifact: CI builds it on every Pages deploy. For a
 * local preview run
 *   wasm-pack build crates/web-analyzer --target web --release --out-dir ../../site/wasm
 * and serve the site over http (module + wasm loading does not work file://):
 *   python3 -m http.server -d site
 */

// ---------------------------------------------------------------------------
// Static tables
// ---------------------------------------------------------------------------

const RANKS = {
  Excellent: { label: 'Excellent', cls: 'rank-excellent' },
  Good:      { label: 'Good',      cls: 'rank-good' },
  Medium:    { label: 'Medium',    cls: 'rank-medium' },
  Poor:      { label: 'Poor',      cls: 'rank-poor' },
  VeryPoor:  { label: 'Very Poor', cls: 'rank-verypoor' },
};
const RANK_ORDER = ['Excellent', 'Good', 'Medium', 'Poor', 'VeryPoor'];

// VRChat performance-rank limits: the value at or below which a metric earns
// each tier (Excellent / Good / Medium / Poor; above Poor = Very Poor).
// Transcribed from docs/reference/performance-stats.md ("PC (Windows)" and
// "Android / Quest" tables) — keep in sync with `Metric::defs` in
// crates/stats/src/lib.rs. `null` = not ranked on that platform.
const THRESHOLDS = {
  'Triangles':                      { pc: [32000, 70000, 70000, 70000], android: [7500, 10000, 15000, 20000] },
  'Skinned Meshes':                 { pc: [1, 2, 8, 16],                android: [1, 1, 2, 2] },
  'Basic Meshes':                   { pc: [4, 8, 16, 24],               android: [1, 1, 2, 2] },
  'Material Slots':                 { pc: [4, 8, 16, 32],               android: [1, 1, 2, 4] },
  'Bones':                          { pc: [75, 150, 256, 400],          android: [75, 90, 150, 150] },
  'PhysBone Components':            { pc: [4, 8, 16, 32],               android: [0, 4, 6, 8] },
  'PhysBone Colliders':             { pc: [4, 8, 16, 32],               android: [0, 4, 8, 16] },
  'PhysBone Affected Transforms':   { pc: [16, 64, 128, 256],           android: [0, 16, 32, 64] },
  'PhysBone Collision Check Count': { pc: [32, 128, 256, 512],          android: [0, 16, 32, 64] },
  'Contacts':                       { pc: [8, 16, 24, 32],              android: [2, 4, 8, 16] },
  'Particle Systems':               { pc: [0, 4, 8, 16],                android: [0, 0, 0, 2] },
  'Total Particles':                { pc: [0, 300, 1000, 2500],         android: [0, 0, 0, 200] },
  'Mesh Particle Polygons':         { pc: [0, 2000, 20000, 50000],      android: [0, 0, 2000, 20000] },
  'Particle Trails':                { pc: [0, 0, 0, 1],                 android: [0, 0, 0, 1] },
  'Particle Collision':             { pc: [0, 0, 0, 1],                 android: [0, 0, 0, 1] },
  'Constraints':                    { pc: [100, 250, 300, 350],         android: [30, 60, 120, 150] },
  'Constraint Depth':               { pc: [20, 50, 80, 100],            android: [5, 15, 35, 50] },
  'Lights':                         { pc: [0, 0, 0, 1],                 android: null },
  'Audio Sources':                  { pc: [1, 4, 8, 8],                 android: null },
  'Trail Renderers':                { pc: [1, 2, 4, 8],                 android: [0, 0, 0, 1] },
  'Line Renderers':                 { pc: [1, 2, 4, 8],                 android: [0, 0, 0, 1] },
  'Cloths':                         { pc: [0, 1, 1, 1],                 android: null },
  'Physics Colliders':              { pc: [0, 1, 8, 8],                 android: null },
  'Physics Rigidbodies':            { pc: [0, 1, 8, 8],                 android: null },
  'Animators':                      { pc: [1, 4, 16, 32],               android: [1, 1, 1, 2] },
};

// The 25 Unity humanoid slots (crates/armature/src/humanoid.rs `HumanBone::ALL`),
// grouped for the checklist. Fingers have no slots — they are detected by name.
const HUMANOID_GROUPS = [
  { title: 'Body', slots: ['Hips', 'Spine', 'Chest', 'UpperChest'] },
  { title: 'Head', slots: ['Neck', 'Head', 'LeftEye', 'RightEye', 'Jaw'] },
  { title: 'Arms', slots: ['LeftShoulder', 'LeftUpperArm', 'LeftLowerArm', 'LeftHand',
                           'RightShoulder', 'RightUpperArm', 'RightLowerArm', 'RightHand'] },
  { title: 'Legs', slots: ['LeftUpperLeg', 'LeftLowerLeg', 'LeftFoot', 'LeftToes',
                           'RightUpperLeg', 'RightLowerLeg', 'RightFoot', 'RightToes'] },
];
const REQUIRED = new Set(['Hips', 'Spine', 'Head', 'LeftUpperArm', 'RightUpperArm', 'LeftLowerArm',
  'RightLowerArm', 'LeftHand', 'RightHand', 'LeftUpperLeg', 'RightUpperLeg', 'LeftLowerLeg',
  'RightLowerLeg', 'LeftFoot', 'RightFoot']);
const RECOMMENDED = new Set(['Chest', 'Neck', 'LeftShoulder', 'RightShoulder']);
const FINGERS = ['Thumb', 'Index', 'Middle', 'Ring', 'Little'];
const FINGER_TOKENS = { Thumb: ['thumb'], Index: ['index'], Middle: ['middle'], Ring: ['ring'], Little: ['little', 'pinky'] };

// VRChat's 15 visemes, in SDK order (what `avatar lint` cross-checks against the descriptor).
const VISEMES = ['sil', 'PP', 'FF', 'TH', 'DD', 'kk', 'CH', 'SS', 'nn', 'RR', 'aa', 'E', 'ih', 'oh', 'ou'];

// Bone colour families — must mirror `boneFamily` / BONE_COLORS in js/viewer.js so
// the tree's dots match the skeleton joints in the 3D view (hues live in analyzer.css).
const SPINE_SLOTS = new Set(['Hips', 'Spine', 'Chest', 'UpperChest', 'Neck']);
const HEAD_SLOTS = new Set(['Head', 'Jaw', 'LeftEye', 'RightEye']);

// ---------------------------------------------------------------------------
// Tiny DOM helpers
// ---------------------------------------------------------------------------

function h(tag, attrs, ...children) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs || {})) {
    if (v == null || v === false) continue;
    if (k === 'class') el.className = v;
    else if (k === 'dataset') Object.assign(el.dataset, v);
    else if (k.startsWith('on') && typeof v === 'function') el.addEventListener(k.slice(2), v);
    else el.setAttribute(k, v === true ? '' : v);
  }
  for (const c of children.flat(Infinity)) {
    if (c == null || c === false) continue;
    el.append(c.nodeType ? c : document.createTextNode(String(c)));
  }
  return el;
}
const fmt = n => (typeof n === 'number' ? n.toLocaleString() : String(n ?? '—'));
const fmtBytes = n => n >= 1048576 ? `${(n / 1048576).toFixed(1)} MB` : n >= 1024 ? `${(n / 1024).toFixed(0)} KB` : `${n} B`;

function rankBadge(rank) {
  if (!rank) return h('span', { class: 'rank-badge rank-na' }, 'n/a');
  const r = RANKS[rank] || { label: rank, cls: 'rank-na' };
  return h('span', { class: 'rank-badge ' + r.cls }, r.label);
}
function statChip(label, value, cls) {
  return h('div', { class: 'an-chip' + (cls ? ' ' + cls : '') },
    h('div', { class: 'an-chip-value' }, value),
    h('div', { class: 'an-chip-label' }, label));
}
const yesNo = b => h('span', { class: b ? 'an-yes' : 'an-no' }, b ? 'yes' : 'no');

function boneFamily(node) {
  const hu = node.humanoid;
  if (!hu) return node.bone_like ? 'unmapped' : 'node';
  if (SPINE_SLOTS.has(hu)) return 'spine';
  if (HEAD_SLOTS.has(hu)) return 'head';
  if (hu.startsWith('Left')) return 'left';
  if (hu.startsWith('Right')) return 'right';
  return 'spine';
}
// Left/right from a bone *name* (for finger detection only — fingers have no humanoid slot).
function nameSide(name) {
  const n = name.toLowerCase();
  if (/left|(^|[^a-z])l([^a-z]|$)/.test(n)) return 'left';
  if (/right|(^|[^a-z])r([^a-z]|$)/.test(n)) return 'right';
  return null;
}

// ---------------------------------------------------------------------------
// Tabs (WAI-ARIA tabs pattern; arrow keys, Home/End; #tab=<id> in the URL hash)
// ---------------------------------------------------------------------------

function tabIdFromHash() {
  const m = /(?:^#|&)tab=([a-z]+)/.exec(location.hash);
  return m ? m[1] : null;
}

function makeTabs(defs) {
  const tabs = [], panels = [];
  const list = h('div', { class: 'an-tablist', role: 'tablist', 'aria-label': 'Report sections' });
  const root = h('div', { class: 'an-tabs' }, list);
  const select = (i, focus) => {
    tabs.forEach((t, j) => {
      const on = i === j;
      t.setAttribute('aria-selected', on ? 'true' : 'false');
      t.tabIndex = on ? 0 : -1;
      panels[j].hidden = !on;
    });
    if (focus) tabs[i].focus();
    history.replaceState(null, '', '#tab=' + defs[i].id);
  };
  defs.forEach((d, i) => {
    const tab = h('button', {
      type: 'button', class: 'an-tab', role: 'tab', id: 'an-tab-' + d.id,
      'aria-controls': 'an-panel-' + d.id, 'aria-selected': 'false', tabindex: '-1',
      onclick: () => select(i, false),
    }, d.label, d.count != null
      ? h('span', { class: 'an-tab-count' + (d.warn ? ' an-tab-count-warn' : '') }, fmt(d.count))
      : null);
    const panel = h('div', {
      class: 'an-panel', role: 'tabpanel', id: 'an-panel-' + d.id,
      'aria-labelledby': 'an-tab-' + d.id, tabindex: '0', hidden: true,
    }, d.body);
    tabs.push(tab); panels.push(panel);
    list.append(tab); root.append(panel);
  });
  list.addEventListener('keydown', e => {
    const cur = tabs.indexOf(document.activeElement);
    if (cur < 0) return;
    let next = null;
    if (e.key === 'ArrowRight') next = (cur + 1) % tabs.length;
    else if (e.key === 'ArrowLeft') next = (cur - 1 + tabs.length) % tabs.length;
    else if (e.key === 'Home') next = 0;
    else if (e.key === 'End') next = tabs.length - 1;
    if (next != null) { e.preventDefault(); select(next, true); }
  });
  const wanted = tabIdFromHash();
  const idx = Math.max(0, defs.findIndex(d => d.id === wanted));
  select(idx, false);
  root.selectTab = id => { const i = defs.findIndex(d => d.id === id); if (i >= 0) select(i, false); };
  return root;
}

// ---------------------------------------------------------------------------
// Overview
// ---------------------------------------------------------------------------

const AXIS_NAMES = { 0: 'X', 1: 'Y', 2: 'Z' };

function collectIssues(report) {
  const a = report.armature, s = report.stats, g = report.global_settings || {};
  const meshes = report.meshes || [], materials = report.materials || [];
  const issues = [];
  const push = (sev, title, ...body) => issues.push({ sev, title, body });

  if (a.missing_required.length) {
    push('error', `${a.missing_required.length} required humanoid bone${a.missing_required.length > 1 ? 's' : ''} missing`,
      'Unity will not import this rig as Humanoid, so VRChat cannot use full-body IK, eye look or ',
      'visemes on it. Missing: ', h('code', {}, a.missing_required.join(', ')),
      '. ', h('code', {}, 'avatar armature fix'), ' can rename recognisable bones into place.');
  }
  if (a.missing_recommended.length) {
    push('warn', `${a.missing_recommended.length} recommended bone${a.missing_recommended.length > 1 ? 's' : ''} missing`,
      'The rig still imports as Humanoid, but VRChat recommends mapping ',
      h('code', {}, a.missing_recommended.join(', ')),
      ' for a well-behaved spine and shoulders (arm IK and posture).');
  }
  const dups = Object.entries(a.duplicate_mappings || {});
  if (dups.length) {
    push('warn', `${dups.length} humanoid slot${dups.length > 1 ? 's' : ''} mapped by more than one bone`,
      'Unity picks one and the other becomes a stray bone; usually a duplicated or mis-named bone. ',
      dups.map(([slot, names], i) => [i ? '; ' : '', h('strong', {}, slot), ' ← ', h('code', {}, names.join(', '))]));
  }
  if (a.armature_roots.length === 0) {
    push('error', 'No skeleton root found', 'No root Model contains bone-like nodes — the file has meshes but no armature to rig.');
  } else if (a.armature_roots.length > 1) {
    push('warn', `${a.armature_roots.length} skeleton roots`,
      'VRChat expects exactly one armature root; extra roots (', h('code', {}, a.armature_roots.join(', ')),
      ') will not follow the avatar and confuse the humanoid mapper.');
  }
  if (g.unit_scale_factor != null && Math.abs(g.unit_scale_factor - 100) > 1e-6) {
    push('warn', `Unit scale factor is ${g.unit_scale_factor}, not 100`,
      'FBX units are centimetres at scale 100 (Unity\'s and Blender\'s convention). Unity applies a ',
      'file scale of ', h('code', {}, (g.unit_scale_factor / 100).toPrecision(4)),
      ' on import (or 0.01 with "convert units" off) — the avatar imports at the wrong size, and VRChat\'s ',
      'eye height / performance checks assume roughly human scale. Re-export with "apply unit scale" or ',
      'fix the import scale factor in Unity.');
  }
  if (g.up_axis != null && g.up_axis !== 1) {
    push('info', `Up axis is ${AXIS_NAMES[g.up_axis] ?? g.up_axis}, not Y`,
      'Unity is Y-up; the importer compensates by rotating the root, which is normally harmless but ',
      'shows up as a 90° rotation on the root transform. The 3D preview above auto-uprights from ',
      'hips→head like ', h('code', {}, 'avatar render'), ', so it may look upright even when Unity will not.');
  }
  if (materials.length === 0 && report.fbx.materials === 0) {
    push('warn', 'No materials in the file',
      'Every mesh will import with Unity\'s default material; you will have to build materials by hand. ',
      'Usually the exporter had "materials" unticked.');
  }
  const unslotted = meshes.filter(m => m.material_slots === 0);
  if (unslotted.length && meshes.length) {
    push('warn', `${unslotted.length} mesh${unslotted.length > 1 ? 'es' : ''} with no material slot`,
      h('code', {}, unslotted.map(m => m.name).join(', ')), ' — renders with the pink "missing material" shader in Unity.');
  }
  const unskinned = meshes.filter(m => !m.skinned);
  if (unskinned.length) {
    push('info', `${unskinned.length} rigid (non-skinned) mesh${unskinned.length > 1 ? 'es' : ''}`,
      h('code', {}, unskinned.map(m => m.name).join(', ')),
      ' — counted as Basic Meshes; they follow their parent bone as a whole rather than deforming.');
  }
  const badPc = RANK_ORDER.indexOf(s.pc_overall) >= RANK_ORDER.indexOf('Poor');
  if (badPc) {
    const worst = s.stats.filter(m => m.pc === s.pc_overall).map(m => m.name);
    push('warn', `PC performance rank is ${RANKS[s.pc_overall]?.label ?? s.pc_overall}`,
      'Driven by ', h('code', {}, worst.join(', ')), '. Very Poor avatars are hidden by default for many users; ',
      'see the Performance tab for how far each metric is over its tier.');
  }
  if (s.android_overall === 'VeryPoor') {
    push('info', 'Not Android/Quest-eligible as-is',
      'Android caps avatars at Poor (a Very Poor avatar cannot be uploaded for Quest). Driven by ',
      h('code', {}, s.stats.filter(m => m.android === 'VeryPoor').map(m => m.name).join(', ')), '.');
  }
  const bs = report.blendshapes || [];
  const visemesFound = new Set(bs.filter(b => b.group === 'viseme').map(b => visemeKey(b.name)).filter(Boolean));
  if (bs.length && visemesFound.size > 0 && visemesFound.size < VISEMES.length) {
    push('info', `${VISEMES.length - visemesFound.size} of the 15 VRChat visemes missing`,
      'The descriptor\'s viseme blendshape mode needs all fifteen; missing: ',
      h('code', {}, VISEMES.filter(v => !visemesFound.has(v.toLowerCase())).map(v => 'vrc.v_' + v).join(', ')),
      '. ', h('code', {}, 'avatar lint'), ' reports this as a viseme↔FBX mismatch.');
  } else if (bs.length && visemesFound.size === 0) {
    push('info', 'No VRChat viseme blendshapes',
      'Lip sync will need jaw-flap or a viseme set authored on the face mesh (vrc.v_sil … vrc.v_ou).');
  }
  return issues;
}

function visemeKey(name) {
  const m = /^(?:vrc\.)?v_?([a-z]+)$/i.exec(name.trim());
  if (!m) {
    const lower = name.toLowerCase();
    return VISEMES.map(v => v.toLowerCase()).find(v => lower === v) || null;
  }
  const k = m[1].toLowerCase();
  return VISEMES.some(v => v.toLowerCase() === k) ? k : null;
}

function renderOverview(report, issues) {
  const a = report.armature, s = report.stats;
  const ready = a.missing_required.length === 0;
  const metric = name => s.stats.find(m => m.name === name);
  const tris = metric('Triangles'), bones = metric('Bones'), slots = metric('Material Slots');
  const out = h('div', {});
  out.append(h('div', { class: 'an-chip-row' },
    statChip('humanoid', h('span', { class: 'rank-badge ' + (ready ? 'rank-excellent' : 'rank-verypoor') },
      ready ? 'ready' : 'not ready'), 'an-chip-hero'),
    statChip('PC rank', rankBadge(s.pc_overall), 'an-chip-hero'),
    statChip('Android rank', rankBadge(s.android_overall), 'an-chip-hero'),
    statChip('triangles', fmt(tris ? tris.value : report.meshes?.reduce((n, m) => n + m.triangles, 0))),
    statChip('bones', fmt(bones ? bones.value : report.fbx.bone_like)),
    statChip('meshes', fmt(report.meshes ? report.meshes.length : report.fbx.geometries)),
    statChip('materials', fmt(report.materials ? report.materials.length : report.fbx.materials)),
    statChip('blendshapes', fmt(report.blendshapes.length))));
  const g = report.global_settings || {};
  out.append(h('p', { class: 'an-fineprint' },
    `Binary FBX ${report.fbx.version} · ${fmt(report.fbx.models)} models, ${fmt(report.fbx.geometries)} geometries, `,
    `${fmt(report.fbx.deformers)} deformers · unit scale ${g.unit_scale_factor ?? '?'} · up axis `,
    `${g.up_axis != null ? AXIS_NAMES[g.up_axis] ?? g.up_axis : '?'}`,
    g.front_axis != null ? ` · front axis ${AXIS_NAMES[g.front_axis] ?? g.front_axis}` : ''));

  out.append(h('h3', {}, 'Issues'));
  if (!issues.length) {
    out.append(h('p', { class: 'an-allclear' }, '✓ Nothing flagged — humanoid-ready, one skeleton root, standard units, every mesh has a material.'));
  } else {
    const icons = { error: '✕', warn: '!', info: 'i' };
    out.append(h('ul', { class: 'an-issues' }, issues.map(i =>
      h('li', { class: 'an-issue-item an-sev-' + i.sev },
        h('span', { class: 'an-issue-icon', 'aria-label': i.sev }, icons[i.sev]),
        h('span', { class: 'an-issue-title' }, i.title),
        h('span', { class: 'an-issue-body' }, i.body)))));
  }
  return out;
}

// ---------------------------------------------------------------------------
// Rig: bone tree + humanoid checklist
// ---------------------------------------------------------------------------

function renderRig(report, ctx) {
  const a = report.armature;
  const nodes = report.bone_tree || [];
  const dupSlots = new Set(Object.keys(a.duplicate_mappings || {}));
  const out = h('div', {});
  const layout = h('div', { class: 'an-rig-layout' });
  out.append(layout);

  // ---- tree ----
  const left = h('div', {});
  const children = new Map();
  const roots = [];
  for (const n of nodes) {
    if (n.parent == null || !nodes.some(p => p.id === n.parent)) roots.push(n);
    else { if (!children.has(n.parent)) children.set(n.parent, []); children.get(n.parent).push(n); }
  }
  // O(n) rebuild of the parent lookup instead of the `some` above for big trees.
  if (nodes.length > 200) {
    roots.length = 0; children.clear();
    const ids = new Set(nodes.map(n => n.id));
    for (const n of nodes) {
      if (n.parent == null || !ids.has(n.parent)) roots.push(n);
      else { if (!children.has(n.parent)) children.set(n.parent, []); children.get(n.parent).push(n); }
    }
  }
  const rowById = new Map();
  ctx.rowById = rowById;
  const buildList = (list, depth) => {
    const ul = h('ul', { role: depth ? 'group' : 'tree' });
    for (const n of list) {
      const kids = children.get(n.id) || [];
      const fam = boneFamily(n);
      const li = h('li', { role: 'treeitem', 'aria-expanded': kids.length ? 'true' : null });
      const toggle = h('button', { type: 'button', class: 'an-tree-toggle' + (kids.length ? '' : ' an-leaf'),
        'aria-label': 'collapse', tabindex: '-1' }, '▾');
      const row = h('div', { class: 'an-tree-row' + (n.bone_like ? '' : ' an-not-bone'), tabindex: '0',
        style: `--depth:${depth}`, dataset: { id: String(n.id) } },
        toggle,
        h('span', { class: 'an-dot an-fam-' + fam, title: n.bone_like ? fam : 'not a bone' }),
        h('span', { class: 'an-tree-name' }, n.name),
        n.humanoid ? h('span', { class: 'an-slot an-fam-' + fam + (dupSlots.has(n.humanoid) ? ' an-slot-dup' : '') }, n.humanoid) : null,
        kids.length ? h('span', { class: 'an-tree-meta' }, kids.length) : null);
      toggle.addEventListener('click', e => {
        e.stopPropagation();
        const c = li.classList.toggle('an-collapsed');
        li.setAttribute('aria-expanded', c ? 'false' : 'true');
        toggle.textContent = c ? '▸' : '▾';
      });
      row.addEventListener('click', () => ctx.select(n.id, 'tree'));
      row.addEventListener('keydown', e => {
        if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); ctx.select(n.id, 'tree'); }
        else if (e.key === 'ArrowLeft' && kids.length && !li.classList.contains('an-collapsed')) toggle.click();
        else if (e.key === 'ArrowRight' && kids.length && li.classList.contains('an-collapsed')) toggle.click();
      });
      rowById.set(n.id, { row, li, node: n });
      li.append(row);
      if (kids.length) li.append(buildList(kids, depth + 1));
      ul.append(li);
    }
    return ul;
  };
  const tree = h('div', { class: 'an-tree' });
  const frag = document.createDocumentFragment();
  frag.append(buildList(roots, 0));
  tree.append(frag);

  const filter = h('input', { type: 'search', placeholder: 'Filter bones…', 'aria-label': 'Filter bones' });
  const setAll = collapsed => {
    for (const { li } of rowById.values()) {
      if (!li.querySelector(':scope > ul')) continue;
      li.classList.toggle('an-collapsed', collapsed);
      li.setAttribute('aria-expanded', collapsed ? 'false' : 'true');
      li.querySelector(':scope > .an-tree-row > .an-tree-toggle').textContent = collapsed ? '▸' : '▾';
    }
  };
  filter.addEventListener('input', () => {
    const q = filter.value.trim().toLowerCase();
    if (!q) { for (const { row } of rowById.values()) row.classList.remove('an-hidden-by-filter'); return; }
    // Show matches and every ancestor of a match; expand everything so they're visible.
    const keep = new Set();
    for (const n of nodes) {
      if (n.name.toLowerCase().includes(q) || (n.humanoid && n.humanoid.toLowerCase().includes(q))) {
        let cur = n;
        while (cur && !keep.has(cur.id)) { keep.add(cur.id); cur = cur.parent != null ? rowById.get(cur.parent)?.node : null; }
      }
    }
    setAll(false);
    for (const [id, { row }] of rowById) row.classList.toggle('an-hidden-by-filter', !keep.has(id));
  });
  left.append(
    h('h3', {}, `Bone tree `, h('span', { class: 'an-tab-count' }, `${nodes.length} nodes · ${a.bone_like_count} bone-like`)),
    h('div', { class: 'an-tree-toolbar' }, filter,
      h('button', { type: 'button', class: 'an-btn an-btn-small', onclick: () => setAll(false) }, 'Expand all'),
      h('button', { type: 'button', class: 'an-btn an-btn-small', onclick: () => setAll(true) }, 'Collapse all'),
      h('button', { type: 'button', class: 'an-btn an-btn-small', onclick: () => {
        setAll(true);
        for (const { li, node } of rowById.values()) if (node.humanoid) {
          let cur = li.parentElement?.closest('li');
          while (cur) { cur.classList.remove('an-collapsed'); cur.setAttribute('aria-expanded', 'true');
            cur.querySelector(':scope > .an-tree-row > .an-tree-toggle').textContent = '▾'; cur = cur.parentElement?.closest('li'); }
        }
      } }, 'Humanoid only')),
    tree,
    h('div', { class: 'an-legend' },
      h('span', {}, h('span', { class: 'an-dot an-fam-spine' }), 'spine'),
      h('span', {}, h('span', { class: 'an-dot an-fam-head' }), 'head'),
      h('span', {}, h('span', { class: 'an-dot an-fam-left' }), 'left'),
      h('span', {}, h('span', { class: 'an-dot an-fam-right' }), 'right'),
      h('span', {}, h('span', { class: 'an-dot an-fam-unmapped' }), 'unmapped bone'),
      h('span', {}, h('span', { class: 'an-dot' }), 'mesh / empty'),
      h('span', {}, 'click a row to highlight it in the 3D view')));
  layout.append(left);

  // ---- right column: checklist + lists ----
  const right = h('div', {});
  const ready = a.missing_required.length === 0;
  right.append(h('h3', {}, 'Humanoid checklist ',
    h('span', { class: 'rank-badge ' + (ready ? 'rank-excellent' : 'rank-verypoor') }, ready ? 'humanoid-ready' : 'not humanoid-ready')));
  const mapped = a.mapped || {};
  const slotToId = new Map();
  for (const n of nodes) if (n.humanoid && !slotToId.has(n.humanoid)) slotToId.set(n.humanoid, n.id);
  const checklist = h('div', { class: 'an-checklist' });
  for (const g of HUMANOID_GROUPS) {
    const grid = h('div', { class: 'an-check-grid' });
    for (const slot of g.slots) {
      const found = !!mapped[slot];
      const req = REQUIRED.has(slot) ? 'required' : RECOMMENDED.has(slot) ? 'recommended' : 'optional';
      const cls = found ? 'an-found' : 'an-missing-' + req;
      const id = slotToId.get(slot);
      const el = h('div', { class: 'an-check ' + cls + (id != null ? ' an-clickable' : ''),
        title: found ? `${slot} ← ${mapped[slot].join(', ')}` : `${slot}: ${req}, not found` },
        h('span', { class: 'an-check-mark', 'aria-hidden': 'true' }, found ? '✓' : req === 'required' ? '✕' : '○'),
        h('span', {}, slot),
        !found ? h('span', { class: 'an-check-req' }, req === 'required' ? 'req' : req === 'recommended' ? 'rec' : 'opt') : null);
      if (id != null) el.addEventListener('click', () => ctx.select(id, 'tree'));
      grid.append(el);
    }
    checklist.append(h('div', { class: 'an-check-group' }, h('div', { class: 'an-check-group-title' }, g.title), grid));
  }
  // Fingers: no humanoid slots — detected from bone names per side.
  const fingerGrid = h('div', { class: 'an-check-grid' });
  const lowerNames = nodes.filter(n => n.bone_like).map(n => ({ n, l: n.name.toLowerCase(), side: nameSide(n.name) }));
  for (const side of ['left', 'right']) for (const f of FINGERS) {
    const hit = lowerNames.find(x => x.side === side && FINGER_TOKENS[f].some(t => x.l.includes(t)));
    const label = (side === 'left' ? 'Left' : 'Right') + f;
    const el = h('div', { class: 'an-check ' + (hit ? 'an-found an-clickable' : 'an-missing-optional'), title: hit ? `${label} ← ${hit.n.name}` : `${label}: no bone named like it` },
      h('span', { class: 'an-check-mark', 'aria-hidden': 'true' }, hit ? '✓' : '○'), h('span', {}, label));
    if (hit) el.addEventListener('click', () => ctx.select(hit.n.id, 'tree'));
    fingerGrid.append(el);
  }
  checklist.append(h('div', { class: 'an-check-group' },
    h('div', { class: 'an-check-group-title' }, `Fingers (by name · ${a.ignored_finger_bones} finger bones recognised)`), fingerGrid));
  right.append(checklist);

  const list = (title, items, cls) => items.length
    ? h('div', { class: 'an-issue' }, h('strong', { class: cls }, title + ': '), h('code', {}, items.join(', ')))
    : null;
  right.append(h('div', { style: 'margin-top:0.8rem' },
    list('Missing required', a.missing_required, 'an-missing-required'),
    list('Missing recommended', a.missing_recommended),
    Object.keys(a.duplicate_mappings || {}).length
      ? h('div', { class: 'an-issue' }, h('strong', {}, 'Duplicate mappings: '),
          Object.entries(a.duplicate_mappings).map(([s, n], i) => [i ? '; ' : '', s, ' ← ', h('code', {}, n.join(', '))]))
      : null,
    a.armature_roots.length !== 1
      ? h('div', { class: 'an-issue' }, h('strong', {}, `Skeleton roots (${a.armature_roots.length}): `), h('code', {}, a.armature_roots.join(', ') || 'none'))
      : h('div', { class: 'an-issue' }, h('strong', {}, 'Skeleton root: '), h('code', {}, a.armature_roots[0])),
    a.mesh_roots?.length ? h('div', { class: 'an-issue' }, h('strong', {}, 'Mesh roots: '), h('code', {}, a.mesh_roots.join(', '))) : null,
    a.unmapped_bones.length
      ? h('details', { class: 'an-details' },
          h('summary', {}, `Unmapped bone-like nodes (${a.unmapped_bones.length}) — accessory / twist / dynamic bones`),
          h('p', {}, h('code', {}, a.unmapped_bones.join(', '))))
      : null,
    h('p', { class: 'an-fineprint' },
      `${a.ignored_finger_bones} finger and ${a.ignored_leaf_bones} leaf *_End bones recognised and excluded from body mapping.`)));
  layout.append(right);
  return out;
}

// ---------------------------------------------------------------------------
// Performance meters
// ---------------------------------------------------------------------------

function rankFor(value, limits) {
  for (let i = 0; i < 4; i++) if (value <= limits[i]) return RANK_ORDER[i];
  return 'VeryPoor';
}

function meter(value, limits, rank) {
  // Scale: show all four thresholds plus headroom, and always the value itself.
  const poor = limits[3];
  const max = Math.max(poor * 1.25, value * 1.08, 1);
  const pct = v => Math.min(100, (v / max) * 100);
  const track = h('div', { class: 'an-meter', role: 'img',
    'aria-label': `${fmt(value)} — ${RANKS[rank]?.label ?? 'n/a'}; limits Excellent ≤ ${fmt(limits[0])}, Good ≤ ${fmt(limits[1])}, Medium ≤ ${fmt(limits[2])}, Poor ≤ ${fmt(limits[3])}` });
  track.append(h('div', { class: 'an-meter-track' }));
  track.append(h('div', { class: 'an-meter-fill ' + (RANKS[rank]?.cls || 'rank-na'), style: `width:${pct(value)}%` }));
  // Ticks: merge equal thresholds into one labelled tick ("Good / Medium / Poor").
  const groups = [];
  limits.forEach((v, i) => {
    const last = groups[groups.length - 1];
    if (last && last.v === v) last.names.push(RANK_ORDER[i]);
    else groups.push({ v, names: [RANK_ORDER[i]], rank: RANK_ORDER[i].toLowerCase() });
  });
  let prevPct = -100;
  groups.forEach((g, i) => {
    const p = pct(g.v);
    track.append(h('div', { class: 'an-meter-tick', style: `left:${p}%`, 'data-rank': g.rank }));
    const label = `${g.names.map(n => n === 'VeryPoor' ? 'V.Poor' : n).join('/')} ≤ ${fmt(g.v)}`;
    // Hide a label that would collide with the previous one (< ~16% of the track apart).
    const hidden = p - prevPct < 16 && i > 0;
    const first = p < 8, lastLbl = p > 92;
    track.append(h('div', { class: 'an-meter-tick-label' + (hidden ? ' an-tick-hidden' : '') + (first ? ' an-tick-first' : lastLbl ? ' an-tick-last' : ''),
      style: `left:${p}%` }, label));
    if (!hidden) prevPct = p;
  });
  return track;
}

function renderPerformance(report) {
  const s = report.stats;
  const out = h('div', {});
  out.append(h('div', { class: 'an-perf-overall' },
    h('span', {}, 'Overall PC: ', rankBadge(s.pc_overall)),
    h('span', {}, 'Android: ', rankBadge(s.android_overall)),
    h('span', { class: 'an-fineprint' }, 'Worst of the measured metrics; an FBX can only be measured on geometry.')));
  out.append(h('div', { class: 'an-meter-legend' },
    RANK_ORDER.map(r => h('span', {}, h('span', { class: 'an-swatch-line', style: `background:var(--an-rank-${r.toLowerCase()})` }), RANKS[r].label)),
    h('span', {}, 'bar = this file; ticks = the limit each tier allows (docs: performance-stats)')));

  const meters = h('div', { class: 'an-meters' });
  for (const m of s.stats) {
    const t = THRESHOLDS[m.name];
    const card = h('div', { class: 'an-meter-card' });
    card.append(h('div', { class: 'an-meter-head' },
      h('span', { class: 'an-meter-name' }, m.name),
      h('span', { class: 'an-meter-value' }, m.value === m.android_value ? fmt(m.value) : `${fmt(m.value)} PC · ${fmt(m.android_value)} Android`)));
    const row = (platform, value, limits, rank) => h('div', { class: 'an-meter-row' },
      h('span', { class: 'an-meter-platform' }, platform),
      limits ? meter(value, limits, rank || rankFor(value, limits)) : h('span', { class: 'an-meter-na' }, 'not ranked on Android'),
      h('span', { class: 'an-meter-rank' }, limits ? rankBadge(rank || rankFor(value, limits)) : ''));
    card.append(row('PC', m.value, t ? t.pc : null, m.pc));
    card.append(row('Android', m.android_value, t ? t.android : null, m.android));
    meters.append(card);
  }
  out.append(meters);

  out.append(h('h3', {}, 'Table'));
  out.append(h('div', { class: 'table-wrap' }, h('table', {},
    h('thead', {}, h('tr', {}, h('th', {}, 'Metric'), h('th', { class: 'an-num' }, 'Value'), h('th', {}, 'PC'), h('th', {}, 'Android'),
      h('th', { class: 'an-num' }, 'PC limits (E / G / M / P)'), h('th', { class: 'an-num' }, 'Android limits'))),
    h('tbody', {}, s.stats.map(m => {
      const t = THRESHOLDS[m.name];
      return h('tr', {},
        h('td', {}, m.name),
        h('td', { class: 'an-num' }, m.value === m.android_value ? fmt(m.value) : `${fmt(m.value)} / ${fmt(m.android_value)}`),
        h('td', {}, rankBadge(m.pc)), h('td', {}, rankBadge(m.android)),
        h('td', { class: 'an-num' }, t ? t.pc.map(fmt).join(' / ') : '—'),
        h('td', { class: 'an-num' }, t && t.android ? t.android.map(fmt).join(' / ') : '—'));
    })))));
  if (s.not_evaluated?.length) {
    out.append(h('p', { class: 'an-fineprint' },
      'Not measurable from an FBX (could still lower the in-game rank): ' + s.not_evaluated.join(', ') + '.'));
  }
  return out;
}

// ---------------------------------------------------------------------------
// Meshes & materials
// ---------------------------------------------------------------------------

function renderMeshes(report, ctx) {
  const out = h('div', {});
  const meshes = report.meshes || [];
  const materials = report.materials || [];
  const manifest = ctx.manifest;

  out.append(h('h3', {}, `Meshes `, h('span', { class: 'an-tab-count' }, meshes.length)));
  if (!meshes.length) out.append(h('p', { class: 'an-bs-empty' }, 'No geometry in this file.'));
  else out.append(h('div', { class: 'table-wrap' }, h('table', {},
    h('thead', {}, h('tr', {}, h('th', {}, 'Mesh'), h('th', { class: 'an-num' }, 'Vertices'), h('th', { class: 'an-num' }, 'Control points'),
      h('th', { class: 'an-num' }, 'Triangles'), h('th', {}, 'Skinned'), h('th', { class: 'an-num' }, 'Slots'), h('th', { class: 'an-num' }, 'Bones influencing'), h('th', {}, 'Materials'))),
    h('tbody', {}, meshes.map(m => {
      const mm = manifest?.meshes?.[m.index];
      const mats = mm ? mm.material_slots.map(i => materials[i]?.name ?? manifest.materials?.[i]?.name ?? `#${i}`) : [];
      return h('tr', {},
        h('td', {}, h('strong', {}, m.name)),
        h('td', { class: 'an-num' }, fmt(m.vertices)), h('td', { class: 'an-num' }, fmt(m.control_points)),
        h('td', { class: 'an-num' }, fmt(m.triangles)), h('td', {}, yesNo(m.skinned)),
        h('td', { class: 'an-num' }, fmt(m.material_slots)), h('td', { class: 'an-num' }, m.skinned ? fmt(m.bones_influencing) : '—'),
        h('td', {}, mats.length ? h('code', {}, mats.join(', ')) : h('span', { class: 'an-no' }, '—')));
    })))));

  out.append(h('h3', {}, `Materials `, h('span', { class: 'an-tab-count' }, materials.length)));
  if (!materials.length) { out.append(h('p', { class: 'an-bs-empty' }, 'No materials in this file.')); return out; }
  const grid = h('div', { class: 'an-mat-grid' });
  const frag = document.createDocumentFragment();
  materials.forEach((m, i) => {
    const thumb = h('div', { class: 'an-mat-thumb' });
    const dims = h('span', { class: 'an-mat-dims' });
    const c = m.diffuse_color;
    const css = c ? `rgb(${Math.round(c[0] * 255)} ${Math.round(c[1] * 255)} ${Math.round(c[2] * 255)})` : null;
    const tex = m.texture;
    const mime = manifest?.materials?.[i]?.texture?.mime || null;
    let placed = false;
    if (tex && tex.embedded && ctx.sceneView) {
      try {
        const bytes = ctx.sceneView.texture(i);
        if (bytes && bytes.length) {
          if (mime === 'image/x-tga') {
            thumb.append(h('div', { class: 'an-mat-none' }, 'TGA (no browser decoder)'));
            dims.textContent = fmtBytes(bytes.length);
            placed = true;
          } else {
            const url = URL.createObjectURL(new Blob([bytes], { type: mime || 'application/octet-stream' }));
            ctx.blobUrls.push(url);
            const img = h('img', { alt: `texture of ${m.name}`, loading: 'lazy' });
            img.addEventListener('load', () => { dims.textContent = `${img.naturalWidth}×${img.naturalHeight} · ${fmtBytes(bytes.length)}`; });
            img.addEventListener('error', () => { thumb.replaceChildren(h('div', { class: 'an-mat-none' }, 'undecodable image')); dims.textContent = fmtBytes(bytes.length); });
            img.src = url;
            thumb.append(img);
            placed = true;
          }
        }
      } catch (e) { /* fall through to swatch */ }
    }
    if (!placed) {
      if (css) thumb.style.background = css;
      else thumb.append(h('div', { class: 'an-mat-none' }, tex ? 'external texture' : 'no texture'));
      if (tex && tex.embedded && tex.embedded_bytes) dims.textContent = fmtBytes(tex.embedded_bytes) + ' embedded';
    }
    frag.append(h('div', { class: 'an-mat-card' },
      thumb,
      h('div', { class: 'an-mat-body' },
        h('div', { class: 'an-mat-name' }, m.name || `material #${i}`),
        h('div', { class: 'an-mat-line' },
          css ? h('span', { class: 'an-swatch', style: `background:${css}`, title: 'diffuse colour' }) : null,
          css ? h('code', {}, c.map(v => v.toFixed(2)).join(', ')) : h('span', { class: 'an-no' }, 'no diffuse colour')),
        h('div', { class: 'an-mat-line' }, tex
          ? [h('code', { title: tex.absolute || '' }, tex.relative || tex.absolute || '(unnamed)'), tex.embedded ? null : h('span', { class: 'an-no' }, ' external')]
          : h('span', { class: 'an-no' }, 'no texture')),
        h('div', { class: 'an-mat-line' }, dims))));
  });
  grid.append(frag);
  out.append(grid);
  return out;
}

// ---------------------------------------------------------------------------
// Blendshapes
// ---------------------------------------------------------------------------

const GROUP_ORDER = ['viseme', 'blink', 'expression', 'other'];
const GROUP_LABEL = { viseme: 'Visemes', blink: 'Blink', expression: 'Expressions', other: 'Other' };

function renderBlendshapes(report) {
  const out = h('div', {});
  const all = report.blendshapes || [];
  if (!all.length) { out.append(h('p', { class: 'an-bs-empty' }, 'No blendshape channels in this file.')); return out; }

  // Viseme checklist across every mesh.
  const present = new Map();
  for (const b of all) if (b.group === 'viseme') { const k = visemeKey(b.name); if (k && !present.has(k)) present.set(k, b); }
  out.append(h('h3', {}, 'VRChat visemes ', h('span', { class: 'an-tab-count' + (present.size < 15 ? ' an-tab-count-warn' : '') }, `${present.size} / 15`)));
  out.append(h('div', { class: 'an-viseme-grid' }, VISEMES.map(v => {
    const b = present.get(v.toLowerCase());
    return h('div', { class: 'an-check ' + (b ? 'an-found' : 'an-missing-recommended'), title: b ? `${b.name} on ${b.mesh ?? '?'}` : 'missing' },
      h('span', { class: 'an-check-mark', 'aria-hidden': 'true' }, b ? '✓' : '○'), h('span', {}, 'vrc.v_' + v));
  })));
  out.append(h('p', { class: 'an-fineprint' }, 'The descriptor\'s viseme blendshape mode expects all fifteen on the face mesh; this is the set ',
    h('code', {}, 'avatar lint'), ' cross-checks against the source FBX.'));

  const search = h('input', { type: 'search', placeholder: 'Filter blendshapes…', 'aria-label': 'Filter blendshapes' });
  const counts = h('span', { class: 'an-fineprint' });
  out.append(h('div', { class: 'an-bs-toolbar' }, search, counts));

  const byMesh = new Map();
  for (const b of all) {
    const key = b.mesh || '(unresolved mesh)';
    if (!byMesh.has(key)) byMesh.set(key, new Map());
    const groups = byMesh.get(key);
    const g = b.group || 'other';
    if (!groups.has(g)) groups.set(g, []);
    groups.get(g).push(b);
  }
  const chips = [];
  const frag = document.createDocumentFragment();
  for (const [mesh, groups] of byMesh) {
    const total = [...groups.values()].reduce((n, l) => n + l.length, 0);
    const meshEl = h('div', { class: 'an-bs-mesh' },
      h('div', { class: 'an-bs-mesh-title' }, mesh, h('span', { class: 'an-tab-count' }, total)));
    for (const g of GROUP_ORDER) {
      const list = groups.get(g); if (!list) continue;
      const listEl = h('div', { class: 'an-bs-list' });
      for (const b of list) {
        const chip = h('span', { class: 'an-bs-chip an-bs-' + g, title: g }, b.name);
        chip._q = b.name.toLowerCase();
        chips.push({ chip, group: null, mesh: meshEl });
        listEl.append(chip);
      }
      const groupEl = h('div', { class: 'an-bs-group' }, h('div', { class: 'an-bs-group-title' }, `${GROUP_LABEL[g]} · ${list.length}`), listEl);
      for (let i = chips.length - list.length; i < chips.length; i++) chips[i].group = groupEl;
      meshEl.append(groupEl);
    }
    frag.append(meshEl);
  }
  out.append(frag);
  const applyFilter = () => {
    const q = search.value.trim().toLowerCase();
    let shown = 0;
    const liveGroups = new Set(), liveMeshes = new Set();
    for (const c of chips) {
      const ok = !q || c.chip._q.includes(q);
      c.chip.classList.toggle('an-hidden-by-filter', !ok);
      if (ok) { shown++; liveGroups.add(c.group); liveMeshes.add(c.mesh); }
    }
    for (const c of chips) { c.group.classList.toggle('an-hidden-by-filter', !liveGroups.has(c.group)); c.mesh.classList.toggle('an-hidden-by-filter', !liveMeshes.has(c.mesh)); }
    counts.textContent = q ? `${shown} of ${all.length} shown` : `${all.length} channels on ${byMesh.size} mesh${byMesh.size === 1 ? '' : 'es'}`;
  };
  search.addEventListener('input', applyFilter);
  applyFilter();
  return out;
}

// ---------------------------------------------------------------------------
// Raw JSON
// ---------------------------------------------------------------------------

function renderRaw(report) {
  const pre = h('pre', { class: 'an-json' });
  const details = h('details', { class: 'an-details' }, h('summary', {}, 'Show the full report (same shape as avatar describe --json)'), pre);
  // Pretty-print lazily — the report can be large.
  details.addEventListener('toggle', () => { if (details.open && !pre.textContent) pre.textContent = JSON.stringify(report, null, 2); }, { once: true });
  return h('div', {}, details);
}

// ---------------------------------------------------------------------------
// Report assembly
// ---------------------------------------------------------------------------

function renderReport(report, ctx) {
  const issues = collectIssues(report);
  const errors = issues.filter(i => i.sev !== 'info').length;
  const a = report.armature;
  const tabs = makeTabs([
    { id: 'overview', label: 'Overview', count: issues.length || null, warn: errors > 0, body: renderOverview(report, issues) },
    { id: 'rig', label: 'Rig', count: a.bone_like_count, warn: a.missing_required.length > 0, body: renderRig(report, ctx) },
    { id: 'performance', label: 'Performance', body: renderPerformance(report) },
    { id: 'meshes', label: 'Meshes & materials', count: (report.meshes?.length ?? report.fbx.geometries), body: renderMeshes(report, ctx) },
    { id: 'blendshapes', label: 'Blendshapes', count: report.blendshapes.length, body: renderBlendshapes(report) },
    { id: 'raw', label: 'Raw JSON', body: renderRaw(report) },
  ]);
  ctx.tabs = tabs;
  return tabs;
}

// ---------------------------------------------------------------------------
// Boot
// ---------------------------------------------------------------------------

async function boot() {
  document.body.classList.add('an-page');
  const $ = id => document.getElementById(id);
  const drop = $('an-drop'), input = $('an-file'), status = $('an-status'), results = $('an-results');
  const viewerEl = $('an-viewer'), filebar = $('an-filebar'), sampleBtn = $('an-sample');
  if (!drop || !input) return;

  let wasm = null;
  try {
    wasm = await import('../wasm/avatar_web_analyzer.js');
    await wasm.default();
    status.textContent = 'Analyzer loaded — drop a binary .fbx file, or try the sample.';
    sampleBtn.disabled = typeof wasm.sample_fbx !== 'function';
  } catch (e) {
    status.textContent = 'The wasm bundle is missing or failed to load. On the published site this '
      + 'is built by CI; for a local preview build it with wasm-pack (see site/README.md) and serve '
      + 'over http, not file://.';
    drop.classList.add('an-disabled');
    return;
  }

  // The viewer module is optional: the report renders without it.
  let viewerModule = null;
  const viewerPromise = import('./viewer.js').then(m => { viewerModule = m; }).catch(e => {
    console.warn('viewer.js unavailable; 3D preview disabled', e);
  });

  const ctx = { viewer: null, sceneView: null, manifest: null, blobUrls: [], rowById: new Map(), tabs: null, report: null, fileName: null,
    select(id, from) {}, };
  let selectedId = null;

  function boneIndexForId(id) {
    const bones = ctx.manifest?.bones; if (!bones) return null;
    if (!ctx._idToIndex) { ctx._idToIndex = new Map(); bones.forEach(b => ctx._idToIndex.set(b.id, b.index)); }
    const i = ctx._idToIndex.get(id); return i == null ? null : i;
  }
  function boneIdForIndex(index) {
    const b = ctx.manifest?.bones?.[index]; return b ? b.id : null;
  }
  ctx.select = (id, from) => {
    if (selectedId === id && from === 'tree') id = null;  // click again to clear
    selectedId = id;
    for (const { row } of ctx.rowById.values()) row.classList.remove('an-selected');
    if (id != null) {
      const r = ctx.rowById.get(id);
      if (r) {
        r.row.classList.add('an-selected');
        let cur = r.li.parentElement?.closest('li');
        while (cur) {
          if (cur.classList.contains('an-collapsed')) cur.querySelector(':scope > .an-tree-row > .an-tree-toggle').click();
          cur = cur.parentElement?.closest('li');
        }
        if (from !== 'tree' || !r.row.matches(':hover')) r.row.scrollIntoView({ block: 'nearest' });
        if (from === 'tree' && ctx.tabs) ctx.tabs.selectTab('rig');
        else if (from === 'viewer' && ctx.tabs) ctx.tabs.selectTab('rig');
      }
    }
    if (from !== 'viewer' && ctx.viewer) { try { ctx.viewer.highlightBone(id == null ? null : boneIndexForId(id)); } catch (e) { /* ignore */ } }
  };

  function reset() {
    for (const u of ctx.blobUrls) URL.revokeObjectURL(u);
    ctx.blobUrls = [];
    if (ctx.viewer) { try { ctx.viewer.dispose(); } catch (e) { /* ignore */ } ctx.viewer = null; }
    if (ctx.sceneView) { try { ctx.sceneView.free(); } catch (e) { /* ignore */ } ctx.sceneView = null; }
    ctx.manifest = null; ctx._idToIndex = null; ctx.rowById = new Map(); ctx.tabs = null; selectedId = null;
    viewerEl.replaceChildren(); viewerEl.hidden = true;
    results.replaceChildren();
  }

  async function handle(bytes, name, size) {
    reset();
    status.textContent = `Analyzing ${name} (${fmtBytes(size)})…`;
    filebar.hidden = true;
    await new Promise(r => setTimeout(r, 0));  // let the status paint before the synchronous wasm call
    let report;
    try {
      report = JSON.parse(wasm.analyze_fbx(bytes, name));
    } catch (e) {
      status.textContent = '';
      results.replaceChildren(h('div', { class: 'notice warn' },
        h('div', { class: 'notice-title' }, 'Could not analyze this file'),
        String(e && e.message ? e.message : e),
        h('p', {}, 'Only binary FBX 7.x is supported — ASCII FBX must be re-exported as binary.')));
      return;
    }
    ctx.report = report; ctx.fileName = name;
    $('an-file-name').textContent = name;
    $('an-file-meta').textContent = `${fmtBytes(size)} · FBX ${report.fbx.version}`;
    filebar.hidden = false;
    drop.classList.add('an-collapsed');

    // Scene view for the 3D preview + textures (optional; report still renders).
    if (typeof wasm.SceneView?.load === 'function') {
      try {
        ctx.sceneView = wasm.SceneView.load(bytes);
        ctx.manifest = JSON.parse(ctx.sceneView.manifest());
      } catch (e) {
        console.warn('SceneView failed', e);
        ctx.sceneView = null; ctx.manifest = null;
      }
    }

    status.textContent = '';
    results.replaceChildren(renderReport(report, ctx));

    if (ctx.sceneView && ctx.manifest) {
      viewerEl.hidden = false;
      await viewerPromise;
      if (viewerModule && typeof viewerModule.createViewer === 'function') {
        try {
          ctx.viewer = viewerModule.createViewer(viewerEl);
          ctx.viewer.onBoneSelect = idx => ctx.select(idx == null ? null : boneIdForIndex(idx), 'viewer');
          ctx.viewer.load(ctx.sceneView, ctx.manifest);
        } catch (e) {
          console.warn('viewer failed', e);
          ctx.viewer = null;
          viewerEl.replaceChildren(h('div', { class: 'an-viewer-fallback' }, '3D preview unavailable in this browser (WebGL). The report below is unaffected.'));
        }
      } else {
        viewerEl.replaceChildren(h('div', { class: 'an-viewer-fallback' }, '3D preview module did not load. The report below is unaffected.'));
      }
    }
  }

  const handleFile = async file => handle(new Uint8Array(await file.arrayBuffer()), file.name, file.size);

  drop.addEventListener('dragover', e => { e.preventDefault(); drop.classList.add('an-over'); });
  drop.addEventListener('dragleave', () => drop.classList.remove('an-over'));
  drop.addEventListener('drop', e => {
    e.preventDefault(); drop.classList.remove('an-over');
    const file = e.dataTransfer.files && e.dataTransfer.files[0];
    if (file) handleFile(file);
  });
  drop.addEventListener('click', () => input.click());
  drop.addEventListener('keydown', e => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); input.click(); } });
  input.addEventListener('change', () => { if (input.files[0]) handleFile(input.files[0]); input.value = ''; });

  sampleBtn.addEventListener('click', () => {
    try {
      const bytes = wasm.sample_fbx();
      handle(bytes, 'sample-humanoid.fbx', bytes.length);
    } catch (e) { status.textContent = 'Sample unavailable: ' + (e.message || e); }
  });
  $('an-another').addEventListener('click', () => input.click());
  $('an-download').addEventListener('click', () => {
    if (!ctx.report) return;
    const blob = new Blob([JSON.stringify(ctx.report, null, 2)], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const a = h('a', { href: url, download: (ctx.fileName || 'avatar').replace(/\.fbx$/i, '') + '.report.json' });
    document.body.append(a); a.click(); a.remove();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  });
  $('an-screenshot').addEventListener('click', () => {
    if (!ctx.viewer) { status.textContent = 'No 3D view to screenshot.'; return; }
    try {
      const url = ctx.viewer.screenshot();
      const a = h('a', { href: url, download: (ctx.fileName || 'avatar').replace(/\.fbx$/i, '') + '.png' });
      document.body.append(a); a.click(); a.remove();
    } catch (e) { status.textContent = 'Screenshot failed: ' + (e.message || e); }
  });
  window.addEventListener('hashchange', () => { const id = tabIdFromHash(); if (id && ctx.tabs) ctx.tabs.selectTab(id); });
}

boot();
