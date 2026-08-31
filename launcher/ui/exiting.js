// 退出进度窗口：按实际内容高度微调窗口尺寸，避免小窗底部留白。
// （自 exiting.html 内联 <script> 抽出，以兼容严格 CSP 的 script-src 'self'。）
// status/hint 均为纯展示文本（无任何逻辑引用），已合并为单行文案。
window.addEventListener("DOMContentLoaded", () => {
  const card = document.querySelector(".card");
  if (!card || !window.__TAURI__) return;
  const h = Math.ceil(card.getBoundingClientRect().height);
  if (h > 0) {
    window.__TAURI__.window
      .getCurrentWindow()
      .setSize({ width: 320, height: h + 2 })
      .catch(() => {});
  }
});
