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

function getNodePool()    { try { return JSON.parse(localStorage.getItem(ICM_NODES_KEY) || '[]'); } catch(_){ return []; } }
function getNetworkPool() { try { return JSON.parse(localStorage.getItem(ICM_NETS_KEY)  || '[]'); } catch(_){ return []; } }
function saveNodePool(arr)    { localStorage.setItem(ICM_NODES_KEY, JSON.stringify(arr)); }
function saveNetworkPool(arr) { localStorage.setItem(ICM_NETS_KEY,  JSON.stringify(arr)); }

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
