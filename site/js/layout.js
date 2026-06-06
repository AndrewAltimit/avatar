/* layout.js - shared layout for the avatar-tools docs site.
 *
 * Each page calls injectLayout({ active: 'reference/sdk3-lint-rules' }).
 * This builds:
 *   - Left sidebar with collapsible sections, active highlight, search trigger.
 *   - In-page TOC rail (auto-built from h2/h3 inside .content).
 *   - Prev/next page footer derived from NAV order.
 *   - Search overlay (filters NAV + headings + page snippets).
 *   - Mobile sidebar toggle and overlay scrim.
 *
 * The structure of NAV below is the single source of truth for nav ordering.
 * Each item's `key` must match the active_key used for that page in _gen.py.
 */

const NAV = [
  {
    label: 'overview',
    items: [
      { href: 'index.html',         text: 'Home',          key: 'home' },
      { href: 'architecture.html',  text: 'How it stacks', key: 'architecture' },
      { href: 'quickstart.html',    text: 'Quick start',   key: 'quickstart' },
    ],
  },
  {
    label: 'crates & cli',
    items: [
      { href: 'crates/index.html',  text: 'Crates',        key: 'crates/index' },
      { href: 'cli/index.html',     text: 'CLI commands',  key: 'cli/index' },
    ],
  },
  {
    label: 'reference',
    items: [
      { href: 'reference/index.html',             text: 'Reference index',  key: 'reference/index' },
      { href: 'reference/humanoid-bones.html',    text: 'Humanoid bones',   key: 'reference/humanoid-bones' },
      { href: 'reference/sdk3-lint-rules.html',   text: 'SDK3 lint rules',  key: 'reference/sdk3-lint-rules' },
      { href: 'reference/armature-repair.html',   text: 'Armature repair',  key: 'reference/armature-repair' },
      { href: 'reference/performance-stats.html', text: 'Performance stats',key: 'reference/performance-stats' },
      { href: 'reference/rig-runtime.html',       text: 'Runtime rig layer',key: 'reference/rig-runtime' },
      { href: 'reference/anim-gen.html',          text: 'Asset generation', key: 'reference/anim-gen' },
      { href: 'reference/osc-runtime.html',       text: 'OSC runtime',      key: 'reference/osc-runtime' },
    ],
  },
];

/* ---------- Helpers ---------- */
function resolveHref(href, depth) {
  if (depth === 0) return href;
  if (/^https?:/.test(href)) return href;
  return '../'.repeat(depth) + href;
}

function depthFromKey(key) {
  if (!key || key === 'home') return 0;
  return key.split('/').length - 1;
}

function flattenNav() {
  const out = [];
  for (const section of NAV) for (const item of section.items) out.push(item);
  return out;
}

function findSiblings(activeKey) {
  const flat = flattenNav();
  const idx = flat.findIndex(x => x.key === activeKey);
  if (idx < 0) return { prev: null, next: null };
  return {
    prev: idx > 0 ? flat[idx - 1] : null,
    next: idx < flat.length - 1 ? flat[idx + 1] : null,
  };
}

function slugify(s) {
  return s.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '');
}

/* ---------- Sidebar ---------- */
function buildSidebar(active, depth) {
  const sidebar = document.createElement('aside');
  sidebar.className = 'sidebar';
  sidebar.id = 'sidebar';

  const brand = document.createElement('a');
  brand.href = resolveHref('index.html', depth);
  brand.className = 'sidebar-brand';
  brand.innerHTML = '<span class="prompt">$</span>avatar';
  sidebar.appendChild(brand);

  /* Search trigger button */
  const searchBtn = document.createElement('button');
  searchBtn.type = 'button';
  searchBtn.className = 'sidebar-search';
  searchBtn.id = 'open-search';
  searchBtn.setAttribute('aria-label', 'Open search');
  searchBtn.innerHTML =
    '<span class="icon">⌕</span>' +
    '<span class="label">Search the site</span>' +
    '<span class="kbd">/</span>';
  sidebar.appendChild(searchBtn);

  for (const section of NAV) {
    const sec = document.createElement('div');
    sec.className = 'sidebar-section';
    sec.dataset.section = section.label;

    const hasActive = section.items.some(item => item.key === active);
    if (hasActive) sec.classList.add('has-active');

    const tog = document.createElement('button');
    tog.type = 'button';
    tog.className = 'sidebar-section-toggle';
    tog.innerHTML = '<span class="arrow">▾</span>' + section.label;
    tog.addEventListener('click', () => {
      sec.classList.toggle('collapsed');
      try {
        const persisted = JSON.parse(localStorage.getItem('sidebar-collapsed') || '{}');
        persisted[section.label] = sec.classList.contains('collapsed');
        localStorage.setItem('sidebar-collapsed', JSON.stringify(persisted));
      } catch (e) {}
    });
    sec.appendChild(tog);

    const nav = document.createElement('nav');
    nav.className = 'sidebar-nav';
    nav.setAttribute('aria-label', section.label);
    for (const item of section.items) {
      const a = document.createElement('a');
      a.href = resolveHref(item.href, depth);
      a.textContent = item.text;
      a.dataset.key = item.key;
      if (item.key === active) a.classList.add('active');
      nav.appendChild(a);
    }
    sec.appendChild(nav);

    try {
      const persisted = JSON.parse(localStorage.getItem('sidebar-collapsed') || '{}');
      if (persisted[section.label] && !hasActive) sec.classList.add('collapsed');
    } catch (e) {}

    sidebar.appendChild(sec);
  }

  const foot = document.createElement('div');
  foot.className = 'sidebar-foot';
  foot.innerHTML =
    '<a href="https://github.com/AndrewAltimit/avatar" target="_blank" rel="noopener">GitHub →</a><br>' +
    'Tooling: MIT or Unlicense.<br>' +
    'Not affiliated with VRChat Inc.';
  sidebar.appendChild(foot);

  return sidebar;
}

/* ---------- Heading ID assignment (before anchors / TOC) ---------- */
function assignHeadingIds() {
  const content = document.querySelector('.content');
  if (!content) return;
  content.querySelectorAll('section.doc-section h2, section.doc-section h3, section.doc-section h4').forEach(h => {
    if (h.id) return;
    const sec = h.closest('section.doc-section');
    if (h.tagName === 'H2' && sec && sec.id) {
      h.id = sec.id;
    } else {
      h.id = slugify(h.textContent || '') || ('h-' + Math.random().toString(36).slice(2, 8));
    }
  });
}

/* ---------- TOC rail ---------- */
function buildTocRail() {
  const content = document.querySelector('.content');
  if (!content) return null;

  const headings = content.querySelectorAll('section.doc-section h2, section.doc-section h3');
  if (headings.length < 2) return null;

  const rail = document.createElement('aside');
  rail.className = 'toc-rail';
  rail.setAttribute('aria-label', 'On this page');

  const title = document.createElement('div');
  title.className = 'toc-title';
  title.textContent = 'On this page';
  rail.appendChild(title);

  const list = document.createElement('ul');
  list.className = 'toc-list';

  headings.forEach(h => {
    const li = document.createElement('li');
    const a = document.createElement('a');
    a.href = '#' + h.id;
    a.textContent = (h.textContent || '').trim();
    a.dataset.target = h.id;
    if (h.tagName === 'H3') a.classList.add('h3');
    li.appendChild(a);
    list.appendChild(li);
  });

  rail.appendChild(list);
  return rail;
}

/* ---------- Heading anchor links (clickable § on h2/h3/h4) ---------- */
function injectHeadingAnchors() {
  const content = document.querySelector('.content');
  if (!content) return;
  content.querySelectorAll('section.doc-section h2, section.doc-section h3, section.doc-section h4').forEach(h => {
    if (h.querySelector('.h-anchor') || !h.id) return;
    const a = document.createElement('a');
    a.className = 'h-anchor';
    a.href = '#' + h.id;
    a.setAttribute('aria-label', 'Anchor link');
    a.textContent = '§';
    h.appendChild(a);
  });
}

/* ---------- Prev/next footer ---------- */
function buildPageNav(active, depth) {
  const { prev, next } = findSiblings(active);
  if (!prev && !next) return null;

  const nav = document.createElement('nav');
  nav.className = 'page-nav';
  nav.setAttribute('aria-label', 'Previous and next page');

  if (prev) {
    const a = document.createElement('a');
    a.href = resolveHref(prev.href, depth);
    a.className = 'pn-prev';
    a.innerHTML = '<div class="pn-label">Previous</div><div class="pn-title">' + prev.text + '</div>';
    nav.appendChild(a);
  }
  if (next) {
    const a = document.createElement('a');
    a.href = resolveHref(next.href, depth);
    a.className = 'pn-next';
    a.innerHTML = '<div class="pn-label">Next</div><div class="pn-title">' + next.text + '</div>';
    nav.appendChild(a);
  }
  return nav;
}

/* ---------- Mobile toggle button ---------- */
function buildMobileToggle() {
  const toggle = document.createElement('button');
  toggle.className = 'sidebar-toggle';
  toggle.setAttribute('aria-label', 'Toggle navigation');
  toggle.setAttribute('aria-expanded', 'false');
  toggle.innerHTML = '&#9776;';
  return toggle;
}

/* ---------- Search overlay ---------- */
function buildSearchOverlay(depth) {
  const overlay = document.createElement('div');
  overlay.className = 'search-overlay';
  overlay.id = 'search-overlay';
  overlay.innerHTML = `
    <div class="search-box" role="dialog" aria-label="Search">
      <div class="search-input-wrap">
        <span class="icon">⌕</span>
        <input type="text" class="search-input" id="search-input" placeholder="Search pages, sections, crates, rules..." aria-label="Search query">
        <button type="button" class="search-close" aria-label="Close">esc</button>
      </div>
      <ul class="search-results" id="search-results" role="listbox"></ul>
      <div class="search-foot">
        <span><kbd>↑</kbd><kbd>↓</kbd> navigate</span>
        <span><kbd>↵</kbd> open</span>
        <span><kbd>esc</kbd> close</span>
      </div>
    </div>
  `;
  overlay.dataset.depth = String(depth);
  return overlay;
}

/* ---------- Main ---------- */
function injectLayout(opts) {
  const { active } = opts || {};
  const depth = depthFromKey(active);

  const sidebar = buildSidebar(active, depth);
  const toggle = buildMobileToggle();
  const overlay = buildSearchOverlay(depth);

  const scrim = document.createElement('div');
  scrim.className = 'sidebar-overlay';
  scrim.id = 'sidebar-scrim';

  toggle.addEventListener('click', () => {
    const open = sidebar.classList.toggle('open');
    toggle.setAttribute('aria-expanded', String(open));
    scrim.classList.toggle('show', open);
  });
  scrim.addEventListener('click', () => {
    sidebar.classList.remove('open');
    toggle.setAttribute('aria-expanded', 'false');
    scrim.classList.remove('show');
  });

  const app = document.querySelector('.app');
  if (app) {
    app.insertBefore(sidebar, app.firstChild);
  } else {
    document.body.insertBefore(sidebar, document.body.firstChild);
  }
  document.body.insertBefore(toggle, document.body.firstChild);
  document.body.appendChild(scrim);
  document.body.appendChild(overlay);

  /* Order matters: assign IDs first → build TOC (clean text) → add § anchors */
  assignHeadingIds();
  const toc = buildTocRail();
  injectHeadingAnchors();
  if (toc && app) {
    app.appendChild(toc);
  } else if (app) {
    app.classList.add('no-toc');
  }

  const content = document.querySelector('.content');
  if (content) {
    const pn = buildPageNav(active, depth);
    if (pn) content.appendChild(pn);
  }

  const openSearch = document.getElementById('open-search');
  if (openSearch) openSearch.addEventListener('click', () => window.openSearch && window.openSearch());

  document.addEventListener('keydown', (e) => {
    if (e.target && (e.target.tagName === 'INPUT' || e.target.tagName === 'TEXTAREA' || e.target.isContentEditable)) return;
    if (e.key === '/') {
      e.preventDefault();
      window.openSearch && window.openSearch();
    }
  });
}

window.injectLayout = injectLayout;
window.SITE_NAV = NAV;
