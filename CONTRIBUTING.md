# 贡献指南

感谢关注 StudyDeadline。提交 Issue 或 PR 前，请先阅读根目录
[README](README.md)、[安全政策](SECURITY.md) 和
[适配器 SDK 规范](docs/adapter-sdk.md)。

## 可以贡献什么

- 修复 UI、同步、托盘、日志、SQLite、安全相关问题；
- 改进平台适配器的解析和测试（必须是只读元数据）；
- 用脱敏 fixtures 补充回归测试；
- Windows/macOS/Linux 的构建、打包和桌面环境适配；
- 文档、翻译和可访问性改进。

## 不要提交

- 真实账号 Cookie、密码、token、Authorization 头、二维码内容；
- 真实课程名、姓名、头像 URL、学校标识、未脱敏的接口响应；
- 题目、答案、试卷、作业正文或任何提交/答题自动化；
- 用于绕过验证码、风控、设备绑定、监考的实现；
- 访问考试的自动换票、自动作答、签到或提交逻辑。

如需提供接口样例，请将字符串替换为占位值，只保留字段名、类型、层级和时间单位。

## 开发

```bash
pnpm install
pnpm check
pnpm dev
```

Rust：

```bash
cargo check --manifest-path apps/desktop/src-tauri/Cargo.toml
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml
```

Linux 构建依赖见 README。Windows/macOS 构建必须在对应系统上完成。

## PR 要求

1. 说明改动目的、影响范围和安全边界；
2. 涉及平台解析的 PR 必须提供脱敏 fixture 和至少一个失败场景测试；
3. 不把开发版注册为开机启动；
4. 不引入未声明的网络请求或第三方数据收集；
5. 更新相关文档（README/平台接口/SDK/架构）；
6. 保持改动聚焦，一个 PR 解决一个明确问题。

## Issue

请包含复现步骤、系统/版本、是否发布版、错误日志中**已脱敏**的部分。
不要粘贴 Cookie、账号信息或完整课表。敏感安全问题请使用
[SECURITY.md](SECURITY.md) 中的私密报告渠道。
