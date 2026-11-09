'use strict';
/* A read-only view of the pet's snapshot, order, language and settings. */
var tauri = window.__TAURI__;
var state = { settings: {}, snapshot: { tasks: [], custom_names: {} }, metrics: null,
  expanded: false, pending: false, wanted: false, inside: false, height: 0, lang: 'en', hash: '' };
var islandEl = document.getElementById('island');
var panelEl = document.getElementById('panel');
var rowsEl = document.getElementById('rows');
var earsEl = document.getElementById('ears');
var hoverTimer, collapseTimer, jumpNoticeTimer;
var jumpNotice = null;
var reducedMotion = window.matchMedia('(prefers-reduced-motion: reduce)');

function invoke(cmd, args) { return tauri.core.invoke(cmd, args || {}); }
function listen(name, handler) { return tauri.event.listen(name, handler); }
function t(key, n) { return SessionUI.t(state.lang, key).replace('{n}', String(n)); }
function el(tag, className, text) {
  var node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}
function motionEnabled() {
  return state.settings.animations !== 'off' && (state.settings.animations === 'always' || !reducedMotion.matches);
}
function statusColor(status) {
  return 'var(--status-' + (status === 'waiting_input' || status === 'waiting_approval' ? 'waiting' : status) + ')';
}
function avatar(task) {
  return SessionUI.avatar(task);
}
function wave(live) {
  var node = el('span', 'wave' + (live ? ' live' : ''));
  node.setAttribute('aria-hidden', 'true');
  for (var i = 0; i < 3; i++) node.appendChild(el('i'));
  return node;
}
function counts() {
  var result = { running: [], waiting: [], failed: [] };
  state.snapshot.tasks.forEach(function (task) {
    if (task.status === 'running') result.running.push(task);
    else if (task.status === 'waiting_input' || task.status === 'waiting_approval') result.waiting.push(task);
    else if (task.status === 'failed') result.failed.push(task);
  });
  return result;
}
function agentNames(tasks) {
  return Array.from(new Set(tasks.map(function (task) { return task.agent_name; })));
}
function compactName(name) { return name === 'Claude Code' ? 'Claude' : name; }
function jumpNoticeMessage() { return jumpNotice ? t('toast.' + jumpNotice.reason) : ''; }
function showJumpOutcome(outcome) {
  clearTimeout(jumpNoticeTimer); jumpNotice = null;
  if (outcome && outcome.kind !== 'success' && outcome.reason_key &&
      (outcome.kind === 'failed' || state.settings.toast_on_degraded !== false)) {
    jumpNotice = { reason: outcome.reason_key, failed: outcome.kind === 'failed' };
    jumpNoticeTimer = setTimeout(function () { jumpNotice = null; renderEars(counts()); resizePanel(); }, 8000);
  }
  renderEars(counts()); resizePanel();
}
function renderEars(group) {
  var left = document.getElementById('activity');
  var right = document.getElementById('attention');
  left.replaceChildren(); right.replaceChildren();
  var tasks = state.snapshot.tasks;
  var active = group.running[0];
  if (active) {
    var busyNames = agentNames(group.running);
    var activeName = busyNames.length === 2 ? busyNames.map(compactName).join(' / ') : compactName(active.agent_name) + (busyNames.length > 2 ? ' +' + (busyNames.length - 1) : '');
    if (busyNames.length !== 2) left.appendChild(avatar(active));
    left.appendChild(el('span', 'ear-name', activeName));
    if (group.running.length) left.append(wave(true), el('span', 'ear-count', String(group.running.length)));
    else left.append(el('span', 'ear-count', String(tasks.length)));
    left.title = group.running.length ? t('islandActive', group.running.length) + ': ' + agentNames(group.running).join(', ') : t('allSessions', tasks.length);
  } else left.append(wave(false), el('span', 'ear-name', tasks.length ? t('allSessions', tasks.length) : 'Agent Monitor'));
  var waiting = group.waiting.length ? group.waiting : group.failed;
  right.className = 'ear ear-right' + (group.waiting.length ? ' ear-waiting' : group.failed.length ? ' ear-failed' : '');
  if (jumpNotice) {
    right.className = 'ear ear-right ' + (jumpNotice.failed ? 'ear-failed' : 'ear-waiting');
    right.append(el('span', 'attention-symbol', '!'), el('span', 'ear-name', jumpNoticeMessage()));
    right.title = jumpNoticeMessage();
  } else if (waiting.length) {
    var names = agentNames(waiting);
    right.append(el('span', 'attention-symbol', '!'), el('span', 'ear-name', compactName(names[0]) + (names.length > 1 ? ' +' + (names.length - 1) : '')),
      el('span', 'ear-count', t(group.waiting.length ? 'islandWaiting' : 'islandFailed', waiting.length)));
    right.title = names.join(', ') + '\n' + t(group.waiting.length ? 'islandWaiting' : 'islandFailed', waiting.length);
  } else {
    right.append(el('span', 'ear-count', t(tasks.length ? 'islandQuiet' : 'empty')));
    right.removeAttribute('title');
  }
  right.setAttribute('aria-live', 'polite');
  earsEl.setAttribute('aria-label', tasks.length ? t('collapsedAria', tasks.length).replace('{a}', String(group.waiting.length + group.failed.length)) : t('emptyAria'));
}
function renderRow(task) {
  var info = SessionUI.taskInfo(task, state.snapshot.custom_names, state.settings, state.lang);
  var row = el('button', 'row session-row'); row.type = 'button'; row.dataset.taskId = task.id;
  row.style.setProperty('--status', statusColor(task.status));
  row.title = [info.title, info.status, task.status_reason, info.model, info.summary, t('jumpHint')].filter(Boolean).join('\n');
  row.setAttribute('aria-label', [task.agent_name, info.title, info.host, info.status, info.model, info.level,
    task.pinned ? t('pinnedLabel') : '', t('jumpHint')].filter(Boolean).join(', '));
  row.appendChild(avatar(task));
  var body = el('div', 'row-content');
  var top = el('div', 'row-top');
  if (task.pinned) {
    var pin = el('span', 'pin');
    pin.innerHTML = '<svg viewBox="0 0 24 24" width="11" height="12" fill="none" stroke="currentColor" stroke-width="1.8"><path d="M9 3h6l-1 6 4 3v2H6v-2l4-3-1-6ZM12 14v7"/></svg>';
    pin.title = t('pinnedLabel'); top.appendChild(pin);
  }
  var time = el('span', 'row-time', info.time); time.dataset.ts = task.last_activity_secs || 0;
  if (isNewCompletedTask(task)) {
    // Same unread marker element as the pet row (shared builder + shared
    // styles): green dot before the title, click to acknowledge, no jump.
    row.classList.add('new-completed');
    top.appendChild(SessionUI.reads.marker(task, info.title, state.lang, function (t) {
      acknowledgeCompletions([t]);
    }));
  }
  top.append(el('span', 'row-title', info.title), time);
  var context = el('div', 'row-context');
  context.appendChild(el('span', 'row-agent', task.agent_name));
  if (info.host) context.appendChild(el('span', 'row-host', info.host));
  if (task.origin === 'synthetic') context.appendChild(el('span', 'sim', 'SIM'));
  var status = el('span', 'row-status session-state'); status.append(el('span', '', info.glyph), el('span', '', info.status));
  var model = el('span', 'row-model', info.model); model.title = info.modelTitle;
  context.appendChild(model);
  if (info.level) context.appendChild(el('span', 'row-level session-level ' + info.level.toLowerCase(), info.level));
  context.appendChild(status);
  body.append(top, context);
  row.appendChild(body);
  row.addEventListener('click', function () {
    showJumpOutcome(null);
    invoke('jump_to_task', { taskId: task.id }).then(showJumpOutcome).catch(function (error) {
      reportError(error); showJumpOutcome({ kind: 'failed', reason_key: 'jump_unavailable' });
    });
    clearTimeout(collapseTimer); state.inside = false; setExpanded(false);
  });
  return row;
}
function render() {
  var group = counts(); renderEars(group);
  document.getElementById('panel-title').textContent = t('sessionsTitle');
  document.getElementById('panel-meta').textContent = t('allSessions', state.snapshot.tasks.length);
  var status = document.getElementById('panel-status'); status.replaceChildren();
  var unread = state.settings.new_badge === false ? [] : unreadCompletions();
  if (unread.length) {
    // Green "new completions" chip, clickable = mark all as read (the pet
    // panel's mark-all button, folded into the island's chip row).
    var chip = el('button', 'status-count new-count');
    chip.type = 'button';
    chip.style.setProperty('--status', statusColor('completed'));
    chip.append(el('span', 'status-dot'), el('span', '', t('newBadge', unread.length)));
    chip.title = t('markAllRead');
    chip.addEventListener('click', function (event) {
      event.stopPropagation();
      acknowledgeCompletions(unread);
    });
    status.appendChild(chip);
  }
  ['waiting', 'running', 'failed'].forEach(function (key) {
    if (!group[key].length) return;
    var label = el('span', 'status-count'); label.style.setProperty('--status', statusColor(key === 'waiting' ? 'waiting_input' : key));
    label.append(el('span', 'status-dot'), el('span', '', t(key === 'waiting' ? 'islandWaiting' : key === 'running' ? 'islandActive' : 'islandFailed', group[key].length)));
    status.appendChild(label);
  });
  if (!status.children.length) status.appendChild(el('span', 'quiet', t('islandQuiet')));
  var scroll = rowsEl.scrollTop;
  var focused = document.activeElement && document.activeElement.dataset.taskId;
  // Snapshot order is authoritative, including pinning. Never slice the data.
  var nodes = state.snapshot.tasks.map(function (task) {
    var item = el('div', 'row-item'); item.setAttribute('role', 'listitem'); item.appendChild(renderRow(task)); return item;
  });
  if (!nodes.length) {
    var empty = el('div', 'empty'); empty.append(el('div', 'empty-title', t('empty')), el('div', 'empty-hint', t('emptyHint'))); nodes.push(empty);
  }
  rowsEl.replaceChildren.apply(rowsEl, nodes);
  rowsEl.scrollTop = scroll;
  if (focused) {
    var next = Array.from(rowsEl.querySelectorAll('.row')).find(function (row) { return row.dataset.taskId === focused; });
    if (next) next.focus({ preventScroll: true });
  }
  document.getElementById('foot-count').textContent = t('allSessions', state.snapshot.tasks.length);
  resizePanel();
}
function desiredHeight() {
  var cap = Math.max(1, Math.min(8, Number(state.settings.max_visible_rows) || 5));
  var items = Array.from(rowsEl.children).slice(0, cap);
  var listHeight = items.reduce(function (sum, item) { return sum + item.offsetHeight; }, 1);
  var top = parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--panel-top'));
  return Math.ceil(Math.min(top + document.querySelector('.panel-head').offsetHeight + listHeight + document.getElementById('panel-foot').offsetHeight,
    Math.min(680, state.metrics.screen_h - 60)));
}
function resizePanel() {
  if (!state.metrics) return;
  var height = desiredHeight();
  var hint = document.getElementById('foot-hint');
  hint.textContent = jumpNotice ? jumpNoticeMessage() : state.snapshot.tasks.length > Math.max(1, Math.min(8, Number(state.settings.max_visible_rows) || 5)) ? t('scrollSessions') : t('jumpHint');
  hint.title = jumpNotice ? jumpNoticeMessage() : '';
  hint.className = jumpNotice ? (jumpNotice.failed ? 'ear-failed' : 'ear-waiting') : '';
  if (state.expanded && !state.pending && height !== state.height) {
    state.height = height;
    invoke('island_expand', { hPt: height, motion: motionEnabled() }).catch(reportError);
  }
}
function reportError(error) { invoke('pet_debug_log', { msg: 'island: ' + error }).catch(function () {}); }

// ── Unread completions ──────────────────────────────────────────
//
// The read state lives in the shared SessionUI module — one truth for the
// pet panel and this island panel (same origin → same localStorage), which
// also owns the cross-window `storage` plumbing. This file only adds the
// island's rendering reactions and keeps the backend watch list honest
// while the pet window is hidden in island mode.

function isNewCompletedTask(task) {
  return state.settings.new_badge !== false && SessionUI.reads.isNew(task);
}

function unreadCompletions() {
  return state.snapshot.tasks.filter(isNewCompletedTask);
}

function syncCompletionWatch() {
  var completions = unreadCompletions().map(function (task) {
    return { task_id: task.id, completion_id: task.completion_id };
  });
  invoke('watch_unread_completions', { completions: completions }).catch(function () {});
}

function acknowledgeCompletions(tasks) {
  if (!SessionUI.reads.acknowledge(tasks, state.snapshot.tasks)) return;
  render();
  syncCompletionWatch();
}

function acknowledgeViewedCompletion(viewed) {
  var task = SessionUI.reads.viewedTask(viewed, state.snapshot.tasks);
  if (task) acknowledgeCompletions([task]);
}
function applyMode(expanded) {
  state.expanded = expanded;
  islandEl.classList.toggle('mode-expanded', expanded);
  islandEl.classList.toggle('mode-collapsed', !expanded);
  panelEl.inert = !expanded; earsEl.inert = expanded;
  earsEl.setAttribute('aria-expanded', String(expanded));
  if (!expanded && panelEl.contains(document.activeElement)) document.activeElement.blur();
}
function setExpanded(expanded) {
  if (!state.metrics) return;
  state.wanted = expanded;
  if (state.pending || state.expanded === expanded) return;
  state.pending = true;
  state.height = desiredHeight();
  applyMode(expanded);
  var delay = motionEnabled() ? (expanded ? 320 : 192) : 0;
  Promise.all([
    invoke(expanded ? 'island_expand' : 'island_collapse', { hPt: state.height, motion: motionEnabled() }).then(function (ok) { if (!ok) throw new Error('Window transition rejected'); }),
    new Promise(function (resolve) { setTimeout(resolve, delay); })
  ]).then(function () { return true; }, function (error) {
    applyMode(!expanded); state.wanted = !expanded; reportError(error); return false;
  }).then(function (ok) {
    state.pending = false;
    if (!ok) return;
    if (state.wanted !== state.expanded) { setExpanded(state.wanted); return; }
    resizePanel();
    if (!state.inside && state.expanded) hover(false);
    else if (state.inside && !state.expanded) hover(true);
  });
}
function hover(inside) {
  state.inside = inside; clearTimeout(hoverTimer); clearTimeout(collapseTimer);
  if (inside && !state.expanded) hoverTimer = setTimeout(function () { setExpanded(true); }, state.settings.hover_expand_ms || 150);
  if (!inside && state.expanded) collapseTimer = setTimeout(function () { setExpanded(false); }, state.settings.collapse_delay_ms || 800);
}
function applySettings(settings) {
  state.settings = settings; state.lang = SessionUI.resolveLanguage(settings.language);
  document.documentElement.lang = state.lang;
  document.documentElement.style.colorScheme = settings.appearance === 'light' || settings.appearance === 'dark' ? settings.appearance : 'light dark';
  document.documentElement.dataset.motion = motionEnabled() ? 'on' : 'off';
  render();
}
function applyMetrics(metrics) {
  state.metrics = metrics;
  islandEl.classList.toggle('has-notch', metrics.notch_w > 0);
  var css = document.documentElement.style;
  css.setProperty('--notch-w', metrics.notch_w + 'px');
  css.setProperty('--bar-h', metrics.bar_h + 'px');
  css.setProperty('--panel-w', metrics.panel_w + 'px');
  css.setProperty('--panel-top', (metrics.notch_w ? metrics.bar_h + 8 : 10) + 'px');
}
async function boot() {
  if (!tauri) return;
  try {
    await listen('tasks-updated', function (event) {
      var hash = JSON.stringify([event.payload.tasks, event.payload.custom_names]);
      if (state.hash === hash) return;
      state.hash = hash; state.snapshot = event.payload; render();
    });
    await listen('settings-changed', function (event) { applySettings(event.payload); });
    await listen('island-hover', function (event) { hover(event.payload); });
    await listen('completion-viewed', function (event) { acknowledgeViewedCompletion(event.payload); });
    // The pet window (or another island session) wrote the read state: adopt
    // it so both surfaces agree on what is still unread. The shared module
    // owns the storage-event plumbing.
    SessionUI.reads.bind(function () {
      render();
      syncCompletionWatch();
    });
    await listen('island-geometry', function (event) {
      if (event.payload.metrics) { applyMetrics(event.payload.metrics); resizePanel(); }
      if (event.payload.mode === 'collapsed' && state.expanded && !state.pending) {
        applyMode(false); state.wanted = false;
      }
    });
    var data = await Promise.all([invoke('island_metrics'), invoke('get_settings'), invoke('get_snapshot')]);
    if (!data[0]) return;
    applyMetrics(data[0]); state.snapshot = data[2];
    state.hash = JSON.stringify([data[2].tasks, data[2].custom_names]); applySettings(data[1]);
    syncCompletionWatch();
    islandEl.addEventListener('mouseenter', function () { hover(true); });
    islandEl.addEventListener('mouseleave', function () { hover(false); });
    earsEl.addEventListener('click', function () { state.inside = true; setExpanded(true); });
    document.addEventListener('keydown', function (event) { if (event.key === 'Escape') { state.inside = false; setExpanded(false); } });
    reducedMotion.addEventListener('change', function () { applySettings(state.settings); });
    setInterval(function () {
      if (!state.expanded) return;
      rowsEl.querySelectorAll('.row-time').forEach(function (node) { node.textContent = SessionUI.relativeTime(Number(node.dataset.ts), state.lang); });
    }, 30000);
  } catch (error) { reportError(error); }
}
boot();
