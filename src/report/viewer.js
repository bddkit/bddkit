// The report viewer: renders the run embedded in the page, never folds events.
// Text goes in through textContent only: the CSP allows inline script, so an
// innerHTML of a response body would be a hole.
(function () {
  'use strict';
  const page = JSON.parse(document.getElementById('run').textContent);
  const run = page.run;

  function el(tag, cls, text) {
    const node = document.createElement(tag);
    if (cls) node.className = cls;
    if (text !== undefined && text !== null) node.textContent = text;
    return node;
  }
  function add(parent) {
    for (let i = 1; i < arguments.length; i++) if (arguments[i]) parent.appendChild(arguments[i]);
    return parent;
  }
  function original(path) {
    return page.artifacts_base + path.split('/').map(encodeURIComponent).join('/');
  }
  // The one choke point for artifact links: an embedded file is decoded once, on first use.
  // Only raster images render in place; anything else, SVG and HTML included, is a download, never a document of this page's origin.
  const SAFE_TYPES = /^image\/(png|jpeg|gif|webp)$/;
  const blobs = new Map();
  function link(path) {
    const f = page.embedded && page.embedded[path];
    if (!f) return original(path);
    if (!blobs.has(path)) {
      const bin = atob(f.data);
      const bytes = new Uint8Array(bin.length);
      for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
      blobs.set(path, URL.createObjectURL(new Blob([bytes], { type: SAFE_TYPES.test(f.media_type) ? f.media_type : 'application/octet-stream' })));
    }
    return blobs.get(path);
  }
  function fmtSize(n) {
    if (n < 1000) return n + ' B';
    const u = ['KB', 'MB', 'GB'];
    let i = -1;
    do { n /= 1000; i++; } while (n >= 1000 && i < u.length - 1);
    return n.toFixed(1) + ' ' + u[i];
  }
  // What happened to a linked file: embedded (and how), left out (and why), or nothing to say.
  function mark(path) {
    const f = page.embedded && page.embedded[path];
    const skipped = page.not_embedded && page.not_embedded[path];
    if (f) {
      const how = f.encoding === 'original' ? 'original'
        : 'WebP ' + (f.encoding === 'webp-lossy' ? 'lossy' : 'lossless') + ', ' + fmtSize(f.size) + ' → ' + fmtSize(f.data.length * 3 / 4 - (f.data.match(/=*$/)[0].length));
      const note = el('span', 'note', 'embedded, ' + how + ' · ');
      const a = el('a', '', 'original file'); a.href = original(path);
      return add(note, a);
    }
    if (skipped) {
      return el('span', 'note', 'not embedded: ' + (skipped.reason === 'limit' ? 'over the limit, ' + fmtSize(skipped.size) : skipped.reason));
    }
    return null;
  }
  function fmtDuration(us) {
    if (us === null || us === undefined) return '';
    if (us < 1000) return us + ' µs';
    if (us < 1e6) return (us / 1000).toFixed(1) + ' ms';
    return (us / 1e6).toFixed(2) + ' s';
  }
  function paragraphs(text) {
    const box = el('div', 'text');
    String(text).split(/\n{2,}/).forEach(function (p) { box.appendChild(el('p', '', p)); });
    return box;
  }
  function tagList(tags) {
    const box = el('span', 'tags');
    (tags || []).forEach(function (t) { box.appendChild(el('span', 'tag', t)); });
    return box;
  }

  // ---- state in the URL hash -------------------------------------------
  // A file that panicked counts as one failure even when it never got as far as a scenario.
  const failedCount = run.files.reduce(function (n, f) {
    return n + f.scenarios.filter(isBad).length + (f.panicked && !f.scenarios.some(isBad) ? 1 : 0);
  }, 0);
  function isBad(s) { return s.status !== 'passed'; }
  const state = { status: failedCount ? 'failed' : 'all', q: '', tag: '', s: '' };
  function readHash() {
    const p = new URLSearchParams(location.hash.slice(1));
    Object.keys(state).forEach(function (k) { if (p.has(k)) state[k] = p.get(k); });
  }
  function writeHash() {
    const p = new URLSearchParams();
    Object.keys(state).forEach(function (k) { if (state[k]) p.set(k, state[k]); });
    try { history.replaceState(null, '', '#' + p.toString()); } catch (e) { /* a sandboxed origin: the state just is not linkable */ }
  }
  readHash();

  // ---- header and banners ----------------------------------------------
  const root = document.getElementById('app');
  const totals = run.finished || { files: run.files.length, failed: failedCount,
    scenarios: run.files.reduce(function (n, f) { return n + f.scenarios.length; }, 0) };
  const head = add(el('header'), el('h1', '', 'bddkit run ' + run.run_id));
  const meta = el('dl', 'meta');
  function row(k, v) { if (v !== null && v !== undefined && v !== '') add(meta, el('dt', '', k), el('dd', '', v)); }
  row('bddkit', run.bddkit);
  row('started', run.started_at_unix_ms ? new Date(run.started_at_unix_ms).toLocaleString() : '');
  if (run.finished) {
    row('duration', run.finished.duration_ms + ' ms');
    row('exit', run.finished.exit + (run.finished.signal ? ' (' + run.finished.signal + ')' : ''));
  }
  row('files', String(totals.files));
  row('scenarios', String(totals.scenarios));
  row('failed', String(totals.failed));
  row('config', run.config);
  if (run.selection && run.selection.tags && run.selection.tags.length) row('tags', run.selection.tags.join(', '));
  if (run.selection && run.selection.paths && run.selection.paths.length) row('paths', run.selection.paths.join(', '));
  head.appendChild(meta);
  root.appendChild(head);

  function banner(text) { root.appendChild(el('div', 'banner', text)); }
  if (!run.masking) banner('Masking was off for this run (--reveal): secrets may appear in clear text.');
  if (!run.evidence) banner('No evidence was collected. Run with --evidence (and --events) to see requests, responses and SQL.');
  if (run.incomplete) banner('This stream is incomplete: the run was killed or cut short, so the report ends where the stream does.');
  if (page.artifacts_temporary && (page.not_embedded || !page.embedded)) banner('Artifacts are under the temporary directory and links will break when it is cleaned. Use --artifacts-dir next to where this report is kept.');

  // ---- filters -----------------------------------------------------------
  const allTags = [];
  run.files.forEach(function (f) {
    f.scenarios.forEach(function (s) { s.tags.forEach(function (t) { if (allTags.indexOf(t) < 0) allTags.push(t); }); });
  });
  allTags.sort();
  const bar = el('div', 'bar');
  const status = el('select');
  [['failed', 'Failed'], ['passed', 'Passed'], ['all', 'All']].forEach(function (o) {
    const opt = el('option', '', o[1]); opt.value = o[0]; status.appendChild(opt);
  });
  status.value = state.status;
  const search = el('input'); search.type = 'search'; search.placeholder = 'Search names, steps, descriptions'; search.value = state.q;
  const tagSel = el('select');
  const none = el('option', '', 'All tags'); none.value = ''; tagSel.appendChild(none);
  allTags.forEach(function (t) { const o = el('option', '', t); o.value = t; tagSel.appendChild(o); });
  tagSel.value = state.tag;
  const counter = el('span', 'count');
  add(bar, status, search, tagSel, counter);
  root.appendChild(bar);
  const list = el('main');
  root.appendChild(list);

  // ---- steps -------------------------------------------------------------
  function body(value) {
    if (value === null || value === undefined) return null;
    if (typeof value === 'object') {
      const box = el('div', 'file');
      add(box, el('a', '', value.path + ' (' + value.size + ' bytes)'));
      box.firstChild.href = link(value.path);
      return add(box, mark(value.path));
    }
    const pre = el('pre', 'body');
    let pretty = null;
    try { pretty = JSON.stringify(JSON.parse(value), null, 2); } catch (e) { pretty = null; }
    pre.textContent = pretty !== null ? pretty : value;
    if (pretty === null || pretty === value) return pre;
    const box = el('div');
    const toggle = el('button', 'toggle', 'raw');
    let raw = false;
    toggle.onclick = function () {
      raw = !raw; pre.textContent = raw ? value : pretty; toggle.textContent = raw ? 'pretty' : 'raw';
    };
    return add(box, toggle, pre);
  }
  function headers(pairs) {
    const table = el('table', 'headers');
    (pairs || []).forEach(function (p) { add(table, add(el('tr'), el('th', '', p[0]), el('td', '', p[1]))); });
    return table;
  }
  function evidence(e) {
    const box = el('details', 'evidence');
    const c = e.content;
    let title = e.title || e.kind;
    if (e.kind === 'http' && c) title = c.method + ' ' + c.url + ' → ' + c.status;
    if (e.kind === 'sql' && c) title = 'SQL: ' + c.sql.split('\n')[0];
    box.appendChild(el('summary', '', title));
    if (e.kind === 'http' && c) {
      add(box, el('h5', '', 'Request'), headers(c.request && c.request.headers), body(c.request && c.request.body),
        el('h5', '', 'Response ' + c.status + (c.duration_us ? ' · ' + fmtDuration(c.duration_us) : '')),
        headers(c.response && c.response.headers), body(c.response && c.response.body));
    } else if (e.kind === 'sql' && c) {
      add(box, el('pre', '', c.sql), el('pre', '', 'binds: ' + JSON.stringify(c.binds)));
    } else if (e.path && /^image\//.test(e.media_type || '')) {
      const img = el('img'); img.src = link(e.path); img.alt = e.title || e.path; img.loading = 'lazy';
      const a = el('a', '', e.path); a.href = link(e.path);
      add(box, img, a, mark(e.path));
    } else if (e.path) {
      const a = el('a', '', e.path + ' (' + e.size + ' bytes)'); a.href = link(e.path);
      add(box, a, mark(e.path), c !== undefined ? body(c) : null);
    } else if (typeof c === 'string') {
      add(box, body(c));
    } else if (c && c.path) {
      add(box, body(c));
    } else if (c !== undefined) {
      add(box, el('pre', '', JSON.stringify(c, null, 2)));
    }
    return box;
  }
  function stepLine(item) {
    const line = el('div', 'line');
    add(line, el('span', 'badge ' + (item.status || ''), item.status || ''),
      el('span', 'kw', item.keyword || ''), el('span', 'txt', item.text || ''),
      el('span', 'dur', fmtDuration(item.duration_us)));
    return line;
  }
  function renderItems(items, parent, failureHolder, failure) {
    items.forEach(function (item) {
      if (item.kind === 'section') {
        const h = el('div', 'section');
        add(h, el('h4', '', item.title));
        if (item.note) h.appendChild(paragraphs(item.note));
        parent.appendChild(h);
        return;
      }
      const hasInner = item.children.length || item.evidence.length || item.attempts.length;
      const wrap = el(hasInner ? 'details' : 'div', 'step ' + (item.kind === 'disabled' ? 'disabled' : item.status || ''));
      if (hasInner) {
        // The failure text sits inside, so the path to a failed step starts open.
        if (item.status === 'failed') wrap.open = true;
        const summary = el('summary');
        summary.appendChild(stepLine(item));
        wrap.appendChild(summary);
      } else {
        wrap.appendChild(stepLine(item));
      }
      if (item.kind === 'disabled') wrap.firstChild.querySelector('.badge').textContent = 'disabled';
      if (item.comment) add(wrap, el('div', 'comment', item.comment));
      item.warnings.forEach(function (w) { add(wrap, el('div', 'warning', w)); });
      if (item.vars) {
        const vars = el('div', 'vars');
        item.vars.forEach(function (v) { vars.appendChild(el('div', '', '<<' + v[0] + '>> = ' + (v[1] === null ? 'null' : v[1]))); });
        wrap.appendChild(vars);
      }
      if (failure && failureHolder.item === item) add(wrap, el('pre', 'failure', failure));
      item.attempts.forEach(function (a) { add(wrap, el('div', 'attempt', 'attempt ' + a.n + ': ' + a.message)); });
      item.evidence.forEach(function (e) { wrap.appendChild(evidence(e)); });
      if (item.children.length) {
        const inner = el('div', 'children');
        renderItems(item.children, inner, failureHolder, failure);
        wrap.appendChild(inner);
      }
      parent.appendChild(wrap);
    });
  }
  function element(label, e) {
    if (!e) return null;
    const box = el('div', 'element');
    add(box, el('strong', '', label + (e.name ? ': ' + e.name : '')));
    if (e.description) box.appendChild(paragraphs(e.description));
    if (e.comment) box.appendChild(el('div', 'comment', e.comment));
    return box;
  }
  function scenarioBody(file, s) {
    const box = el('div', 'scenario-body');
    if (s.rule) add(box, element('Rule', s.rule), s.rule.background ? element('Rule background', s.rule.background) : null);
    if (s.examples) box.appendChild(element('Examples', s.examples));
    if (s.description) box.appendChild(paragraphs(s.description));
    if (s.comment) box.appendChild(el('div', 'comment', s.comment));
    const steps = el('div', 'steps');
    const holder = { item: null };
    s.items.forEach(function (i) { if (i.kind === 'step' && i.status === 'failed') holder.item = i; });
    renderItems(s.items, steps, holder, s.failure);
    box.appendChild(steps);
    if (s.failure && !holder.item) box.appendChild(el('pre', 'failure', s.failure));
    if (s.status === 'open') box.appendChild(el('div', 'warning', 'The stream ended inside this scenario.'));
    return box;
  }

  // ---- files and scenarios ----------------------------------------------
  const haystacks = new Map();
  function haystack(file, s) {
    let h = haystacks.get(s);
    if (h === undefined) {
      const parts = [file.file, file.name, s.name, s.description || ''];
      (function walk(items) { items.forEach(function (i) { parts.push(i.text || '', i.title || ''); walk(i.children); }); })(s.items);
      h = parts.join('\n').toLowerCase();
      haystacks.set(s, h);
    }
    return h;
  }
  function matches(file, s) {
    if (state.status === 'failed' && !isBad(s)) return false;
    if (state.status === 'passed' && s.status !== 'passed') return false;
    if (state.tag && s.tags.indexOf(state.tag) < 0) return false;
    if (state.q && haystack(file, s).indexOf(state.q.toLowerCase()) < 0) return false;
    return true;
  }
  function render() {
    list.textContent = '';
    let shown = 0;
    run.files.forEach(function (file, fi) {
      const visible = [];
      file.scenarios.forEach(function (s, si) { if (matches(file, s)) visible.push([s, si]); });
      const unfiltered = state.status !== 'passed' && !state.q && !state.tag;
      if (!visible.length && !(file.panicked && unfiltered)) return;
      shown += visible.length;
      const section = el('section', 'file-block');
      const h = el('h2', '', file.file);
      if (file.name) h.appendChild(el('span', 'sub', ' ' + file.name));
      if (file.panicked) h.appendChild(el('span', 'badge failed', 'panicked'));
      add(section, h, tagList(file.tags));
      if (file.panicked) section.appendChild(el('div', 'warning', 'This file panicked: the run went on, but the scenarios it had not reached are not in the stream.'));
      if (file.description) section.appendChild(paragraphs(file.description));
      if (file.comment) section.appendChild(el('div', 'comment', file.comment));
      add(section, element('Background', file.background));
      visible.forEach(function (pair) {
        const s = pair[0], id = fi + ':' + pair[1];
        const d = el('details', 'scenario ' + s.status);
        d.id = 's' + id;
        const sum = el('summary');
        add(sum, el('span', 'badge ' + s.status, s.status), el('span', 'name', s.name), tagList(s.tags),
          el('span', 'dur', fmtDuration(s.duration_us)));
        d.appendChild(sum);
        let built = false;
        function build() { if (!built) { built = true; d.appendChild(scenarioBody(file, s)); } }
        // The URL follows what the person clicks, not what the page opens by itself.
        sum.addEventListener('click', function () {
          state.s = d.open ? '' : id;
          writeHash();
        });
        d.addEventListener('toggle', function () { if (d.open) build(); });
        // Built here, not left to `toggle`: a details opened before it is in the page may never fire it.
        if (isBad(s) || state.s === id) { d.open = true; build(); }
        section.appendChild(d);
      });
      list.appendChild(section);
    });
    counter.textContent = shown + ' scenarios';
    if (!shown) list.appendChild(el('p', 'empty', 'No scenarios match.'));
    const target = state.s && document.getElementById('s' + state.s);
    if (target) target.scrollIntoView();
  }
  function changed() {
    state.status = status.value; state.q = search.value; state.tag = tagSel.value; state.s = '';
    writeHash(); render();
  }
  status.onchange = tagSel.onchange = changed;
  search.oninput = changed;
  window.addEventListener('hashchange', function () { readHash(); status.value = state.status; search.value = state.q; tagSel.value = state.tag; render(); });
  render();
})();
