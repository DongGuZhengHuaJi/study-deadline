# 雨课堂平台接口文档（学生端 Web）

> 状态：2026-11-04 在标准雨课堂区域完成最小只读验证。
> **雨课堂没有公开学生 API。** 本文记录的端点和字段来自学生端 Web 行为与前端路由观察，
> 不是官方接口文档，随时可能改版、鉴权失败或触发风控。
>
> 本项目的使用边界：仅本人账号、仅作业/考试元数据、低频只读；不读取题目/答案/试卷，
> 不提交作业/考试，不签到，不绕过验证码。继续使用前请阅读雨课堂用户协议并自行评估风险。

## 1. 区域与会话

| 区域 | Base URL | 验证状态 |
|---|---|---|
| 标准雨课堂 | `https://www.yuketang.cn` | 已实测 |
| 荷塘雨课堂 | `https://pro.yuketang.cn` | 未实测，路径预期一致 |
| 长江雨课堂 | `https://changjiang.yuketang.cn` | 未实测，路径预期一致 |
| 黄河雨课堂 | `https://huanghe.yuketang.cn` | 未实测，路径预期一致 |

- 区域之间会话隔离，不能拿 A 区域的 Cookie 请求 B 区域。
- 考试子域（例如 `*-exam.yuketang.cn`）有独立会话和换取流程；本项目**不使用**考试子域，
  也不读取考试内容。
- 请求中的 `{base}` 均应替换为用户实际选择区域的 Base URL。

## 2. 登录与会话提取

### 2.1 登录页面

在应用内 WebView 打开：

```text
{base}/v2/web/index
```

由用户完成二维码登录或学校 SSO。应用不接触密码，也不自动化验证码。

### 2.2 需要的 Cookie

| Cookie | 用途 | 备注 |
|---|---|---|
| `sessionid` | 主会话 | 通常为 HttpOnly/Secure |
| `csrftoken` | 请求头 `X-CSRFToken` | 可能被接口轮换 |
| `login_type` | 页面状态 | 不用于鉴权判断 |
| `django_language` | 语言偏好 | 可选 |
| `sensorsdata*` / `sajssdk*` | 前端统计 | 本项目仅按浏览器行为携带，不解析 |

实现要求：

- 只读取域名属于 `yuketang.cn` / `*.yuketang.cn` 的 Cookie。
- 不要读取、保存或转发微信、学校 SSO 域的 Cookie。
- Cookie 必须在登录 WebView 仍存活时通过 Tauri Rust API 读取；
  登录窗口关闭后再读取可能拿不到。
- 不要通过页面 `document.cookie` 获取 `sessionid`：HttpOnly Cookie 不可见，且会削弱安全边界。

### 2.3 会话验证

登录窗口读取到 Cookie 后，不要仅凭跳转 URL 判断成功。使用一次轻量只读请求验证，例如：

```text
GET {base}/v2/api/web/courses/list?identity=2
```

- 2xx 且返回可解析的 `data.list`：会话可用。
- 401 / 403 / 3xx 跳转：会话无效或已过期。
- 解析失败：可能是页面/接口改版，记录为 `SCHEMA_CHANGED`，不要继续清空任务。

## 3. 请求头约定

### 3.1 所有平台请求

应用内 `reqwest` 请求使用与 WebView 一致的桌面 Chrome User-Agent，并发送：

```http
Cookie: <从 WebView 读取的会话 Cookie>
Referer: {base}/v2/web/index
Origin: {base}
xtbz: ykt
Accept: application/json, text/plain, */*
Accept-Language: zh-CN,zh;q=0.9
X-Requested-With: XMLHttpRequest
User-Agent: <desktop-chrome-ua>
```

如果存在 `csrftoken`：

```http
X-CSRFToken: <csrftoken>
```

课程进度请求还需要：

```http
classroom-id: <classroom_id>
```

### 3.2 请求安全

- 只允许 HTTPS。
- 手动构造 Cookie 请求头，不依赖第三方 Cookie jar 的隐式行为。
- **不要跟随重定向**：带 Cookie 的跨域重定向可能泄露会话；遇到 3xx 按会话失效处理。
- 只允许 `yuketang.cn` 域名；不要将 Cookie 发给 CDN 或第三方。
- `Cookie` / `Authorization` / `X-CSRFToken` 不进入日志。
- 请求之间限速：课程树串行请求，每门课间隔至少 1–2 秒。
- 默认 30 分钟刷新；建议尊重 429 / 风控并暂停本轮。

## 4. 只读接口

### 4.1 用户信息 / CSRF

```text
GET {base}/v/course_meta/user_info
```

用途：

- 验证会话有效性；
- 页面行为中可能铸造/轮换 `csrftoken`。

本项目当前不使用其响应体作为任务来源；若使用响应体，请勿记录姓名等个人信息。
CSRF Cookie 可能更新，后续请求应以最新 `csrftoken` 为准。

### 4.2 课程列表

```text
GET {base}/v2/api/web/courses/list?identity=2
```

响应（示意，省略无关字段）：

```json
{
  "errcode": 0,
  "errmsg": "success",
  "data": {
    "list": [
      {
        "classroom_id": 10000001,
        "name": "课程班级名称",
        "term": 202601,
        "course": {
          "id": 20000001,
          "name": "课程名称",
          "university_id": 1,
          "platform": 1
        }
      }
    ]
  }
}
```

字段用途：

- `classroom_id`：课程/班级上下文，也是课程详情页 URL 的参数。
- `course.id`：课程 API 请求参数（课程进度接口的 `course_id`）。
- `course.name` / `name`：展示课程名称。
- `term`：学期编码。

**学期筛选建议**：取返回列表中最大的有效 `term` 作为当前学期；只刷新当前学期，避免扫描
所有历史课程。若平台增加了未来学期但无内容，可结合课程进度是否非空进行修正。
具体策略应随真实账号数据验证，不要假设 `term` 编码格式在所有学校一致。

### 4.3 课程进度 / 截止时间树

```text
GET {base}/c27/online_courseware/schedule/course/progress/?classroom_id={classroom_id}&course_id={course_id}
```

请求需带 `classroom-id` 请求头。响应示意：

```json
{
  "success": true,
  "msg": "success",
  "data": {
    "sku_id": 100,
    "course_progress": [
      {
        "id": 30000001,
        "name": "章节名称",
        "order": 1,
        "section_leaf_list": [
          {
            "id": 40000001,
            "leafinfo_id": 50000001,
            "chapter_id": 30000001,
            "name": "作业或考试标题",
            "leaf_type": 5,
            "is_score": true,
            "start_time": 1793756253000,
            "end_time": 0,
            "score_deadline": 1795190340000,
            "progress": {
              "done": 0,
              "total": 0,
              "status": "未开始",
              "total_score": 100.0,
              "user_score": 0
            }
          }
        ]
      }
    ]
  }
}
```

字段说明：

| 字段 | 类型 | 说明 |
|---|---|---|
| `leaf_type` | number | 叶子类型；实测 `5/6/9` 可带截止时间，`8` 多为视频/内容 |
| `is_score` | bool | 是否计分/需要完成 |
| `name` | string | 作业/考试标题 |
| `start_time` | number | **毫秒**时间戳；0 表示无 |
| `end_time` | number | **毫秒**时间戳；0 表示无 |
| `score_deadline` | number | **毫秒**时间戳；0 表示无截止时间 |
| `progress.done/total` | number | 完成量；可用于判断是否已完成，但不要把 0/0 视为完成 |
| `progress.status` | string | 平台状态文本，可能变化 |
| `id` / `leafinfo_id` | number | 稳定标识；用于去重和任务 ID |
| `chapter_id` | number | 章节 ID |

实测 `leaf_type` 语义（仅作为过滤参考，不能视为官方契约）：

- `5`：带截止时间的作业/考试类叶子；
- `6`：带截止时间的考试类叶子；
- `8`：视频/学习内容，通常 `score_deadline=0`；
- `9`：带截止时间的测验/任务类叶子。

**实现建议**：

```text
纳入条件：is_score == true && score_deadline > 0
标题：name（清理空白，限制长度）
截止：score_deadline（毫秒）
开始：start_time（毫秒，0 -> null）
类型：leaf_type=6 -> exam；leaf_type=9 -> quiz；其余 -> assignment
完成：total > 0 && done >= total，或 status 包含“完成/已交/已批阅”
```

不同区域和教师配置可能产生其他 `leaf_type`；遇到未知类型不要直接丢弃，若其
`score_deadline > 0 && is_score == true`，至少应展示为通用任务，并在适配器版本中补充映射。

### 4.4 考试预告

```text
GET {base}/api/v3/classroom/on-lesson-upcoming-exam
```

响应示意：

```json
{
  "code": 0,
  "msg": "success",
  "data": {
    "onLessonClassrooms": [],
    "upcomingExam": [
      {
        "id": 60000001,
        "classroom_id": 10000001,
        "classroom_name": "课程班级名称",
        "title": "考试标题",
        "type": "exam",
        "start_time": 1793756253,
        "end_time": 1795190340
      }
    ]
  }
}
```

注意：

- 本接口的时间是**秒**，课程叶子是**毫秒**；必须分别归一化，避免日期错位。
- `upcomingExam` 可能与课程进度树中的叶子重复：同一场考试同时出现在两个数据源。
- 考试接口只提供预告，不应用它读取考试内容或换取考试子域 token。

### 4.5 旧接口

```text
GET {base}/mooc-api/v1/lms/learn/course/chapter?cid={course_id}&classroom_id={classroom_id}
```

在 2026-11-04 实测返回 400。旧接口不可依赖；不要围绕旧字段结构编写唯一解析路径。

## 5. 任务归一化

建议统一映射为以下字段：

```text
remote_id      平台+课程+叶子的稳定 ID
title          leaf.name
course         course.name
kind           assignment | exam | quiz
due_at         score_deadline (ms)
starts_at      start_time (ms) / exam.start_time (s * 1000)
status         平台状态或归一化后的状态
completed      完成判断
url            {base}/v2/web/studentLog/{classroom_id}
source         yuketang
fetched_at     抓取时间
```

### 5.1 去重

考试数据与课程叶子可能重复。建议：

1. 先建立课程叶子的任务；
2. 对 `upcomingExam` 使用 key
   `(classroom_id, normalized_title, starts_at, due_at)` 查找已有项；
3. 命中时把叶子标记为 `exam`，不要重新插入；
4. 未命中时才新增考试任务。

不要只按 `remote_id` 去重：不同数据源使用不同 ID 体系。

### 5.2 完整快照与部分失败

- 一门课程进度失败时，`complete=false`，保留上次成功任务，不要删除未返回的任务。
- 课程列表、考试接口失败同样标记为不完整。
- 只有所有当前学期课程都成功时才允许把未返回任务标记为“不再出现”。
- 不要根据“未返回”推断为用户已提交；`completed` 只能来自进度字段。

## 6. 点击跳转

前端路由源码确认：

```text
/v2/web/studentLog/:classroomid
```

因此课程详情 URL 为：

```text
{base}/v2/web/studentLog/{classroom_id}
```

注意这里使用的是 **classroom_id**，不是 `course.id`。用错 ID 时平台可能显示
“高校认证/无权查看”，看起来像登录问题，实际是课程上下文错误。

当前版本把这个 URL 交给**系统浏览器**打开。系统浏览器与应用内 WebView 的 Cookie
彼此隔离；如果浏览器未登录雨课堂，需要用户在浏览器里登录。不要尝试通过注入 Cookie、
浏览器扩展或读取浏览器数据库来“共享”会话。

## 7. 错误处理

| 情况 | 建议 |
|---|---|
| 401 / 403 | `AUTH_EXPIRED`，停止该账号刷新，提示重新登录 |
| 3xx 重定向 | 视为会话失效；不要跟随重定向并转发 Cookie |
| 429 | `RATE_LIMITED`，停止本轮，指数退避，尊重 `Retry-After` |
| 5xx / 网络错误 | 保留旧任务，稍后重试 |
| JSON 字段缺失 | `SCHEMA_CHANGED`，保留旧数据，记录脱敏字段结构 |
| 单门课程失败 | `PARTIAL`，不删除旧任务 |
| 考试/课程数据重复 | 归一化 + 去重，不重复提醒 |
| 考试子域无法访问 | 不读取考试内容，仅提示用户在浏览器打开 |

## 8. 测试与脱敏

建议测试覆盖：

- 四个区域的页面可达性（至少实际账号所在区域）；
- 登录成功、登录过期、错误区域；
- 课程列表为空、分页/字段缺失；
- 课程进度为空、部分课程失败、429；
- 课程叶子时间为 0、秒/毫秒混用；
- 考试预告与叶子去重；
- 重启后会话可用；
- URL 使用 `classroom_id` 打开正确的课程页。

提交到仓库的 fixtures 必须：

- 删除 Cookie、Authorization、CSRF token；
- 替换课程名、姓名、头像 URL、学校标识等为占位值；
- 保留字段名、类型、数组层级和时间单位；
- 不要提交真实账号响应原文件。

## 9. 已知风险与限制

- 无官方学生 API；接口属于非公开 Web 接口，改版是常态。
- 用户协议禁止抓取、反向工程和绕过技术限制；本工具是只读低频的本人账号工具，
  仍不等于平台授权。
- 不同学校可能有 SSO、额外院校认证、字段差异；未实测区域不能保证可用。
- 考试子域、考试内容、监考/签到/作答流程不在本项目范围内。
- 平台可能通过风控、验证码、设备绑定或限流阻止自动读取；不支持绕过。
- 账号受限、接口失效、数据缺失均由使用者自行承担。

## 10. 参考

以下公开项目仅用于了解学生端 Web 行为，本项目未复制其代码：

- [`powercess/yuketang-mcp`](https://github.com/powercess/yuketang-mcp) — 学生端只读接口示例
- [`Auto-CQUPT-Plan/RainClassSDK`](https://github.com/Auto-CQUPT-Plan/RainClassSDK) — 雨课堂登录/课程流程参考
- [雨课堂用户协议](https://qn-help.yuketang.cn/user-agreement-2412262.html)

## 11. 变更记录

- 2026-11-04：首次实测标准区域；确认 `classroom_id` 课程页路由、`score_deadline`
  毫秒时间戳、考试接口秒时间戳；记录旧 `mooc-api` 接口 400。
