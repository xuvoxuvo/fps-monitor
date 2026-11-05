# vendor

这个目录只有一个外部程序：

| 文件 | 说明 | 版本 | 许可证 |
| --- | --- | --- | --- |
| `PresentMon.exe` | [Intel PresentMon](https://github.com/GameTechDev/PresentMon) 命令行帧时间采集工具 | 2.5.1 | MIT |

`SHA256SUMS` 固定它的字节。`build.rs` 在编译前校验，不匹配直接构建失败；
`.github/workflows/build.yml` 还会运行 `scripts/verify-vendor.ps1` 独立校验，损坏的检出
不可能变成发布产物。

## 为什么只保留 PresentMon？

FPS 来自 Windows 的 ETW 帧事件（`Microsoft-Windows-DXGI`），PresentMon 已经把这套
复杂逻辑封装成一个稳定的 CLI：每个呈现帧输出一行 CSV，包含
`MsBetweenPresents` / `FrameTime` 等列。重新实现它不会减少什么，反而容易漏掉
swapchain 去重、DX9、多平面 overlay 等细节。

CPU 占用、GPU 占用、GPU 温度不需要 PresentMon：

* CPU：`GetSystemTimes`
* GPU 温度：`D3DKMTQueryAdapterInfo(ADAPTER_PERFDATA)`
* GPU 占用：PDH `GPU Engine` 计数器

因此 **没有 LibreHardwareMonitor、没有 .NET 程序集、没有 PawnIO、没有内核驱动**。
CPU 温度被有意移除——它必须加载驱动，和“零依赖”目标冲突。

## 文件夹版与单文件版

* **文件夹版**：`fps-monitor.exe` + `bin\PresentMon.exe`，保持相对位置即可。
* **单文件版**（`--features embed-assets`）：PresentMon 的字节编进 exe，首次运行解包到
  `%LOCALAPPDATA%\fps-monitor\runtime\<内容ID>\`：先写临时文件，再原子重命名，随后
  SHA-256 校验；损坏会自动重解包。

## 更新 PresentMon

1. 从官方 Release 下载新的 `PresentMon.exe`，覆盖本目录。
2. 运行 `./scripts/update-vendor-hashes.ps1`。
3. 把 exe 和 `SHA256SUMS` 放在同一个提交里。
4. 更新本表格版本号。

不要为了让构建通过而手改 `SHA256SUMS`，否则唯一的完整性检查就失效了。
