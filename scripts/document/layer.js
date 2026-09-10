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
    selected: ['About this passage', 'על הפרט שבחרת'],
    back: ['Back', 'חזרה'],
    readingFrom: ['Saved readings from ', 'נתונים שנשמרו מתוך '],
    comparedFrom: ['This passage was compared with the saved reading from ', 'הקטע הושווה לנתון שנשמר מתוך '],
    quotedFrom: ['This passage was compared with the selected quotation from ', 'הקטע הושווה לציטוט שנבחר מתוך '],
    calculationFrom: ['The saved calculation uses readings from ', 'החישוב השמור משתמש בנתונים מתוך '],
    matchesReading: ['It matches the captured result.', 'הוא תואם לתוצאה השמורה.'],
    differsReading: ['It differs from the captured result.', 'הוא שונה מהתוצאה השמורה.'],
    says: ['The document says', 'במסמך כתוב'],
    savedResult: ['The saved result is', 'התוצאה השמורה היא'],
    calculation: ['Saved calculation', 'החישוב השמור'],
    rounding: ['Displayed decimal places', 'מספר הספרות אחרי הנקודה בתצוגה'],
    contextSources: ['Sources offered as context: ', 'מקורות שניתנו כרקע: '],
    unavailableNote: ['There is not enough captured evidence to check this passage.', 'אין מספיק מידע שמור כדי לבדוק את הקטע הזה.'],
    sourceScope: ['These are the selected readings used by the passage you came from.', 'אלה הנתונים שנבחרו לשימוש בקטע שממנו הגעת.'],
    and: [' and ', ' ו־'],
    overview: ['All document evidence', 'כל המקורות והבדיקות של המסמך'],
    related: ['Related correction', 'תיקון קשור'],
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
    download: ['Save document copy', 'שמירת עותק המסמך'],
    saveNote: ['Download saves this document, its evidence and your decisions. Reloading the original discards unsaved choices. Temporary activity inside the document is not saved.', 'ההורדה שומרת את המסמך, המקורות וההחלטות שלך. טעינת המקור מחדש מוחקת בחירות שלא נשמרו. פעילות זמנית בתוך המסמך אינה נשמרת.'],
    downloaded: ['Document copy prepared for download.', 'עותק המסמך מוכן להורדה.'],
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
  let selectedClaim = null;
  let sourceView = null;
  let sourceReturn = null;
  let placement = null;
  let panelBounds = null;
  let pinned = false;
  let pointerInside = false;
  let hoverTimer = null;
  let leaveTimer = null;
  let hoverCandidate = null;
  let dismissedClaim = null;
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

  function validGeometry(value) {
    if (!value || typeof value.pointer !== 'boolean' || !value.anchor || !value.viewport) return false;
    const values = [value.x, value.y, value.anchor.left, value.anchor.top, value.anchor.right, value.anchor.bottom,
      value.viewport.width, value.viewport.height];
    return values.every(number => typeof number === 'number' && Number.isFinite(number) && Math.abs(number) <= 10000000) &&
      value.viewport.width > 0 && value.viewport.height > 0 && value.anchor.right >= value.anchor.left && value.anchor.bottom >= value.anchor.top;
  }

  function holdPanelBounds() {
    if (!panel.hidden && !panelBounds) {
      const box = panel.getBoundingClientRect();
      panelBounds = { left: box.left, top: box.top, width: box.width, height: box.height };
    }
  }

  function placePanel() {
    if (panel.hidden || !panelBounds && (!placement || panel.dataset.view === 'overview')) return;
    const width = window.innerWidth;
    const height = window.innerHeight;
    if (!Number.isFinite(width) || !Number.isFinite(height) || width <= 24 || height <= 24) return;
    if (panelBounds) {
      // Internal navigation changes the content, not the surrounding window. Only
      // a smaller viewport may constrain the saved box; the body scrolls inside it.
      const pw = Math.min(panelBounds.width, width - 24);
      const ph = Math.min(panelBounds.height, height - 24);
      panel.style.width = pw + 'px'; panel.style.height = ph + 'px';
      panel.style.maxHeight = ph + 'px';
      panel.style.right = 'auto'; panel.style.bottom = 'auto';
      panel.style.left = Math.max(12, Math.min(panelBounds.left, width - pw - 12)) + 'px';
      panel.style.top = Math.max(12, Math.min(panelBounds.top, height - ph - 12)) + 'px';
      return;
    }
    const geometry = placement.geometry;
    let x = geometry.x, y = geometry.y, left = geometry.anchor.left, top = geometry.anchor.top, bottom = geometry.anchor.bottom;
    if (placement.frame) {
      const frame = iframe.getBoundingClientRect();
      const sx = frame.width / geometry.viewport.width, sy = frame.height / geometry.viewport.height;
      x = frame.left + x * sx; y = frame.top + y * sy;
      left = frame.left + left * sx; top = frame.top + top * sy; bottom = frame.top + bottom * sy;
    }
    if (![x, y, left, top, bottom].every(Number.isFinite)) return;
    panel.style.maxHeight = (height - 24) + 'px';
    panel.style.right = 'auto'; panel.style.bottom = 'auto';
    const box = panel.getBoundingClientRect();
    const pw = Math.min(box.width, width - 24), ph = Math.min(box.height, height - 24);
    let px = geometry.pointer ? x + 12 : left;
    let py = geometry.pointer ? y + 12 : bottom + 8;
    if (px + pw > width - 12 && geometry.pointer) px = x - pw - 12;
    if (py + ph > height - 12) py = (geometry.pointer ? y - 12 : top - 8) - ph;
    panel.style.left = Math.max(12, Math.min(px, width - pw - 12)) + 'px';
    panel.style.top = Math.max(12, Math.min(py, height - ph - 12)) + 'px';
  }

  function clearHover() {
    clearTimeout(hoverTimer); clearTimeout(leaveTimer);
    hoverTimer = null; leaveTimer = null; hoverCandidate = null;
  }

  function pinPanel() {
    pinned = true;
    clearHover();
    panel.dataset.pinned = 'true';
  }

  function scheduleLeave() {
    clearTimeout(leaveTimer);
    if (pinned) return;
    leaveTimer = setTimeout(() => {
      if (!pinned && !pointerInside && !panel.contains(document.activeElement)) closePanel(false);
    }, 360);
  }

  function closePanel(restoreFocus = true) {
    clearHover();
    if (panel.hidden) return;
    const returnClaim = focusClaim;
    panel.hidden = true;
    opener.hidden = false;
    opener.setAttribute('aria-expanded', 'false');
    focusClaim = null;
    selectedClaim = null;
    sourceView = null; sourceReturn = null; placement = null; panelBounds = null;
    dismissedClaim = restoreFocus ? returnClaim : null;
    pinned = false; pointerInside = false;
    renderPanel();
    // Click-away must leave focus with the control the reader just clicked.
    if (!restoreFocus) return;
    if (returnClaim && frameReady) {
      iframe.contentWindow.postMessage({ type: 'kp:focus', nonce, id: returnClaim }, '*');
      iframe.focus();
    } else opener.focus();
  }

  function openPanel(id, geometry = null, preview = false) {
    clearHover();
    panelBounds = null;
    panel.removeAttribute('style');
    focusClaim = typeof id === 'string' && claims.has(id) ? id : null;
    selectedClaim = focusClaim;
    sourceView = null; sourceReturn = null;
    placement = geometry ? { frame: true, geometry } : null;
    pinned = !preview;
    panel.hidden = false;
    opener.hidden = true;
    opener.setAttribute('aria-expanded', 'true');
    renderPanel();
    panel.querySelector('.kp-panel-content').scrollTop = 0;
    const target = panel.querySelector(selectedClaim ? '#kp-panel-title' : '[data-kp-close]');
    if (target && !preview) target.focus({ preventScroll: true });
    placePanel();
  }

  function openSource(id, inputs, event, originClaim) {
    holdPanelBounds();
    sourceReturn = { claim: selectedClaim, placement, originClaim, scroll: panel.querySelector('.kp-panel-content').scrollTop };
    sourceView = { id, inputs };
    if (!placement) {
      const rect = event.currentTarget.getBoundingClientRect();
      placement = { frame: false, geometry: { pointer: false, x: rect.left, y: rect.bottom,
        anchor: rect, viewport: { width: window.innerWidth, height: window.innerHeight } } };
    }
    pinPanel();
    renderPanel();
    panel.querySelector('.kp-panel-content').scrollTop = 0;
    panel.querySelector('#kp-panel-title').focus({ preventScroll: true });
    placePanel();
  }

  function backToEvidence() {
    const previous = sourceReturn;
    const source = sourceView.id;
    selectedClaim = previous.claim; placement = previous.placement;
    if (!selectedClaim) panelBounds = null;
    sourceView = null; sourceReturn = null;
    pinPanel(); renderPanel();
    panel.querySelector('.kp-panel-content').scrollTop = previous.scroll;
    const link = Array.from(panel.querySelectorAll('[data-source]')).find(node => node.dataset.source === source &&
      (node.dataset.sourceClaim || null) === previous.originClaim);
    (link || panel.querySelector('#kp-panel-title')).focus({ preventScroll: true });
    placePanel();
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
    const edits = element('div', null, 'kp-edit-list');
    paragraph(edits, t('edits'), 'kp-label');
    card.append(edits);
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

  function renderClaim(parent, claim, check, focused = false) {
    const card = focused ? element('article', null, 'kp-claim-card') : detail(parent, claim.label);
    if (focused) {
      card.setAttribute('aria-labelledby', 'kp-panel-title');
      parent.append(card);
    }
    card.dataset.claim = claim.id;
    claimNodes.set(claim.id, card);
    card.append(badge(check.status));
    if (check.source_changed) paragraph(card, t('changed'), 'kp-changed');
    paragraph(card, t('says') + ' “' + check.actual + '”.', 'kp-reading');
    const arithmetic = ['sum', 'difference', 'product', 'ratio'].includes(claim.kind);
    if (claim.kind === 'inference') {
      paragraph(card, claim.reason || check.detail);
      paragraph(card, t('inference'), 'kp-muted');
    } else if (check.status === 'unavailable') {
      paragraph(card, t('unavailableNote'));
    }
    if (claim.inputs.length) {
      const row = paragraph(card, t(claim.kind === 'inference' || check.status === 'unavailable' ? 'contextSources' :
        arithmetic ? 'calculationFrom' : claim.kind === 'quote' ? 'quotedFrom' : 'comparedFrom'), 'kp-explanation');
      const ids = Array.from(new Set(claim.inputs.map(input => input.source)));
      ids.forEach((id, index) => {
        if (index) row.append(document.createTextNode(index === ids.length - 1 ? t('and') : ', '));
        row.append(sourceButton(id, claim.inputs, claim.id));
      });
      row.append(document.createTextNode('.'));
    }
    if (check.status === 'match' || check.status === 'mismatch') {
      paragraph(card, t(check.status === 'match' ? 'matchesReading' : 'differsReading'));
      const readings = claim.inputs.map(input => selectedReadings(input.source, [input])[0]);
      if (arithmetic && readings.every(reading => reading && reading.status === 'available' && reading.value && reading.value.type === 'number')) {
        const operator = { sum: ' + ', difference: ' − ', product: ' × ', ratio: ' ÷ ' }[claim.kind];
        let expression = readings.map(reading => String(reading.value.value)).join(operator);
        if (claim.format && Object.hasOwn(claim.format, 'scale')) expression = '(' + expression + ') × ' + String(claim.format.scale);
        paragraph(card, t('calculation') + ': ' + expression + ' → ' + check.expected, 'kp-expression');
        if (claim.format) paragraph(card, t('rounding') + ': ' + String(claim.format.decimals || 0) + '.', 'kp-muted');
      } else paragraph(card, t('savedResult') + ' “' + check.expected + '”.', 'kp-reading');
      paragraph(card, t('checkNote'), 'kp-muted');
    }
    const technical = detail(card, t('details'));
    technical.dataset.technical = '';
    technical.open = false;
    field(technical, t('claimId'), claim.id);
    field(technical, t('checkKind'), claim.kind);
    if (check.status === 'unavailable') paragraph(technical, check.detail);
    field(technical, t('captured'), check.checked_at);
    for (const input of claim.inputs) field(technical, t('location'), selectorKey(input));
    if (claim.format) field(technical, t('details'), JSON.stringify(claim.format));
  }

  function selectorKey(input) {
    return JSON.stringify(Object.fromEntries(Object.entries(input).filter(([key]) => key !== 'source').sort(([a], [b]) => a.localeCompare(b))));
  }

  function selectedReadings(id, inputs) {
    const selections = Object.values(data.sources[id].selections);
    if (!inputs) return selections;
    const wanted = Array.from(new Set(inputs.filter(input => input.source === id).map(selectorKey)));
    return wanted.map(key => selections.find(selection => selectorKey(selection.selector) === key)).filter(Boolean);
  }

  function sourceButton(id, inputs = null, originClaim = null) {
    const link = button(data.sources[id].name, event => openSource(id, inputs, event, originClaim), 'kp-dependency');
    link.dataset.source = id;
    if (originClaim) link.dataset.sourceClaim = originClaim;
    return link;
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

  function renderSource(parent, id, index, inputs = null) {
    const source = data.sources[id];
    const card = element('article', null, 'kp-source-view');
    card.dataset.sourceView = id;
    card.setAttribute('aria-labelledby', 'kp-panel-title');
    card.id = 'kp-source-' + index;
    parent.append(card);
    card.append(badge(source.status === 'unavailable' ? 'unavailable' : source.format === 'record' ? 'record' : 'file'));
    if (source.status === 'available') {
      paragraph(card, t(inputs ? 'sourceScope' : 'selectedOnly'));
      if (source.representation === 'extraction') paragraph(card, t('extracted'));
    }
    if (source.reread === false) paragraph(card, t('notReread'), 'kp-changed');
    if (source.status === 'unavailable') paragraph(card, source.reason);
    const selections = selectedReadings(id, inputs);
    for (const selection of selections) {
      const selected = element('div', null, 'kp-selection');
      if (selection.status === 'available' && selection.value) paragraph(selected, selection.value.value, 'kp-excerpt');
      else paragraph(selected, selection.status === 'available' ? t('identityOnly') : selection.reason || t('unavailable'), 'kp-muted');
      card.append(selected);
    }
    if (source.uri) citationLink(card, source.uri);
    paragraph(card, t('snapshotNote'), 'kp-muted');
    const technical = detail(card, t('details'));
    technical.dataset.technical = '';
    technical.open = false;
    field(technical, t('sourceId'), id);
    field(technical, t('captured'), source.read_at);
    field(technical, t('attempted'), source.attempted_at);
    field(technical, t('revision'), source.sha256);
    for (const selection of selections) {
      field(technical, t('location'), selectorKey(selection.selector));
      if (selection.location) field(technical, t('location'), selection.location);
      if (selection.citation) {
        field(technical, t('source'), selection.citation.name);
        field(technical, t('location'), selection.citation.at);
        field(technical, t('captured'), selection.citation.date);
      }
    }
  }

  function renderPanel() {
    const previousContent = panel.querySelector('.kp-panel-content');
    const scroll = previousContent ? previousContent.scrollTop : 0;
    const openClaims = new Set(Array.from(claimNodes.entries()).filter(([, node]) => node.open).map(([id]) => id));
    const active = document.activeElement;
    const activeGroup = active && active.closest ? active.closest('[data-group]') : null;
    const activeGroupId = activeGroup ? activeGroup.dataset.group : null;
    const focused = selectedClaim ? claims.get(selectedClaim) : null;
    const checks = currentChecks();
    panel.replaceChildren();
    claimNodes.clear();
    const header = element('header', null, 'kp-header');
    const heading = element('div');
    const title = element('h1', sourceView ? data.sources[sourceView.id].name : focused ? focused.label : t('heading'));
    title.id = 'kp-panel-title';
    title.tabIndex = -1;
    if (sourceView) {
      const back = button(t('back'), backToEvidence, 'kp-back');
      back.dataset.action = 'back';
      heading.append(back);
    }
    heading.append(element('p', t(sourceView ? 'source' : focused ? 'selected' : 'snapshot'), 'kp-eyebrow'), title);
    const close = button('×', () => closePanel(), 'kp-close');
    close.setAttribute('aria-label', t('close'));
    close.dataset.kpClose = '';
    header.append(heading, close);
    // Only the content scrolls. The title and close control remain in the viewport.
    const content = element('div', null, 'kp-panel-content');
    panel.append(header, content);
    panel.setAttribute('aria-labelledby', title.id);
    panel.dataset.view = sourceView ? 'source' : focused ? 'claim' : 'overview';
    panel.dataset.pinned = String(pinned);
    if (!placement) panel.removeAttribute('style');
    const relevantGroups = focused ? data.groups.filter(group => group.members.includes(focused.id)) : data.groups;
    if (sourceView) {
      renderSource(content, sourceView.id, sourceIds.indexOf(sourceView.id), sourceView.inputs);
    } else if (focused) {
      renderClaim(content, focused, checks[focused.id], true);
      if (relevantGroups.length) {
        const related = element('section', null, 'kp-section');
        related.append(element('h2', t('related')));
        paragraph(related, t('reviewNote'), 'kp-muted');
        relevantGroups.forEach(group => renderGroup(related, group));
        content.append(related);
      }
    } else {
      paragraph(content, t('snapshotNote'), 'kp-intro');
      const summary = element('section', null, 'kp-section');
      summary.append(element('h2', t('summary')));
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
      content.append(summary);
      if (data.groups.length) {
        const groups = element('section', null, 'kp-section');
        groups.append(element('h2', t('proposals')));
        paragraph(groups, t('reviewNote'), 'kp-muted');
        data.groups.forEach(group => renderGroup(groups, group));
        content.append(groups);
      }
      const checked = element('section', null, 'kp-section');
      checked.append(element('h2', t('claims')));
      data.claims.forEach(claim => renderClaim(checked, claim, checks[claim.id]));
      content.append(checked);
      const sources = element('section', null, 'kp-section');
      sources.append(element('h2', t('sources')));
      paragraph(sources, t('selectedOnly'), 'kp-muted');
      sourceIds.forEach(id => {
        const row = element('div', null, 'kp-source-row');
        row.append(sourceButton(id));
        if (data.sources[id].representation === 'extraction') paragraph(row, t('extracted'), 'kp-muted');
        if (data.sources[id].reread === false) paragraph(row, t('notReread'), 'kp-changed');
        sources.append(row);
      });
      content.append(sources);
    }
    const footer = element('footer', null, 'kp-footer');
    if (focused && !sourceView) {
      const all = button(t('overview'), () => openPanel());
      all.dataset.action = 'overview';
      footer.append(all);
    }
    if (!sourceView && (!focused || relevantGroups.length)) {
      const download = button(t('download'), downloadCopy, 'kp-button kp-primary kp-download');
      download.dataset.action = 'download';
      download.disabled = !!pending;
      footer.append(download);
      paragraph(footer, t('saveNote'), 'kp-muted');
    }
    if (!focused && !sourceView) {
      const technical = detail(footer, t('details'));
      field(technical, t('generated'), data.generated_at);
    }
    if (footer.childElementCount) content.append(footer);
    for (const id of openClaims) if (claimNodes.has(id) && claimNodes.get(id).localName === 'details') claimNodes.get(id).open = true;
    content.scrollTop = scroll;
    if (!panel.hidden && activeGroupId) {
      const card = Array.from(panel.querySelectorAll('[data-group]')).find(node => node.dataset.group === activeGroupId);
      if (card) card.querySelector('summary').focus();
    }
    placePanel();
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
      savedPanel.removeAttribute('data-view');
      savedPanel.removeAttribute('data-pinned');
      savedPanel.removeAttribute('style');
      savedPanel.removeAttribute('aria-labelledby');
      savedPanel.hidden = true;
      clone.querySelector('#kp-notice').textContent = '';
      const savedOpener = clone.querySelector('#kp-open');
      savedOpener.hidden = false;
      savedOpener.setAttribute('aria-expanded', 'false');
      const html = '<!doctype html>\n' + clone.outerHTML + '\n';
      const url = URL.createObjectURL(new Blob([html], { type: 'text/html;charset=utf-8' }));
      const link = element('a');
      link.dataset.kpDownload = '';
      link.href = url;
      link.download = (data.title.replace(/[\\/:*?"<>|\u0000-\u001f]/g, '-').slice(0, 100) || 'document') + '-copy.html';
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
      if (!panel.hidden && focusClaim && placement) iframe.contentWindow.postMessage({ type: 'kp:locate', nonce, id: focusClaim }, '*');
    } else if (message.type === 'kp:dismiss') {
      closePanel(false);
    } else if (['kp:open', 'kp:hover', 'kp:position'].includes(message.type) && typeof message.id === 'string' && claims.has(message.id) && validGeometry(message.geometry)) {
      if (message.type === 'kp:open') {
        dismissedClaim = null;
        openPanel(message.id, message.geometry);
      } else if (message.type === 'kp:position') {
        if (!panel.hidden && focusClaim === message.id && placement && placement.frame) {
          placement = { frame: true, geometry: message.geometry };
          placePanel();
        }
      } else if (!pinned && dismissedClaim !== message.id) {
        clearHover();
        const candidate = { id: message.id, geometry: message.geometry, nonce };
        hoverCandidate = candidate;
        hoverTimer = setTimeout(() => {
          if (hoverCandidate === candidate && !pinned && nonce === candidate.nonce) openPanel(candidate.id, candidate.geometry, true);
        }, 180);
      } else if (pinned && focusClaim) iframe.contentWindow.postMessage({ type: 'kp:locate', nonce, id: focusClaim }, '*');
    } else if (message.type === 'kp:leave' && typeof message.id === 'string' && claims.has(message.id)) {
      if (dismissedClaim === message.id) dismissedClaim = null;
      if (hoverCandidate && hoverCandidate.id === message.id) { clearTimeout(hoverTimer); hoverCandidate = null; }
      if (focusClaim === message.id) scheduleLeave();
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
  panel.addEventListener('pointerenter', () => { pointerInside = true; clearTimeout(leaveTimer); });
  panel.addEventListener('pointerleave', () => { pointerInside = false; scheduleLeave(); });
  panel.addEventListener('click', () => { holdPanelBounds(); pinPanel(); }, true);
  panel.addEventListener('focusin', pinPanel);
  panel.addEventListener('toggle', placePanel, true);
  if (typeof ResizeObserver === 'function') new ResizeObserver(placePanel).observe(panel);
  window.addEventListener('resize', () => {
    placePanel();
    if (!panel.hidden && focusClaim && placement) iframe.contentWindow.postMessage({ type: 'kp:locate', nonce, id: focusClaim }, '*');
  });
  window.addEventListener('scroll', placePanel, true);
  document.addEventListener('click', event => {
    // The clicked control may have been replaced by its action. Use the original
    // event path as well as containment, so an internal action is not click-away.
    const path = event.composedPath();
    if (panel.hidden || path.includes(panel) || path.includes(opener) ||
        panel.contains(event.target) || opener.contains(event.target) ||
        event.target.closest && event.target.closest('[data-kp-download]')) return;
    closePanel(false);
  });
  panel.addEventListener('keydown', event => { if (event.key === 'Escape') { event.preventDefault(); closePanel(); } });
  document.addEventListener('keydown', event => {
    if (event.key === 'Escape' && !panel.hidden) { event.preventDefault(); closePanel(panel.contains(document.activeElement)); }
  });
  try { loadFrame(selectedHTML()); renderPanel(); } catch (_) { announce(t('invalid')); }
}());
