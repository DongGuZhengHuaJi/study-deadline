# CI / CD

仓库使用 GitHub Actions 覆盖 Linux、Windows 和 macOS。

## CI（`.github/workflows/ci.yml`）

触发：

- push 到 `main`
- pull request
- 手动 `workflow_dispatch`

矩阵：

| 平台 | Runner | 内容 |
|---|---|---|
| Linux | `ubuntu-24.04` | 前端检查、`cargo check --all-targets`、`cargo test --lib` |
| Windows | `windows-latest` | 前端检查、`cargo check --all-targets` |
| macOS | `macos-latest` | 前端检查、`cargo check --all-targets` |

Linux 会安装 WebKitGTK/AppIndicator 等构建依赖。CI 不产出安装包，也不发布任何东西。

## CD（`.github/workflows/release.yml`）

触发：

- push 形如 `v*` 的 tag（例如 `v0.1.0`）
- 手动 `workflow_dispatch`（只构建并保留 Actions artifacts，不创建 Release）

矩阵构建：

| 平台 | 产物 | GitHub artifact |
|---|---|---|
| Linux | `.deb` | `study-deadline-linux` |
| Windows | NSIS `setup .exe` | `study-deadline-windows` |
| macOS | `.dmg` | `study-deadline-macos` |

tag 触发时，`release` job 会：

1. 下载三个平台的 artifacts；
2. 生成 `SHA256SUMS.txt`；
3. 创建或更新对应 tag 的 GitHub Release，并上传全部产物和校验和。

### 发布步骤

1. 更新版本号（`apps/desktop/package.json`、`apps/desktop/src-tauri/Cargo.toml`、
   `apps/desktop/src-tauri/tauri.conf.json` 保持一致）。
2. 提交并推送到 `main`。
3. 创建并推送 tag：

```bash
git tag v0.1.0
git push origin v0.1.0
```

4. 等待 Actions 的 `Release` workflow 完成；在仓库 Releases 查看产物。

如果只想先验证三平台构建而不创建公开 Release，使用 Actions 页面的
**Release → Run workflow**，构建产物会出现在该次 Actions run 的 artifacts 中。

## 签名状态

当前工作流产出的是 **unsigned** 安装包：

- Windows 未配置 Authenticode，SmartScreen 会提示未知发布者；
- macOS 未签名/公证，Gatekeeper 可能阻止打开；
- Linux `.deb` 无签名。

这不是构建失败，而是分发前的待办。要支持签名，需要额外配置：

- Windows 代码签名证书和 `tauri.conf.json` 的签名配置；
- Apple Developer ID、公证凭据和 Tauri macOS signing 环境；
- 更新工作流 secrets，并对 tag 发布做签名校验。

不要把证书、私钥或 Apple 凭据提交到仓库；使用 GitHub Actions secrets。

## 架构限制

`macos-latest` 和 `windows-latest` 当前产出主机架构（通常为 x64 或 arm64，取决于 runner）。
如需同时发布 Intel/Apple Silicon 或 Windows arm64，需要增加构建矩阵和交叉打包策略。

Linux 的 AppImage 未在默认 CD 中生成：AppImage 体积更大且依赖额外打包工具，当前优先 `.deb`。
