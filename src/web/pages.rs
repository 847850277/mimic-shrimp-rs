//! 内置的轻量级 HTML 页面。

/// 微信通道未启用时的提示页。
pub(crate) fn weixin_connect_disabled_page_html() -> String {
    r##"<!doctype html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>微信接入未启用</title>
  <style>
    :root {
      --ink: #1d241f;
      --muted: #68726b;
      --line: rgba(29, 36, 31, 0.12);
      --accent: #0d6b3f;
      --panel: rgba(255, 250, 240, 0.94);
      --warn: #915b15;
      --shadow: 0 22px 50px rgba(34, 40, 31, 0.12);
    }

    * { box-sizing: border-box; }

    body {
      margin: 0;
      min-height: 100vh;
      color: var(--ink);
      font: 16px/1.6 "SF Pro Text", "PingFang SC", "Helvetica Neue", sans-serif;
      background:
        radial-gradient(circle at top right, rgba(13, 107, 63, 0.12), transparent 26%),
        radial-gradient(circle at bottom left, rgba(145, 91, 21, 0.12), transparent 24%),
        linear-gradient(180deg, #f8f3e9 0%, #efe6d7 100%);
      display: grid;
      place-items: center;
      padding: 24px;
    }

    .card {
      width: min(760px, 100%);
      background: var(--panel);
      border: 1px solid var(--line);
      border-radius: 32px;
      box-shadow: var(--shadow);
      padding: 32px;
      backdrop-filter: blur(16px);
    }

    .eyebrow {
      display: inline-flex;
      align-items: center;
      gap: 8px;
      padding: 6px 12px;
      border-radius: 999px;
      border: 1px solid var(--line);
      color: var(--warn);
      background: rgba(255, 255, 255, 0.72);
      font-size: 12px;
      letter-spacing: 0.08em;
      text-transform: uppercase;
    }

    h1 {
      margin: 18px 0 12px;
      font-size: clamp(34px, 6vw, 52px);
      line-height: 1.06;
      letter-spacing: -0.04em;
    }

    p {
      margin: 0;
      color: var(--muted);
      font-size: 17px;
    }

    .block {
      margin-top: 24px;
      padding: 18px 20px;
      border-radius: 22px;
      background: rgba(255, 255, 255, 0.72);
      border: 1px solid var(--line);
    }

    .block h2 {
      margin: 0 0 10px;
      font-size: 18px;
      letter-spacing: -0.02em;
    }

    ol {
      margin: 0;
      padding-left: 20px;
      color: var(--ink);
    }

    code, pre {
      font-family: "SFMono-Regular", "Menlo", "Monaco", monospace;
      font-size: 14px;
    }

    pre {
      margin: 0;
      padding: 14px 16px;
      border-radius: 16px;
      background: #1f241f;
      color: #edf5ee;
      overflow: auto;
    }

    .hint {
      margin-top: 18px;
      font-size: 14px;
      color: var(--muted);
    }
  </style>
</head>
<body>
  <main class="card">
    <div class="eyebrow">Wechat Disabled</div>
    <h1>微信通道当前未启用。</h1>
    <p>要使用微信扫码接入页，请先在服务端打开 <code>WEIXIN_ENABLED</code>，然后重启服务。</p>

    <section class="block">
      <h2>推荐做法</h2>
      <ol>
        <li>在项目根目录的 <code>.env</code> 中加入 <code>WEIXIN_ENABLED=true</code></li>
        <li>重启当前服务进程</li>
        <li>重新打开 <code>/weixin/connect</code> 页面</li>
      </ol>
    </section>

    <section class="block">
      <h2>示例配置</h2>
      <pre>WEIXIN_ENABLED=true</pre>
    </section>

    <p class="hint">如果你已经改过环境变量但页面还是这样，通常是服务进程还没重启。</p>
  </main>
</body>
</html>
"##
    .to_string()
}

/// 微信扫码接入页。
pub(crate) fn weixin_connect_page_html() -> String {
    r##"<!doctype html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>微信接入</title>
  <style>
    :root {
      --bg: #f5f3ec;
      --panel: rgba(255, 251, 242, 0.92);
      --ink: #1c2a1f;
      --muted: #68756b;
      --line: rgba(28, 42, 31, 0.12);
      --accent: #0d6b3f;
      --accent-soft: rgba(13, 107, 63, 0.12);
      --warn: #9d5f00;
      --error: #a52c2c;
      --shadow: 0 20px 50px rgba(35, 38, 29, 0.12);
    }

    * { box-sizing: border-box; }

    body {
      margin: 0;
      min-height: 100vh;
      color: var(--ink);
      font: 16px/1.6 "SF Pro Text", "PingFang SC", "Helvetica Neue", sans-serif;
      background:
        radial-gradient(circle at top left, rgba(13, 107, 63, 0.14), transparent 28%),
        radial-gradient(circle at bottom right, rgba(173, 126, 49, 0.14), transparent 24%),
        linear-gradient(180deg, #f8f5ee 0%, #efe8da 100%);
    }

    .shell {
      width: min(1040px, calc(100vw - 32px));
      margin: 0 auto;
      padding: 40px 0 56px;
    }

    .hero {
      display: grid;
      grid-template-columns: minmax(0, 1.2fr) minmax(320px, 420px);
      gap: 24px;
      align-items: stretch;
    }

    .panel {
      background: var(--panel);
      border: 1px solid var(--line);
      border-radius: 28px;
      box-shadow: var(--shadow);
      backdrop-filter: blur(14px);
    }

    .intro {
      padding: 32px;
      position: relative;
      overflow: hidden;
    }

    .intro::after {
      content: "";
      position: absolute;
      right: -40px;
      bottom: -60px;
      width: 220px;
      height: 220px;
      border-radius: 999px;
      background: radial-gradient(circle, rgba(13, 107, 63, 0.18), transparent 70%);
      pointer-events: none;
    }

    .eyebrow {
      display: inline-flex;
      align-items: center;
      gap: 8px;
      padding: 6px 12px;
      border-radius: 999px;
      background: rgba(255, 255, 255, 0.62);
      border: 1px solid var(--line);
      color: var(--muted);
      font-size: 12px;
      letter-spacing: 0.08em;
      text-transform: uppercase;
    }

    h1 {
      margin: 18px 0 12px;
      font-size: clamp(34px, 5vw, 54px);
      line-height: 1.04;
      letter-spacing: -0.04em;
    }

    .lead {
      max-width: 34rem;
      margin: 0;
      color: var(--muted);
      font-size: 17px;
    }

    .steps {
      margin: 28px 0 0;
      padding: 0;
      list-style: none;
      display: grid;
      gap: 12px;
    }

    .steps li {
      display: flex;
      gap: 12px;
      align-items: flex-start;
      color: var(--ink);
    }

    .steps strong {
      display: inline-grid;
      place-items: center;
      width: 28px;
      height: 28px;
      border-radius: 999px;
      background: var(--accent-soft);
      color: var(--accent);
      font-size: 13px;
      flex: none;
    }

    .qr-card {
      padding: 28px;
      display: grid;
      gap: 18px;
    }

    .status {
      display: inline-flex;
      align-items: center;
      gap: 10px;
      width: fit-content;
      padding: 8px 14px;
      border-radius: 999px;
      background: rgba(255, 255, 255, 0.72);
      border: 1px solid var(--line);
      color: var(--muted);
      font-size: 14px;
    }

    .status::before {
      content: "";
      width: 9px;
      height: 9px;
      border-radius: 999px;
      background: currentColor;
      opacity: 0.85;
    }

    .status.pending { color: var(--warn); }
    .status.success { color: var(--accent); }
    .status.error { color: var(--error); }

    .qr-frame {
      display: grid;
      place-items: center;
      aspect-ratio: 1;
      border-radius: 24px;
      background:
        linear-gradient(135deg, rgba(255,255,255,0.9), rgba(245,240,230,0.8)),
        #fff;
      border: 1px solid var(--line);
      overflow: hidden;
      min-height: 320px;
    }

    .qr-frame img {
      width: min(100%, 320px);
      height: auto;
      display: block;
    }

    .qr-placeholder {
      padding: 28px;
      text-align: center;
      color: var(--muted);
    }

    .qr-fallback {
      display: flex;
      flex-wrap: wrap;
      gap: 10px;
      align-items: center;
      color: var(--muted);
      font-size: 13px;
    }

    .qr-fallback a {
      color: var(--accent);
      text-decoration: none;
      font-weight: 600;
    }

    .qr-fallback a:hover {
      text-decoration: underline;
    }

    .actions {
      display: flex;
      flex-wrap: wrap;
      gap: 12px;
    }

    button {
      appearance: none;
      border: 0;
      border-radius: 16px;
      padding: 12px 18px;
      font: inherit;
      cursor: pointer;
      transition: transform 140ms ease, box-shadow 140ms ease, opacity 140ms ease;
    }

    button:hover { transform: translateY(-1px); }
    button:disabled { opacity: 0.58; cursor: wait; transform: none; }

    .primary {
      background: var(--accent);
      color: #f8fbf7;
      box-shadow: 0 12px 24px rgba(13, 107, 63, 0.22);
    }

    .secondary {
      background: rgba(255, 255, 255, 0.75);
      color: var(--ink);
      border: 1px solid var(--line);
    }

    .detail {
      color: var(--muted);
      font-size: 14px;
      white-space: pre-wrap;
      word-break: break-word;
    }

    .accounts {
      margin-top: 24px;
      padding: 24px 28px 28px;
    }

    .accounts h2 {
      margin: 0 0 8px;
      font-size: 22px;
      letter-spacing: -0.02em;
    }

    .accounts p {
      margin: 0 0 18px;
      color: var(--muted);
    }

    .account-list {
      display: grid;
      gap: 12px;
    }

    .account-item {
      display: grid;
      gap: 6px;
      padding: 16px 18px;
      border-radius: 18px;
      background: rgba(255, 255, 255, 0.7);
      border: 1px solid var(--line);
    }

    .account-top {
      display: flex;
      justify-content: space-between;
      gap: 12px;
      align-items: center;
    }

    .account-actions {
      display: flex;
      gap: 10px;
      flex-wrap: wrap;
      margin-top: 8px;
    }

    .account-button {
      padding: 8px 12px;
      border-radius: 12px;
      font-size: 13px;
    }

    .pill {
      display: inline-flex;
      align-items: center;
      gap: 8px;
      padding: 4px 10px;
      border-radius: 999px;
      font-size: 12px;
      background: rgba(13, 107, 63, 0.1);
      color: var(--accent);
    }

    .pill.offline {
      background: rgba(165, 44, 44, 0.1);
      color: var(--error);
    }

    .pill.paused {
      background: rgba(157, 95, 0, 0.12);
      color: var(--warn);
    }

    .empty {
      padding: 24px;
      text-align: center;
      color: var(--muted);
      border: 1px dashed var(--line);
      border-radius: 18px;
      background: rgba(255, 255, 255, 0.45);
    }

    .footnote {
      margin-top: 14px;
      color: var(--muted);
      font-size: 13px;
    }

    @media (max-width: 860px) {
      .shell { padding-top: 24px; }
      .hero { grid-template-columns: 1fr; }
      .intro, .qr-card, .accounts { padding: 22px; }
      .qr-frame { min-height: 280px; }
    }
  </style>
</head>
<body>
  <main class="shell">
    <section class="hero">
      <article class="panel intro">
        <div class="eyebrow">Wechat iLink Bot</div>
        <h1>打开页面，扫码，接入完成。</h1>
        <p class="lead">
          这个页面会自动为你生成二维码，并持续等待扫码确认。你不用手动复制
          <code>session_key</code>，也不用自己轮询接口。
        </p>
        <ol class="steps">
          <li><strong>1</strong><span>页面加载后自动生成二维码。</span></li>
          <li><strong>2</strong><span>用微信扫描二维码，并在手机中完成确认。</span></li>
          <li><strong>3</strong><span>页面会自动检测连接结果，成功后展示当前账号状态。</span></li>
        </ol>
        <p class="footnote">保持这个页面打开，直到状态变成“已连接”。</p>
      </article>

      <section class="panel qr-card">
        <div id="status" class="status pending">正在准备二维码...</div>
        <div class="qr-frame">
          <img id="qr-image" alt="微信扫码二维码" hidden>
          <div id="qr-placeholder" class="qr-placeholder">二维码生成中，请稍候。</div>
        </div>
        <div class="qr-fallback">
          <span>如果右侧空白或浏览器拦截了嵌入页面：</span>
          <a id="qr-open-link" href="#" target="_blank" rel="noreferrer noopener">在新窗口打开二维码</a>
        </div>
        <div class="actions">
          <button id="refresh-btn" class="primary" type="button">重新生成二维码</button>
          <button id="accounts-btn" class="secondary" type="button">刷新账号状态</button>
        </div>
        <div id="detail" class="detail">页面会自动发起扫码接入。</div>
      </section>
    </section>

      <section class="panel accounts">
      <h2>当前微信账号</h2>
      <p>连接成功后，这里会显示当前已保存的微信账号和运行状态。若会话失效或暂停，可以直接在这里重新扫码接管。</p>
      <div id="account-list" class="account-list">
        <div class="empty">还没有检测到已连接账号。</div>
      </div>
    </section>
  </main>

  <script>
    const statusEl = document.getElementById("status");
    const detailEl = document.getElementById("detail");
    const qrImageEl = document.getElementById("qr-image");
    const qrPlaceholderEl = document.getElementById("qr-placeholder");
    const qrOpenLinkEl = document.getElementById("qr-open-link");
    const refreshBtn = document.getElementById("refresh-btn");
    const accountsBtn = document.getElementById("accounts-btn");
    const accountListEl = document.getElementById("account-list");

    const state = {
      sessionKey: null,
      pollToken: 0,
      busy: false,
      closed: false,
    };

    function setStatus(kind, text) {
      statusEl.className = "status " + kind;
      statusEl.textContent = text;
    }

    function setDetail(text) {
      detailEl.textContent = text;
    }

    function setQr(imageUrl, openUrl) {
      if (!imageUrl) {
        qrImageEl.hidden = true;
        qrImageEl.removeAttribute("src");
        qrOpenLinkEl.setAttribute("href", "#");
        qrPlaceholderEl.hidden = false;
        return;
      }
      qrImageEl.src = imageUrl;
      qrImageEl.hidden = false;
      qrOpenLinkEl.setAttribute("href", openUrl || imageUrl);
      qrPlaceholderEl.hidden = true;
    }

    function setBusy(busy) {
      state.busy = busy;
      refreshBtn.disabled = busy;
    }

    async function apiJson(url, options) {
      const response = await fetch(url, options);
      const raw = await response.text();
      let payload = null;
      try {
        payload = raw ? JSON.parse(raw) : null;
      } catch (_) {
        throw new Error(raw || ("HTTP " + response.status));
      }
      if (!response.ok) {
        throw new Error(payload && payload.error ? payload.error.message : (raw || ("HTTP " + response.status)));
      }
      return payload;
    }

    function renderAccounts(accounts) {
      if (!Array.isArray(accounts) || accounts.length === 0) {
        accountListEl.innerHTML = '<div class="empty">还没有检测到已连接账号。</div>';
        return;
      }

      accountListEl.innerHTML = accounts.map((account) => {
        const isPaused = Boolean(account.paused_until_ms) && Number(account.paused_until_ms) > Date.now();
        const statusClass = isPaused ? "pill paused" : (account.running ? "pill" : "pill offline");
        const statusText = isPaused ? "会话暂停中" : (account.running ? "在线监听中" : "未运行");
        const lastError = account.last_error ? '<div><strong>最近错误：</strong>' + escapeHtml(account.last_error) + '</div>' : '';
        const pausedUntil = account.paused_until_ms
          ? '<div><strong>暂停到：</strong>' + formatTimestamp(account.paused_until_ms) + '</div>'
          : '';
        const lastRestart = account.last_restart_at_ms
          ? '<div><strong>最近保活重启：</strong>' + formatTimestamp(account.last_restart_at_ms) + '</div>'
          : '';
        const restartButton = (!account.running || isPaused)
          ? '<button class="secondary account-button" type="button" data-account-action="reconnect" data-account-id="' + escapeHtml(account.account_id || "") + '">重新扫码接管</button>'
          : '<button class="secondary account-button" type="button" data-account-action="restart" data-account-id="' + escapeHtml(account.account_id || "") + '">重启监听</button>';
        return [
          '<article class="account-item">',
          '  <div class="account-top">',
          '    <strong>' + escapeHtml(account.account_id || "-") + '</strong>',
          '    <span class="' + statusClass + '">' + statusText + '</span>',
          '  </div>',
          '  <div><strong>微信用户：</strong>' + escapeHtml(account.linked_user_id || "-") + '</div>',
          '  <div><strong>最近事件：</strong>' + formatTimestamp(account.last_event_at_ms) + '</div>',
          pausedUntil,
          lastRestart,
          lastError,
          '  <div class="account-actions">' + restartButton + '</div>',
          '</article>'
        ].join("");
      }).join("");
    }

    function escapeHtml(text) {
      return String(text)
        .replaceAll("&", "&amp;")
        .replaceAll("<", "&lt;")
        .replaceAll(">", "&gt;")
        .replaceAll('"', "&quot;");
    }

    function formatTimestamp(value) {
      if (!value) {
        return "-";
      }
      const date = new Date(value);
      if (Number.isNaN(date.getTime())) {
        return String(value);
      }
      return date.toLocaleString("zh-CN", { hour12: false });
    }

    async function loadAccounts() {
      try {
        const accounts = await apiJson("/weixin/accounts", { method: "GET" });
        renderAccounts(accounts);
      } catch (error) {
        accountListEl.innerHTML = '<div class="empty">读取账号状态失败：' + escapeHtml(error.message) + '</div>';
      }
    }

    async function waitForLogin(sessionKey, pollToken) {
      while (!state.closed && state.sessionKey === sessionKey && state.pollToken === pollToken) {
        let payload;
        try {
          payload = await apiJson("/weixin/login/wait", {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: JSON.stringify({ session_key: sessionKey, timeout_ms: 10000 })
          });
        } catch (error) {
          setStatus("error", "连接状态查询失败");
          setDetail(error.message);
          setBusy(false);
          return;
        }

        if (state.sessionKey !== sessionKey || state.pollToken !== pollToken) {
          return;
        }

        if (payload.connected) {
          setStatus("success", "已连接成功");
          setDetail([
            payload.message || "与微信连接成功。",
            payload.account_id ? ("账号: " + payload.account_id) : null,
            payload.linked_user_id ? ("用户: " + payload.linked_user_id) : null
          ].filter(Boolean).join("\n"));
          setBusy(false);
          await loadAccounts();
          return;
        }

        const message = payload.message || "等待扫码确认...";
        setStatus("pending", "等待扫码确认");
        setDetail(message);

        if (message.includes("过期") || message.includes("没有进行中的微信登录会话")) {
          await startLogin(true);
          return;
        }
      }
    }

    async function startLogin(force, accountId) {
      state.pollToken += 1;
      const pollToken = state.pollToken;
      state.sessionKey = null;
      setBusy(true);
      setQr(null, null);
      setStatus("pending", "正在生成二维码...");
      setDetail(accountId
        ? ("正在为账号重新生成二维码: " + accountId)
        : "请稍候，页面会自动生成新的二维码。");

      let payload;
      try {
        payload = await apiJson("/weixin/login/start", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ force: Boolean(force), account_id: accountId || null })
        });
      } catch (error) {
        setStatus("error", "二维码生成失败");
        setDetail(error.message);
        setBusy(false);
        return;
      }

      if (state.pollToken !== pollToken) {
        return;
      }

      state.sessionKey = payload.session_key;
      setQr(payload.qr_code_data_url || null, payload.qr_code_url || null);
      setStatus("pending", "请扫码并确认");
      setDetail((payload.message || "二维码已生成。") + "\n会话: " + payload.session_key);
      await waitForLogin(payload.session_key, pollToken);
    }

    refreshBtn.addEventListener("click", () => {
      startLogin(true);
    });

    accountsBtn.addEventListener("click", () => {
      loadAccounts();
    });

    accountListEl.addEventListener("click", async (event) => {
      const button = event.target.closest("[data-account-action]");
      if (!button) {
        return;
      }
      const accountId = button.getAttribute("data-account-id");
      const action = button.getAttribute("data-account-action");
      if (!accountId) {
        return;
      }
      if (action === "reconnect") {
        startLogin(true, accountId);
        return;
      }
      if (action === "restart") {
        try {
          await apiJson("/weixin/accounts/" + encodeURIComponent(accountId) + "/restart", {
            method: "POST"
          });
          setStatus("pending", "监听已重启");
          setDetail("已请求重启账号监听: " + accountId);
          await loadAccounts();
        } catch (error) {
          setStatus("error", "监听重启失败");
          setDetail(error.message);
        }
      }
    });

    window.addEventListener("beforeunload", () => {
      state.closed = true;
    });

    loadAccounts();
    startLogin(false);
  </script>
</body>
</html>
"##
    .to_string()
}
