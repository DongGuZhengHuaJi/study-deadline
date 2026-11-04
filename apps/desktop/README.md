# StudyDeadline Desktop

这是 StudyDeadline 的 Tauri 桌面应用目录。

项目总览、安装方式、安全边界和文档入口见仓库根目录的
[README](../../README.md)。

## 开发

```bash
cd apps/desktop
pnpm dev
```

开发模式需要 Vite 服务；不要给开发版开启开机启动。
完整构建：

```bash
pnpm tauri build --bundles deb
```
