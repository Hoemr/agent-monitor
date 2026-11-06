'use strict';

/* Agent Monitor — Settings window (vanilla JS, no bundler).
 *
 * A native macOS Settings pane: source list on the left, grouped cards on the
 * right. Every change saves instantly through `set_settings` with the complete
 * Settings object (no OK/Cancel, no polling). Rust stays the single source of
 * truth: the normalized object returned by the command is what gets rendered,
 * and the `settings-changed` event from either window is applied immediately.
 *
 * Browser preview (packaged app never loads the mock path):
 *   src/settings.html?mock=1[&appearance=light|dark][&lang=en|zh][&group=general]
 * exposes `window.__AM_SETTINGS_TEST__` for scripts/ui-selftest-settings.html.
 */

// ── Tauri bridge (with browser mock for layout work) ────────────

var MOCK = location.search.indexOf('mock=') !== -1;
var tauri = window.__TAURI__ || null;
var invokeLog = [];
var mockListeners = {};
var mockState = {
  settings: null,
  meta: null,
  icons: { collapsed: null, hidden: null },
  iconMeta: {},
  dismissed: 3,
  closed: false,
  updateCheckMode: 'available',   // 'available' | 'uptodate' | 'error'
  permissionModes: { request: 'granted', reset: 'ok' }  // 'granted'|'denied'|'error', 'ok'|'error'
};

function clone(value) {
  return value === undefined ? value : JSON.parse(JSON.stringify(value));
}

function invoke(cmd, args) {
  args = args || {};
  invokeLog.push({ cmd: cmd, args: clone(args) });
  if (MOCK) {
    try {
      return Promise.resolve(mockInvoke(cmd, args));
    } catch (err) {
      return Promise.reject(err);
    }
  }
  if (!tauri || !tauri.core) return Promise.reject(new Error('Tauri bridge unavailable'));
  return tauri.core.invoke(cmd, args);
}

function listen(event, handler) {
  if (MOCK) {
    (mockListeners[event] = mockListeners[event] || []).push(handler);
    return Promise.resolve(function () {});
  }
  if (!tauri || !tauri.event) return Promise.resolve(function () {});
  return tauri.event.listen(event, handler);
}

function mockEmit(event, payload) {
  var handlers = mockListeners[event] || [];
  for (var i = 0; i < handlers.length; i++) {
    try { handlers[i]({ event: event, payload: clone(payload) }); } catch (e) { /* handler bug must not break mock */ }
  }
}

function queryParam(name) {
  var match = location.search.match(new RegExp('[?&]' + name + '=([^&]+)'));
  return match ? decodeURIComponent(match[1]) : null;
}

// ── Settings contract (mirrors src-tauri/src/settings.rs) ───────

var GROUPS = ['general', 'attention', 'monitor', 'pet', 'jump', 'update', 'data'];
var GROUP_KEY = 'am-settings-group';

var RETENTION_CHOICES = [1, 6, 24, 72, 168, 720];
var ROW_CHOICES = [3, 5, 8];
var RANGES = {
  hover_expand_ms: { min: 100, max: 1000, step: 10 },
  collapse_delay_ms: { min: 300, max: 2000, step: 50 },
  snap_distance: { min: 0, max: 40, step: 1 },
  hidden_sliver: { min: 8, max: 24, step: 1 },
  collapsed_size: { min: 48, max: 96, step: 4 },
  auto_hide_after_s: { min: 0, max: 60, step: 5 }
};

// Custom icons. Rust sniffs the real image bytes and enforces the same 2 MB
// cap (src-tauri/src/icons.rs), so client-side validation is a fast pre-check.
var ICON_MAX_BYTES = 2 * 1024 * 1024;
var ICON_ACCEPT = 'image/png,image/jpeg,image/webp';
var ICON_MIME_KIND = { 'image/png': 'PNG', 'image/jpeg': 'JPEG', 'image/webp': 'WebP' };

// Browser-mock artwork only (never used in the packaged app); lets
// `settings.html?mock=1&icon=custom` show a realistic preview for screenshots.
var SAMPLE_ICON = 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAJAAAACQCAYAAADnRuK4AAACoklEQVR42u3c3U4bMRCGYa8VKD3P1SKVJLTlajkHikR6B9kIb3btmeeVcpYD/7z5ZrxeKAUAAAAAsB5TT4P5+ft8tiXzvP2aJgIRJoRQE2nI1L1ApIkrUyVPXNZY+4k40qi7BCJPnjSaiCONukgg8uRMo0VMfHgmz4i8P7UnUSVPXpbYu0oeEm0iEHlI9G2ByEOi1Z5EIzZV+qBlbyt50LLHShjWKWHSRwp9WyDykEgJw5gvlCG5QMqXMnaJ3dwX6AMlDNsI9OMkf7Iz54AEghIGAiGaQPofXOOCBIISBgKBQCAQQCAQCMOwcwsPCQQCgUAgEEAgEAjDH+N75uNQXkvYVyfKXgKRJ838qsU1zxQCZZFntPlqokEgEAgEcrzNON966XWO3j73xxwS3R/Lvre1D1PCoks02vx2FhmaaBAIBAKBgARN9LX8O/ZznxS18a/kyTkeAg24WRElquQxPgKBQCAQHOPL8P9c4e5Q9p+nfvuMu0Os43zIBOp1k6LJE7qE9bZZEeUJ/yQ66qZJIBAIIBAIBE10Mz0/94ne6FfyGH9agUZf/AjzqBbdfDTRIBDcxqOREdd82ATaPcW65xp1PtWim0fqHmh0iYYfv18wnMJAIBAIBAIIBAKBQCAQQCAQCMXrHJBAAIFAIBAIBAIIBAKhT4G+/kyT5cGcCxIIShgIhIgC6YMw54AEQrnpHxa6lUdTAp3/KmNZuWbvlTA4haFzgZQx5as5gUhEHiUM2/ZAUkj6NCcQicijhGHbY7wUkj7NCUQi8jSXMBLllmeRHohEeeUppZTlNv/Rvf1QvCzzw6+9DQjjyLNsAkmjVOLc9jmQNEohz+0SSBqFF2e9J9HSKKw86ySQRAonzbYCkSlUyvdVXgilLQAAAAAAXOA/zIl9I8PKu0wAAAAASUVORK5CYII=';

function defaultSettings() {
  return {
    version: 1,
    language: 'system',
    appearance: 'system',
    show_summary: true,
    show_jump_level: true,
    new_badge: true,
    notify_on_question: true,
    alert_sound: true,
    auto_show_on_question: true,
    auto_check_updates: true,
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
    icon_style: 'builtin',
    pet_kind: 'classic',
    mascot_pack: 'nailong',
    toast_on_degraded: true,
    auto_dismiss_invalid: false,
    legacy_imported: false
  };
}

function withDefaults(raw) {
  var defaults = defaultSettings();
  if (!raw || typeof raw !== 'object') return defaults;
  var out = Object.assign({}, defaults, raw);
  out.enabled_agents = Object.assign({}, defaults.enabled_agents, raw.enabled_agents || {});
  if (typeof raw.version !== 'number') out.version = defaults.version;
  return out;
}

function clamp(value, min, max) {
  return Math.min(max, Math.max(min, value));
}

function snapTo(value, choices) {
  var best = choices[0];
  for (var i = 1; i < choices.length; i++) {
    if (Math.abs(choices[i] - value) < Math.abs(best - value)) best = choices[i];
  }
  return best;
}

/* Mirrors Settings::normalize() so the mock behaves like Rust. */
function normalizeSettings(settings) {
  var out = withDefaults(settings);
  out.version = 1;
  out.retention_hours = snapTo(out.retention_hours, RETENTION_CHOICES);
  out.max_visible_rows = clamp(snapTo(out.max_visible_rows, ROW_CHOICES), 1, 8);
  out.hover_expand_ms = clamp(out.hover_expand_ms, 100, 1000);
  out.collapse_delay_ms = clamp(out.collapse_delay_ms, 300, 2000);
  out.snap_distance = clamp(out.snap_distance, 0, 40);
  out.hidden_sliver = clamp(out.hidden_sliver, 8, 24);
  out.collapsed_size = clamp(out.collapsed_size, 48, 96);
  out.auto_hide_after_s = clamp(out.auto_hide_after_s, 0, 60);
  // New attention flags are plain booleans; coerce so an older/edited
  // settings.json can never put a non-boolean behind a switch.
  out.notify_on_question = !!out.notify_on_question;
  out.alert_sound = !!out.alert_sound;
  out.auto_show_on_question = !!out.auto_show_on_question;
  out.auto_check_updates = !!out.auto_check_updates;
  if (out.icon_style !== 'custom') out.icon_style = 'builtin';
  if (out.pet_kind !== 'mascot') out.pet_kind = 'classic';
  if (typeof out.mascot_pack !== 'string' || !out.mascot_pack.trim() || out.mascot_pack === 'agent-cat') out.mascot_pack = 'nailong';
  if (out.mascot_pack === 'byte-slime') out.mascot_pack = 'doraemon';
  return out;
}

function resetScope(settings, scope) {
  var out = withDefaults(settings);
  var d = defaultSettings();
  if (scope === 'all') return d;
  if (scope === 'general') {
    out.language = d.language;
    out.appearance = d.appearance;
    out.show_summary = d.show_summary;
    out.show_jump_level = d.show_jump_level;
    out.new_badge = d.new_badge;
  } else if (scope === 'attention') {
    out.notify_on_question = d.notify_on_question;
    out.alert_sound = d.alert_sound;
    out.auto_show_on_question = d.auto_show_on_question;
  } else if (scope === 'update') {
    out.auto_check_updates = d.auto_check_updates;
  } else if (scope === 'monitor') {
    out.refresh_interval = d.refresh_interval;
    out.retention_hours = d.retention_hours;
    out.enabled_agents = clone(d.enabled_agents);
    out.max_visible_rows = d.max_visible_rows;
  } else if (scope === 'pet') {
    out.hover_expand_ms = d.hover_expand_ms;
    out.collapse_delay_ms = d.collapse_delay_ms;
    out.snap_distance = d.snap_distance;
    out.hidden_sliver = d.hidden_sliver;
    out.collapsed_size = d.collapsed_size;
    out.auto_hide_after_s = d.auto_hide_after_s;
    out.edge_hide_enabled = d.edge_hide_enabled;
    out.animations = d.animations;
    out.icon_style = d.icon_style;
    out.pet_kind = d.pet_kind;
    out.mascot_pack = d.mascot_pack;
  } else if (scope === 'jump') {
    out.toast_on_degraded = d.toast_on_degraded;
    out.auto_dismiss_invalid = d.auto_dismiss_invalid;
  }
  return normalizeSettings(out);
}

// ── i18n ────────────────────────────────────────────────────────

var I18N = {
  en: {
    window: { title: 'Settings' },
    nav: {
      label: 'Settings sections',
      general: 'General', attention: 'Attention', monitor: 'Monitoring',
      pet: 'Desktop Pet', jump: 'Jump', update: 'Updates', data: 'Data'
    },
    pane: {
      general: 'General', attention: 'Attention', monitor: 'Monitoring',
      pet: 'Desktop Pet', jump: 'Jump', update: 'Updates', data: 'Data'
    },
    common: {
      cancel: 'Cancel', reset: 'Reset', clear: 'Clear', open: 'Open',
      decrease: 'Decrease', increase: 'Increase', restore: 'Restore'
    },
    saveHint: 'Changes save automatically',
    status: {
      saved: 'Saved',
      saveFailed: 'Could not save settings',
      reset: 'Defaults restored',
      dismissedCleared: 'Cleared {n} dismissed tasks',
      positionReset: 'Pet position reset'
    },
    loading: 'Loading…',
    error: {
      title: 'Settings are only available inside Agent Monitor',
      hint: 'In a browser, add ?mock=1 to the URL to preview this window.'
    },
    general: {
      pane: 'General',
      appearanceHeading: 'Appearance',
      language: { label: 'Language' },
      langSystem: 'System', langEn: 'English', langZh: '中文',
      appearance: { label: 'Appearance' },
      appSystem: 'System', appLight: 'Light', appDark: 'Dark',
      appearanceNote: 'Language applies to the menu bar, the pet and this window. Appearance follows the system unless overridden.',
      displayHeading: 'Display',
      showSummary: { label: 'Show summaries', desc: 'Show the latest activity summary on the second line.' },
      showJump: { label: 'Show jump level', desc: 'Show the L0–L3 jump-level hint on each row.' },
      newBadge: { label: 'Show new completions', desc: 'Show how many tasks completed since you last opened the panel, on the collapsed pet. Hidden when the count is zero.' },
      reset: { label: 'Reset General…', confirm: 'Restore the General section to defaults?' }
    },
    attention: {
      pane: 'Attention',
      alerts: 'When an agent waits for you',
      notify: {
        label: 'Notify me about new questions',
        desc: 'Show a macOS notification when an agent starts waiting for an answer, so it is visible while another app is focused.'
      },
      sound: {
        label: 'Play a sound',
        desc: 'Play the system “Glass” sound with every alert. It still plays when notifications are turned off.'
      },
      reveal: {
        label: 'Reveal the pet',
        desc: 'Expand the always-on-top pet so the waiting session is one click away. It appears without stealing keyboard focus.'
      },
      note: 'Only new questions alert. A question that is already waiting is not repeated on every refresh, and questions that existed at launch stay quiet.',
      reset: { label: 'Reset Attention…', confirm: 'Restore the Attention section to defaults?' }
    },
    monitor: {
      pane: 'Monitoring',
      collection: 'Collection',
      refresh: { label: 'Refresh interval' },
      refreshAuto: 'Auto', refresh3: '3 s', refresh5: '5 s', refresh10: '10 s',
      refreshNote: 'File events are the main source. Auto polls every 5 s while the panel is visible and every 30 s when it is hidden or idle.',
      retention: { label: 'Keep finished sessions' },
      retention1: '1 h', retention6: '6 h', retention24: '24 h', retention72: '3 d',
      retention168: '1 week', retention720: '1 month',
      retentionNote: 'Finished sessions are removed after this long. Running and waiting sessions are always kept.',
      agents: 'Agents',
      agentNote: 'Turn off agents you do not use. Their sessions are not collected or shown.',
      codex: { label: 'Codex', desc: 'Collect Codex CLI and app sessions.' },
      antigravity: { label: 'Antigravity', desc: 'Collect desktop and agy CLI sessions from their local databases.' },
      dsh: { label: 'DSH', desc: 'Collect DSH sessions.' },
      zcode: { label: 'ZCode', desc: 'Collect ZCode sessions.' },
      claude: { label: 'Claude Code', desc: 'Collect Claude Code CLI and VS Code sessions.' },
      window: 'Window',
      rows: { label: 'Visible rows' },
      rows3: '3', rows5: '5', rows8: '8',
      rowsNote: 'The expanded panel is 44 + rows × 64 + 26 pt tall, limited to the visible screen area.',
      reset: { label: 'Reset Monitoring…', confirm: 'Restore the Monitoring section to defaults?' }
    },
    pet: {
      pane: 'Desktop Pet',
      gestures: 'Hover',
      hover: { label: 'Expand delay', desc: 'How long the pointer rests before the pet opens.' },
      collapse: { label: 'Collapse delay', desc: 'Grace period after the pointer leaves.' },
      edges: 'Edges',
      snap: { label: 'Snap distance', desc: 'Snap to a screen edge within this distance. 0 disables snapping.' },
      sliver: { label: 'Edge sliver width', desc: 'Visible strip when the pet is hidden at an edge.' },
      size: { label: 'Collapsed size', desc: 'Pet size and hidden strip length share this value (48–96 px).' },
      autoHideSection: 'Auto-hide',
      autoHide: { label: 'Hide after snapping', desc: 'After snapping to an edge, tuck the idle pet into a visible edge strip. Requires Hide to edge; Off keeps it visible.' },
      autoHideOff: 'Off',
      edgeHide: {
        label: 'Hide to edge',
        descOn: 'Hiding the pet tucks it into the screen edge.',
        descOff: 'Off: hiding sends the pet to the menu bar tray. Choose “Show Pet” in the menu to bring it back.'
      },
      motion: 'Motion',
      animations: { label: 'Animations' },
      animSystem: 'Auto', animAlways: 'Always', animOff: 'Off',
      animationsNote: 'Auto follows Reduce Motion in System Settings.',
      iconSection: 'Icon',
      iconStyle: { label: 'Icon style' },
      iconBuiltin: 'Built-in', iconCustom: 'Custom',
      iconStyleNote: 'Custom uses your own images; empty slots fall back to the collapsed icon, then the built-in glyph. Built-in always uses the three-bar glyph.',
      iconNoCustomNote: 'No custom image yet — the built-in glyph is used until you add one.',
      iconSpec: 'Square image: 144 × 144 px for the collapsed icon, 64 × 64 px for the edge tab. Transparent background, PNG/JPEG/WebP, max 2 MB.',
      iconCollapsed: { label: 'Collapsed icon', desc: 'Shown when the pet is collapsed or snapped.' },
      iconHidden: { label: 'Edge-hidden tab icon', desc: 'Optional. Shown on the small tab when the pet is hidden at a screen edge.' },
      iconBuiltinGlyph: 'Built-in glyph',
      iconChoose: 'Choose…', iconReplace: 'Replace…', iconRemove: 'Remove',
      iconSaved: 'Icon saved', iconRemoved: 'Custom icon removed',
      iconError: {
        mime: 'Unsupported file type. Use a PNG, JPEG or WebP image.',
        tooLarge: 'Image is larger than 2 MB. Choose a smaller file.',
        read: 'Could not read that image.',
        save: 'Could not save the icon.',
        remove: 'Could not remove the icon.'
      },
      kindSection: 'Pet style',
      kind: { label: 'Presentation' },
      kindClassic: 'Capsule', kindMascot: 'Mascot',
      kindNote: 'The mascot is a roaming Shimeji-style pet driven by the Rust engine: it walks, falls, can be dragged and thrown, and reacts to task state. The capsule keeps the classic status beads.',
      pack: { label: 'Mascot pack' },
      packNote: 'Import a Shimeji-ee pack folder (conf/*.xml + img/*.png). Built-in packs: Agent Cat, Byte Slime.',
      packBuiltin: 'Built-in',
      packMissing: 'not installed',
      packImport: 'Import pack…',
      packImporting: 'Importing…',
      packImported: 'Pet pack imported',
      packImportError: 'Import failed',
      packBroken: 'unusable',
      reset: { label: 'Reset Desktop Pet…', confirm: 'Restore the Desktop Pet section to defaults?' }
    },
    jump: {
      pane: 'Jump',
      feedback: 'Feedback',
      toast: {
        label: 'Show “degraded” notices',
        desc: 'Jump failures are always reported. This only controls the softer “opened the app instead” notice.'
      },
      autoDismiss: {
        label: 'Remove tasks with a dead target',
        desc: 'When a jump target no longer exists, remove the task and show a toast with Undo.'
      },
      setup: 'Permissions & setup',
      appLocation: 'App location',
      appLocationDesc: 'The bundle this window is running from — the copy System Settings must contain.',
      staleGrant: 'Checked in System Settings but still “Not granted”? The grant may belong to an old ad-hoc build or another copy. Current local builds keep a fixed signing identity. Press “Reset this app’s permissions…” to clear the old record, add this build with “Reveal App”, and quit and reopen Agent Monitor if needed.',
      status: {
        accessibility: 'Accessibility',
        automation: 'Automation',
        windowAccess: 'Window access',
        granted: 'Granted',
        notGranted: 'Not granted',
        checking: 'Checking…',
        accessibilityDesc: 'Reads window titles and raises the right window (L2/L3 jumps).',
        automationDesc: 'Runs the System Events AppleScript calls used by L2/L3.',
        windowAccessDesc: 'The real read test: window titles of a running agent app.',
        systemEventsOn: 'System Events responded.',
        hint: {
          accessibility: 'Add Agent Monitor.app under Privacy & Security → Accessibility.',
          automation: 'Allow Agent Monitor to control System Events when macOS asks.',
          windowAccess: 'Needs Accessibility, and Automation for the System Events calls.'
        },
        detail: {
          needsAccessibility: 'Grant Accessibility first.',
          needsAutomation: 'Allow System Events under Privacy & Security → Automation.',
          osascriptFailed: 'The AppleScript helper failed; see the app log.',
          diagnosticsFailed: 'The self-check could not run; try again.',
          noAgentApp: 'No running agent app to read right now.',
          windowStateUnavailable: 'Window information is unavailable.',
          other: 'macOS reported an unknown permission state.'
        },
        failed: 'Could not run the self-check: {message}'
      },
      actions: {
        checkLabel: 'Self-check',
        checkDesc: 'Re-read the permission states and each agent’s capabilities.',
        checkButton: 'Check again',
        requestLabel: 'Request permission',
        requestDesc: 'Ask macOS to add this build to Accessibility so it shows up in the list.',
        requestButton: 'Request permission',
        requestHint: 'Permission requested. Turn the switch on in System Settings, then press “Check again”.',
        requestTrusted: 'Accessibility is already authorized. The status refreshes automatically.',
        revealLabel: 'Reveal the app in Finder',
        revealDesc: 'Show Agent Monitor.app in Finder so it can be dragged into the Accessibility list.',
        revealButton: 'Reveal App',
        resetLabel: 'Reset this app’s permissions…',
        resetDesc: 'Clear this build’s Accessibility and Automation records, then add the current build again.',
        resetButton: 'Reset…',
        resetConfirm: 'Clear this app’s Accessibility and Automation records? You will need to grant them again.',
        resetConfirmOk: 'Clear records',
        resetHint: 'Permission records reset. Add this build again with “Reveal App”, then press “Check again”.',
        open: 'Open',
        openAccessibility: 'Open Accessibility settings',
        openAutomation: 'Open Automation settings'
      },
      guide: {
        heading: 'First run',
        summary: 'Setup guide',
        summaryMissing: 'Permissions needed ({count})',
        summaryHint: '3 steps',
        intro: 'Three steps make window-level jumps work:',
        step1: 'Open System Settings → Privacy & Security → Accessibility, then add Agent Monitor.app with + — or drag it in from Finder after “Reveal App”.',
        step2: 'Come back here and press “Check again” until Accessibility and Window access read “Granted”.',
        step3: 'The first jump that needs System Events triggers macOS’s Automation prompt — allow Agent Monitor to control System Events.',
        adhoc: 'Local builds now use a persistent signing certificate. Moving from an old ad-hoc build requires one new grant; subsequent builds using the same certificate retain their identity.',
        appPath: 'App bundle: {path}'
      },
      agents: {
        heading: 'Agent capabilities',
        desc: 'Maximum jump level, install/run state and session-route support, as measured on this Mac.',
        installed: 'Installed',
        notInstalled: 'Not installed',
        running: 'Running',
        notRunning: 'Stopped',
        deepLink: 'Session route',
        noDeepLink: 'App level only',
        noDiagnostics: 'The self-check has not reported yet.'
      },
      reset: { label: 'Reset Jump…', confirm: 'Restore the Jump section to defaults?' }
    },
    update: {
      pane: 'Updates',
      versionSection: 'Version',
      currentVersion: 'Current version',
      releases: 'Releases',
      releasesNote: 'Release notes and downloads are published on GitHub. Opening the page is left to your browser; the URL above is shown for copying.',
      localBuild: 'Locally maintained version',
      localBuildNote: 'This version is updated locally to preserve its saved permissions. Online replacement is disabled. Use the local build script when updating.',
      autoChecks: 'Automatic checks',
      autoCheck: {
        label: 'Check for updates automatically',
        desc: 'Check GitHub Releases once about 4 seconds after launch. When Agent Monitor is up to date or offline, nothing is shown.'
      },
      statusSection: 'Update',
      check: {
        label: 'Check for Updates',
        desc: 'Ask GitHub whether a newer release exists. Nothing is downloaded until you confirm.'
      },
      checkButton: 'Check for Updates',
      idle: 'Not checked yet.',
      checking: 'Checking for updates…',
      upToDate: 'Up to date ({version}).',
      available: 'Update available: {version}',
      published: 'Published {date}',
      notesHeading: 'Release notes',
      installButton: 'Download & Install',
      installing: 'Downloading and installing {version}…',
      progressPct: 'Downloading… {pct}%',
      progressBytes: 'Downloaded {bytes}',
      installed: 'Update {version} installed — restart to finish.',
      restartButton: 'Restart Now',
      errorTitle: 'Could not check for updates.',
      errorInstall: 'Could not install the update.',
      errorHint: 'Check your network connection and try again.',
      reset: { label: 'Reset Updates…', confirm: 'Restore the Updates section to defaults?' }
    },
    data: {
      pane: 'Data',
      storage: 'Storage',
      dataDir: 'Data folder',
      settingsPath: 'Settings file',
      petPath: 'Pet placement file',
      dismissedPath: 'Dismissed tasks file',
      version: 'Version',
      settingsVersion: 'Settings format',
      notAvailable: 'Unavailable',
      actions: 'Actions',
      openData: { label: 'Open Data Folder', desc: 'Reveal the folder in Finder.', button: 'Open' },
      clearDismissed: {
        label: 'Clear Dismissed Tasks',
        desc: 'Ignored tasks can be collected again.',
        confirm: 'Clear all dismissed tasks?'
      },
      resetPosition: { label: 'Reset Pet Position', desc: 'Move the pet back to the bottom-right corner.', button: 'Reset' },
      danger: 'Reset',
      resetAll: {
        label: 'Reset All Settings…',
        desc: 'Pet position and dismissed tasks are kept.',
        confirm: 'Restore every setting to its default?'
      },
      legacy: 'Settings were imported from an older version.',
      corrupt: 'The settings file could not be read. A backup was kept at {path}.'
    }
  },
  zh: {
    window: { title: '设置' },
    nav: {
      label: '设置分区',
      general: '通用', attention: '提醒', monitor: '监控',
      pet: '桌宠', jump: '跳转', update: '更新', data: '数据'
    },
    pane: {
      general: '通用', attention: '提醒', monitor: '监控',
      pet: '桌宠', jump: '跳转', update: '更新', data: '数据'
    },
    common: {
      cancel: '取消', reset: '恢复', clear: '清除', open: '打开',
      decrease: '减少', increase: '增加', restore: '恢复'
    },
    saveHint: '更改会自动保存',
    status: {
      saved: '已保存',
      saveFailed: '无法保存设置',
      reset: '已恢复默认值',
      dismissedCleared: '已清除 {n} 个已忽略任务',
      positionReset: '桌宠位置已重置'
    },
    loading: '正在载入…',
    error: {
      title: '设置仅在 Agent Monitor 中可用',
      hint: '在浏览器中给网址加上 ?mock=1 即可预览此窗口。'
    },
    general: {
      pane: '通用',
      appearanceHeading: '外观',
      language: { label: '语言' },
      langSystem: '跟随系统', langEn: 'English', langZh: '中文',
      appearance: { label: '外观' },
      appSystem: '跟随系统', appLight: '浅色', appDark: '深色',
      appearanceNote: '语言应用于菜单栏、桌宠和此窗口。默认跟随系统外观，也可以单独指定。',
      displayHeading: '显示',
      showSummary: { label: '显示摘要', desc: '在第二行显示最新的活动摘要。' },
      showJump: { label: '显示跳转等级', desc: '在每行显示 L0–L3 的跳转等级提示。' },
      newBadge: { label: '显示新完成数量', desc: '在收起的桌宠上显示自上次打开面板以来完成的任务数量；数量为 0 时自动隐藏。' },
      reset: { label: '恢复通用默认设置…', confirm: '将“通用”恢复为默认设置？' }
    },
    attention: {
      pane: '提醒',
      alerts: 'Agent 等待你回答时',
      notify: {
        label: '新问题时发送系统通知',
        desc: 'Agent 开始等待回答时发一条 macOS 通知，即使你正专注于其他应用也能看到。'
      },
      sound: {
        label: '播放提示音',
        desc: '每次提醒播放系统“Glass”声音；即使关闭了通知也会单独播放。'
      },
      reveal: {
        label: '自动展开桌宠',
        desc: '展开置顶的桌宠，让等待中的会话一次点击即可到达；不会抢走键盘焦点。'
      },
      note: '仅对“新出现”的问题提醒。已在等待中的问题不会在每次刷新时重复提醒，启动前就存在的老问题也不会打扰你。',
      reset: { label: '恢复提醒默认设置…', confirm: '将“提醒”恢复为默认设置？' }
    },
    monitor: {
      pane: '监控',
      collection: '采集',
      refresh: { label: '刷新间隔' },
      refreshAuto: '自动', refresh3: '3 秒', refresh5: '5 秒', refresh10: '10 秒',
      refreshNote: '文件事件是主要来源。自动模式在面板可见时每 5 秒轮询，隐藏或空闲时每 30 秒。',
      retention: { label: '保留已结束会话' },
      retention1: '1 小时', retention6: '6 小时', retention24: '24 小时', retention72: '3 天',
      retention168: '1 周', retention720: '1 个月',
      retentionNote: '结束的会话在此时间后移除；运行中和等待中的会话始终保留。',
      agents: 'Agent',
      agentNote: '关闭不使用的 Agent，它们的会话不会被采集和显示。',
      codex: { label: 'Codex', desc: '采集 Codex 命令行与应用会话。' },
      antigravity: { label: 'Antigravity', desc: '采集桌面版和 agy CLI 的本地会话。' },
      dsh: { label: 'DSH', desc: '采集 DSH 会话。' },
      zcode: { label: 'ZCode', desc: '采集 ZCode 会话。' },
      claude: { label: 'Claude Code', desc: '采集 Claude Code 命令行与 VS Code 会话。' },
      window: '窗口',
      rows: { label: '可见行数' },
      rows3: '3', rows5: '5', rows8: '8',
      rowsNote: '展开高度 = 44 + 行数 × 64 + 26 pt，并受可见屏幕区域限制。',
      reset: { label: '恢复监控默认设置…', confirm: '将“监控”恢复为默认设置？' }
    },
    pet: {
      pane: '桌宠',
      gestures: '悬停',
      hover: { label: '展开延迟', desc: '指针悬停多久后展开桌宠。' },
      collapse: { label: '收起延迟', desc: '指针离开后的宽限时间。' },
      edges: '边缘',
      snap: { label: '吸附距离', desc: '距屏幕边缘小于该值时自动吸附，0 表示不吸附。' },
      sliver: { label: '边缘隐藏宽度', desc: '在屏幕边缘隐藏时保留的可见条宽度。' },
      size: { label: '收起尺寸', desc: '同时调整收起后的尺寸与边缘隐藏条的高度（上下吸附时为宽度），48–96 px。' },
      autoHideSection: '自动隐藏',
      autoHide: { label: '吸附后自动隐藏', desc: '吸附到屏幕边缘后，空闲超过该时长就收成可见的边缘条。仅在开启“隐藏到屏幕边缘”时生效；关闭则保持显示。' },
      autoHideOff: '关闭',
      edgeHide: {
        label: '隐藏到屏幕边缘',
        descOn: '隐藏时把桌宠贴到屏幕边缘。',
        descOff: '已关闭：隐藏时桌宠会收起到菜单栏托盘，可从菜单“显示桌宠”恢复。'
      },
      motion: '动态效果',
      animations: { label: '动画' },
      animSystem: '自动', animAlways: '总是', animOff: '关闭',
      animationsNote: '自动模式跟随系统“减弱动态效果”。',
      iconSection: '图标',
      iconStyle: { label: '图标样式' },
      iconBuiltin: '内置', iconCustom: '自定义',
      iconStyleNote: '自定义使用你自己的图片；槽位留空时回退为收起态图标，再回退为内置图案。内置始终使用三横线图标。',
      iconNoCustomNote: '尚未添加自定义图片，添加前仍使用内置图标。',
      iconSpec: '正方形图片：收起态 144 × 144 像素，边缘标签 64 × 64 像素；透明背景，支持 PNG/JPEG/WebP，最大 2 MB。',
      iconCollapsed: { label: '收起态图标', desc: '桌宠收起或吸附后显示。' },
      iconHidden: { label: '边缘隐藏标签图标', desc: '可选。桌宠隐藏到屏幕边缘时显示在小标签上。' },
      iconBuiltinGlyph: '内置图标',
      iconChoose: '选择…', iconReplace: '替换…', iconRemove: '移除',
      iconSaved: '图标已保存', iconRemoved: '已移除自定义图标',
      iconError: {
        mime: '不支持的文件类型，请使用 PNG、JPEG 或 WebP 图片。',
        tooLarge: '图片超过 2 MB，请选择更小的文件。',
        read: '无法读取该图片。',
        save: '无法保存图标。',
        remove: '无法移除图标。'
      },
      kindSection: '宠物形态',
      kind: { label: '呈现方式' },
      kindClassic: '经典胶囊', kindMascot: '活动宠物',
      kindNote: '活动宠物是 Rust 引擎驱动的 Shimeji 式宠物：会走动、掉落、可拖拽抛掷，并对任务状态做出反应。经典胶囊保留状态珠显示。',
      pack: { label: '宠物包' },
      packNote: '导入 Shimeji-ee 格式的宠物包文件夹（conf/*.xml + img/*.png）。内置：Agent Cat、Byte Slime。',
      packBuiltin: '内置',
      packMissing: '未安装',
      packImport: '导入宠物包…',
      packImporting: '导入中…',
      packImported: '宠物包已导入',
      packImportError: '导入失败',
      packBroken: '不可用',
      reset: { label: '恢复桌宠默认设置…', confirm: '将“桌宠”恢复为默认设置？' }
    },
    jump: {
      pane: '跳转',
      feedback: '反馈',
      toast: {
        label: '显示“降级”提示',
        desc: '跳转失败始终会提示；此开关只控制较轻微的“已改为打开应用”提示。'
      },
      autoDismiss: {
        label: '移除目标已失效的任务',
        desc: '跳转目标失效时自动移除任务，并弹出带“撤销”的提示。'
      },
      setup: '权限与配置',
      appLocation: 'App 位置',
      appLocationDesc: '当前窗口正在运行的 App 副本——也就是系统设置里需要包含的那一份。',
      staleGrant: '系统设置里勾选了但仍显示未授权？可能是授权仍绑定旧 ad-hoc 版本或另一个副本。当前本机构建使用固定签名身份。点“重新授权”清除旧记录，并用“显示 App”把当前版本加进辅助功能列表；必要时退出并重开 Agent Monitor。',
      status: {
        accessibility: '辅助功能',
        automation: '自动化',
        windowAccess: '窗口读取',
        granted: '已授权',
        notGranted: '未授权',
        checking: '正在检查…',
        accessibilityDesc: '读取窗口标题并抬起正确的窗口（L2/L3 跳转）。',
        automationDesc: '执行 L2/L3 所需的 System Events AppleScript 调用。',
        windowAccessDesc: '真实读取测试：读取正在运行的 Agent 应用的窗口标题。',
        systemEventsOn: 'System Events 已响应。',
        hint: {
          accessibility: '在“隐私与安全性”→“辅助功能”中加入 Agent Monitor.app。',
          automation: 'macOS 询问时允许 Agent Monitor 控制 System Events。',
          windowAccess: '需要辅助功能权限，System Events 调用还需要自动化权限。'
        },
        detail: {
          needsAccessibility: '请先授予辅助功能权限。',
          needsAutomation: '在“隐私与安全性”→“自动化”中允许 System Events。',
          osascriptFailed: 'AppleScript 调用失败，请查看应用日志。',
          diagnosticsFailed: '自检未能运行，请重试。',
          noAgentApp: '当前没有正在运行的 Agent 应用可供读取。',
          windowStateUnavailable: '暂时无法读取窗口信息。',
          other: 'macOS 返回了未知的权限状态。'
        },
        failed: '自检未能运行：{message}'
      },
      actions: {
        checkLabel: '自检',
        checkDesc: '重新读取三项权限状态与各 Agent 的跳转能力。',
        checkButton: '重新检查',
        requestLabel: '请求授权',
        requestDesc: '请 macOS 把当前版本加入辅助功能列表，使其出现在系统设置中。',
        requestButton: '请求授权',
        requestHint: '已请求授权，请在系统设置中打开开关后点“重新检查”。',
        requestTrusted: '辅助功能已授权，状态会自动刷新。',
        revealLabel: '在访达中显示 App',
        revealDesc: '在访达中显示 Agent Monitor.app，便于拖入辅助功能列表。',
        revealButton: '显示 App',
        resetLabel: '重置本应用授权…',
        resetDesc: '清除本版本的辅助功能与自动化授权记录，然后重新添加当前版本。',
        resetButton: '重置…',
        resetConfirm: '清除本应用的辅助功能与自动化授权记录？之后需要重新授权。',
        resetConfirmOk: '清除记录',
        resetHint: '已重置本应用的授权记录，请用“显示 App”重新添加当前版本并点“重新检查”。',
        open: '打开',
        openAccessibility: '打开辅助功能设置',
        openAutomation: '打开自动化设置'
      },
      guide: {
        heading: '首次配置',
        summary: '配置向导',
        summaryMissing: '需要授权（{count} 项）',
        summaryHint: '3 步',
        intro: '三步让窗口级跳转生效：',
        step1: '打开“系统设置”→“隐私与安全性”→“辅助功能”，点“+”加入 Agent Monitor.app；或先点“显示 App”再从访达拖入。',
        step2: '回到这里点“重新检查”，直到“辅助功能”和“窗口读取”都显示“已授权”。',
        step3: '第一次需要 System Events 的跳转会触发 macOS 自动化授权弹窗——允许 Agent Monitor 控制 System Events。',
        adhoc: '本机构建现已使用固定签名证书。从旧 ad-hoc 版本切换时需授权一次；后续使用同一证书重建时，应用身份保持不变。',
        appPath: 'App 位置：{path}'
      },
      agents: {
        heading: 'Agent 能力',
        desc: '本机实测的最高跳转等级、安装/运行状态与会话级跳转支持。',
        installed: '已安装',
        notInstalled: '未安装',
        running: '运行中',
        notRunning: '未运行',
        deepLink: '支持会话级跳转',
        noDeepLink: '仅应用级',
        noDiagnostics: '自检尚未返回结果。'
      },
      reset: { label: '恢复跳转默认设置…', confirm: '将“跳转”恢复为默认设置？' }
    },
    update: {
      pane: '更新',
      versionSection: '版本',
      currentVersion: '当前版本',
      releases: '发布页面',
      releasesNote: '发布说明与下载包都发布在 GitHub。打开网页请使用浏览器；上方地址可直接复制。',
      localBuild: '本机维护版本',
      localBuildNote: '这个版本通过本机更新来保留已授予的权限，不使用在线覆盖。后续更新请使用本项目的打包脚本。',
      autoChecks: '自动检查',
      autoCheck: {
        label: '自动检查更新',
        desc: '启动约 4 秒后在后台检查一次 GitHub Releases。已是最新或离线时不会打扰你。'
      },
      statusSection: '更新',
      check: {
        label: '检查更新',
        desc: '向 GitHub 查询是否有更新的版本。在你确认之前不会下载任何内容。'
      },
      checkButton: '检查更新',
      idle: '尚未检查。',
      checking: '正在检查更新…',
      upToDate: '已是最新版本（{version}）。',
      available: '发现新版本：{version}',
      published: '发布于 {date}',
      notesHeading: '发布说明',
      installButton: '下载并安装',
      installing: '正在下载并安装 {version}…',
      progressPct: '正在下载… {pct}%',
      progressBytes: '已下载 {bytes}',
      installed: '版本 {version} 已安装——重启后完成。',
      restartButton: '立即重启',
      errorTitle: '无法检查更新。',
      errorInstall: '无法安装更新。',
      errorHint: '请检查网络连接后重试。',
      reset: { label: '恢复更新默认设置…', confirm: '将“更新”恢复为默认设置？' }
    },
    data: {
      pane: '数据',
      storage: '存储',
      dataDir: '数据文件夹',
      settingsPath: '设置文件',
      petPath: '桌宠位置文件',
      dismissedPath: '已忽略任务文件',
      version: '版本',
      settingsVersion: '设置格式版本',
      notAvailable: '不可用',
      actions: '操作',
      openData: { label: '打开数据文件夹', desc: '在访达中显示该文件夹。', button: '打开' },
      clearDismissed: {
        label: '清除已忽略任务',
        desc: '已忽略的任务之后可以重新被采集。',
        confirm: '清除所有已忽略任务？'
      },
      resetPosition: { label: '重置桌宠位置', desc: '把桌宠移回屏幕右下角。', button: '重置' },
      danger: '恢复',
      resetAll: {
        label: '恢复所有设置…',
        desc: '桌宠位置和已忽略任务不受影响。',
        confirm: '将所有设置恢复为默认值？'
      },
      legacy: '设置已从旧版本迁移。',
      corrupt: '设置文件无法读取，备份保存在 {path}。'
    }
  }
};

var LANG = 'en';

function effectiveLang() {
  if (!state.settings) {
    return /^zh/i.test(navigator.language || '') ? 'zh' : 'en';
  }
  if (state.settings.language === 'zh') return 'zh';
  if (state.settings.language === 'en') return 'en';
  var system = (state.meta && state.meta.system_language) || navigator.language || 'en';
  return /^zh/i.test(system) ? 'zh' : 'en';
}

function i(key, vars) {
  var parts = key.split('.');
  var value = I18N[LANG] || I18N.en;
  for (var p = 0; p < parts.length; p++) value = value && value[parts[p]];
  if (typeof value !== 'string') {
    value = I18N.en;
    for (var q = 0; q < parts.length; q++) value = value && value[parts[q]];
  }
  if (typeof value !== 'string') return key;
  if (vars) {
    value = value.replace(/\{(\w+)\}/g, function (match, name) {
      return vars[name] === undefined || vars[name] === null ? '' : String(vars[name]);
    });
  }
  return value;
}

function leafKeys(object, prefix) {
  var keys = [];
  var base = prefix || '';
  for (var key in object) {
    if (!Object.prototype.hasOwnProperty.call(object, key)) continue;
    var path = base ? base + '.' + key : key;
    if (object[key] && typeof object[key] === 'object') keys = keys.concat(leafKeys(object[key], path));
    else keys.push(path);
  }
  return keys;
}

// ── DOM helpers ─────────────────────────────────────────────────

function h(tag, attrs, children) {
  var node = document.createElement(tag);
  if (attrs) {
    for (var key in attrs) {
      if (!Object.prototype.hasOwnProperty.call(attrs, key)) continue;
      var value = attrs[key];
      if (value === null || value === undefined || value === false) continue;
      if (key === 'class') node.className = value;
      else if (key === 'text') node.textContent = value;
      else if (key === 'html') node.innerHTML = value;
      else if (key.slice(0, 2) === 'on') node.addEventListener(key.slice(2), value);
      else if (value === true) node.setAttribute(key, '');
      else node.setAttribute(key, String(value));
    }
  }
  append(node, children);
  return node;
}

function append(node, children) {
  if (children === null || children === undefined) return;
  if (Array.isArray(children)) {
    for (var i = 0; i < children.length; i++) append(node, children[i]);
    return;
  }
  if (typeof children === 'string' || typeof children === 'number') {
    node.appendChild(document.createTextNode(String(children)));
    return;
  }
  node.appendChild(children);
}

function card(rows) {
  return h('div', { class: 'card' }, rows);
}

function section(captionKey, content, notes) {
  var wrap = h('section', { class: 'section' }, [
    h('div', { class: 'section-caption', text: i(captionKey) }),
    content
  ]);
  if (notes) {
    var list = Array.isArray(notes) ? notes : [notes];
    for (var n = 0; n < list.length; n++) {
      if (!list[n]) continue;
      wrap.appendChild(h('p', { class: 'note', text: list[n] }));
    }
  }
  return wrap;
}

function slug(key) {
  return String(key).replace(/[^a-z0-9]+/gi, '-');
}

var ICONS = {
  general: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"><path d="M2.5 4.3h11M2.5 8h11M2.5 11.7h11"/><circle cx="5.4" cy="4.3" r="1.6" fill="currentColor" stroke="none"/><circle cx="10.4" cy="8" r="1.6" fill="currentColor" stroke="none"/><circle cx="6.4" cy="11.7" r="1.6" fill="currentColor" stroke="none"/></svg>',
  attention: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M3.4 11.1h9.2c-.9-1-1.3-2.2-1.3-3.9V6a3.3 3.3 0 0 0-6.6 0v1.2c0 1.7-.4 2.9-1.3 3.9Z"/><path d="M6.6 13.1a1.5 1.5 0 0 0 2.8 0"/><path d="M8 1.6V2.6"/></svg>',
  monitor: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><circle cx="8" cy="8" r="5.4"/><path d="M8 4.7V8l2.3 1.5"/></svg>',
  pet: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round"><rect x="2.6" y="3.1" width="10.8" height="9.8" rx="2.2"/><circle cx="8" cy="8" r="1.7" fill="currentColor" stroke="none"/></svg>',
  jump: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M4.4 11.6 11.6 4.4M6.2 4.4h5.4v5.4"/></svg>',
  update: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M8 2.4v6.9"/><path d="M5.3 6.8 8 9.5l2.7-2.7"/><path d="M2.9 12.6h10.2"/></svg>',
  data: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"><rect x="2.2" y="4" width="11.6" height="8" rx="1.7"/><path d="M4.4 9.7h1.7"/><circle cx="11.1" cy="9.7" r=".9" fill="currentColor" stroke="none"/></svg>'
};

// ── State ───────────────────────────────────────────────────────

var state = {
  settings: null,
  meta: null,
  group: 'general',
  confirm: null,       // { kind: 'reset'|'clear-dismissed', scope }
  status: null,        // { kind: 'saved'|'error'|'info', text }
  focusAfterRender: null,
  bootError: null,
  icons: { collapsed: null, hidden: null },
  iconMeta: {},
  petPacks: [],               // list_pet_packs result, normalized
  diagnostics: null,          // get_jump_diagnostics result, normalized
  diagnosticsLoaded: false,   // one successful read
  diagnosticsLoading: false,
  diagnosticsRequested: false, // lazy: the jump pane was opened at least once
  diagnosticsError: null,
  guideOpen: false,           // collapsible setup guide (remembered across renders)
  jumpAction: null,           // 'request' | 'reset' while a permission command runs
  update: {
    phase: 'idle',            // idle|checking|uptodate|available|installing|installed|error
    result: null,             // { current_version, available, version, notes, pub_date }
    progress: null,           // { downloaded, total }
    error: null,
    errorKind: null,          // 'check' | 'install'
    installedVersion: null,
    checking: false
  },
  pendingUpdatePane: false    // open-update-pane arrived before boot finished
};

var saveQueue = { inFlight: false, pending: null };
var statusTimer = null;

var sidebarEl = document.getElementById('sidebar');
var paneTitleEl = document.getElementById('pane-title');
var paneBodyEl = document.getElementById('pane-body');
var paneScrollEl = document.getElementById('pane-scroll');
var saveHintEl = document.getElementById('save-hint');
var saveStatusEl = document.getElementById('save-status');

// ── Save pipeline (instant save, full object, no polling) ───────

function save(next) {
  state.settings = normalizeSettings(next);
  renderAll();
  // One write in flight at a time; a burst of changes (stepper, arrow keys)
  // coalesces into the latest value instead of racing out of order.
  saveQueue.pending = clone(state.settings);
  if (!saveQueue.inFlight) flushSaveQueue();
}

function flushSaveQueue() {
  var payload = saveQueue.pending;
  if (!payload) {
    saveQueue.inFlight = false;
    return;
  }
  saveQueue.pending = null;
  saveQueue.inFlight = true;
  invoke('set_settings', { settings: payload }).then(function (normalized) {
    saveQueue.inFlight = false;
    if (!saveQueue.pending) {
      applySettings(normalized);
      flashStatus('saved', i('status.saved'), 1600);
    }
    if (saveQueue.pending) flushSaveQueue();
  }).catch(function (err) {
    saveQueue.inFlight = false;
    saveQueue.pending = null;
    flashStatus('error', i('status.saveFailed'), 5000);
    invoke('get_settings').then(applySettings).catch(function () {});
    if (window.console) console.error('set_settings failed', err);
  });
}

function commitTop(key, value) {
  var next = clone(state.settings);
  next[key] = value;
  save(next);
}

function commitAgent(agentKey, value) {
  var next = clone(state.settings);
  next.enabled_agents = next.enabled_agents || {};
  next.enabled_agents[agentKey] = value;
  save(next);
}

function applySettings(next) {
  if (!next) return false;
  var normalized = normalizeSettings(next);
  var changed = !state.settings || JSON.stringify(normalized) !== JSON.stringify(state.settings);
  state.settings = normalized;
  if (changed) renderAll();
  return changed;
}

function flashStatus(kind, text, ms) {
  state.status = { kind: kind, text: text };
  renderSaveBar();
  if (statusTimer) { clearTimeout(statusTimer); statusTimer = null; }
  if (ms) {
    statusTimer = setTimeout(function () {
      statusTimer = null;
      state.status = null;
      renderSaveBar();
    }, ms);
  }
}

function renderSaveBar() {
  saveHintEl.textContent = i('saveHint');
  if (!state.status) {
    saveStatusEl.hidden = true;
    saveStatusEl.className = '';
    saveStatusEl.textContent = '';
    return;
  }
  saveStatusEl.hidden = false;
  saveStatusEl.className = state.status.kind;
  saveStatusEl.textContent = state.status.text;
}

// ── Confirmations (inline, keyboard cancellable) ────────────────

function openConfirm(kind, scope) {
  state.confirm = { kind: kind, scope: scope || null };
  state.focusAfterRender = 'confirm-cancel';
  renderAll();
}

function cancelConfirm() {
  var confirm = state.confirm;
  state.confirm = null;
  if (confirm && confirm.kind === 'reset') {
    state.focusAfterRender = confirm.scope === 'all' ? 'reset-all' : 'reset:' + confirm.scope;
  } else if (confirm && confirm.kind === 'clear-dismissed') {
    state.focusAfterRender = 'clear-dismissed';
  } else if (confirm && confirm.kind === 'reset-permissions') {
    state.focusAfterRender = 'reset-permissions';
  }
  renderAll();
}

function doReset(scope) {
  state.confirm = null;
  invoke('reset_settings', { scope: scope }).then(function (normalized) {
    state.focusAfterRender = scope === 'all' ? 'reset-all' : 'reset:' + scope;
    if (!applySettings(normalized)) renderAll();
    flashStatus('saved', i('status.reset'), 1800);
  }).catch(function (err) {
    flashStatus('error', i('status.saveFailed'), 5000);
    if (window.console) console.error('reset_settings failed', err);
  });
}

function button(cls, label, opts) {
  opts = opts || {};
  return h('button', {
    type: 'button',
    class: 'btn ' + (cls || ''),
    text: label,
    disabled: opts.disabled ? true : null,
    'data-focus': opts.focus || null,
    'data-action': opts.action || null,
    'aria-label': opts.ariaLabel || null,
    title: opts.title || null,
    onclick: opts.onclick
  });
}

// ── Row builders ────────────────────────────────────────────────

function switchRow(opts) {
  var id = 'switch-' + slug(opts.key);
  var descId = id + '-desc';
  var desc = opts.descKey ? i(opts.descKey, opts.descVars) : null;
  var input = h('input', {
    type: 'checkbox',
    class: 'switch',
    role: 'switch',
    id: id,
    'data-setting': opts.key,
    'data-focus': 'switch:' + opts.key,
    'aria-label': i(opts.titleKey),
    'aria-describedby': desc ? descId : null,
    onchange: function () {
      if (opts.agent) commitAgent(opts.agent, input.checked);
      else commitTop(opts.key, input.checked);
    }
  });
  input.checked = !!opts.checked;
  return h('label', { class: 'row row-switch', 'data-row': opts.key }, [
    h('span', { class: 'row-text' }, [
      h('span', { class: 'row-title', text: i(opts.titleKey) }),
      desc ? h('span', { class: 'row-desc', id: descId, text: desc }) : null
    ]),
    h('span', { class: 'control' }, [input])
  ]);
}

function segRow(opts) {
  var name = 'seg-' + opts.key;
  var group = h('div', {
    class: 'segmented',
    role: 'radiogroup',
    'aria-label': i(opts.titleKey),
    'data-seg': opts.key
  });
  for (var index = 0; index < opts.options.length; index++) {
    var option = opts.options[index];
    (function (option) {
      var input = h('input', {
        type: 'radio',
        name: name,
        value: option.value,
        'data-setting': opts.key,
        'data-value': option.value,
        'data-focus': 'seg:' + opts.key + ':' + option.value,
        'aria-label': i(opts.titleKey) + ': ' + (option.label || i(option.labelKey)),
        onchange: function () {
          if (!input.checked) return;
          if (opts.agent) commitAgent(opts.agent, option.value);
          else commitTop(opts.key, option.value);
        }
      });
      input.checked = option.value === opts.value;
      group.appendChild(h('label', { class: 'seg' }, [
        input,
        h('span', { class: 'seg-label', text: option.label || i(option.labelKey) })
      ]));
    })(option);
  }
  return h('div', { class: 'row' }, [
    h('span', { class: 'row-text' }, [h('span', { class: 'row-title', text: i(opts.titleKey) })]),
    h('span', { class: 'control' }, [group])
  ]);
}

function formatValue(opts, value) {
  // Sliders whose zero value means "disabled" render a word instead of "0 s".
  if (opts.zeroKey && Number(value) === 0) return i(opts.zeroKey);
  return value + ' ' + opts.unit;
}

function setSliderFill(input) {
  var min = Number(input.min);
  var max = Number(input.max);
  var value = Number(input.value);
  var pct = max === min ? 0 : ((value - min) / (max - min)) * 100;
  input.style.setProperty('--pct', pct.toFixed(2) + '%');
}

function sliderRow(opts) {
  var input = h('input', {
    type: 'range',
    class: 'slider',
    min: opts.min,
    max: opts.max,
    step: opts.step,
    'data-setting': opts.key,
    'data-focus': 'slider:' + opts.key,
    'aria-label': i(opts.titleKey),
    oninput: function () {
      var value = Number(input.value);
      output.textContent = formatValue(opts, value);
      input.setAttribute('aria-valuetext', formatValue(opts, value));
      setSliderFill(input);
    },
    onchange: function () {
      commitTop(opts.key, Number(input.value));
    }
  });
  input.value = String(opts.value);
  input.setAttribute('aria-valuetext', formatValue(opts, opts.value));
  setSliderFill(input);
  var output = h('output', {
    class: 'slider-value',
    'data-output-for': opts.key,
    text: formatValue(opts, opts.value)
  });
  return h('div', { class: 'row', 'data-row': opts.key }, [
    h('span', { class: 'row-text' }, [
      h('span', { class: 'row-title', text: i(opts.titleKey) }),
      opts.descKey ? h('span', { class: 'row-desc', text: i(opts.descKey) }) : null
    ]),
    h('span', { class: 'control' }, [input, output])
  ]);
}

function stepperRow(opts) {
  function nudge(delta) {
    var current = Number(state.settings[opts.key]);
    var next = clamp(current + delta, opts.min, opts.max);
    if (next === current) return;
    commitTop(opts.key, next);
  }
  function stepButton(delta, glyph, focusKey) {
    return h('button', {
      type: 'button',
      class: 'stepper-btn',
      'data-setting': opts.key,
      'data-step': String(delta),
      'data-focus': focusKey,
      'aria-label': i(delta > 0 ? 'common.increase' : 'common.decrease') + ' ' + i(opts.titleKey),
      text: glyph,
      onclick: function () { nudge(delta); }
    });
  }
  return h('div', { class: 'row', 'data-row': opts.key }, [
    h('span', { class: 'row-text' }, [
      h('span', { class: 'row-title', text: i(opts.titleKey) }),
      opts.descKey ? h('span', { class: 'row-desc', text: i(opts.descKey) }) : null
    ]),
    h('span', { class: 'control' }, [
      h('span', { class: 'stepper', role: 'group', 'aria-label': i(opts.titleKey) }, [
        stepButton(-opts.step, '−', 'stepper-down:' + opts.key),
        h('output', {
          class: 'stepper-value',
          'data-output-for': opts.key,
          'aria-live': 'polite',
          text: formatValue(opts, opts.value)
        }),
        stepButton(opts.step, '+', 'stepper-up:' + opts.key)
      ])
    ])
  ]);
}

function infoRow(labelKey, value, rowKey) {
  return h('div', { class: 'row', 'data-row': rowKey || null }, [
    h('span', { class: 'row-text' }, [h('span', { class: 'row-title', text: i(labelKey) })]),
    h('span', { class: 'control' }, [h('span', { class: 'info-value', text: value })])
  ]);
}

function actionRow(titleKey, descKey, controls) {
  return h('div', { class: 'row' }, [
    h('span', { class: 'row-text' }, [
      h('span', { class: 'row-title', text: i(titleKey) }),
      descKey ? h('span', { class: 'row-desc', text: i(descKey) }) : null
    ]),
    h('span', { class: 'control' }, controls)
  ]);
}

function confirmRow(opts) {
  return h('div', { class: 'row row-confirm' }, [
    h('span', { class: 'row-text' }, [h('span', { class: 'row-title', text: opts.prompt })]),
    h('span', { class: 'control confirm-actions' }, [
      button('plain', i('common.cancel'), { focus: 'confirm-cancel', onclick: cancelConfirm }),
      button('danger', opts.okLabel, { focus: 'confirm-ok', onclick: opts.onOk })
    ])
  ]);
}

function confirmBar(opts) {
  return h('div', { class: 'confirm-bar' }, [
    h('span', { class: 'confirm-q', text: opts.prompt }),
    h('span', { class: 'confirm-actions' }, [
      button('plain', i('common.cancel'), { focus: 'confirm-cancel', onclick: cancelConfirm }),
      button('danger', opts.okLabel, { focus: 'confirm-ok', onclick: opts.onOk })
    ])
  ]);
}

function pathRow(labelKey, path) {
  var display = path ? String(path).replace(/^\/Users\/[^/]+/, '~') : i('data.notAvailable');
  return h('div', { class: 'row row-stacked', 'data-row': 'path-' + slug(labelKey) }, [
    h('span', { class: 'row-text' }, [h('span', { class: 'row-title', text: i(labelKey) })]),
    h('code', {
      class: 'path-value',
      tabindex: '0',
      'aria-label': i(labelKey),
      title: path || '',
      text: display
    })
  ]);
}

function resetFooter(scope, labelKey) {
  var confirm = state.confirm;
  if (confirm && confirm.kind === 'reset' && confirm.scope === scope) {
    return confirmBar({
      prompt: i(labelKey + '.confirm'),
      okLabel: i('common.reset'),
      onOk: function () { doReset(scope); }
    });
  }
  return button('plain danger', i(labelKey + '.label'), {
    action: 'reset-' + scope,
    focus: 'reset:' + scope,
    onclick: function () { openConfirm('reset', scope); }
  });
}

// ── Custom icons (Desktop Pet → Icon) ──────────────────────────

function normalizeIcons(raw) {
  var out = { collapsed: null, hidden: null };
  if (!raw || typeof raw !== 'object') return out;
  ['collapsed', 'hidden'].forEach(function (slot) {
    var value = raw[slot];
    if (typeof value === 'string' && /^data:image\/(?:png|jpeg|webp);base64,/.test(value)) {
      out[slot] = value;
    }
  });
  return out;
}

function applyIcons(next) {
  var icons = normalizeIcons(next);
  var changed = JSON.stringify(icons) !== JSON.stringify(state.icons || {});
  state.icons = icons;
  if (changed) renderAll();
  return changed;
}

function loadCustomIcons() {
  return invoke('get_custom_icons').then(function (icons) {
    applyIcons(icons);
  }).catch(function (err) {
    // Backend unavailable (older build / window closing): keep what we show.
    if (window.console) console.warn('get_custom_icons unavailable', err);
  });
}

// ── Mascot packs (Rust pet engine, docs/PETS.md) ────────────────

function applyPetPacks(packs) {
  var next = Array.isArray(packs) ? packs : [];
  var changed = JSON.stringify(next) !== JSON.stringify(state.petPacks || []);
  state.petPacks = next;
  if (changed) renderAll();
  return changed;
}

var petPackPreviewUrls = {};

function loadPetPacks() {
  return invoke('list_pet_packs').then(function (packs) {
    return Promise.all((packs || []).map(function (pack) {
      if (petPackPreviewUrls[pack.name] || !pack.frame_size || !pack.frame_size[0]) return;
      return invoke('get_pet_pack_image', {
        pack: pack.name, image: pack.kind === 'state' ? 'idle' : '/shime1.png'
      }).then(function (url) {
        petPackPreviewUrls[pack.name] = url;
      }).catch(function () { /* Metadata remains usable without a preview. */ });
    })).then(function () { applyPetPacks(packs); });
  }).catch(function (err) {
    if (window.console) console.warn('list_pet_packs unavailable', err);
  });
}

function readPackFileAsBase64(file) {
  return new Promise(function (resolve, reject) {
    var reader = new FileReader();
    reader.onload = function () { resolve(String(reader.result || '').split(',')[1] || ''); };
    reader.onerror = function () { reject(new Error('read failed: ' + file.name)); };
    reader.readAsDataURL(file);
  });
}

function importPetPackFolder(input) {
  var files = Array.prototype.slice.call(input.files || []).filter(function (file) {
    var path = file.webkitRelativePath || file.name;
    if (/__MACOSX|\/\./.test(path)) return false;
    return /conf\/[^/]+\.xml$/i.test(path) || /img\/[^/]+\.png$/i.test(path);
  });
  if (!files.length) {
    flashStatus('error', i('pet.packImportError'), 5000);
    return;
  }
  var packName = (files[0].webkitRelativePath || files[0].name).split('/')[0];
  flashStatus('info', i('pet.packImporting'), 12000);
  Promise.all(files.map(function (file) {
    return readPackFileAsBase64(file).then(function (data) {
      return { path: file.webkitRelativePath || file.name, data_base64: data };
    });
  })).then(function (payload) {
    return invoke('import_pet_pack', { name: packName, files: payload });
  }).then(function () {
    flashStatus('saved', i('pet.packImported'), 3000);
    return loadPetPacks();
  }).catch(function (err) {
    flashStatus('error', i('pet.packImportError') + ': ' + (err && err.message ? err.message : err), 8000);
  });
}

function buildMascotSection() {
  var settings = state.settings;
  var packs = state.petPacks || [];
  var cards = [segRow({
    key: 'pet_kind',
    titleKey: 'pet.kind.label',
    value: settings.pet_kind,
    options: [
      { value: 'classic', labelKey: 'pet.kindClassic' },
      { value: 'mascot', labelKey: 'pet.kindMascot' }
    ]
  })];
  if (settings.pet_kind === 'mascot') {
    // Pack picker: vertical radio list (names are long; a horizontal
    // segmented row overflows with more than a couple of packs).
    var usable = packs.filter(function (pack) {
      return pack.frame_size && pack.frame_size[0] > 0;
    });
    usable.forEach(function (pack) {
      var bits = [];
      if (pack.builtin) bits.push(i('pet.packBuiltin'));
      if (pack.author) bits.push(pack.author);
      if (pack.license) bits.push(pack.license);
      cards.push(h('label', { class: 'row pack-row' }, [
        h('input', {
          type: 'radio',
          name: 'mascot-pack',
          value: pack.name,
          'aria-label': pack.display_name || pack.name,
          checked: settings.mascot_pack === pack.name,
          'data-setting': 'mascot_pack',
          'data-value': pack.name,
          onchange: function () {
            if (this.checked) commitTop('mascot_pack', pack.name);
          }
        }),
        h('img', { class: 'pack-preview', src: petPackPreviewUrls[pack.name] || '', alt: '', hidden: !petPackPreviewUrls[pack.name] }),
        h('span', { class: 'row-text' }, [
          h('span', { class: 'row-title', text: pack.display_name || pack.name }),
          h('span', { class: 'row-desc', text: bits.join(' · ') || pack.name })
        ])
      ]));
    });
    if (!usable.some(function (pack) { return pack.name === settings.mascot_pack; })) {
      cards.push(h('div', { class: 'row' }, [
        h('span', { class: 'row-text' }, [
          h('span', { class: 'row-title', text: settings.mascot_pack + ' — ' + i('pet.packMissing') })
        ])
      ]));
    }
    var broken = packs.filter(function (pack) {
      return !pack.frame_size || pack.frame_size[0] === 0;
    });
    broken.forEach(function (pack) {
      cards.push(h('div', { class: 'row' }, [
        h('span', { class: 'row-text' }, [
          h('span', { class: 'row-title', text: pack.name + ' — ' + i('pet.packBroken') }),
          h('span', { class: 'row-desc', text: (pack.warnings || []).join('；') })
        ])
      ]));
    });
    // Import: same visually-hidden input pattern as the icon picker.
    var importInput = h('input', {
      type: 'file',
      class: 'icon-file',
      id: 'pet-pack-import',
      webkitdirectory: '',
      multiple: '',
      onchange: function () { importPetPackFolder(importInput); importInput.value = ''; }
    });
    cards.push(h('div', { class: 'row' }, [
      h('span', { class: 'row-text' }, [
        h('span', { class: 'row-title', text: i('pet.packNote') })
      ]),
      h('span', { class: 'control' }, [
        importInput,
        h('label', { class: 'btn icon-choose', for: 'pet-pack-import', text: i('pet.packImport') })
      ])
    ]));
  }
  return section('pet.kindSection', card(cards), [i('pet.kindNote')]);
}

function dataUrlBytes(dataUrl) {
  if (typeof dataUrl !== 'string') return 0;
  var comma = dataUrl.indexOf(',');
  if (comma === -1) return 0;
  var payload = dataUrl.slice(comma + 1).replace(/=+$/, '');
  return Math.floor(payload.length * 3 / 4);
}

function mimeFromDataUrl(dataUrl) {
  var match = /^data:(image\/(?:png|jpeg|webp));base64,/.exec(String(dataUrl || ''));
  return match ? match[1] : null;
}

function formatBytes(bytes) {
  if (!bytes) return '';
  if (bytes < 1024) return bytes + ' B';
  var kb = bytes / 1024;
  return (kb < 10 ? kb.toFixed(1) : String(Math.round(kb))) + ' KB';
}

function iconSizeLabel(slot) {
  var dataUrl = state.icons[slot];
  if (!dataUrl) return null;
  var meta = state.iconMeta[slot];
  if (!meta || meta.url !== dataUrl) meta = {};
  var mime = meta.mime || mimeFromDataUrl(dataUrl);
  var bytes = meta.bytes || dataUrlBytes(dataUrl);
  var kind = ICON_MIME_KIND[mime] || '';
  var size = formatBytes(bytes);
  return kind && size ? kind + ' · ' + size : (size || kind);
}

function flashIconError(err, fallbackKey) {
  var message = '';
  if (err && typeof err === 'string') message = err;
  else if (err && err.message) message = String(err.message);
  flashStatus('error', message || i(fallbackKey), 5000);
}

function handleIconFile(slot, input) {
  var file = input.files && input.files[0];
  try { input.value = ''; } catch (e) { /* allow re-picking the same file */ }
  if (!file) return;
  // The client only accepts the documented formats; Rust sniffs magic bytes too.
  if (!ICON_MIME_KIND[file.type]) {
    flashStatus('error', i('pet.iconError.mime'), 5000);
    return;
  }
  if (file.size > ICON_MAX_BYTES) {
    flashStatus('error', i('pet.iconError.tooLarge'), 5000);
    return;
  }
  var reader = new FileReader();
  reader.onload = function () {
    var dataUrl = String(reader.result || '');
    // Decoded-size estimate from the base64 payload (data URLs are ~4/3 raw).
    if (!mimeFromDataUrl(dataUrl) || dataUrlBytes(dataUrl) > ICON_MAX_BYTES) {
      flashStatus('error', i(dataUrlBytes(dataUrl) > ICON_MAX_BYTES ? 'pet.iconError.tooLarge' : 'pet.iconError.mime'), 5000);
      return;
    }
    saveIcon(slot, dataUrl);
  };
  reader.onerror = function () {
    flashStatus('error', i('pet.iconError.read'), 5000);
  };
  reader.readAsDataURL(file);
}

function saveIcon(slot, dataUrl) {
  invoke('save_custom_icon', { slot: slot, dataUrl: dataUrl }).then(function (meta) {
    meta = meta || {};
    state.iconMeta[slot] = {
      url: dataUrl,
      mime: meta.mime || mimeFromDataUrl(dataUrl),
      bytes: Number(meta.bytes) || dataUrlBytes(dataUrl),
      // Rust serialises `updated_at`; accept camelCase too.
      updatedAt: meta.updatedAt !== undefined ? meta.updatedAt : meta.updated_at
    };
    return loadCustomIcons().then(function () {
      flashStatus('saved', i('pet.iconSaved'), 2000);
    });
  }).catch(function (err) {
    flashIconError(err, 'pet.iconError.save');
  });
}

function removeIcon(slot) {
  invoke('clear_custom_icon', { slot: slot }).then(function () {
    delete state.iconMeta[slot];
    return loadCustomIcons().then(function () {
      flashStatus('info', i('pet.iconRemoved'), 2000);
    });
  }).catch(function (err) {
    flashIconError(err, 'pet.iconError.remove');
  });
}

function iconSlotRow(slot) {
  var titleKey = slot === 'collapsed' ? 'pet.iconCollapsed.label' : 'pet.iconHidden.label';
  var descKey = slot === 'collapsed' ? 'pet.iconCollapsed.desc' : 'pet.iconHidden.desc';
  var dataUrl = state.icons[slot];
  var inputId = 'icon-file-' + slot;
  var fileInput = h('input', {
    type: 'file',
    class: 'icon-file',
    id: inputId,
    accept: ICON_ACCEPT,
    'data-slot': slot,
    'data-focus': 'icon-file:' + slot,
    'aria-label': i(titleKey) + ': ' + i('pet.iconChoose'),
    onchange: function () { handleIconFile(slot, fileInput); }
  });
  var actions = [
    fileInput,
    h('label', { class: 'btn icon-choose', for: inputId, text: dataUrl ? i('pet.iconReplace') : i('pet.iconChoose') })
  ];
  if (dataUrl) {
    actions.push(button('plain danger', i('pet.iconRemove'), {
      action: 'icon-remove-' + slot,
      focus: 'icon-remove:' + slot,
      ariaLabel: i('pet.iconRemove') + ' ' + i(titleKey),
      title: i('pet.iconRemove') + ' ' + i(titleKey),
      onclick: function () { removeIcon(slot); }
    }));
  }
  var preview = h('div', {
    class: 'icon-preview',
    'data-icon-preview': slot,
    'aria-hidden': 'true'
  }, dataUrl
    ? [h('img', { class: 'icon-preview-img', src: dataUrl, alt: '' })]
    : [h('span', { class: 'icon-glyph' }, [h('i'), h('i'), h('i')])]);

  return h('div', { class: 'icon-row', 'data-icon-row': slot }, [
    preview,
    h('div', { class: 'icon-info' }, [
      h('span', { class: 'icon-head' }, [
        h('span', { class: 'row-title', text: i(titleKey) }),
        h('span', { class: 'row-desc icon-size', 'data-icon-size': slot,
          text: iconSizeLabel(slot) || i('pet.iconBuiltinGlyph') })
      ]),
      h('span', { class: 'icon-desc', text: i(descKey) }),
      h('span', { class: 'icon-actions' }, actions)
    ])
  ]);
}

function buildIconSection() {
  var settings = state.settings;
  var styleCard = card([
    segRow({
      key: 'icon_style',
      titleKey: 'pet.iconStyle.label',
      value: settings.icon_style,
      options: [
        { value: 'builtin', labelKey: 'pet.iconBuiltin' },
        { value: 'custom', labelKey: 'pet.iconCustom' }
      ]
    })
  ]);
  var slotsCard = card([iconSlotRow('collapsed'), iconSlotRow('hidden')]);
  var notes = [i('pet.iconStyleNote'), i('pet.iconSpec')];
  if (settings.icon_style === 'custom' && !state.icons.collapsed && !state.icons.hidden) {
    notes.push(i('pet.iconNoCustomNote'));
  }
  return section('pet.iconSection', h('div', { class: 'card-stack' }, [styleCard, slotsCard]), notes);
}

// ── Jump diagnostics (get_jump_diagnostics) ────────────────────
//
// The Jump pane reads the three macOS permission states and the per-agent
// capability table once, the first time it is opened, then refreshes:
//   • on “Check again”,
//   • whenever the Settings window regains focus/visibility while the pane is
//     selected (so a System Settings toggle is reflected without extra clicks),
//   • and after request/reset permission commands finish.
// It is never polled.

function pick(raw, snake, camel, fallback) {
  if (raw && raw[snake] !== undefined) return raw[snake];
  if (raw && raw[camel] !== undefined) return raw[camel];
  return fallback;
}

/* Accepts the documented snake_case payload and camelCase spellings, so an
 * older/newer Rust side can never blank the pane. */
function normalizeDiagnostics(raw) {
  if (!raw || typeof raw !== 'object') return null;
  function perm(key, camel) {
    var entry = pick(raw, key, camel, null);
    if (entry && typeof entry === 'object') {
      return { granted: !!entry.granted, detail: entry.detail === undefined ? null : entry.detail };
    }
    return { granted: !!entry, detail: null };
  }
  var agents = raw.agents;
  return {
    accessibility: perm('accessibility', 'accessibility'),
    automation: perm('automation', 'automation'),
    window_access: perm('window_access', 'windowAccess'),
    system_events: !!pick(raw, 'system_events', 'systemEvents', false),
    app_bundle_path: pick(raw, 'app_bundle_path', 'appBundlePath', null) || null,
    agents: Array.isArray(agents) ? agents.map(function (agent) {
      var level = String(pick(agent, 'max_level', 'maxLevel', 'L0') || 'L0');
      // Rust serialises JumpLevel as "L3"; accept a lower-case spelling too.
      if (/^l[0-3]$/i.test(level)) level = level.toUpperCase();
      return {
        key: agent.key || '',
        name: agent.name || agent.key || '',
        bundle_id: pick(agent, 'bundle_id', 'bundleId', null),
        app_name: pick(agent, 'app_name', 'appName', null),
        max_level: level,
        notes: agent.notes || '',
        installed: !!agent.installed,
        running: !!agent.running,
        deep_link: !!pick(agent, 'deep_link', 'deepLink', false)
      };
    }) : []
  };
}

// Unknown detail codes fall back to a localised generic message; the raw code is
// kept as the element's title for support tickets (never silently dropped).
var DIAGNOSTIC_DETAIL_KEYS = {
  needs_accessibility: 'needsAccessibility',
  needs_automation: 'needsAutomation',
  osascript_failed: 'osascriptFailed',
  diagnostics_failed: 'diagnosticsFailed',
  no_running_agent_app: 'noAgentApp',
  no_process: 'noAgentApp',
  window_state_unavailable: 'windowStateUnavailable',
  terminal_unavailable: 'windowStateUnavailable'
};

// “Could not verify” rather than “denied”: grey pill, still explains itself.
var UNVERIFIED_DETAILS = {
  diagnostics_failed: 1,
  no_running_agent_app: 1,
  no_process: 1,
  window_state_unavailable: 1,
  terminal_unavailable: 1
};

function diagnosticDetail(code) {
  var key = DIAGNOSTIC_DETAIL_KEYS[code];
  return key ? i('jump.status.detail.' + key) : i('jump.status.detail.other');
}

function jumpErrorText(err) {
  return String(err && err.message ? err.message : err);
}

/* A permission command is running: every permission button is disabled. */
function jumpActionBusy() {
  return !!state.jumpAction;
}

function jumpCheckDisabled() {
  return state.jumpAction !== null || state.diagnosticsLoading;
}

/* Accessibility + Automation grants that are missing (window access is a
 * read test, not a grant). Drives the collapsed guide's call-to-action. */
function missingPermissionCount() {
  var diag = state.diagnostics;
  if (!diag) return 0;
  var count = 0;
  if (!(diag.accessibility && diag.accessibility.granted)) count += 1;
  if (!(diag.automation && diag.automation.granted)) count += 1;
  return count;
}

/* One-line “granted but still not authorised” explanation, only while
 * Accessibility is missing. */
function staleGrantAlert() {
  var diag = state.diagnostics;
  if (!diag || state.diagnosticsLoading) return null;
  if (diag.accessibility && diag.accessibility.granted) return null;
  return h('p', {
    class: 'stale-alert',
    'data-stale-alert': '',
    role: 'note',
    text: i('jump.staleGrant')
  });
}

function shortPath(path) {
  var text = String(path || '');
  return text ? text.replace(/^\/Users\/[^/]+/, '~') : '—';
}

/* Read-only “App location” row: the running bundle, so the user can verify the
 * copy listed in System Settings is exactly this build. */
function appLocationRow() {
  var path = state.diagnostics && state.diagnostics.app_bundle_path;
  return h('div', { class: 'row row-stacked', 'data-row': 'jump-app-location' }, [
    h('span', { class: 'row-text' }, [
      h('span', { class: 'row-title', text: i('jump.appLocation') }),
      h('span', { class: 'row-desc', text: i('jump.appLocationDesc') })
    ]),
    h('code', {
      class: 'path-value',
      tabindex: '0',
      'aria-label': i('jump.appLocation'),
      title: path || '',
      text: shortPath(path)
    })
  ]);
}

/* macOS prompt: add this build to the Accessibility list. Never resets TCC. */
function requestAccessibilityPermission() {
  if (state.jumpAction) return;
  state.jumpAction = 'request';
  renderAll();
  invoke('request_accessibility_permission').then(function (granted) {
    state.jumpAction = null;
    renderAll();
    flashStatus('info',
      i(granted ? 'jump.actions.requestTrusted' : 'jump.actions.requestHint'),
      granted ? 3600 : 5200);
    loadJumpDiagnostics(true);
  }).catch(function (err) {
    state.jumpAction = null;
    renderAll();
    flashStatus('error', jumpErrorText(err), 5000);
  });
}

function openResetPermissionsConfirm() {
  if (state.jumpAction) return;
  state.confirm = { kind: 'reset-permissions', scope: null };
  state.focusAfterRender = 'confirm-cancel';
  renderAll();
}

/* tccutil-based reset of this bundle's grants (only after inline confirmation). */
function resetThisAppPermissions() {
  state.confirm = null;
  if (state.jumpAction) return;
  state.jumpAction = 'reset';
  state.focusAfterRender = 'reset-permissions';
  renderAll();
  invoke('reset_app_permissions').then(function () {
    state.jumpAction = null;
    renderAll();
    flashStatus('info', i('jump.actions.resetHint'), 6000);
    loadJumpDiagnostics(true);
  }).catch(function (err) {
    state.jumpAction = null;
    renderAll();
    flashStatus('error', jumpErrorText(err), 5000);
  });
}

/* Coming back from System Settings should refresh the pills immediately. */
function refreshJumpOnReturn() {
  if (document.hidden) return;
  if (state.group !== 'jump') return;
  if (state.jumpAction) return;               // the command re-checks on finish
  if (!state.diagnosticsRequested) return;    // the lazy first load owns this
  loadJumpDiagnostics(true);
}

/* A re-render can race the native <details> toggle task; read the live DOM
 * state first so the guide never collapses under the user. */
function syncGuideState() {
  var guide = document.querySelector('details.guide[data-guide]');
  if (guide) state.guideOpen = guide.open;
}

/* Only the first open triggers a read; "Check again" passes force=true. */
function loadJumpDiagnostics(force) {
  if (state.diagnosticsLoading) return;
  if (!force && state.diagnosticsLoaded) return;
  state.diagnosticsLoading = true;
  state.diagnosticsError = null;
  renderAll();
  invoke('get_jump_diagnostics').then(function (data) {
    state.diagnosticsLoading = false;
    state.diagnostics = normalizeDiagnostics(data);
    state.diagnosticsLoaded = !!state.diagnostics;
    renderAll();
  }).catch(function (err) {
    state.diagnosticsLoading = false;
    state.diagnosticsError = String(err && err.message ? err.message : err);
    renderAll();
  });
}

function ensureJumpDiagnostics() {
  if (state.diagnosticsRequested || state.bootError || !state.settings) return;
  state.diagnosticsRequested = true;
  // Deferred one microtask so the pane paints its “Checking…” state first.
  Promise.resolve().then(function () { loadJumpDiagnostics(); });
}

function permissionState(key) {
  if (state.diagnosticsLoading || !state.diagnostics) {
    return { cls: 'unknown', text: i('jump.status.checking'), granted: null };
  }
  var entry = state.diagnostics[key] || {};
  return {
    cls: entry.granted ? 'ok' : (UNVERIFIED_DETAILS[entry.detail] ? 'unknown' : 'warn'),
    text: entry.granted ? i('jump.status.granted') : i('jump.status.notGranted'),
    granted: !!entry.granted
  };
}

function diagnosticHint(row) {
  var diag = state.diagnostics;
  if (!diag) return null;
  var entry = diag[row.key] || {};
  if (!entry.granted) {
    if (entry.detail) return { text: diagnosticDetail(entry.detail), raw: String(entry.detail) };
    if (row.key === 'accessibility') return { text: i('jump.status.hint.accessibility'), raw: null };
    if (row.key === 'automation') return { text: i('jump.status.hint.automation'), raw: null };
    return { text: i('jump.status.hint.windowAccess'), raw: null };
  }
  if (row.key === 'automation' && diag.system_events) {
    return { text: i('jump.status.systemEventsOn'), raw: null };
  }
  return null;
}

function diagnosticRow(row) {
  var status = permissionState(row.key);
  var hint = diagnosticHint(row);
  var info = h('div', { class: 'status-info' }, [
    h('div', { class: 'status-head' }, [
      h('span', { class: 'row-title', text: i(row.titleKey) }),
      h('span', {
        class: 'status-pill ' + status.cls,
        'data-pill': row.key,
        'data-pill-state': status.cls,
        role: 'status',
        text: status.text
      })
    ]),
    h('span', { class: 'row-desc', text: i(row.descKey) }),
    hint ? h('span', {
      class: 'status-detail',
      'data-detail': row.key,
      title: hint.raw && !DIAGNOSTIC_DETAIL_KEYS[hint.raw] ? hint.raw : null,
      text: hint.text
    }) : null
  ]);
  var children = [info];
  if (row.permission) {
    (function (kind) {
      children.push(button('', i('jump.actions.open'), {
        action: 'open-permission-' + kind,
        focus: 'open-permission:' + kind,
        ariaLabel: i(kind === 'accessibility' ? 'jump.actions.openAccessibility' : 'jump.actions.openAutomation'),
        title: i(kind === 'accessibility' ? 'jump.actions.openAccessibility' : 'jump.actions.openAutomation'),
        disabled: jumpActionBusy(),
        onclick: function () {
          invoke('open_permission_settings', { kind: kind }).catch(function (err) {
            flashStatus('error', jumpErrorText(err), 5000);
          });
        }
      }));
    })(row.permission);
  }
  return h('div', { class: 'row status-row', 'data-status-row': row.key }, children);
}

var DIAGNOSTIC_ROWS = [
  { key: 'accessibility', titleKey: 'jump.status.accessibility', descKey: 'jump.status.accessibilityDesc', permission: 'accessibility' },
  { key: 'automation', titleKey: 'jump.status.automation', descKey: 'jump.status.automationDesc', permission: 'automation' },
  { key: 'window_access', titleKey: 'jump.status.windowAccess', descKey: 'jump.status.windowAccessDesc', permission: null }
];

function buildJumpActions() {
  var busy = jumpActionBusy();
  var rows = [];

  rows.push(actionRow('jump.actions.checkLabel', 'jump.actions.checkDesc', [
    button('', i('jump.actions.checkButton'), {
      action: 'jump-check',
      focus: 'jump-check',
      disabled: jumpCheckDisabled(),
      onclick: function () { loadJumpDiagnostics(true); }
    })
  ]));

  rows.push(actionRow('jump.actions.requestLabel', 'jump.actions.requestDesc', [
    button('primary', i('jump.actions.requestButton'), {
      action: 'request-accessibility',
      focus: 'request-accessibility',
      disabled: jumpCheckDisabled(),
      onclick: requestAccessibilityPermission
    })
  ]));

  rows.push(actionRow('jump.actions.revealLabel', 'jump.actions.revealDesc', [
    button('', i('jump.actions.revealButton'), {
      action: 'reveal-app',
      focus: 'reveal-app',
      disabled: busy,
      onclick: function () {
        invoke('reveal_app_in_finder').catch(function (err) {
          flashStatus('error', jumpErrorText(err), 5000);
        });
      }
    })
  ]));

  if (state.confirm && state.confirm.kind === 'reset-permissions') {
    rows.push(confirmRow({
      prompt: i('jump.actions.resetConfirm'),
      okLabel: i('jump.actions.resetConfirmOk'),
      onOk: resetThisAppPermissions
    }));
  } else {
    rows.push(actionRow('jump.actions.resetLabel', 'jump.actions.resetDesc', [
      button('danger', i('jump.actions.resetButton'), {
        action: 'reset-permissions',
        focus: 'reset-permissions',
        ariaLabel: i('jump.actions.resetLabel'),
        title: i('jump.actions.resetLabel'),
        disabled: jumpCheckDisabled(),
        onclick: openResetPermissionsConfirm
      })
    ]));
  }

  return card(rows);
}

function buildJumpSetup() {
  var frag = document.createDocumentFragment();
  var stack = [];

  stack.push(card(DIAGNOSTIC_ROWS.map(function (row) { return diagnosticRow(row); })));

  // “Checked in System Settings but still not granted”: read it right after the
  // pill it explains. Disappears as soon as the grant is seen.
  var alert = staleGrantAlert();
  if (alert) stack.push(alert);

  stack.push(card([appLocationRow()]));
  stack.push(buildJumpActions());

  var notes = [];
  if (state.diagnosticsError) {
    notes.push(i('jump.status.failed', { message: state.diagnosticsError }));
  }
  frag.appendChild(section('jump.setup', h('div', { class: 'card-stack' }, stack), notes.length ? notes : null));
  return frag;
}

/* Collapsible setup guide: a native <details> so it stays keyboard accessible
 * and takes no vertical space until the user opens it. The open state is kept
 * across re-renders (diagnostics refresh, language switch). */
function buildJumpGuide() {
  var missing = missingPermissionCount();
  var summaryText = missing > 0
    ? i('jump.guide.summaryMissing', { count: missing })
    : i('jump.guide.summary');
  var stepKeys = ['jump.guide.step1', 'jump.guide.step2', 'jump.guide.step3'];
  var steps = h('ol', { class: 'steps' }, stepKeys.map(function (key, index) {
    return h('li', { class: 'step', 'data-step': String(index + 1) }, [
      h('span', { class: 'step-text', text: i(key) })
    ]);
  }));
  var path = state.diagnostics && state.diagnostics.app_bundle_path;

  var body = h('div', { class: 'guide-body' }, [
    h('p', { class: 'guide-intro', text: i('jump.guide.intro') }),
    steps,
    h('p', { class: 'guide-note', text: i('jump.guide.adhoc') }),
    h('p', { class: 'guide-note guide-note-warn', text: i('jump.staleGrant') }),
    path ? h('p', { class: 'guide-note guide-path', text: i('jump.guide.appPath', { path: shortPath(path) }) }) : null
  ]);

  var details = h('details', {
    class: 'guide',
    'data-guide': '',
    open: state.guideOpen ? true : null
  }, [
    h('summary', { class: 'guide-summary', 'data-guide-summary': '' }, [
      h('span', { class: 'guide-chevron', 'aria-hidden': 'true' }),
      h('span', {
        class: 'guide-summary-text',
        'data-guide-title': '',
        'data-guide-missing': missing > 0 ? 'true' : 'false',
        text: summaryText
      }),
      h('span', { class: 'guide-summary-hint', text: i('jump.guide.summaryHint') })
    ]),
    body
  ]);
  details.addEventListener('toggle', function () {
    state.guideOpen = details.open;
  });

  return section('jump.guide.heading', details);
}

function agentRow(agent) {
  var name = agent.name || agent.key || '—';
  var level = agent.max_level || 'L0';
  var stateText = agent.installed ? i('jump.agents.installed') : i('jump.agents.notInstalled');
  if (agent.installed) {
    stateText += ' · ' + (agent.running ? i('jump.agents.running') : i('jump.agents.notRunning'));
  }
  return h('div', { class: 'agent-row', 'data-agent': agent.key || slug(name) }, [
    h('div', { class: 'agent-head' }, [
      h('span', { class: 'row-title agent-name', text: name }),
      h('span', {
        class: 'agent-level',
        'data-level': level,
        'aria-label': i('jump.agents.heading') + ': ' + name + ' ' + level,
        text: level
      })
    ]),
    h('span', { class: 'agent-state', 'data-agent-state': agent.installed ? (agent.running ? 'installed-running' : 'installed') : 'not-installed', text: stateText }),
    agent.notes ? h('span', { class: 'agent-notes', text: agent.notes }) : null,
    h('span', { class: 'agent-flags' }, [
      h('span', {
        class: 'agent-flag' + (agent.deep_link ? ' on' : ''),
        'data-deep-link': agent.deep_link ? 'yes' : 'no',
        text: agent.deep_link ? i('jump.agents.deepLink') : i('jump.agents.noDeepLink')
      })
    ])
  ]);
}

function buildJumpAgents() {
  var agents = state.diagnostics && state.diagnostics.agents;
  var content;
  if (Array.isArray(agents) && agents.length) {
    content = card(agents.map(agentRow));
  } else {
    content = card([h('div', { class: 'row row-stacked' }, [
      h('span', { class: 'row-desc', text: i('jump.agents.noDiagnostics') })
    ])]);
  }
  return section('jump.agents.heading', content, i('jump.agents.desc'));
}

// ── In-app updates (docs/UPDATING.md) ──────────────────────────
//
// The Updates pane is a small state machine (idle → checking → available →
// installing → installed, or error). Nothing is polled: the “Check for
// Updates” button and the tray `open-update-pane` event are the only triggers,
// and progress comes from `update-progress` events. A manual check returns its
// result; `update-available` is only the startup auto-check.

var RELEASES_URL = 'https://github.com/Hoemr/agent-monitor/releases';

function updateVersionText() {
  var meta = state.meta || {};
  return meta.app_version ? String(meta.app_version) : '—';
}

function normalizeUpdateResult(raw) {
  raw = raw || {};
  return {
    current_version: raw.current_version || raw.currentVersion || '',
    available: !!raw.available,
    version: raw.version || '',
    notes: raw.notes || '',
    pub_date: raw.pub_date || raw.pubDate || ''
  };
}

/* latest.json carries an RFC 3339 timestamp; show just the calendar date. */
function formatUpdateDate(value) {
  var text = String(value || '');
  var match = /^(\d{4}-\d{2}-\d{2})/.exec(text);
  return match ? match[1] : text;
}

/* During boot there is no settings object yet; the boot render paints the
 * phase stored in the meantime. Away from the pane we only store the state so
 * a burst of progress events cannot re-render an unrelated pane. */
function renderUpdateView() {
  if (!state.settings || state.group !== 'update') return;
  renderAll();
}

function applyUpdateResult(raw) {
  var result = normalizeUpdateResult(raw);
  state.update.result = result;
  state.update.error = null;
  state.update.errorKind = null;
  state.update.checking = false;
  state.update.phase = result.available ? 'available' : 'uptodate';
  renderUpdateView();
}

/* Startup auto-check → `update-available`. Paint it; never install. */
function applyUpdateAvailable(raw) {
  if (!raw || state.update.checking) return;
  if (state.update.phase === 'installing' || state.update.phase === 'installed') return;
  applyUpdateResult(raw);
}

function applyUpdateProgress(raw) {
  if (!raw || state.update.phase === 'installed') return;
  var downloaded = Number(raw.downloaded);
  if (!isFinite(downloaded) || downloaded < 0) downloaded = 0;
  var total = raw.total === null || raw.total === undefined ? null : Number(raw.total);
  if (total !== null && (!isFinite(total) || total <= 0)) total = null;
  state.update.checking = false;
  state.update.phase = 'installing';
  state.update.progress = { downloaded: downloaded, total: total };
  renderUpdateView();
}

function applyUpdateInstalled(raw) {
  var result = state.update.result || {};
  state.update.checking = false;
  state.update.phase = 'installed';
  state.update.installedVersion = (raw && raw.version) || result.version || '';
  state.update.progress = { downloaded: 1, total: 1 };
  renderUpdateView();
}

function checkForUpdate() {
  if (state.meta && state.meta.local_build) return;
  // Idempotent: a click and the tray event must never start two checks.
  if (state.update.checking || state.update.phase === 'installing') return;
  state.update.checking = true;
  state.update.phase = 'checking';
  state.update.error = null;
  state.update.errorKind = null;
  renderUpdateView();
  invoke('check_for_update').then(function (payload) {
    applyUpdateResult(payload);
  }).catch(function (err) {
    state.update.checking = false;
    state.update.phase = 'error';
    state.update.errorKind = 'check';
    state.update.error = String(err && err.message ? err.message : err);
    renderUpdateView();
  });
}

function installUpdate() {
  if (state.update.phase !== 'available') return;
  state.update.phase = 'installing';
  state.update.progress = { downloaded: 0, total: null };
  state.update.error = null;
  state.update.errorKind = null;
  renderUpdateView();
  invoke('install_update').then(function () {
    // Normal path: `update-installed` already moved us on. This fallback keeps
    // the user from being stuck at 100% if the event was lost.
    if (state.update.phase === 'installing') {
      state.update.phase = 'installed';
      state.update.installedVersion = (state.update.result && state.update.result.version) || '';
      state.update.progress = { downloaded: 1, total: 1 };
      renderUpdateView();
    }
  }).catch(function (err) {
    state.update.phase = 'error';
    state.update.errorKind = 'install';
    state.update.error = String(err && err.message ? err.message : err);
    renderUpdateView();
  });
}

function restartApp() {
  invoke('restart_app').catch(function (err) {
    flashStatus('error', String(err && err.message ? err.message : err), 5000);
  });
}

/* Tray “Check for Updates…” → Settings on the Updates pane plus one check. */
function openUpdatePane() {
  if (!state.settings) {
    state.pendingUpdatePane = true;
    return;
  }
  state.pendingUpdatePane = false;
  if (state.group !== 'update') selectGroup('update');
  checkForUpdate();
}

function updateProgressInfo(progress) {
  var downloaded = progress ? progress.downloaded : 0;
  var total = progress ? progress.total : null;
  if (total && total > 0) {
    var pct = Math.max(0, Math.min(100, Math.round((downloaded / total) * 100)));
    return { pct: pct, text: i('update.progressPct', { pct: pct }) };
  }
  var bytes = formatBytes(downloaded);
  return { pct: null, text: i('update.progressBytes', { bytes: bytes || '0 B' }) };
}

function updateCheckButton() {
  var u = state.update;
  var busy = u.checking || u.phase === 'checking' || u.phase === 'installing';
  return button('', i('update.checkButton'), {
    action: 'update-check',
    focus: 'update-check',
    disabled: busy,
    onclick: function () { checkForUpdate(); }
  });
}

function updatePanel() {
  var u = state.update;
  var phase = u.phase || 'idle';
  var children = [];

  if (phase === 'available') {
    var version = (u.result && u.result.version) || '';
    children.push(h('div', { class: 'update-head' }, [
      h('span', {
        class: 'update-title',
        role: 'status',
        'aria-live': 'polite',
        'data-update-title': '',
        text: i('update.available', { version: version })
      }),
      h('span', { class: 'control update-actions' }, [
        button('', i('update.installButton'), {
          action: 'update-install',
          focus: 'update-install',
          onclick: function () { installUpdate(); }
        })
      ])
    ]));
    if (u.result && u.result.pub_date) {
      children.push(h('span', {
        class: 'update-meta',
        'data-update-meta': '',
        text: i('update.published', { date: formatUpdateDate(u.result.pub_date) })
      }));
    }
    if (u.result && u.result.notes) {
      children.push(h('div', {
        class: 'update-notes',
        'data-update-notes': '',
        tabindex: '0',
        'aria-label': i('update.notesHeading'),
        text: u.result.notes
      }));
    }
  } else if (phase === 'installing') {
    var target = (u.result && u.result.version) || '';
    var info = updateProgressInfo(u.progress);
    var barAttrs = { class: 'update-progress', 'data-update-progress': '' };
    if (info.pct !== null) {
      barAttrs.role = 'progressbar';
      barAttrs['aria-valuemin'] = '0';
      barAttrs['aria-valuemax'] = '100';
      barAttrs['aria-valuenow'] = String(info.pct);
    } else {
      barAttrs['data-indeterminate'] = 'true';
    }
    children.push(h('span', {
      class: 'update-title',
      role: 'status',
      'aria-live': 'polite',
      'data-update-title': '',
      text: i('update.installing', { version: target })
    }));
    children.push(h('div', barAttrs, [
      h('div', {
        class: 'update-progress-fill',
        style: 'width:' + (info.pct === null ? 35 : info.pct) + '%'
      })
    ]));
    children.push(h('span', {
      class: 'update-progress-text',
      'data-update-progress-text': '',
      text: info.text
    }));
  } else if (phase === 'installed') {
    var installed = u.installedVersion || (u.result && u.result.version) || '';
    children.push(h('div', { class: 'update-head' }, [
      h('span', {
        class: 'update-title',
        role: 'status',
        'aria-live': 'polite',
        'data-update-title': '',
        text: i('update.installed', { version: installed })
      }),
      h('span', { class: 'control update-actions' }, [
        button('', i('update.restartButton'), {
          action: 'update-restart',
          focus: 'update-restart',
          onclick: restartApp
        })
      ])
    ]));
  } else if (phase === 'checking') {
    children.push(h('span', {
      class: 'update-title',
      role: 'status',
      'aria-live': 'polite',
      'data-update-title': '',
      text: i('update.checking')
    }));
  } else if (phase === 'uptodate') {
    var current = (u.result && u.result.current_version) || updateVersionText();
    children.push(h('span', {
      class: 'update-title',
      role: 'status',
      'aria-live': 'polite',
      'data-update-title': '',
      text: i('update.upToDate', { version: current })
    }));
  } else if (phase === 'error') {
    children.push(h('span', {
      class: 'update-title update-error-title',
      role: 'status',
      'aria-live': 'polite',
      'data-update-title': '',
      text: i(u.errorKind === 'install' ? 'update.errorInstall' : 'update.errorTitle')
    }));
    if (u.error) {
      children.push(h('span', { class: 'update-error', 'data-update-error': '', text: u.error }));
    }
    children.push(h('span', { class: 'update-meta', text: i('update.errorHint') }));
  } else {
    children.push(h('span', {
      class: 'update-title',
      role: 'status',
      'aria-live': 'polite',
      'data-update-title': '',
      text: i('update.idle')
    }));
  }

  return h('div', { class: 'row row-stacked update-panel', 'data-update-phase': phase }, children);
}

function buildUpdate() {
  var settings = state.settings;
  var frag = document.createDocumentFragment();

  frag.appendChild(section('update.versionSection', card([
    infoRow('update.currentVersion', updateVersionText(), 'update-version'),
    pathRow('update.releases', RELEASES_URL)
  ]), i('update.releasesNote')));

  if (state.meta && state.meta.local_build) {
    frag.appendChild(section('update.localBuild', card([
      h('div', { class: 'row row-stacked', text: i('update.localBuildNote') })
    ])));
    return frag;
  }

  frag.appendChild(section('update.autoChecks', card([
    switchRow({
      key: 'auto_check_updates',
      titleKey: 'update.autoCheck.label',
      descKey: 'update.autoCheck.desc',
      checked: settings.auto_check_updates
    })
  ])));

  frag.appendChild(section('update.statusSection', card([
    actionRow('update.check.label', 'update.check.desc', [updateCheckButton()]),
    updatePanel()
  ])));

  frag.appendChild(h('div', { class: 'pane-actions' }, [resetFooter('update', 'update.reset')]));
  return frag;
}

// ── Pane builders ──────────────────────────────────────────────

function buildGeneral() {
  var settings = state.settings;
  var frag = document.createDocumentFragment();
  frag.appendChild(section('general.appearanceHeading', card([
    segRow({
      key: 'language',
      titleKey: 'general.language.label',
      value: settings.language,
      options: [
        { value: 'system', labelKey: 'general.langSystem' },
        { value: 'en', labelKey: 'general.langEn' },
        { value: 'zh', labelKey: 'general.langZh' }
      ]
    }),
    segRow({
      key: 'appearance',
      titleKey: 'general.appearance.label',
      value: settings.appearance,
      options: [
        { value: 'system', labelKey: 'general.appSystem' },
        { value: 'light', labelKey: 'general.appLight' },
        { value: 'dark', labelKey: 'general.appDark' }
      ]
    })
  ]), i('general.appearanceNote')));

  frag.appendChild(section('general.displayHeading', card([
    switchRow({
      key: 'show_summary',
      titleKey: 'general.showSummary.label',
      descKey: 'general.showSummary.desc',
      checked: settings.show_summary
    }),
    switchRow({
      key: 'show_jump_level',
      titleKey: 'general.showJump.label',
      descKey: 'general.showJump.desc',
      checked: settings.show_jump_level
    }),
    switchRow({
      key: 'new_badge',
      titleKey: 'general.newBadge.label',
      descKey: 'general.newBadge.desc',
      checked: settings.new_badge
    })
  ])));

  frag.appendChild(h('div', { class: 'pane-actions' }, [resetFooter('general', 'general.reset')]));
  return frag;
}

function buildAttention() {
  var settings = state.settings;
  var frag = document.createDocumentFragment();
  frag.appendChild(section('attention.alerts', card([
    switchRow({
      key: 'notify_on_question',
      titleKey: 'attention.notify.label',
      descKey: 'attention.notify.desc',
      checked: settings.notify_on_question
    }),
    switchRow({
      key: 'alert_sound',
      titleKey: 'attention.sound.label',
      descKey: 'attention.sound.desc',
      checked: settings.alert_sound
    }),
    switchRow({
      key: 'auto_show_on_question',
      titleKey: 'attention.reveal.label',
      descKey: 'attention.reveal.desc',
      checked: settings.auto_show_on_question
    })
  ]), i('attention.note')));

  frag.appendChild(h('div', { class: 'pane-actions' }, [resetFooter('attention', 'attention.reset')]));
  return frag;
}

function buildMonitor() {
  var settings = state.settings;
  var frag = document.createDocumentFragment();
  frag.appendChild(section('monitor.collection', card([
    segRow({
      key: 'refresh_interval',
      titleKey: 'monitor.refresh.label',
      value: settings.refresh_interval,
      options: [
        { value: 'auto', labelKey: 'monitor.refreshAuto' },
        { value: '3s', labelKey: 'monitor.refresh3' },
        { value: '5s', labelKey: 'monitor.refresh5' },
        { value: '10s', labelKey: 'monitor.refresh10' }
      ]
    }),
    segRow({
      key: 'retention_hours',
      titleKey: 'monitor.retention.label',
      value: settings.retention_hours,
      options: [
        { value: 1, labelKey: 'monitor.retention1' },
        { value: 6, labelKey: 'monitor.retention6' },
        { value: 24, labelKey: 'monitor.retention24' },
        { value: 72, labelKey: 'monitor.retention72' },
        { value: 168, labelKey: 'monitor.retention168' },
        { value: 720, labelKey: 'monitor.retention720' }
      ]
    })
  ]), [i('monitor.refreshNote'), i('monitor.retentionNote')]));

  frag.appendChild(section('monitor.agents', card([
    switchRow({ key: 'enabled_agents.codex', agent: 'codex', titleKey: 'monitor.codex.label', descKey: 'monitor.codex.desc', checked: settings.enabled_agents.codex }),
    switchRow({ key: 'enabled_agents.antigravity', agent: 'antigravity', titleKey: 'monitor.antigravity.label', descKey: 'monitor.antigravity.desc', checked: settings.enabled_agents.antigravity }),
    switchRow({ key: 'enabled_agents.dsh', agent: 'dsh', titleKey: 'monitor.dsh.label', descKey: 'monitor.dsh.desc', checked: settings.enabled_agents.dsh }),
    switchRow({ key: 'enabled_agents.zcode', agent: 'zcode', titleKey: 'monitor.zcode.label', descKey: 'monitor.zcode.desc', checked: settings.enabled_agents.zcode }),
    switchRow({ key: 'enabled_agents.claude_code', agent: 'claude_code', titleKey: 'monitor.claude.label', descKey: 'monitor.claude.desc', checked: settings.enabled_agents.claude_code })
  ]), i('monitor.agentNote')));

  frag.appendChild(section('monitor.window', card([
    segRow({
      key: 'max_visible_rows',
      titleKey: 'monitor.rows.label',
      value: settings.max_visible_rows,
      options: [
        { value: 3, labelKey: 'monitor.rows3' },
        { value: 5, labelKey: 'monitor.rows5' },
        { value: 8, labelKey: 'monitor.rows8' }
      ]
    })
  ]), i('monitor.rowsNote')));

  frag.appendChild(h('div', { class: 'pane-actions' }, [resetFooter('monitor', 'monitor.reset')]));
  return frag;
}

function buildPet() {
  var settings = state.settings;
  var frag = document.createDocumentFragment();
  frag.appendChild(buildIconSection());
  frag.appendChild(buildMascotSection());
  frag.appendChild(section('pet.gestures', card([
    sliderRow(Object.assign({ key: 'hover_expand_ms', titleKey: 'pet.hover.label', descKey: 'pet.hover.desc', value: settings.hover_expand_ms, unit: 'ms' }, RANGES.hover_expand_ms)),
    sliderRow(Object.assign({ key: 'collapse_delay_ms', titleKey: 'pet.collapse.label', descKey: 'pet.collapse.desc', value: settings.collapse_delay_ms, unit: 'ms' }, RANGES.collapse_delay_ms))
  ])));

  frag.appendChild(section('pet.edges', card([
    stepperRow(Object.assign({ key: 'snap_distance', titleKey: 'pet.snap.label', descKey: 'pet.snap.desc', value: settings.snap_distance, unit: 'px' }, RANGES.snap_distance)),
    stepperRow(Object.assign({ key: 'hidden_sliver', titleKey: 'pet.sliver.label', descKey: 'pet.sliver.desc', value: settings.hidden_sliver, unit: 'px' }, RANGES.hidden_sliver)),
    sliderRow(Object.assign({ key: 'collapsed_size', titleKey: 'pet.size.label', descKey: 'pet.size.desc', value: settings.collapsed_size, unit: 'px' }, RANGES.collapsed_size)),
    switchRow({
      key: 'edge_hide_enabled',
      titleKey: 'pet.edgeHide.label',
      descKey: settings.edge_hide_enabled ? 'pet.edgeHide.descOn' : 'pet.edgeHide.descOff',
      checked: settings.edge_hide_enabled
    })
  ])));

  frag.appendChild(section('pet.autoHideSection', card([
    sliderRow(Object.assign({ key: 'auto_hide_after_s', titleKey: 'pet.autoHide.label', descKey: 'pet.autoHide.desc', value: settings.auto_hide_after_s, unit: 's', zeroKey: 'pet.autoHideOff' }, RANGES.auto_hide_after_s))
  ])));

  frag.appendChild(section('pet.motion', card([
    segRow({
      key: 'animations',
      titleKey: 'pet.animations.label',
      value: settings.animations,
      options: [
        { value: 'system', labelKey: 'pet.animSystem' },
        { value: 'always', labelKey: 'pet.animAlways' },
        { value: 'off', labelKey: 'pet.animOff' }
      ]
    })
  ]), i('pet.animationsNote')));

  frag.appendChild(h('div', { class: 'pane-actions' }, [resetFooter('pet', 'pet.reset')]));
  return frag;
}

function buildJump() {
  var settings = state.settings;
  var frag = document.createDocumentFragment();
  frag.appendChild(section('jump.feedback', card([
    switchRow({
      key: 'toast_on_degraded',
      titleKey: 'jump.toast.label',
      descKey: 'jump.toast.desc',
      checked: settings.toast_on_degraded
    }),
    switchRow({
      key: 'auto_dismiss_invalid',
      titleKey: 'jump.autoDismiss.label',
      descKey: 'jump.autoDismiss.desc',
      checked: settings.auto_dismiss_invalid
    })
  ])));

  frag.appendChild(buildJumpSetup());
  frag.appendChild(buildJumpGuide());
  frag.appendChild(buildJumpAgents());

  frag.appendChild(h('div', { class: 'pane-actions' }, [resetFooter('jump', 'jump.reset')]));
  return frag;
}

function buildData() {
  var settings = state.settings;
  var meta = state.meta || {};
  var frag = document.createDocumentFragment();

  var notes = [];
  if (meta.legacy_imported) notes.push(i('data.legacy'));
  if (meta.corrupt_backup) notes.push(i('data.corrupt', { path: meta.corrupt_backup }));

  frag.appendChild(section('data.storage', card([
    pathRow('data.dataDir', meta.data_dir),
    pathRow('data.settingsPath', meta.settings_path),
    pathRow('data.petPath', meta.pet_path),
    pathRow('data.dismissedPath', meta.dismissed_path),
    infoRow('data.version', meta.app_version || i('data.notAvailable')),
    infoRow('data.settingsVersion', meta.version === undefined ? i('data.notAvailable') : String(meta.version))
  ]), notes.length ? notes : null));

  var actions = [];
  actions.push(actionRow('data.openData.label', 'data.openData.desc', [
    button('', i('data.openData.button'), {
      action: 'open-data-dir',
      focus: 'open-data-dir',
      onclick: function () {
        invoke('open_data_dir').catch(function (err) {
          flashStatus('error', String(err && err.message ? err.message : err), 5000);
        });
      }
    })
  ]));

  var clearConfirm = state.confirm && state.confirm.kind === 'clear-dismissed';
  if (clearConfirm) {
    actions.push(confirmRow({
      prompt: i('data.clearDismissed.confirm'),
      okLabel: i('common.clear'),
      onOk: function () {
        state.confirm = null;
        invoke('clear_dismissed').then(function (count) {
          var n = Number(count);
          if (isNaN(n)) n = 0;
          if (MOCK) mockState.dismissed = 0;
          state.focusAfterRender = 'clear-dismissed';
          renderAll();
          flashStatus('info', i('status.dismissedCleared', { n: n }), 2400);
        }).catch(function (err) {
          flashStatus('error', String(err && err.message ? err.message : err), 5000);
        });
      }
    }));
  } else {
    actions.push(actionRow('data.clearDismissed.label', 'data.clearDismissed.desc', [
      button('danger', i('common.clear'), {
        action: 'clear-dismissed',
        focus: 'clear-dismissed',
        onclick: function () { openConfirm('clear-dismissed', null); }
      })
    ]));
  }

  actions.push(actionRow('data.resetPosition.label', 'data.resetPosition.desc', [
    button('', i('data.resetPosition.button'), {
      action: 'reset-position',
      focus: 'reset-position',
      onclick: function () {
        invoke('reset_pet_position_command').then(function () {
          flashStatus('info', i('status.positionReset'), 2000);
        }).catch(function (err) {
          flashStatus('error', String(err && err.message ? err.message : err), 5000);
        });
      }
    })
  ]));
  frag.appendChild(section('data.actions', card(actions)));

  var resetAllOpen = state.confirm && state.confirm.kind === 'reset' && state.confirm.scope === 'all';
  var resetAll = resetAllOpen
    ? confirmRow({
      prompt: i('data.resetAll.confirm'),
      okLabel: i('common.reset'),
      onOk: function () { doReset('all'); }
    })
    : actionRow('data.resetAll.label', 'data.resetAll.desc', [
      button('plain danger', i('common.reset'), {
        action: 'reset-all',
        focus: 'reset-all',
        onclick: function () { openConfirm('reset', 'all'); }
      })
    ]);
  frag.appendChild(section('data.danger', card([resetAll])));

  return frag;
}

var BUILDERS = {
  general: buildGeneral,
  attention: buildAttention,
  monitor: buildMonitor,
  pet: buildPet,
  jump: buildJump,
  update: buildUpdate,
  data: buildData
};

// ── Rendering ───────────────────────────────────────────────────

function captureFocus() {
  var active = document.activeElement;
  if (!active || !active.getAttribute) return null;
  return active.getAttribute('data-focus');
}

function restoreFocus(key) {
  if (!key) return;
  var node = document.querySelector('[data-focus="' + key + '"]');
  if (!node) return;
  try { node.focus({ preventScroll: true }); } catch (e) {
    try { node.focus(); } catch (e2) { /* focus is best-effort */ }
  }
}

function renderSidebar() {
  sidebarEl.setAttribute('aria-label', i('nav.label'));
  var frag = document.createDocumentFragment();
  for (var g = 0; g < GROUPS.length; g++) {
    var group = GROUPS[g];
    var selected = state.group === group;
    frag.appendChild(h('button', {
      type: 'button',
      class: 'nav-item',
      role: 'tab',
      id: 'tab-' + group,
      'data-group': group,
      'data-focus': 'nav:' + group,
      'aria-selected': selected ? 'true' : 'false',
      'aria-controls': 'content',
      tabindex: selected ? '0' : '-1',
      title: i('nav.' + group),
      onclick: function (event) {
        selectGroup(event.currentTarget.getAttribute('data-group'));
      }
    }, [
      h('span', { class: 'nav-icon', html: ICONS[group], 'aria-hidden': 'true' }),
      h('span', { class: 'nav-label', text: i('nav.' + group) })
    ]));
  }
  sidebarEl.textContent = '';
  sidebarEl.appendChild(frag);
}

function renderPane() {
  paneTitleEl.textContent = state.bootError ? i('window.title') : i('pane.' + state.group);
  paneBodyEl.textContent = '';
  if (state.bootError) {
    paneBodyEl.appendChild(h('div', { class: 'error-state' }, [
      h('div', { class: 'error-title', text: i('error.title') }),
      h('div', { class: 'error-hint', text: i('error.hint') }),
      h('div', { class: 'error-hint', text: state.bootError })
    ]));
    return;
  }
  if (!state.settings) {
    paneBodyEl.appendChild(h('div', { class: 'loading', text: i('loading') }));
    return;
  }
  paneBodyEl.appendChild(BUILDERS[state.group]());
  if (state.group === 'jump') ensureJumpDiagnostics();
}

function renderAll() {
  LANG = effectiveLang();
  syncGuideState();
  document.documentElement.lang = LANG;
  document.documentElement.setAttribute('lang', LANG);
  document.documentElement.setAttribute('data-lang', LANG);
  document.documentElement.setAttribute('data-appearance',
    (state.settings && state.settings.appearance) || 'system');
  document.documentElement.setAttribute('data-animations',
    (state.settings && state.settings.animations) || 'system');
  document.title = i('window.title');

  var focusKey = captureFocus();
  var scrollTop = paneScrollEl.scrollTop;

  renderSidebar();
  renderPane();
  renderSaveBar();

  paneScrollEl.scrollTop = scrollTop;
  var target = state.focusAfterRender || focusKey;
  state.focusAfterRender = null;
  restoreFocus(target);
}

function selectGroup(group) {
  if (GROUPS.indexOf(group) === -1) return;
  if (state.group === group && !state.confirm) return;
  state.group = group;
  state.confirm = null;
  try { localStorage.setItem(GROUP_KEY, group); } catch (e) { /* private mode */ }
  renderAll();
}

// ── Mock fixture (browser layout work only) ────────────────────

function mockInitialSettings() {
  var settings = defaultSettings();
  var appearance = queryParam('appearance');
  if (appearance === 'light' || appearance === 'dark') settings.appearance = appearance;
  var language = queryParam('lang');
  if (language === 'en' || language === 'zh') settings.language = language;
  var icon = queryParam('icon');
  if (icon === 'custom' || icon === 'both') settings.icon_style = 'custom';
  return settings;
}

function mockInitialIcons() {
  var icon = queryParam('icon');
  if (icon === 'custom' || icon === 'both') {
    return { collapsed: SAMPLE_ICON, hidden: icon === 'both' ? SAMPLE_ICON : null };
  }
  return { collapsed: null, hidden: null };
}

function mockMeta() {
  var dir = '/Users/you/Library/Application Support/agent-monitor';
  return {
    data_dir: dir,
    settings_path: dir + '/settings.json',
    pet_path: dir + '/pet.json',
    dismissed_path: dir + '/dismissed.json',
    version: 1,
    app_version: '0.7.0',
    system_language: 'en',
    legacy_imported: false,
    corrupt_backup: null
  };
}

/* Browser-mock updater fixtures (never used in the packaged app). */
var MOCK_UPDATE_NOTES = 'What’s new in 0.7.1\n\n' +
  '• Settings → Updates pane: in-app download and install with a live progress bar.\n' +
  '• Check for Updates from the menu bar; the pane opens automatically and checks once.\n' +
  '• Jump self-check now reports per-agent session routes.\n\n' +
  'Fixes\n' +
  '• Restart is only offered after the new bundle is installed.';

function mockUpdateResult() {
  return {
    current_version: (mockState.meta && mockState.meta.app_version) || '0.7.0',
    available: true,
    version: '0.7.1',
    notes: MOCK_UPDATE_NOTES,
    pub_date: '2026-10-21'
  };
}

/* Fixture for `get_jump_diagnostics` (documented shape, docs/JUMP.md).
 * Deliberately mixed: Accessibility missing, Automation granted, window reads
 * blocked by the accessibility grant — exactly the most common real state, so
 * the preview/selftest exercises every pill class and every hint. */
function mockDiagnostics() {
  return {
    accessibility: { granted: false, detail: null },
    automation: { granted: true, detail: null },
    window_access: { granted: false, detail: 'needs_accessibility' },
    system_events: true,
    app_bundle_path: '/Users/you/Documents/AgentMonitor/dist/Agent Monitor.app',
    agents: [
      {
        key: 'codex', name: 'Codex',
        bundle_id: 'com.openai.codex', app_name: 'ChatGPT',
        max_level: 'L3',
        notes: 'codex://threads/<id> deep link; verified by a window-title change.',
        installed: true, running: true, deep_link: true
      },
      {
        key: 'dsh', name: 'DSH',
        bundle_id: 'com.deepseek.dsh', app_name: 'DeepSeek Harness',
        max_level: 'L3',
        notes: 'Sidebar session rows are selected with a verified macOS AX click (Accessibility required); dsh://open only focuses the app.',
        installed: true, running: true, deep_link: true
      },
      {
        key: 'zcode', name: 'ZCode',
        bundle_id: 'dev.zcode.app', app_name: 'ZCode',
        max_level: 'L1',
        notes: 'The app exits immediately on this host; only app activation is declared.',
        installed: false, running: false, deep_link: false
      },
      {
        key: 'claude-code', name: 'Claude Code',
        bundle_id: 'com.microsoft.VSCode', app_name: 'Visual Studio Code',
        max_level: 'L2',
        notes: 'Modelled as VS Code-hosted; raises the matching workspace window.',
        installed: true, running: false, deep_link: false
      }
    ]
  };
}

function mockInvoke(cmd, args) {
  switch (cmd) {
    case 'get_settings':
      return clone(mockState.settings);
    case 'set_settings': {
      var next = normalizeSettings(args.settings);
      mockState.settings = next;
      mockEmit('settings-changed', clone(next));
      return clone(next);
    }
    case 'reset_settings': {
      mockState.settings = resetScope(mockState.settings, args.scope);
      mockEmit('settings-changed', clone(mockState.settings));
      return clone(mockState.settings);
    }
    case 'get_settings_meta':
      return clone(mockState.meta);
    case 'get_custom_icons':
      return clone(mockState.icons);
    case 'save_custom_icon': {
      var slot = args.slot;
      if (slot !== 'collapsed' && slot !== 'hidden') throw new Error('unknown icon slot: ' + slot);
      var mime = mimeFromDataUrl(args.dataUrl);
      if (!mime) throw new Error('unsupported image format (use PNG, JPEG or WebP)');
      var bytes = dataUrlBytes(args.dataUrl);
      if (bytes > ICON_MAX_BYTES) {
        throw new Error('image is too large: ' + Math.round(bytes / 1024) + ' KB (max 2048 KB)');
      }
      mockState.icons[slot] = args.dataUrl;
      mockState.iconMeta[slot] = { mime: mime, bytes: bytes, updated_at: Math.floor(Date.now() / 1000) };
      mockEmit('icons-changed', clone(mockState.icons));
      return { slot: slot, mime: mime, bytes: bytes, updated_at: Math.floor(Date.now() / 1000) };
    }
    case 'clear_custom_icon': {
      var clearSlot = args.slot;
      if (clearSlot !== 'collapsed' && clearSlot !== 'hidden') throw new Error('unknown icon slot: ' + clearSlot);
      mockState.icons[clearSlot] = null;
      delete mockState.iconMeta[clearSlot];
      mockEmit('icons-changed', clone(mockState.icons));
      return null;
    }
    case 'list_pet_packs':
      return [
        { name: 'nailong', display_name: '奶龙', kind: 'state', builtin: true, frame_size: [512, 512], window_size: 0, warnings: [],
          menu: [{ action: 'interact', label_key: 'menuPet' }], actions: ['interact'] },
        { name: 'doraemon', display_name: '哆啦 A 梦', kind: 'state', builtin: true, frame_size: [512, 512], window_size: 0, warnings: [],
          menu: [{ action: 'interact', label_key: 'menuPet' }], actions: ['interact'] },
        { name: 'tihun', display_name: '啼魂', kind: 'state', builtin: true, frame_size: [512, 512], window_size: 0, warnings: [],
          menu: [{ action: 'interact', label_key: 'menuPet' }], actions: ['interact'] },
        { name: 'british-blue', kind: 'state', builtin: true, frame_size: [1024, 1024], window_size: 132, warnings: [],
          menu: [{ action: 'interact', label_key: 'menuPet' }], actions: ['interact'] }
      ];
    case 'get_pet_pack_image':
      if (['nailong', 'doraemon', 'tihun', 'british-blue'].indexOf(args.pack) !== -1) {
        return '../src-tauri/pets/' + args.pack + '/' + args.image + '.png';
      }
      if (args.pack !== 'agent-cat' && args.pack !== 'byte-slime') {
        throw new Error('帧 ' + args.image + ' 不在宠物包 ' + args.pack + ' 中');
      }
      return 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==';
    case 'import_pet_pack': {
      var packName = String(args.name || '').trim();
      if (!packName || !/^[\w\- ]+$/.test(packName)) throw new Error('宠物包名只能包含字母、数字、空格、- 和 _');
      mockEmit('pet-packs-changed', null);
      return { name: packName, builtin: false, frame_size: [32, 32], warnings: [] };
    }
    case 'clear_dismissed': {
      var count = mockState.dismissed;
      mockState.dismissed = 0;
      return count;
    }
    case 'open_data_dir':
    case 'reset_pet_position_command':
      return null;
    case 'get_jump_diagnostics':
      return clone(mockDiagnostics());
    case 'reveal_app_in_finder':
      return null;
    case 'open_permission_settings': {
      var kind = args.kind;
      if (kind !== 'accessibility' && kind !== 'automation') {
        throw new Error('unknown permission kind: ' + kind);
      }
      return null;
    }
    case 'request_accessibility_permission': {
      var requestMode = (mockState.permissionModes && mockState.permissionModes.request) || 'granted';
      return new Promise(function (resolve, reject) {
        setTimeout(function () {
          if (requestMode === 'error') {
            reject(new Error('accessibility prompt failed (mock)'));
            return;
          }
          // 'granted' = already trusted (true); 'prompted' = prompt shown but the
          // switch is still off (false, the common first-run case).
          resolve(requestMode !== 'prompted' && requestMode !== 'denied');
        }, 40);
      });
    }
    case 'reset_app_permissions': {
      var resetMode = (mockState.permissionModes && mockState.permissionModes.reset) || 'ok';
      return new Promise(function (resolve, reject) {
        setTimeout(function () {
          if (resetMode === 'error') {
            reject(new Error('tccutil reset failed (mock)'));
            return;
          }
          resolve('Accessibility, Automation');
        }, 40);
      });
    }
    case 'check_for_update': {
      var mode = mockState.updateCheckMode || 'available';
      var current = (mockState.meta && mockState.meta.app_version) || '0.7.0';
      // Deferred so the “Checking…” state is observable and a second trigger
      // lands while the first check is still in flight.
      return new Promise(function (resolve, reject) {
        setTimeout(function () {
          if (mode === 'error') {
            reject(new Error('Could not reach github.com (mock)'));
            return;
          }
          if (mode === 'uptodate') {
            resolve({ current_version: current, available: false });
            return;
          }
          resolve(mockUpdateResult());
        }, 40);
      });
    }
    case 'install_update':
      // Mirrors the real backend: progress events during the download, then
      // update-installed, and the command resolves once installation is done.
      return new Promise(function (resolve) {
        var steps = [30, 60, 100];
        steps.forEach(function (downloaded, index) {
          setTimeout(function () {
            mockEmit('update-progress', { downloaded: downloaded, total: 100 });
          }, 40 * (index + 1));
        });
        setTimeout(function () {
          mockEmit('update-installed', { version: '0.7.1' });
          resolve(null);
        }, 160);
      });
    case 'restart_app':
      return null;
    case 'close_settings':
      mockState.closed = true;
      return null;
    default:
      return null;
  }
}

// ── Keyboard ────────────────────────────────────────────────────

sidebarEl.addEventListener('keydown', function (event) {
  var handled = { ArrowDown: 1, ArrowUp: 1, Home: 1, End: 1 };
  if (!handled[event.key]) return;
  event.preventDefault();
  var index = GROUPS.indexOf(state.group);
  if (event.key === 'ArrowDown') index = (index + 1) % GROUPS.length;
  else if (event.key === 'ArrowUp') index = (index - 1 + GROUPS.length) % GROUPS.length;
  else if (event.key === 'Home') index = 0;
  else index = GROUPS.length - 1;
  state.focusAfterRender = 'nav:' + GROUPS[index];
  selectGroup(GROUPS[index]);
});

document.addEventListener('keydown', function (event) {
  if (event.key !== 'Escape') return;
  if (state.confirm) {
    event.preventDefault();
    event.stopPropagation();
    cancelConfirm();
    return;
  }
  event.preventDefault();
  invoke('close_settings').catch(function () { /* window is hidden by Rust */ });
});

// ── Mock-only test hooks (never active in the packaged app) ─────

if (MOCK) {
  window.__AM_SETTINGS_TEST__ = {
    get settings() { return clone(state.settings); },
    get meta() { return clone(state.meta); },
    setLocalBuild: function (value) { state.meta.local_build = value; renderPane(); },
    get group() { return state.group; },
    get confirm() { return clone(state.confirm); },
    get status() { return clone(state.status); },
    get invokeLog() { return clone(invokeLog); },
    get closed() { return mockState.closed; },
    get dismissedCount() { return mockState.dismissed; },
    get icons() { return clone(state.icons); },
    get iconMeta() { return clone(state.iconMeta); },
    get diagnostics() { return clone(state.diagnostics); },
    get diagnosticsLoading() { return state.diagnosticsLoading; },
    get diagnosticsLoaded() { return state.diagnosticsLoaded; },
    get diagnosticsRequested() { return state.diagnosticsRequested; },
    get diagnosticsError() { return state.diagnosticsError; },
    get jumpAction() { return state.jumpAction; },
    get guideOpen() { return state.guideOpen; },
    setPermissionCommandModes: function (modes) {
      modes = modes || {};
      if (modes.request) mockState.permissionModes.request = modes.request;
      if (modes.reset) mockState.permissionModes.reset = modes.reset;
    },
    get update() {
      return clone({
        phase: state.update.phase,
        result: state.update.result,
        progress: state.update.progress,
        error: state.update.error,
        errorKind: state.update.errorKind,
        installedVersion: state.update.installedVersion,
        checking: state.update.checking
      });
    },
    setUpdateCheckMode: function (mode) {
      mockState.updateCheckMode = mode || 'available';
    },
    emitUpdateAvailable: function (payload) {
      mockEmit('update-available', payload ? clone(payload) : mockUpdateResult());
    },
    emitUpdateProgress: function (payload) {
      mockEmit('update-progress', clone(payload || { downloaded: 50, total: 100 }));
    },
    emitUpdateInstalled: function (payload) {
      mockEmit('update-installed', clone(payload || { version: '0.7.1' }));
    },
    emitOpenUpdatePane: function () {
      mockEmit('open-update-pane', null);
    },
    setIcons: function (next) {
      mockState.icons = normalizeIcons(next);
      mockEmit('icons-changed', clone(mockState.icons));
    },
    groups: GROUPS.slice(),
    dictionaries: I18N,
    dictionaryKeys: function (lang) { return leafKeys(I18N[lang] || {}); },
    effectiveLang: effectiveLang,
    setSettings: function (next) {
      mockState.settings = normalizeSettings(next);
      mockEmit('settings-changed', clone(mockState.settings));
    },
    selectGroup: selectGroup,
    render: renderAll,
    lastCommand: function (cmd) {
      for (var i = invokeLog.length - 1; i >= 0; i--) {
        if (invokeLog[i].cmd === cmd) return clone(invokeLog[i]);
      }
      return null;
    }
  };
}

// ── Bootstrap ───────────────────────────────────────────────────

function loadInitialGroup() {
  var fromQuery = MOCK ? queryParam('group') : null;
  if (fromQuery && GROUPS.indexOf(fromQuery) !== -1) return fromQuery;
  try {
    var saved = localStorage.getItem(GROUP_KEY);
    if (saved && GROUPS.indexOf(saved) !== -1) return saved;
  } catch (e) { /* localStorage unavailable */ }
  return 'general';
}

function boot() {
  if (MOCK) {
    mockState.settings = mockInitialSettings();
    mockState.meta = mockMeta();
    mockState.icons = mockInitialIcons();
  }
  state.group = loadInitialGroup();
  LANG = effectiveLang();

  // Event-driven only: settings and icons are never polled.
  listen('settings-changed', function (event) {
    applySettings(event && event.payload);
  }).catch(function (err) {
    if (window.console) console.warn('settings-changed listener unavailable', err);
  });
  listen('icons-changed', function (event) {
    applyIcons(event && event.payload);
  }).catch(function (err) {
    if (window.console) console.warn('icons-changed listener unavailable', err);
  });
  listen('pet-packs-changed', function () {
    loadPetPacks();
  }).catch(function (err) {
    if (window.console) console.warn('pet-packs-changed listener unavailable', err);
  });

  // In-app updater: startup notice, install progress, installed, tray trigger.
  listen('update-available', function (event) {
    applyUpdateAvailable(event && event.payload);
  }).catch(function (err) {
    if (window.console) console.warn('update-available listener unavailable', err);
  });
  listen('update-progress', function (event) {
    applyUpdateProgress(event && event.payload);
  }).catch(function (err) {
    if (window.console) console.warn('update-progress listener unavailable', err);
  });
  listen('update-installed', function (event) {
    applyUpdateInstalled(event && event.payload);
  }).catch(function (err) {
    if (window.console) console.warn('update-installed listener unavailable', err);
  });
  listen('open-update-pane', function () {
    openUpdatePane();
  }).catch(function (err) {
    if (window.console) console.warn('open-update-pane listener unavailable', err);
  });

  Promise.all([invoke('get_settings'), invoke('get_settings_meta')]).then(function (results) {
    state.settings = normalizeSettings(results[0]);
    state.meta = results[1] || {};
    renderAll();
    loadCustomIcons();
    loadPetPacks();
    // A tray “Check for Updates…” can arrive before the first settings read.
    if (state.pendingUpdatePane) openUpdatePane();
    var selected = sidebarEl.querySelector('.nav-item[aria-selected="true"]');
    if (selected && document.hasFocus()) {
      try { selected.focus({ preventScroll: true }); } catch (e) { /* best-effort */ }
    }
  }).catch(function (err) {
    state.bootError = String(err && err.message ? err.message : err);
    document.getElementById('app').classList.add('boot-failed');
    renderAll();
  });

  // The window is created hidden and shown again later (close = hide). Re-read
  // once whenever it becomes visible again so a missed event can never leave a
  // stale pane. This is event/focus driven, never a timer.
  if (!MOCK) {
    var refresh = function () {
      if (document.hidden) return;
      Promise.all([invoke('get_settings'), invoke('get_settings_meta')]).then(function (results) {
        state.meta = results[1] || state.meta;
        applySettings(results[0]);
        loadCustomIcons();
        loadPetPacks();
      }).catch(function () { /* keep the last known settings */ });
    };
    window.addEventListener('focus', refresh);
    document.addEventListener('visibilitychange', refresh);
  }

  // Permissions can be toggled in System Settings while this window is in the
  // background: refresh the Jump pills when it comes back, but only while the
  // Jump pane is selected. Active in the mock too so the selftest can drive it.
  window.addEventListener('focus', refreshJumpOnReturn);
  document.addEventListener('visibilitychange', refreshJumpOnReturn);
}

boot();
