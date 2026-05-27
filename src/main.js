var invoke = window.__TAURI__.core.invoke;
var listen = window.__TAURI__.event.listen;

// ── Prevent browser context menu ───────────────────────────────
document.addEventListener('contextmenu', function(e){ e.preventDefault(); });

// ── Language ───────────────────────────────────────────────────
var L = { en:{ live:'LIVE', done:'DONE', refresh:'REFRESH', prefs:'SETTINGS', hide:'HIDE', quit:'QUIT',
  noSess:'NO ACTIVE SESSIONS', start:'START A CLAUDE CODE SESSION',
  waiting:'Waiting for you', finished:'Finished', working:'Working...', thinking:'Thinking...', responding:'Responding...' },
  zh:{ live:'活跃', done:'完成', refresh:'刷新', prefs:'设置', hide:'隐藏', quit:'退出',
  noSess:'暂无活跃会话', start:'启动一个 Claude Code 会话',
  waiting:'等待输入', finished:'已结束', working:'工作中...', thinking:'思考中...', responding:'回复中...' }};
var lang = 'en';
function setLang(l){ lang=l; document.body.setAttribute('data-lang',l); }

// ── DOM refs ───────────────────────────────────────────────────
var c = document.getElementById('content');
var r = document.getElementById('refresh-btn');
var s = document.getElementById('settings-btn');
var clk = document.getElementById('clock');
var ctx = document.getElementById('ctx-menu');
var prefsP = document.getElementById('prefs-panel');
var prefsO = document.getElementById('prefs-overlay');

// ── Clock ──────────────────────────────────────────────────────
function updClock(){ clk.textContent = new Date().toTimeString().slice(0,8); }
setInterval(updClock,1000); updClock();

// ── Helpers ────────────────────────────────────────────────────
function esc(v){ if(!v) return ''; var d=document.createElement('div'); d.textContent=v; return d.innerHTML; }
function pad(n){ return String(n).padStart(2,'0'); }
function fmtUp(s){ if(!s||s<60) return '<1m'; if(s<3600) return Math.floor(s/60)+'m'; var h=Math.floor(s/3600); return h+'h'+(s%3600>0?Math.floor((s%3600)/60)+'m':''); }
function fmtTk(n){ if(!n) return '0'; if(n>=1e6) return (n/1e6).toFixed(1)+'M'; if(n>=1000) return Math.round(n/1000)+'K'; return String(n); }
function smod(m){ if(!m) return ''; m=m.toLowerCase(); if(/opus/.test(m)) return 'OPUS'; if(/sonnet/.test(m)) return 'SONNET'; if(/haiku/.test(m)) return 'HAIKU'; if(/mimo/.test(m)) return 'MIMO'; return m.substring(0,5).toUpperCase(); }
function schr(s){ return {working:'>',thinking:'~',responding:'#',idle:'=',done:'v'}[s]||' '; }
function isDone(s){ return s==='done'||s==='idle'; }
function trStat(tx){ var m={ 'Waiting for you':'waiting','Finished':'finished','Working...':'working','Thinking...':'thinking','Responding...':'responding' }; return L[lang][m[tx]]||tx; }
function t(k){ return (L[lang]&&L[lang][k])||k; }

// ── Settings ───────────────────────────────────────────────────
var settings = { clickThrough:false, opacity:1, lang:'en', pollMs:5000, size:'s' };
function loadS(){ try{ var r=localStorage.getItem('am-s'); if(r){ var o=JSON.parse(r); Object.assign(settings,o); setLang(settings.lang); pollI=settings.pollMs; } }catch(e){} }
function saveS(){ settings.lang=lang; settings.pollMs=pollI; localStorage.setItem('am-s',JSON.stringify(settings)); }

// ── Collapse state ─────────────────────────────────────────────
var expanded = new Set();
function saveExp(){ expanded.clear(); document.querySelectorAll('.project-folder:not(.collapsed)').forEach(function(el){ var n=el.querySelector('.folder-name'); if(n) expanded.add(n.textContent); }); }
function restExp(){ document.querySelectorAll('.project-folder').forEach(function(el){ var n=el.querySelector('.folder-name'); if(!n) return; if(expanded.has(n.textContent)) el.classList.remove('collapsed'); else el.classList.add('collapsed'); }); }

// ── Render ─────────────────────────────────────────────────────
function render(sessions){
  saveExp();
  var e=Object.entries(sessions), total=0, notDone=0;
  for(var i=0;i<e.length;i++) for(var j=0;j<e[i][1].sessions.length;j++){ total++; if(!isDone(e[i][1].sessions[j].status)) notDone++; }
  document.getElementById('footer-status').innerHTML = pad(e.length)+'P | '+pad(total)+'F | <span style="color:var(--amber)">'+pad(notDone)+' LIVE</span>';
  if(!e.length){ c.innerHTML='<div id="empty-state"><div class="boot-text"><span class="blink">_</span> '+t('noSess')+'<br><span style="color:var(--white-dim)">'+t('start')+'</span></div></div>'; return; }
  e.sort(function(a,b){ return a[0].localeCompare(b[0]); });
  var h='';
  for(var pi=0;pi<e.length;pi++){
    var pn=e[pi][0], proj=e[pi][1];
    var ss=proj.sessions.slice().sort(function(a,b){ return (a.file_path||'').localeCompare(b.file_path||''); });
    var dc=ss.filter(function(s){ return isDone(s.status); }).length, ac=ss.length-dc, act=ac>0;
    h+='<div class="project-folder '+(act?'folder-active':'folder-done')+' collapsed">';
    h+='<div class="folder-header" onclick="this.parentElement.classList.toggle(\'collapsed\')"><span class="folder-arrow">&#9660;</span><span class="folder-icon">'+(act?'[+]':'[v]')+'</span><span class="folder-name" title="'+esc(ss[0]&&ss[0].project_path||'')+'">'+esc(pn)+'</span><span class="folder-meta"><span class="folder-badge '+(act?'badge-active':'badge-done')+'">'+(act?ac+' '+t('live'):t('done'))+'</span><span class="folder-count">'+ss.length+'F</span></span></div>';
    h+='<div class="folder-sessions">';
    for(var si=0;si<ss.length;si++){
      var sx=ss[si], done=isDone(sx.status);
      h+='<div class="file-row status-'+sx.status+'" data-fp="'+esc(sx.file_path)+'" data-pp="'+esc(sx.project_path)+'"><span class="file-indent">|</span><span class="file-status '+(done?'file-done':'file-active')+'">['+schr(sx.status)+']</span><span class="file-name">'+esc(sx.name||sx.id.substring(0,8))+'</span>'+(sx.model?'<span class="file-model">'+esc(smod(sx.model))+'</span>':'')+(sx.source?'<span class="file-source">'+esc(sx.source.toUpperCase())+'</span>':'')+'</div>';
      h+='<div class="file-detail" data-fp="'+esc(sx.file_path)+'" data-pp="'+esc(sx.project_path)+'"><span class="file-indent">|</span><span class="detail-status '+(done?'detail-done':'')+'">'+esc(trStat(sx.status_text||sx.status))+'</span>'+(sx.tool_detail?'<span class="detail-tool">// '+esc(sx.tool_detail)+'</span>':'');
      var tkn=(sx.tokens_in||0)+(sx.tokens_out||0);
      if(tkn>0||(!done&&sx.uptime_secs>0)||sx.message_count>0){ h+='<span class="detail-meta">'; if(tkn>0) h+='TKN '+fmtTk(sx.tokens_in)+'/'+fmtTk(sx.tokens_out)+' '; if(!done&&sx.uptime_secs>0) h+='T+'+fmtUp(sx.uptime_secs)+' '; if(sx.message_count>0) h+='MSG '+sx.message_count; h+='</span>'; }
      h+='</div>';
    }
    h+='</div></div>';
  }
  c.innerHTML=h;
  restExp();
  c.querySelectorAll('.file-row,.file-detail').forEach(function(el){ el.addEventListener('click',function(){ invoke('open_in_vscode',{filePath:el.dataset.fp,projectPath:el.dataset.pp}); }); });
}

// ── Context menu ───────────────────────────────────────────────
function buildCtx(){ ctx.innerHTML='<div class="ctx-item" data-action="refresh">[R] '+t('refresh')+'</div><div class="ctx-item" data-action="prefs">[*] '+t('prefs')+'</div><div class="ctx-sep"></div><div class="ctx-item" data-action="hide">[_] '+t('hide')+'</div><div class="ctx-item ctx-quit" data-action="quit">[X] '+t('quit')+'</div>'; }

// ── Preferences i18n ────────────────────────────────────────────
var P = {
  en: { title:'[*] SETTINGS', height:'HEIGHT', poll:'POLL INT', lang:'LANGUAGE', ct:'CLICK THRU', op:'OPACITY',
    sizeS:'DEFAULT (520)', sizeM:'COMPACT (350)', sizeL:'EXPANDED (700)',
    poll3:'3s FAST', poll5:'5s NORMAL', poll10:'10s SLOW',
    en:'ENGLISH', zh:'CHINESE', ok:'OK', cancel:'CANCEL' },
  zh: { title:'[*] 偏好设置', height:'窗口高度', poll:'轮询间隔', lang:'语言', ct:'鼠标穿透', op:'透明度',
    sizeS:'默认 (520)', sizeM:'紧凑 (350)', sizeL:'扩展 (700)',
    poll3:'3s 快速', poll5:'5s 正常', poll10:'10s 慢速',
    en:'英文', zh:'中文', ok:'确定', cancel:'取消' }
};
function pt(k){ return (P[lang]&&P[lang][k])||k; }

function buildPrefs(){
  var s=document.getElementById('pref-size');
  s.innerHTML='<option value="s">'+pt('sizeS')+'</option><option value="m">'+pt('sizeM')+'</option><option value="l">'+pt('sizeL')+'</option>';
  var p=document.getElementById('pref-poll');
  p.innerHTML='<option value="3000">'+pt('poll3')+'</option><option value="5000">'+pt('poll5')+'</option><option value="10000">'+pt('poll10')+'</option>';
  var l=document.getElementById('pref-lang');
  l.innerHTML='<option value="en">'+pt('en')+'</option><option value="zh">'+pt('zh')+'</option>';
  document.getElementById('prefs-title').textContent=pt('title');
  document.getElementById('prefs-label-height').textContent=pt('height');
  document.getElementById('prefs-label-poll').textContent=pt('poll');
  document.getElementById('prefs-label-lang').textContent=pt('lang');
  document.getElementById('prefs-label-ct').textContent=pt('ct');
  document.getElementById('prefs-label-op').textContent=pt('op');
  document.getElementById('pref-ok').textContent=pt('ok');
  document.getElementById('pref-cancel').textContent=pt('cancel');
}

document.addEventListener('contextmenu',function(e){ e.preventDefault(); ctx.style.display='block'; ctx.style.left=Math.min(e.clientX,window.innerWidth-150)+'px'; ctx.style.top=Math.min(e.clientY,window.innerHeight-130)+'px'; });
document.addEventListener('click',function(e){ if(!ctx.contains(e.target)) ctx.style.display='none'; });
ctx.addEventListener('click',async function(e){ e.stopPropagation(); var a=e.target.dataset.action; if(!a) return; ctx.style.display='none';
  if(a==='refresh'){ invoke('refresh').then(function(j){ if(j) render(JSON.parse(j)); }).catch(function(){}); }
  else if(a==='prefs'){ openPrefs(); }
  else if(a==='hide'){ invoke('hide_window'); }
  else if(a==='quit'){ invoke('quit_app'); }
});

// ── Preferences ─────────────────────────────────────────────────
function openPrefs(){
  buildPrefs();
  document.getElementById('pref-size').value = settings.size;
  document.getElementById('pref-poll').value = String(pollI);
  document.getElementById('pref-lang').value = lang;
  document.getElementById('pref-clickthrough').checked = settings.clickThrough;
  var opv = Math.round(settings.opacity*100);
  document.getElementById('pref-opacity').value = opv;
  document.getElementById('pref-opacity-val').textContent = opv+'%';
  prefsO.style.display='block'; prefsP.style.display='block';
}
function closePrefs(){ prefsO.style.display='none'; prefsP.style.display='none'; }
prefsO.addEventListener('click',closePrefs);
document.getElementById('pref-opacity').addEventListener('input',function(){ document.getElementById('pref-opacity-val').textContent=this.value+'%'; });
document.getElementById('pref-ok').addEventListener('click',async function(){
  var ns=document.getElementById('pref-size').value;
  pollI=parseInt(document.getElementById('pref-poll').value);
  setLang(document.getElementById('pref-lang').value);
  settings.clickThrough=document.getElementById('pref-clickthrough').checked;
  settings.opacity=parseInt(document.getElementById('pref-opacity').value)/100;
  settings.size=ns; settings.lang=lang;
  startP(); saveS();
  await invoke('set_size',{scale:ns});
  await invoke('set_clickthrough',{enabled:settings.clickThrough});
  document.body.style.opacity=settings.opacity;
  buildCtx(); buildPrefs();
  invoke('refresh').then(function(j){ if(j) render(JSON.parse(j)); }).catch(function(){});
  closePrefs();
});
document.getElementById('pref-cancel').addEventListener('click',closePrefs);

// ── Init ────────────────────────────────────────────────────────
loadS(); buildCtx();
// Apply initial language attribute
document.body.setAttribute('data-lang', lang);
invoke('set_clickthrough',{enabled:settings.clickThrough});
document.body.style.opacity=settings.opacity;

invoke('get_sessions_json').then(function(j){ render(JSON.parse(j)); }).catch(function(e){ c.innerHTML='<div id="empty-state"><div class="boot-text" style="color:var(--red)">ERR: '+esc(String(e))+'</div></div>'; });

// ── Polling ────────────────────────────────────────────────────
var pollI=settings.pollMs, pollT=null;
function startP(){ if(pollT) clearInterval(pollT); pollT=setInterval(async function(){ try{ var j=await invoke('refresh'); if(j&&j!=='{}') render(JSON.parse(j)); }catch(e){} },pollI); }
startP();

// ── Events ─────────────────────────────────────────────────────
listen('sessions-updated',function(e){ try{ var d=typeof e.payload==='string'?JSON.parse(e.payload):e.payload; render(d); }catch(e){} });
listen('tray-action',function(e){
  if(e.payload==='prefs') openPrefs();
  else if(e.payload==='refresh'){ invoke('refresh').then(function(j){ if(j) render(JSON.parse(j)); }).catch(function(){}); }
  else if(e.payload==='toggle-ct'){ toggleClickThrough(); }
});

// ── Keyboard shortcuts ─────────────────────────────────────────
function toggleClickThrough(){
  settings.clickThrough = !settings.clickThrough;
  invoke('set_clickthrough',{enabled:settings.clickThrough});
  saveS();
}
document.addEventListener('keydown',function(e){
  if(e.ctrlKey && e.shiftKey && e.key==='C'){ e.preventDefault(); toggleClickThrough(); }
});

// ── Buttons ────────────────────────────────────────────────────
s.addEventListener('click',openPrefs);
r.addEventListener('click',async function(){ r.classList.add('spinning'); try{ var j=await invoke('refresh'); render(JSON.parse(j)); }catch(e){} setTimeout(function(){ r.classList.remove('spinning'); },600); });
