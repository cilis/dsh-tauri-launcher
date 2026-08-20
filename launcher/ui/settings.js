// 设置窗口：开机启动 + 创建全局快捷方式（全局快捷键注册/注销）。
// 通过 withGlobalTauri 注入的 window.__TAURI__ 调用后端命令。
/* global window, document */
(function () {
  "use strict";

  const { invoke } = window.__TAURI__.core;

  const autoEl = document.getElementById("toggle-autostart");
  const hotkeyEl = document.getElementById("toggle-shortcut");
  const desktopEl = document.getElementById("toggle-desktop");
  const terminateEl = document.getElementById("toggle-terminate");
  const hotkeyLabel = document.getElementById("hotkey-label");
  const statusEl = document.getElementById("status");

  let busy = false;

  function setStatus(text, isError) {
    statusEl.textContent = text || "";
    statusEl.classList.toggle("err", Boolean(isError));
  }

  function formatError(e) {
    if (typeof e === "string") return e;
    if (e instanceof Error) return e.message;
    try {
      return JSON.stringify(e);
    } catch {
      return String(e);
    }
  }

  async function applyChange(el, cmd) {
    const enabled = el.checked;
    busy = true;
    autoEl.disabled = hotkeyEl.disabled = desktopEl.disabled = terminateEl.disabled = true;
    setStatus("正在保存…");
    try {
      await invoke(cmd, { enabled });
      setStatus("已保存");
    } catch (e) {
      // 保存失败时回滚开关状态
      el.checked = !enabled;
      setStatus(`保存失败：${formatError(e)}`, true);
    } finally {
      busy = false;
      autoEl.disabled = hotkeyEl.disabled = desktopEl.disabled = terminateEl.disabled = false;
      setTimeout(() => {
        if (!busy) setStatus("");
      }, 2000);
    }
  }

  window.addEventListener("DOMContentLoaded", async () => {
    document.getElementById("close-btn")?.addEventListener("click", () => {
      void invoke("close_settings").catch(() => {});
    });

    autoEl.addEventListener("change", () =>
      void applyChange(autoEl, "set_autostart_setting"),
    );
    hotkeyEl.addEventListener("change", () =>
      void applyChange(hotkeyEl, "set_global_shortcut_setting"),
    );
    desktopEl.addEventListener("change", () =>
      void applyChange(desktopEl, "set_desktop_shortcut_setting"),
    );
    terminateEl.addEventListener("change", () =>
      void applyChange(terminateEl, "set_terminate_harness_on_exit_setting"),
    );

    try {
      const s = await invoke("get_settings");
      autoEl.checked = Boolean(s.autostart);
      hotkeyEl.checked = Boolean(s.global_shortcut);
      desktopEl.checked = Boolean(s.desktop_shortcut);
      terminateEl.checked = Boolean(s.terminate_harness_on_exit);
      if (s.hotkey) hotkeyLabel.textContent = s.hotkey;
    } catch (e) {
      setStatus(`读取设置失败：${formatError(e)}`, true);
    }
  });
})();
