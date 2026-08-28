/* Analyzer page: drag-and-drop FBX analysis, fully client-side.
 *
 * Loads the wasm bundle wasm-pack builds from crates/web-analyzer (the same
 * diagnose graph the CLI uses: avatar-fbx + avatar-armature + avatar-stats)
 * and renders its JSON report. The dropped file is read into memory and
 * handed to wasm — it never leaves the browser.
 *
 * The bundle is a build artifact: CI builds it on every Pages deploy. For a
 * local preview run
 *   wasm-pack build crates/web-analyzer --target web --release --out-dir ../../site/wasm
 * and serve the site over http (module + wasm loading does not work file://):
 *   python3 -m http.server -d site
 */

const RANKS = {
  Excellent: { label: 'Excellent', cls: 'rank-excellent' },
  Good:      { label: 'Good',      cls: 'rank-good' },
  Medium:    { label: 'Medium',    cls: 'rank-medium' },
  Poor:      { label: 'Poor',      cls: 'rank-poor' },
  VeryPoor:  { label: 'Very Poor', cls: 'rank-verypoor' },
};

function h(tag, attrs, ...children) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs || {})) {
    if (k === 'class') el.className = v;
    else el.setAttribute(k, v);
  }
  for (const c of children.flat()) {
    if (c == null) continue;
    el.append(c.nodeType ? c : document.createTextNode(String(c)));
  }
  return el;
}

function rankBadge(rank) {
  if (!rank) return h('span', { class: 'rank-badge rank-na' }, 'n/a');
  const r = RANKS[rank] || { label: rank, cls: 'rank-na' };
  return h('span', { class: 'rank-badge ' + r.cls }, r.label);
}

function statChip(label, value) {
  return h('div', { class: 'an-chip' },
    h('div', { class: 'an-chip-value' }, value),
    h('div', { class: 'an-chip-label' }, label));
}

function section(title, ...children) {
  return h('section', { class: 'doc-section' }, h('h2', {}, title), ...children);
}

function renderReport(report, fileName) {
  const out = h('div', {});

  // --- File summary -------------------------------------------------------
  const f = report.fbx;
  out.append(section('File',
    h('p', {}, h('code', {}, fileName), ` — binary FBX, version ${f.version}.`),
    h('div', { class: 'an-chip-row' },
      statChip('models', f.models),
      statChip('bone-like', f.bone_like),
      statChip('geometries', f.geometries),
      statChip('materials', f.materials),
      statChip('deformers', f.deformers),
      statChip('blendshapes', report.blendshapes.length))));

  // --- Humanoid rig -------------------------------------------------------
  const a = report.armature;
  const ready = a.missing_required.length === 0;
  const rig = section('Humanoid rig check');
  rig.append(h('p', {},
    h('span', { class: 'rank-badge ' + (ready ? 'rank-excellent' : 'rank-verypoor') },
      ready ? 'humanoid-ready' : 'not humanoid-ready'),
    ready
      ? ' Every Unity-required humanoid bone maps from this skeleton.'
      : ' Unity cannot auto-configure this rig as Humanoid until the missing required bones exist.'));
  if (a.armature_roots.length !== 1) {
    rig.append(h('div', { class: 'notice warn' },
      h('div', { class: 'notice-title' }, `${a.armature_roots.length} armature roots`),
      'VRChat expects exactly one skeleton root. Roots found: ',
      h('code', {}, a.armature_roots.join(', ') || 'none')));
  }
  const issueList = (title, items) => items.length
    ? h('div', { class: 'an-issue' }, h('strong', {}, title + ': '),
        h('code', {}, items.join(', ')))
    : null;
  for (const el of [
    issueList('Missing required', a.missing_required),
    issueList('Missing recommended', a.missing_recommended),
    issueList('Duplicate mappings', Object.keys(a.duplicate_mappings)),
  ]) if (el) rig.append(el);

  const mapped = Object.entries(a.mapped);
  if (mapped.length) {
    const tbl = h('table', {},
      h('thead', {}, h('tr', {}, h('th', {}, 'Humanoid bone'), h('th', {}, 'Source bone'))),
      h('tbody', {}, mapped.map(([slot, names]) =>
        h('tr', {}, h('td', {}, slot), h('td', {}, h('code', {}, names.join(', ')))))));
    rig.append(h('details', { class: 'an-details' },
      h('summary', {}, `Mapped bones (${mapped.length})`),
      h('div', { class: 'table-wrap' }, tbl)));
  }
  if (a.unmapped_bones.length) {
    rig.append(h('details', { class: 'an-details' },
      h('summary', {}, `Unmapped bone-like nodes (${a.unmapped_bones.length}) — accessory / twist / dynamic bones`),
      h('p', {}, h('code', {}, a.unmapped_bones.join(', ')))));
  }
  rig.append(h('p', { class: 'an-fineprint' },
    `${a.ignored_finger_bones} finger and ${a.ignored_leaf_bones} leaf *_End bones recognized and excluded from body mapping.`));
  out.append(rig);

  // --- Performance --------------------------------------------------------
  const s = report.stats;
  const perf = section('Performance rank (geometry)');
  perf.append(h('p', {},
    'Overall — PC: ', rankBadge(s.pc_overall), ' Android: ', rankBadge(s.android_overall),
    '. Worst of the measured metrics only; an FBX can only be measured on geometry.'));
  perf.append(h('div', { class: 'table-wrap' }, h('table', {},
    h('thead', {}, h('tr', {},
      h('th', {}, 'Metric'), h('th', {}, 'Value'), h('th', {}, 'PC'), h('th', {}, 'Android'))),
    h('tbody', {}, s.stats.map(m =>
      h('tr', {},
        h('td', {}, m.name),
        h('td', {}, m.value === m.android_value
          ? m.value.toLocaleString()
          : `${m.value.toLocaleString()} / ${m.android_value.toLocaleString()} (Android)`),
        h('td', {}, rankBadge(m.pc)),
        h('td', {}, rankBadge(m.android))))))));
  if (s.not_evaluated.length) {
    perf.append(h('p', { class: 'an-fineprint' },
      'Not measurable from an FBX (could still lower the in-game rank): ' + s.not_evaluated.join(', ') + '.'));
  }
  out.append(perf);

  // --- Blendshapes --------------------------------------------------------
  if (report.blendshapes.length) {
    const byMesh = new Map();
    for (const b of report.blendshapes) {
      const key = b.mesh || '(unresolved mesh)';
      if (!byMesh.has(key)) byMesh.set(key, []);
      byMesh.get(key).push(b.name);
    }
    const bs = section('Blendshape channels');
    for (const [mesh, names] of byMesh) {
      bs.append(h('details', { class: 'an-details' },
        h('summary', {}, `${mesh} (${names.length})`),
        h('p', {}, h('code', {}, names.join(', ')))));
    }
    out.append(bs);
  }

  return out;
}

async function boot() {
  const drop = document.getElementById('an-drop');
  const input = document.getElementById('an-file');
  const status = document.getElementById('an-status');
  const results = document.getElementById('an-results');
  if (!drop || !input) return;

  let analyze;
  try {
    const mod = await import('../wasm/avatar_web_analyzer.js');
    await mod.default();
    analyze = mod.analyze_fbx;
    status.textContent = 'Analyzer loaded — drop a binary .fbx file.';
  } catch (e) {
    status.textContent = 'The wasm bundle is missing or failed to load. On the published site this '
      + 'is built by CI; for a local preview build it with wasm-pack (see site/README.md) and serve '
      + 'over http, not file://.';
    drop.classList.add('an-disabled');
    return;
  }

  async function handle(file) {
    results.replaceChildren();
    status.textContent = `Analyzing ${file.name} (${(file.size / 1048576).toFixed(1)} MB)…`;
    try {
      const bytes = new Uint8Array(await file.arrayBuffer());
      const report = JSON.parse(analyze(bytes, file.name));
      status.textContent = '';
      results.replaceChildren(renderReport(report, file.name));
    } catch (e) {
      status.textContent = '';
      results.replaceChildren(h('div', { class: 'notice warn' },
        h('div', { class: 'notice-title' }, 'Could not analyze this file'),
        String(e && e.message ? e.message : e),
        h('p', {}, 'Only binary FBX 7.x is supported — ASCII FBX must be re-exported as binary.')));
    }
  }

  drop.addEventListener('dragover', e => { e.preventDefault(); drop.classList.add('an-over'); });
  drop.addEventListener('dragleave', () => drop.classList.remove('an-over'));
  drop.addEventListener('drop', e => {
    e.preventDefault();
    drop.classList.remove('an-over');
    const file = e.dataTransfer.files && e.dataTransfer.files[0];
    if (file) handle(file);
  });
  drop.addEventListener('click', () => input.click());
  drop.addEventListener('keydown', e => {
    if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); input.click(); }
  });
  input.addEventListener('change', () => { if (input.files[0]) handle(input.files[0]); });
}

boot();
