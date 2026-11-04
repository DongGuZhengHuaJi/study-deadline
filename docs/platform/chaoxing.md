# Chaoxing / 学习通 read-only adapter research (verified 2026-09/10)

> 实现见 [`adapters/chaoxing/index.js`](../../adapters/chaoxing/index.js) 与
> [`manifest.json`](../../adapters/chaoxing/manifest.json)。本文记录 2026-11-04
> 的调查结论；学习通没有公开学生 API，接口和页面结构可能随时变化。

**实现要点（中文摘要）**

- 登录入口：`https://passport2.chaoxing.com/login?fid=&refer=https%3A%2F%2Fi.chaoxing.com`；
  会话至少需要 `_uid` / `UID`，并在 `i.chaoxing.com/base` 上做一次验活。
- Cookie 只取 `*.chaoxing.com` 域；学校 SSO/IdP Cookie 不进入平台会话。
- 课程列表用 `mooc1-api.chaoxing.com/mycourse/backclazzdata`（JSON）。
- 作业列表用 `mooc1-api.chaoxing.com/work/stu-work` 聚合页；绝对截止时间从每条
  `data` 详情页的“作答时间”解析，详情请求有数量和 45 秒预算上限。
- 考试用 `mooc1.chaoxing.com/exam-ans/exam/test/examcode/examlist` 全校表，一次请求
  获得绝对起止时间；为空时回退 `exam-ans/exam/phone/examcode` 的相对时间。
- 所有时间按北京时间（UTC+8）解析；系统浏览器不共享 WebView 会话，因此任务卡链接
  使用稳定的课程入口/考试列表页，不保存带时效的 `enc` 详情链接。

Scope: own account, read-only assignment/exam metadata. No account requests were made; findings come from public 2026 client code plus anonymous checks of public login/redirect behavior. All IDs/cookies below are placeholders.

## 0. Bottom line

- Simplest reliable login entry: `https://passport2.chaoxing.com/login?fid=&refer=https%3A%2F%2Fi.chaoxing.com` in the embedded WebView. `https://i.chaoxing.com/base` also works and redirects anonymous users there. [High]
- Strongest session indicator: `_uid` (or `UID`) on a `.chaoxing.com` cookie domain, plus a successful `GET https://i.chaoxing.com/base` that is not the passport login page. `fid` is school context, not proof of auth. [High]
- Cheapest sync path (no `enc`, no N+1): JSON course list `backclazzdata` + aggregate `work/stu-work` + exam table `examlist` (fallback `exam/phone/examcode`). These were verified usable without `enc` in 2026-10. [High endpoints, medium school coverage]
- Exact homework deadlines require per-course `stucoursemiddle` → `work/list` → `work/task` detail: N courses + M pending details. The detail page is the only place with an absolute homework deadline. [High]
- `tpi` is not verified anywhere; treat it as a confusion with `cpi`/`personId`. Do not build on it. [High that no evidence exists]

## 1. Login and session

### 1.1 Entry URLs

| Purpose | URL | Notes | Confidence |
|---|---|---|---|
| Generic student login | `https://passport2.chaoxing.com/login?fid=&refer=https%3A%2F%2Fi.chaoxing.com` | Simplest generic entry; phone/password, QR, institution tab. Used by a 2026 desktop client. | High |
| Home / success page | `https://i.chaoxing.com/base` | Anonymous request 302s to passport login; logged-in title `个人空间`, contains `#frame_content`. | High |
| Institution-scoped login | `https://passport2.chaoxing.com/login?loginType=4&newversion=true&fid=<fid>&refer=http://i.chaoxing.com` | Use when school `fid` is known; otherwise generic page lets user choose school. | High pattern, school-specific fid |
| School portal variants | `https://<school>.fanya.chaoxing.com`, `https://<school>.mh.chaoxing.com` | Some schools CAS-redirect to Chaoxing (e.g. `authserver.eitech.edu.cn` → `eitech.mh.chaoxing.com`, `wfwfid=333892`). | Medium |
| Logout | `https://passport2.chaoxing.com/logout` | Not needed for read-only sync. | High |
| Not an entry | `https://sso.chaoxing.com/apis/login/userLogin4Uname.do` | Legacy cookie-supplement endpoint used after a login; not an SSO entry. | High |

Anonymous checks confirmed:
- `GET https://i.chaoxing.com/base` → 302 to `passport2.chaoxing.com/login?...`.
- Login page has `<title>用户登录</title>`, `#loginBtn`, `#passportAgreement`, and text `手机号` / `学习通密码`.

### 1.2 Success detection

Do not trust status code alone; Chaoxing often returns 200 with a login/error page. Require:
- Final URL stays on `https://i.chaoxing.com/base...` (not `passport2.chaoxing.com/login`).
- Body does not contain login markers: `passport2.chaoxing.com/login`, `<title>用户登录</title>`, `id="loginBtn"`.
- Positive markers (at least one): `id="frame_content"`, `aria-label="账号：`, `id="siteName"`, `var uid =`.

`https://passport2.chaoxing.com/mooc/accountManage` with `id="messageName" value="..."` is a useful alternate session probe, but `i.chaoxing.com/base` is the primary. [High]

### 1.3 Cookies

| Cookie | Role | Notes |
|---|---|---|
| `_uid` / `UID` | Primary user identity | Best single auth indicator; sometimes only one case is present. |
| `_d` | Session/device token | Classic reusable cookie. |
| `vc3` | Session validation token | Classic reusable cookie; may rotate. |
| `uf` | User/session token | Commonly present; don't require it alone. |
| `fid` | School/organization ID | Context, not auth. May be `-1`/`0` in login flow; parse actual from base page. |
| `p_auth_token`, `cx_p_token`, `xxtenc` | Newer auth-bearing tokens | Seen in 2026 sessions; treat as secrets. |
| `JSESSIONID`, `route`, `k8s`, `jrose` | Infra/routing | Not sufficient proof of login; host/path-scoped. |

Domain policy:
- Session cookies are normally domain-scoped to `.chaoxing.com`, shared by `i`, `mooc1`, `mooc1-1`, `mooc1-2`, `mooc1-api`, `mooc2-ans`, `passport2`.
- In the WebView, query several hosts (`i`, `mooc1`, `mooc1-api`, `mooc2-ans`, `passport2`) and merge cookies whose domain is `chaoxing.com` or ends with `.chaoxing.com`.
- Never read or forward school IdP cookies (`authserver.*`, `*.edu.cn`). Never send Chaoxing cookies to non-Chaoxing hosts.
- Send all Chaoxing cookies, not a hand-picked two; at minimum require `_uid`/`UID`. A 2026 reference notes `_d`, `UID`, `vc3` were sufficient for one account, but this is not a contract. [Medium]
- Extract HttpOnly cookies via the native WebView cookie manager while the login WebView is alive; `document.cookie` cannot see them.

### 1.4 School SSO

- Generic passport page supports school selection, phone/password, and QR. If the school uses CAS/IdP, the WebView will navigate to an external domain; allow top-level navigation and popups (`window.open`).
- Do not block non-Chaoxing navigation during login. A 2026 desktop client blocks all non-Chaoxing hosts; that would break school CAS flows. Allow IdP navigation, but only harvest `.chaoxing.com` cookies after returning to `i.chaoxing.com`.
- Use a desktop Chrome UA, not an Android WebView UA (`; wv`), to reduce "unsupported browser" rejections. Enable third-party cookies for the login WebView.
- Some IdPs use 2FA/SMS; some block embedded browsers. If blocked, the only safe fallback is user-driven login in a normal browser plus manual cookie import, or the official app; do not automate credential/captcha handling. [Medium]
- Programmatic password login exists (`POST https://passport2.chaoxing.com/fanyalogin`; `uname`/`password` AES-128-CBC/PKCS7/Base64 with key=iv `u2oh6Vu^HWe4_AES`, `fid=-1`, `refer=https%3A%2F%2Fi.chaoxing.com`), but it is out of scope for a WebView app and is captcha-prone. Do not add it unless required.

## 2. Profile, fid, and course list

### 2.1 Base page (`https://i.chaoxing.com/base`)

Server-rendered HTML. Robust extraction:

```js
const fid = /"fid"\s*:\s*"?(\d+)"?/i.exec(html)?.[1]
         ?? /var\s+fid\s*=\s*['"](\d+)['"]/i.exec(html)?.[1]
         ?? cookieFid;
const uid = /var\s+uid\s*=\s*['"]?(\d+)/i.exec(html)?.[1]
         ?? cookieUid;
const name = /aria-label=["']账号[：:]([^"']+)["']/i.exec(html)?.[1];
const school = /id=["']siteName["'][^>]*title=["']([^"']+)["']/i.exec(html)?.[1];
```

Notes:
- Support both `:` and full-width `：`.
- Do not fall back to `427` or `1024` for `fid`; those are guesses from other accounts and can select the wrong school. If missing, use the JSON course list (which does not need `fid`) and omit/empty `fid` on exam URLs.
- `https://contactsyd.chaoxing.com/pc/user/getUserInfo` is reported discontinued in 2026; do not use it.

### 2.2 Preferred course list: JSON

```
GET https://mooc1-api.chaoxing.com/mycourse/backclazzdata?view=json&rss=1
Referer: https://mooc1-api.chaoxing.com/
```

Response is JSON, not HTML:

```json
{"result":1,"channelList":[{"key":153050293,"cpi":500602665,
 "content":{"id":153050293,"cpi":500602665,"name":"2026-2027-1-通信",
 "course":{"data":[{"id":244459960,"name":"信号与系统"}]}}}]}
```

Field mapping (do not mix them):
- `channelList[].content.id` = **clazzId**
- `channelList[].content.course.data[0].id` = **courseId**
- `channelList[].content.cpi` = **cpi** (also called `personId`/`curPersonId`)
- `content.course.data[0].name` = course name; `content.name` = class/semester name
- `content.isretire` may be missing. Do not skip the course when it is missing; request all courses and let empty data mean no work.

Variants: `?view=json&mcode=` and no query also worked in 2026 clients. Try `view=json&rss=1` first, then `view=json&mcode=`, then no query. [High]

### 2.3 HTML fallback course list

```
POST https://mooc2-ans.chaoxing.com/mooc2-ans/visit/courselistdata
Content-Type: application/x-www-form-urlencoded
Referer: https://mooc2-ans.chaoxing.com/mooc2-ans/visit/interaction
Origin: https://mooc2-ans.chaoxing.com
X-Requested-With: XMLHttpRequest

courseType=1&courseFolderId=0&query=&superstarClass=0
```

Selectors/fields:
- Block: `div.course` or `li.course`.
- IDs: `input.courseId` / `input[name=courseId]`, `input.clazzId` / `input[name=clazzId]`, `input.curPersonId` (cpi).
- Link: `a[href*="stucoursemiddle"]`; parse `courseid`, `clazzid`, `cpi` with URLSearchParams.
- Name: `span.course-name[title]` or link text; teacher: `p.line2.color3[title]` (avoid `color2`, which is the school).
- Closed course marker: `not-open-tip`.

Regex fallback used by a 2026 MIT client: scan for `stucoursemiddle?courseid=(\d+)&clazzid=(\d+)&cpi=(\d+)`, then read the next ~2500 chars for `course-name[^>]*title="([^"]+)"`. [High]

Folder caveat: `courseFolderId=0` only returns the root. If using HTML, also `GET https://mooc2-ans.chaoxing.com/mooc2-ans/visit/interaction` and POST once per `courseFolderId` to avoid omitting folder courses. The JSON endpoint avoids this.

## 3. Course/homework/exam endpoints and tokens

### 3.1 Endpoint map

| Purpose | Request | Response | enc required? | Confidence |
|---|---|---|---|---|
| Course list (JSON) | `GET https://mooc1-api.chaoxing.com/mycourse/backclazzdata?view=json&rss=1` | JSON | No | High |
| Course list (HTML) | `POST https://mooc2-ans.chaoxing.com/mooc2-ans/visit/courselistdata` | HTML | No | High |
| Course middle page | `GET https://mooc1-2.chaoxing.com/visit/stucoursemiddle?courseid=<courseId>&clazzid=<classId>&vc=1&cpi=<cpi>` | HTML + redirect to `mycourse/stu?...enc=...` | No for entrance | High |
| Homework list (per course) | `GET https://mooc1.chaoxing.com/mooc2/work/list?...` | HTML | Yes: `workEnc`, `t`, `stuenc` | High |
| Homework aggregate | `GET https://mooc1-api.chaoxing.com/work/stu-work` | HTML | No | High |
| Homework detail | data URL from list, typically `https://mooc1.chaoxing.com/mooc-ans/mooc2/work/task?...` | HTML | Uses its own `enc` | High |
| Exam table | `GET https://mooc1.chaoxing.com/exam-ans/exam/test/examcode/examlist?edition=1&nohead=0&fid=<fid>` | HTML table | No | High |
| Exam aggregate | `GET https://mooc1-api.chaoxing.com/exam-ans/exam/phone/examcode` | HTML list | No | High |
| Exam table (API host) | `GET https://mooc1-api.chaoxing.com/exam-ans/exam/test/examcode/examlist` | HTML table | No | High |

Old non-`exam-ans` exam paths 302 to the `exam-ans` path. The `examlist` table is school-wide, not per-course; there is no need for N requests for exams. [High]

### 3.2 Token glossary

| Token | Where it comes from | Meaning/use |
|---|---|---|
| `enc` (page) | Final URL of `stucoursemiddle` redirect (`mycourse/stu?...enc=`), or hidden `#enc` | Page-state signature; used as `stuenc` on work list and chapter pages. Opaque, per-page/time-bound. |
| `workEnc` | Hidden input `#workEnc` on course middle page | Must be used as the `enc` parameter of `mooc2/work/list`. Using page `enc` causes `无权限`. |
| `examEnc` | Hidden input `#examEnc` | Extracted by clients but no verified 2026 endpoint uses it. Exam lists use global endpoints; do not depend on it. |
| `openc` | Hidden input `#openc` | Passed to work list and chapter pages; include when present. |
| `oldenc` | Hidden input `#oldenc` | Used for `studentstudy` chapter navigation, not assignment/exam metadata. |
| `stuenc` | Page-level `enc` | Passed as `stuenc` on work list; some layouts call it `standardEnc`. |
| `t` | `Date.now()` | Millisecond timestamp/freshness parameter. |
| `cpi` | Course list (`content.cpi`/`personId`/`curPersonId`) | Course/person context ID required by course pages and work list. |
| `ut` | Fixed `s` | Student role. |
| `tpi` | No verified occurrence | Do not implement; likely a confusion with `cpi`. |

Token extraction pattern, supporting both attribute orders:

```js
function hidden(html, id) {
  const e = id.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return new RegExp(`id=["']${e}["'][^>]*value=["']([^"']*)["']`, "i").exec(html)?.[1]
      ?? new RegExp(`value=["']([^"']*)["'][^>]*id=["']${e}["']`, "i").exec(html)?.[1]
      ?? "";
}
```

Prefer the final URL `enc` over hidden `#enc` when both exist. Never truncate or hand-construct `enc`; a truncated token can load a page with no content/iframe.

### 3.3 Homework list (per course)

```
GET https://mooc1.chaoxing.com/mooc2/work/list
  ?courseId=<courseId>&classId=<classId>&cpi=<cpi>&ut=s
  &openc=<openc>&enc=<workEnc>&t=<ms>&stuenc=<pageEnc>&isdisplaytable=2
Referer: <course middle page URL or https://mooc1.chaoxing.com/>
X-Requested-With: XMLHttpRequest
```

Critical details:
- `enc` must be `workEnc`, not the page-level `enc`.
- `t` and `stuenc` are also required; missing/wrong values produce `无权限` / `没有此页面访问权限`.
- `openc` and `isdisplaytable=2` are included by a 2026 client but may be optional in some layouts.
- Response is server-rendered HTML, not JSON.

Parsing:
- Items: `<li ... data="<detail URL>">` (some layouts also have `onclick="goTask(this);"`).
- Title: `p.overHidden2.fl` or `p.overHidden2`.
- Status: `p.status.fl` or `p.status`.
- Relative time: `div.time` (e.g. `剩余124小时44分钟`); absolute deadline is only on the detail page.
- Progress: text like `已完成任务点: N/M` or a `span` pair.

Regex-safe approach (QuickJS has no DOM):

```js
const items = [...html.matchAll(/<li[^>]*\bdata=["']([^"']+)["'][^>]*>([\s\S]{0,4000}?)<\/li>/gi)];
const title = /<p[^>]*class=["'][^"']*\boverHidden2\b[^"']*["'][^>]*>([\s\S]*?)<\/p>/i.exec(block)?.[1];
const status = /<p[^>]*class=["'][^"']*\bstatus\b[^"']*["'][^>]*>([\s\S]*?)<\/p>/i.exec(block)?.[1];
```

Decode HTML entities (`&amp;`, `&#39;`, `&quot;`, `&nbsp;`) and URL-decode the `data` value before use. Resolve relative URLs against the page URL and allow only `https://*.chaoxing.com`.

### 3.4 Homework aggregate (no enc, no per-course request)

```
GET https://mooc1-api.chaoxing.com/work/stu-work
Referer: https://mooc1-api.chaoxing.com/
```

This is a server-rendered aggregate pending-work page. Each `<li data="...">` contains an `aria-label` with all fields, e.g.:

```
aria-label="作业名称第三次作业作业状态未提交所属课程《概率论》剩余时间2天3小时"
```

Parse each field separately (avoid one fully optional regex):

```js
const aria = /aria-label=["']([^"']*)["']/i.exec(block)?.[1] ?? "";
const title = /作业名称([\s\S]*?)作业状态/.exec(aria)?.[1];
const status = /作业状态([\s\S]*?)所属课程/.exec(aria)?.[1];
const course = /所属课程《([^》]*)》/.exec(aria)?.[1];
const remaining = /剩余时间([^"'<]*)/.exec(aria)?.[1] ?? null;
```

- `taskrefId` / `courseId` from the `data` URL query.
- `剩余时间` may be absent; then due date is unknown, not zero.
- DOM fallback (for WebView debugging, not adapter): `ul.nav > li`, `div[role=option] p`, first span = status, second span = course, `.fr` = remaining.
- This aggregate may omit completed items; do not infer completion from absence. Parse status.
- No `enc` is required (2026-10 verified). [High]

### 3.5 Homework detail and deadline

Use the `data` URL exactly as returned. Typical form:

```
https://mooc1.chaoxing.com/mooc-ans/mooc2/work/task?courseId=<courseId>&classId=<classId>&cpi=<cpi>&workId=<workId>&answerId=<answerId>&enc=<detailEnc>
```

The URL carries its own independent `enc`; do not reconstruct it. Parse the detail page after stripping tags:

```js
const text = decodeHtml(stripTags(html)).replace(/\s+/g, " ");
const m = /作答时间[：:]\s*(\d{1,2}-\d{1,2}\s+\d{1,2}:\d{2})\s*(?:至|~|-|到)\s*(\d{1,2}-\d{1,2}\s+\d{1,2}:\d{2})/.exec(text);
```

Also support full dates (`2026-06-21 18:51`, `2026年6月21日 18:51`) and single `提交截止时间` / `结束时间` / `截止时间`.

- The month/day form has **no year**; infer the nearest year (see §4).
- Absolute homework deadline exists only here (or on the exam table for exams). The list's `div.time` is a countdown, not a deadline.
- To control requests, fetch details only for statuses that are not `已完成|已提交|待批阅|已批阅|已批改`; skip completed/awaiting-grade items.
- `work/list` + details is inherently N+1: one list per course, one detail per pending work. Cache details for at least 2–5 minutes and refresh at 30-minute intervals.

### 3.6 Exam table (absolute times, one request)

```
GET https://mooc1.chaoxing.com/exam-ans/exam/test/examcode/examlist?edition=1&nohead=0&fid=<fid>
```

or the API-host form verified by a 2026 MIT client:

```
GET https://mooc1-api.chaoxing.com/exam-ans/exam/test/examcode/examlist
```

Response is server-rendered HTML, not JSON. Rows are `<tr class="dataTr">` inside `table.dataTable`. Columns (0-based):

| Index | Content |
|---|---|
| 0 | 编号 |
| 1 | 名称 |
| 2 | 时间 (`YYYY-MM-DD HH:MM 至 YYYY-MM-DD HH:MM`) |
| 3 | 时长 |
| 4 | 考试状态 |
| 5 | 作答状态 |
| 6 | 分数 |
| 7 | 方式 |
| 8 | 操作 (`go('...')` link) |

Parsing:

```js
const rows = [...html.matchAll(/<tr[^>]*class=["'][^"']*\bdataTr\b[^"']*["'][^>]*>([\s\S]*?)<\/tr>/gi)];
const cells = [...row[1].matchAll(/<td[^>]*>([\s\S]*?)<\/td>/gi)]
  .map(m => decodeHtml(stripTags(m[1])).trim());
const timeMatch = /(\d{4}-\d{2}-\d{2})\s+(\d{1,2}:\d{2})\s*(?:至|~|-|到)\s*(\d{4}-\d{2}-\d{2})\s+(\d{1,2}:\d{2})/.exec(cells[2]);
```

- Prefer `cells[5]` (作答状态) for completion; `cells[4]` (考试状态) may say `已结束` even when not submitted.
- The table is school-wide and has no course column; do not invent a course. The title often is the course name, but treat that as a display fallback.
- `fid` is optional in practice (empty `fid=` worked in a 2026 userscript); include it when known.
- Filter by date/status client-side; it may include old exams.
- The action link `go('...')` may contain `examId` inside a `refer` parameter; parse it for dedupe if available. Otherwise dedupe by `(title, start, end)`.

### 3.7 Exam aggregate (relative times, fallback)

```
GET https://mooc1-api.chaoxing.com/exam-ans/exam/phone/examcode
```

Server-rendered HTML; entries are `<li data="/exam-ans/exam/phone/task-exam?taskrefId=...&courseId=...">`. Useful fields:
- `aria-label='考试名称：期中考试; 考试状态：已完成; 剩余时间：1天'` (single quotes may be used).
- DOM fallback: `ul.ks_list > li`, `dl dt` = title, `dl dd` = remaining, `span.ks_state` = status, `img[src*=ks_02]` = expired.
- No separate course field; title often equals course name.
- Times are relative (`剩余 X 天 X 小时 X 分钟`); convert to absolute using the fetch anchor (see §4). Prefer the `examlist` table when absolute times are available.
- Use this as a fallback when `examlist` is empty or blocked. Dedupe against `examlist` by `examId` or `(title, start, end)`.

## 4. Source URLs and time parsing

### 4.1 URL templates

Use the exact `data` URLs from the list whenever possible; do not fabricate `enc`. When constructing:

```
Dashboard:       https://i.chaoxing.com/base
Course middle:   https://mooc1-2.chaoxing.com/visit/stucoursemiddle?courseid=<courseId>&clazzid=<classId>&vc=1&cpi=<cpi>
Course home:     https://mooc2-ans.chaoxing.com/mooc2-ans/mycourse/stu?courseid=<courseId>&clazzid=<classId>&cpi=<cpi>&enc=<pageEnc>&v=2&hideHead=0
Work list:       https://mooc1.chaoxing.com/mooc2/work/list?courseId=<courseId>&classId=<classId>&cpi=<cpi>&ut=s&openc=<openc>&enc=<workEnc>&t=<ms>&stuenc=<pageEnc>&isdisplaytable=2
Work detail:     https://mooc1.chaoxing.com/mooc-ans/mooc2/work/task?courseId=<courseId>&classId=<classId>&cpi=<cpi>&workId=<workId>&answerId=<answerId>&enc=<detailEnc>
Exam table:      https://mooc1.chaoxing.com/exam-ans/exam/test/examcode/examlist?edition=1&nohead=0&fid=<fid>
Exam aggregate:  https://mooc1-api.chaoxing.com/exam-ans/exam/phone/examcode
```

Rules:
- `courseid`/`clazzid` are lowercase on course pages; `courseId`/`classId` are camelCase on work list. Parse both case-insensitively when reading URLs.
- If `enc`/`workEnc` is unavailable or stale, link to the course page or exam list, not a fabricated detail URL.
- For exams, prefer the list page over `task-exam` to avoid accidentally starting an exam.
- For homework, the detail/task page is metadata/read-only; avoid `doHomeWorkNew`/answer pages.
- Open source URLs in the authenticated in-app WebView, not the system browser; Chaoxing cookies are isolated per browser profile. Do not inject cookies into a system browser or put them in URLs.
- This app opens task links in the system browser, which does not share the WebView session. Therefore the implemented adapter stores a stable course entrance (`stucoursemiddle?courseid=...&clazzid=...&vc=1&cpi=...`) as the task URL, not the time-bound `data` detail URL; the detail URL is used only for deadline parsing.
- Allow only `https:` and hosts `chaoxing.com` / `*.chaoxing.com`; reject `javascript:`, `http:`, userinfo tricks, and non-Chaoxing redirects.

### 4.2 Time zone and parsing

All Chaoxing wall-clock strings for assignments/exams are Beijing time (UTC+8, no DST). Do not rely on the system timezone or `new Date("YYYY-MM-DD HH:MM")`; parse manually:

```js
const BEIJING_OFFSET_MS = 8 * 3600 * 1000;
function parseBeijing(y, mo, d, hh, mm, ss = 0) {
  return Date.UTC(y, mo - 1, d, hh, mm, ss) - BEIJING_OFFSET_MS;
}
```

Year inference for yearless homework dates:
- If a source send time exists, use its year; if the deadline month is January and the source month is December, add one year.
- Otherwise try `nowYear - 1`, `nowYear`, `nowYear + 1`; choose the candidate closest to now. Mark the result as inferred; do not use an inferred year to send an aggressive reminder if the exact exam table has a year.
- For December→January cross-year assignments, prefer the future candidate when the past candidate is more than a few days old.

Relative times (`work/stu-work`, `exam/phone/examcode`, `work/list div.time`):
- Parse `天`, `小时`, `分钟`, `秒` independently; do not use one all-optional regex.
- If the label exists with all zeros (`剩余0分钟`), treat as due at the anchor time. If the label is absent, return `null` (unknown).
- Store `anchorMs` + `remainingMs`, not only the computed absolute time. On display, recompute from the stored anchor; re-sync before showing precise countdowns. A cached "2 days left" becomes wrong after the app is closed for days.
- Prefer absolute times from `examlist` / `work/task` over relative countdowns when both exist.

Other normalization:
- If a timestamp is in seconds (< 1e12), multiply by 1000.
- Return `dueAt`/`startsAt` as Unix milliseconds in the adapter model.
- `examlist` gives start and end; `startsAt` = start, `dueAt` = end. Homework detail `作答时间 A 至 B`: `startsAt` = A, `dueAt` = B. Aggregate relative times give only `dueAt`.

## 5. Failure modes and handling

| Symptom | Meaning | Action |
|---|---|---|
| 302 to `passport2.chaoxing.com/login?...` | Session expired | `AUTH_EXPIRED`; stop refresh; keep last tasks; prompt re-login. |
| 200 page with `<title>用户登录</title>`, `#loginBtn`, `passport2.../login` | Logged-out HTML returned with 200 | Same as above; don't trust status code. |
| `无权限` / `没有此页面访问权限` on work list | Wrong/stale `workEnc`, `t`, `stuenc`, `openc`, or missing referer/context | Re-fetch course middle page, retry once; if still failing, mark partial. Do not retry in a loop. |
| `enc校验失败` | Missing/invalid `enc` on a new mobile JSON API | Use the no-enc HTML endpoints instead; do not try to forge `enc`. |
| `9010` + `验证码` page, `antispiderShowVerify.ac`, `processVerifyPng.ac` | Anti-scraping/risk control | `RISK_CONTROL`; stop; ask user to complete verification manually in the WebView. Never solve captchas automatically. |
| 412 / 403 / 429 | Rate limit/WAF | Stop immediately; respect `Retry-After`; exponential backoff. |
| 5xx / SSL EOF / network error | Transient | Retry 2–3 times with backoff; keep old tasks. |
| Empty course list | Could be no courses, wrong `fid`, or login page | Check base page and JSON response; don't assume empty. |
| Course list misses folder courses | HTML `courseFolderId=0` only root | Use JSON `backclazzdata`, or enumerate `interaction` folders. |
| Missing `workEnc` | Course has no open work tab, or page incomplete | Mark course partial; don't treat as "no homework". |
| `isretire` missing | Not necessarily retired | Request the course anyway; don't skip. |
| Exam table empty | Could be no exams, wrong school, or login page | Check headers/table presence; fallback to `exam/phone/examcode`; distinguish empty from error. |
| Exam title is course name | Table has no course column | Use title as display fallback; don't fabricate course. |
| `待批阅` status | Submitted, awaiting grading | Treat as completed for reminders; don't nag. |
| `已结束` exam with empty answer status | Window ended, submission unknown | Don't mark completed automatically; mark expired/unknown. |
| Mobile-only/proctored exam | Web may show metadata but require app | Read metadata only; don't bypass; link to list. |
| HTML selectors disappeared | Platform redesign | `SCHEMA_CHANGED`; keep old tasks; ship adapter update. |
| Login WebView navigates to school IdP | Normal SSO | Allow navigation; don't block; only harvest `.chaoxing.com` cookies after return. |
| "Load cannot follow more than 20 redirections" after login / SMS code | Stale `route`/session Cookie, or WebKitGTK's default `NO_THIRD_PARTY` cookie policy breaking the passport2 → SSO → `i.chaoxing.com` chain | Use an ephemeral/incognito login WebView and set the Linux WebKitGTK cookie policy to `ACCEPT_ALWAYS`; do not reuse the platform cookie jar across attempts. |
| Cookies present but requests redirect to login | Cookie host/domain or stale token | Re-read all `.chaoxing.com` cookies; verify base page; re-login. |

Rate-limit guidance:
- Per-account serial requests; if parallel, max 2–3 with 1–3 s jitter.
- Cache course list 5 min, homework list 2 min, login probe 5 min; full refresh every 30 min.
- Don't poll exams more than every 30 min; they change rarely.
- Don't fetch detail pages for completed/awaiting-grade work.

## 6. Reference projects and licensing

| Project | License | Date/status | What to learn |
|---|---|---|---|
| [DanMo661/Xuexitong-mcp](https://github.com/DanMo661/Xuexitong-mcp) (alias `xuexitong-cli`) | MIT | v1.4.1, pushed 2026-10-27; real-account verified 2026-09~10 | Primary endpoint behavior: base page, course list, `work/list` + `workEnc`/`t`/`stuenc`, `work/task` deadline, `examlist` table. |
| [Han-maker-wp/xuexitong-cli-skill](https://github.com/Han-maker-wp/xuexitong-cli-skill) | MIT | 2026-09 real-machine calibration | Login detection, `#frame_content`/`s=` flow, iframe tabs, `enc` truncation pitfalls. |
| [Han-maker-wp/chaoxing-mcp](https://github.com/Han-maker-wp/chaoxing-mcp) | MIT | pushed 2026-10-21 | HTTP-direct course/work behavior. |
| [linpigeon/UniversityTodos](https://github.com/linpigeon/UniversityTodos) | AGPL-3.0 | pushed 2026-10-25 | No-enc aggregate endpoints (`work/stu-work`, `exam/phone/examcode`), JSON course list, relative-time anchor design. |
| [DiYanYe/eitech-lms-helper](https://github.com/DiYanYe/eitech-lms-helper) | GPL-3.0 | verified 2026-09-25 | Three `enc` values, work list parameters, school CAS example, request pacing. |
| [SamBUFF/chaoxing-labor-course](https://github.com/SamBUFF/chaoxing-labor-course) | MIT | 2026-10 | DOM/token reference: `#workEnc`, `#examEnc`, `#oldenc`, `#openc`, work list selectors. |
| [Cooanyh/chaoxing-list](https://github.com/Cooanyh/chaoxing-list) | AGPL-3.0 | userscript v2.4.10 | Dual exam sources, aggregate work/exam parsing, `data` URL handling. |
| [K4F7/chaoxing-mcp](https://github.com/K4F7/chaoxing-mcp) | No root license found (all rights reserved) | pushed 2026-10-03 | Desktop reminder architecture, WebView2 login; note it blocks non-Chaoxing navigation, which breaks external SSO. Do not copy. |
| [Samueli924/chaoxing](https://github.com/Samueli924/chaoxing) | GPL-3.0 | pushed 2026-09-28 | Confirms `_uid`, HTML course list, session check; `fid` default 1024 is an anti-pattern. |
| [shiliu153/chaoxing-homework-hub](https://github.com/shiliu153/chaoxing-homework-hub) | MIT | pushed 2026-11-02 | Confirms work list needs `workEnc`/`examEnc`/`openc`. |

Licensing notes: MIT code can be reused with its copyright/license notice, but prefer distilled reimplementation. GPL-3.0 and AGPL-3.0 code should not be copied into a permissively licensed app without complying with those licenses. K4F7 has no root license; treat as all rights reserved and use only as behavioral reference. There is no official public student API; endpoints are non-public web interfaces, and platform terms may prohibit scraping/reverse engineering. Keep the app read-only, own-account, low-frequency, and never bypass captchas or exam restrictions.

## 7. Recommended adapter flow for this repo

The local SDK is synchronous QuickJS with no DOM, `ctx.http`, `ctx.session`, `ctx.config`, `ctx.sleep`, and signed manifests. Use regex over raw HTML, not selectors.

Suggested `fetchTasks` order:

1. `GET https://i.chaoxing.com/base`; verify session; parse `fid`/`uid`.
2. `GET https://mooc1-api.chaoxing.com/mycourse/backclazzdata?view=json&rss=1`; parse JSON courses. Fallback to POST `courselistdata` + HTML regex.
3. `GET https://mooc1-api.chaoxing.com/work/stu-work`; parse aggregate pending work. This avoids per-course work-list requests and `enc`.
4. `GET https://mooc1.chaoxing.com/exam-ans/exam/test/examcode/examlist?edition=1&nohead=0&fid=<fid>`; parse absolute exam rows. Fallback to `https://mooc1-api.chaoxing.com/exam-ans/exam/phone/examcode`.
5. Optional precision: for pending work only, per course:
   - `GET https://mooc1-2.chaoxing.com/visit/stucoursemiddle?courseid=...&clazzid=...&vc=1&cpi=...`; follow same-site redirects manually (max 3–5); capture final URL `enc` and hidden `workEnc`/`openc`.
   - `GET https://mooc1.chaoxing.com/mooc2/work/list?...&enc=<workEnc>&t=<ms>&stuenc=<pageEnc>&isdisplaytable=2`.
   - For each pending item, `GET` its `data` URL; parse `作答时间`.
6. Normalize:
   - `remoteId`: `cx:work:<courseId>:<workId>` or `cx:exam:<examId>`; use `taskrefId`/`workId`/`examId`, not title or list index.
   - `title`, `course`, `kind`, `dueAt`, `startsAt`, `status`, `completed`, `url`.
   - Dedupe work by `(courseId, workId)`; exams by `examId` or `(normalized title, start, end)`. Aggregate vs per-course and `examlist` vs `exam/phone` will overlap.
   - `complete=false` on any auth/risk/schema/partial-course failure; the host keeps the last snapshot.
7. URL validation: only HTTPS and `chaoxing.com`/`*.chaoxing.com`; return stored `data` URL if trusted, else course/list fallback.

Manifest fit:
- `auth.loginUrl`: `https://passport2.chaoxing.com/login?fid=&refer=https%3A%2F%2Fi.chaoxing.com`
- `auth.allowedOrigins`: `https://*.chaoxing.com` plus the user's school IdP origin if known. Do not block external IdP navigation during login.
- `permissions.http`: `https://*.chaoxing.com/*` (or explicit `i`, `mooc1`, `mooc1-1`, `mooc1-2`, `mooc1-api`, `mooc2-ans`, `passport2`).
- `sessionCookies`: `_uid`, `UID`, `_d`, `vc3`, `uf`, `fid`, `p_auth_token`, `cx_p_token`, `xxtenc`. Do not include IdP cookies; avoid `JSESSIONID`/`route` unless a specific endpoint requires them.
- Set the HTTP response size cap to at least 512 KB: `stucoursemiddle` pages can be 130–260 KB.
- Allow same-site redirects within `*.chaoxing.com` (e.g. `mooc1-2` → `mooc2-ans`), but never follow to non-Chaoxing hosts. If the host forbids redirects entirely, implement a manual same-site redirect loop and re-attach cookies.
- QuickJS: strip `<script>`/`<style>` before regex; use bounded windows (`[\s\S]{0,4000}?`) to avoid catastrophic backtracking on 200 KB pages.

## 8. Confidence summary

- High: login URL and success markers; `_uid`/`UID` as primary cookie; base-page `fid`/`uid`/name extraction; JSON course list fields; aggregate `work/stu-work` and `exam/phone/examcode` no-enc behavior; `work/list` requiring `workEnc`/`t`/`stuenc`; `work/task` yearless deadline; `examlist` table columns and absolute times; Beijing timezone; N+1 analysis; 9010/`无权限` failure modes.
- Medium: minimum cookie set (`_d`/`UID`/`vc3`); exact role of `p_auth_token`/`cx_p_token`/`xxtenc`; school-specific SSO/`wfwfid` variants; `openc`/`isdisplaytable` necessity; whether `work/stu-work` includes all courses for every school; `examlist` fid parameter necessity.
- Low/unverified: `tpi` (no evidence); `examEnc` endpoint usage (no verified use); non-degree/continuing-education accounts; schools whose exam list is app-only.

## Sources

- [DanMo661/Xuexitong-mcp](https://github.com/DanMo661/Xuexitong-mcp) — MIT, 2026-10-27, primary endpoint behavior.
- [DanMo661/xuexitong-cli alias](https://github.com/DanMo661/xuexitong-cli)
- [Han-maker-wp/xuexitong-cli-skill login/nav](https://github.com/Han-maker-wp/xuexitong-cli-skill/blob/main/skills/xuexitong/references/login-and-nav.md) — MIT, 2026-09 calibration.
- [Han-maker-wp/chaoxing-mcp](https://github.com/Han-maker-wp/chaoxing-mcp) — MIT, 2026-10-21.
- [linpigeon/UniversityTodos three-source spec](https://github.com/linpigeon/UniversityTodos/blob/master/docs/三源作业获取实现规格.md) — AGPL-3.0, 2026-10-25.
- [DiYanYe/eitech-lms-helper verified API doc](https://github.com/DiYanYe/eitech-lms-helper/blob/main/docs/接口实测文档.md) — GPL-3.0, 2026-09-25.
- [SamBUFF/chaoxing-labor-course DOM reference](https://github.com/SamBUFF/chaoxing-labor-course/blob/main/references/chaoxing-dom.md) — MIT, 2026-10.
- [Cooanyh/chaoxing-list userscript](https://github.com/Cooanyh/chaoxing-list/blob/main/chaoxing-list.user.js) — AGPL-3.0.
- [K4F7/chaoxing-mcp](https://github.com/K4F7/chaoxing-mcp) — no root license; behavioral reference only.
- [Samueli924/chaoxing](https://github.com/Samueli924/chaoxing) — GPL-3.0.
- [shiliu153/chaoxing-homework-hub](https://github.com/shiliu153/chaoxing-homework-hub) — MIT, 2026-11-02.
- [flutter_chaoxing API docs](https://pub.dev/documentation/flutter_chaoxing/latest/flutter_chaoxing/ChaoxingApiService-class.html) — mobile/client API contrast only.