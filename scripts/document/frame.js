(function () {
  'use strict';
  const config = __KPOPPER_CONFIG__;
  const claims = new Map(config.claims.map(claim => [claim.id, claim]));
  const original = new Map();
  let initialized = false;
  let tracking = null;
  const locations = new Map();

  function send(type, fields) {
    parent.postMessage(Object.assign({ type, nonce: config.nonce }, fields), '*');
  }

  function anchors(id) {
    // IDs never become selectors. Attribute values are compared as data.
    return Array.from(document.querySelectorAll('[data-kpopper-claim]'))
      .filter(element => element.getAttribute('data-kpopper-claim') === id);
  }

  function visible(element) {
    if (!element.isConnected) return false;
    for (let current = element; current; current = current.parentElement) {
      if (current.hidden || current.hasAttribute('inert') || current.getAttribute('aria-hidden') === 'true') return false;
      const style = getComputedStyle(current);
      if (style.display === 'none' || style.visibility === 'hidden' || style.visibility === 'collapse' ||
          style.contentVisibility === 'hidden' || Number.parseFloat(style.opacity) === 0 ||
          style.fontSize === '0px') return false;
      if (current.localName === 'details' && !current.open) {
        const summary = Array.from(current.children).find(child => child.localName === 'summary');
        if (!summary || !summary.contains(element)) return false;
      }
    }
    // An empty reading has no text box; its identity/text and visible ancestors
    // are still checked before an insertion is accepted.
    if (element.textContent === '') return true;
    const hasArea = rects => Array.from(rects).some(rect => rect.width > 0 && rect.height > 0);
    if (hasArea(element.getClientRects())) return true;
    // display:contents has no element box even though its text is rendered.
    const range = document.createRange();
    range.selectNodeContents(element);
    return hasArea(range.getClientRects());
  }

  function observe(ids) {
    return ids.map(id => {
      const matches = anchors(id);
      const element = matches.length === 1 ? matches[0] : null;
      return {
        id, count: matches.length, text: element ? element.textContent : null,
        connected: !!element && element.isConnected,
        same: !!element && original.get(id) === element,
        visible: !!element && visible(element)
      };
    });
  }

  function passes(observations) {
    return observations.every(item => item.count === 1 && item.connected && item.same && item.visible &&
      item.text === claims.get(item.id).text);
  }

  function markedTarget(event) {
    const target = event.target && event.target.closest ? event.target.closest('[data-kpopper-claim]') : null;
    return target && claims.has(target.getAttribute('data-kpopper-claim')) ? target : null;
  }

  function geometry(element, event) {
    const rect = element.getBoundingClientRect();
    const pointer = !!event && event.type !== 'keydown' && (event.type !== 'click' || event.detail > 0) &&
      Number.isFinite(event.clientX) && Number.isFinite(event.clientY);
    tracking = { element, pointer, dx: pointer ? event.clientX - rect.left : 0, dy: pointer ? event.clientY - rect.top : 0 };
    locations.set(element.getAttribute('data-kpopper-claim'), tracking);
    return trackedGeometry();
  }

  function trackedGeometry() {
    const rect = tracking.element.getBoundingClientRect();
    return { x: tracking.pointer ? rect.left + tracking.dx : rect.left, y: tracking.pointer ? rect.top + tracking.dy : rect.bottom,
      pointer: tracking.pointer, anchor: { left: rect.left, top: rect.top, right: rect.right, bottom: rect.bottom },
      viewport: { width: window.innerWidth, height: window.innerHeight } };
  }

  function openEvidence(element, event) {
    send('kp:open', { id: element.getAttribute('data-kpopper-claim'), geometry: geometry(element, event) });
  }

  function updatePosition() {
    if (!tracking || !tracking.element.isConnected) return;
    send('kp:position', { id: tracking.element.getAttribute('data-kpopper-claim'), geometry: trackedGeometry() });
  }

  function scrollFragment(event) {
    if (event.defaultPrevented || event.button != null && event.button !== 0 ||
        event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
    const link = event.target && event.target.closest ? event.target.closest('a[href]') : null;
    if (!link || link.hasAttribute('download')) return;
    const href = link.getAttribute('href');
    const target = (link.getAttribute('target') || '').trim().toLowerCase();
    if (!href.startsWith('#') || target && target !== '_self') return;
    // srcdoc inherits the embedding file's base URL. Keep fragments in this document
    // without rewriting authored links or loading that file into the opaque frame.
    event.preventDefault();
    let fragment = href.slice(1);
    try { fragment = decodeURIComponent(fragment); } catch (_) { /* Keep a literal malformed escape. */ }
    if (!fragment) { window.scrollTo({ top: 0, left: 0 }); return; }
    const destination = document.getElementById(fragment) || Array.from(document.querySelectorAll('a[name]'))
      .find(anchor => anchor.getAttribute('name') === fragment);
    if (destination) destination.scrollIntoView({ block: 'start' });
    else if (fragment.toLowerCase() === 'top') window.scrollTo({ top: 0, left: 0 });
  }

  function initialize() {
    if (initialized) return;
    initialized = true;
    const style = document.createElement('style');
    style.textContent = '[data-kpopper-mark]{text-decoration-line:underline;text-decoration-style:dotted;' +
      'text-decoration-color:#68796d;text-underline-offset:.18em;cursor:help}' +
      '[data-kpopper-mark="mismatch"],[data-kpopper-mark="unavailable"]{text-decoration-color:#a65b26}' +
      '[data-kpopper-mark]:focus-visible{outline:2px solid #427059;outline-offset:3px}';
    document.head.append(style);
    for (const claim of config.claims) {
      const matches = anchors(claim.id);
      if (matches.length !== 1) continue;
      const element = matches[0];
      original.set(claim.id, element);
      element.setAttribute('data-kpopper-mark', claim.status);
      if (!element.hasAttribute('tabindex')) element.setAttribute('tabindex', '0');
      // Keep existing author semantics and accessible names intact.
      if (!element.hasAttribute('title')) element.setAttribute('title', config.evidenceLabel + ': ' + claim.label);
    }
    document.addEventListener('click', event => {
      const target = markedTarget(event);
      if (target) openEvidence(target, event);
      else send('kp:dismiss', {});
    }, true);
    document.addEventListener('pointerover', event => {
      const target = markedTarget(event);
      if (event.pointerType !== 'mouse' || !target || target.contains(event.relatedTarget)) return;
      send('kp:hover', { id: target.getAttribute('data-kpopper-claim'), geometry: geometry(target, event) });
    });
    document.addEventListener('pointerout', event => {
      const target = markedTarget(event);
      if (event.pointerType !== 'mouse' || !target || target.contains(event.relatedTarget)) return;
      send('kp:leave', { id: target.getAttribute('data-kpopper-claim') });
    });
    document.addEventListener('keydown', event => {
      if (event.key === 'Escape') { send('kp:dismiss', {}); return; }
      const target = markedTarget(event);
      if (target && (event.key === 'Enter' || event.key === ' ')) {
        event.preventDefault();
        openEvidence(target, event);
      }
    });
    // Bubble after the author's element/document handlers so cancelled clicks win.
    window.addEventListener('click', scrollFragment);
    window.addEventListener('scroll', updatePosition, true);
    window.addEventListener('resize', updatePosition);
    const observations = observe(Array.from(claims.keys()));
    send('kp:ready', { observations });
  }

  window.addEventListener('message', event => {
    if (event.source !== parent || !event.data || event.data.nonce !== config.nonce) return;
    const message = event.data;
    if (message.type === 'kp:locate' && typeof message.id === 'string' && claims.has(message.id)) {
      const matches = anchors(message.id);
      if (matches.length === 1 && original.get(message.id) === matches[0]) {
        if (locations.has(message.id)) tracking = locations.get(message.id);
        else geometry(matches[0]);
        updatePosition();
      }
      return;
    }
    if (message.type === 'kp:focus' && typeof message.id === 'string' && claims.has(message.id)) {
      const matches = anchors(message.id);
      if (matches.length === 1 && original.get(message.id) === matches[0] && visible(matches[0])) matches[0].focus();
      return;
    }
    if (message.type !== 'kp:probe' || !initialized || typeof message.request !== 'string' ||
        message.request.length > 160 || !Array.isArray(message.ids) || !message.ids.length ||
        message.ids.length > claims.size || new Set(message.ids).size !== message.ids.length ||
        !message.ids.every(id => typeof id === 'string' && claims.has(id))) return;
    const observations = observe(message.ids);
    send('kp:probe-result', { request: message.request, observations, ok: passes(observations) });
  });

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', initialize, { once: true });
  else initialize();
}());
