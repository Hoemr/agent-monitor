/* Shared session language and presentation for the pet and island. */
(function () {
var I18N = {
  en: {
    sessionsTitle: 'Sessions',
    allSessions: '{n} sessions',
    pinnedLabel: 'Pinned',
    scrollSessions: 'Scroll for more sessions',
    islandQuiet: 'All quiet',
    islandActive: '{n} running',
    islandWaiting: '{n} waiting',
    islandFailed: '{n} failed',
    empty: 'No active sessions',
    emptyHint: 'Start a Codex, DSH, ZCode or Claude Code session',
    sessions: 'sessions',
    visibleCountTitle: 'Showing {v} of {t} sessions',
    needAttention: 'need attention',
    clear: 'Clear',
    collapse: 'Collapse',
    settings: 'Settings',
    undo: 'Undo',
    update: 'Update',
    updateAvailable: 'Version {v} is available',
    dismissed: 'Removed from the list.',
    dragHint: 'Drag to move',
    menuJump: 'Jump', menuRun: 'Run', menuWalk: 'Walk a bit',
    menuBounce: 'Celebrate', menuSit: 'Sit down', menuTrip: 'Stumble',
    menuStand: 'Stand still', menuCenter: 'Go to screen center',
    menuPet: 'Head pat', menuNap: 'Nap', menuCelebrate: 'Celebrate',
    notice: 'Notice: ',
    newBadge: '{n} new',
    newRowAria: 'Newly completed',
    markAllRead: 'Mark all as read',
    markRead: 'Mark as read',
    rename: 'Rename conversation',
    nameHint: 'Leave empty to restore the original name',
    saveName: 'Save',
    cancelName: 'Cancel',
    pin: 'Pin conversation',
    unpin: 'Unpin conversation',
    terminal: 'Terminal',
    modelUnknown: 'Model unknown',
    modelTitle: 'Latest recorded model: {model}',
    questionChip: '{n} waiting',
    questionChipAria: '{n} sessions waiting for your answer',
    questionAria: '{n} waiting for your answer',
    dotQuestion: '{n} waiting for an answer',
    dotNew: '{n} newly completed',
    dotFailed: '{n} interrupted or failed',
    collapsedAria: 'Agent Monitor, {n} sessions, {a} need attention, click to expand',
    emptyAria: 'Agent Monitor, no active sessions, click to expand',
    status: {
      running: 'Running', waiting_input: 'Waiting for input',
      waiting_approval: 'Waiting for approval', completed: 'Completed',
      failed: 'Failed', idle: 'Idle', unknown: 'Unknown'
    },
    jumpHint: 'Click to open',
    toast: {
      zcode_index_unavailable: 'Could not read ZCode’s current session index.',
      jump_in_progress: 'A session switch is already in progress. Wait for it to finish before selecting another.',
      jump_unavailable: 'Session switching is unavailable. Restart Agent Monitor.',
      zcode_session_ambiguous: 'This ZCode session needs a unique title. Rename unnamed or duplicate sessions in ZCode first.',
      zcode_search_unavailable: 'Could not open ZCode’s task search.',
      zcode_session_not_found: 'No uniquely matching ZCode session was found in task search.',
      zcode_session_unconfirmed: 'Could not confirm that ZCode selected the requested session.',
      opened_window: 'Opened the window — the exact session could not be selected.',
      opened_workspace: 'Opened the ZCode workspace — session selection needs a newer ZCode.',
      opened_session: '',
      opened_terminal_tab: '',
      activated_app: '',
      activated_app_only: 'Activated the app — the window could not be identified.',
      window_not_found: 'Activated the app — matching window not found.',
      windows_focus_failed: 'Could not focus the target app. Open it and select the conversation manually.',
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
      project_not_found: 'The project was not found in the DSH sidebar.',
      task_gone: 'The task no longer exists.',
      grant: 'Grant permission'
    }
  },
  zh: {
    sessionsTitle: '会话',
    allSessions: '{n} 个会话',
    pinnedLabel: '已置顶',
    scrollSessions: '滚动查看全部会话',
    islandQuiet: '暂无待处理',
    islandActive: '{n} 个运行中',
    islandWaiting: '{n} 待回答',
    islandFailed: '{n} 个失败',
    empty: '暂无活跃会话',
    emptyHint: '启动 Codex、DSH、ZCode 或 Claude Code 会话',
    sessions: '个会话',
    visibleCountTitle: '当前窗口显示 {v} 个，共 {t} 个会话',
    needAttention: '个待处理',
    clear: '清除',
    collapse: '收起',
    settings: '设置',
    undo: '撤销',
    update: '更新',
    updateAvailable: '新版本 {v} 可用',
    dismissed: '已从列表移除。',
    dragHint: '拖动以移动',
    menuJump: '跳一下', menuRun: '跑步', menuWalk: '走两步',
    menuBounce: '撒个欢', menuSit: '坐下', menuTrip: '摔一跤',
    menuStand: '站好', menuCenter: '回到屏幕中间',
    menuPet: '摸摸头', menuNap: '打个盹', menuCelebrate: '庆祝一下',
    notice: '提示：',
    newBadge: '{n} 个新完成',
    newRowAria: '新完成',
    markAllRead: '全部标为已读',
    markRead: '标为已读',
    rename: '自定义会话名称',
    nameHint: '留空可恢复原始名称',
    saveName: '保存',
    cancelName: '取消',
    pin: '置顶此会话',
    unpin: '取消置顶',
    terminal: '终端',
    modelUnknown: '模型未知',
    modelTitle: '最近记录的模型：{model}',
    questionChip: '{n} 待回答',
    questionChipAria: '{n} 个会话等待你的回答',
    questionAria: '{n} 个会话等待你的回答',
    dotQuestion: '{n} 个会话等待回答',
    dotNew: '{n} 个新完成',
    dotFailed: '{n} 个已中断或失败',
    collapsedAria: 'Agent Monitor，{n} 个会话，{a} 个待处理，点击展开',
    emptyAria: 'Agent Monitor，暂无活跃会话，点击展开',
    status: {
      running: '运行中', waiting_input: '等待输入',
      waiting_approval: '等待审批', completed: '完成',
      failed: '失败', idle: '空闲', unknown: '未知'
    },
    jumpHint: '点击跳转',
    toast: {
      zcode_index_unavailable: '无法读取 ZCode 当前会话索引。',
      jump_in_progress: '正在切换会话，请完成后再选择另一条。',
      jump_unavailable: '会话切换暂不可用，请重启 Agent Monitor。',
      zcode_session_ambiguous: '无法唯一识别该 ZCode 会话，请先在 ZCode 中区分重名或未命名会话。',
      zcode_search_unavailable: '无法打开 ZCode 的任务搜索。',
      zcode_session_not_found: 'ZCode 任务搜索中没有可唯一确认的目标会话。',
      zcode_session_unconfirmed: '无法确认 ZCode 已选中你指定的会话。',
      opened_window: '已打开对应窗口，无法定位具体会话。',
      opened_workspace: '已打开 ZCode 工作区窗口；会话级定位需要升级 ZCode。',
      opened_session: '',
      opened_terminal_tab: '',
      activated_app: '',
      activated_app_only: '已激活应用，无法识别对应窗口。',
      window_not_found: '已激活应用，未找到匹配窗口。',
      windows_focus_failed: '未能聚焦目标应用，请打开它并手动选择会话。',
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
      project_not_found: 'DSH 侧栏中找不到该项目。',
      task_gone: '任务已不存在。',
      grant: '去授权'
    }
  }
};

var STATUS_GLYPH = {
  running: '●', waiting_input: '!', waiting_approval: '!',
  completed: '✓', failed: '✕', idle: '○', unknown: '?'
};
function relativeTime(epochSecs, lang) {
  if (!epochSecs) return '';
  var diff = Math.max(0, Math.floor(Date.now() / 1000) - epochSecs);
  if (diff < 60) return lang === 'zh' ? '刚刚' : 'now';
  if (diff < 3600) return Math.floor(diff / 60) + 'm';
  if (diff < 86400) return Math.floor(diff / 3600) + 'h';
  return Math.floor(diff / 86400) + 'd';
}

function resolveLanguage(value) {
  if (value === "zh" || value === "en") return value;
  return (navigator.language || "").toLowerCase().indexOf("zh") === 0 ? "zh" : "en";
}
function translate(lang, key) {
  var value = I18N[lang] || I18N.en;
  key.split(".").forEach(function (part) { value = value && value[part]; });
  return typeof value === "string" ? value : key;
}
function taskInfo(task, names, settings, lang) {
  var t = function (key) { return translate(lang, key); };
  var model = task.model || t("modelUnknown");
  return {
    title: (names || {})[task.id] || task.title,
    status: t("status." + task.status),
    glyph: STATUS_GLYPH[task.status] || "?",
    host: task.host === "vscode" ? "VSCode" : task.host === "terminal" ? t("terminal") : "",
    model: model, modelTitle: t("modelTitle").replace("{model}", model),
    summary: settings.show_summary !== false ? (task.summary || "") : "",
    level: settings.show_jump_level !== false ? (task.level || "L0") : "",
    time: relativeTime(task.last_activity_secs, lang)
  };
}
function avatar(task) {
  var node = document.createElement('span');
  node.className = 'agent-avatar session-avatar';
  node.textContent = task.agent_name.replace(/\s+/g, '').slice(0, 2).toUpperCase();
  node.setAttribute('aria-hidden', 'true');
  return node;
}

// ── Unread completions: one shared read state ───────────────────
//
// Read state belongs to each completed task version. Opening a panel never
// acknowledges it; a confirmed jump acknowledges only the clicked version.
// The state lives here so the pet panel and the island panel share one
// truth (same app origin → same localStorage); cross-window changes arrive
// through the `storage` event via `reads.bind(onChange)`.

var COMPLETION_READ_KEY = 'am-completion-reads-v2';
var completionReads = loadCompletionReads();

function loadCompletionReads() {
  try {
    var stored = JSON.parse(localStorage.getItem(COMPLETION_READ_KEY));
    if (stored && typeof stored.since === 'number' && stored.read &&
        typeof stored.read === 'object' && !Array.isArray(stored.read)) return stored;
  } catch (e) { /* storage unavailable: keep read state for this session */ }
  return { since: Math.floor(Date.now() / 1000), read: {} };
}

function saveCompletionReads() {
  try { localStorage.setItem(COMPLETION_READ_KEY, JSON.stringify(completionReads)); }
  catch (e) { /* storage unavailable: keep read state for this session */ }
}

function isNewCompletedTask(task) {
  return task.status === 'completed' && typeof task.completion_id === 'string' &&
    completionReads.read[task.id] !== task.completion_id;
}

function unreadCompletions(tasks) {
  return (tasks || []).filter(isNewCompletedTask);
}

/// Acknowledge task versions. Every entry must still agree with the caller's
/// snapshot (status completed, same completion id) — a stale acknowledgement
/// never marks a task the snapshot no longer reports as that completion.
function acknowledgeCompletions(tasks, snapshotTasks) {
  var changed = false;
  (tasks || []).forEach(function (task) {
    if (task.status !== 'completed' || typeof task.completion_id !== 'string') return;
    if (snapshotTasks && !snapshotTasks.some(function (current) {
      return current.id === task.id && current.status === 'completed' &&
        current.completion_id === task.completion_id;
    })) return;
    if (completionReads.read[task.id] === task.completion_id) return;
    completionReads.read[task.id] = task.completion_id;
    changed = true;
  });
  if (changed) saveCompletionReads();
  return changed;
}

/// Completed sessions whose last activity predates the read state's birth are
/// never "new": seed them as read exactly once (called from applySnapshot).
function seedHistoricalCompletions(snapshotTasks) {
  var changed = false;
  (snapshotTasks || []).forEach(function (task) {
    if (task.status === 'completed' && task.completion_id &&
        completionReads.read[task.id] === undefined &&
        task.last_activity_secs <= completionReads.since) {
      completionReads.read[task.id] = task.completion_id;
      changed = true;
    }
  });
  if (changed) saveCompletionReads();
  return changed;
}

/// Validate a backend `completion-viewed` payload against the snapshot and
/// return the matching task (or null) for the caller to acknowledge.
function viewedCompletionTask(viewed, snapshotTasks) {
  if (!viewed || typeof viewed.task_id !== 'string' || typeof viewed.completion_id !== 'string') return null;
  return (snapshotTasks || []).find(function (item) {
    return item.id === viewed.task_id && item.status === 'completed' &&
      viewed.completion_id === item.completion_id;
  }) || null;
}

function reloadCompletionReads() {
  completionReads = loadCompletionReads();
}

/// Test isolation: tests run under a distinct storage key and deterministic
/// empty state (persisted acknowledgements from other runs must not leak in).
function initCompletionReads(key) {
  if (key) COMPLETION_READ_KEY = key;
  reloadCompletionReads();
}

function resetCompletionReads() {
  completionReads = { since: 0, read: {} };
  saveCompletionReads();
}

function bindCompletionReads(onChange) {
  window.addEventListener('storage', function (event) {
    if (event.key !== COMPLETION_READ_KEY) return;
    reloadCompletionReads();
    onChange();
  });
}

/// The unread marker itself: one element, one behaviour, one look — the pet
/// row and the island row both render this before the task title. Clicking
/// (or Enter/Space) acknowledges without jumping; `onAcknowledge(task)` is
/// the surface's own acknowledge path.
function newDotMarker(task, displayTitle, lang, onAcknowledge) {
  var marker = document.createElement('span');
  marker.className = 'new-dot';
  marker.setAttribute('role', 'button');
  marker.tabIndex = 0;
  var label = translate(lang, 'markRead');
  marker.title = label;
  marker.setAttribute('aria-label', label + ': ' + displayTitle);
  var acknowledge = function (event) {
    event.stopPropagation();
    event.preventDefault();
    onAcknowledge(task);
  };
  marker.addEventListener('click', acknowledge);
  marker.addEventListener('keydown', function (event) {
    if (event.key === 'Enter' || event.key === ' ') acknowledge(event);
  });
  return marker;
}

window.SessionUI = { I18N: I18N, STATUS_GLYPH: STATUS_GLYPH, resolveLanguage: resolveLanguage, t: translate, relativeTime: relativeTime, taskInfo: taskInfo, avatar: avatar,
  reads: {
    init: initCompletionReads,
    isNew: isNewCompletedTask,
    unread: unreadCompletions,
    acknowledge: acknowledgeCompletions,
    seedHistorical: seedHistoricalCompletions,
    viewedTask: viewedCompletionTask,
    reload: reloadCompletionReads,
    reset: resetCompletionReads,
    bind: bindCompletionReads,
    marker: newDotMarker,
    snapshot: function () { return JSON.parse(JSON.stringify(completionReads)); }
  } };
}());
