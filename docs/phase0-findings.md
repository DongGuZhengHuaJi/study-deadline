# Phase 0 Spike 结论

完成日期：2026-11-04（Linux 开发机，真实雨课堂账号）

> 本文是工程验证记录，不包含真实 Cookie、账号、课程名或个人课表。
> 接口细节见 [雨课堂平台文档](platform/yuketang.md)。

## 已验证

- Tauri 2.12 + WebKitGTK 2.52，内嵌 WebView 完成雨课堂二维码/SSO 登录。
- 在登录 WebView 存活时，通过 Rust 侧 `WebviewWindow::cookies()` 读取到 `sessionid`、`csrftoken`（包括 HttpOnly）。
- 使用相同桌面 UA + Cookie + `xtbz: ykt` + `X-CSRFToken` + `Referer`/`Origin`，由 `reqwest` 请求成功：
  - 用户信息接口 HTTP 200
  - 课程列表 HTTP 200
  - `c27` 课程进度接口 HTTP 200
  - 考试预告接口 HTTP 200
- 应用重新构建/重启后，重新打开登录窗口仍能读取有效 `sessionid` 并继续用 reqwest 拉取（WebView Cookie 持久化验证）。
- 定位到课程进度树 `section_leaf_list` 作为带截止时间的任务数据源，字段见平台文档。
- 确认课程详情页使用 `/v2/web/studentLog/{classroom_id}`，而不是 `course_id`。

## 未验证 / 后续

- Windows 10/11 实机上的 Tauri Cookie API（本次在 Linux X11 验证；正式发布前必须在 Windows 验证）。
- 加密会话持久化：Phase 0 只在 WebView/内存中使用；当前正式版已实现
  OS keyring 主密钥 + AES-GCM，见 [架构说明](architecture.md)。
- 精选叶子深链；当前正式版跳课程详情页。
- 平台接口长期稳定性、风控阈值。

## 结论

登录态提取与后端复用链路可行；可以进入正式产品实现。正式产品仍必须遵守只读、低频、本人账号边界。
