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

    // ---------- 前进 / 后退（插件协作通道） ----------
    // DSH 在跨源 iframe 里，外壳无法直接操作它的 history；
    // postMessage 是唯一合法的跨源操作，由插件在 DSH 页面内同源执行。
    function sendNav(dir) {
      const frame = document.getElementById("dsh-frame");
      if (!frame || !frameUrl || !frame.contentWindow) return;
      try {
        frame.contentWindow.postMessage({ __tbNav: dir }, new URL(frameUrl).origin);
      } catch {
        /* iframe 未就绪或 origin 解析失败时忽略 */
      }
    }
    document.getElementById("tb-back")?.addEventListener("click", () => sendNav("back"));
    document.getElementById("tb-forward")?.addEventListener("click", () => sendNav("forward"));

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

  /* ---------- 标题栏主题跟随（DSH 浅/深） ---------- */
  // 色板为 DSH 官方设计 token 的真实值（浅/深两套），仅作兜底：
  // 插件在线时以它发来的计算样式逐项覆盖（--tb-* 全部取 DSH 实际生效色）。

  const TB_PALETTES = {
    light: {
      bg: "#f9fafb", // --dsw-specific-sidebar-fill: bluish-50
      fg: "#0f1115", // --dsw-alias-label-primary: bluish-1000
      titleFg: "#0f1115",
      hover: "rgba(17, 24, 39, 0.08)",
      menuBg: "#e9ecf2", // --dsw-alias-bg-overlay: bluish-150
      menuBorder: "rgba(0, 0, 0, 0.10)", // --dsw-alias-border-l2: #0000001a
      menuShadow: "0 10px 34px rgba(15, 23, 42, 0.14)",
      sep: "rgba(0, 0, 0, 0.04)", // --dsw-alias-border-l1: #0000000a
      danger: "#ec1313", // --dsw-alias-state-error-primary: red-600
      closeHoverBg: "#e81123",
      closeHoverFg: "#ffffff",
    },
    dark: {
      bg: "#1b1b1c", // --dsw-specific-sidebar-fill: bluish-900
      fg: "#f9fafb", // --dsw-alias-label-primary: bluish-50
      titleFg: "#f9fafb",
      hover: "rgba(255, 255, 255, 0.08)",
      menuBg: "#61666b", // --dsw-alias-bg-overlay: bluish-700
      menuBorder: "rgba(255, 255, 255, 0.12)", // --dsw-alias-border-l2: #ffffff1f
      menuShadow: "0 10px 34px rgba(0, 0, 0, 0.5)",
      sep: "rgba(255, 255, 255, 0.06)", // --dsw-alias-border-l1: #ffffff0f
      danger: "#f25a5a", // --dsw-alias-state-error-primary: red-400
      closeHoverBg: "#e81123",
      closeHoverFg: "#ffffff",
    },
  };

  // 插件主题到达后即以 DSH 为准，系统主题兜底不再介入
  let pluginThemeApplied = false;

  // 颜色值来自插件消息（可信来源），只做基本形状校验；非法值回退色板
  const pickColor = (value, fallback) =>
    typeof value === "string" &&
    value.length >= 3 &&
    value.length <= 64 &&
    !value.includes("var(") &&
    /^[#a-zA-Z0-9(),.\s%-]+$/.test(value)
      ? value
      : fallback;

  function applyTitlebarTheme(mode, overrides) {
    const p = TB_PALETTES[mode === "dark" ? "dark" : "light"];
    const ov = overrides || {};
    const s = document.documentElement.style;
    s.setProperty("--tb-bg", pickColor(ov.bg, p.bg));
    s.setProperty("--tb-fg", pickColor(ov.fg, p.fg));
    s.setProperty("--tb-title-fg", pickColor(ov.fg, p.titleFg));
    s.setProperty("--tb-hover", p.hover);
    s.setProperty("--tb-menu-bg", pickColor(ov.menuBg, p.menuBg));
    s.setProperty("--tb-menu-border", pickColor(ov.menuBorder, p.menuBorder));
    s.setProperty("--tb-menu-shadow", p.menuShadow);
    s.setProperty("--tb-sep", pickColor(ov.sep, p.sep));
    s.setProperty("--tb-danger", pickColor(ov.danger, p.danger));
    s.setProperty("--tb-close-hover-bg", p.closeHoverBg);
    s.setProperty("--tb-close-hover-fg", p.closeHoverFg);
  }

  function setupThemeFollow() {
    // 主通道：Web 插件（运行在 DSH iframe 里）双向 postMessage：
    // 主题中继 + 会话导航状态（后退/前进键的可用性）。
    // 外壳收到任何消息后回显进 iframe（诊断 + 插件侧可观测），并周期 ping
    // 让插件回报最新状态（单发丢失也能自愈）。
    const echoToFrame = (payload) => {
      const frame = document.getElementById("dsh-frame");
      if (!frame || !frameUrl || !frame.contentWindow) return;
      try {
        frame.contentWindow.postMessage(payload, new URL(frameUrl).origin);
      } catch {
        /* 忽略 */
      }
    };

    window.addEventListener("message", (event) => {
      const data = event.data;
      if (!data || !frameUrl) return;
      try {
        if (event.origin !== new URL(frameUrl).origin) return;
      } catch {
        return;
      }
      if (data.__tbNavStatus) {
        const back = document.getElementById("tb-back");
        const fwd = document.getElementById("tb-forward");
        if (back) back.disabled = !data.back;
        if (fwd) fwd.disabled = !data.forward;
        echoToFrame({
          __tbNavEcho: {
            stage: "status",
            back: Boolean(back && !back.disabled),
            forward: Boolean(fwd && !fwd.disabled),
          },
        });
        return;
      }
      if (data.__dshLauncherTheme !== 1) return;
      pluginThemeApplied = true;
      applyTitlebarTheme(data.scheme, {
        bg: data.bg,
        fg: data.fg,
        menuBg: data.menuBg,
        menuBorder: data.menuBorder,
        sep: data.sep,
        danger: data.danger,
      });
      echoToFrame({ __tbNavEcho: { stage: "theme", scheme: data.scheme } });
    });

    // 心跳探针：每 3 秒 ping 一次，插件回以最新导航状态
    setInterval(() => {
      echoToFrame({ __tbNav: "ping" });
    }, 3000);

    // 兜底：插件未运行（未安装 / DSH 在普通浏览器打开）→ 跟随 Windows 系统主题
    if (window.matchMedia) {
      const mq = window.matchMedia("(prefers-color-scheme: dark)");
      const onScheme = () => {
        if (!pluginThemeApplied) applyTitlebarTheme(mq.matches ? "dark" : "light");
      };
      if (mq.addEventListener) mq.addEventListener("change", onScheme);
      onScheme();
    } else {
      applyTitlebarTheme("light");
    }
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
    setupThemeFollow();
    document.getElementById("retry-btn")?.addEventListener("click", () => {
      const log = document.getElementById("install-log");
      if (log) log.innerHTML = "";
      void run();
    });
    void run();
  });
})();
