(function () {
  'use strict';
  const iframe = document.getElementById('kp-document');
  const panel = document.getElementById('kp-panel');
  const opener = document.getElementById('kp-open');
  const notice = document.getElementById('kp-notice');
  const dataElement = document.getElementById('kp-data');
  let data;
  let runtime;
  try {
    data = JSON.parse(dataElement.textContent);
    runtime = JSON.parse(document.getElementById('kp-runtime').textContent);
  } catch (_) {
    notice.textContent = 'The document evidence could not be opened. Use an intact saved copy.';
    return;
  }
  const hebrew = /^he(?:-|$)/i.test(data.language);
  const words = {
    evidence: ['Evidence', 'מקורות ובדיקות'], close: ['Close evidence', 'סגירת המקורות והבדיקות'],
    heading: ['Behind this document', 'מאחורי המסמך'], snapshot: ['Saved evidence', 'מקורות שמורים'],
    snapshotNote: ['These offline snapshots do not update by themselves. Opening this file does not reread live sources.', 'המקורות השמורים אינם מתעדכנים מעצמם. פתיחת הקובץ אינה קוראת את המקורות מחדש.'],
    summary: ['Checks in this copy', 'בדיקות בעותק הזה'], match: ['Matches snapshot', 'תואם למקור השמור'],
    mismatch: ['Differs from snapshot', 'שונה מהמקור השמור'], unavailable: ['Evidence unavailable', 'המקור אינו זמין'],
    unchecked: ['Not checked', 'לא נבדק'], scope: ['What is covered', 'מה נכלל בבדיקה'],
    anchored: ['marked claims', 'טענות מסומנות'], blocks: ['unmarked text blocks', 'קטעי טקסט ללא סימון'],
    values: ['possible unmarked values', 'ערכים אפשריים ללא סימון'],
    unmarked: ['Unmarked text is not checked. These counts are a text heuristic, not a measure of semantic coverage.', 'טקסט שאינו מסומן לא נבדק. הספירה היא הערכה לפי דפוסי טקסט, ואינה מודדת את כיסוי התוכן.'],
    excerpts: ['Examples of unmarked text', 'דוגמאות לטקסט ללא סימון'],
    proposals: ['Review proposed updates', 'בדיקת העדכונים המוצעים'],
    atomic: ['The related edits below are one decision; all are applied together.', 'השינויים הקשורים הם החלטה אחת; כולם מוחלים יחד.'],
    reviewNote: ['Accepting uses the saved snapshot; it does not reread sources or check reasoning. Accepting or resetting an accepted update restarts the document’s interactive controls.', 'אישור משתמש במקור השמור; הוא אינו קורא מקורות מחדש או בודק מסקנות. אישור או איפוס של עדכון שאושר מפעילים מחדש את הרכיבים האינטראקטיביים במסמך.'],
    before: ['Before', 'לפני'], after: ['After', 'אחרי'], edits: ['Included edits', 'השינויים הכלולים'],
    accept: ['Accept together', 'אישור כל השינויים'], reject: ['Keep original', 'שמירת המקור'],
    reset: ['Reset decision', 'איפוס ההחלטה'], pending: ['Awaiting your review', 'ממתין לבדיקה שלך'],
    accepted: ['Accepted in this copy', 'אושר בעותק הזה'], rejected: ['Original kept', 'המקור נשמר'],
    blocked: ['Update unavailable', 'העדכון אינו זמין'], blockedNote: ['Required evidence is unavailable. The related edits stay together.', 'מקור נדרש אינו זמין. השינויים הקשורים נשארים יחד.'],
    acceptedNotice: ['Update accepted. Download a copy to save this decision.', 'העדכון אושר. יש להוריד עותק כדי לשמור את ההחלטה.'],
    rejectedNotice: ['Original kept. Download a copy to save this decision.', 'המקור נשמר. יש להוריד עותק כדי לשמור את ההחלטה.'],
    resetNotice: ['Decision reset. Download a copy to save this change.', 'ההחלטה אופסה. יש להוריד עותק כדי לשמור את השינוי.'],
    guard: ['This update was not applied: a marked passage changed, disappeared, or could not be verified. Reopen the saved copy to review its original passages.', 'העדכון לא הוחל: קטע מסומן השתנה, נעלם או לא ניתן לאימות. יש לפתוח מחדש את העותק השמור כדי לבדוק את הקטעים המקוריים.'],
    hidden: ['This update was not applied because a marked passage is currently hidden. Reveal its section or tab, then try again.', 'העדכון לא הוחל מפני שקטע מסומן מוסתר כרגע. יש להציג את הסעיף או הלשונית שלו ולנסות שוב.'],
    waiting: ['Checking the current passages…', 'בדיקת הקטעים המוצגים…'],
    claims: ['Claim checks', 'בדיקות הטענות'], actual: ['In this document', 'במסמך הזה'], expected: ['From the saved evidence', 'לפי המקור השמור'],
    checkNote: ['A match means only that the named check agrees with captured values.', 'התאמה פירושה רק שהבדיקה המוגדרת תואמת לערכים שנשמרו.'],
    inference: ['Reasoning needs human judgment; it has no automatic verification.', 'מסקנה דורשת שיקול דעת אנושי; אין לה אימות אוטומטי.'],
    changed: ['Source changed since the previous snapshot', 'המקור השתנה מאז הקריאה השמורה הקודמת'],
    sources: ['Source snapshots', 'מקורות שמורים'], source: ['Source', 'מקור'], sourceLink: ['Open original citation', 'פתיחת הקישור למקור'],
    extracted: ['Agent-made extraction; the original source is not independently verified.', 'חילוץ שבוצע בידי סוכן; המקור המקורי לא אומת באופן עצמאי.'],
    record: ['Captured record reading', 'נתון שנשמר ברשומה'], file: ['Captured file reading', 'נתון שנקרא מקובץ'],
    selectedOnly: ['Only selected values and excerpts are included here.', 'רק הערכים והקטעים שנבחרו נכללים כאן.'],
    notReread: ['Not reread on this refresh', 'לא נקרא מחדש בעדכון הזה'],
    identityOnly: ['Source identity captured; no excerpt was selected.', 'פרטי המקור נשמרו; לא נבחר קטע ממנו.'],
    location: ['Location', 'מיקום'], captured: ['Read on', 'נקרא בתאריך'], attempted: ['Read attempted on', 'ניסיון הקריאה בתאריך'],
    details: ['Technical details', 'פרטים טכניים'], revision: ['Captured revision (SHA-256)', 'גרסה שמורה (SHA-256)'],
    claimId: ['Claim ID', 'מזהה הטענה'], sourceId: ['Source ID', 'מזהה המקור'], checkKind: ['Check', 'סוג הבדיקה'],
    generated: ['Snapshot prepared on', 'המקור השמור הוכן בתאריך'], decisionDate: ['Decision saved on', 'ההחלטה נשמרה בתאריך'],
    download: ['Download reviewed copy', 'הורדת העותק שנבדק'],
    saveNote: ['Download saves this document, its evidence and your decisions. Reloading the original discards unsaved choices. Temporary activity inside the document is not saved.', 'ההורדה שומרת את המסמך, המקורות וההחלטות שלך. טעינת המקור מחדש מוחקת בחירות שלא נשמרו. פעילות זמנית בתוך המסמך אינה נשמרת.'],
    downloaded: ['Reviewed copy prepared for download.', 'העותק שנבדק מוכן להורדה.'],
    exportFailed: ['The copy could not be prepared. Your decisions are still available in this window.', 'לא ניתן להכין את העותק. ההחלטות שלך עדיין זמינות בחלון הזה.'],
    invalid: ['The saved document could not be reconstructed safely. Use an intact saved copy.', 'לא ניתן לשחזר את המסמך השמור בבטחה. יש להשתמש בעותק שמור תקין.']
  };
  const t = key => words[key][hebrew ? 1 : 0];
  const claims = new Map(data.claims.map(claim => [claim.id, claim]));
  const sourceIds = Object.keys(data.sources);
  const claimNodes = new Map();
  let nonce = '';
  let frameReady = false;
  let pending = null;
  let focusClaim = null;
  let serial = 0;

  function element(tag, text, className) {
    const node = document.createElement(tag);
    if (text !== undefined && text !== null) node.textContent = String(text);
    if (className) node.className = className;
    return node;
  }

  function button(text, action, className) {
    const node = element('button', text, className || 'kp-button');
    node.type = 'button';
    node.addEventListener('click', action);
    return node;
  }

  function paragraph(parent, text, className) {
    const node = element('p', text, className);
    parent.append(node);
    return node;
  }

  function detail(parent, title) {
    const node = element('details', null, 'kp-details');
    node.append(element('summary', title));
    parent.append(node);
    return node;
  }

  function field(parent, label, value) {
    if (value === undefined || value === null) return;
    const row = element('div', null, 'kp-field');
    row.append(element('span', label, 'kp-label'), element('div', value, 'kp-value'));
    parent.append(row);
  }

  function currentChecks() {
    const result = Object.assign({}, data.checks);
    for (const group of data.groups) if (group.decision === 'accepted') Object.assign(result, group.after_checks);
    return result;
  }

  function safeJSON(value) {
    return JSON.stringify(value).replace(/</g, '\\u003c').replace(/\u2028/g, '\\u2028').replace(/\u2029/g, '\\u2029');
  }

  function randomToken() {
    const bytes = new Uint8Array(16);
    crypto.getRandomValues(bytes);
    return Array.from(bytes, byte => byte.toString(16).padStart(2, '0')).join('');
  }

  function selectedHTML(nextGroup, nextDecision) {
    const points = Array.from(data.authored_html);
    const edits = data.groups.flatMap(group =>
      (group === nextGroup ? nextDecision : group.decision) === 'accepted' ? group.edits : []);
    edits.sort((a, b) => a.start - b.start);
    let previousEnd = -1;
    for (const edit of edits) {
      if (!Number.isInteger(edit.start) || !Number.isInteger(edit.end) || edit.start < 0 ||
          edit.end < edit.start || edit.end > points.length || edit.start < previousEnd ||
          typeof edit.before_raw !== 'string' || typeof edit.after_raw !== 'string' ||
          points.slice(edit.start, edit.end).join('') !== edit.before_raw) throw new Error('Invalid exact edit');
      previousEnd = edit.end;
    }
    const segments = [];
    let cursor = 0;
    for (const edit of edits) {
      segments.push(points.slice(cursor, edit.start).join(''), edit.after_raw);
      cursor = edit.end;
    }
    segments.push(points.slice(cursor).join(''));
    return segments.join('');
  }

  function attribute(value) {
    return String(value).replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;');
  }

  function loadFrame(html) {
    const nextNonce = randomToken();
    const checks = currentChecks();
    const config = { nonce: nextNonce, evidenceLabel: t('evidence'), claims: data.claims.map(claim => ({
      id: claim.id, label: claim.label, text: checks[claim.id].actual, status: checks[claim.id].status
    })) };
    const bridge = runtime.bridge.replace('__KPOPPER_CONFIG__', () => safeJSON(config));
    if (/<\/script/i.test(bridge)) throw new Error('Unsafe bridge');
    const points = Array.from(html);
    if (!Number.isInteger(data.head_end) || data.head_end < 0 || data.head_end > points.length) throw new Error('Invalid head');
    const injection = '<meta http-equiv="Content-Security-Policy" content="' + attribute(runtime.csp) + '">' +
      '<script>' + bridge + '<' + '/script>';
    const srcdoc = points.slice(0, data.head_end).join('') + injection + points.slice(data.head_end).join('');
    const previousNonce = nonce;
    const previousReady = frameReady;
    nonce = nextNonce;
    frameReady = false;
    try {
      iframe.srcdoc = srcdoc;
    } catch (error) {
      nonce = previousNonce;
      frameReady = previousReady;
      throw error;
    }
  }

  function announce(text) { notice.textContent = text; }

  function closePanel() {
    panel.hidden = true;
    opener.hidden = false;
    opener.setAttribute('aria-expanded', 'false');
    if (focusClaim && frameReady) {
      iframe.contentWindow.postMessage({ type: 'kp:focus', nonce, id: focusClaim }, '*');
      iframe.focus();
    } else opener.focus();
    focusClaim = null;
  }

  function openPanel(id) {
    focusClaim = typeof id === 'string' && claims.has(id) ? id : null;
    panel.hidden = false;
    opener.hidden = true;
    opener.setAttribute('aria-expanded', 'true');
    const target = focusClaim ? claimNodes.get(focusClaim) : panel.querySelector('[data-kp-close]');
    if (target) {
      if (focusClaim) target.open = true;
      const focus = focusClaim ? target.querySelector('summary') : target;
      focus.focus();
      if (focusClaim) target.scrollIntoView({ block: 'nearest' });
    }
  }

  function probeState(message, operation) {
    if (typeof message.ok !== 'boolean' || !Array.isArray(message.observations) || message.observations.length !== operation.ids.length) return 'invalid';
    const checks = currentChecks();
    const seen = new Set();
    let hidden = false;
    for (const item of message.observations) {
      if (!item || !operation.ids.includes(item.id) || seen.has(item.id)) return 'invalid';
      seen.add(item.id);
      if (item.count !== 1 || item.connected !== true || item.same !== true || item.text !== checks[item.id].actual ||
          typeof item.visible !== 'boolean') return 'invalid';
      if (!item.visible) hidden = true;
    }
    if (message.ok !== !hidden) return 'invalid';
    return hidden ? 'hidden' : 'valid';
  }

  function applyDecision(group, decision, html) {
    const prior = { decision: group.decision, decided_at: group.decided_at };
    try {
      group.decision = decision;
      group.decided_at = decision === 'pending' ? null : new Date().toISOString();
      if (html !== null) loadFrame(html);
      dataElement.textContent = safeJSON(data);
      renderPanel();
      announce(t(decision === 'accepted' ? 'acceptedNotice' : decision === 'rejected' ? 'rejectedNotice' : 'resetNotice'));
    } catch (_) {
      Object.assign(group, prior);
      renderPanel();
      announce(t('invalid'));
    }
  }

  function decide(group, decision) {
    if (pending || group.status !== 'ready' || !['pending', 'accepted', 'rejected'].includes(decision) || group.decision === decision) return;
    const needsProbe = decision === 'accepted' || group.decision === 'accepted';
    if (!needsProbe) { applyDecision(group, decision, null); return; }
    if (!frameReady) { announce(t('guard')); return; }
    let html;
    try { html = selectedHTML(group, decision); } catch (_) { announce(t('invalid')); return; }
    const operation = { request: randomToken() + '-' + (++serial), nonce, group, decision, html, ids: group.members.slice() };
    pending = operation;
    operation.timeout = setTimeout(() => {
      if (pending !== operation) return;
      pending = null;
      renderPanel();
      announce(t('guard'));
    }, 5000);
    renderPanel();
    announce(t('waiting'));
    iframe.contentWindow.postMessage({ type: 'kp:probe', nonce, request: operation.request, ids: operation.ids }, '*');
  }

  function badge(status) { return element('span', t(status), 'kp-badge kp-' + status); }

  function renderGroup(parent, group) {
    const card = detail(parent, group.label);
    card.classList.add('kp-group');
    card.dataset.group = group.id;
    card.open = group.decision === 'pending';
    card.append(badge(group.status === 'blocked' ? 'blocked' : group.decision));
    if (group.status === 'blocked') { paragraph(card, t('blockedNote')); return; }
    paragraph(card, t('atomic'), 'kp-muted');
    for (const context of group.contexts) {
      const comparison = element('div', null, 'kp-comparison');
      field(comparison, t('before'), context.before);
      field(comparison, t('after'), context.after);
      card.append(comparison);
    }
    const edits = detail(card, t('edits'));
    for (const edit of group.edits) {
      paragraph(edits, claims.get(edit.id).label, 'kp-edit-title');
      field(edits, t('before'), edit.before);
      field(edits, t('after'), edit.after);
    }
    const actions = element('div', null, 'kp-actions');
    if (group.decision === 'pending') {
      const accept = button(t('accept'), () => decide(group, 'accepted'), 'kp-button kp-primary');
      accept.dataset.action = 'accept';
      accept.disabled = !!pending || !frameReady;
      const reject = button(t('reject'), () => decide(group, 'rejected'));
      reject.dataset.action = 'reject';
      reject.disabled = !!pending;
      actions.append(accept, reject);
    } else {
      const reset = button(t('reset'), () => decide(group, 'pending'));
      reset.dataset.action = 'reset';
      reset.disabled = !!pending || group.decision === 'accepted' && !frameReady;
      actions.append(reset);
      field(card, t('decisionDate'), group.decided_at);
    }
    card.append(actions);
  }

  function renderClaim(parent, claim, check) {
    const card = detail(parent, claim.label);
    card.dataset.claim = claim.id;
    claimNodes.set(claim.id, card);
    card.append(badge(check.status));
    if (check.source_changed) paragraph(card, t('changed'), 'kp-changed');
    field(card, t('actual'), check.actual);
    if (check.expected !== null) field(card, t('expected'), check.expected);
    if (claim.kind === 'inference') paragraph(card, t('inference'), 'kp-muted');
    if (check.status === 'unchecked' || check.status === 'unavailable') paragraph(card, check.detail);
    for (const id of new Set(claim.inputs.map(input => input.source))) {
      const link = element('a', data.sources[id].name);
      link.href = '#kp-source-' + sourceIds.indexOf(id);
      link.addEventListener('click', () => { document.getElementById('kp-source-' + sourceIds.indexOf(id)).open = true; });
      const row = element('p', null, 'kp-source-link');
      row.append(element('span', t('source') + ': '), link);
      card.append(row);
    }
    const technical = detail(card, t('details'));
    field(technical, t('claimId'), claim.id);
    field(technical, t('checkKind'), claim.kind);
    field(technical, t('captured'), check.checked_at);
  }

  function citationLink(parent, uri) {
    try {
      const url = new URL(uri);
      if (!['http:', 'https:'].includes(url.protocol)) return;
      const link = element('a', t('sourceLink'));
      link.href = url.href;
      link.target = '_blank';
      link.rel = 'noopener noreferrer';
      parent.append(link);
    } catch (_) { /* A citation is optional, never executable evidence. */ }
  }

  function renderSource(parent, id, index) {
    const source = data.sources[id];
    const card = detail(parent, source.name);
    card.id = 'kp-source-' + index;
    if (source.status === 'available') {
      paragraph(card, t(source.representation === 'extraction' ? 'extracted' : source.format === 'record' ? 'record' : 'file'), 'kp-muted');
    }
    if (source.reread === false) paragraph(card, t('notReread'), 'kp-changed');
    if (source.status === 'unavailable') {
      card.append(badge('unavailable'));
      paragraph(card, source.reason);
    }
    for (const selection of Object.values(source.selections)) {
      const selected = element('div', null, 'kp-selection');
      if (selection.status === 'available' && selection.value) paragraph(selected, selection.value.value, 'kp-excerpt');
      else paragraph(selected, selection.status === 'available' ? t('identityOnly') : selection.reason || t('unavailable'), 'kp-muted');
      if (selection.location) field(selected, t('location'), selection.location);
      else if (selection.selector && Object.hasOwn(selection.selector, 'pointer')) field(selected, t('location'), selection.selector.pointer || '/');
      if (selection.citation) {
        field(selected, t('source'), selection.citation.name);
        field(selected, t('location'), selection.citation.at);
        field(selected, t('captured'), selection.citation.date);
      }
      card.append(selected);
    }
    if (source.uri) citationLink(card, source.uri);
    const technical = detail(card, t('details'));
    field(technical, t('sourceId'), id);
    field(technical, t('captured'), source.read_at);
    field(technical, t('attempted'), source.attempted_at);
    field(technical, t('revision'), source.sha256);
  }

  function renderPanel() {
    const scroll = panel.scrollTop;
    const openClaims = new Set(Array.from(claimNodes.entries()).filter(([, node]) => node.open).map(([id]) => id));
    const active = document.activeElement;
    const activeGroup = active && active.closest ? active.closest('[data-group]') : null;
    const activeGroupId = activeGroup ? activeGroup.dataset.group : null;
    panel.replaceChildren();
    claimNodes.clear();
    const header = element('header', null, 'kp-header');
    const heading = element('div');
    heading.append(element('p', t('snapshot'), 'kp-eyebrow'), element('h1', t('heading')));
    const close = button('×', closePanel, 'kp-close');
    close.setAttribute('aria-label', t('close'));
    close.dataset.kpClose = '';
    header.append(heading, close);
    panel.append(header);
    paragraph(panel, t('snapshotNote'), 'kp-intro');
    const summary = element('section', null, 'kp-section');
    summary.append(element('h2', t('summary')));
    const checks = currentChecks();
    const counts = element('div', null, 'kp-counts');
    for (const status of ['match', 'mismatch', 'unavailable', 'unchecked']) {
      const count = Object.values(checks).filter(check => check.status === status).length;
      const item = element('div', null, 'kp-count');
      item.append(element('strong', count), element('span', t(status)));
      counts.append(item);
    }
    summary.append(counts);
    paragraph(summary, t('checkNote'), 'kp-muted');
    const coverage = detail(summary, t('scope'));
    coverage.open = true;
    paragraph(coverage, t('unmarked'), 'kp-scope-note');
    for (const [key, label] of [['anchored_claims', 'anchored'], ['unmarked_blocks', 'blocks'], ['unmarked_values', 'values']]) {
      paragraph(coverage, String(data.coverage[key]) + ' · ' + t(label), 'kp-stat');
    }
    if (data.coverage.excerpts.length) {
      const excerpts = detail(coverage, t('excerpts'));
      data.coverage.excerpts.forEach(text => paragraph(excerpts, text, 'kp-excerpt'));
    }
    panel.append(summary);
    if (data.groups.length) {
      const groups = element('section', null, 'kp-section');
      groups.append(element('h2', t('proposals')));
      paragraph(groups, t('reviewNote'), 'kp-muted');
      data.groups.forEach(group => renderGroup(groups, group));
      panel.append(groups);
    }
    const checked = element('section', null, 'kp-section');
    checked.append(element('h2', t('claims')));
    data.claims.forEach(claim => renderClaim(checked, claim, checks[claim.id]));
    panel.append(checked);
    const sources = element('section', null, 'kp-section');
    sources.append(element('h2', t('sources')));
    paragraph(sources, t('selectedOnly'), 'kp-muted');
    sourceIds.forEach((id, index) => renderSource(sources, id, index));
    panel.append(sources);
    const footer = element('footer', null, 'kp-footer');
    const download = button(t('download'), downloadCopy, 'kp-button kp-primary kp-download');
    download.dataset.action = 'download';
    download.disabled = !!pending;
    footer.append(download);
    paragraph(footer, t('saveNote'), 'kp-muted');
    const technical = detail(footer, t('details'));
    field(technical, t('generated'), data.generated_at);
    panel.append(footer);
    for (const id of openClaims) if (claimNodes.has(id)) claimNodes.get(id).open = true;
    panel.scrollTop = scroll;
    if (!panel.hidden && activeGroupId) {
      const card = Array.from(panel.querySelectorAll('[data-group]')).find(node => node.dataset.group === activeGroupId);
      if (card) card.querySelector('summary').focus();
    }
  }

  function downloadCopy() {
    if (pending) return;
    try {
      selectedHTML();
      const clone = document.documentElement.cloneNode(true);
      clone.querySelector('#kp-data').textContent = safeJSON(data);
      clone.querySelector('#kp-runtime').textContent = safeJSON(runtime);
      clone.querySelector('#kp-document').removeAttribute('srcdoc');
      const savedPanel = clone.querySelector('#kp-panel');
      savedPanel.replaceChildren();
      savedPanel.hidden = true;
      clone.querySelector('#kp-notice').textContent = '';
      const savedOpener = clone.querySelector('#kp-open');
      savedOpener.hidden = false;
      savedOpener.setAttribute('aria-expanded', 'false');
      const html = '<!doctype html>\n' + clone.outerHTML + '\n';
      const url = URL.createObjectURL(new Blob([html], { type: 'text/html;charset=utf-8' }));
      const link = element('a');
      link.href = url;
      link.download = (data.title.replace(/[\\/:*?"<>|\u0000-\u001f]/g, '-').slice(0, 100) || 'document') + '-reviewed.html';
      document.body.append(link);
      link.click();
      link.remove();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
      announce(t('downloaded'));
    } catch (_) { announce(t('exportFailed')); }
  }

  // Install the listener before the first srcdoc assignment; the frame has an opaque origin.
  window.addEventListener('message', event => {
    const message = event.data;
    if (event.source !== iframe.contentWindow || !message || message.nonce !== nonce) return;
    if (message.type === 'kp:ready') {
      frameReady = true;
      renderPanel();
    } else if (message.type === 'kp:open' && typeof message.id === 'string' && claims.has(message.id)) {
      openPanel(message.id);
    } else if (message.type === 'kp:probe-result' && pending && message.request === pending.request && nonce === pending.nonce) {
      const operation = pending;
      clearTimeout(operation.timeout);
      pending = null;
      const state = probeState(message, operation);
      if (state !== 'valid') { renderPanel(); announce(t(state === 'hidden' ? 'hidden' : 'guard')); return; }
      applyDecision(operation.group, operation.decision, operation.html);
    }
  });
  opener.textContent = t('evidence');
  panel.setAttribute('dir', hebrew ? 'rtl' : 'ltr');
  panel.setAttribute('lang', hebrew ? 'he' : 'en');
  panel.setAttribute('role', 'dialog');
  panel.setAttribute('aria-modal', 'false');
  panel.setAttribute('aria-label', t('evidence'));
  opener.addEventListener('click', () => openPanel());
  panel.addEventListener('keydown', event => { if (event.key === 'Escape') { event.preventDefault(); closePanel(); } });
  try { loadFrame(selectedHTML()); renderPanel(); } catch (_) { announce(t('invalid')); }
}());
