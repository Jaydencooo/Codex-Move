# 构建、签名与发布指南 (Release, Code Signing & Runtime Guide)

## 1. 本地可复现构建要求

### 1.1 开发环境依赖（仅构建者需要，最终用户无需安装）
- **Rust**：`1.77+`（建议使用 `rustup` 安装最新稳定版）
- **Node.js**：`20.x` 或 `22.x / 24.x`（仅用于编译前端静态资源）
- **系统编译工具链**：
  - **Windows**：Visual Studio C++ Build Tools 或 MinGW-w64 UCRT 工具链，以及 Windows SDK。
  - **macOS**：Xcode Command Line Tools (`xcode-select --install`)。

### 1.2 构建步骤
```bash
# 1. 安装前端依赖（锁定版本）
npm ci

# 2. 执行前端类型检查与测试
npm run typecheck
npm test

# 3. 执行 Rust 后端全量测试
cd src-tauri && cargo test && cd ..

# 4. 构建生产级桌面安装包
npm run tauri build
```

---

## 2. Windows WebView2 运行时检测与离线限制说明

> [!IMPORTANT]
> **关于运行时依赖的诚实声明**：
> Codex Move 使用 Tauri 2 构建，因此无需内置上百 MB 的 Chromium（Electron），安装包体积仅约数 MB。但 Tauri 在 Windows 上依赖系统内置的 **Microsoft Edge WebView2 Runtime** 来渲染界面。

1. **默认在线环境策略（DownloadBootstrapper）**：
   - Windows 11 及绝大多数已更新的 Windows 10（1803 及以上）系统均已预装 WebView2。
   - 若目标 Windows 电脑缺失 WebView2，安装包内置的引导程序会自动从微软官方 CDN 下载并安装 WebView2 Runtime。
2. **无外网 / 严格离线环境限制**：
   - 如果用户的旧电脑或新电脑处于**完全断开互联网的纯内网环境**，且系统为精简版 Windows 10（未预装 WebView2），则默认小体积安装包无法自动下载 WebView2。
   - **解决方案**：我们在 GitHub Releases 中同时说明了如何获取微软官方 WebView2 Evergreen Standalone Installer（离线安装包），或者在构建时切换 `tauri.conf.json` 中的 `webviewInstallMode.type = "offlineInstaller"` 生成内嵌运行时离线安装包。

---

## 3. 代码签名与 macOS 公证 (Code Signing & Notarization)

### 3.1 缺少签名凭据时的测试构建说明
- 在未配置商用代码签名证书（Windows EV/OV Authenticode 证书、Apple Developer ID Application 证书）时，GitHub Actions 与本地构建会生成**未签名（Unsigned / Ad-hoc Signed）安装包**。
- **系统提示说明（切勿关闭系统安全功能）**：
  - **Windows**：首次打开可能弹出 Microsoft Defender SmartScreen 提示（“Windows 已保护你的电脑”）。用户可通过点击“更多信息”→“仍要运行”启动（前提是核对 GitHub Release 提供的 SHA-256 校验和一致）。
  - **macOS**：首次打开未公证构建时，Gatekeeper 会提示“无法验证开发者”。用户可在验证 SHA-256 一致后，在“访达 (Finder)”中按住 `Control` 键点击应用选择“打开”，或在“系统设置 → 隐私与安全性”中点击“仍要打开”。**我们绝不建议用户执行 `spctl --master-disable` 等全局关闭系统安全防护的危险命令。**

### 3.2 正式签名所需的 GitHub Actions Secrets
当维护者准备好证书后，在仓库 `Settings -> Secrets and variables -> Actions` 中配置以下凭据即可激活自动签名与公证：
- **macOS 签名与公证**：
  - `APPLE_CERTIFICATE`：Base64 编码的 `.p12` 开发者证书
  - `APPLE_CERTIFICATE_PASSWORD`：证书导出密码
  - `APPLE_SIGNING_IDENTITY`：`Developer ID Application: ...`
  - `APPLE_ID` / `APPLE_PASSWORD`（App 专用密码）/ `APPLE_TEAM_ID`
- **Windows 代码签名**：
  - `WINDOWS_CERTIFICATE`：Base64 编码的 `.pfx` 证书或 Azure Trusted Signing 配置
  - `WINDOWS_CERTIFICATE_PASSWORD`：证书密码

---

## 4. 性能与资源占用测量方法

禁止虚构性能数字。每次构建发布包后，使用以下标准流程记录实测指标：

1. **安装包体积（GitHub Actions 实测构建产物）**：
   - **Windows x64 (`x86_64-pc-windows-msvc`)**：
     - NSIS 安装包 (`Codex Move_0.1.0_x64-setup.exe`)：**5.41 MiB**
     - MSI 安装包 (`Codex Move_0.1.0_x64_en-US.msi`)：**7.76 MiB**
   - **macOS Apple Silicon (`aarch64-apple-darwin`)**：
     - DMG 镜像 (`Codex Move_0.1.0_aarch64.dmg`)：**7.48 MB**
   - **macOS Intel (`x86_64-apple-darwin`)**：
     - DMG 镜像 (`Codex Move_0.1.0_x64.dmg`)：**7.86 MB**
2. **冷启动时间**：
   - 在 `main.rs` 入口记录 `Instant::now()`，在前端首屏 `useEffect` 完成首次 IPC 握手 (`get_system_overview`) 时记录耗时并输出到结构化日志 `startup_duration_ms`。
3. **空闲内存与流式传输峰值内存**：
   - 使用系统内置工具（Windows `Get-Process codex-move | Select-Object WorkingSet64, PrivateMemorySize64`，macOS `ps -o rss= -p <PID>`）记录首屏静止 10 秒后的内存占用，以及传输 1 GB Fixture 时的内存波动（验证有界 4 MiB 缓冲不发生持续内存增长）。
