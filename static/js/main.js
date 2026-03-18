/**
 * HA Container Manager - Main JS
 * CodeMirror 에디터 초기화 및 공통 유틸리티
 */

document.addEventListener('DOMContentLoaded', function () {
  initCodeEditors();
});

/**
 * .code-editor 클래스의 textarea를 CodeMirror 에디터로 변환
 */
function initCodeEditors() {
  document.querySelectorAll('textarea.code-editor').forEach(function (textarea) {
    const mode = textarea.dataset.mode || 'shell';

    const cmMode = {
      'shell': 'shell',
      'yaml': 'yaml',
      'javascript': 'javascript',
    }[mode] || 'shell';

    const editor = CodeMirror.fromTextArea(textarea, {
      mode: cmMode,
      theme: 'material-darker',
      lineNumbers: true,
      lineWrapping: false,
      autofocus: false,
      indentUnit: 2,
      tabSize: 2,
      extraKeys: {
        'Tab': function (cm) { cm.replaceSelection('  '); }
      }
    });

    // 높이 조정 (data-mode에 따라)
    const minHeight = textarea.style.minHeight || '120px';
    editor.setSize('100%', null);
    editor.getWrapperElement().style.minHeight = minHeight;

    // textarea와 동기화 (form submit 시)
    editor.on('change', function () {
      editor.save();
    });

    // 참조 저장 (외부에서 값 설정 시 사용)
    textarea._cmEditor = editor;
  });
}

/**
 * 클립보드 복사 유틸리티
 */
function copyToClipboard(text) {
  if (navigator.clipboard && navigator.clipboard.writeText) {
    return navigator.clipboard.writeText(text);
  }
  // fallback
  const ta = document.createElement('textarea');
  ta.value = text;
  ta.style.position = 'fixed';
  ta.style.opacity = '0';
  document.body.appendChild(ta);
  ta.select();
  document.execCommand('copy');
  document.body.removeChild(ta);
  return Promise.resolve();
}

// ── 노드 풀 (localStorage) ────────────────────────────────────
const ICM_NODES_KEY    = 'icm_nodes';
const ICM_NETS_KEY     = 'icm_networks';
const ICM_PROFILES_KEY = 'icm_ansible_profiles';
const ICM_PROFILE_KEY  = 'icm_active_profile'; // 활성 프로파일 이름

function getNodePool()    { try { return JSON.parse(localStorage.getItem(ICM_NODES_KEY)    || '[]'); } catch(_){ return []; } }
function getNetworkPool() { try { return JSON.parse(localStorage.getItem(ICM_NETS_KEY)    || '[]'); } catch(_){ return []; } }
function getProfilePool() { try { return JSON.parse(localStorage.getItem(ICM_PROFILES_KEY)|| '[]'); } catch(_){ return []; } }
function saveNodePool(arr)    { localStorage.setItem(ICM_NODES_KEY,    JSON.stringify(arr)); }
function saveNetworkPool(arr) { localStorage.setItem(ICM_NETS_KEY,     JSON.stringify(arr)); }
function saveProfilePool(arr) { localStorage.setItem(ICM_PROFILES_KEY, JSON.stringify(arr)); }

/** 활성 Ansible 프로파일 이름 반환 */
function getActiveProfileName() { return localStorage.getItem(ICM_PROFILE_KEY) || ''; }
/** 활성 Ansible 프로파일 이름 저장 */
function setActiveProfileName(name) { localStorage.setItem(ICM_PROFILE_KEY, name); }
/** 활성 프로파일 객체 반환 (프로파일 풀에서 이름으로 조회) */
function getActiveProfile() {
  const name = getActiveProfileName();
  if (!name) return null;
  return getProfilePool().find(p => p.name === name) || null;
}

/**
 * 셔틀 위젯 초기화
 * @param {string} leftId  - 왼쪽(풀) <ul> id
 * @param {string} rightId - 오른쪽(선택) <ul> id
 * @param {Function} getItems    - () => [{label, value}] 풀 아이템 목록
 * @param {Function} onAdd       - (item) => rendered <li> innerHTML (추가 시 오른쪽에 넣을 HTML)
 * @param {Function} onGetValues - (rightUl) => any  폼 제출 전 값 추출 콜백
 */
function initShuttle(leftId, rightId, getItems, onAdd) {
  const leftUl  = document.getElementById(leftId);
  const rightUl = document.getElementById(rightId);
  if (!leftUl || !rightUl) return;

  function rebuildLeft() {
    const used = new Set(
      Array.from(rightUl.querySelectorAll('li[data-hostname]'))
           .map(li => li.dataset.hostname)
    );
    leftUl.innerHTML = '';
    getItems().forEach(item => {
      if (used.has(item.hostname)) return;
      const li = document.createElement('li');
      li.className = 'list-group-item list-group-item-action shuttle-item py-2';
      li.dataset.hostname = item.hostname;
      li.dataset.ip = item.ip || '';
      li.innerHTML = `<span class="fw-semibold">${esc(item.hostname)}</span> <small class="text-muted">${esc(item.ip)}</small>`;
      li.onclick = () => li.classList.toggle('active');
      leftUl.appendChild(li);
    });
  }

  window[leftId + '_rebuildLeft'] = rebuildLeft;

  document.getElementById(leftId + '_add')?.addEventListener('click', () => {
    leftUl.querySelectorAll('li.active').forEach(li => {
      li.classList.remove('active');
      const item = { hostname: li.dataset.hostname, ip: li.dataset.ip };
      const newLi = document.createElement('li');
      newLi.className = 'list-group-item shuttle-item py-1';
      newLi.dataset.hostname = item.hostname;
      newLi.dataset.ip = item.ip;
      newLi.innerHTML = onAdd(item);
      newLi.querySelector('.shuttle-remove')?.addEventListener('click', () => {
        newLi.remove();
        rebuildLeft();
      });
      rightUl.appendChild(newLi);
      rebuildLeft();
    });
  });

  rebuildLeft();
}

/**
 * HTML 엔티티 디코딩
 */
function decodeHtml(html) {
  const txt = document.createElement('textarea');
  txt.innerHTML = html;
  return txt.value;
}

/**
 * 토스트 알림 표시
 */
function showToast(message, type = 'success') {
  const container = document.getElementById('toastContainer') || createToastContainer();
  const toast = document.createElement('div');
  toast.className = `alert alert-${type} alert-dismissible shadow-sm`;
  toast.style.cssText = 'min-width:250px;';
  toast.innerHTML = `
    ${message}
    <button type="button" class="btn-close" data-bs-dismiss="alert"></button>
  `;
  container.appendChild(toast);
  setTimeout(() => toast.remove(), 3000);
}

function createToastContainer() {
  const div = document.createElement('div');
  div.id = 'toastContainer';
  div.style.cssText = 'position:fixed;top:70px;right:20px;z-index:9999;';
  document.body.appendChild(div);
  return div;
}

// ── Ansible 프로파일 유틸리티 ─────────────────────────────────────────────────

/**
 * Ansible 프로파일 목록을 로드하여 <select> 요소를 채웁니다.
 * @param {string} selectId - <select> 요소 id
 * @param {Function} [onChange] - 선택 변경 시 콜백(profile 객체 또는 null)
 */
async function loadProfileSelector(selectId, onChange) {
  const sel = document.getElementById(selectId);
  if (!sel) return;
  try {
    const profiles = await fetch('/api/ansible-profiles').then(r => r.json());
    sel.innerHTML = '<option value="">-- 프로파일 선택 --</option>';
    profiles.forEach(p => {
      const opt = document.createElement('option');
      opt.value = p.name;
      opt.textContent = p.name;
      opt.dataset.profile = JSON.stringify(p);
      sel.appendChild(opt);
    });
    if (onChange) {
      sel.addEventListener('change', () => {
        const opt = sel.selectedOptions[0];
        const profile = opt && opt.dataset.profile ? JSON.parse(opt.dataset.profile) : null;
        onChange(profile);
      });
    }
  } catch (e) {
    console.warn('프로파일 로드 실패:', e);
  }
}

/**
 * Ansible 프로파일로 인벤토리 YAML vars 블록을 반환합니다.
 * @param {Object} profile - DbAnsibleProfile 객체
 * @returns {string} vars 블록 YAML
 */
function buildProfileVarsYaml(profile) {
  if (!profile) return '';
  const lines = [];
  lines.push(`    ansible_user: ${profile.ssh_user || 'root'}`);
  if (profile.auth_method === 'key' && profile.ssh_key) {
    lines.push(`    ansible_ssh_private_key_file: ${profile.ssh_key}`);
  } else if (profile.ssh_password) {
    lines.push(`    ansible_ssh_pass: ${profile.ssh_password}`);
  }
  if (profile.become) {
    lines.push(`    ansible_become: true`);
    if (profile.become_method) lines.push(`    ansible_become_method: ${profile.become_method}`);
    if (profile.become_password) lines.push(`    ansible_become_password: ${profile.become_password}`);
  }
  return lines.join('\n');
}

/**
 * CodeMirror 에디터(또는 textarea)의 YAML 인벤토리 vars 블록을 프로파일로 업데이트합니다.
 * @param {string} textareaId - textarea id
 * @param {Object} profile - DbAnsibleProfile 객체
 */
function applyProfileToInventory(textareaId, profile) {
  const ta = document.getElementById(textareaId);
  if (!ta) return;
  const editor = ta._cmEditor;
  const current = editor ? editor.getValue() : ta.value;

  const varsVars = buildProfileVarsYaml(profile);
  if (!varsVars) return;

  // vars: 블록 교체 또는 추가
  let updated;
  if (current.includes('  vars:')) {
    // vars 블록을 새 값으로 교체 (다음 최상위 키 또는 파일 끝까지)
    updated = current.replace(
      /(  vars:\n)([\s\S]*?)(?=\n\S|\n?$)/,
      `  vars:\n${varsVars}\n`
    );
  } else {
    updated = current.trimEnd() + '\n  vars:\n' + varsVars + '\n';
  }

  if (editor) {
    editor.setValue(updated);
  } else {
    ta.value = updated;
  }
}

/**
 * 노드 목록 + 프로파일로 인벤토리 YAML 전체를 생성합니다.
 * @param {Array} nodes - DbNode 배열
 * @param {Object|null} profile - DbAnsibleProfile 또는 null
 * @returns {string}
 */
function buildInventoryYaml(nodes, profile) {
  const lines = ['all:', '  hosts:'];
  nodes.forEach(n => {
    lines.push(`    ${n.hostname}:`);
    lines.push(`      ansible_host: ${n.ip}`);
  });
  if (profile) {
    lines.push('  vars:');
    const vars = buildProfileVarsYaml(profile);
    if (vars) lines.push(vars);
  }
  return lines.join('\n') + '\n';
}
