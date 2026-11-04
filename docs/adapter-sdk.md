# 平台适配器 SDK 规范（v1 设计稿）

> **重要状态说明（2026-11-04）**
>
> 当前 `v0.1.0` 已接入 **QuickJS 受控运行时、manifest 校验、雨课堂/mock 适配器、
> 设置页自检和本地 Ed25519 签名包安装（开发公钥）**。雨课堂的定时刷新和登录验证
> 都通过 SDK runtime 执行；内置 Rust 实现保留为加载失败回退。
> **远程目录、自动更新和生产密钥轮换尚未完成**。
>
> 本文既是当前边界的说明，也是后续 `v1` SDK 的目标契约。下面的远程/自动更新能力
> 尚未启用；本地签名安装已经可用。

## 1. 目标

1. **统一接口**：所有平台都返回相同的任务模型，UI/调度/存储不需要知道平台差异。
2. **隔离**：平台适配器不直接接触数据库、窗口、文件系统、进程或原始 Cookie。
3. **可更新**：平台改版时，只更新适配器包，不强制更新主程序。
4. **可验证**：适配器包可签名、校验、回滚；不静默安装未签名代码。
5. **轻量**：不为每个平台引入 Chromium/Node；使用内置受控 JS 运行时。
6. **可测试**：适配器可以用脱敏 fixtures 做契约测试，不依赖真实账号。

## 2. 当前实现与目标实现的差距

| 能力 | 当前 v0.1.0 | 目标 v1 |
|---|---|---|
| 适配器位置 | `adapters/yuketang` + `adapters/mock` JS 包；Rust 作为回退 | 所有平台独立签名包 |
| 添加平台 | 新增包 + `load_by_id` 注册；当前雨课堂/mock 已走 JS | 编写适配器包、安装 |
| 运行时 | 雨课堂/mock QuickJS；Rust 回退 | 全部平台 QuickJS 受控运行时 |
| 签名 | 本地 Ed25519 校验（开发公钥） | Ed25519 包签名 + 生产密钥 |
| 热更新 | 本地 `.sdapkg` 安装 | 签名目录 + 远程安装 + 自动回滚 |
| UI 隔离 | 无 | 适配器无窗口/文件/进程权限 |

### 当前已实现的最小闭环

- `adapters/yuketang` 与 `adapters/mock` 是内置 QuickJS 包；
- `src-tauri/src/adapters/manifest.rs` 负责 manifest 校验、包目录加载；
- `src-tauri/src/adapters/runtime.rs` 负责 QuickJS 执行、HTTP/session/log/config/sleep
  受控 API、任务字段/URL 校验和归一化；
- `src-tauri/src/refresh.rs` 通过 `load_by_id` 选择最新兼容适配器并执行；雨课堂和 mock
  的实时刷新/登录验证均已走 runtime；
- `src-tauri/src/adapters/install.rs` 实现 Ed25519 签名校验、版本化安装和降级阻断；
- `scripts/adapter-keygen.mjs` / `scripts/adapter-pack.mjs` 可生成密钥并打包 `.sdapkg`；
- 设置页的“适配器 SDK（运行时预览）”可运行 mock 自检、查看内置/已安装包并安装 `.sdapkg`；
- 单测覆盖 manifest、URL 白名单、cookie 解析、签名校验、防篡改、安装和两套适配器加载。

已安装的适配器使用更高 `sequence` 时会被 `load_by_id` 选中，从而可覆盖内置包；
面向用户的账号创建/平台选择 UI 仍需完善。Rust 实现保留为适配器加载失败时的回退。

## 3. 总体架构

```text
宿主 Host（Tauri / Rust）
├─ Adapter Registry       发现内置/已安装版本，兼容性检查
├─ Adapter Runtime        QuickJS，一个账号任务一个专用 OS 线程
├─ Host HTTP              域名/路径白名单、Cookie 注入、限流、超时
├─ Session Store          WebView 登录 → 加密 Cookie → 宿主注入
├─ Package Installer      验签、解压、版本目录、原子切换、回滚
├─ Task Validator         Rust 侧再次校验输出
└─ UI / Scheduler         只消费 Task

适配器包 Adapter（JS）
manifest.json + index.js
├─ checkSession(ctx)
├─ fetchTasks(ctx)
└─ resolveTaskUrl(ctx, task)
```

## 4. 统一任务模型

适配器返回归一化任务，而不是平台页面结构：

```ts
type TaskKind = "assignment" | "exam" | "quiz";

type TaskDraft = {
  remoteId: string;       // 平台内稳定 ID；必须避免仅用标题
  title: string;
  course: string;
  kind: TaskKind;
  dueAt?: number;         // Unix 毫秒；无截止时间则省略
  startsAt?: number;      // 考试开始时间，Unix 毫秒
  status?: string;        // 平台状态或归一化状态
  completed?: boolean;
  url?: string;           // 精确页面；当前允许课堂详情页
  description?: string;
};

type FetchResult = {
  tasks: TaskDraft[];
  complete: boolean;      // 只有完整成功快照才为 true
};
```

规则：

- **时间统一为 Unix 毫秒**；适配器负责把秒/毫秒、无时区字符串统一。
- 无截止时间的任务可返回，但不得进入紧急排序或提醒。
- `remoteId` 应在平台内稳定；建议使用课程 + 叶子/活动 ID，不使用标题哈希。
- `complete=false` 时宿主保留旧任务，不删除未返回项。
- 不返回题目、答案、试卷内容、作业正文或用户隐私数据。
- `url` 只允许 HTTPS，并必须属于宿主允许的域名。

## 5. 适配器接口

```ts
export default defineAdapter({
  checkSession(ctx): boolean {
    // 可选：用一次轻量只读请求验证会话
    return true;
  },

  fetchTasks(ctx): FetchResult {
    const courses = ctx.http.getJson<CoursesResponse>(
      `${ctx.config.baseUrl}/...`,
    );
    // 只读解析，返回统一任务
    return { tasks, complete };
  },

  resolveTaskUrl(ctx, task): string {
    return task.url ?? `${ctx.config.baseUrl}/...`;
  },
});
```

设计为**同步接口**：每次拉取在专用线程执行；HTTP 调用在该线程同步完成。
这样可以避免 QuickJS Promise/AsyncContext 桥接复杂度，也便于设置硬超时。

### 5.1 Host Context

适配器只能拿到以下能力：

```ts
interface AdapterContext {
  http: {
    request(req: HttpRequest): HttpResponse;
    getJson<T>(url: string, options?: RequestOptions): T;
    getText(url: string, options?: RequestOptions): string;
  };
  session: {
    get(key: string): string | null;
  };
  log: {
    debug(message: string, fields?: Record<string, unknown>): void;
    info(message: string, fields?: Record<string, unknown>): void;
    warn(message: string, fields?: Record<string, unknown>): void;
    error(message: string, fields?: Record<string, unknown>): void;
  };
  crypto: {
    sha256(input: string): string;
    md5(input: string): string;
    hmacSha256(key: string, data: string): string;
    base64Encode(input: string): string;
    base64Decode(input: string): string;
  };
  config: {
    region: string;
    baseUrl: string;
    timezone: string;
    locale: string;
  };
}
```

禁止暴露：

- 原始 Cookie jar；
- 文件系统、网络 socket、进程/命令执行；
- 宿主数据库、窗口、托盘、Tauri IPC；
- 其他适配器或账号的会话。

HTTP 宿主负责：

- 自动注入当前账号的 Cookie 与 UA；
- 域名/路径前缀白名单；
- 只允许 HTTPS；不跟随跨域重定向；
- 请求超时、响应大小、解压大小限制；
- 每账号串行限速与 `Retry-After`；
- 脱敏日志。

适配器可以设置平台需要的普通请求头（如 `xtbz`、`X-CSRFToken`、`Referer`、
`classroom-id`），但不能直接设置/读取原始 Cookie。敏感 token 只能通过 manifest
声明的 `session.get()` 获取，且日志必须脱敏。

## 6. manifest.json

```json
{
  "schemaVersion": 1,
  "id": "example-platform",
  "name": "示例平台",
  "version": "1.0.0",
  "apiVersion": 1,
  "minHostVersion": "0.2.0",
  "maxHostVersion": "0.9.0",
  "keyId": "example-key-2026",
  "sequence": 1,
  "entry": "index.js",
  "icon": "icon.png",
  "permissions": {
    "http": [
      "https://www.example.edu/*",
      "https://*.example.edu/*"
    ],
    "session": [
      "cookie:csrftoken",
      "localStorage:auth_token"
    ]
  },
  "auth": {
    "loginUrl": "https://www.example.edu/login",
    "allowedOrigins": [
      "https://www.example.edu",
      "https://*.example.edu"
    ],
    "successUrlPatterns": ["/dashboard"],
    "sessionCookies": ["sessionid", "csrftoken"]
  }
}
```

字段：

| 字段 | 说明 |
|---|---|
| `apiVersion` | 适配器接口主版本；宿主只加载兼容版本 |
| `minHostVersion` / `maxHostVersion` | 主机版本范围 |
| `keyId` | 签名公钥标识，支持密钥轮换 |
| `sequence` | 单调递增，用于阻止降级 |
| `permissions.http` | 允许访问的域名/路径模式 |
| `permissions.session` | 允许读取的 Cookie/localStorage 键名 |
| `auth.loginUrl` | 登录窗口入口 |
| `successUrlPatterns` | 可选的成功页面模式；必须辅以实际接口验证 |
| `sessionCookies` | 登录后允许提取的 Cookie 名 |

不允许：

- `permissions` 之外的域名；
- `http://`、`file://`、`javascript:` 或自定义协议；
- 通配符匹配到非目标主域；
- manifest 未声明的 session key；
- 任意系统权限。

## 7. 登录

宿主统一完成：

1. 创建无 IPC 权限的独立登录 WebView；
2. 导航到 `auth.loginUrl`，只允许 HTTPS 白名单；
3. 用户手动扫码/SSO/验证码；
4. 在登录 WebView 存活时，用 Tauri Rust Cookie API 读取声明的 Cookie；
5. 可选读取 manifest 声明的 localStorage token（受控桥）；
6. 用适配器提供的一次轻量验证请求确认会话；
7. AES-256-GCM 加密，密文入 SQLite，主密钥入 OS 凭据库；
8. 登录窗口关闭，宿主注入 Cookie 给适配器 HTTP。

禁止：

- 保存密码；
- 自动识别/绕过验证码；
- 向第三方发送 Cookie；
- 把登录 WebView 暴露给适配器任意执行任意 JS；
- 将裸 Cookie 传给 UI/远程页面。

## 8. 运行时与安全

目标运行时：**QuickJS（rquickjs）**，原因是：

- 体积小，适合轻量桌面；
- TypeScript 适配器开发成本低；
- 可以严格限制宿主 API；
- 签名后的第一方适配器风险可控。

执行约束：

- 每个运行使用独立 OS 线程；
- esbuild 打包成 IIFE 单文件；宿主 `eval` 后取 `globalThis.adapter`；
- 不使用 QuickJS module loader 或 AsyncContext 作为 MVP 依赖；
- 设置内存上限、栈上限、执行超时、HTTP 单请求超时；
- 适配器无文件/进程/原生网络能力；
- 返回的任务在 Rust 侧再次校验字段、类型、时间和域名；
- QuickJS 是“签名第一方代码的护栏”，不是敌对代码的强沙箱。若未来开放第三方
  不受信插件，应迁移至 WASM（Extism/wasmtime）或独立进程。

## 9. 签名与安装

### 9.1 包结构

```text
adapter-id-1.0.0.zip
├─ manifest.json
├─ index.js
└─ icon.png
```

- `index.js` 必须是打包后的单文件，不引用 `node_modules`。
- 包内禁止符号链接、绝对路径、`..` 路径。
- 限制包大小、文件数和解压后大小（防 zip bomb）。

### 9.2 签名

目标流程：

1. 计算完整包（含 manifest）的 SHA-256；
2. 用 Ed25519 对规范化 manifest + 包哈希签名；
3. 安装前验证 `keyId`、签名、哈希、`apiVersion`、主机版本、`sequence`；
4. 验签通过后再解压；
5. 安装到版本化目录，SQLite 指针原子切换；
6. 保留上一版本；加载失败/多次运行时失败自动回滚；
7. 拒绝降级包；远程目录本身也需要签名。

密钥管理：

- 适配器签名密钥独立于主程序更新密钥；
- 私钥离线或放在受保护的发布任务，不进入普通 CI；
- 公钥内置应用；支持多公钥轮换；
- 生产模式只加载签名包；debug 构建才允许显式 `--adapter-dev-dir`。

本地实现状态：

- 已实现 Ed25519 签名校验、`.sdapkg` JSON 包、版本化安装、重复拒绝和 sequence 降级阻断；
- 当前内置 `dev-2026` 开发公钥；
- 尚未实现远程目录、自动更新、密钥轮换 UI 和生产密钥发布流程。

### 本地签名快速开始

```bash
# 1. 生成开发密钥（私钥写入 signing/，已 gitignore）
node scripts/adapter-keygen.mjs dev-2026 signing/adapter-dev-key.json

# 2. 打包一个适配器；manifest 必须包含 keyId 和 sequence
node scripts/adapter-pack.mjs adapters/mock signing/adapter-dev-key.json /tmp/mock.sdapkg

# 3. 在应用设置 → 适配器 SDK → 安装本地签名适配器，选择 /tmp/mock.sdapkg
```

仓库中的 `adapters/mock/mock-0.1.0.sdapkg` 是用当前开发公钥签名的测试包，供安装回归使用。
发布前请替换 `TRUSTED_KEYS` 中的开发公钥并重新构建应用。

## 10. 开发与测试工作流（目标）

```text
adapters/<id>/
├─ manifest.json
├─ src/index.ts
├─ tests/
│  ├─ fixtures/          # 脱敏 JSON/HTML
│  └─ contract.test.ts
└─ package.json
```

建议流程：

1. 以只读方式抓取自己的账号响应；
2. **脱敏**：移除 Cookie、姓名、课程名、标题、头像、学校标识等真实值；
3. 编写 `src/index.ts`，使用 `defineAdapter`；
4. 用 fixture 驱动 `fetchTasks`，断言任务数量、字段、时间单位、去重和 `complete`；
5. 包体积检查、manifest 校验、域名白名单检查；
6. 签名、安装到测试主机、用真实账号手动验证；
7. 发布到签名目录，保留前一版本。

建议测试用例：

- 空课程、空进度、单课程失败；
- 401/403、429、5xx；
- 秒/毫秒时间混用；
- 无截止时间、已完成任务；
- 考试与叶子重复；
- 平台新增未知字段/未知 `leaf_type`；
- 结果中不包含原始 Cookie 或个人信息。

## 11. 版本与兼容性

- `apiVersion`：接口不兼容时递增，例如 `1` → `2`。
- `minHostVersion` / `maxHostVersion`：确保使用支持的宿主能力。
- `version`：适配器自身 SemVer。
- `sequence`：单调递增，用于拒绝降级。
- 主机需要保留多个兼容版本，失败时回滚。
- 平台改版时优先发适配器版本，不要求主程序更新。

## 12. UI 与错误呈现

适配器只返回数据，不渲染 UI。宿主负责：

- 任务排序、倒计时、分组、状态徽标；
- 错误提示：`AUTH_EXPIRED`、`REAUTH_REQUIRED`、`RATE_LIMITED`、
  `RISK_CONTROL`、`SCHEMA_CHANGED`、`PARTIAL`、`NETWORK`；
- 部分成功时显示“部分课程未刷新”，保留旧任务；
- 失败时不静默清空任务。

## 13. 当前如何增加一个内置平台（过渡方案）

在 v1 宿主完成前，新增平台仍然需要修改核心代码并重新发版：

1. 新建 `apps/desktop/src-tauri/src/<platform>.rs`；
2. 实现与 `yuketang.rs` 同形态的 `fetch_snapshot(session, account_id)`；
3. 复用 `models.rs` 的 `Task`、`FetchSnapshot` 和既有任务校验规则；
4. 在登录/账号流程中增加该平台入口；
5. 添加脱敏 fixtures 和契约测试；
6. 重新编译并发布主程序。

雨课堂现在已经迁移到 `adapters/yuketang`，Rust 仅作为回退；剩余差距主要是账号 UI
仍只暴露雨课堂入口，以及远程签名目录和自动更新尚未实现。

## 14. 参考

- 当前雨课堂适配器：[`adapters/yuketang/index.js`](../adapters/yuketang/index.js)
- Rust 回退实现：[`src-tauri/src/yuketang.rs`](../apps/desktop/src-tauri/src/yuketang.rs)
- 统一任务模型：[`src-tauri/src/models.rs`](../apps/desktop/src-tauri/src/models.rs)
- 雨课堂接口与字段：[`docs/platform/yuketang.md`](platform/yuketang.md)
- 架构说明：[`docs/architecture.md`](architecture.md)
