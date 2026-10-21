'use strict';

/* Agent Monitor — desktop pet UI (vanilla JS, no bundler).
 *
 * The UI is a pure renderer of the Rust snapshot. It never parses agent files
 * and never branches on agent names; agent identity/colour is data-driven.
 *
 * Performance rules:
 *   - no timer faster than 30 s (one shared timer refreshes relative times),
 *   - no animation while collapsed/idle,
 *   - duplicate snapshots are ignored (render only on change).
 */

// ── Tauri bridge (with a browser mock for layout work) ──────────

var MOCK = location.search.indexOf('mock=') !== -1;
var tauri = window.__TAURI__ || null;

function invoke(cmd, args) {
  if (MOCK) return Promise.resolve(mockInvoke(cmd, args));
  if (!tauri || !tauri.core) return Promise.reject(new Error('Tauri bridge unavailable'));
  return tauri.core.invoke(cmd, args || {});
}

function listen(event, handler) {
  if (MOCK) return Promise.resolve(function () {});
  if (!tauri || !tauri.event) return Promise.resolve(function () {});
  return tauri.event.listen(event, handler);
}

function startWindowDrag() {
  if (MOCK) return;
  try {
    if (tauri && tauri.window) tauri.window.getCurrentWindow().startDragging();
  } catch (e) { /* dragging is best-effort */ }
}

// ── i18n ────────────────────────────────────────────────────────

var I18N = {
  en: {
    title: 'AGENT MONITOR',
    empty: 'No active sessions',
    emptyHint: 'Start a Codex, DSH, ZCode or Claude Code session',
    sessions: 'sessions',
    needAttention: 'need attention',
    clear: 'Clear',
    dragHint: 'Drag to move',
    collapsedAria: 'Agent Monitor, {n} sessions, {a} need attention, click to expand',
    emptyAria: 'Agent Monitor, no active sessions, click to expand',
    status: {
      running: 'Running', waiting_input: 'Waiting for input',
      waiting_approval: 'Waiting for approval', completed: 'Completed',
      failed: 'Failed', idle: 'Idle', unknown: 'Unknown'
    },
    jumpHint: 'Click to open',
    toast: {
      opened_window: 'Opened the window — the exact session could not be selected.',
      opened_session: '',
      opened_terminal_tab: '',
      activated_app: '',
      activated_app_only: 'Activated the app — the window could not be identified.',
      window_not_found: 'Activated the app — matching window not found.',
      window_ambiguous: 'Matched more than one window — session not distinguished.',
      deep_link_unverified: 'Session link not confirmed — opened the app instead.',
      focus_unconfirmed: 'Focus requested, result could not be confirmed.',
      needs_accessibility: 'Accessibility permission is required to locate windows.',
      needs_automation: 'Automation permission is required to locate windows.',
      app_not_found: 'The application is not installed or cannot be opened.',
      target_invalid: 'The target is no longer available.',
      workspace_missing: 'The workspace folder no longer exists.',
      rollout_missing: 'The session log is gone.',
      terminal_tab_not_found: 'No matching terminal tab was found.',
      no_target: 'This task has no jump target.',
      no_session_route: 'No session-level route is available.',
      no_window_hint: 'No window title is available to match.',
      task_gone: 'The task no longer exists.',
      grant: 'Grant permission'
    }
  },
  zh: {
    title: 'AGENT MONITOR',
    empty: '暂无活跃会话',
    emptyHint: '启动 Codex、DSH、ZCode 或 Claude Code 会话',
    sessions: '个会话',
    needAttention: '个待处理',
    clear: '清除',
    dragHint: '拖动以移动',
    collapsedAria: 'Agent Monitor，{n} 个会话，{a} 个待处理，点击展开',
    emptyAria: 'Agent Monitor，暂无活跃会话，点击展开',
    status: {
      running: '运行中', waiting_input: '等待输入',
      waiting_approval: '等待审批', completed: '完成',
      failed: '失败', idle: '空闲', unknown: '未知'
    },
    jumpHint: '点击跳转',
    toast: {
      opened_window: '已打开对应窗口，无法定位具体会话。',
      opened_session: '',
      opened_terminal_tab: '',
      activated_app: '',
      activated_app_only: '已激活应用，无法识别对应窗口。',
      window_not_found: '已激活应用，未找到匹配窗口。',
      window_ambiguous: '匹配到多个窗口，无法区分具体会话。',
      deep_link_unverified: '未能确认会话链接，已改为打开应用。',
      focus_unconfirmed: '已请求聚焦，结果未确认。',
      needs_accessibility: '需要辅助功能权限才能定位窗口。',
      needs_automation: '需要自动化权限才能定位窗口。',
      app_not_found: '应用未安装或无法打开。',
      target_invalid: '目标已不可用。',
      workspace_missing: '工作区目录已不存在。',
      rollout_missing: '会话日志已不存在。',
      terminal_tab_not_found: '未找到匹配的终端标签页。',
      no_target: '该任务没有可跳转目标。',
      no_session_route: '没有可用的会话级跳转方式。',
      no_window_hint: '没有可用于匹配的窗口标题。',
      task_gone: '任务已不存在。',
      grant: '去授权'
    }
  }
};

var LANG_KEY = 'am-lang';
var lang = 'en';
try {
  var saved = localStorage.getItem(LANG_KEY);
  lang = saved === 'zh' || saved === 'en'
    ? saved
    : (navigator.language || '').toLowerCase().indexOf('zh') === 0 ? 'zh' : 'en';
} catch (e) { /* localStorage may be unavailable */ }

function t(key) {
  var table = I18N[lang] || I18N.en;
  var parts = key.split('.');
  var v = table;
  for (var i = 0; i < parts.length; i++) { v = v && v[parts[i]]; }
  if (typeof v === 'string') return v;
  return key;
}

// ── DOM refs ────────────────────────────────────────────────────

var appEl = document.getElementById('app');
var petEl = document.getElementById('pet');
var badgeEl = document.getElementById('badge');
var panelEl = document.getElementById('panel');
var listEl = document.getElementById('list');
var dragHandleEl = document.getElementById('drag-handle');
var collapseBtn = document.getElementById('collapse-btn');
var attentionEl = document.getElementById('attention');
var footerCountEl = document.getElementById('footer-count');
var footerHintEl = document.getElementById('footer-hint');
var langBtn = document.getElementById('lang-btn');
var toastEl = document.getElementById('toast');
var toastTextEl = document.getElementById('toast-text');
var toastActionEl = document.getElementById('toast-action');

// ── State ───────────────────────────────────────────────────────

var snapshot = { tasks: [], attention_count: 0, agents: [] };
var petState = {
  mode: 'collapsed', edge: 'none', hidden: false,
  max_visible_rows: 5, hover_expand_ms: 150, collapse_delay_ms: 800
};
var lastRenderedHash = '';
var pendingJump = null;
var jumpCalls = [];
var hoverTimer = null;
var collapseTimer = null;
var suppressLeaveUntil = 0;
var toastTimer = null;
var pointerDown = null;
var dragged = false;

var STATUS_GLYPH = {
  running: '▶', waiting_input: '?', waiting_approval: '!',
  completed: '✓', failed: '✕', idle: '○', unknown: '·'
};

var AGENT_COLORS = ['--cyan', '--green', '--purple', '--blue', '--amber', '--pink'];
function agentColor(key) {
  var h = 0;
  for (var i = 0; i < key.length; i++) h = (h * 31 + key.charCodeAt(i)) >>> 0;
  return 'var(' + AGENT_COLORS[h % AGENT_COLORS.length] + ')';
}

// ── Helpers ─────────────────────────────────────────────────────

function relativeTime(epochSecs) {
  if (!epochSecs) return '';
  var diff = Math.max(0, Math.floor(Date.now() / 1000) - epochSecs);
  if (diff < 60) return lang === 'zh' ? '刚刚' : 'now';
  if (diff < 3600) return Math.floor(diff / 60) + 'm';
  if (diff < 86400) return Math.floor(diff / 3600) + 'h';
  return Math.floor(diff / 86400) + 'd';
}

function levelClass(level) {
  var l = (level || 'L0').toLowerCase();
  return 'level ' + l;
}

function el(tag, className, text) {
  var node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined && text !== null) node.textContent = text;
  return node;
}

// ── Rendering ───────────────────────────────────────────────────

function render(snapshotData) {
  if (!snapshotData) return;
  var hash = JSON.stringify(snapshotData.tasks) + '|' + snapshotData.attention_count;
  snapshot = snapshotData;
  if (hash === lastRenderedHash) return;
  lastRenderedHash = hash;

  var tasks = snapshot.tasks || [];
  var attention = snapshot.attention_count || 0;
  var hasTasks = tasks.length > 0;

  appEl.classList.toggle('no-tasks', !hasTasks);

  badgeEl.hidden = attention === 0;
  badgeEl.textContent = attention > 99 ? '99+' : String(attention);

  petEl.setAttribute('aria-label', hasTasks
    ? t('collapsedAria').replace('{n}', tasks.length).replace('{a}', attention)
    : t('emptyAria'));

  attentionEl.hidden = attention === 0;
  attentionEl.textContent = '⚠ ' + attention;

  footerCountEl.textContent = tasks.length + ' ' + t('sessions');
  footerHintEl.textContent = attention > 0 ? attention + ' ' + t('needAttention') : '';
  langBtn.textContent = lang === 'en' ? '中' : 'EN';

  if (petState.mode === 'expanded' || petState.mode === 'dragging') renderList(tasks);
  if (MOCK && window.__AM_TEST__) window.__AM_TEST__.lastHash = hash;
}

function renderList(tasks) {
  var previousScroll = listEl.scrollTop;
  listEl.textContent = '';

  if (!tasks.length) {
    var empty = el('div', 'empty');
    empty.appendChild(el('div', '', t('empty')));
    empty.appendChild(el('div', 'dim', t('emptyHint')));
    listEl.appendChild(empty);
    return;
  }

  for (var i = 0; i < tasks.length; i++) listEl.appendChild(renderTask(tasks[i]));

  listEl.scrollTop = previousScroll;
}

function renderTask(task) {
  var row = document.createElement('button');
  row.type = 'button';
  row.className = 'task status-' + task.status + (task.status === 'waiting_input' ||
    task.status === 'waiting_approval' || task.status === 'failed' ? ' attention' : '');
  row.setAttribute('role', 'listitem');
  row.dataset.taskId = task.id;
  row.style.setProperty('--agent-color', agentColor(task.agent_key || ''));

  var statusLabel = t('status.' + task.status) || task.status;
  row.setAttribute('aria-label',
    task.agent_name + ', ' + task.title + ', ' + statusLabel + ', ' +
    (task.level || 'L0') + ', ' + t('jumpHint'));
  row.title = task.title + '\n' + statusLabel + ' · ' +
    (task.status_reason || '') + '\n' + t('jumpHint') + ' (' + (task.level || 'L0') + ')';

  var top = el('div', 'task-top');
  top.appendChild(el('span', 'agent', task.agent_name));
  if (task.origin === 'synthetic') top.appendChild(el('span', 'sim', 'SIM'));
  var title = el('span', 'title', task.title);
  title.title = task.title;
  top.appendChild(title);

  var status = el('div', 'status');
  status.appendChild(el('span', 'status-glyph', STATUS_GLYPH[task.status] || '·'));
  var statusText = statusLabel;
  if (task.summary) statusText += ' · ' + task.summary;
  status.appendChild(el('span', 'status-text', statusText));

  var meta = el('div', 'meta');
  var timeEl = el('span', 'time', relativeTime(task.last_activity_secs));
  timeEl.dataset.ts = String(task.last_activity_secs || 0);
  meta.appendChild(timeEl);
  meta.appendChild(el('span', levelClass(task.level), task.level || 'L0'));

  var dismiss = el('span', 'dismiss', '×');
  dismiss.setAttribute('role', 'button');
  dismiss.setAttribute('aria-label', t('clear') + ': ' + task.title);
  dismiss.title = t('clear');
  dismiss.addEventListener('click', function (event) {
    event.stopPropagation();
    event.preventDefault();
    invoke('dismiss_task', { taskId: task.id }).catch(function () {});
  });

  row.appendChild(top);
  row.appendChild(status);
  row.appendChild(meta);
  row.appendChild(dismiss);

  row.addEventListener('click', function (event) {
    event.stopPropagation();
    if (dragged) { dragged = false; return; }
    jumpToTask(task);
  });

  return row;
}

function refreshTimes() {
  var nodes = listEl.querySelectorAll('.time[data-ts]');
  for (var i = 0; i < nodes.length; i++) {
    nodes[i].textContent = relativeTime(Number(nodes[i].dataset.ts));
  }
}

// ── Jump + feedback ─────────────────────────────────────────────

function jumpToTask(task) {
  if (pendingJump === task.id) return; // exactly one jump per click
  pendingJump = task.id;
  jumpCalls.push(task.id);
  invoke('jump_to_task', { taskId: task.id }).then(function (outcome) {
    pendingJump = null;
    handleOutcome(outcome);
  }).catch(function (err) {
    pendingJump = null;
    showToast(String(err && err.message ? err.message : err), 'error', null);
  });
}

function handleOutcome(outcome) {
  if (!outcome) return;
  // Success needs no toast; degradation/failure always does.
  if (outcome.kind === 'success') return;
  var key = 'toast.' + (outcome.reason_key || 'target_invalid');
  var message = t(key);
  if (!message || message === key) message = outcome.reason_key || t('toast.target_invalid');
  showToast(message, outcome.kind === 'failed' ? 'error' : 'warn', outcome.permission_needed);
}

function showToast(message, kind, permissionNeeded) {
  if (!message) return;
  toastTextEl.textContent = message;
  toastEl.className = '';
  if (kind === 'error') toastEl.classList.add('error');
  else if (kind === 'warn') toastEl.classList.add('warn');
  toastEl.hidden = false;

  if (permissionNeeded) {
    toastActionEl.hidden = false;
    toastActionEl.textContent = t('toast.grant');
    toastActionEl.onclick = function (event) {
      event.stopPropagation();
      invoke('open_permission_settings', { kind: permissionNeeded }).catch(function () {});
      hideToast();
    };
  } else {
    toastActionEl.hidden = true;
    toastActionEl.onclick = null;
  }

  if (toastTimer) clearTimeout(toastTimer);
  toastTimer = setTimeout(hideToast, 4200);
}

function hideToast() {
  toastEl.hidden = true;
  if (toastTimer) { clearTimeout(toastTimer); toastTimer = null; }
}

// ── Pet state machine (visual side) ─────────────────────────────

function applyPetState(next) {
  if (!next) return;
  petState = Object.assign({}, petState, next);
  var mode = petState.mode || 'collapsed';
  appEl.className = appEl.className
    .replace(/\bmode-[a-z_]+\b/g, '')
    .trim();
  appEl.classList.add('mode-' + mode);
  if (!snapshot.tasks || snapshot.tasks.length === 0) appEl.classList.add('no-tasks');
  appEl.dataset.edge = petState.edge || 'none';

  var expanded = mode === 'expanded' || mode === 'dragging';
  panelEl.hidden = !expanded;
  if (expanded) {
    renderList(snapshot.tasks || []);
    listEl.scrollTop = listEl.scrollTop; // preserve while re-showing
  }
}

function setExpanded(expanded) {
  invoke('set_expanded', { expanded: !!expanded }).catch(function () {});
}

function requestCollapse() {
  invoke('request_collapse', {}).catch(function () {});
}

// ── Hover behaviour ─────────────────────────────────────────────

function enterPet() {
  if (collapseTimer) { clearTimeout(collapseTimer); collapseTimer = null; }
  var mode = petState.mode;
  if (mode === 'collapsed' || mode === 'edge_hidden') {
    if (hoverTimer) return;
    hoverTimer = setTimeout(function () {
      hoverTimer = null;
      suppressLeaveUntil = Date.now() + 500;
      setExpanded(true);
    }, petState.hover_expand_ms || 150);
  }
}

function leavePet() {
  if (hoverTimer) { clearTimeout(hoverTimer); hoverTimer = null; }
  if (Date.now() < suppressLeaveUntil) return;
  if (petState.mode !== 'expanded') return;
  if (collapseTimer) clearTimeout(collapseTimer);
  collapseTimer = setTimeout(function () {
    collapseTimer = null;
    requestCollapse();
  }, petState.collapse_delay_ms || 800);
}

appEl.addEventListener('mouseenter', enterPet);
appEl.addEventListener('mouseleave', leavePet);

// ── Pointer / drag handling ─────────────────────────────────────

function onPointerDown(event, source) {
  if (event.button !== 0) return;
  pointerDown = { x: event.clientX, y: event.clientY, source: source, moved: false };
  dragged = false;
}

function onPointerMove(event) {
  if (!pointerDown) return;
  var dx = event.clientX - pointerDown.x;
  var dy = event.clientY - pointerDown.y;
  if (!pointerDown.moved && (Math.abs(dx) > 4 || Math.abs(dy) > 4)) {
    pointerDown.moved = true;
    dragged = true;
    startWindowDrag();
  }
}

function onPointerUp() {
  var wasClick = pointerDown && !pointerDown.moved;
  pointerDown = null;
  // Reset `dragged` on the next tick so the click handler (which fires after
  // pointerup) can still see it and suppress the jump.
  if (!wasClick) setTimeout(function () { dragged = false; }, 0);
}

petEl.addEventListener('mousedown', function (event) {
  if (petState.mode === 'expanded') return;
  onPointerDown(event, 'pet');
});
petEl.addEventListener('click', function (event) {
  event.stopPropagation();
  if (dragged) { dragged = false; return; }
  setExpanded(true);
});
petEl.addEventListener('keydown', function (event) {
  if (event.key === 'Enter' || event.key === ' ') {
    event.preventDefault();
    setExpanded(true);
  }
});

dragHandleEl.addEventListener('mousedown', function (event) {
  onPointerDown(event, 'handle');
  startWindowDrag();
});
document.addEventListener('mousemove', onPointerMove);
document.addEventListener('mouseup', onPointerUp);

collapseBtn.addEventListener('click', function (event) {
  event.stopPropagation();
  setExpanded(false);
});

document.addEventListener('keydown', function (event) {
  if (event.key === 'Escape') {
    hideToast();
    if (petState.mode === 'expanded') setExpanded(false);
  }
});

listEl.addEventListener('scroll', function () {
  // Scrolling must never be interpreted as a collapse trigger.
  if (collapseTimer) { clearTimeout(collapseTimer); collapseTimer = null; }
}, { passive: true });

langBtn.addEventListener('click', function (event) {
  event.stopPropagation();
  lang = lang === 'en' ? 'zh' : 'en';
  try { localStorage.setItem(LANG_KEY, lang); } catch (e) { /* ignore */ }
  appEl.setAttribute('data-lang', lang);
  lastRenderedHash = '';
  render(snapshot);
  applyPetState({});
});

// ── Mock fixture (browser layout work only) ─────────────────────

function mockSnapshot() {
  var now = Math.floor(Date.now() / 1000);
  return {
    collected_at_secs: now,
    attention_count: 2,
    agents: [
      { key: 'codex', name: 'Codex' }, { key: 'dsh', name: 'DSH' },
      { key: 'zcode', name: 'ZCode' }, { key: 'claude-code', name: 'Claude Code' }
    ],
    tasks: [
      { id: 'codex:1', agent_key: 'codex', agent_name: 'Codex', host: 'desktop', title: 'Refactor the pet geometry and snap rules', status: 'running', status_reason: 'task_started', last_activity_secs: now - 20, collected_at_secs: now, summary: 'Editing pet.rs', origin: 'index', target: {}, target_validity: 'valid', invalid_reason: null, capability: { max_level: 'L3', notes: '', requires: [] }, level: 'L3', pinned: false },
      { id: 'dsh:2', agent_key: 'dsh', agent_name: 'DSH', host: 'desktop', title: 'Prove the finite-sample bridge conjecture', status: 'waiting_approval', status_reason: 'approval/policy', last_activity_secs: now - 90, collected_at_secs: now, summary: null, origin: 'cache', target: {}, target_validity: 'valid', invalid_reason: null, capability: { max_level: 'L3', notes: '', requires: [] }, level: 'L3', pinned: false },
      { id: 'zcode:3', agent_key: 'zcode', agent_name: 'ZCode', host: 'desktop', title: '美化论文 Figure 1 与 ML 写作规范', status: 'completed', status_reason: 'task_status = completed', last_activity_secs: now - 3600, collected_at_secs: now, summary: 'GLM-5.3-Flash', origin: 'index', target: {}, target_validity: 'valid', invalid_reason: null, capability: { max_level: 'L2', notes: '', requires: [] }, level: 'L2', pinned: false },
      { id: 'claude-code:4', agent_key: 'claude-code', agent_name: 'Claude Code', host: 'vscode', title: 'Fix the failing LaTeX build in the appendix', status: 'waiting_input', status_reason: 'ask user question', last_activity_secs: now - 240, collected_at_secs: now, summary: 'OPUS · 12 msg', origin: 'event', target: {}, target_validity: 'valid', invalid_reason: null, capability: { max_level: 'L2', notes: '', requires: [] }, level: 'L2', pinned: false },
      { id: 'dsh:5', agent_key: 'dsh', agent_name: 'DSH', host: 'desktop', title: 'Training-free DRE literature positioning audit with a very long title that must be ellipsized', status: 'failed', status_reason: 'turn aborted', last_activity_secs: now - 7200, collected_at_secs: now, summary: 'turn 3', origin: 'cache', target: {}, target_validity: 'invalid', invalid_reason: 'workspace_missing', capability: { max_level: 'L3', notes: '', requires: [] }, level: 'L0', pinned: false },
      { id: 'sim:6', agent_key: 'simulated', agent_name: 'Simulated Agent', host: 'fixture', title: 'Extension acceptance fixture', status: 'running', status_reason: 'fixture', last_activity_secs: now - 5, collected_at_secs: now, summary: 'fixture', origin: 'synthetic', target: {}, target_validity: 'unconfirmed', invalid_reason: null, capability: { max_level: 'L1', notes: '', requires: [] }, level: 'L1', pinned: false }
    ]
  };
}

function mockInvoke(cmd, args) {
  if (cmd === 'get_snapshot' || cmd === 'refresh') return mockSnapshot();
  if (cmd === 'set_expanded') {
    applyPetState({ mode: args && args.expanded ? 'expanded' : 'collapsed' });
    return true;
  }
  if (cmd === 'request_collapse') {
    if (petState.mode === 'expanded') applyPetState({ mode: 'collapsed' });
    return true;
  }
  if (cmd === 'dismiss_task') {
    snapshot.tasks = (snapshot.tasks || []).filter(function (t) { return t.id !== args.taskId; });
    snapshot.attention_count = snapshot.tasks.filter(function (t) {
      return t.status === 'waiting_input' || t.status === 'waiting_approval' || t.status === 'failed';
    }).length;
    lastRenderedHash = '';
    render(snapshot);
    return true;
  }
  if (cmd === 'get_pet_state') {
    return { mode: 'expanded', edge: 'right', hidden: false, max_visible_rows: 5, hover_expand_ms: 150, collapse_delay_ms: 800 };
  }
  if (cmd === 'jump_to_task') {
    return { task_id: 'mock', kind: 'degraded', requested: 'L3', achieved: 'L2', reason_key: 'opened_window', detail: null, next_step: null, permission_needed: null };
  }
  return true;
}

// ── Mock-only test hooks (never active in the packaged app) ─────

if (MOCK) {
  window.__AM_TEST__ = {
    get snapshot() { return snapshot; },
    get petState() { return petState; },
    get jumpCalls() { return jumpCalls.slice(); },
    render: render,
    applyPetState: applyPetState,
    setMockSnapshot: function (next) { lastRenderedHash = ''; render(next); },
    statusGlyphs: Object.keys(STATUS_GLYPH),
    languages: Object.keys(I18N),
    t: t
  };
}

// ── Bootstrap ───────────────────────────────────────────────────

function boot() {
  appEl.setAttribute('data-lang', lang);
  document.addEventListener('contextmenu', function (event) { event.preventDefault(); });

  if (MOCK) {
    document.body.classList.add('mock');
    render(mockSnapshot());
    var collapsed = location.search.indexOf('mock=collapsed') !== -1;
    applyPetState(collapsed
      ? { mode: 'collapsed', edge: 'right', hidden: false }
      : mockInvoke('get_pet_state'));
    footerHintEl.textContent = 'SIM';
    return;
  }

  invoke('get_snapshot').then(render).catch(function (err) {
    listEl.textContent = '';
    var empty = el('div', 'empty');
    empty.appendChild(el('div', '', 'ERR'));
    empty.appendChild(el('div', 'dim', String(err)));
    listEl.appendChild(empty);
  });

  invoke('get_pet_state').then(applyPetState).catch(function () {});

  listen('tasks-updated', function (event) { render(event.payload); });
  listen('pet-state', function (event) { applyPetState(event.payload); });
  listen('jump-feedback', function (event) { handleOutcome(event.payload); });

  // One shared timer: only relative-time labels change, no re-render.
  setInterval(refreshTimes, 30000);
}

boot();
