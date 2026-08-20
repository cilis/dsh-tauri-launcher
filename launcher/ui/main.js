// DeepSeek Harness 启动器前端：检测安装 → 自动安装（动画+日志）→ 成功提示 → 启动并跳转到本地 Web 界面。
// 通过 withGlobalTauri 注入的 window.__TAURI__ 调用后端命令。
/* global window, document */
(function () {
  "use strict";

  const { invoke } = window.__TAURI__.core;
  const { listen } = window.__TAURI__.event;

  const phases = [
    "checking",
    "installing",
    "success",
    "launching",
    "linking",
    "error",
  ];

  function show(phase) {
    for (const p of phases) {
      const el = document.getElementById(`phase-${p}`);
      if (el) el.classList.toggle("hidden", p !== phase);
    }
  }

  function setText(id, text) {
    const el = document.getElementById(id);
    if (el) el.textContent = text;
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

  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

  let unlisten = null;

  function clearUnlisten() {
    if (unlisten) {
      unlisten();
      unlisten = null;
    }
  }

  function appendLog(stream, line) {
    const log = document.getElementById("install-log");
    if (!log) return;
    const div = document.createElement("div");
    div.className = stream === "stderr" ? "log-line log-err" : "log-line";
    div.textContent = line;
    log.appendChild(div);
    while (log.childElementCount > 400) {
      log.firstElementChild?.remove();
    }
    log.scrollTop = log.scrollHeight;
  }

  function showError(message) {
    clearUnlisten();
    setText("error-message", message);
    show("error");
  }

  async function run() {
    clearUnlisten();
    show("checking");
    setText("checking-status", "正在检查 DeepSeek Harness 安装状态…");

    try {
      const check = await invoke("check_dsh");

      if (!check.installed) {
        if (check.error) {
          showError(`检查失败：${check.error}`);
          return;
        }
        // 未安装 → 自动安装（显示加载动画与实时日志）
        show("installing");
        unlisten = await listen("install-output", (e) =>
          appendLog(e.payload.stream, e.payload.line),
        );
        const version = await invoke("install_dsh");
        clearUnlisten();
        // 安装成功提示
        setText("success-hint", `DeepSeek Harness v${version} 安装成功，正在启动…`);
        show("success");
        await sleep(2600);
      } else {
        setText(
          "checking-status",
          `已检测到 DeepSeek Harness v${check.version ?? "?"}，正在启动…`,
        );
        await sleep(900);
      }

      show("launching");
      const info = await invoke("launch_dsh");
      setText("linking-hint", `本地服务：${info.url}`);
      show("linking");
      await sleep(900);
      window.location.href = info.url;
    } catch (e) {
      showError(`操作失败：${formatError(e)}`);
    }
  }

  window.addEventListener("DOMContentLoaded", () => {
    document.getElementById("retry-btn")?.addEventListener("click", () => {
      const log = document.getElementById("install-log");
      if (log) log.innerHTML = "";
      void run();
    });
    void run();
  });
})();
