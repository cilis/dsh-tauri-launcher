/**
 * dsh-tauri-launcher — browser half.
 * 在设置面板注册「桌面启动」分区：桌面端启动开关（含快速确认与乐观反馈）、
 * 添加快捷方式按钮、关闭确认弹窗、诊断信息（仅出错时显示）。
 * 与宿主通信走 /api/dsh-tauri-launcher/*（同源 fetch，仅回环）。
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

    exports.inject = ['slots']

    exports.apply = function (ctx) {
      const slots = ctx.get('slots')
      if (slots === undefined) return

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

    return module.exports
  },
})
