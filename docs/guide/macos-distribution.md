# macOS 发布与签名

## 当前临时方案

当前仓库尚未配置 Apple Developer ID 证书。为避免 macOS 将应用判定为“已损坏”，`src-tauri/tauri.conf.json` 将 `bundle.macOS.signingIdentity` 固定为 `"-"`。这要求 Tauri 对完整 `.app` 执行 ad-hoc 签名并生成资源封条；发布工作流随后用 `codesign --verify --deep --strict` 验证应用包。

这解决的是签名完整性问题，不是 Apple 身份认证。用户首次打开仍会遇到 Gatekeeper 的“无法验证开发者”提示，需要在 Finder 中按住 Control 打开，或在“系统设置 -> 隐私与安全性”中选择“仍要打开”。不应将 ad-hoc 签名描述为已签名或已公证。

发布前应确认每个 macOS 架构的任务都通过 `Verify macOS app signature`。若该步骤失败，不能发布该 DMG。

## 后续正式方案

当准备公开分发时，切换为 Apple Developer ID 签名并公证。现有 `release.yml` 已保留自动切换路径：只要配置以下 GitHub Actions Secrets，macOS job 就会使用 Developer ID 身份覆盖 ad-hoc 默认值，并在 Tauri 打包后提交公证。

| Secret                       | 用途                                               |
| ---------------------------- | -------------------------------------------------- |
| `APPLE_CERTIFICATE`          | Developer ID Application `.p12` 文件的 Base64 内容 |
| `APPLE_CERTIFICATE_PASSWORD` | `.p12` 导出密码                                    |
| `APPLE_ID`                   | 用于公证的 Apple Account 邮箱                      |
| `APPLE_PASSWORD`             | 该账号生成的 app-specific password                 |
| `APPLE_TEAM_ID`              | Apple Developer Team ID                            |

实施步骤：

1. 加入 Apple Developer Program，并为 `com.klaragraff.quill` 创建 Developer ID Application 证书。
2. 导出含私钥的 `.p12`，Base64 编码后写入 `APPLE_CERTIFICATE`，并配置证书密码。
3. 为用于公证的 Apple Account 创建 app-specific password，并配置剩余三个 Secrets。
4. 发布一个预发布标签，确认两种 macOS 架构的构建日志出现 Developer ID 身份与公证成功信息。
5. 下载 DMG 后验证：`codesign --verify --deep --strict --verbose=4 <App>.app`、`spctl --assess --type execute --verbose=4 <App>.app`，并确认 `spctl` 接受应用。
6. 仅在上述验证通过后，将正式 Release 发布，并再评估启用自动更新通道。

不要通过移除签名或建议用户执行 `xattr -cr` 来绕过发布问题；每个 Release 都必须从 GitHub 附件重新下载后验证。
