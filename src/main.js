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
    enabled_agents: { codex: true, dsh: true, zcode: true, claude_code: true, antigravity: true },
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

function resolveLanguage(value) { return SessionUI.resolveLanguage(value); }

// ── i18n ────────────────────────────────────────────────────────

var I18N = SessionUI.I18N;

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
var mascotEl = document.getElementById('mascot-sprite');
var badgeEl = document.getElementById('badge');
var noticeDotEl = document.getElementById('notice-dot');
var statusDotsEl = document.getElementById('status-dots');
var statusDotQuestionEl = document.getElementById('status-dot-question');
var statusDotNewEl = document.getElementById('status-dot-new');
var statusDotFailedEl = document.getElementById('status-dot-failed');
var panelEl = document.getElementById('panel');
var listEl = document.getElementById('list');
var dragbarEl = document.getElementById('dragbar');
var collapseBtn = document.getElementById('collapse-btn');
var headerMetaEl = document.getElementById('header-meta');
var headerQuestionEl = document.getElementById('header-question');
var headerNewEl = document.getElementById('header-new');
var markAllReadEl = document.getElementById('mark-all-read');
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
// A native window drag suspends webview mouse events; the synthetic click
// after release can arrive after the `dragged` flag was reset. Time-based
// suppression catches it in every ordering.
var suppressClickUntil = 0;
var petStatePrevMode = null;

// ── "New completed" tracking + custom icons ──────────────────────
//
// Read state belongs to each completed task version. Opening the panel never
// acknowledges it; a confirmed jump acknowledges only the clicked version.
// The state itself lives in the shared SessionUI module (one truth for the
// pet panel and the island panel); this file wires the pet-side UI reactions.
SessionUI.reads.init(MOCK ? "am-test-completion-reads-v2" : undefined);
var lastCompletionWatch = '';
// Rust emits `jump-feedback` for every jump *and* returns the outcome, so one
// click can deliver the same soft result twice. `degraded` / `unconfirmed`
// outcomes are therefore reported at most once per task+reason per app session
// (in memory only); failures always report.
var softOutcomeNotified = {};
// Last update version announced; a repeated delivery of the same version is
// ignored for the rest of the session (in memory only).
var lastUpdateVersion = null;
var customIcons = { collapsed: null, hidden: null };

var STATUS_GLYPH = SessionUI.STATUS_GLYPH;

// Reason keys that mean the jump target itself is permanently gone. When
// `auto_dismiss_invalid` is on, these outcomes remove the row and offer Undo.
var INVALID_TARGET_REASONS = [
  'target_invalid', 'task_gone', 'workspace_missing', 'rollout_missing',
  'no_target', 'app_not_found'
];


// ── Helpers ─────────────────────────────────────────────────────

function relativeTime(epochSecs) { return SessionUI.relativeTime(epochSecs, lang); }

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

function isNewCompletedTask(task) {
  return SessionUI.reads.isNew(task);
}

function unreadCompletionIds() {
  return (snapshot.tasks || []).filter(isNewCompletedTask).map(function (task) { return task.id; });
}

function countNewCompleted() {
  return unreadCompletionIds().length;
}

function acknowledgeCompletion(task) {
  acknowledgeCompletions([task]);
}

function acknowledgeCompletions(tasks) {
  if (!SessionUI.reads.acknowledge(tasks, snapshot.tasks)) return;
  updateNewIndicators();
  if (isPanelOpen()) renderList(snapshot.tasks || []);
}

function acknowledgeViewedCompletion(viewed) {
  var task = SessionUI.reads.viewedTask(viewed, snapshot.tasks);
  if (task) acknowledgeCompletion(task);
}

function syncCompletionWatch() {
  var completions = (snapshot.tasks || []).filter(isNewCompletedTask).map(function (task) {
    return { task_id: task.id, completion_id: task.completion_id };
  });
  var key = JSON.stringify(completions);
  if (key === lastCompletionWatch) return;
  lastCompletionWatch = key;
  invoke('watch_unread_completions', { completions: completions }).catch(function () {});
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
  syncCompletionWatch();
  var open = isPanelOpen();
  var enabled = settings.new_badge !== false;

  var unreadCount = countNewCompleted();

  var showHeader = enabled && open && unreadCount > 0;
  headerNewEl.hidden = !showHeader;
  markAllReadEl.hidden = !showHeader;
  markAllReadEl.textContent = t('markAllRead');
  headerNewEl.textContent = showHeader
    ? t('newBadge').replace('{n}', unreadCount)
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
  syncStatusDots();
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

// ── Collapsed semantic status cluster ──────────────────────────
//
// Three independent dots, painted only while the pet is collapsed /
// edge-hidden / tray-hidden: amber = a session is waiting on a question,
// green = a task completed since the last look, red = a session failed or was
// interrupted. The data comes from the existing snapshot/question state only
// (no Rust duplication, no timers); `updateQuestionSurfaces` is the common
// repaint point for render, settings changes and `attention-pending`.
function failedTaskCount() {
  var tasks = snapshot.tasks || [];
  var count = 0;
  for (var i = 0; i < tasks.length; i++) {
    if (tasks[i].status === 'failed') count++;
  }
  return count;
}

function paintStatusDot(dotEl, active, count, labelKey, parts) {
  dotEl.hidden = !active;
  if (!active) {
    dotEl.removeAttribute('title');
    dotEl.removeAttribute('aria-label');
    return;
  }
  var label = t(labelKey).replace('{n}', count);
  dotEl.title = label;
  dotEl.setAttribute('aria-label', label);
  parts.push(label);
}

function syncStatusDots() {
  var open = isPanelOpen();
  var questionCount = effectiveQuestionCount();
  var newCount = settings.new_badge !== false ? countNewCompleted() : 0;
  var failedCount = failedTaskCount();
  var active = !open && (questionCount > 0 || newCount > 0 || failedCount > 0);
  statusDotsEl.hidden = !active;

  var parts = [];
  paintStatusDot(statusDotQuestionEl, questionCount > 0, questionCount, 'dotQuestion', parts);
  paintStatusDot(statusDotNewEl, newCount > 0, newCount, 'dotNew', parts);
  paintStatusDot(statusDotFailedEl, failedCount > 0, failedCount, 'dotFailed', parts);

  var separator = lang === 'zh' ? '，' : ', ';
  if (active) statusDotsEl.setAttribute('aria-label', parts.join(separator));
  else statusDotsEl.removeAttribute('aria-label');

  // The 14 px tab only fits one dot: highest priority wins (question > failed >
  // new) and that dot carries `.lead` for the compact CSS variant.
  statusDotQuestionEl.classList.toggle('lead', questionCount > 0);
  statusDotFailedEl.classList.toggle('lead', questionCount === 0 && failedCount > 0);
  statusDotNewEl.classList.toggle('lead',
    questionCount === 0 && failedCount === 0 && newCount > 0);
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

// ── Roaming mascot (Rust pet engine) ────────────────────────────
//
// Rust runs the Shimeji-compatible behavior engine and moves this window
// every tick; the webview only blits the current PNG at the anchor slot.
// State arrives via `pet-state` (mascot flag + settings) and `pet-frame`
// (image + pose anchor + facing).

var mascotFrameUrls = {};   // "pack|/shime1.png" or "pack|state" -> data URL
var mascotPackSizes = {};   // pack -> [frame_w, frame_h]
var mascotPackMeta = {};    // pack -> { kind, windowSize, menu: [{action,label_key}] }
var mascotPackMetaPending = {};
var mascotMenu = null;      // { l, t, r, b } window growth while menu open
var lastMascotFrame = null;

function mascotAnchorSlot() {
  var size = Math.max(48, Math.min(96, Number(settings.collapsed_size) || 72));
  // Must match Rust's MASCOT_BOTTOM_MARGIN (6 logical px above the bottom).
  return { size: size, x: size / 2, y: size - 6 };
}

function ensureMascotPackMeta(pack, done) {
  if (mascotPackMeta[pack]) { done(); return; }
  if (mascotPackMetaPending[pack]) return;
  mascotPackMetaPending[pack] = true;
  invoke('list_pet_packs').then(function (packs) {
    (packs || []).forEach(function (p) {
      if (p.frame_size && p.frame_size[0] > 0) {
        mascotPackSizes[p.name] = [p.frame_size[0], p.frame_size[1]];
      }
      // The backend defines its own right-click menu (id + i18n key): new
      // pet kinds never need frontend menu tables.
      mascotPackMeta[p.name] = {
        kind: p.kind || 'shimeji',
        windowSize: p.window_size || 72,
        menu: p.menu || []
      };
    });
    delete mascotPackMetaPending[pack];
    done();
  }).catch(function () {
    delete mascotPackMetaPending[pack];
    mascotPackMeta[pack] = { kind: 'shimeji', windowSize: 72, menu: [] };
    mascotPackSizes[pack] = [32, 32];
    done();
  });
}

function ensureMascotFrame(pack, image, done) {
  var key = pack + '|' + image;
  if (mascotFrameUrls[key]) { done(mascotFrameUrls[key]); return; }
  invoke('get_pet_pack_image', { pack: pack, image: image }).then(function (url) {
    mascotFrameUrls[key] = url;
    done(url);
  }).catch(function () { /* keep the previous frame showing */ });
}

function drawMascotFrame(frame) {
  if (!frame || !frame.kind) return;
  petDebug('draw kind=' + frame.kind + ' petState.mascot=' + petState.mascot +
    ' hidden=' + mascotEl.hidden);
  lastMascotFrame = frame;
  var pack = (petState.settings && petState.settings.mascot_pack) || 'nailong';
  ensureMascotPackMeta(pack, function () {
    var slot = mascotAnchorSlot();
    if (frame.kind === 'state') {
      // Familiar-style state image: fills the pet square, APNG animates
      // itself, no mirroring math. Coordinates are window coordinates (the
      // sprite is a direct child of #app).
      ensureMascotFrame(pack, frame.state, function (url) {
        mascotEl.classList.add('state');
        mascotEl.src = url;
        mascotEl.style.width = slot.size + 'px';
        mascotEl.style.height = slot.size + 'px';
        positionMascotInWindow();
      });
      return;
    }
    mascotEl.classList.remove('state');
    var size = mascotPackSizes[pack] || [32, 32];
    var fw = size[0];
    var fh = size[1];
    var zoom = slot.size / Math.max(fw, fh, 1);
    ensureMascotFrame(pack, frame.image, function (url) {
      var ax = Number(frame.image_anchor && frame.image_anchor[0]) || 0;
      var ay = Number(frame.image_anchor && frame.image_anchor[1]) || 0;
      var anchorX = frame.facing_right ? fw - ax : ax;  // mirrored art
      mascotEl.src = url;
      mascotEl.style.width = (fw * zoom) + 'px';
      mascotEl.style.height = (fh * zoom) + 'px';
      mascotEl.style.left = (slot.x - anchorX * zoom) + 'px';
      mascotEl.style.top = (slot.y - ay * zoom) + 'px';
      mascotEl.style.transform = frame.facing_right ? 'scaleX(-1)' : 'none';
    });
  });
}

// When the pet is edge-hidden, the window shrinks to a sliver; the state
// art is transparent at its borders, so WITHOUT shifting it the sliver
// shows nothing but the status dots. Slide the art so a body part lands in
// the visible strip (head-side stays as centered as the sliver allows).
function positionMascotInWindow() {
  if (mascotEl.hidden) return;
  var size = Math.max(48, Math.min(96, Number(settings.collapsed_size) || 72));
  if (petState.mode !== 'edge_hidden') {
    mascotEl.style.left = '0px';
    mascotEl.style.top = '0px';
    return;
  }
  var sliver = Number(settings.hidden_sliver) || 14;
  var edge = petState.edge || 'none';
  // Center the art's body on the visible strip: state-pack canvases have
  // transparent borders, so showing the art's edge would show nothing.
  if (edge === 'right' || edge === 'left') {
    mascotEl.style.left = (sliver / 2 - size / 2) + 'px';
    mascotEl.style.top = '0px';
  } else if (edge === 'bottom' || edge === 'top') {
    mascotEl.style.left = '0px';
    mascotEl.style.top = (sliver / 2 - size / 2) + 'px';
  }
}

var mascotDebugCount = 0;
function petDebug(msg) {
  // Render-chain telemetry: the first hops go to the Rust log (stderr under
  // AGENT_MONITOR_DEBUG_MASCOT) so headless runs can find the broken link.
  if (mascotDebugCount > 12) return;
  mascotDebugCount += 1;
  invoke('pet_debug_log', { msg: msg }).catch(function () {});
}

function initMascot() {
  if (MOCK) return;   // headless UI tests never receive frames
  petDebug('listener registering, petState.mascot=' + petState.mascot);
  listen('pet-frame', function (event) {
    var payload = event.payload || {};
    petDebug('frame received kind=' + payload.kind + ' state=' + (payload.state || payload.image));
    if (!petState.mascot) return;
    drawMascotFrame(payload);
  });
  mascotEl.addEventListener('load', function () {
    petDebug('img loaded ' + mascotEl.naturalWidth + 'x' + mascotEl.naturalHeight +
      ' rect=' + mascotEl.style.left + ',' + mascotEl.style.top + ' ' + mascotEl.style.width);
  });
  mascotEl.addEventListener('error', function () {
    petDebug('img ERROR src=' + String(mascotEl.src).slice(0, 60));
  });
  // Right-click anywhere on the window opens the NATIVE action menu at the
  // cursor (Rust side): zero geometry impact, no window growing.
  appEl.addEventListener('contextmenu', function (event) {
    if (petState.mode !== 'collapsed' && petState.mode !== 'edge_hidden') return;
    event.preventDefault();
    event.stopPropagation();
    cancelAutoHide();
    invoke('pet_menu_open').catch(function () {}).then(refreshAutoHide);
  });
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
  // The status capsule shares the top-left corner with the centred glyph. At the
  // smallest sizes there is not enough room for both at full size, so CSS uses
  // a tighter capsule and a slightly narrower glyph for that band.
  appEl.setAttribute('data-pet-size', collapsedSize < 64 ? 'xs' : 'md');

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
    // The strip's along-edge length is shared with the collapsed pet size.
    var sliver = Math.max(8, Math.min(24, Number(settings.hidden_sliver) || 14));
    var length = Math.max(48, Math.min(96, Number(settings.collapsed_size) || 72));
    var horizontal = petState.edge === 'top' || petState.edge === 'bottom';
    appEl.style.width = (horizontal ? length : sliver) + 'px';
    appEl.style.height = (horizontal ? sliver : length) + 'px';
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
    '|' + snapshotData.question_count + '|' + JSON.stringify(snapshotData.custom_names || {});
  if (MOCK) snapshotData.tasks.forEach(function (task) {
    if (!task.completion_id) task.completion_id = String(task.last_activity_secs);
  });
  snapshot = snapshotData;
  // Seed historical answers once; later metadata updates cannot revive them.
  SessionUI.reads.seedHistorical(snapshot.tasks || []);
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

var nameDialog = null;
function openNameEditor(task) {
  if (nameDialog) return;
  if (collapseTimer) { clearTimeout(collapseTimer); collapseTimer = null; }
  invoke('set_expanded', { expanded: true, focus: true }).catch(function () {});
  var dialog = document.createElement('dialog');
  nameDialog = dialog;
  dialog.className = 'name-dialog';
  dialog.setAttribute('aria-label', t('rename'));
  var form = document.createElement('form');
  var label = el('label', '', t('rename'));
  var input = document.createElement('input');
  input.id = 'session-name';
  input.type = 'text';
  input.maxLength = 120;
  input.value = (snapshot.custom_names || {})[task.id] || '';
  input.placeholder = task.title;
  label.htmlFor = input.id;
  var hint = el('p', 'dim', t('nameHint'));
  var error = el('p', 'name-error');
  error.setAttribute('role', 'alert');
  var actions = el('div', 'name-actions');
  var cancel = el('button', 'link-btn', t('cancelName'));
  cancel.type = 'button';
  var save = el('button', 'link-btn', t('saveName'));
  save.type = 'submit';
  actions.append(cancel, save);
  form.append(label, input, hint, error, actions);
  dialog.appendChild(form);
  document.body.appendChild(dialog);
  function closeEditor() {
    dialog.close();
    dialog.remove();
    nameDialog = null;
    var row = Array.from(listEl.children).find(function (row) { return row.dataset.taskId === task.id; });
    if (row) row.querySelector('.rename').focus();
  }
  cancel.addEventListener('click', closeEditor);
  dialog.addEventListener('cancel', function (event) { event.preventDefault(); closeEditor(); });
  form.addEventListener('submit', function (event) {
    event.preventDefault();
    save.disabled = true;
    invoke('set_task_name', { taskId: task.id, name: input.value }).then(function (next) {
      render(next);
      closeEditor();
    }).catch(function (reason) {
      error.textContent = String(reason && reason.message ? reason.message : reason);
      save.disabled = false;
    });
  });
  dialog.showModal();
  input.focus();
  input.select();
}

function renderTask(task) {
  var info = SessionUI.taskInfo(task, snapshot.custom_names, settings, lang);
  var displayName = info.title;
  var row = document.createElement('button');
  row.type = 'button';
  row.className = 'task session-row status-' + task.status + (task.status === 'waiting_input' ||
    task.status === 'waiting_approval' || task.status === 'failed' ? ' attention' : '');
  row.classList.toggle('pinned', !!task.pinned);
  row.classList.toggle('has-summary', !!info.summary);
  row.setAttribute('role', 'listitem');
  row.dataset.taskId = task.id;

  var showLevel = settings.show_jump_level !== false;
  var showSummary = settings.show_summary !== false;
  var statusLabel = info.status;
  var levelText = showLevel ? (task.level || 'L0') : '';
  // Rows that completed since the user last looked carry a labelled green dot,
  // so "which one is new" is visible on the row itself, not only as a count.
  var isNewRow = settings.new_badge !== false && isNewCompletedTask(task);
  if (isNewRow) row.classList.add('new-completed');
  row.setAttribute('aria-label',
    (isNewRow ? t('newRowAria') + ', ' : '') +
    task.agent_name + ', ' + displayName + ', ' + statusLabel +
    (levelText ? ', ' + levelText : '') + ', ' + t('jumpHint'));
  row.title = displayName + '\n' + statusLabel + ' · ' +
    (task.status_reason || '') + '\n' + t('jumpHint') +
    (isNewRow ? '\n' + t('newRowAria') : '') +
    (levelText ? ' (' + levelText + ')' : '');

  var avatar = SessionUI.avatar(task);
  avatar.classList.add('task-avatar');
  row.appendChild(avatar);
  var top = el('div', 'task-top');
  var pin = el('span', 'pin' + (task.pinned ? ' active' : ''));
  pin.innerHTML = '<svg viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.7" aria-hidden="true"><path d="M9 3h6l-1 6 4 3v2H6v-2l4-3-1-6ZM12 14v7"/></svg>';
  pin.setAttribute('role', 'button');
  pin.tabIndex = 0;
  pin.setAttribute('aria-label', t(task.pinned ? 'unpin' : 'pin') + ': ' + displayName);
  pin.setAttribute('aria-pressed', task.pinned ? 'true' : 'false');
  pin.title = t(task.pinned ? 'unpin' : 'pin');
  var togglePin = function (event) {
    event.stopPropagation();
    event.preventDefault();
    invoke('set_task_pinned', { taskId: task.id, pinned: !task.pinned }).then(render).catch(function (error) {
      showNotice(String(error && error.message ? error.message : error), 'error', null);
    });
  };
  pin.addEventListener('click', togglePin);
  pin.addEventListener('keydown', function (event) {
    if (event.key === 'Enter' || event.key === ' ') togglePin(event);
  });
  top.appendChild(pin);
  if (isNewRow) {
    top.appendChild(SessionUI.reads.marker(task, displayName, lang, acknowledgeCompletion));
  }
  var title = el('span', 'title', displayName);
  title.title = task.title;
  top.appendChild(title);
  var rename = el("span", "rename");
  rename.innerHTML = '<svg viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.7" aria-hidden="true"><path d="m15 5 4 4M4 20l4-1 12-12a2.8 2.8 0 0 0-4-4L4 15v5Z"/></svg>';
  rename.setAttribute("role", "button");
  rename.tabIndex = 0;
  rename.title = t("rename");
  rename.setAttribute("aria-label", t("rename") + ": " + displayName);
  function editName(event) {
    event.preventDefault();
    event.stopPropagation();
    openNameEditor(task);
  }
  rename.addEventListener("click", editName);
  rename.addEventListener("keydown", function (event) {
    if (event.key === "Enter" || event.key === " ") editName(event);
  });
  top.appendChild(rename);

  var status = el('div', 'status');
  status.appendChild(el('span', 'agent', task.agent_name));
  if (info.host) status.appendChild(el('span', 'host', info.host));
  if (task.origin === 'synthetic') status.appendChild(el('span', 'sim', 'SIM'));
  var phase = el('span', 'session-state');
  phase.appendChild(el('span', 'status-glyph', STATUS_GLYPH[task.status] || '?'));
  phase.appendChild(el('span', 'status-text', statusLabel));
  var modelName = info.model;
  var model = el('span', 'task-model', modelName);
  model.title = info.modelTitle;
  model.setAttribute('aria-label', model.title);
  status.appendChild(model);
  if (showLevel) status.appendChild(el('span', levelClass(task.level) + ' session-level', task.level || 'L0'));
  status.appendChild(phase);

  var meta = el('div', 'meta');
  var timeEl = el('span', 'time', relativeTime(task.last_activity_secs));
  timeEl.dataset.ts = String(task.last_activity_secs || 0);
  meta.appendChild(timeEl);

  var dismiss = el('span', 'dismiss', '×');
  dismiss.setAttribute('role', 'button');
  dismiss.setAttribute('aria-label', t('clear') + ': ' + displayName);
  dismiss.title = t('clear');
  dismiss.addEventListener('click', function (event) {
    event.stopPropagation();
    event.preventDefault();
    invoke('dismiss_task', { taskId: task.id }).catch(function () {});
  });

  row.appendChild(top);
  row.appendChild(status);
  if (showSummary && task.summary) row.appendChild(el('span', 'status-summary', task.summary));
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
    if (outcome && outcome.kind === 'success' && outcome.achieved === 'L3' && outcome.task_id === task.id) {
      acknowledgeCompletion(task);
    }
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
  var soft = outcome.kind === 'degraded' || outcome.kind === 'unconfirmed';
  // `toast_on_degraded=false` may silence only the softer notices; failures
  // are always reported.
  var suppressed = settings.toast_on_degraded === false && soft;
  // A repeated jump at an old/unconfirmed target must not nag again: soft
  // outcomes are reported at most once per task+reason per app session
  // (in memory only). Failures bypass this entirely and always report.
  var dedupeKey = soft && outcome.task_id ? outcome.task_id + ':' + reason : null;
  if (dedupeKey && softOutcomeNotified[dedupeKey]) return;
  var markReported = function () {
    if (dedupeKey) softOutcomeNotified[dedupeKey] = true;
  };

  if (settings.auto_dismiss_invalid &&
      (outcome.kind === 'failed' || outcome.kind === 'degraded') &&
      INVALID_TARGET_REASONS.indexOf(reason) !== -1 && outcome.task_id) {
    var taskId = outcome.task_id;
    if (autoDismissing[taskId]) return; // duplicate delivery of the same outcome
    autoDismissing[taskId] = true;
    invoke('dismiss_task', { taskId: taskId }).then(function (removed) {
      if (!removed) {
        delete autoDismissing[taskId];
        if (!suppressed) {
          markReported();
          showNotice(message, kind, outcome.permission_needed, null, key);
        }
        return;
      }
      // Auto-dismiss always keeps an Undo affordance, even for degraded
      // outcomes that would otherwise be silent.
      markReported();
      showNotice(message, kind, null, { type: 'undo', taskId: taskId }, key, 'dismissed');
    }).catch(function () {
      delete autoDismissing[taskId];
      if (!suppressed) {
        markReported();
        showNotice(message, kind, outcome.permission_needed, null, key);
      }
    });
    return;
  }

  if (suppressed) return;
  markReported();
  showNotice(message, kind, outcome.permission_needed, null, key);
}

function noticeMessage() {
  if (!notice) return '';
  if (!notice.messageKey) return notice.message || '';
  var message = t(notice.messageKey);
  if (message === notice.messageKey) message = notice.message || '';
  if (notice.vars) {
    for (var name in notice.vars) {
      if (Object.prototype.hasOwnProperty.call(notice.vars, name)) {
        message = message.split('{' + name + '}').join(notice.vars[name]);
      }
    }
  }
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
  } else if (action && action.type === 'open_settings') {
    toastActionEl.hidden = false;
    toastActionEl.textContent = t('update');
    toastActionEl.onclick = function (event) {
      event.stopPropagation();
      openSettings();
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

function showNotice(message, kind, permissionNeeded, action, messageKey, suffixKey, vars) {
  if (!message && !messageKey) return;
  notice = {
    message: message || '',
    kind: kind || 'info',
    permissionNeeded: permissionNeeded || null,
    action: action || null,
    messageKey: messageKey || null,
    suffixKey: suffixKey || null,
    vars: vars || null
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

// ── In-app update notification ──────────────────────────────────
//
// Rust emits `update-available` ({current_version, available, version, notes,
// pub_date}) when the updater finds a newer build. One notice is painted (toast
// while expanded, info dot while collapsed) whose action opens the Settings
// window (Updates pane). Repeated delivery of the same version is ignored for
// the rest of the session; in-memory only, no timer.
function handleUpdateAvailable(payload) {
  payload = payload || {};
  if (payload.available === false) return;
  var version = payload.version || null;
  if (!version) return;
  if (lastUpdateVersion === version) return;
  lastUpdateVersion = version;
  showNotice('', 'info', null, { type: 'open_settings' }, 'updateAvailable',
    null, { v: String(version) });
}

// ── Pet state machine (visual side) ─────────────────────────────

function applyPetState(next) {
  if (!next) return;
  petState = Object.assign({}, petState, next);
  // PetStateView carries the full Settings object; apply it synchronously so a
  // mode change never paints with stale typography/rows/flags.
  if (next.settings) applySettings(next.settings);

  var mode = petState.mode || 'collapsed';
  // While a native drag is in flight the webview gets no pointer events, so
  // the suppression window must keep refreshing for as long as the drag
  // lasts — a long drag would otherwise outlive the initial window and the
  // synthesized release-click would open the panel.
  if (mode === 'dragging') {
    suppressClickUntil = Date.now() + 800;
  } else if (petStatePrevMode === 'dragging') {
    suppressClickUntil = Date.now() + 600;
  }
  petStatePrevMode = mode;
  appEl.className = appEl.className
    .replace(/\bmode-[a-z_]+\b/g, '')
    .trim();
  appEl.classList.add('mode-' + mode);
  // Mascot visibility: the img stays visible in EVERY collapsed-like state,
  // including edge-hidden — the shrunken sliver window crops it to "a piece
  // of the pet peeking out", which is exactly the classic behaviour. Only
  // the expanded task panel replaces it. (`hidden` must NOT hide the img:
  // that left just the status dots on the edge tab.)
  var mascotVisible = !!petState.mascot && mode !== 'expanded';
  appEl.classList.toggle('mode-mascot', !!petState.mascot);
  mascotEl.hidden = !mascotVisible;
  positionMascotInWindow();
  if (!snapshot.tasks || snapshot.tasks.length === 0) appEl.classList.add('no-tasks');
  appEl.dataset.edge = petState.edge || 'none';
  appEl.dataset.hidden = petState.hidden ? 'true' : 'false';
  appEl.dataset.hiddenToTray = petState.hidden_to_tray ? 'true' : 'false';

  // In mascot mode dragging moves the pet itself: keep the sprite visible
  // while dragging instead of swapping to the task panel.
  var expanded = mode === 'expanded' || (mode === 'dragging' && !petState.mascot);
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
// hovers/drags it, tuck it into a reachable edge sliver. Auto-hide is disabled
// when edge hiding is off; only an explicit hide may send it to the tray.
// `auto_hide_after_s = 0` disables it, and a free-floating pet (edge "none")
// never hides itself. This is the only additional timer in the page; it exists
// only while armed.

var autoHideTimer = null;
var pointerInsidePet = false;

function cancelAutoHide() {
  if (autoHideTimer) { clearTimeout(autoHideTimer); autoHideTimer = null; }
}

function refreshAutoHide() {
  cancelAutoHide();
  var seconds = Number(settings.auto_hide_after_s) || 0;
  if (seconds <= 0 || settings.edge_hide_enabled === false) return;
  if (pointerInsidePet) return;
  if (petState.mode !== 'collapsed') return;
  if (!petState.edge || petState.edge === 'none') return;
  autoHideTimer = setTimeout(function () {
    autoHideTimer = null;
    if (pointerInsidePet) return;
    if (petState.mode !== 'collapsed') return;
    if (!petState.edge || petState.edge === 'none') return;
    invoke('hide_pet', { automatic: true }).catch(function () {});
  }, seconds * 1000);
}

function enterPet() {
  pointerInsidePet = true;
  cancelAutoHide();
  // In mascot mode the pointer is essentially always "on the pet" while it
  // walks, and hover-expand races the right-click action menu: the panel
  // popped up mid-right-click and the menu vanished. Mascot expands by
  // left-click instead.
  if (petState.mascot) return;
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
  pointerInsidePet = false;
  if (nameDialog) return;
  if (hoverTimer) { clearTimeout(hoverTimer); hoverTimer = null; }
  if (petState.mode === 'collapsed') {
    refreshAutoHide();
    return;
  }
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
  if (nameDialog) return;
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
    suppressClickUntil = Date.now() + 600;
    startWindowDrag();
  }
}

function onPointerUp() {
  var wasClick = pointerDown && !pointerDown.moved;
  if (pointerDown && pointerDown.moved) suppressClickUntil = Date.now() + 600;
  pointerDown = null;
  // Reset `dragged` on the next tick so the click handler (which fires after
  // pointerup) can still see it and suppress the jump.
  if (!wasClick) setTimeout(function () { dragged = false; }, 0);
}

petEl.addEventListener('mousedown', function (event) {
  if (event.button !== 0) return;   // right button opens the action menu
  if (petState.mode === 'expanded') return;
  onPointerDown(event, 'pet');
});
petEl.addEventListener('click', function (event) {
  event.stopPropagation();
  if (dragged || Date.now() < suppressClickUntil) {
    dragged = false;
    return;
  }
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

markAllReadEl.addEventListener('click', function (event) {
  event.stopPropagation();
  acknowledgeCompletions((snapshot.tasks || []).filter(isNewCompletedTask));
});

settingsBtn.addEventListener('click', function (event) {
  event.stopPropagation();
  openSettings();
});

document.addEventListener('keydown', function (event) {
  // Cmd+, (macOS Preferences) is the standard settings shortcut.
  if ((event.metaKey || event.ctrlKey) && event.key === ',') {
    event.preventDefault();
    openSettings();
    return;
  }
  if (event.key === 'Escape') {
    if (nameDialog) return;
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
  // Design-preview helper: `?mock=collapsed&new=1` adds a completion that only
  // shows as the green collapsed status dot / expanded header chip + row marker.
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
var permissionCalls = 0;
var restoreCalls = [];
var hidePetCalls = 0;
var expandCalls = [];
var collapseCalls = 0;

// Mock icon slots: tiny valid PNG data URLs exercise the exact payload shape
// (`{collapsed, hidden}`) without shipping image assets.
var MOCK_ICON_A = 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==';
var MOCK_ICON_B = 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=';
var mockIcons = { collapsed: MOCK_ICON_A, hidden: null };
var mockJumpOutcome = null;

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
  if (cmd === "set_task_name") {
    if (!mockTaskById(args.taskId)) throw new Error("task_gone");
    snapshot.custom_names = snapshot.custom_names || {};
    var name = args.name.trim();
    if (name) snapshot.custom_names[args.taskId] = name;
    else delete snapshot.custom_names[args.taskId];
    return snapshot;
  }
  if (cmd === 'set_task_pinned') {
    var pinTarget = mockTaskById(args && args.taskId);
    if (pinTarget) pinTarget.pinned = !!args.pinned;
    snapshot.tasks.sort(function (a, b) {
      return Number(!!b.pinned) - Number(!!a.pinned) || b.last_activity_secs - a.last_activity_secs;
    });
    render(snapshot);
    return snapshot;
  }
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
  if (cmd === 'open_permission_settings') {
    permissionCalls++;
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
    var outcome = mockJumpOutcome ? Object.assign({ task_id: taskId }, mockJumpOutcome) : (task && task.target_validity === 'invalid')
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
    get completionReads() { return SessionUI.reads.snapshot(); },
    get unreadCompletionIds() { return unreadCompletionIds(); },
    reloadCompletionReads: function () {
      SessionUI.reads.reload();
      updateNewIndicators();
      if (isPanelOpen()) renderList(snapshot.tasks || []);
    },
    setMockJumpOutcome: function (outcome) { mockJumpOutcome = outcome; },
    emitCompletionViewed: function (viewed) { mockEmit('completion-viewed', viewed); },
    get permissionCalls() { return permissionCalls; },
    get notifiedSoftOutcomes() { return Object.keys(softOutcomeNotified); },
    // Drive the Tauri `update-available` event exactly like the updater would.
    emitUpdate: function (payload) {
      var next = payload || {};
      mockEmit('update-available', next);
      return next;
    },
    // Drive a `jump-feedback` payload (also used for reasons the mock command
    // cannot produce, e.g. a second degradation reason for the same task).
    emitJumpFeedback: function (outcome) {
      mockEmit('jump-feedback', outcome);
      return outcome;
    },
    // Test-only: forget the once-per-task+reason soft-warning memory (the real
    // app only forgets it when the process ends).
    clearSoftOutcomeNotices: function () { softOutcomeNotified = {}; },
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
  listen('completion-viewed', function (event) { acknowledgeViewedCompletion(event.payload); });
  // The island window (same origin) wrote the read state: adopt it, refresh
  // the collapsed badge and re-sync the backend watch list. The shared
  // module owns the storage-event plumbing; this callback is pet-side only.
  SessionUI.reads.bind(function () {
    updateNewIndicators();
    if (isPanelOpen()) renderList(snapshot.tasks || []);
    syncCompletionWatch();
  });
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
    listen('update-available', function (event) { handleUpdateAvailable(event.payload); });
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
    // Design-preview helper: `?mock=1&update=0.7.0` paints the in-app update
    // notice exactly as the `update-available` event does.
    var previewUpdate = (location.search.match(/[?&]update=([0-9][^&]*)/) || [])[1];
    if (previewUpdate) {
      handleUpdateAvailable({
        current_version: '0.6.0', available: true,
        version: decodeURIComponent(previewUpdate), notes: null, pub_date: null
      });
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
  listen('update-available', function (event) { handleUpdateAvailable(event.payload); });
  initCustomIcons();
  initMascot();

  // One shared timer: only relative-time labels change, no re-render.
  setInterval(refreshTimes, 30000);
}

boot();
