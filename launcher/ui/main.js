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

  /* ---------- 标题栏与 iframe 外壳 ---------- */

  let frameUrl = "";

  /** DSH 就绪：隐藏启动卡片，显示标题栏并用 iframe 承载（不再整页跳转）。 */
  function showFrame(url) {
    frameUrl = url;
    document.getElementById("app")?.classList.add("hidden");
    document.getElementById("titlebar")?.classList.remove("hidden");
    const frame = document.getElementById("dsh-frame");
    if (frame) {
      frame.src = url;
      frame.classList.remove("hidden");
    }
  }

  /** 标题栏三键：最小化 / 最大化还原 / 关闭（关闭被 Rust 拦截为隐藏到托盘）。 */
  function setupTitlebar() {
    if (!window.__TAURI__) return;
    const appWindow = window.__TAURI__.window.getCurrentWindow();

    document.getElementById("btn-min")?.addEventListener("click", () => {
      void appWindow.minimize();
    });
    document.getElementById("btn-max")?.addEventListener("click", () => {
      void appWindow.toggleMaximize();
    });
    document.getElementById("btn-close")?.addEventListener("click", () => {
      void appWindow.close();
    });

    // ---------- 标题栏菜单 ----------
    const menuBtn = document.getElementById("menu-btn");
    const menuPanel = document.getElementById("menu-panel");

    function closeMenu() {
      menuPanel?.classList.add("hidden");
      menuBtn?.classList.remove("open");
    }

    menuBtn?.addEventListener("click", () => {
      const isOpen = !menuPanel?.classList.contains("hidden");
      if (isOpen) {
        closeMenu();
      } else {
        menuPanel?.classList.remove("hidden");
        menuBtn?.classList.add("open");
      }
    });

    // 点击标题栏其他区域收起；切到 iframe（父窗口失焦）时也收起
    document.addEventListener("mousedown", (e) => {
      if (
        menuPanel &&
        !menuPanel.classList.contains("hidden") &&
        !menuPanel.contains(e.target) &&
        e.target !== menuBtn
      ) {
        closeMenu();
      }
    });
    window.addEventListener("blur", closeMenu);

    document
      .getElementById("menu-settings")
      ?.addEventListener("click", () => {
        closeMenu();
        void invoke("open_settings_window").catch((e) =>
          console.error("打开设置失败", e),
        );
      });

    document
      .getElementById("menu-reload")
      ?.addEventListener("click", () => {
        closeMenu();
        // iframe 跨源无法直接 contentWindow.location.reload()，重设 src 重载
        const frame = document.getElementById("dsh-frame");
        if (frame && frameUrl) frame.src = frameUrl;
      });

    document
      .getElementById("menu-browser")
      ?.addEventListener("click", () => {
        closeMenu();
        if (frameUrl) {
          void invoke("open_in_browser", { url: frameUrl }).catch((e) =>
            console.error("浏览器打开失败", e),
          );
        }
      });

    document.getElementById("menu-quit")?.addEventListener("click", () => {
      void invoke("quit_app").catch((e) => console.error("退出失败", e));
    });

    // 最大化图标随窗口状态切换
    async function refreshMaxGlyph() {
      try {
        const maximized = await appWindow.isMaximized();
        document
          .querySelector(".ic-max")
          ?.classList.toggle("hidden", maximized);
        document
          .querySelector(".ic-restore")
          ?.classList.toggle("hidden", !maximized);
      } catch {
        /* 权限或时序问题不影响主流程 */
      }
    }
    if (window.__TAURI__.event?.listen) {
      void window.__TAURI__.event
        .listen("tauri://resize", () => void refreshMaxGlyph())
        .catch(() => {});
    }
    void refreshMaxGlyph();
  }

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
      // 不再整页跳转：交给 iframe 承载，标题栏得以常驻
      showFrame(info.url);
    } catch (e) {
      showError(`操作失败：${formatError(e)}`);
    }
  }

  window.addEventListener("DOMContentLoaded", () => {
    setupTitlebar();
    document.getElementById("retry-btn")?.addEventListener("click", () => {
      const log = document.getElementById("install-log");
      if (log) log.innerHTML = "";
      void run();
    });
    void run();
  });
})();
