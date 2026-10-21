# Listary 对 winit 0.30.13 的补丁

本分支 `listary/0.30.13` 从 `v0.30.13` 标签拉出，只放 Listary 需要的改动，由 Listary 仓库 `app/Cargo.toml` 的 `[patch.crates-io]` 以 git 源顶替 crates.io 的 winit 0.30.13。`v0.30.13` 的 `src/` 与 crates.io 发布包逐字相同（发布包里的 `Cargo.toml.orig` 就是仓库的 `Cargo.toml`），包版本没有改变。上游 [Apache-2.0](LICENSE) 许可证不变。

Slint 1.18 的 winit 后端、`accesskit_winit`、`glutin-winit` 都依赖 winit 0.30，顶替后它们用的是同一份。下文 `app/` 开头的路径都在 Listary 仓库。

## 改了什么

### 处理任务栏重建广播时先放锁（taskbar-created-deadlock）

[src/platform_impl/windows/event_loop.rs](src/platform_impl/windows/event_loop.rs) 的 `public_window_callback_inner` 里，`TASKBAR_CREATED` 分支原来拿着 `window_state_lock()` 调 `set_skip_taskbar`。现在先把 `skip_taskbar` 读出来、放掉锁，再调用。带 `LISTARY PATCH` 注释。

这是上游 [rust-windowing/winit#4563](https://github.com/rust-windowing/winit/pull/4563)（提交 `1fe178b2`，2026-07-27 合进 master）的回补，改法与上游相同。上游 0.30 线没有回补，crates.io 上 0.30 线最新仍是 0.30.13；0.31 已带这处修复，但 Slint 1.18 要求 0.30。

为什么要改：主屏缩放变化、插拔或切换主屏、Explorer 重启时，Windows 向所有顶层窗口广播 `TaskbarCreated`（[微软文档](https://learn.microsoft.com/en-us/windows/win32/shell/taskbar#taskbar-creation-notification)）。`set_skip_taskbar` 经 `ITaskbarList` 跨进程 `SendMessage` 给 Explorer，等待期间本线程会处理别人发来的同步消息。其中一条进到同一个窗口的 winit 回调、再拿同一把锁时，主线程就自己锁死了。Listary 在虚拟机里改缩放时撞上过一次，转储的主线程栈正是这条路径。

这处补丁只消掉这一条路径。winit 窗口过程里还有别处拿着锁调 Win32 接口（抓鼠标、光标裁剪、尺寸查询），没有发现第二处会跨进程等任务栏的，这里没有改动。

### dpi 从 crates.io 取（dpi-from-registry）

[Cargo.toml](Cargo.toml) 里 `dpi` 依赖去掉了 `path = "dpi"`，只留版本要求 `0.1.1`，带 `LISTARY PATCH` 注释。crates.io 发布包的清单本来就没有这个路径，所以这样解析出的依赖和用 crates.io 的 winit 0.30.13 时相同（现在是 dpi 0.1.2）。

不改的话，从 git 取 winit 会顺带用仓库里的 dpi 0.1.1，和别的包（如 muda）从 crates.io 取的 dpi 并存两份，两边的 dpi 类型对不上。只动了清单，没有动源码。

## 升级或撤除时核对

1. 上游 0.30 线发了带 #4563 的版本，或 Slint 换到已带修复的 winit 时，Listary 改回 registry 版本，删掉 `[patch.crates-io]` 里的 winit 一行。还要保留时，从新版本的标签拉新分支，把本分支的补丁提交搬过去。
2. 新版本里找 `TASKBAR_CREATED` 的分支，确认调 `set_skip_taskbar` 时不再持有 `window_state_lock()`。
3. 在 `app/` 执行 `cargo tree -p listary --edges normal,build --locked -i winit` 和 `-i dpi`，确认 winit 和 dpi 各只有一份，winit 来源是本分支、dpi 来自 crates.io。`app/crates/listary-ui/tests/slint_discipline.rs` 也按锁文件检查 winit 的来源。
4. 补丁只改函数体，丢失后不会有编译错误。行为验证在虚拟机上做：开着欢迎窗口和文件搜索窗口，主屏缩放 100% 和 150% 来回改多次，再重启一次 Explorer，Listary 不应卡死。
