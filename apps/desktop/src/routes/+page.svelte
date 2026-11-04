<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import { onMount } from "svelte";

  type Task = {
    id: string;
    account_id: string;
    remote_id: string;
    title: string;
    course: string;
    kind: "assignment" | "exam" | "quiz" | string;
    due_at: number | null;
    starts_at: number | null;
    status: string;
    completed: boolean;
    url: string;
    source: string;
    fetched_at: number;
  };

  type Settings = {
    refresh_minutes: number;
    notifications_enabled: boolean;
    notify_lead_minutes: number[];
    autostart: boolean;
    always_on_top: boolean;
    risk_accepted: boolean;
  };

  type Account = {
    id: string;
    display_name: string;
    region: string;
    base_url: string;
    authenticated: boolean;
    last_success_at: number | null;
    last_error: string | null;
  };

  type Sync = {
    refreshing: boolean;
    last_attempt_at: number | null;
    last_success_at: number | null;
    last_error: string | null;
    needs_login: boolean;
    warnings: string[];
  };

  type Region = { id: string; label: string; base_url: string };
  type AdapterInfo = {
    id: string;
    name: string;
    version: string;
    api_version: number;
    source: string;
    path: string;
  };
  type AdapterSmokeResult = {
    adapter_id: string;
    adapter_version: string;
    complete: boolean;
    task_count: number;
    task_titles: string[];
  };
  type Dashboard = {
    tasks: Task[];
    account: Account | null;
    settings: Settings;
    sync: Sync;
    regions: Region[];
    log_path: string;
    version: string;
  };

  let dashboard = $state<Dashboard | null>(null);
  let loading = $state(true);
  let view = $state<"compact" | "settings">("compact");
  let now = $state(Date.now());
  let busy = $state("");
  let notice = $state("");
  let loginFlow = $state(false);
  let loginPending = $state(false);
  let selectedRegion = $state("www");
  let consent = $state(false);
  let showLogs = $state(false);
  let logLoading = $state(false);
  let logText = $state("");
  let adapters = $state<AdapterInfo[]>([]);
  let sdkBusy = $state(false);
  let sdkError = $state("");
  let sdkSmoke = $state<AdapterSmokeResult | null>(null);
  let initialized = false;

  const kindLabels: Record<string, string> = {
    assignment: "作业",
    exam: "考试",
    quiz: "测验",
  };

  let authenticated = $derived(dashboard?.account?.authenticated === true);
  let showLogin = $derived(!loading && (!authenticated || loginFlow));
  let activeTasks = $derived.by(() => {
    const tasks = dashboard?.tasks ?? [];
    return tasks
      .filter((task) => {
        if (task.completed) return false;
        const target = targetFor(task);
        return target !== null && target > now;
      })
      .sort((a, b) => (targetFor(a) ?? 0) - (targetFor(b) ?? 0));
  });
  let pastTasks = $derived.by(() => {
    const tasks = dashboard?.tasks ?? [];
    return tasks
      .filter((task) => {
        const target = targetFor(task);
        return task.completed || (target !== null && target <= now);
      })
      .sort((a, b) => (targetFor(b) ?? 0) - (targetFor(a) ?? 0));
  });
  let untimedTasks = $derived(
    (dashboard?.tasks ?? []).filter(
      (task) => !task.completed && targetFor(task) === null,
    ),
  );
  let pendingCount = $derived(activeTasks.length + untimedTasks.length);

  onMount(() => {
    const timer = window.setInterval(() => {
      now = Date.now();
    }, 1000);
    let unlistenState: UnlistenFn | undefined;
    let unlistenSettings: UnlistenFn | undefined;

    void loadDashboard();
    void loadAdapters();
    void (async () => {
      unlistenState = await listen("state-changed", () => {
        void loadDashboard();
      });
      unlistenSettings = await listen("ui://open-settings", () => {
        void openSettings();
      });
    })();

    return () => {
      window.clearInterval(timer);
      unlistenState?.();
      unlistenSettings?.();
    };
  });

  async function loadDashboard() {
    try {
      const data = await invoke<Dashboard>("bootstrap");
      dashboard = data;
      if (!initialized) {
        initialized = true;
        consent = data.settings.risk_accepted;
        if (data.account) selectedRegion = data.account.region;
      }
      loading = false;
    } catch (error) {
      loading = false;
      notice = `读取应用状态失败: ${String(error)}`;
    }
  }

  function targetFor(task: Task): number | null {
    if (task.kind === "exam") {
      if (task.starts_at && task.starts_at > now) return task.starts_at;
      if (task.due_at && task.due_at > now) return task.due_at;
      return task.starts_at ?? task.due_at;
    }
    return task.due_at ?? task.starts_at;
  }

  function isOngoing(task: Task): boolean {
    return (
      task.kind === "exam" &&
      task.starts_at !== null &&
      task.starts_at <= now &&
      task.due_at !== null &&
      task.due_at > now
    );
  }

  function countdownText(task: Task): string {
    if (task.completed) return "已完成";
    const target = targetFor(task);
    if (target === null) return "—";
    const diff = target - now;
    if (diff <= 0) return "已截止";
    const seconds = Math.floor(diff / 1000);
    const minutes = Math.floor(seconds / 60);
    const hours = Math.floor(minutes / 60);
    const days = Math.floor(hours / 24);
    if (minutes < 10) return `${Math.max(1, minutes)}分${seconds % 60}秒`;
    if (hours < 1) return `${minutes}分钟`;
    if (days < 1) return `${hours}小时${minutes % 60}分`;
    return `${days}天${hours % 24}小时`;
  }

  function countdownLabel(task: Task): string {
    if (task.completed) return "已完成";
    if (isOngoing(task)) return "进行中 · 距结束";
    if (task.kind === "exam") return "距开考";
    return "距截止";
  }

  function urgencyClass(task: Task): string {
    if (task.completed) return "overdue";
    const target = targetFor(task);
    if (target === null) return "";
    const diff = target - now;
    if (diff <= 0) return "overdue";
    if (diff <= 60 * 60 * 1000) return "urgent";
    if (diff <= 24 * 60 * 60 * 1000) return "soon";
    return "";
  }

  function taskClass(task: Task): string {
    const urgency = urgencyClass(task);
    if (urgency === "urgent") return "task is-urgent";
    if (urgency === "soon") return "task is-soon";
    if (task.completed || (targetFor(task) ?? 0) <= now) return "task is-muted";
    return "task";
  }

  function formatDateTime(ms: number | null): string {
    if (!ms) return "—";
    return new Intl.DateTimeFormat("zh-CN", {
      month: "numeric",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    }).format(new Date(ms));
  }

  function formatFreshness(ms: number | null): string {
    if (!ms) return "尚未同步";
    const diff = Math.max(0, now - ms);
    if (diff < 60_000) return "刚刚更新";
    if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} 分钟前更新`;
    if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} 小时前更新`;
    return `${Math.floor(diff / 86_400_000)} 天前更新`;
  }

  function kindLabel(kind: string): string {
    return kindLabels[kind] ?? "任务";
  }

  function statusSummary(): string {
    if (!dashboard) return "";
    if (dashboard.sync.refreshing) return "同步中…";
    if (dashboard.sync.last_error) return "同步异常";
    if (dashboard.sync.needs_login) return "需要重新登录";
    return dashboard.sync.last_success_at ? "已同步" : "等待首次同步";
  }

  async function refreshNow() {
    if (busy) return;
    busy = "refresh";
    notice = "";
    try {
      await invoke("refresh_now");
      await loadDashboard();
    } catch (error) {
      notice = `刷新失败: ${String(error)}`;
      await loadDashboard();
    } finally {
      busy = "";
    }
  }

  async function startLogin() {
    if (!dashboard) return;
    if (!consent) {
      notice = "请先确认只读使用边界。";
      return;
    }
    busy = "login";
    notice = "";
    try {
      if (!dashboard.settings.risk_accepted) {
        dashboard = await invoke<Dashboard>("update_settings", {
          settings: { ...dashboard.settings, risk_accepted: true },
        });
      }
      await invoke("open_login", { regionId: selectedRegion });
      loginPending = true;
    } catch (error) {
      notice = `打开登录窗口失败: ${String(error)}`;
    } finally {
      busy = "";
    }
  }

  async function finishLogin() {
    if (busy) return;
    busy = "finish-login";
    notice = "";
    try {
      const data = await invoke<Dashboard>("finish_login", {
        regionId: selectedRegion,
      });
      dashboard = data;
      loginPending = false;
      loginFlow = false;
      view = "compact";
      await invoke("set_view", { view: "compact" });
    } catch (error) {
      notice = `登录验证失败: ${String(error)}`;
    } finally {
      busy = "";
    }
  }

  async function closeLogin() {
    await invoke("close_login").catch(() => {});
    loginPending = false;
  }

  async function cancelLoginFlow() {
    if (loginPending) {
      await closeLogin();
    }
    loginFlow = false;
    notice = "";
  }

  async function openSettings() {
    if (!dashboard) {
      await loadDashboard();
    }
    if (!dashboard) return;
    view = "settings";
    await invoke("set_view", { view: "settings" }).catch(() => {});
  }

  async function closeSettings() {
    view = "compact";
    await invoke("set_view", { view: "compact" }).catch(() => {});
  }

  async function applySettings(patch: Partial<Settings>) {
    if (!dashboard || busy) return;
    busy = "settings";
    notice = "";
    try {
      const data = await invoke<Dashboard>("update_settings", {
        settings: { ...dashboard.settings, ...patch },
      });
      dashboard = data;
      consent = data.settings.risk_accepted;
    } catch (error) {
      notice = `保存设置失败: ${String(error)}`;
    } finally {
      busy = "";
    }
  }

  async function openTask(task: Task) {
    notice = "";
    try {
      await invoke("open_task", { taskId: task.id });
    } catch (error) {
      notice = `打开任务失败: ${String(error)}`;
    }
  }

  async function logout() {
    if (!dashboard?.account) return;
    if (!window.confirm("删除本地会话与任务缓存，并退出当前账号？")) return;
    busy = "logout";
    notice = "";
    try {
      const data = await invoke<Dashboard>("logout");
      dashboard = data;
      view = "compact";
      loginFlow = false;
      loginPending = false;
      showLogs = false;
      await invoke("set_view", { view: "compact" }).catch(() => {});
    } catch (error) {
      notice = `退出失败: ${String(error)}`;
    } finally {
      busy = "";
    }
  }

  async function loadAdapters() {
    try {
      adapters = await invoke<AdapterInfo[]>("list_adapters");
      sdkError = "";
    } catch (error) {
      sdkError = `读取适配器失败: ${String(error)}`;
    }
  }

  async function runSdkSmoke() {
    if (sdkBusy) return;
    sdkBusy = true;
    sdkError = "";
    sdkSmoke = null;
    try {
      sdkSmoke = await invoke<AdapterSmokeResult>("run_adapter_smoke");
    } catch (error) {
      sdkError = `SDK 自检失败: ${String(error)}`;
    } finally {
      sdkBusy = false;
    }
  }

  async function loadLogs() {
    logLoading = true;
    try {
      logText = await invoke<string>("get_recent_logs", { lines: 250 });
    } catch (error) {
      logText = String(error);
    } finally {
      logLoading = false;
    }
  }

  async function toggleLogs() {
    showLogs = !showLogs;
    if (showLogs) await loadLogs();
  }

  async function openLogDir() {
    try {
      await invoke("open_log_dir");
    } catch (error) {
      notice = `打开日志目录失败: ${String(error)}`;
    }
  }

  async function beginWindowDrag(event: PointerEvent) {
    if (event.button !== 0) return;
    const target = event.target as HTMLElement | null;
    if (
      target?.closest(
        "button, a, input, select, textarea, [data-no-drag]",
      )
    ) {
      return;
    }
    await invoke("start_window_drag").catch(() => {});
  }

  async function hideWindow() {
    await invoke("hide_main_window").catch(() => {});
  }

  async function minimizeWindow() {
    await invoke("minimize_main_window").catch(() => {});
  }

  async function quitApp() {
    await invoke("quit_app").catch(() => {});
  }
</script>

<div class="app">
  <header
    class="titlebar"
    role="toolbar"
    aria-label="窗口标题栏"
    tabindex="-1"
    onpointerdown={beginWindowDrag}
  >
    <div class="brand">
      {#if view === "settings"}
        <button
          class="icon-button"
          title="返回"
          aria-label="返回"
          onclick={closeSettings}>←</button
        >
        <span>设置</span>
      {:else}
        <span class="brand-mark">DDL</span>
        <span>课业倒计时</span>
        <span class="brand-sub">{authenticated ? "雨课堂 · 只读" : "只读提醒"}</span>
      {/if}
    </div>
    <div class="titlebar-actions">
      {#if authenticated && view === "compact"}
        <button
          class="icon-button"
          title="刷新"
          aria-label="刷新"
          disabled={Boolean(busy) || dashboard?.sync.refreshing}
          onclick={refreshNow}
        ><span class={dashboard?.sync.refreshing ? "spin" : ""}>⟳</span></button
        >
        <button
          class="icon-button"
          title="设置"
          aria-label="设置"
          onclick={openSettings}>⚙</button
        >
      {/if}
      <button
        class="icon-button"
        title="最小化"
        aria-label="最小化"
        onclick={minimizeWindow}>–</button
      >
      <button
        class="icon-button"
        title="隐藏到托盘"
        aria-label="隐藏到托盘"
        onclick={hideWindow}>×</button
      >
    </div>
  </header>

  <main class="content">
    {#if loading}
      <div class="empty">正在读取本地状态…</div>
    {:else if showLogin}
      <div class="login-wrap">
        <section class="login-card">
          <span class="brand-mark">DDL</span>
          <h1>{dashboard?.account ? "重新登录雨课堂" : "连接雨课堂"}</h1>
          <p class="lead">
            内嵌浏览器登录后只读取作业/考试元数据与截止时间。应用不保存密码。
          </p>

          {#if loginPending}
            <div class="login-pending">
              登录窗口已经打开。
              <ol>
                <li>在登录窗口完成扫码或学校 SSO；</li>
                <li>保持登录窗口打开，回到这里点击“我已完成登录”；</li>
                <li>应用会读取会话并拉取课程任务。</li>
              </ol>
            </div>
            <div class="login-actions">
              <button
                class="primary"
                disabled={Boolean(busy)}
                onclick={finishLogin}
              >{busy === "finish-login" ? "正在拉取任务…" : "我已完成登录"}</button
              >
              <button class="ghost" disabled={Boolean(busy)} onclick={closeLogin}
                >关闭登录窗口</button
              >
              {#if authenticated && loginFlow}
                <button class="ghost" disabled={Boolean(busy)} onclick={cancelLoginFlow}
                  >取消</button
                >
              {/if}
            </div>
          {:else}
            <div class="risk-box">
              <strong>非官方只读工具。</strong>
              仅使用你本人账号，只读取作业/考试标题、课程、时间和链接；不读取题目/答案、不提交作业、不绕过验证码，默认 30 分钟低频刷新。平台没有公开学生 API，雨课堂用户协议禁止抓取/反向工程，因此仍存在账号受限和接口随时失效的风险。请自行评估并承担使用风险。
            </div>

            <label class="field">
              <span>雨课堂入口</span>
              <select
                bind:value={selectedRegion}
                disabled={Boolean(busy) || loginPending}
              >
                {#each dashboard?.regions ?? [] as region (region.id)}
                  <option value={region.id}>{region.label}</option>
                {/each}
              </select>
            </label>

            <label class="consent">
              <input type="checkbox" bind:checked={consent} />
              <span>我已阅读并同意仅使用本人账号、只读元数据，并理解账号风险和接口失效风险。</span>
            </label>

            <div class="login-actions">
              <button
                class="primary"
                disabled={Boolean(busy) || !consent}
                onclick={startLogin}
              >{busy === "login" ? "正在打开登录窗口…" : "打开登录窗口"}</button
              >
              {#if dashboard?.account}
                <button class="ghost" disabled={Boolean(busy)} onclick={logout}
                  >删除会话</button
                >
              {/if}
              {#if authenticated && loginFlow}
                <button class="ghost" disabled={Boolean(busy)} onclick={cancelLoginFlow}
                  >取消</button
                >
              {/if}
            </div>
            {#if dashboard?.account && !authenticated}
              <p class="lead" style="margin: 12px 0 0;">
                已有账号记录，但本地会话缺失或已失效。请重新登录。
              </p>
            {/if}
          {/if}

          {#if notice}
            <div class="notice">{notice}</div>
          {/if}
        </section>
      </div>
    {:else if view === "settings"}
      <section class="settings">
        <h1>设置</h1>
        <p class="lead">配置刷新、提醒和窗口行为。所有数据保存在本机。</p>

        {#if notice}
          <div class="notice">{notice}</div>
        {/if}

        <div class="settings-grid">
          <section class="settings-card">
            <h2>同步</h2>
            <div class="setting-row">
              <span>
                刷新间隔
                <span class="setting-hint">默认 30 分钟；不会自动提交任何内容</span>
              </span>
              <select
                value={dashboard?.settings.refresh_minutes}
                disabled={Boolean(busy)}
                onchange={(event) =>
                  applySettings({
                    refresh_minutes: Number(event.currentTarget.value),
                  })}
              >
                <option value={15}>15 分钟</option>
                <option value={30}>30 分钟</option>
                <option value={60}>1 小时</option>
                <option value={120}>2 小时</option>
              </select>
            </div>
            <div class="setting-row">
              <span>系统通知 <span class="setting-hint">24h / 6h / 1h / 10m 阈值</span></span>
              <button
                class="toggle"
                class:on={dashboard?.settings.notifications_enabled}
                aria-label="切换系统通知"
                aria-pressed={dashboard?.settings.notifications_enabled}
                disabled={Boolean(busy)}
                onclick={() =>
                  applySettings({
                    notifications_enabled:
                      !dashboard?.settings.notifications_enabled,
                  })}
              ></button>
            </div>
            <div class="setting-row">
              <span>开机启动</span>
              <button
                class="toggle"
                class:on={dashboard?.settings.autostart}
                aria-label="切换开机启动"
                aria-pressed={dashboard?.settings.autostart}
                disabled={Boolean(busy)}
                onclick={() =>
                  applySettings({ autostart: !dashboard?.settings.autostart })}
              ></button>
            </div>
            <div class="setting-row">
              <span>窗口置顶</span>
              <button
                class="toggle"
                class:on={dashboard?.settings.always_on_top}
                aria-label="切换窗口置顶"
                aria-pressed={dashboard?.settings.always_on_top}
                disabled={Boolean(busy)}
                onclick={() =>
                  applySettings({
                    always_on_top: !dashboard?.settings.always_on_top,
                  })}
              ></button>
            </div>
            <div class="settings-actions">
              <button
                class="ghost"
                disabled={Boolean(busy) || dashboard?.sync.refreshing}
                onclick={refreshNow}>立即刷新</button
              >
              <span class="account-meta">
                {formatFreshness(dashboard?.sync.last_success_at ?? null)}
              </span>
            </div>
          </section>

          <section class="settings-card">
            <h2>账号</h2>
            <div class="account-row">
              <div class="account-avatar">雨</div>
              <div>
                <div class="account-name">
                  {dashboard?.account?.display_name ?? "未连接"}
                </div>
                <div class="account-meta">
                  {dashboard?.account?.base_url ?? "请先登录雨课堂"}
                </div>
              </div>
            </div>
            <div class="account-actions">
              {#if authenticated}
                <button
                  class="ghost"
                  disabled={Boolean(busy)}
                  onclick={() => {
                    loginFlow = true;
                    view = "compact";
                    invoke("set_view", { view: "compact" }).catch(() => {});
                  }}>重新登录</button
                >
                <button class="danger" disabled={Boolean(busy)} onclick={logout}
                  >退出并删除会话</button
                >
              {:else}
                <button
                  class="primary"
                  disabled={Boolean(busy)}
                  onclick={() => {
                    loginFlow = true;
                    view = "compact";
                    invoke("set_view", { view: "compact" }).catch(() => {});
                  }}>登录</button
                >
              {/if}
            </div>
            {#if dashboard?.sync.last_error}
              <div class="banner error" style="margin-top:12px; margin-bottom:0;">
                {dashboard.sync.last_error}
              </div>
            {/if}
          </section>

          <section class="settings-card full">
            <h2>适配器 SDK（运行时预览）</h2>
            <div class="account-meta">
              当前版本已接入 QuickJS 受控运行时和 manifest 加载。雨课堂仍走内置 Rust 适配器；
              签名安装与远程热更新尚未启用。
            </div>
            <div style="display:grid; gap:6px; margin-top:10px;">
              {#each adapters as adapter (adapter.id)}
                <div class="setting-row">
                  <span>
                    <strong>{adapter.name}</strong>
                    <span class="setting-hint">
                      {adapter.id} · v{adapter.version} · API v{adapter.api_version} · {adapter.source}
                    </span>
                  </span>
                  <span class="account-meta">manifest OK</span>
                </div>
              {:else}
                <div class="account-meta">没有找到内置适配器包。</div>
              {/each}
            </div>
            <div class="settings-actions">
              <button class="ghost" disabled={sdkBusy} onclick={runSdkSmoke}>
                {sdkBusy ? "运行中…" : "运行 mock 自检"}
              </button>
              <button class="ghost" disabled={sdkBusy} onclick={loadAdapters}
                >重新扫描</button
              >
            </div>
            {#if sdkError}
              <div class="banner error" style="margin:10px 0 0;">{sdkError}</div>
            {/if}
            {#if sdkSmoke}
              <div class="banner" style="margin:10px 0 0;">
                SDK 自检成功：{sdkSmoke.adapter_id} v{sdkSmoke.adapter_version} 返回
                {sdkSmoke.task_count} 个任务（complete={String(sdkSmoke.complete)}）。
                {#if sdkSmoke.task_titles.length > 0}
                  标题：{sdkSmoke.task_titles.join("、")}
                {/if}
              </div>
            {/if}
          </section>

          <section class="settings-card full">
            <h2>日志与诊断</h2>
            <div class="account-meta">
              日志会脱敏 Cookie、token 与授权头；不包含题目或答案。路径：
              {dashboard?.log_path ?? "—"}
            </div>
            <div class="settings-actions">
              <button class="ghost" onclick={toggleLogs}>
                {showLogs ? "隐藏最近日志" : "查看最近日志"}
              </button>
              <button class="ghost" disabled={logLoading} onclick={loadLogs}
                >刷新日志</button
              >
              <button class="ghost" onclick={openLogDir}>打开日志目录</button>
            </div>
            {#if showLogs}
              <pre class="logs">{logLoading ? "加载中…" : logText || "暂无日志"}</pre>
            {/if}
          </section>

          <section class="settings-card full">
            <h2>关于</h2>
            <div class="setting-row">
              <span>版本</span><span>{dashboard?.version ?? "—"}</span>
            </div>
            <div class="setting-row">
              <span>数据范围</span><span>本人账号 · 只读 · 本机存储</span>
            </div>
            <div class="settings-actions">
              <button class="danger" onclick={quitApp}>退出应用</button>
            </div>
          </section>
        </div>
        <div class="footer-note">
          非官方客户端，不保证接口长期稳定；请自行评估平台条款与账号风险。
        </div>
      </section>
    {:else}
      <div class="status-line">
        <span>
          <span
            class="dot"
            class:error={dashboard?.sync.last_error}
            class:syncing={dashboard?.sync.refreshing}
          ></span>
          {statusSummary()}
        </span>
        <span class="freshness" title={formatDateTime(dashboard?.sync.last_success_at ?? null)}>
          {formatFreshness(dashboard?.sync.last_success_at ?? null)}
        </span>
      </div>

      {#if notice}
        <div class="banner error">{notice}</div>
      {/if}
      {#if dashboard?.sync.last_error}
        <div class="banner error">{dashboard.sync.last_error}</div>
      {/if}
      {#if (dashboard?.sync.warnings.length ?? 0) > 0}
        <div class="banner warning">
          部分课程未完整刷新，已保留上次成功结果（{dashboard?.sync.warnings.length} 条警告）。
        </div>
      {/if}
      {#if dashboard?.sync.needs_login}
        <div class="banner warning needs-login">
          <span>登录已失效，任务已缓存但无法继续刷新。</span>
          <button
            class="ghost"
            onclick={() => {
              loginFlow = true;
            }}>重新登录</button
          >
        </div>
      {/if}

      {#if activeTasks.length === 0 && untimedTasks.length === 0 && pastTasks.length === 0}
        <div class="empty">
          <strong>暂无带截止时间的任务</strong>
          刷新后，雨课堂当前学期的作业和考试会显示在这里。
        </div>
      {:else}
        {#if activeTasks.length === 0 && untimedTasks.length === 0}
          <div class="empty">
            <strong>暂无即将到期的任务</strong>
            历史任务保留在下方。
          </div>
        {/if}
        <div class="task-list">
          {#each activeTasks as task (task.id)}
            <button
              class={taskClass(task)}
              title="在系统浏览器中打开；如未登录请在浏览器内登录"
              onclick={() => openTask(task)}
            >
              <div class="task-main">
                <div class="task-kicker">
                  <span class="platform-chip">雨</span>
                  <span class="course-name">{task.course || "未命名课程"}</span>
                  <span class="kind">{kindLabel(task.kind)}</span>
                </div>
                <p class="task-title">{task.title}</p>
                <div class="task-meta">
                  {isOngoing(task) ? "进行中" : task.kind === "exam" ? "开考" : "截止"} {formatDateTime(
                    isOngoing(task) ? task.due_at : targetFor(task),
                  )}
                  {#if task.status && task.status !== "pending"}
                    · {task.status}
                  {/if}
                </div>
              </div>
              <div class="task-side">
                <span class="countdown {urgencyClass(task)}">{countdownText(task)}</span>
                <span class="countdown-label">{countdownLabel(task)}</span>
              </div>
            </button>
          {/each}
        </div>

        {#if untimedTasks.length > 0}
          <div class="section-title"><span>无截止时间</span></div>
          <div class="task-list">
            {#each untimedTasks as task (task.id)}
              <button class={taskClass(task)} onclick={() => openTask(task)}>
                <div class="task-main">
                  <div class="task-kicker">
                    <span class="platform-chip">雨</span>
                    <span class="course-name">{task.course}</span>
                    <span class="kind">{kindLabel(task.kind)}</span>
                  </div>
                  <p class="task-title">{task.title}</p>
                </div>
                <div class="task-side">
                  <span class="countdown overdue">—</span>
                  <span class="countdown-label">无时间</span>
                </div>
              </button>
            {/each}
          </div>
        {/if}

        {#if pastTasks.length > 0}
          <details class="past">
            <summary>已截止 / 已完成（{pastTasks.length}）</summary>
            <div class="task-list">
              {#each pastTasks as task (task.id)}
                <button class={taskClass(task)} onclick={() => openTask(task)}>
                  <div class="task-main">
                    <div class="task-kicker">
                      <span class="platform-chip">雨</span>
                      <span class="course-name">{task.course}</span>
                      <span class="kind">{kindLabel(task.kind)}</span>
                    </div>
                    <p class="task-title">{task.title}</p>
                    <div class="task-meta">
                      截止 {formatDateTime(task.due_at)}
                    </div>
                  </div>
                  <div class="task-side">
                    <span class="countdown overdue">
                      {task.completed ? "已完成" : "已截止"}
                    </span>
                  </div>
                </button>
              {/each}
            </div>
          </details>
        {/if}
      {/if}
      <div class="footer-note">
        只读提醒 · 任务在系统浏览器打开 · 不提交、答题或签到
      </div>
    {/if}
  </main>
</div>
