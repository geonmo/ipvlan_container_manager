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
