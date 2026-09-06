/**
 * dsh-tauri-launcher — browser half.
 * 在设置面板注册「桌面启动」分区：桌面端启动开关（含快速确认与乐观反馈）、
 * 添加快捷方式按钮、关闭确认弹窗、诊断信息（仅出错时显示）。
 * 与宿主通信走 /api/dsh-tauri-launcher/*（同源 fetch，仅回环）。
 *
 * 除设置分区外，本文件还承载两个与桌面端外壳（Tauri 主窗口）协作的功能，
 * 仅在页面被 iframe 内嵌（桌面端）时启用：
 * 1. 标题栏主题中继：把 DSH 主题方案与真实生效的 token 发给外壳（installThemeRelay）；
 * 2. 会话导航栈 + 系统主题跟随（installSessionNav / installSystemThemeFollow）。
 *
 * 模块级结构（2026-09 阶段一拆分，见优化报告 v2）：
 * - createSessionNavStack：应用层会话导航栈（纯逻辑，无 DOM/服务依赖，可单测）
 * - retryLadder：服务就绪重试梯子（theme/change 事件、sessions 服务可能后于插件就绪）
 * - installDesktopSection / installThemeRelay / installSystemThemeFollow / installSessionNav
 */
window.__ModuleLoader__.load({
  id: '@lenorin/dsh-tauri-launcher',
  factory: (require) => {
    const module = { exports: {} }
    const exports = module.exports
    const React = require('react')

    const CSS = `
.tauri-dsl-root { display: flex; flex-direction: column; gap: 12px; padding: 4px 0 8px; max-width: 560px; }
.tauri-dsl-desc { font-size: 12px; line-height: 1.6; color: var(--dsw-alias-label-secondary); }
.tauri-dsl-row { display: flex; align-items: center; justify-content: space-between; gap: 12px; padding: 12px 14px; border: 1px solid var(--dsw-alias-border-l1); border-radius: 12px; background: var(--dsw-alias-bg-layer-2); cursor: pointer; }
.tauri-dsl-left { display: flex; flex-direction: column; gap: 3px; min-width: 0; }
.tauri-dsl-label { font-size: 13px; font-weight: 600; color: var(--dsw-alias-label-primary); }
.tauri-dsl-state { display: flex; align-items: center; gap: 6px; font-size: 12px; color: var(--dsw-alias-label-secondary); }
.tauri-dsl-dot { width: 8px; height: 8px; border-radius: 50%; flex-shrink: 0; background: var(--dsw-alias-label-secondary); }
.tauri-dsl-dot-on { background: var(--dsw-alias-state-success-primary); box-shadow: 0 0 6px var(--dsw-alias-state-success-primary); }
.tauri-dsl-dot-unknown { background: var(--dsw-alias-state-warn-primary); }
.tauri-dsl-dot-pending { background: var(--dsw-alias-state-warn-primary); animation: tauri-dsl-pulse 1s ease-in-out infinite; }
@keyframes tauri-dsl-pulse { 0%, 100% { opacity: 1; } 50% { opacity: 0.3; } }
.tauri-dsl-switch { position: relative; width: 42px; height: 24px; flex-shrink: 0; display: inline-block; }
.tauri-dsl-switch input { opacity: 0; width: 0; height: 0; position: absolute; }
.tauri-dsl-track { position: absolute; inset: 0; border-radius: 999px; background: var(--dsw-alias-bg-layer-1); border: 1px solid var(--dsw-alias-border-l2); transition: background 0.15s ease, border-color 0.15s ease; }
.tauri-dsl-track::before { content: ''; position: absolute; width: 16px; height: 16px; left: 3px; top: 3px; border-radius: 50%; background: var(--dsw-alias-label-secondary); transition: transform 0.15s ease, background 0.15s ease; }
.tauri-dsl-switch input:checked + .tauri-dsl-track { background: var(--dsw-alias-brand-primary); border-color: transparent; }
.tauri-dsl-switch input:checked + .tauri-dsl-track::before { transform: translateX(18px); background: var(--dsw-alias-label-primary-inverted); }
.tauri-dsl-switch input:disabled + .tauri-dsl-track { opacity: 0.55; }
.tauri-dsl-row:has(input:disabled) { cursor: wait; }
.tauri-dsl-linkbtn { display: inline-flex; align-items: center; gap: 6px; padding: 6px 12px; border-radius: 8px; border: 1px solid var(--dsw-alias-border-l2); background: var(--dsw-alias-bg-layer-2); color: var(--dsw-alias-label-primary); font-size: 12px; cursor: pointer; }
.tauri-dsl-linkbtn:disabled { opacity: 0.55; cursor: default; }
.tauri-dsl-overlay { position: fixed; inset: 0; z-index: 9999; display: flex; align-items: center; justify-content: center; background: rgba(0, 0, 0, 0.45); }
.tauri-dsl-dialog { width: min(340px, calc(100vw - 48px)); padding: 18px 18px 14px; border-radius: 14px; background: var(--dsw-alias-bg-overlay); border: 1px solid var(--dsw-alias-border-l2); box-shadow: 0 18px 50px rgba(0, 0, 0, 0.4); display: flex; flex-direction: column; gap: 10px; }
.tauri-dsl-dialog-title { font-size: 14px; font-weight: 600; color: var(--dsw-alias-label-primary); }
.tauri-dsl-dialog-body { font-size: 13px; line-height: 1.7; color: var(--dsw-alias-label-secondary); }
.tauri-dsl-dialog-actions { display: flex; justify-content: flex-end; gap: 10px; margin-top: 4px; }
.tauri-dsl-btn-ghost { padding: 7px 16px; border-radius: 8px; border: 1px solid var(--dsw-alias-border-l2); background: transparent; color: var(--dsw-alias-label-primary); font-size: 13px; cursor: pointer; }
.tauri-dsl-btn-ghost:hover { background: var(--dsw-alias-bg-layer-2); }
.tauri-dsl-btn-primary { padding: 7px 16px; border-radius: 8px; border: none; background: var(--dsw-alias-brand-primary); color: var(--dsw-alias-label-primary-inverted); font-size: 13px; font-weight: 600; cursor: pointer; }
.tauri-dsl-btn-primary:hover { filter: brightness(1.08); }
.tauri-dsl-error { font-size: 12px; color: var(--dsw-alias-state-error-primary); white-space: pre-wrap; word-break: break-all; }
.tauri-dsl-details { font-size: 12px; color: var(--dsw-alias-label-secondary); }
.tauri-dsl-details summary { cursor: pointer; user-select: none; }
.tauri-dsl-diag { margin: 6px 0 0; padding: 8px 10px; max-height: 220px; overflow: auto; background: var(--dsw-alias-bg-layer-2); border: 1px solid var(--dsw-alias-border-l1); border-radius: 8px; font-family: Consolas, 'Cascadia Mono', monospace; font-size: 11px; line-height: 1.5; color: var(--dsw-alias-label-secondary); white-space: pre-wrap; word-break: break-all; }
/* ⚠️ 外壳集成 hack（脆弱，已知会随外壳升级失效）：
   设置导航图标替换依赖 DSH 外壳 bundle 的 CSS Modules hash 类名（.VOzbGW_*）与
   分区序号（nth-child(5)），二者任一变化该规则即失效；失效时仅回退为默认齿轮，
   无功能影响。若外壳未来提供按分区 id 注入图标的能力，应迁移到该机制。 */
.VOzbGW_navList button:nth-child(5) svg { display: none; }
.VOzbGW_navList button:nth-child(5)::before {
  content: '';
  width: 16px;
  height: 16px;
  flex-shrink: 0;
  background-color: currentColor;
  -webkit-mask: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='black' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'%3E%3Crect width='20' height='14' x='2' y='3' rx='2'/%3E%3Cline x1='8' x2='16' y1='21' y2='21'/%3E%3Cline x1='12' x2='12' y1='17' y2='21'/%3E%3C/svg%3E") center / 16px 16px no-repeat;
  mask: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='black' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'%3E%3Crect width='20' height='14' x='2' y='3' rx='2'/%3E%3Cline x1='8' x2='16' y1='21' y2='21'/%3E%3Cline x1='12' x2='12' y1='17' y2='21'/%3E%3C/svg%3E") center / 16px 16px no-repeat;
  opacity: 0.85;
}
.VOzbGW_navList button:nth-child(5)[aria-current='true']::before {
  background-color: var(--dsw-alias-brand-primary);
  opacity: 1;
}
/* 布局框架定制：body 前缀保证特异性 (0,1,1) 压过产品的 (0,1,0)，不依赖样式注入顺序。
   全部使用官方 DOM 原生标记（CSS modules 的 hash_原名 class 子串匹配），不依赖
   @linxin666/dsh-web-ui-all 兼容层挂的 data-dsh-frame / data-pane 语义标记。
   注意不能用裸 [class*='frame']：官方另有 user-questions/attachment/subagent
   四个 *_frame 组件会被误伤；AppFrame 是唯一内部含 sidebarCol 的 frame，用 :has() 收紧。 */
/* 框架底色与侧栏一致：sidebar 与页面空白融为一体，conversation 借边框勾勒边界 */
body [class*='frame']:has([class*='sidebarCol']) {
  background: var(--dsw-specific-sidebar-fill);
}
/* sidebar 隐藏右边框，分隔线职责移交 conversation 左边框 */
body [class*='sidebarCol'] {
  border-right: none;
}
/* conversation 列：顶部分隔线 + 左边框 + 左上角 10px 圆角（仅裁切形状，不改背景；
   产品对 centerCol 自带 overflow:hidden，圆角裁切天然成立） */
body [class*='centerCol'] {
  border-top: 1px solid var(--dsw-alias-border-l1);
  border-left: 1px solid var(--dsw-alias-border-l1);
  border-top-left-radius: 10px;
}
`

    const API_BASE = '/api/dsh-tauri-launcher'
    /** 自定义请求头：同源 fetch 可带，跨站 form POST 无法伪造（CSRF 防线）。 */
    const CSRF_HEADERS = { 'x-requested-with': 'dsh-tauri-launcher' }

    const api = {
      async state() {
        const response = await fetch(API_BASE + '/state', { headers: CSRF_HEADERS })
        return response.json()
      },
      async setDesktop(enabled) {
        const response = await fetch(API_BASE + '/set-desktop', {
          method: 'POST',
          headers: { ...CSRF_HEADERS, 'content-type': 'application/json' },
          body: JSON.stringify({ enabled }),
        })
        return response.json()
      },
      async setShortcut() {
        const response = await fetch(API_BASE + '/set-shortcut', { method: 'POST', headers: CSRF_HEADERS })
        return response.json()
      },
      async diagnose() {
        const response = await fetch(API_BASE + '/diagnose', { headers: CSRF_HEADERS })
        return response.json()
      },
    }

    /**
     * 服务就绪重试梯子：目标服务（theme/change 事件、sessions）可能在插件
     * 之后才就绪，且 preference 等于默认值时不会发变化事件——用短重试保证
     * 初始状态一定送达（成功或达到 maxTries 后停止）。返回清理函数。
     */
    function retryLadder(attempt, { immediate = false, intervalMs = 500, maxTries = 12 } = {}) {
      let done = false
      let tries = 0
      const run = () => {
        if (done) return
        try {
          if (attempt()) done = true
        } catch { /* 服务未就绪，下一轮重试 */ }
      }
      if (immediate) run()
      const timer = setInterval(() => {
        tries += 1
        if (done || tries >= maxTries) {
          clearInterval(timer)
          return
        }
        run()
      }, intervalMs)
      return () => clearInterval(timer)
    }

    /**
     * 应用层会话导航栈（纯逻辑，无 DOM / DSH 服务依赖，可单测）。
     * DSH 是 React SPA：切会话不产生浏览器历史条目，history.back() 无效，
     * 前进/后退等价于在「最近访问会话」双栈上移动（对标浏览器语义）：
     * - 非跳转来源的 current 变化 → 上一个会话压 backStack、清空 forwardStack；
     * - goBack/goForward 弹栈跳转，当前会话压对向栈；栈条目 { id, address }
     *   （address 供 catalog 子会话 openSubagent）；上限 cap、连续重复跳过；
     * - 跳转前置 pendingJump 锁，防止自身触发的变化被当作用户切换重复压栈，
     *   超时（jumpTimeoutMs）兜底解锁，由调用方传入 onUnlock 回调同步最新状态。
     */
    function createSessionNavStack({ cap = 50, jumpTimeoutMs = 1500 } = {}) {
      let backStack = []
      let forwardStack = []
      let currentId = undefined
      let currentAddress = undefined
      let pendingJump = null
      let jumpTimer = null

      const pushStack = (stack, entry) => {
        const top = stack[stack.length - 1]
        if (top && top.id === entry.id && top.address === entry.address) return
        stack.push(entry)
        if (stack.length > cap) stack.shift()
      }

      const clearJumpTimer = () => {
        if (jumpTimer) {
          clearTimeout(jumpTimer)
          jumpTimer = null
        }
      }

      return {
        /** 前进/后退两个方向当前是否可走（供外壳置灰按钮）。 */
        status: () => ({ back: backStack.length > 0, forward: forwardStack.length > 0 }),
        /** 以快照初始化当前会话（订阅前先取一次快照；跳转解锁后同步最新状态）。 */
        init: (id, address) => {
          currentId = id
          currentAddress = address
        },
        /** 跳转锁是否生效中。 */
        isPending: () => pendingJump !== null,
        /**
         * 非跳转来源的 current 变化。返回 true 表示状态对外可见变化（调用方应上报）。
         * 规则：current 变 undefined 只清 address；同 id 只更新 address（不压栈）；
         * 真正切换才压 backStack 并清空 forwardStack。
         */
        onExternalChange(snap) {
          if (snap.id === undefined) {
            currentAddress = undefined
            return true
          }
          if (snap.id === currentId) {
            if (snap.address !== undefined) currentAddress = snap.address
            return false
          }
          if (currentId !== undefined) {
            pushStack(backStack, { id: currentId, address: currentAddress })
            forwardStack = []
          }
          currentId = snap.id
          currentAddress = snap.address
          return true
        },
        /**
         * 跳转锁生效期间的变化。返回 true 表示跳转已被确认（命中目标）或被
         * 用户的新切换取代（二者都应解锁并同步状态后上报）。
         */
        onPendingChange(snap) {
          if (snap.id === pendingJump.id || (snap.id !== undefined && snap.id !== currentId)) {
            clearJumpTimer()
            pendingJump = null
            currentId = snap.id
            currentAddress = snap.address
            return true
          }
          return false
        },
        /** 后退：弹出目标（当前会话压 forwardStack）。无可退时返回 null。 */
        goBack() {
          if (backStack.length === 0) return null
          const target = backStack.pop()
          if (currentId !== undefined) pushStack(forwardStack, { id: currentId, address: currentAddress })
          return target
        },
        /** 前进：对称于 goBack。 */
        goForward() {
          if (forwardStack.length === 0) return null
          const target = forwardStack.pop()
          if (currentId !== undefined) pushStack(backStack, { id: currentId, address: currentAddress })
          return target
        },
        /**
         * 准备一次跳转：置 pendingJump 锁、同步 current，并启动超时兜底 timer。
         * 超时先解锁再回调 onUnlock（调用方应读最新快照并上报）；跳转动作抛出
         * 异常时调用 abortJump 回滚锁（timer 保留：到期仍会同步一次状态）。
         */
        jumpTo(entry, onUnlock) {
          pendingJump = { id: entry.id, address: entry.address }
          currentId = entry.id
          currentAddress = entry.address
          clearJumpTimer()
          jumpTimer = setTimeout(() => {
            pendingJump = null
            onUnlock()
          }, jumpTimeoutMs)
        },
        /** 跳转动作抛出异常时回滚锁。 */
        abortJump: () => {
          pendingJump = null
        },
        /** 释放 timer（插件卸载时）。 */
        dispose: clearJumpTimer,
      }
    }

    exports.inject = ['slots']

    /* ---------- 设置分区「桌面启动」 ---------- */

    function installDesktopSection(ctx, slots) {
      const styleEl = document.createElement('style')
      styleEl.dataset.plugin = '@lenorin/dsh-tauri-launcher'
      styleEl.textContent = CSS
      document.head.appendChild(styleEl)
      ctx.effect(() => () => styleEl.remove(), 'dsh-tauri-launcher: css')

      function errText(error) {
        if (typeof error === 'string') return error
        if (error && error.message) return error.message
        try { return JSON.stringify(error) } catch { return String(error) }
      }

      async function fetchState() {
        try {
          const state = await api.state()
          return { desktop: state.desktop, shortcut: Boolean(state.shortcut), error: '', diag: '' }
        } catch (error) {
          let diag = ''
          try {
            const diagnosed = await api.diagnose()
            diag = (diagnosed && diagnosed.diag) || ''
          } catch { /* 诊断失败不影响错误展示 */ }
          return { desktop: null, shortcut: false, error: '读取状态失败：' + errText(error), diag }
        }
      }

      function DesktopSection() {
        const [snap, setSnap] = React.useState(null)
        const [busy, setBusy] = React.useState(false)
        const [pending, setPending] = React.useState(null)
        const [linkBusy, setLinkBusy] = React.useState(false)
        const [confirmOpen, setConfirmOpen] = React.useState(false)

        React.useEffect(() => {
          let alive = true
          const merge = (s) => setSnap((prev) => (prev && prev.error ? { ...s, error: prev.error } : s))
          fetchState().then((s) => { if (alive) merge(s) })
          const timer = setInterval(() => { fetchState().then((s) => { if (alive) merge(s) }) }, 10000)
          return () => { alive = false; clearInterval(timer) }
        }, [])

        const performToggle = async (target) => {
          setBusy(true)
          setPending(target ? 'on' : 'off')
          try {
            const result = await api.setDesktop(target)
            if (result && result.ok) {
              setSnap((prev) => ({
                desktop: result.desktop,
                shortcut: typeof result.shortcut === 'boolean' ? result.shortcut : (prev ? prev.shortcut : false),
                error: '',
                diag: result.diag || '',
              }))
            } else {
              setSnap((prev) => ({
                desktop: typeof result.desktop === 'boolean' ? result.desktop : (prev ? prev.desktop : null),
                shortcut: prev ? prev.shortcut : false,
                error: (result && result.error) || '操作失败',
                diag: (result && result.diag) || (prev ? prev.diag : ''),
              }))
            }
          } catch (error) {
            setSnap((prev) => ({ desktop: prev ? prev.desktop : null, shortcut: prev ? prev.shortcut : false, error: '操作失败：' + errText(error), diag: prev ? prev.diag : '' }))
          } finally {
            setBusy(false)
            setPending(null)
          }
        }

        const toggle = async () => {
          if (busy) return
          const target = !(snap && snap.desktop === true)
          if (target === false) {
            setConfirmOpen(true)
            return
          }
          await performToggle(true)
        }

        const addLink = async () => {
          if (linkBusy) return
          setLinkBusy(true)
          try {
            const result = await api.setShortcut()
            if (result && result.ok) {
              setSnap((prev) => ({ ...(prev || { desktop: null, shortcut: false, diag: '' }), shortcut: true, error: '', diag: result.diag || (prev ? prev.diag : '') }))
            } else {
              setSnap((prev) => ({ ...(prev || { desktop: null, shortcut: false, diag: '' }), error: (result && result.error) || '添加快捷方式失败', diag: (result && result.diag) || (prev ? prev.diag : '') }))
            }
          } catch (error) {
            setSnap((prev) => ({ ...(prev || { desktop: null, shortcut: false, diag: '' }), error: '添加快捷方式失败：' + errText(error) }))
          } finally {
            setLinkBusy(false)
          }
        }

        const desktop = snap ? snap.desktop : null
        const shortcut = snap ? snap.shortcut : false
        const checked = pending ? pending === 'on' : desktop === true
        const stateText = pending
          ? (pending === 'on' ? '正在启动…' : '正在退出…')
          : desktop === true ? '运行中' : desktop === false ? '已停止' : '状态未知'
        const dotClass = 'tauri-dsl-dot' + (pending ? ' tauri-dsl-dot-pending' : desktop === true ? ' tauri-dsl-dot-on' : desktop === null ? ' tauri-dsl-dot-unknown' : '')

        return React.createElement('div', { className: 'tauri-dsl-root' },
          React.createElement('div', { className: 'tauri-dsl-desc' }, '启动或退出本机桌面应用（DeepSeek Harness Tauri）。开启时自动创建桌面快捷方式，关闭时自动删除；开机启动与全局快捷键请在桌面应用托盘 → 设置中管理。'),
          React.createElement('label', { className: 'tauri-dsl-row' },
            React.createElement('div', { className: 'tauri-dsl-left' },
              React.createElement('span', { className: 'tauri-dsl-label' }, '桌面端启动'),
              React.createElement('span', { className: 'tauri-dsl-state' },
                React.createElement('span', { className: dotClass }),
                stateText,
              ),
            ),
            React.createElement('span', { className: 'tauri-dsl-switch' },
              React.createElement('input', { type: 'checkbox', checked, onChange: toggle, disabled: busy }),
              React.createElement('span', { className: 'tauri-dsl-track' }),
            ),
          ),
          desktop === true && !pending ? React.createElement('button', {
            className: 'tauri-dsl-linkbtn',
            onClick: addLink,
            disabled: linkBusy || shortcut,
          }, shortcut ? '已添加快捷方式' : '添加快捷方式') : null,
          snap && snap.error ? React.createElement('div', { className: 'tauri-dsl-error' }, snap.error) : null,
          snap && snap.error && snap.diag ? React.createElement('details', { className: 'tauri-dsl-details' },
            React.createElement('summary', null, '诊断信息'),
            React.createElement('pre', { className: 'tauri-dsl-diag' }, snap.diag),
          ) : null,
          confirmOpen ? React.createElement('div', { className: 'tauri-dsl-overlay', onClick: () => setConfirmOpen(false) },
            React.createElement('div', { className: 'tauri-dsl-dialog', onClick: (event) => event.stopPropagation() },
              React.createElement('div', { className: 'tauri-dsl-dialog-title' }, '确定关闭桌面端启动？'),
              React.createElement('div', { className: 'tauri-dsl-dialog-body' }, '桌面应用将退出，同时删除桌面快捷方式。之后可随时在设置中重新开启，或通过命令行启动。'),
              React.createElement('div', { className: 'tauri-dsl-dialog-actions' },
                React.createElement('button', { className: 'tauri-dsl-btn-ghost', onClick: () => setConfirmOpen(false) }, '✗ 取消'),
                React.createElement('button', {
                  className: 'tauri-dsl-btn-primary',
                  onClick: () => { setConfirmOpen(false); void performToggle(false) },
                }, '✓ 确认关闭'),
              ),
            ),
          ) : null,
        )
      }

      slots.inject('settings.section', () => slots.register(
        { name: 'settings.section', id: 'desktop-launch', order: 30, label: '桌面启动' },
        () => React.createElement(DesktopSection, null),
      ))
    }

    /* ---------- 标题栏主题中继（DSH → 外壳） ---------- */

    /**
     * 桌面端把 DSH 嵌在 iframe 里；这里读取官方主题服务解析出的浅/深方案，
     * 并把 DSH 页面真实生效的设计 token（计算样式）发给启动器外壳，
     * 让标题栏与 DSH 配色逐像素一致。仅当本页面被嵌入（iframe）时才有意义，
     * 普通浏览器标签（window.parent === window）直接跳过。
     * 注意：内置主题的快照 tokens 是空表，真实颜色在 CSS 变量里
     * （body[data-ds-dark-theme] 切换），必须经 getComputedStyle 取。
     */
    function installThemeRelay(ctx) {
      const readVar = (name) => {
        try {
          const style = getComputedStyle(document.body)
          let value = style.getPropertyValue(name).trim()
          const ref = value.match(/^var\((--[a-zA-Z0-9-]+)\)$/)
          if (ref) {
            const inner = style.getPropertyValue(ref[1]).trim()
            if (inner) value = inner
          }
          return value || null
        } catch {
          return null
        }
      }

      let themeSent = false
      const postTheme = () => {
        if (window.parent === window) return
        let scheme = null
        try {
          const theme = ctx.get('theme')
          if (theme && typeof theme.getTheme === 'function') {
            const snap = theme.getTheme()
            const s = snap && snap.active && snap.active.colorScheme
            if (s === 'light' || s === 'dark') scheme = s
          }
        } catch { /* 主题服务异常：退化到 DOM 推断 */ }
        if (!scheme) {
          scheme = document.body.hasAttribute('data-ds-dark-theme') ? 'dark' : 'light'
        }
        const payload = {
          __dshLauncherTheme: 1,
          scheme,
          bg: readVar('--dsw-specific-sidebar-fill'),
          fg: readVar('--dsw-alias-label-primary'),
          menuBg: readVar('--dsw-alias-bg-overlay'),
          menuBorder: readVar('--dsw-alias-border-l2'),
          sep: readVar('--dsw-alias-border-l1'),
          danger: readVar('--dsw-alias-state-error-primary'),
        }
        themeSent = true
        window.parent.postMessage(payload, '*')
        // 返回 true 供重试梯子判定“已送达”，停止后续重试（与历史 themeSent 语义一致）
        return true
      }

      try {
        ctx.on('theme/change', postTheme)
      } catch { /* 事件不可用时依赖下面的重试梯子 */ }

      // 主题服务可能在本插件之后才就绪，且 preference 等于默认值时不会发
      // theme/change：用重试梯子保证初始状态一定送达（成功或 6 秒后停止）。
      const disposeTimer = retryLadder(postTheme, {})
      ctx.effect(() => disposeTimer, 'dsh-tauri-launcher: theme relay timer')
    }

    /* ---------- 桌面端系统主题跟随（外壳 → DSH） ---------- */

    /**
     * WebView2 页面里的 prefers-color-scheme 只有宿主显式设置
     * PreferredColorScheme 才会随 Windows 变化，而 tauri/wry 未暴露该能力
     * （wry#806），DSH 的 `system` 偏好会被冻结在启动时的取值。启动器轮询
     * 到系统主题变化后经外壳 postMessage 转发到这里：偏好为 `system` 时把
     * ThemeRuntime 持有的 MediaQueryList.matches（冻结值）覆盖为真实系统值
     * 并触发重发布——偏好保持「跟随系统」不变，界面随真实系统主题切换；
     * 用户手动选了固定主题时不干预。
     * ⚠️ 兼容层 hack：依赖 ThemeRuntime 的 media/publish 运行时成员（无真
     * 私有化），外壳升级若重命名则静默退化为现状（跟随失效，无副作用）。
     */
    function installSystemThemeFollow(ctx, parentOrigin) {
      const applySystemTheme = (scheme) => {
        try {
          const theme = ctx.get('theme')
          if (!theme || !(theme.media instanceof MediaQueryList)) return
          if (typeof theme.publish !== 'function') return
          const snap = typeof theme.getTheme === 'function' ? theme.getTheme() : null
          if (!snap || snap.preference !== 'system') return
          if (snap.active && snap.active.colorScheme === scheme) return
          const desc = Object.getOwnPropertyDescriptor(MediaQueryList.prototype, 'matches')
          if (!desc || typeof desc.get !== 'function') return
          const wantDark = scheme === 'dark'
          Object.defineProperty(theme.media, 'matches', {
            configurable: true,
            get() { return wantDark }
          })
          theme.publish()
        } catch { /* 主题服务异常：忽略，等待下次通知 */ }
      }

      const onSystemThemeMessage = (event) => {
        const data = event.data || {}
        if (event.source !== window.parent) return
        if (event.origin !== parentOrigin) return
        const scheme = data.__tbSystemTheme
        if (scheme === 'light' || scheme === 'dark') applySystemTheme(scheme)
      }
      window.addEventListener('message', onSystemThemeMessage)
      ctx.effect(() => () => window.removeEventListener('message', onSystemThemeMessage), 'dsh-tauri-launcher: system theme follow')
    }

    /* ---------- 会话导航（应用层会话栈，等价浏览器前进/后退） ---------- */

    /**
     * sessions 服务存在就绪时序：一次性 ctx.get 可能拿到 undefined（静默失效），
     * 故用重试梯子初始化；外壳每 3 秒 ping 一次，状态周期性回报实现自愈。
     */
    function installSessionNav(ctx, parentOrigin) {
      let navDispose = null

      const initNav = () => {
        const sessions = ctx.get('sessions')
        const store = sessions && sessions.list
        if (!store || typeof store.subscribe !== 'function') {
          return false
        }

        const snapCurrent = () => {
          const snap = store.getSnapshot()
          return { id: snap.current, address: snap.currentAddress }
        }

        const nav = createSessionNavStack()

        const sendStatus = () => {
          if (window.parent === window) return
          const payload = { __tbNavStatus: { back: nav.status().back, forward: nav.status().forward } }
          try {
            // 用 '*' 投递：WebView2 下对 http://tauri.localhost 虚拟主机的
            // 精确 targetOrigin 匹配有丢弃嫌疑；外壳侧会校验 event.origin。
            window.parent.postMessage(payload, '*')
          } catch { /* 忽略 */ }
        }

        const jumpTo = (entry) => {
          nav.jumpTo(entry, () => {
            // 超时兜底解锁：读最新快照同步当前会话并上报。
            const snap = snapCurrent()
            nav.init(snap.id, snap.address)
            sendStatus()
          })
          try {
            if (entry.address) sessions.openSubagent(entry.address)
            else sessions.open(entry.id)
          } catch {
            nav.abortJump()
          }
        }

        const goBack = () => {
          const target = nav.goBack()
          if (!target) return
          sendStatus()
          jumpTo(target)
        }

        const goForward = () => {
          const target = nav.goForward()
          if (!target) return
          sendStatus()
          jumpTo(target)
        }

        const onListChange = () => {
          const snap = snapCurrent()
          if (nav.isPending()) {
            if (nav.onPendingChange(snap)) sendStatus()
            return
          }
          if (nav.onExternalChange(snap)) sendStatus()
        }

        const onNavMessage = (event) => {
          const data = event.data || {}
          if (event.source !== window.parent) return
          if (event.origin !== parentOrigin) return
          const dir = data.__tbNav
          if (dir === 'back') goBack()
          else if (dir === 'forward') goForward()
          else if (dir === 'ping') sendStatus()
        }
        window.addEventListener('message', onNavMessage)

        const onKeydown = (event) => {
          if (!event.altKey || event.ctrlKey || event.shiftKey) return
          if (event.key === 'ArrowLeft') {
            event.preventDefault()
            goBack()
          } else if (event.key === 'ArrowRight') {
            event.preventDefault()
            goForward()
          }
        }
        window.addEventListener('keydown', onKeydown, true)

        // 鼠标侧键：DOM button 3 = 后退（XButton1）、4 = 前进（XButton2）
        const onMouseup = (event) => {
          if (event.button === 3) {
            event.preventDefault()
            goBack()
          } else if (event.button === 4) {
            event.preventDefault()
            goForward()
          }
        }
        window.addEventListener('mouseup', onMouseup, true)

        const unsubscribe = store.subscribe(onListChange)
        const init = snapCurrent()
        nav.init(init.id, init.address)
        sendStatus()

        navDispose = () => {
          if (typeof unsubscribe === 'function') unsubscribe()
          nav.dispose()
          window.removeEventListener('message', onNavMessage)
          window.removeEventListener('keydown', onKeydown, true)
          window.removeEventListener('mouseup', onMouseup, true)
        }
        return true
      }

      // 立即尝试一次，再走重试梯子（服务可能在之后才就绪）
      const disposeTimer = retryLadder(initNav, { immediate: true })

      ctx.effect(() => () => {
        disposeTimer()
        if (navDispose) navDispose()
      }, 'dsh-tauri-launcher: session nav')
    }

    /**
     * 父窗口 origin：跨源读不到 window.parent.location，用 document.referrer
     * 推导（兜底 http://tauri.localhost）。
     */
    function deriveParentOrigin() {
      try {
        if (document.referrer) return new URL(document.referrer).origin
      } catch { /* 兜底 http://tauri.localhost */ }
      return 'http://tauri.localhost'
    }

    exports.apply = function (ctx) {
      const slots = ctx.get('slots')
      if (slots === undefined) return

      installDesktopSection(ctx, slots)

      // 主题中继内部自带 window.parent 检查（浏览器直开自动不启用）。
      installThemeRelay(ctx)

      // 以下功能仅在 iframe 内嵌（桌面端）时有意义；浏览器直开时
      // Chromium 原生跟随系统主题、且无标题栏，直接跳过。
      if (window.parent !== window) {
        const parentOrigin = deriveParentOrigin()
        installSystemThemeFollow(ctx, parentOrigin)
        installSessionNav(ctx, parentOrigin)
      }
    }

    return module.exports
  },
})
