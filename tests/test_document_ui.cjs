'use strict';

// These tests execute the shipped scripts and native-generated artifacts in a DOM.
// They model message delivery, layout visibility, focus intent and downloads. They
// do not claim native browser sandbox/CSP, rendering or download verification.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const path = require('node:path');
const vm = require('node:vm');
const { webcrypto } = require('node:crypto');
const { parseHTML } = require('./document-support/node_modules/linkedom');

const root = path.resolve(__dirname, '..');
const generator = path.join(__dirname, 'document_ui_fixture.py');
const python = process.env.PYTHON || 'python3';

function runPython(args = [], input) {
  const result = spawnSync(python, [generator, ...args], { cwd: root, encoding: 'utf8', input, maxBuffer: 8 * 1024 * 1024 });
  if (result.error) throw result.error;
  assert.equal(result.status, 0, result.stderr);
  return JSON.parse(result.stdout);
}
const fixture = runPython();

function domEnvironment(html) {
  const { document, window } = parseHTML(html);
  const listeners = new Map();
  Object.defineProperty(document, 'readyState', { value: 'loading', configurable: true });
  Object.defineProperty(document, 'activeElement', { get() { return document._testFocused || document.body; }, configurable: true });
  window.HTMLElement.prototype.focus = function () { this.ownerDocument._testFocused = this; };
  window.HTMLElement.prototype.scrollIntoView = function () { this._testScrolled = true; };
  window.HTMLElement.prototype.getBoundingClientRect = function () {
    const viewport = this.ownerDocument._testViewport;
    const box = this._testBounds || (this.id === 'kp-document' ? { left: 0, top: 0, width: viewport.innerWidth, height: viewport.innerHeight } :
      this.id === 'kp-panel' ? { left: Number.parseFloat(this.style.left) || 0, top: Number.parseFloat(this.style.top) || 0,
        width: Math.min(Number.parseFloat(this.style.width) || 440, viewport.innerWidth - 24), height: Math.min(Number.parseFloat(this.style.height) || this._testHeight || 320, Number.parseFloat(this.style.maxHeight) || Infinity) } :
      { left: 140, top: 200, width: 120, height: 24 });
    return { ...box, right: box.left + box.width, bottom: box.top + box.height };
  };
  function computedStyle(element) {
    const result = { display: 'block', visibility: 'visible', opacity: '1', contentVisibility: 'visible', fontSize: '14px' };
    const properties = { display: 'display', visibility: 'visibility', opacity: 'opacity', 'content-visibility': 'contentVisibility', 'font-size': 'fontSize' };
    function apply(style) {
      for (const [css, js] of Object.entries(properties)) if (style.getPropertyValue(css)) result[js] = style.getPropertyValue(css);
    }
    // This is a bounded layout model, not a CSS engine: use the actual matching
    // simple rules in these fixtures, followed by their inline styles.
    for (const sheet of document.querySelectorAll('style')) {
      for (const rule of sheet.sheet.cssRules) {
        // Pseudo-elements are not the element itself. css-select cannot match
        // these selectors; this bounded model excludes their separate box styles.
        if (rule.selectorText && !rule.selectorText.includes('::') && Object.keys(properties).some(name => rule.style.getPropertyValue(name)) &&
            element.matches(rule.selectorText)) apply(rule.style);
      }
    }
    apply(element.style);
    return result;
  }
  window.HTMLElement.prototype.getClientRects = function () {
    if (this._testRects !== undefined) return this._testRects;
    if (this.ownerDocument._testComputedStyle(this).display === 'contents' || this.textContent === '') return [];
    return [{ width: 20, height: 12 }];
  };
  document._testComputedStyle = computedStyle;
  document.createRange = () => {
    let selected;
    return {
      selectNodeContents(element) { selected = element; },
      getClientRects() {
        assert.ok(selected, 'the content range must select an element');
        document._testRangeReads = (document._testRangeReads || 0) + 1;
        if (selected._testRangeRects !== undefined) return selected._testRangeRects;
        if (selected._testRects !== undefined) return selected._testRects;
        if (selected.textContent === '') return [];
        // A display:contents element has no box, but its rendered text does.
        return [{ width: 20, height: 12 }];
      }
    };
  };
  const sandbox = {
    document, console, crypto: webcrypto, Uint8Array, Blob, URL, Date,
    setTimeout, clearTimeout, innerWidth: 1200, innerHeight: 800,
    ResizeObserver: class {
      constructor(callback) { this.callback = callback; }
      observe(node) { node._testResize = this.callback; }
      disconnect() {}
    },
    getComputedStyle: computedStyle,
    addEventListener(type, handler) {
      if (!listeners.has(type)) listeners.set(type, []);
      listeners.get(type).push(handler);
      if (type !== 'message') window.addEventListener(type, handler);
    },
    scrollTo(options) {
      document._testWindowScroll = options;
    }
  };
  sandbox.window = sandbox;
  document._testViewport = sandbox;
  const context = vm.createContext(sandbox);
  return {
    document, sandbox, context, listeners,
    event(type, extra = {}) {
      const event = new window.Event(type, { bubbles: true, cancelable: true });
      Object.assign(event, extra);
      return event;
    },
    receive(message, source) { for (const handler of listeners.get('message') || []) handler({ data: structuredClone(message), source }); },
    fireWindow(type, extra = {}) { for (const handler of listeners.get(type) || []) handler({ type, ...extra }); },
    execute(script) { return vm.runInContext(script, context, { timeout: 1000 }); },
    ready() {
      Object.defineProperty(document, 'readyState', { value: 'complete', configurable: true });
      document.dispatchEvent(new window.Event('DOMContentLoaded'));
    }
  };
}

function openArtifact(html = fixture.html, options = {}) {
  const host = domEnvironment(html);
  host.frames = [];
  host.messages = [];
  host.held = [];
  host.blobs = [];
  host.timers = new Map();
  host.timerId = 0;
  host.holdProbes = false;
  host.holdReady = options.holdReady || false;
  host.iframe = host.document.getElementById('kp-document');
  host.panel = host.document.getElementById('kp-panel');
  host.opener = host.document.getElementById('kp-open');
  host.payload = () => JSON.parse(host.document.getElementById('kp-data').textContent);
  host.notice = () => host.document.getElementById('kp-notice').textContent;
  host.group = id => host.payload().groups.find(group => group.members.includes(id));
  host.control = (id, action) => Array.from(host.panel.querySelectorAll('[data-group]')).find(node => node.dataset.group === host.group(id).id)
    .querySelector('[data-action="' + action + '"]');
  host.choose = (id, action) => { const control = host.control(id, action); assert.ok(control, action); assert.equal(control.disabled, false); control.click(); };
  host.emit = message => host.receive(message, host.frameWindow);
  host.fireTimers = milliseconds => {
    for (const [id, item] of Array.from(host.timers)) {
      if (item.ms <= milliseconds) { host.timers.delete(id); item.fn(); }
    }
  };
  host.sandbox.setTimeout = (fn, ms) => { const id = ++host.timerId; host.timers.set(id, { fn, ms }); return id; };
  host.sandbox.clearTimeout = id => host.timers.delete(id);
  host.sandbox.URL = class extends URL {
    static createObjectURL(blob) { host.blobs.push(blob); return 'blob:offline-test/' + host.blobs.length; }
    static revokeObjectURL() {}
  };
  host.frameWindow = {
    postMessage(message) {
      host.messages.push({ direction: 'to-frame', message: structuredClone(message) });
      host.frame.receive(message, host.frame.parentProxy);
    }
  };
  Object.defineProperty(host.iframe, 'contentWindow', { value: host.frameWindow });
  Object.defineProperty(host.iframe, 'srcdoc', {
    get() { return host.currentSrcdoc || ''; },
    set(value) {
      assert.ok(host.listeners.has('message'), 'parent installs listener before srcdoc');
      if (host.rejectNextSrcdoc) { host.rejectNextSrcdoc = false; throw new Error('Simulated srcdoc assignment failure'); }
      host.currentSrcdoc = value;
      const frame = domEnvironment(value);
      frame.parentProxy = {
        postMessage(message) {
          const record = { direction: 'from-frame', message: structuredClone(message), source: host.frameWindow };
          host.messages.push(record);
          if (message.type === 'kp:probe-result' && host.holdProbes || message.type === 'kp:ready' && host.holdReady) host.held.push(record);
          else host.receive(message, host.frameWindow);
        }
      };
      frame.sandbox.parent = frame.parentProxy;
      host.frame = frame;
      host.frames.push(frame);
      const scripts = Array.from(frame.document.querySelectorAll('script')).filter(node => !node.hasAttribute('type'));
      for (const script of scripts) frame.execute(script.textContent);
      frame.ready();
    }
  });
  for (const script of Array.from(host.document.querySelectorAll('script')).filter(node => !node.hasAttribute('type'))) host.execute(script.textContent);
  host.ready();
  host.nonce = () => host.messages.filter(record => record.direction === 'from-frame' && record.message.type === 'kp:ready').at(-1).message.nonce;
  host.anchor = id => Array.from(host.frame.document.querySelectorAll('[data-kpopper-claim]')).find(node => node.getAttribute('data-kpopper-claim') === id);
  host.pointer = (id, type, x = 180, y = 210, extra = {}) => host.anchor(id).dispatchEvent(host.frame.event(type,
    { pointerType: 'mouse', clientX: x, clientY: y, button: 0, detail: type === 'click' ? 1 : 0, ...extra }));
  host.selectedSource = () => {
    const text = host.currentSrcdoc;
    const start = text.indexOf('<meta http-equiv="Content-Security-Policy"');
    assert.ok(start >= 0);
    const end = text.indexOf('</script>', start) + '</script>'.length;
    return text.slice(0, start) + text.slice(end);
  };
  return host;
}

function withPayload(change, base = fixture.html) {
  const { document } = parseHTML(base);
  const element = document.getElementById('kp-data');
  const data = JSON.parse(element.textContent);
  change(data);
  element.textContent = JSON.stringify(data).replace(/</g, '\\u003c');
  return '<!doctype html>' + document.documentElement.outerHTML;
}

test('generated shell isolates author HTML and preserves style, script, Unicode and original source bytes', () => {
  const host = openArtifact();
  assert.equal(host.iframe.getAttribute('sandbox'), 'allow-scripts');
  assert.equal(host.selectedSource(), fixture.authored);
  assert.equal(host.frame.document.getElementById('author-style').textContent, 'body{background:#f3e8df;color:#243342}em{font-weight:700}');
  assert.equal(host.frame.sandbox.authorStarted, true);
  host.frame.document.getElementById('author-control').click();
  assert.equal(host.frame.sandbox.authorCount, 1);
  assert.equal(host.frame.document.getElementById('tutorial').textContent, '</script><script>tutorial()</script>');
  assert.equal(host.frame.document.getElementById('kp-panel'), null);
  assert.equal(host.anchor('registered').title, "Evidence: Registered readers $& $` $'");
  assert.match(host.currentSrcdoc, /Content-Security-Policy/);
  assert.ok(host.currentSrcdoc.indexOf('Content-Security-Policy') < host.currentSrcdoc.indexOf('author-style'));
});

test('source and reason attacks stay inert throughout the shell and its bridge configuration', () => {
  const host = openArtifact();
  assert.equal(host.sandbox.evidenceExecuted, undefined);
  assert.equal(host.frame.sandbox.evidenceExecuted, undefined);
  assert.equal(host.panel.querySelector('script'), null);
  assert.equal(host.panel.querySelector('img'), null);
  assert.ok(host.panel.textContent.includes(fixture.attack));
  assert.ok(!fixture.html.includes('NEVER EMBED THE UNSELECTED SECRET'));
  assert.equal(host.document.querySelectorAll('script').length, 3);
  const raw = host.document.getElementById('kp-data').textContent;
  assert.ok(!raw.includes('<'));
  assert.deepEqual(JSON.parse(raw).authored_html, fixture.authored);
});

test('coverage honesty, missing evidence, inference, revision changes and partial refresh are visible', () => {
  const host = openArtifact();
  const text = host.panel.textContent;
  for (const phrase of ['Unmarked text is not checked', 'not a measure of semantic coverage', '500',
    'Evidence unavailable', 'Reasoning needs human judgment', 'Source changed since the previous snapshot',
    'Not reread on this refresh', 'do not update by themselves', 'Agent-made extraction']) assert.ok(text.includes(phrase), phrase);
  const missing = host.group('missing');
  assert.equal(missing.status, 'blocked');
  assert.equal(host.control('missing', 'accept'), null);
  assert.equal(host.payload().checks.missing.expected, null);
  const zero = openArtifact(fixture.zero);
  assert.equal(zero.payload().coverage.unmarked_values, 0);
  assert.ok(zero.panel.textContent.includes('Unmarked text is not checked'));
});

test('acceptance changes the complete group with Python codepoint offsets and restarts authored runtime', () => {
  const host = openArtifact();
  host.frame.document.getElementById('author-control').click();
  const originalNonce = host.nonce();
  host.choose('registered', 'accept');
  assert.equal(host.group('registered').decision, 'accepted');
  assert.match(host.group('registered').decided_at, /^\d{4}-\d\d-\d\dT/);
  assert.equal(host.anchor('registered').textContent, '100');
  assert.equal(host.anchor('rate').textContent, '60%');
  assert.equal(host.selectedSource(), fixture.variants.counts);
  assert.notEqual(host.nonce(), originalNonce);
  assert.equal(host.frame.sandbox.authorCount, 0);
  assert.equal(host.anchor('registered').getAttribute('data-kpopper-mark'), 'match');
  assert.equal(host.group('books').decision, 'pending');
});

test('independent groups accept separately and resetting acceptance retains the other group', () => {
  const host = openArtifact();
  host.choose('registered', 'accept');
  host.choose('books', 'accept');
  assert.equal(host.selectedSource(), fixture.variants.both);
  host.choose('registered', 'reset');
  assert.equal(host.selectedSource(), fixture.variants.books);
  assert.equal(host.group('registered').decision, 'pending');
  assert.equal(host.group('registered').decided_at, null);
  assert.equal(host.group('books').decision, 'accepted');
  assert.equal(host.anchor('rate').getAttribute('data-kpopper-mark'), 'mismatch');
});

test('reject and reset rejected decisions preserve the exact original and mismatch marks', () => {
  const host = openArtifact();
  const frames = host.frames.length;
  host.choose('registered', 'reject');
  assert.equal(host.group('registered').decision, 'rejected');
  assert.equal(host.selectedSource(), fixture.authored);
  assert.equal(host.anchor('rate').getAttribute('data-kpopper-mark'), 'mismatch');
  host.choose('registered', 'reset');
  assert.equal(host.group('registered').decision, 'pending');
  assert.equal(host.frames.length, frames);
});

test('malicious proposed text is HTML-escaped and cannot become executable author content', () => {
  const host = openArtifact();
  host.choose('note', 'accept');
  assert.equal(host.anchor('note').textContent, fixture.attack);
  assert.equal(host.selectedSource(), fixture.variants.attack);
  assert.equal(host.frame.sandbox.evidenceExecuted, undefined);
  assert.equal(host.anchor('note').querySelector('script'), null);
  assert.equal(host.anchor('note').querySelector('img'), null);
});

for (const [name, mutate] of [
  ['text mutation', host => { host.anchor('rate').textContent = 'a changed value'; }],
  ['duplicate anchor', host => { host.anchor('rate').after(host.anchor('rate').cloneNode(true)); }],
  ['detached anchor', host => { host.anchor('rate').remove(); }],
  ['same-text replacement', host => { host.anchor('rate').replaceWith(host.anchor('rate').cloneNode(true)); }],
  ['hidden anchor', host => { host.anchor('rate').hidden = true; }],
  ['hidden ancestor', host => { host.anchor('rate').parentElement.style.display = 'none'; }],
  ['transparent ancestor', host => { host.anchor('rate').parentElement.style.opacity = '0'; }],
  ['no rendered box', host => { host.anchor('rate')._testRects = []; }]
]) {
  test('a fresh probe blocks atomic acceptance after ' + name, () => {
    const host = openArtifact();
    const before = host.payload();
    mutate(host);
    host.choose('registered', 'accept');
    assert.deepEqual(host.payload(), before);
    assert.equal(host.frames.length, 1);
    assert.match(host.notice(), /was not applied/);
    assert.equal(host.anchor('registered').textContent, '80');
  });
}

test('reset of an accepted group also requires a fresh valid probe', () => {
  const host = openArtifact();
  host.choose('registered', 'accept');
  host.anchor('rate').textContent = 'dynamic';
  const before = host.payload();
  host.choose('registered', 'reset');
  assert.deepEqual(host.payload(), before);
  assert.match(host.notice(), /was not applied/);
});

test('parent rejects forged sources, wrong nonces, malformed reports, stale and timed-out requests', () => {
  const host = openArtifact();
  host.holdProbes = true;
  host.choose('registered', 'accept');
  const first = host.held.at(-1).message;
  host.receive(first, {});
  host.emit({ ...first, nonce: 'wrong' });
  host.emit({ ...first, request: 'wrong' });
  host.emit({ ...first, type: 'unknown' });
  assert.equal(host.group('registered').decision, 'pending');
  assert.ok(host.control('registered', 'accept').disabled);
  host.fireTimers(5000);
  host.emit(first);
  assert.equal(host.group('registered').decision, 'pending');
  host.choose('registered', 'accept');
  const second = host.held.at(-1).message;
  assert.notEqual(second.request, first.request);
  host.emit(first);
  assert.equal(host.group('registered').decision, 'pending');
  host.emit({ ...second, observations: [second.observations[0], second.observations[0]] });
  assert.equal(host.group('registered').decision, 'pending');
  assert.match(host.notice(), /was not applied/);
  host.choose('registered', 'accept');
  const third = host.held.at(-1).message;
  host.emit(third);
  assert.equal(host.group('registered').decision, 'accepted');
  host.emit(third);
  assert.equal(host.frames.length, 2);
});

test('per-load nonce rejects old frame messages even when WindowProxy identity is reused', () => {
  const host = openArtifact();
  const oldNonce = host.nonce();
  host.holdReady = true;
  host.choose('registered', 'accept');
  assert.ok(host.control('books', 'accept').disabled);
  host.emit({ type: 'kp:ready', nonce: oldNonce });
  host.emit({ type: 'kp:open', nonce: oldNonce, id: 'books' });
  assert.ok(host.panel.hidden);
  assert.ok(host.control('books', 'accept').disabled);
  host.emit(host.held.at(-1).message);
  assert.equal(host.control('books', 'accept').disabled, false);
});

test('bridge rejects requests from another window or nonce and only accepts its finite protocol', () => {
  const host = openArtifact();
  const frame = host.frame;
  const before = host.messages.length;
  const message = { type: 'kp:probe', nonce: host.nonce(), request: 'request', ids: ['registered'] };
  frame.receive(message, {});
  frame.receive({ ...message, nonce: 'stale' }, frame.parentProxy);
  frame.receive({ ...message, type: 'execute' }, frame.parentProxy);
  frame.receive({ ...message, ids: ['registered', 'registered'] }, frame.parentProxy);
  frame.receive({ ...message, ids: ['unknown'] }, frame.parentProxy);
  assert.equal(host.messages.length, before);
  frame.receive(message, frame.parentProxy);
  assert.equal(host.messages.at(-1).message.type, 'kp:probe-result');
  assert.equal(host.messages.at(-1).message.ok, true);
});

test('invalid codepoint ranges, original slices and overlapping edits never apply', () => {
  for (const mutate of [
    data => { data.groups.find(group => group.members.includes('registered')).edits[0].start = -1; },
    data => { data.groups.find(group => group.members.includes('registered')).edits[0].end = 9999999; },
    data => { data.groups.find(group => group.members.includes('registered')).edits[0].before_raw = 'wrong'; },
    data => { const group = data.groups.find(group => group.members.includes('registered')); group.edits.push({ ...group.edits[0] }); }
  ]) {
    const host = openArtifact(withPayload(mutate));
    const before = host.payload();
    host.choose('registered', 'accept');
    assert.deepEqual(host.payload(), before);
    assert.equal(host.frames.length, 1);
    assert.match(host.notice(), /could not be reconstructed safely/);
  }
});

test('English and Hebrew panel controls support keyboard open, Escape close and focus return', () => {
  for (const [html, dir, title] of [[fixture.html, 'ltr', 'Evidence'], [fixture.hebrew, 'rtl', 'מקורות ובדיקות']]) {
    const host = openArtifact(html);
    assert.equal(host.panel.getAttribute('dir'), dir);
    assert.equal(host.opener.textContent, title);
    assert.equal(host.frame.document.documentElement.getAttribute('dir'), 'ltr');
    assert.equal(host.frame.document.documentElement.lang, 'en');
    host.opener.click();
    assert.equal(host.panel.hidden, false);
    assert.equal(host.opener.getAttribute('aria-expanded'), 'true');
    assert.equal(host.document.activeElement.dataset.kpClose, '');
    host.panel.dispatchEvent(host.event('keydown', { key: 'Escape' }));
    assert.equal(host.document.activeElement, host.opener);
    assert.equal(host.panel.hidden, true);
    const anchor = host.anchor('rate');
    const key = host.frame.event('keydown', { key: 'Enter' });
    anchor.dispatchEvent(key);
    assert.equal(key.defaultPrevented, true);
    assert.equal(host.panel.hidden, false);
    assert.equal(host.document.activeElement.id, 'kp-panel-title');
    assert.equal(host.document.activeElement.textContent, host.payload().claims.find(claim => claim.id === 'rate').label);
    host.panel.querySelector('[data-kp-close]').click();
    assert.equal(host.frame.document.activeElement, anchor);
    assert.equal(host.document.activeElement, host.iframe);
  }
});

test('citation links accept only HTTP(S) and isolate their opener', () => {
  const good = openArtifact();
  good.opener.click();
  good.panel.querySelector('[data-source="words"]').click();
  const citation = good.panel.querySelector('a[target="_blank"]');
  assert.equal(citation.href, 'https://example.test/citation');
  assert.equal(citation.rel, 'noopener noreferrer');
  for (const uri of ['javascript:window.evidenceExecuted=true', 'data:text/html,<script>alert(1)</script>', 'file:///private.txt']) {
    const host = openArtifact(withPayload(data => { data.sources.words.uri = uri; }));
    host.opener.click();
    host.panel.querySelector('[data-source="words"]').click();
    assert.equal(host.panel.querySelector('a[target="_blank"]'), null);
  }
});

test('download escapes the whole payload, removes transient DOM and reopens with the same decisions and snapshot', async () => {
  const host = openArtifact();
  host.opener.click();
  host.choose('registered', 'accept');
  host.choose('books', 'reject');
  host.choose('note', 'accept');
  host.frame.document.getElementById('author-control').click();
  host.frame.document.body.append(host.frame.document.createElement('hr'));
  host.panel.querySelector('[data-action="download"]').click();
  assert.equal(host.blobs.length, 1);
  const exported = await host.blobs[0].text();
  const { document } = parseHTML(exported);
  assert.equal(document.querySelectorAll('script').length, 3);
  assert.equal(document.getElementById('kp-document').getAttribute('srcdoc'), null);
  assert.equal(document.getElementById('kp-panel').textContent, '');
  assert.equal(document.getElementById('kp-panel').hidden, true);
  assert.equal(document.getElementById('kp-notice').textContent, '');
  for (const id of ['kp-data', 'kp-runtime']) assert.ok(!document.getElementById(id).textContent.includes('<'));
  const reopened = openArtifact(exported);
  assert.deepEqual(reopened.payload(), host.payload());
  assert.equal(reopened.group('registered').decision, 'accepted');
  assert.equal(reopened.group('books').decision, 'rejected');
  assert.equal(reopened.anchor('note').textContent, fixture.attack);
  assert.equal(reopened.frame.document.querySelector('hr'), null);
  assert.equal(reopened.frame.sandbox.authorCount, 0);
  assert.equal(reopened.sandbox.evidenceExecuted, undefined);
  assert.equal(reopened.frame.sandbox.evidenceExecuted, undefined);
  const validated = runPython(['--validate'], exported);
  assert.equal(validated.selected, reopened.selectedSource());
  assert.equal(validated.summary.sources.words.sha256, fixture.data.sources.words.sha256);
  assert.equal(validated.summary.checks.interpretation.status, 'unchecked');
  const reloadedOriginal = openArtifact();
  assert.equal(reloadedOriginal.group('registered').decision, 'pending');
  assert.equal(reloadedOriginal.selectedSource(), fixture.authored);
});

test('download is disabled while an atomic decision is awaiting a probe', () => {
  const host = openArtifact();
  host.holdProbes = true;
  host.choose('registered', 'accept');
  assert.ok(host.panel.querySelector('[data-action="download"]').disabled);
  host.panel.querySelector('[data-action="download"]').click();
  assert.equal(host.blobs.length, 0);
});

test('ordinary fragment links scroll within the authored frame without navigating to its embedding file', () => {
  const host = openArtifact();
  const frame = host.frame;
  const target = frame.document.getElementById('chapter-שלום');
  const link = frame.document.getElementById('toc-chapter');
  const originalMarkup = link.outerHTML;
  const click = frame.event('click', { button: 0 });
  link.querySelector('em').dispatchEvent(click);
  assert.equal(click.defaultPrevented, true);
  assert.equal(target._testScrolled, true);
  assert.equal(link.outerHTML, originalMarkup);
  assert.equal(host.frames.length, 1);
  for (const id of ['toc-empty', 'toc-top']) {
    delete frame.document._testWindowScroll;
    const event = frame.event('click', { button: 0 });
    frame.document.getElementById(id).dispatchEvent(event);
    assert.equal(event.defaultPrevented, true);
    assert.equal(frame.document._testWindowScroll.top, 0);
    assert.equal(frame.document._testWindowScroll.left, 0);
  }
  const missing = frame.event('click', { button: 0 });
  frame.document.getElementById('toc-missing').dispatchEvent(missing);
  assert.equal(missing.defaultPrevented, true);
  assert.equal(host.selectedSource(), fixture.authored);
});

test('fragment scrolling respects author-cancelled, modified and differently targeted clicks', () => {
  const host = openArtifact();
  const frame = host.frame;
  const target = frame.document.getElementById('chapter-שלום');
  const authored = frame.event('click', { button: 0 });
  frame.document.getElementById('toc-authored').dispatchEvent(authored);
  assert.equal(frame.sandbox.authorFragmentHandled, true);
  assert.equal(authored.defaultPrevented, true);
  assert.equal(target._testScrolled, undefined);
  for (const fields of [{ ctrlKey: true }, { metaKey: true }, { shiftKey: true }, { altKey: true }, { button: 1 }]) {
    const modified = frame.event('click', { button: 0, ...fields });
    frame.document.getElementById('toc-chapter').dispatchEvent(modified);
    assert.equal(modified.defaultPrevented, false);
    assert.equal(target._testScrolled, undefined);
  }
  for (const id of ['toc-other', 'toc-external']) {
    const click = frame.event('click', { button: 0 });
    frame.document.getElementById(id).dispatchEvent(click);
    assert.equal(click.defaultPrevented, false);
    assert.equal(target._testScrolled, undefined);
  }
});

test('intact passages inside closed disclosures give localized reveal-and-retry guidance', () => {
  for (const [name, phrase] of [['disclosure', 'Reveal its section or tab'], ['disclosure_he', 'להציג את הסעיף או הלשונית']]) {
    const host = openArtifact(fixture.layouts[name].html);
    const before = host.payload();
    host.choose('cap', 'accept');
    assert.deepEqual(host.payload(), before);
    assert.ok(host.notice().includes(phrase));
    assert.equal(host.anchor('main').textContent, '24');
    host.frame.document.getElementById('disclosure').open = true;
    host.choose('cap', 'accept');
    assert.equal(host.group('cap').decision, 'accepted');
    assert.equal(host.selectedSource(), fixture.layouts[name].selected);
  }
});

test('an inactive authored tab remains guarded until revealed', () => {
  const host = openArtifact(fixture.layouts.tab.html);
  host.choose('cap', 'accept');
  assert.equal(host.group('cap').decision, 'pending');
  assert.match(host.notice(), /currently hidden/);
  host.frame.document.getElementById('tab').classList.add('active');
  host.choose('cap', 'accept');
  assert.equal(host.group('cap').decision, 'accepted');
});

test('display:contents uses the text range boxes and accepts the exact proposal', () => {
  const host = openArtifact(fixture.layouts.contents.html);
  assert.equal(host.anchor('cap').getClientRects().length, 0);
  host.choose('cap', 'accept');
  assert.equal(host.group('cap').decision, 'accepted');
  assert.equal(host.selectedSource(), fixture.layouts.contents.selected);
  assert.ok(host.frames[0].document._testRangeReads > 0);
});

test('empty text anchors accept insertions after visible-ancestor checks, despite having no rect', () => {
  const host = openArtifact(fixture.layouts.empty.html);
  assert.equal(host.anchor('cap').getClientRects().length, 0);
  host.anchor('cap').parentElement.hidden = true;
  host.choose('cap', 'accept');
  assert.equal(host.group('cap').decision, 'pending');
  assert.match(host.notice(), /currently hidden/);
  host.anchor('cap').parentElement.hidden = false;
  host.choose('cap', 'accept');
  assert.equal(host.group('cap').decision, 'accepted');
  assert.equal(host.selectedSource(), fixture.layouts.empty.selected);
});

test('a hidden and modified passage is still treated as stale rather than a visibility-only retry', () => {
  const host = openArtifact(fixture.layouts.disclosure.html);
  host.anchor('cap').textContent = 'Modified';
  host.choose('cap', 'accept');
  assert.equal(host.group('cap').decision, 'pending');
  assert.match(host.notice(), /passage changed/);
  assert.doesNotMatch(host.notice(), /Reveal its section/);
});

test('frame preparation failure preserves the active channel and permits retry', () => {
  const host = openArtifact();
  const frame = host.frame;
  const nonce = host.nonce();
  const before = host.payload();
  host.execute(`{
    const replace = String.prototype.replace;
    String.prototype.replace = function (pattern, replacement) {
      if (pattern === '__KPOPPER_CONFIG__') {
        String.prototype.replace = replace;
        throw new Error('Simulated bridge preparation failure');
      }
      return replace.call(this, pattern, replacement);
    };
  }`);
  host.choose('registered', 'accept');
  assert.deepEqual(host.payload(), before);
  assert.equal(host.frame, frame);
  assert.equal(host.control('registered', 'accept').disabled, false);
  host.anchor('rate').click();
  assert.equal(host.panel.hidden, false);
  host.choose('registered', 'accept');
  assert.equal(host.group('registered').decision, 'accepted');
});

test('srcdoc assignment failure rolls back decisions and retains the prior frame, controls and retry', () => {
  const host = openArtifact();
  for (const action of ['accept', 'reset']) {
    const frame = host.frame;
    const nonce = host.nonce();
    const before = host.payload();
    host.frame.document.getElementById('author-control').click();
    const count = host.frame.sandbox.authorCount;
    host.rejectNextSrcdoc = true;
    host.choose('registered', action);
    assert.deepEqual(host.payload(), before);
    assert.equal(host.frame, frame);
    assert.equal(host.frame.sandbox.authorCount, count);
    assert.equal(host.control('registered', action).disabled, false);
    host.anchor('rate').click();
    assert.equal(host.panel.hidden, false);
    host.panel.querySelector('[data-kp-close]').click();
    assert.equal(host.frame.document.activeElement, host.anchor('rate'));
    host.choose('registered', action);
    assert.equal(host.group('registered').decision, action === 'accept' ? 'accepted' : 'pending');
  }
});

test('large escaped source values are applied as exact joined segments', () => {
  const host = openArtifact(fixture.layouts.large.html);
  const edit = host.group('cap').edits[0];
  assert.equal(edit.after_raw.length, 100000);
  host.choose('cap', 'accept');
  assert.equal(host.group('cap').decision, 'accepted');
  assert.equal(host.anchor('cap').textContent, '&'.repeat(20000));
  assert.equal(host.selectedSource(), fixture.layouts.large.selected);
});

test('a marked passage opens only its own explanation and links to the relevant source', () => {
  const host = openArtifact();
  host.anchor('registered').click();
  assert.equal(host.panel.hidden, false);
  assert.deepEqual([...host.panel.querySelectorAll('[data-claim]')].map(n => n.dataset.claim), ['registered']);
  assert.equal(host.panel.querySelector('h1').textContent, host.payload().claims.find(c => c.id === 'registered').label);
  assert.equal(host.panel.querySelector('.kp-counts'), null);
  assert.equal(host.panel.dataset.view, 'claim');
  assert.equal(host.panel.querySelector('[data-source-view]'), null);
  assert.equal(host.panel.querySelectorAll('[data-source]').length, 1);
  assert.equal(host.panel.querySelector('[data-source]').dataset.source, 'counts');
  assert.equal(host.panel.querySelectorAll('[data-group]').length, 1);
  assert.equal(host.panel.querySelector('[data-group]').dataset.group, host.group('registered').id);
  host.anchor('books').click();
  assert.deepEqual([...host.panel.querySelectorAll('[data-claim]')].map(n => n.dataset.claim), ['books']);
  assert.equal(host.panel.querySelector('[data-source="counts"]'), null);
});

test('all evidence is an explicit choice after opening one marked passage', () => {
  const host = openArtifact();
  host.anchor('rate').click();
  host.panel.querySelector('[data-action="overview"]').click();
  assert.equal(host.panel.hidden, false);
  assert.equal(host.panel.dataset.view, 'overview');
  assert.equal(host.panel.querySelectorAll('[data-claim]').length, host.payload().claims.length);
  assert.ok(host.panel.querySelector('.kp-counts'));
  host.panel.querySelector('[data-kp-close]').click();
  host.opener.click();
  assert.equal(host.panel.querySelectorAll('[data-claim]').length, host.payload().claims.length);
});

test('clicking outside closes without stealing focus or preventing authored interaction', () => {
  const host = openArtifact();
  const before = host.payload();
  host.anchor('rate').click();
  const control = host.frame.document.getElementById('author-control');
  control.focus();
  control.click();
  assert.equal(host.panel.hidden, true);
  assert.equal(host.frame.sandbox.authorCount, 1);
  assert.equal(host.frame.document.activeElement, control);
  assert.deepEqual(host.payload(), before);
  host.opener.click();
  host.panel.querySelector('h1').click();
  assert.equal(host.panel.hidden, false);
  host.document.body.click();
  assert.equal(host.panel.hidden, true);
});

test('outside dismissal accepts only the current authored frame and nonce', () => {
  const host = openArtifact();
  host.anchor('rate').click();
  const message = {type: 'kp:dismiss', nonce: host.nonce()};
  host.receive(message, {});
  assert.equal(host.panel.hidden, false);
  host.emit({...message, nonce: 'wrong'});
  assert.equal(host.panel.hidden, false);
  host.emit(message);
  assert.equal(host.panel.hidden, true);
});

test('close control stays in a fixed header outside the panel scrolling region', () => {
  const host = openArtifact();
  host.anchor('rate').click();
  const close = host.panel.querySelector('[data-kp-close]');
  const scroller = host.panel.querySelector('.kp-panel-content');
  assert.ok(scroller);
  assert.equal(close.closest('.kp-header').parentElement, host.panel);
  assert.equal(scroller.contains(close), false);
  scroller.scrollTop = 400;
  close.click();
  assert.equal(host.panel.hidden, true);
  assert.equal(host.frame.document.activeElement, host.anchor('rate'));
});

test('focused review keeps its context across decisions and exports the full intact snapshot', async () => {
  const host = openArtifact();
  host.anchor('rate').click();
  host.choose('rate', 'accept');
  assert.deepEqual([...host.panel.querySelectorAll('[data-claim]')].map(n => n.dataset.claim), ['rate']);
  assert.equal(host.panel.querySelector('[data-claim="rate"]').textContent.includes('60%'), true);
  host.panel.querySelector('[data-action="download"]').click();
  assert.equal(host.panel.hidden, false);
  const exported = await host.blobs[0].text();
  const reopened = openArtifact(exported);
  assert.deepEqual(reopened.payload(), host.payload());
  assert.equal(reopened.panel.hidden, true);
  assert.equal(reopened.anchor('rate').textContent, '60%');
  host.choose('rate', 'reset');
  assert.deepEqual([...host.panel.querySelectorAll('[data-claim]')].map(n => n.dataset.claim), ['rate']);
});

test('contextual evidence follows pointer coordinates, flips at corners and translates iframe offset/scale', () => {
  const host = openArtifact();
  for (const [x, y] of [[20, 20], [1180, 20], [20, 780], [1180, 780]]) {
    host.pointer('rate', 'click', x, y);
    const rect = host.panel.getBoundingClientRect();
    assert.ok(rect.left >= 12 && rect.right <= 1188);
    assert.ok(rect.top >= 12 && rect.bottom <= 788);
    assert.ok(Math.min(Math.abs(rect.left - x), Math.abs(rect.right - x)) <= 12);
    assert.ok(Math.min(Math.abs(rect.top - y), Math.abs(rect.bottom - y)) <= 12);
  }
  host.iframe._testBounds = { left: 80, top: 50, width: 600, height: 400 };
  host.pointer('rate', 'click', 200, 240);
  assert.equal(Number.parseFloat(host.panel.style.left), 192);
  assert.equal(Number.parseFloat(host.panel.style.top), 182);
});

test('keyboard placement follows the anchor and updates on frame scroll, resize and content growth', () => {
  const host = openArtifact();
  host.anchor('rate')._testBounds = { left: 500, top: 500, width: 100, height: 20 };
  host.anchor('rate').dispatchEvent(host.frame.event('keydown', { key: 'Enter' }));
  assert.equal(Number.parseFloat(host.panel.style.left), 500);
  assert.equal(Number.parseFloat(host.panel.style.top), 172);
  host.anchor('rate')._testBounds.top = 80;
  host.frame.fireWindow('scroll');
  assert.equal(Number.parseFloat(host.panel.style.top), 108);
  host.panel._testHeight = 740;
  host.panel.querySelector('[data-technical]').dispatchEvent(host.event('toggle'));
  assert.ok(host.panel.getBoundingClientRect().bottom <= 788);
  host.sandbox.innerWidth = 700;
  host.sandbox.innerHeight = 500;
  host.frame.sandbox.innerWidth = 700;
  host.frame.sandbox.innerHeight = 500;
  host.fireWindow('resize');
  assert.ok(host.panel.getBoundingClientRect().right <= 688);
  assert.ok(host.panel.getBoundingClientRect().bottom <= 488);
  host.panel._testHeight = 200;
  host.panel._testResize();
  assert.equal(Number.parseFloat(host.panel.style.top), 108);
});

test('hover previews delay without stealing focus, permit pointer travel and click pins the same view', () => {
  const host = openArtifact();
  host.opener.focus();
  host.pointer('rate', 'pointerover');
  assert.equal(host.panel.hidden, true);
  host.fireTimers(250);
  assert.equal(host.panel.hidden, false);
  assert.equal(host.panel.dataset.pinned, 'false');
  assert.equal(host.document.activeElement, host.opener);
  host.pointer('rate', 'pointerout');
  host.panel.dispatchEvent(host.event('pointerenter'));
  host.fireTimers(450);
  assert.equal(host.panel.hidden, false);
  host.panel.dispatchEvent(host.event('pointerleave'));
  host.fireTimers(450);
  assert.equal(host.panel.hidden, true);
  host.pointer('rate', 'pointerover');
  host.fireTimers(250);
  host.pointer('rate', 'click');
  assert.equal(host.panel.dataset.pinned, 'true');
  host.pointer('rate', 'pointerout');
  host.pointer('books', 'pointerover');
  host.fireTimers(450);
  assert.equal(host.panel.dataset.view, 'claim');
  assert.equal(host.panel.querySelector('[data-claim]').dataset.claim, 'rate');
  assert.equal(host.panel.hidden, false);
  host.frame.document.body.dispatchEvent(host.frame.event('keydown', { key: 'Escape' }));
  assert.equal(host.panel.hidden, true);
});

test('touch and cancelled hover do not create transient previews', () => {
  const host = openArtifact();
  host.pointer('rate', 'pointerover', 180, 210, { pointerType: 'touch' });
  host.fireTimers(450);
  assert.equal(host.panel.hidden, true);
  host.pointer('rate', 'pointerover');
  host.pointer('rate', 'pointerout');
  host.fireTimers(450);
  assert.equal(host.panel.hidden, true);
  host.pointer('rate', 'click', 180, 210, { pointerType: 'touch' });
  assert.equal(host.panel.hidden, false);
  assert.equal(host.panel.dataset.pinned, 'true');
});

test('a source opens separately with only this claim’s readings, Back and fresh collapsed technical details', () => {
  const host = openArtifact();
  host.pointer('registered', 'pointerover');
  host.fireTimers(250);
  assert.equal(host.panel.querySelector('[data-source-view]'), null);
  assert.equal(host.panel.querySelectorAll('[data-technical]').length, 1);
  host.panel.querySelector('[data-technical]').open = true;
  host.panel.querySelector('[data-source="counts"]').click();
  assert.equal(host.panel.dataset.view, 'source');
  assert.equal(host.panel.dataset.pinned, 'true');
  assert.equal(host.panel.querySelector('[data-claim]'), null);
  assert.equal(host.panel.querySelectorAll('.kp-selection').length, 1);
  assert.ok(host.panel.textContent.includes('100'));
  assert.ok(!host.panel.textContent.includes('/finished'));
  assert.equal(host.panel.querySelector('[data-technical]').open, false);
  assert.equal(host.panel.querySelectorAll('details details').length, 0);
  host.panel.querySelector('[data-action="back"]').click();
  assert.equal(host.panel.dataset.view, 'claim');
  assert.equal(host.panel.querySelector('[data-claim]').dataset.claim, 'registered');
  assert.equal(host.panel.querySelector('[data-technical]').open, false);
  host.panel.querySelector('[data-action="overview"]').click();
  host.panel.querySelector('[data-source="words"]').click();
  assert.equal(host.panel.dataset.view, 'source');
  host.panel.querySelector('[data-action="back"]').click();
  assert.equal(host.panel.dataset.view, 'overview');
  assert.equal(host.panel.style.left || '', '');
});

test('claim prose explains the saved comparison/calculation before technical identifiers', () => {
  const host = openArtifact();
  host.pointer('rate', 'click');
  const claim = host.panel.querySelector('[data-claim]');
  const technical = claim.querySelector('[data-technical]');
  assert.ok(technical);
  assert.equal(technical.open, false);
  const plain = claim.cloneNode(true);
  plain.querySelector('[data-technical]').remove();
  assert.match(plain.textContent, /saved calculation/i);
  assert.ok(plain.textContent.includes('60 ÷ 100'));
  assert.ok(plain.textContent.includes('60%'));
  assert.ok(!plain.textContent.includes('75%'));
  assert.ok(!plain.textContent.includes('/registered'));
  assert.ok(!plain.textContent.includes('ratio'));
  assert.equal(claim.querySelector('[data-source-view]'), null);
  assert.equal(claim.querySelectorAll('article article,details details').length, 0);
});

for (const language of ['en', 'he']) {
  test('claim explanations omit the clicked text and matching value repetition (' + language + ')', () => {
    const host = openArtifact(language === 'he' ? fixture.hebrew : fixture.html);
    for (const id of ['stable', 'registered', 'rate', 'interpretation', 'missing']) {
      host.pointer(id, 'click');
      const card = host.panel.querySelector('[data-claim]');
      assert.doesNotMatch(card.textContent, /The document says|במסמך כתוב/);
      if (id === 'stable') {
        assert.equal(card.querySelectorAll('.kp-reading').length, 0);
        assert.ok(card.querySelector('[data-source="stable"]'));
      } else if (id === 'registered') {
        assert.ok(card.querySelector('.kp-reading').textContent.includes('100'));
      } else if (id === 'interpretation') {
        assert.ok(card.textContent.includes('Participation alone does not establish satisfaction.'));
        assert.ok(!card.textContent.includes('build a reading habit'));
      }
    }
  });
}

test('geometry and hover messages require the active source/nonce and finite dimensions', () => {
  const host = openArtifact();
  host.pointer('rate', 'click');
  const valid = host.messages.filter(record => record.message.type === 'kp:open').at(-1).message;
  host.panel.querySelector('[data-kp-close]').click();
  host.receive(valid, {});
  host.emit({ ...valid, nonce: 'wrong' });
  host.emit({ ...valid, geometry: { ...valid.geometry, x: Infinity } });
  host.emit({ ...valid, geometry: { ...valid.geometry, viewport: { width: 0, height: 800 } } });
  assert.equal(host.panel.hidden, true);
  host.emit({ ...valid, type: 'kp:hover', nonce: 'wrong' });
  host.fireTimers(450);
  assert.equal(host.panel.hidden, true);
});

test('contextual route, pin and position are absent from the exported shell', async () => {
  const host = openArtifact();
  host.pointer('rate', 'click', 600, 300);
  host.panel.querySelector('[data-source="counts"]').click();
  const before = host.payload();
  host.panel.querySelector('[data-action="back"]').click();
  host.panel.querySelector('[data-action="download"]').click();
  const html = await host.blobs[0].text();
  const { document } = parseHTML(html);
  const saved = document.getElementById('kp-panel');
  for (const attribute of ['style', 'data-pinned', 'data-view']) assert.equal(saved.getAttribute(attribute), null);
  assert.equal(saved.textContent, '');
  const reopened = openArtifact(html);
  assert.deepEqual(reopened.payload(), before);
  assert.equal(reopened.panel.hidden, true);
  assert.equal(reopened.panel.dataset.view, 'overview');
});

test('raw unavailable-check diagnostics stay under the closed technical disclosure', () => {
  const host = openArtifact();
  host.anchor('missing').click();
  const card = host.panel.querySelector('[data-claim="missing"]');
  const technical = card.querySelector('[data-technical]');
  const diagnostic = host.payload().checks.missing.detail;
  assert.equal(technical.open, false);
  assert.ok(technical.textContent.includes(diagnostic));
  const visible = card.cloneNode(true);
  visible.querySelector('[data-technical]').remove();
  assert.ok(!visible.textContent.includes(diagnostic));
});

test('programmatically focused titles have no outline while keyboard controls retain one', () => {
  const host = openArtifact();
  host.pointer('rate', 'click');
  assert.equal(host.document.activeElement.id, 'kp-panel-title');
  const rules = [...host.document.querySelectorAll('style')].flatMap(style => [...style.sheet.cssRules]);
  assert.equal(rules.find(rule => rule.selectorText === '#kp-panel-title:focus')?.style.getPropertyValue('outline'), 'none');
  assert.ok(rules.some(rule => rule.selectorText?.includes('button:focus-visible') && rule.style.getPropertyValue('outline') !== 'none'));
});

test('source navigation and Back retain the same outer box while content grows or shrinks', () => {
  const host = openArtifact();
  host.pointer('rate', 'click', 620, 460);
  const box = host.panel.getBoundingClientRect();
  host.panel.querySelector('[data-source="counts"]').click();
  host.panel._testHeight = 700;
  host.panel._testResize();
  assert.deepEqual(host.panel.getBoundingClientRect(), box);
  assert.equal(host.panel.dataset.view, 'source');
  host.panel.querySelector('[data-action="back"]').click();
  host.panel._testHeight = 180;
  host.panel._testResize();
  assert.deepEqual(host.panel.getBoundingClientRect(), box);
  assert.equal(host.panel.dataset.view, 'claim');
});

test('internal disclosures retain the box, viewport shrink clamps it, and a new passage resets it', () => {
  const host = openArtifact();
  host.pointer('rate', 'click', 160, 210);
  const box = host.panel.getBoundingClientRect();
  host.panel.querySelector('[data-technical] summary').click();
  host.panel._testHeight = 700;
  host.panel.querySelector('[data-technical]').dispatchEvent(host.event('toggle'));
  assert.deepEqual(host.panel.getBoundingClientRect(), box);
  host.sandbox.innerWidth = 350; host.sandbox.innerHeight = 250;
  host.fireWindow('resize');
  assert.ok(host.panel.getBoundingClientRect().right <= 338);
  assert.ok(host.panel.getBoundingClientRect().bottom <= 238);
  host.sandbox.innerWidth = 1200; host.sandbox.innerHeight = 800;
  host.panel._testHeight = 220;
  host.pointer('books', 'click', 170, 150);
  assert.equal(host.panel.style.height || '', '');
  assert.equal(host.panel.getBoundingClientRect().height, 220);
});

test('source views explain evidence without document-saving controls or an empty footer', () => {
  const host = openArtifact();
  host.pointer('rate', 'click');
  host.panel.querySelector('[data-source="counts"]').click();
  assert.equal(!!host.panel.querySelector('[data-action="download"]'), false);
  assert.equal(!!host.panel.querySelector('.kp-footer'), false);
  const source = host.panel.querySelector('[data-source-view]');
  assert.ok(source.querySelector('.kp-badge'));
  assert.equal(source.querySelector('[data-technical]').open, false);
  host.panel.querySelector('[data-action="back"]').click();
  assert.ok(host.panel.querySelector('[data-action="download"]'));
  host.panel.querySelector('[data-action="overview"]').click();
  host.panel.querySelector('[data-source="words"]').click();
  assert.equal(!!host.panel.querySelector('[data-action="download"]'), false);
});

test('document saving uses a neutral label rather than implying the document was checked', () => {
  for (const [html,label] of [[fixture.html,'Save document copy'],[fixture.hebrew,'שמירת עותק המסמך']]) {
    const host = openArtifact(html);
    host.opener.click();
    assert.equal(host.panel.querySelector('[data-action="download"]').textContent,label);
  }
});
