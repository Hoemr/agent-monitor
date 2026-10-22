'use strict';

/* Agent Monitor — desktop pet UI (vanilla JS, no bundler).
 *
 * The UI is a pure renderer of the Rust snapshot. It never parses agent files
 * and never branches on agent names; agent identity/colour is data-driven.
 *
 * Settings are owned by Rust (single source of truth): the page reads them on
 * boot via `get_settings`, applies them immediately, and follows the
 * `settings-changed` event — no reload, no polling, no localStorage copy.
 *
 * Performance rules:
 *   - no timer faster than 30 s (one shared timer refreshes relative times),
 *   - no animation while collapsed/idle (and `animations=off` removes them all),
 *   - duplicate snapshots are ignored (render only on change).
 */

// ── Tauri bridge (with a browser mock for layout work) ──────────

var MOCK = location.search.indexOf('mock=') !== -1;
var tauri = window.__TAURI__ || null;
var mockListeners = {};

function invoke(cmd, args) {
  if (MOCK) return Promise.resolve(mockInvoke(cmd, args));
  if (!tauri || !tauri.core) return Promise.reject(new Error('Tauri bridge unavailable'));
  return tauri.core.invoke(cmd, args || {});
}

function listen(event, handler) {
  if (MOCK) {
    (mockListeners[event] = mockListeners[event] || []).push(handler);
    return Promise.resolve(function () {
      var list = mockListeners[event] || [];
      var index = list.indexOf(handler);
      if (index !== -1) list.splice(index, 1);
    });
  }
  if (!tauri || !tauri.event) return Promise.resolve(function () {});
  return tauri.event.listen(event, handler);
}

function mockEmit(event, payload) {
  var list = (mockListeners[event] || []).slice();
  for (var i = 0; i < list.length; i++) {
    try { list[i]({ event: event, payload: payload }); } catch (e) { /* isolate handlers */ }
  }
}

function startWindowDrag() {
  if (MOCK) return;
  try {
    if (tauri && tauri.window) tauri.window.getCurrentWindow().startDragging();
  } catch (e) { /* dragging is best-effort */ }
}

// ── Settings (Rust is authoritative; this is a read-through mirror) ─

function defaultSettings() {
  return {
    version: 1,
    language: 'system',
    appearance: 'system',
    show_summary: true,
    show_jump_level: true,
    refresh_interval: 'auto',
    retention_hours: 6,
    enabled_agents: { codex: true, dsh: true, zcode: true, claude_code: true },
    max_visible_rows: 5,
    hover_expand_ms: 150,
    collapse_delay_ms: 800,
    snap_distance: 16,
    hidden_sliver: 14,
    collapsed_size: 72,
    auto_hide_after_s: 0,
    edge_hide_enabled: true,
    animations: 'system',
    new_badge: true,
    icon_style: 'builtin',
    toast_on_degraded: true,
    auto_dismiss_invalid: false,
    legacy_imported: false
  };
}

var settings = defaultSettings();

function resolveLanguage(value) {
  if (value === 'zh') return 'zh';
  if (value === 'en') return 'en';
  return (navigator.language || '').toLowerCase().indexOf('zh') === 0 ? 'zh' : 'en';
}

// ── i18n ────────────────────────────────────────────────────────

var I18N = {
  en: {
    sessionsTitle: 'Sessions',
    empty: 'No active sessions',
    emptyHint: 'Start a Codex, DSH, ZCode or Claude Code session',
    sessions: 'sessions',
    visibleCountTitle: 'Showing {v} of {t} sessions',
    needAttention: 'need attention',
    clear: 'Clear',
    collapse: 'Collapse',
    settings: 'Settings',
    undo: 'Undo',
    dismissed: 'Removed from the list.',
    dragHint: 'Drag to move',
    notice: 'Notice: ',
    newBadge: '{n} new',
    newAria: '{n} newly completed',
    questionChip: '{n} waiting',
    questionChipAria: '{n} sessions waiting for your answer',
    questionAria: '{n} waiting for your answer',
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
      ax_unavailable: 'Could not read the DSH sidebar (Accessibility permission or an unsupported app version).',
      session_row_not_found: 'The session is not visible in the DSH sidebar — expand its project or scroll it into view.',
      session_ambiguous: 'Several sessions share this title; project/time evidence could not tell them apart.',
      session_click_unverified: 'Clicked the session row, but the window did not switch.',
      task_gone: 'The task no longer exists.',
      grant: 'Grant permission'
    }
  },
  zh: {
    sessionsTitle: '会话',
    empty: '暂无活跃会话',
    emptyHint: '启动 Codex、DSH、ZCode 或 Claude Code 会话',
    sessions: '个会话',
    visibleCountTitle: '当前窗口显示 {v} 个，共 {t} 个会话',
    needAttention: '个待处理',
    clear: '清除',
    collapse: '收起',
    settings: '设置',
    undo: '撤销',
    dismissed: '已从列表移除。',
    dragHint: '拖动以移动',
    notice: '提示：',
    newBadge: '{n} 个新完成',
    newAria: '{n} 个新完成',
    questionChip: '{n} 待回答',
    questionChipAria: '{n} 个会话等待你的回答',
    questionAria: '{n} 个会话等待你的回答',
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
      ax_unavailable: '无法读取 DSH 侧栏（需要辅助功能权限，或应用版本不兼容）。',
      session_row_not_found: 'DSH 侧栏中没有该会话——请展开所属项目或滚动到可见位置。',
      session_ambiguous: '存在同名会话，项目/时间信息不足以区分。',
      session_click_unverified: '已点击会话行，但窗口没有切换。',
      task_gone: '任务已不存在。',
      grant: '去授权'
    }
  }
};

var lang = resolveLanguage(settings.language);

function t(key) {
  var table = I18N[lang] || I18N.en;
  var parts = key.split('.');
  var value = table;
  for (var i = 0; i < parts.length; i++) { value = value && value[parts[i]]; }
  if (typeof value === 'string') return value;
  return key;
}

// ── DOM refs ────────────────────────────────────────────────────

var appEl = document.getElementById('app');
var petEl = document.getElementById('pet');
var petIconEl = document.getElementById('pet-icon');
var glyphEl = petEl.querySelector('.pet-glyph');
var badgeEl = document.getElementById('badge');
var newBadgeEl = document.getElementById('new-badge');
var noticeDotEl = document.getElementById('notice-dot');
var panelEl = document.getElementById('panel');
var listEl = document.getElementById('list');
var dragbarEl = document.getElementById('dragbar');
var collapseBtn = document.getElementById('collapse-btn');
var headerMetaEl = document.getElementById('header-meta');
var headerQuestionEl = document.getElementById('header-question');
var headerNewEl = document.getElementById('header-new');
var panelTitleEl = document.getElementById('panel-title');
var footerCountEl = document.getElementById('footer-count');
var footerHintEl = document.getElementById('footer-hint');
var settingsBtn = document.getElementById('settings-btn');
var toastEl = document.getElementById('toast');
var toastTextEl = document.getElementById('toast-text');
var toastActionEl = document.getElementById('toast-action');

// ── State ───────────────────────────────────────────────────────

var snapshot = { tasks: [], attention_count: 0, question_count: 0, agents: [] };
var petState = {
  mode: 'collapsed', edge: 'none', hidden: false, hidden_to_tray: false,
  max_visible_rows: 5, hover_expand_ms: 150, collapse_delay_ms: 800
};
var FOOTER_HINT = MOCK ? 'SIM' : '';
var lastRenderedHash = '';
var pendingJump = null;
var jumpCalls = [];
var hoverTimer = null;
var collapseTimer = null;
var suppressLeaveUntil = 0;
// Jump-feedback notice: state outlives the surface it is painted on (toast vs.
// dot), so a collapse in the middle of a jump never strands text in the pet.
var noticeExpiry = { expanded: 4200, collapsed: 30000 };
var notice = null;
var noticeSurface = null; // 'toast' while expanded/dragging, 'dot' while closed
var noticeTimer = null;
var noticeExpiresAt = 0;
// Rust emits `jump-feedback` for every jump *and* returns the same outcome from
// the command, so one click can deliver the outcome twice. Auto-dismiss must be
// idempotent or the second delivery would replace the Undo toast.
var autoDismissing = {};
var pointerDown = null;
var dragged = false;

// ── "New completed" tracking + custom icons ──────────────────────
//
// `seenAtSecs` is the last moment the user looked at the panel. It is set at
// boot and refreshed on every render while the panel is open — never while the
// pet is collapsed/edge-hidden/tray-hidden. Completed tasks newer than it are
// surfaced on the collapsed pet and as a header chip.
var seenAtSecs = Math.floor(Date.now() / 1000);
// Count captured when the panel opens, shown in the header while it is open
// (the pet badge itself resets to 0 at that moment because the user now sees
// the list).
var pendingHeaderNew = 0;
var customIcons = { collapsed: null, hidden: null };

var STATUS_GLYPH = {
  running: '●', waiting_input: '!', waiting_approval: '!',
  completed: '✓', failed: '✕', idle: '○', unknown: '?'
};

// Reason keys that mean the jump target itself is permanently gone. When
// `auto_dismiss_invalid` is on, these outcomes remove the row and offer Undo.
var INVALID_TARGET_REASONS = [
  'target_invalid', 'task_gone', 'workspace_missing', 'rollout_missing',
  'no_target', 'app_not_found'
];


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

function settingsCopy() {
  return Object.assign({}, settings);
}

// ── New-completion indicator ────────────────────────────────────

function nowSecs() {
  return Math.floor(Date.now() / 1000);
}

function isPanelOpen() {
  return petState.mode === 'expanded' || petState.mode === 'dragging';
}

function countNewCompleted() {
  var tasks = snapshot.tasks || [];
  var count = 0;
  for (var i = 0; i < tasks.length; i++) {
    if (tasks[i].status === 'completed' && (tasks[i].last_activity_secs || 0) > seenAtSecs) {
      count++;
    }
  }
  return count;
}

function updatePetAria() {
  var tasks = snapshot.tasks || [];
  var attention = snapshot.attention_count || 0;
  var label = tasks.length
    ? t('collapsedAria').replace('{n}', tasks.length).replace('{a}', attention)
    : t('emptyAria');
  var separator = lang === 'zh' ? '，' : ', ';
  // A blocking question is the strongest signal the collapsed pet can carry,
  // so it is announced before the softer "new completions" count.
  var questions = effectiveQuestionCount();
  var questionText = questions > 0
    ? t('questionAria').replace('{n}', questions)
    : '';
  if (questionText) label += separator + questionText;
  var newCount = countNewCompleted();
  if (newCount > 0 && settings.new_badge !== false) {
    label += separator + t('newAria').replace('{n}', newCount);
  }
  // A pending notice is only announced while the pet is the visible surface.
  var noticeText = notice && !isPanelOpen() ? noticeMessage() : '';
  if (noticeText) label += separator + t('notice') + noticeText;
  petEl.setAttribute('aria-label', label);
  var titleParts = [];
  if (questionText) titleParts.push(questionText);
  if (noticeText) titleParts.push(noticeText);
  if (titleParts.length) petEl.title = titleParts.join(separator);
  else petEl.removeAttribute('title');
}

function updateNewIndicators() {
  var open = isPanelOpen();
  // While the panel is open the user is looking: advance the baseline. The
  // counts painted below therefore reflect what was waiting *before* opening.
  if (open) seenAtSecs = nowSecs();

  var enabled = settings.new_badge !== false;
  var count = open ? 0 : countNewCompleted();
  var showBadge = enabled && count > 0;
  newBadgeEl.hidden = !showBadge;
  newBadgeEl.textContent = count > 99 ? '99+' : String(count);
  newBadgeEl.classList.toggle('dot',
    petState.mode === 'edge_hidden' || petState.mode === 'tray_hidden');

  var showHeader = enabled && open && pendingHeaderNew > 0;
  headerNewEl.hidden = !showHeader;
  headerNewEl.textContent = showHeader
    ? t('newBadge').replace('{n}', pendingHeaderNew)
    : '';

  updateQuestionSurfaces();
}

// ── Pending-question attention ──────────────────────────────────
//
// Rust counts the tasks that block on a question (`waiting_input` /
// `waiting_approval`), exposes the total as `question_count` on every snapshot
// and emits `attention-pending` when NEW questions appear. The page only paints
// that state:
//
//   * collapsed/hidden: `#badge` gets the amber `.question` variant,
//   * a one-shot `.alerting` pulse for ~4 s (one timer, only while active),
//   * expanded header: the amber `#header-question` chip,
//   * the collapsed pet's aria-label/title announce the count.
//
// macOS notification, sound and auto-expansion belong to Rust: the page never
// calls focus() and never expands itself on this event.

var attentionState = {
  holdMs: 4000,   // how long the one-shot pulse stays on screen
  timer: null,    // at most one timer, and only while alerting
  active: false,  // an attention-pending event is being surfaced
  alerts: [],     // pending alert list from the last event
  eventCount: 0   // question_count carried by the event (snapshot may lag)
};
// Last question_count painted; a rise pulses even without an event.
var questionBaseline = null;

function countWaitingQuestions(tasks) {
  var list = tasks || [];
  var count = 0;
  for (var i = 0; i < list.length; i++) {
    if (list[i].status === 'waiting_input' || list[i].status === 'waiting_approval') count++;
  }
  return count;
}

function snapshotQuestionCount(data) {
  if (!data) return 0;
  if (typeof data.question_count === 'number' && isFinite(data.question_count)) {
    return Math.max(0, Math.floor(data.question_count));
  }
  // Older snapshots (and hand-written fixtures) may omit the field; deriving it
  // from the waiting statuses keeps the surfaces honest either way.
  return countWaitingQuestions(data.tasks);
}

function effectiveQuestionCount() {
  var base = snapshotQuestionCount(snapshot);
  // The event is fresher than the snapshot for the moment it arrives, so it may
  // raise the painted count; the snapshot stays authoritative afterwards.
  if (attentionState.active && attentionState.eventCount > base) {
    return attentionState.eventCount;
  }
  return base;
}

function animationsOff() {
  var mode = settings.animations || 'system';
  if (mode === 'off') return true;
  if (mode === 'always') return false;
  try {
    return !!(window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches);
  } catch (e) {
    return false;
  }
}

function updateQuestionSurfaces() {
  var count = effectiveQuestionCount();
  var visible = count > 0;
  badgeEl.classList.toggle('question', visible);
  headerQuestionEl.hidden = !visible;
  if (visible) {
    var aria = t('questionChipAria').replace('{n}', count);
    headerQuestionEl.textContent = t('questionChip').replace('{n}', count);
    headerQuestionEl.title = aria;
    headerQuestionEl.setAttribute('aria-label', aria);
  } else {
    headerQuestionEl.textContent = '';
    headerQuestionEl.removeAttribute('title');
    headerQuestionEl.removeAttribute('aria-label');
  }
  updatePetAria();
}

function clearAttentionState() {
  if (attentionState.timer) {
    clearTimeout(attentionState.timer);
    attentionState.timer = null;
  }
  attentionState.active = false;
  attentionState.alerts = [];
  attentionState.eventCount = 0;
  petEl.classList.remove('alerting');
  badgeEl.classList.remove('alerting');
  updateQuestionSurfaces();
}

// Exactly one timer exists while alerting; a second event restarts it instead
// of stacking pulses. `animations=off` / Reduce Motion keeps the amber state but
// never adds the animated class.
function startAttentionPulse() {
  if (attentionState.timer) clearTimeout(attentionState.timer);
  attentionState.timer = setTimeout(clearAttentionState, attentionState.holdMs);
  if (animationsOff()) {
    petEl.classList.remove('alerting');
    badgeEl.classList.remove('alerting');
    return;
  }
  petEl.classList.add('alerting');
  badgeEl.classList.add('alerting');
}

function handleAttentionPending(payload) {
  payload = payload || {};
  var alerts = Array.isArray(payload.alerts) ? payload.alerts : [];
  var count = typeof payload.question_count === 'number' && isFinite(payload.question_count)
    ? Math.max(0, Math.floor(payload.question_count))
    : alerts.length;
  attentionState.active = true;
  attentionState.alerts = alerts.slice(0, 8);
  attentionState.eventCount = count;
  // Paint the amber variant right away; the confirming snapshot may still be in
  // flight. Deliberately no focus()/set_expanded(): Rust owns auto_show.
  updateQuestionSurfaces();
  startAttentionPulse();
}

// ── Custom icon slots ───────────────────────────────────────────
//
// Rust stores the uploaded images and hands the webview data URLs. `custom`
// style falls back to the built-in glyph whenever a slot is missing, so a
// half-configured setup never renders an empty 72 px square.
function applyCustomIcon() {
  var mode = petState.mode || 'collapsed';
  var preferHidden = mode === 'edge_hidden' || mode === 'tray_hidden';
  var url = preferHidden
    ? (customIcons.hidden || customIcons.collapsed)
    : customIcons.collapsed;
  var active = settings.icon_style === 'custom' && !!url;

  if (active) petIconEl.setAttribute('src', url);
  petIconEl.hidden = !active;
  glyphEl.hidden = active;
  petEl.classList.toggle('custom-icon', active);
}

function applyIcons(payload) {
  if (!payload) return;
  customIcons = {
    collapsed: payload.collapsed || null,
    hidden: payload.hidden || null
  };
  applyCustomIcon();
}

function initCustomIcons() {
  invoke('get_custom_icons').then(applyIcons).catch(function () {});
  listen('icons-changed', function (event) { applyIcons(event.payload); });
}

// ── Settings application ────────────────────────────────────────

function applySettings(next) {
  if (!next) return;
  var previous = settings;
  settings = Object.assign({}, settings, next);

  lang = resolveLanguage(settings.language);
  appEl.setAttribute('data-lang', lang);
  document.documentElement.lang = lang === 'zh' ? 'zh-CN' : 'en';

  var appearance = settings.appearance || 'system';
  if (appearance === 'light' || appearance === 'dark') {
    document.documentElement.style.colorScheme = appearance;
  } else {
    document.documentElement.style.removeProperty('color-scheme');
  }

  document.documentElement.setAttribute('data-animations', settings.animations || 'system');

  // Collapsed/snapped pet size (logical px). Rust resizes the real window with
  // the same value; this variable keeps the CSS shape identical in the mock and
  // the packaged app.
  var collapsedSize = Math.max(48, Math.min(96, Number(settings.collapsed_size) || 72));
  document.documentElement.style.setProperty('--collapsed-size', collapsedSize + 'px');

  petState.max_visible_rows = settings.max_visible_rows;
  petState.hover_expand_ms = settings.hover_expand_ms;
  petState.collapse_delay_ms = settings.collapse_delay_ms;

  applyRows();
  // The footer count depends on max_visible_rows as well as the data, so a
  // settings change repaints it even when the render hash below is unchanged.
  updateFooterCount();

  var needsRender = previous.language !== settings.language ||
    previous.show_summary !== settings.show_summary ||
    previous.show_jump_level !== settings.show_jump_level ||
    previous.max_visible_rows !== settings.max_visible_rows;
  if (needsRender) {
    lastRenderedHash = '';
    render(snapshot);
  }

  applyStaticStrings();
  applyCustomIcon();
  updateNewIndicators();
  // Turning animations off while a pulse is running drops the class immediately;
  // the state timer still clears itself normally.
  if (animationsOff()) {
    petEl.classList.remove('alerting');
    badgeEl.classList.remove('alerting');
  }
  syncNotice();
  refreshAutoHide();
}

// `settings.max_visible_rows` drives the CSS max-height; the window itself is
// resized by Rust to exactly 44 + rows × 64 + 26 logical px. In the mock the
// page emulates that window height so layout/screenshots match the real app.
// The 1..8 clamp is shared with Rust and with the footer's visible count.
function rowCap() {
  return Math.max(1, Math.min(8, Number(settings.max_visible_rows) || 5));
}

function applyRows() {
  var rows = rowCap();
  listEl.style.maxHeight = (rows * 64) + 'px';
  syncMockGeometry();
}

function panelHeightFor(rows) {
  return 44 + rows * 64 + 26;
}

function syncMockGeometry() {
  if (!MOCK) return;
  var mode = petState.mode || 'collapsed';
  if (mode === 'edge_hidden') {
    // Rust resizes the real window to hidden_sliver × hidden_sliver.
    var sliver = Math.max(8, Math.min(24, Number(settings.hidden_sliver) || 14));
    appEl.style.width = sliver + 'px';
    appEl.style.height = sliver + 'px';
    return;
  }
  appEl.style.removeProperty('width');
  var expanded = mode === 'expanded' || mode === 'dragging';
  if (!expanded) { appEl.style.removeProperty('height'); return; }
  var rows = rowCap();
  // Mirror Rust's `visible_rows`/`expanded_height_logical`: never taller than
  // the row cap, never taller than the data, always at least one row.
  var tasks = (snapshot.tasks || []).length;
  var visible = Math.max(1, Math.min(rows, tasks));
  appEl.style.height = panelHeightFor(visible) + 'px';
}

function applyStaticStrings() {
  panelEl.setAttribute('aria-label', t('sessionsTitle'));
  collapseBtn.setAttribute('aria-label', t('collapse'));
  collapseBtn.title = t('collapse');
  settingsBtn.setAttribute('aria-label', t('settings'));
  settingsBtn.title = t('settings');
}

function openSettings() {
  invoke('open_settings').catch(function () {});
}

// ── Rendering ───────────────────────────────────────────────────

// Footer count: how many sessions the current window can actually show, plus
// the total when the list is scrolled. `visible` follows the same rule as the
// expanded window height (min(tasks, clamp(max_visible_rows, 1..8))), except
// that an empty snapshot stays 0 instead of the window's one-row minimum.
function updateFooterCount() {
  var total = (snapshot.tasks || []).length;
  var visible = Math.min(total, rowCap());
  var truncated = total > visible;
  footerCountEl.textContent = truncated
    ? visible + ' / ' + total + ' ' + t('sessions')
    : total + ' ' + t('sessions');
  var title = t('visibleCountTitle').replace('{v}', visible).replace('{t}', total);
  footerCountEl.title = title;
  footerCountEl.setAttribute('aria-label', title);
}

function render(snapshotData) {
  if (!snapshotData) return;
  var hash = JSON.stringify(snapshotData.tasks) + '|' + snapshotData.attention_count +
    '|' + snapshotData.question_count;
  snapshot = snapshotData;
  if (hash === lastRenderedHash) return;
  lastRenderedHash = hash;

  var tasks = snapshot.tasks || [];
  var attention = snapshot.attention_count || 0;
  var hasTasks = tasks.length > 0;

  appEl.classList.toggle('no-tasks', !hasTasks);

  badgeEl.hidden = attention === 0;
  badgeEl.textContent = attention > 99 ? '99+' : String(attention);

  // A rising waiting-question count pulses exactly like an event. The first
  // render only seeds the baseline (existing questions at boot must not pulse).
  var questions = snapshotQuestionCount(snapshot);
  if (questionBaseline !== null && questions > questionBaseline) startAttentionPulse();
  questionBaseline = questions;

  updatePetAria();

  panelTitleEl.textContent = t('sessionsTitle');
  headerMetaEl.hidden = attention === 0;
  headerMetaEl.textContent = attention > 0
    ? attention + ' ' + t('needAttention')
    : '';

  updateFooterCount();
  footerHintEl.textContent = FOOTER_HINT;
  applyStaticStrings();
  updateNewIndicators();

  if (petState.mode === 'expanded' || petState.mode === 'dragging') renderList(tasks);
  syncMockGeometry();
  if (MOCK && window.__AM_TEST__) window.__AM_TEST__.lastHash = hash;
}

function renderList(tasks) {
  var previousScroll = listEl.scrollTop;
  listEl.textContent = '';

  if (!tasks.length) {
    var empty = el('div', 'empty');
    empty.appendChild(el('div', 'empty-title', t('empty')));
    empty.appendChild(el('div', 'empty-hint', t('emptyHint')));
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

  var showLevel = settings.show_jump_level !== false;
  var showSummary = settings.show_summary !== false;
  var statusLabel = t('status.' + task.status) || task.status;
  var levelText = showLevel ? (task.level || 'L0') : '';
  row.setAttribute('aria-label',
    task.agent_name + ', ' + task.title + ', ' + statusLabel +
    (levelText ? ', ' + levelText : '') + ', ' + t('jumpHint'));
  row.title = task.title + '\n' + statusLabel + ' · ' +
    (task.status_reason || '') + '\n' + t('jumpHint') +
    (levelText ? ' (' + levelText + ')' : '');

  var top = el('div', 'task-top');
  var title = el('span', 'title', task.title);
  title.title = task.title;
  top.appendChild(title);

  var status = el('div', 'status');
  status.appendChild(el('span', 'agent', task.agent_name));
  if (task.origin === 'synthetic') status.appendChild(el('span', 'sim', 'SIM'));
  status.appendChild(el('span', 'status-glyph', STATUS_GLYPH[task.status] || '?'));
  status.appendChild(el('span', 'status-text', statusLabel));
  if (showSummary && task.summary) status.appendChild(el('span', 'status-summary', task.summary));

  var meta = el('div', 'meta');
  var timeEl = el('span', 'time', relativeTime(task.last_activity_secs));
  timeEl.dataset.ts = String(task.last_activity_secs || 0);
  meta.appendChild(timeEl);
  if (showLevel) meta.appendChild(el('span', levelClass(task.level), task.level || 'L0'));

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
    showNotice(String(err && err.message ? err.message : err), 'error', null);
  });
}

function handleOutcome(outcome) {
  if (!outcome) return;
  // Success needs no toast; degradation/failure may (settings permitting).
  if (outcome.kind === 'success') return;

  var reason = outcome.reason_key || 'target_invalid';
  var key = 'toast.' + reason;
  var message = t(key);
  if (!message || message === key) message = reason;
  var kind = outcome.kind === 'failed' ? 'error' : 'warn';
  // `toast_on_degraded=false` may silence only the softer notices; failures
  // are always reported.
  var suppressed = settings.toast_on_degraded === false &&
    (outcome.kind === 'degraded' || outcome.kind === 'unconfirmed');

  if (settings.auto_dismiss_invalid &&
      (outcome.kind === 'failed' || outcome.kind === 'degraded') &&
      INVALID_TARGET_REASONS.indexOf(reason) !== -1 && outcome.task_id) {
    var taskId = outcome.task_id;
    if (autoDismissing[taskId]) return; // duplicate delivery of the same outcome
    autoDismissing[taskId] = true;
    invoke('dismiss_task', { taskId: taskId }).then(function (removed) {
      if (!removed) {
        delete autoDismissing[taskId];
        if (!suppressed) showNotice(message, kind, outcome.permission_needed, null, key);
        return;
      }
      // Auto-dismiss always keeps an Undo affordance, even for degraded
      // outcomes that would otherwise be silent.
      showNotice(message, kind, null, { type: 'undo', taskId: taskId }, key, 'dismissed');
    }).catch(function () {
      delete autoDismissing[taskId];
      if (!suppressed) showNotice(message, kind, outcome.permission_needed, null, key);
    });
    return;
  }

  if (suppressed) return;
  showNotice(message, kind, outcome.permission_needed, null, key);
}

function noticeMessage() {
  if (!notice) return '';
  if (!notice.messageKey) return notice.message || '';
  var message = t(notice.messageKey);
  if (message === notice.messageKey) message = notice.message || '';
  if (notice.suffixKey) message = message + ' ' + t(notice.suffixKey);
  return message;
}

function paintNoticeToast() {
  if (!notice) return;
  toastTextEl.textContent = noticeMessage();
  toastEl.className = '';
  if (notice.kind === 'error') toastEl.classList.add('error');
  else if (notice.kind === 'warn') toastEl.classList.add('warn');
  toastEl.hidden = false;

  var permissionNeeded = notice.permissionNeeded;
  var action = notice.action;
  if (permissionNeeded) {
    toastActionEl.hidden = false;
    toastActionEl.textContent = t('toast.grant');
    toastActionEl.onclick = function (event) {
      event.stopPropagation();
      invoke('open_permission_settings', { kind: permissionNeeded }).catch(function () {});
      dismissNotice();
    };
  } else if (action && action.type === 'undo') {
    toastActionEl.hidden = false;
    toastActionEl.textContent = t('undo');
    toastActionEl.onclick = function (event) {
      event.stopPropagation();
      undoDismiss(action.taskId);
    };
  } else {
    toastActionEl.hidden = true;
    toastActionEl.onclick = null;
  }
}

function paintNoticeDot() {
  var kind = (notice && notice.kind) || 'info';
  if (kind !== 'warn' && kind !== 'error') kind = 'info';
  noticeDotEl.className = 'notice-dot ' + kind;
  noticeDotEl.hidden = !notice;
}

// One timer, only while a notice exists. The lifetime depends on the surface:
// the toast expires like before (~4 s); the collapsed dot stays much longer so
// the user can still expand and act on it.
function scheduleNoticeExpiry() {
  if (noticeTimer) { clearTimeout(noticeTimer); noticeTimer = null; }
  if (!notice) return;
  var lifetime = isPanelOpen() ? noticeExpiry.expanded : noticeExpiry.collapsed;
  noticeExpiresAt = Date.now() + lifetime;
  noticeTimer = setTimeout(dismissNotice, lifetime);
}

function syncNotice() {
  if (!notice) {
    noticeSurface = null;
    toastEl.hidden = true;
    noticeDotEl.hidden = true;
    updatePetAria();
    return;
  }
  var surface = isPanelOpen() ? 'toast' : 'dot';
  if (surface === 'toast') {
    noticeDotEl.hidden = true;
    paintNoticeToast();
  } else {
    // Never squeeze the toast into the collapsed/edge-hidden/tray-hidden box:
    // the text-free dot is the whole surface there.
    toastEl.hidden = true;
    paintNoticeDot();
  }
  if (surface !== noticeSurface) {
    noticeSurface = surface;
    scheduleNoticeExpiry();
  }
  updatePetAria();
}

function showNotice(message, kind, permissionNeeded, action, messageKey, suffixKey) {
  if (!message && !messageKey) return;
  notice = {
    message: message || '',
    kind: kind || 'info',
    permissionNeeded: permissionNeeded || null,
    action: action || null,
    messageKey: messageKey || null,
    suffixKey: suffixKey || null
  };
  noticeSurface = null; // a fresh notice always gets a fresh lifetime
  syncNotice();
}

function dismissNotice() {
  notice = null;
  noticeSurface = null;
  noticeExpiresAt = 0;
  if (noticeTimer) { clearTimeout(noticeTimer); noticeTimer = null; }
  toastEl.hidden = true;
  noticeDotEl.hidden = true;
  updatePetAria();
}

function undoDismiss(taskId) {
  delete autoDismissing[taskId];
  invoke('restore_dismissed_task', { taskId: taskId }).catch(function () {});
  dismissNotice();
}

// ── Pet state machine (visual side) ─────────────────────────────

function applyPetState(next) {
  if (!next) return;
  var wasOpen = isPanelOpen();
  var nextMode = next.mode || petState.mode || 'collapsed';
  var willOpen = nextMode === 'expanded' || nextMode === 'dragging';
  if (willOpen && !wasOpen) {
    // Remember what accumulated before the baseline is advanced below, so the
    // header can announce "N new" while the user reads the list.
    pendingHeaderNew = settings.new_badge !== false ? countNewCompleted() : 0;
  } else if (!willOpen) {
    pendingHeaderNew = 0;
  }

  petState = Object.assign({}, petState, next);
  // PetStateView carries the full Settings object; apply it synchronously so a
  // mode change never paints with stale typography/rows/flags.
  if (next.settings) applySettings(next.settings);

  var mode = petState.mode || 'collapsed';
  appEl.className = appEl.className
    .replace(/\bmode-[a-z_]+\b/g, '')
    .trim();
  appEl.classList.add('mode-' + mode);
  if (!snapshot.tasks || snapshot.tasks.length === 0) appEl.classList.add('no-tasks');
  appEl.dataset.edge = petState.edge || 'none';
  appEl.dataset.hidden = petState.hidden ? 'true' : 'false';
  appEl.dataset.hiddenToTray = petState.hidden_to_tray ? 'true' : 'false';

  var expanded = mode === 'expanded' || mode === 'dragging';
  panelEl.hidden = !expanded;
  syncMockGeometry();
  applyCustomIcon();
  updateNewIndicators();
  // Collapsing must move an active notice from the toast to the pet dot (and
  // expanding must bring the full toast back), so it is surfaced on every mode.
  syncNotice();
  refreshAutoHide();
  if (expanded) renderList(snapshot.tasks || []);
}

function setExpanded(expanded, focus) {
  invoke('set_expanded', { expanded: !!expanded, focus: !!focus }).catch(function () {});
}

function requestCollapse(force) {
  invoke('request_collapse', { force: !!force }).catch(function () {});
}

// ── Hover behaviour ─────────────────────────────────────────────

// ── Hide after snapping ─────────────────────────────────────────
//
// One-shot timer: while the pet rests collapsed on a screen edge and nobody
// hovers/drags it, hide it (edge sliver or tray, following `edge_hide_enabled`).
// `auto_hide_after_s = 0` disables it, and a free-floating pet (edge "none")
// never hides itself. This is the only additional timer in the page; it exists
// only while armed.

var autoHideTimer = null;

function cancelAutoHide() {
  if (autoHideTimer) { clearTimeout(autoHideTimer); autoHideTimer = null; }
}

function refreshAutoHide() {
  cancelAutoHide();
  var seconds = Number(settings.auto_hide_after_s) || 0;
  if (seconds <= 0) return;
  if (petState.mode !== 'collapsed') return;
  if (!petState.edge || petState.edge === 'none') return;
  autoHideTimer = setTimeout(function () {
    autoHideTimer = null;
    if (petState.mode !== 'collapsed') return;
    if (!petState.edge || petState.edge === 'none') return;
    invoke('hide_pet').catch(function () {});
  }, seconds * 1000);
}

function enterPet() {
  cancelAutoHide();
  if (collapseTimer) { clearTimeout(collapseTimer); collapseTimer = null; }
  var mode = petState.mode;
  if (mode === 'collapsed' || mode === 'edge_hidden') {
    if (hoverTimer) return;
    hoverTimer = setTimeout(function () {
      hoverTimer = null;
      suppressLeaveUntil = Date.now() + 500;
      // Hover never focuses the pet: this must stay non-intrusive.
      setExpanded(true, false);
    }, petState.hover_expand_ms || 150);
  }
}

function leavePet() {
  if (hoverTimer) { clearTimeout(hoverTimer); hoverTimer = null; }
  if (petState.mode !== 'expanded') return;
  // Never drop the collapse: if the pointer leaves inside the post-expand
  // suppression window, collapse as soon as that window plus the user delay has
  // elapsed. (Previously an early leave could leave the panel open forever.)
  var delay = petState.collapse_delay_ms || 800;
  var suppressedFor = suppressLeaveUntil - Date.now();
  if (suppressedFor > 0) delay += suppressedFor;
  if (collapseTimer) clearTimeout(collapseTimer);
  collapseTimer = setTimeout(function () {
    collapseTimer = null;
    requestCollapse(false);
  }, delay);
}

appEl.addEventListener('mouseenter', enterPet);
appEl.addEventListener('mouseleave', leavePet);

// Clicking anywhere outside makes the pet window resign key focus. Collapse
// immediately (`force`) so an outside click always dismisses the popover, even
// inside the post-resize suppression window. Hover-expanded panels never focus,
// so for those the mouse-leave path above handles it.
window.addEventListener('blur', function () {
  if (petState.mode === 'expanded') {
    cancelAutoHide();
    requestCollapse(true);
  }
});

// ── Pointer / drag handling ─────────────────────────────────────

function onPointerDown(event, source) {
  if (event.button !== 0) return;
  cancelAutoHide();
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
  // Explicit click focuses the popover (like a menu-bar panel) so clicking
  // anywhere else produces a blur that collapses it immediately.
  setExpanded(true, true);
});
petEl.addEventListener('keydown', function (event) {
  if (event.key === 'Enter' || event.key === ' ') {
    event.preventDefault();
    setExpanded(true, true);
  }
});

dragbarEl.addEventListener('mousedown', function (event) {
  if (event.button !== 0 || event.target.closest('button')) return;
  onPointerDown(event, 'handle');
  startWindowDrag();
});
document.addEventListener('mousemove', onPointerMove);
document.addEventListener('mouseup', onPointerUp);

collapseBtn.addEventListener('click', function (event) {
  event.stopPropagation();
  setExpanded(false);
});

settingsBtn.addEventListener('click', function (event) {
  event.stopPropagation();
  openSettings();
});

document.addEventListener('keydown', function (event) {
  // Cmd+, (macOS Preferences) is the standard settings shortcut.
  if (event.metaKey && event.key === ',') {
    event.preventDefault();
    openSettings();
    return;
  }
  if (event.key === 'Escape') {
    dismissNotice();
    if (petState.mode === 'expanded') requestCollapse(true);
  }
});

listEl.addEventListener('scroll', function () {
  // Scrolling must never be interpreted as a collapse trigger.
  if (collapseTimer) { clearTimeout(collapseTimer); collapseTimer = null; }
}, { passive: true });

// ── Mock fixture (browser layout work only) ─────────────────────

function mockSettings() {
  var query = location.search;
  var next = defaultSettings();
  var match = query.match(/[?&]lang=(zh|en)\b/);
  if (match) next.language = match[1];
  match = query.match(/[?&]rows=(\d+)/);
  if (match) next.max_visible_rows = Math.max(1, Math.min(8, parseInt(match[1], 10)));
  match = query.match(/[?&]appearance=(light|dark|system)\b/);
  if (match) next.appearance = match[1];
  match = query.match(/[?&]animations=(system|always|off)\b/);
  if (match) next.animations = match[1];
  if (/[?&]show_summary=0\b/.test(query)) next.show_summary = false;
  if (/[?&]show_jump_level=0\b/.test(query)) next.show_jump_level = false;
  if (/[?&]toast_on_degraded=0\b/.test(query)) next.toast_on_degraded = false;
  if (/[?&]auto_dismiss_invalid=1\b/.test(query)) next.auto_dismiss_invalid = true;
  match = query.match(/[?&]collapsed_size=(\d+)/);
  if (match) next.collapsed_size = Math.max(48, Math.min(96, parseInt(match[1], 10)));
  match = query.match(/[?&]auto_hide=(\d+)/);
  if (match) next.auto_hide_after_s = Math.max(0, Math.min(60, parseInt(match[1], 10)));
  match = query.match(/[?&]hidden_sliver=(\d+)/);
  if (match) next.hidden_sliver = Math.max(8, Math.min(24, parseInt(match[1], 10)));
  match = query.match(/[?&]icon_style=(builtin|custom)\b/);
  if (match) next.icon_style = match[1];
  if (/[?&]new_badge=0\b/.test(query)) next.new_badge = false;
  return next;
}

function mockSnapshot() {
  var now = Math.floor(Date.now() / 1000);
  var tasks = [
      { id: 'codex:1', agent_key: 'codex', agent_name: 'Codex', host: 'desktop', title: 'Refactor the pet geometry and snap rules', status: 'running', status_reason: 'task_started', last_activity_secs: now - 20, collected_at_secs: now, summary: 'Editing pet.rs', origin: 'index', target: {}, target_validity: 'valid', invalid_reason: null, capability: { max_level: 'L3', notes: '', requires: [] }, level: 'L3', pinned: false },
      { id: 'dsh:2', agent_key: 'dsh', agent_name: 'DSH', host: 'desktop', title: 'Prove the finite-sample bridge conjecture', status: 'waiting_approval', status_reason: 'approval/policy', last_activity_secs: now - 90, collected_at_secs: now, summary: null, origin: 'cache', target: {}, target_validity: 'valid', invalid_reason: null, capability: { max_level: 'L3', notes: '', requires: [] }, level: 'L3', pinned: false },
      { id: 'zcode:3', agent_key: 'zcode', agent_name: 'ZCode', host: 'desktop', title: '美化论文 Figure 1 与 ML 写作规范', status: 'completed', status_reason: 'task_status = completed', last_activity_secs: now - 3600, collected_at_secs: now, summary: 'GLM-5.3-Flash', origin: 'index', target: {}, target_validity: 'valid', invalid_reason: null, capability: { max_level: 'L2', notes: '', requires: [] }, level: 'L2', pinned: false },
      { id: 'claude-code:4', agent_key: 'claude-code', agent_name: 'Claude Code', host: 'vscode', title: 'Fix the failing LaTeX build in the appendix', status: 'waiting_input', status_reason: 'ask user question', last_activity_secs: now - 240, collected_at_secs: now, summary: 'OPUS, 12 messages', origin: 'event', target: {}, target_validity: 'valid', invalid_reason: null, capability: { max_level: 'L2', notes: '', requires: [] }, level: 'L2', pinned: false },
      { id: 'dsh:5', agent_key: 'dsh', agent_name: 'DSH', host: 'desktop', title: 'Training-free DRE literature positioning audit with a very long title that must be ellipsized', status: 'failed', status_reason: 'turn aborted', last_activity_secs: now - 7200, collected_at_secs: now, summary: 'turn 3', origin: 'cache', target: {}, target_validity: 'invalid', invalid_reason: 'workspace_missing', capability: { max_level: 'L3', notes: '', requires: [] }, level: 'L0', pinned: false },
      { id: 'sim:6', agent_key: 'simulated', agent_name: 'Simulated Agent', host: 'fixture', title: 'Extension acceptance fixture', status: 'running', status_reason: 'fixture', last_activity_secs: now - 5, collected_at_secs: now, summary: 'fixture', origin: 'synthetic', target: {}, target_validity: 'unconfirmed', invalid_reason: null, capability: { max_level: 'L1', notes: '', requires: [] }, level: 'L1', pinned: false }
  ];
  // Design-preview helper: `?mock=collapsed&new=1` paints the new-completion
  // badge that only appears after a completion while the panel was closed.
  if (/[?&]new=1\b/.test(location.search)) {
    tasks.unshift({
      id: 'zcode:new', agent_key: 'zcode', agent_name: 'ZCode', host: 'desktop',
      title: 'Finish the icon framework and badge polish', status: 'completed',
      status_reason: 'task_status = completed', last_activity_secs: now + 1,
      collected_at_secs: now, summary: 'Finished', origin: 'index', target: {},
      target_validity: 'valid', invalid_reason: null,
      capability: { max_level: 'L2', notes: '', requires: [] }, level: 'L2', pinned: false
    });
  }
  return {
    collected_at_secs: now,
    attention_count: 2,
    // The default fixture has two blocking questions (dsh:2 waiting_approval,
    // claude-code:4 waiting_input); keep the count derived so it cannot drift.
    question_count: countWaitingQuestions(tasks),
    agents: [
      { key: 'codex', name: 'Codex' }, { key: 'dsh', name: 'DSH' },
      { key: 'zcode', name: 'ZCode' }, { key: 'claude-code', name: 'Claude Code' }
    ],
    tasks: tasks
  };
}

var mockDismissed = {};
var openSettingsCalls = 0;
var restoreCalls = [];
var hidePetCalls = 0;
var expandCalls = [];
var collapseCalls = 0;

// Mock icon slots: tiny valid PNG data URLs exercise the exact payload shape
// (`{collapsed, hidden}`) without shipping image assets.
var MOCK_ICON_A = 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==';
var MOCK_ICON_B = 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=';
var mockIcons = { collapsed: MOCK_ICON_A, hidden: null };

function mockTaskById(taskId) {
  var tasks = snapshot.tasks || [];
  for (var i = 0; i < tasks.length; i++) if (tasks[i].id === taskId) return tasks[i];
  return null;
}

// Default `attention-pending` payload for the browser preview/selftest: mirrors
// what Rust emits after a scan that found new questions.
function mockAttentionPayload() {
  var tasks = snapshot.tasks || [];
  var alerts = [];
  for (var i = 0; i < tasks.length; i++) {
    var task = tasks[i];
    if (task.status !== 'waiting_input' && task.status !== 'waiting_approval') continue;
    alerts.push({
      task_id: task.id,
      agent_key: task.agent_key,
      agent_name: task.agent_name,
      title: task.title,
      status: task.status,
      status_reason: task.status_reason
    });
  }
  return {
    alerts: alerts.slice(0, 8),
    question_count: alerts.length,
    auto_show: true
  };
}

function mockInvoke(cmd, args) {
  if (cmd === 'get_snapshot' || cmd === 'refresh') return mockSnapshot();
  if (cmd === 'get_settings') return settingsCopy();
  if (cmd === 'get_custom_icons') {
    return { collapsed: mockIcons.collapsed, hidden: mockIcons.hidden };
  }
  if (cmd === 'set_settings') {
    // Mirror Rust: normalise into a full Settings value, broadcast
    // `settings-changed`, and let the listener apply it (no local shortcut, so
    // the event path is exercised exactly like the packaged app).
    var next = Object.assign({}, settings, (args && args.settings) || {});
    if ((mockListeners['settings-changed'] || []).length) {
      mockEmit('settings-changed', next);
    } else {
      applySettings(next);
    }
    return next;
  }
  if (cmd === 'open_settings') {
    openSettingsCalls++;
    return true;
  }
  if (cmd === 'hide_pet') {
    hidePetCalls++;
    applyPetState({ mode: 'edge_hidden', hidden: true, hidden_to_tray: false });
    return true;
  }
  if (cmd === 'restore_dismissed_task') {
    var restoreId = args && args.taskId;
    restoreCalls.push(restoreId);
    var restored = mockDismissed[restoreId];
    if (restored) {
      delete mockDismissed[restoreId];
      snapshot.tasks = (snapshot.tasks || []).concat([restored]);
      snapshot.attention_count = snapshot.tasks.filter(function (task) {
        return task.status === 'waiting_input' || task.status === 'waiting_approval' || task.status === 'failed';
      }).length;
      snapshot.question_count = countWaitingQuestions(snapshot.tasks);
      lastRenderedHash = '';
      render(snapshot);
    }
    return true;
  }
  if (cmd === 'set_expanded') {
    if (args && args.expanded) expandCalls.push(!!args.focus);
    applyPetState({ mode: args && args.expanded ? 'expanded' : 'collapsed' });
    return true;
  }
  if (cmd === 'request_collapse') {
    collapseCalls++;
    if (petState.mode === 'expanded') applyPetState({ mode: 'collapsed' });
    return true;
  }
  if (cmd === 'dismiss_task') {
    var removed = mockTaskById(args && args.taskId);
    if (removed) mockDismissed[removed.id] = removed;
    snapshot.tasks = (snapshot.tasks || []).filter(function (task) { return task.id !== (args && args.taskId); });
    snapshot.attention_count = snapshot.tasks.filter(function (task) {
      return task.status === 'waiting_input' || task.status === 'waiting_approval' || task.status === 'failed';
    }).length;
    snapshot.question_count = countWaitingQuestions(snapshot.tasks);
    lastRenderedHash = '';
    render(snapshot);
    return !!removed;
  }
  if (cmd === 'get_pet_state') {
    return {
      mode: 'expanded', edge: 'right', hidden: false, hidden_to_tray: false,
      settings: settingsCopy()
    };
  }
  if (cmd === 'jump_to_task') {
    var taskId = args && args.taskId;
    var task = mockTaskById(taskId);
    var outcome = (task && task.target_validity === 'invalid')
      ? {
        task_id: taskId, kind: 'failed', requested: 'L3', achieved: 'L0',
        reason_key: task.invalid_reason || 'target_invalid', detail: null,
        next_step: null, permission_needed: null
      }
      : {
        task_id: taskId || 'mock', kind: 'degraded', requested: 'L3', achieved: 'L2',
        reason_key: 'opened_window', detail: null, next_step: null, permission_needed: null
      };
    // Rust both emits `jump-feedback` and returns the outcome; mirror that so
    // the UI's duplicate-delivery handling is exercised in the selftest.
    mockEmit('jump-feedback', outcome);
    return outcome;
  }
  return true;
}

// ── Mock-only test hooks (never active in the packaged app) ─────

if (MOCK) {
  window.__AM_TEST__ = {
    get snapshot() { return snapshot; },
    get petState() { return petState; },
    get settings() { return settingsCopy(); },
    get jumpCalls() { return jumpCalls.slice(); },
    get openSettingsCalls() { return openSettingsCalls; },
    get hidePetCalls() { return hidePetCalls; },
    get expandCalls() { return expandCalls.slice(); },
    get collapseCalls() { return collapseCalls; },
    get restoreCalls() { return restoreCalls.slice(); },
    get customIcons() { return { collapsed: customIcons.collapsed, hidden: customIcons.hidden }; },
    get seenAtSecs() { return seenAtSecs; },
    // Pending-question attention state (badge variant / pulse / header chip).
    get attention() {
      return {
        active: attentionState.active,
        holdMs: attentionState.holdMs,
        alerts: attentionState.alerts.slice(),
        eventCount: attentionState.eventCount,
        timerActive: !!attentionState.timer,
        pulsing: petEl.classList.contains('alerting'),
        questionCount: effectiveQuestionCount()
      };
    },
    // Shorten the ~4 s pulse hold so the selftest can watch it clear.
    setAttentionHold: function (ms) {
      var next = Number(ms);
      attentionState.holdMs = isFinite(next) && next > 0 ? next : 4000;
      return attentionState.holdMs;
    },
    // Drive the Tauri `attention-pending` event exactly like Rust would.
    emitAttention: function (payload) {
      var next = payload || mockAttentionPayload();
      mockEmit('attention-pending', next);
      return next;
    },
    emitIconsChanged: function (payload) {
      mockIcons = {
        collapsed: (payload && payload.collapsed) || null,
        hidden: (payload && payload.hidden) || null
      };
      mockEmit('icons-changed', { collapsed: mockIcons.collapsed, hidden: mockIcons.hidden });
    },
    setSettings: function (next) { return invoke('set_settings', { settings: next }); },
    openSettings: openSettings,
    render: render,
    applyPetState: applyPetState,
    // `hideToast` kept as the legacy hook name; both clear the notice state.
    hideToast: dismissNotice,
    clearNotice: dismissNotice,
    setNoticeExpiry: function (opts) {
      if (!opts) return;
      if (typeof opts.expanded === 'number') noticeExpiry.expanded = opts.expanded;
      if (typeof opts.collapsed === 'number') noticeExpiry.collapsed = opts.collapsed;
    },
    get notice() {
      if (!notice) return null;
      return {
        kind: notice.kind,
        message: notice.message,
        messageKey: notice.messageKey,
        action: notice.action,
        surface: noticeSurface,
        expiresAt: noticeExpiresAt
      };
    },
    setMockSnapshot: function (next) { lastRenderedHash = ''; render(next); },
    statusGlyphs: Object.keys(STATUS_GLYPH),
    languages: Object.keys(I18N),
    t: t
  };
}

// ── Bootstrap ───────────────────────────────────────────────────

// One-shot v0.3.x migration: the old frontend kept the language and poll
// interval in localStorage. Rust owns the new settings file, so hand the legacy
// values over once and remove them. If nothing is stored this is a no-op.
function migrateLegacySettings(done) {
  var legacyLang = null;
  var legacyPoll = null;
  var hasLegacy = false;
  try {
    var langValue = localStorage.getItem('am-lang');
    if (langValue === 'zh' || langValue === 'en') {
      legacyLang = langValue;
      hasLegacy = true;
    }
    var raw = localStorage.getItem('am-s');
    if (raw) {
      var parsed = JSON.parse(raw);
      if (parsed && typeof parsed.pollMs === 'number') {
        legacyPoll = parsed.pollMs;
        hasLegacy = true;
      }
    }
  } catch (e) { /* storage unavailable: nothing to migrate */ }
  if (!hasLegacy) { done(); return; }
  invoke('import_legacy_settings', { lang: legacyLang, pollMs: legacyPoll }).then(function (next) {
    if (next) applySettings(next);
    try {
      localStorage.removeItem('am-lang');
      localStorage.removeItem('am-s');
    } catch (e) { /* ignore */ }
  }).catch(function () {
    // Already migrated (or the write failed): never block boot on this.
  }).then(done);
}

function boot() {
  // Everything already completed at startup is considered seen.
  seenAtSecs = nowSecs();
  appEl.setAttribute('data-lang', lang);
  document.documentElement.lang = lang === 'zh' ? 'zh-CN' : 'en';
  document.documentElement.setAttribute('data-animations', settings.animations || 'system');

  if (MOCK) {
    // Design preview only: paint a desktop-like backdrop behind the translucent
    // material so screenshots show the real vibrancy composition.
    if (location.search.indexOf('desktop=1') !== -1) {
      document.documentElement.classList.add('mock-desktop');
    }
  }
  document.addEventListener('contextmenu', function (event) { event.preventDefault(); });

  if (MOCK) {
    document.body.classList.add('mock');
    listen('tasks-updated', function (event) { render(event.payload); });
    listen('pet-state', function (event) { applyPetState(event.payload); });
    listen('jump-feedback', function (event) { handleOutcome(event.payload); });
    listen('settings-changed', function (event) { applySettings(event.payload); });
    listen('attention-pending', function (event) { handleAttentionPending(event.payload); });
    applySettings(mockSettings());
    render(mockSnapshot());
    var collapsed = location.search.indexOf('mock=collapsed') !== -1;
    var edgeHidden = location.search.indexOf('mock=edge_hidden') !== -1;
    var view = mockInvoke('get_pet_state');
    if (edgeHidden) {
      view = Object.assign({}, view, { mode: 'edge_hidden', edge: 'right', hidden: true });
    } else if (collapsed) {
      view = Object.assign({}, view, { mode: 'collapsed' });
    }
    applyPetState(view);
    initCustomIcons();
    // Design-preview helper: `?mock=collapsed&notice=warn|error|info` paints the
    // notice dot exactly as it appears after a jump that outlived the collapse.
    var noticeKind = (location.search.match(/[?&]notice=(warn|error|info)\b/) || [])[1];
    if (noticeKind) {
      var noticeKeys = {
        warn: 'toast.opened_window',
        error: 'toast.workspace_missing',
        info: 'toast.focus_unconfirmed'
      };
      showNotice(t(noticeKeys[noticeKind]), noticeKind, null, null, noticeKeys[noticeKind]);
    }
    return;
  }

  migrateLegacySettings(function () {
    invoke('get_settings').then(applySettings).catch(function () {});
  });
  invoke('get_snapshot').then(render).catch(function (err) {
    listEl.textContent = '';
    var empty = el('div', 'empty');
    empty.appendChild(el('div', 'empty-title', 'Unable to read sessions'));
    empty.appendChild(el('div', 'empty-hint', String(err)));
    listEl.appendChild(empty);
  });

  invoke('get_pet_state').then(applyPetState).catch(function () {});

  listen('tasks-updated', function (event) { render(event.payload); });
  listen('pet-state', function (event) { applyPetState(event.payload); });
  listen('jump-feedback', function (event) { handleOutcome(event.payload); });
  listen('settings-changed', function (event) { applySettings(event.payload); });
  listen('attention-pending', function (event) { handleAttentionPending(event.payload); });
  initCustomIcons();

  // One shared timer: only relative-time labels change, no re-render.
  setInterval(refreshTimes, 30000);
}

boot();
