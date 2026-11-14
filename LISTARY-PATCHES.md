# Listary 对 winit 0.30.13 的补丁

本分支 `listary/0.30.13` 从 `v0.30.13` 标签拉出，只放 Listary 需要的改动，由 Listary 仓库 `app/Cargo.toml` 的 `[patch.crates-io]` 以 git 源顶替 crates.io 的 winit 0.30.13。`v0.30.13` 的 `src/` 与 crates.io 发布包逐字相同（发布包里的 `Cargo.toml.orig` 就是仓库的 `Cargo.toml`），包版本没有改变。上游 [Apache-2.0](LICENSE) 许可证不变。

Slint 1.18 的 winit 后端、`accesskit_winit`、`glutin-winit` 都依赖 winit 0.30，顶替后它们用的是同一份。下文 `app/` 开头的路径都在 Listary 仓库。

## 改了什么

### 处理任务栏重建广播时先放锁（taskbar-created-deadlock）

[src/platform_impl/windows/event_loop.rs](src/platform_impl/windows/event_loop.rs) 的 `public_window_callback_inner` 里，`TASKBAR_CREATED` 分支原来拿着 `window_state_lock()` 调 `set_skip_taskbar`。现在先把 `skip_taskbar` 读出来、放掉锁，再调用。带 `LISTARY PATCH` 注释。

这是上游 [rust-windowing/winit#4563](https://github.com/rust-windowing/winit/pull/4563)（提交 `1fe178b2`，2026-07-27 合进 master）的回补，改法与上游相同。上游 0.30 线没有回补，crates.io 上 0.30 线最新仍是 0.30.13；0.31 已带这处修复，但 Slint 1.18 要求 0.30。

为什么要改：主屏缩放变化、插拔或切换主屏、Explorer 重启时，Windows 向所有顶层窗口广播 `TaskbarCreated`（[微软文档](https://learn.microsoft.com/en-us/windows/win32/shell/taskbar#taskbar-creation-notification)）。`set_skip_taskbar` 经 `ITaskbarList` 跨进程 `SendMessage` 给 Explorer，等待期间本线程会处理别人发来的同步消息。其中一条进到同一个窗口的 winit 回调、再拿同一把锁时，主线程就自己锁死了。Listary 在虚拟机里改缩放时撞上过一次，转储的主线程栈正是这条路径。

这处补丁只消掉这一条路径。winit 窗口过程里还有别处拿着锁调 Win32 接口（抓鼠标、光标裁剪、尺寸查询），没有发现第二处会跨进程等任务栏的，这里没有改动。

### 拖放目标应答 QueryInterface（drop-target-query-interface）

[src/platform_impl/windows/drop_handler.rs](src/platform_impl/windows/drop_handler.rs) 里拖放目标的 `QueryInterface` 原来是 `unimplemented!()`。现在问 `IUnknown` 或 `IDropTarget` 时把同一个指针加一次引用后交出去，返回 `S_OK`；问别的接口返回 `E_NOINTERFACE`；输出指针或接口 ID 为空时返回 `E_POINTER`。只要输出指针不为空，失败时先把它置空。带 `LISTARY PATCH` 注释。

为什么要改：winit 给每扇窗都用 `RegisterDragDrop` 注册这个拖放目标。系统本身不调它的 `QueryInterface`，但挂钩 OLE 的第三方模块（防泄密、文档加密软件）会调。这是 `extern "system"` 回调，panic 出不去，进程当场中止。Listary 7.1 在装了这类软件的机器上建第一扇窗就退出，Sentry 收到过多次。

上游在 0.31 重写拖放时修好了（[rust-windowing/winit#4571](https://github.com/rust-windowing/winit/pull/4571)），0.30 线没有回补，Slint 1.18 要求 0.30。

同一文件末尾的单元测试经虚表调 `QueryInterface`，跟第三方模块的调法一样：两种接口取到同一个指针、引用计数加减平衡、别的接口被拒且输出置空、空指针返回 `E_POINTER`。没有这处补丁，第一次调用就让测试进程中止。

### dpi 从 crates.io 取（dpi-from-registry）

[Cargo.toml](Cargo.toml) 里 `dpi` 依赖去掉了 `path = "dpi"`，只留版本要求 `0.1.1`，带 `LISTARY PATCH` 注释。crates.io 发布包的清单本来就没有这个路径，所以这样解析出的依赖和用 crates.io 的 winit 0.30.13 时相同（现在是 dpi 0.1.2）。

不改的话，从 git 取 winit 会顺带用仓库里的 dpi 0.1.1，和别的包（如 muda）从 crates.io 取的 dpi 并存两份，两边的 dpi 类型对不上。只动了清单，没有动源码。

### 拿焦点时不补报 AltGr 带出的假左 Ctrl（altgr-fake-ctrl-on-focus）

[src/platform_impl/windows/keyboard.rs](src/platform_impl/windows/keyboard.rs) 的 `synthesize_kbd_state` 在窗口拿到或失去焦点时，把按着的修饰键补报成按键事件。现在当前布局有 AltGr、右 Alt 又按着时，不补报左 Ctrl（新函数 `is_alt_graph_fake_ctrl`）。带 `LISTARY PATCH` 注释。

为什么要改：德语、法语这类带 AltGr 的布局，按住 AltGr 时系统自己按下一个假的左 Ctrl。winit 平时按 `is_current_fake` 把这个假 Ctrl 的按下和抬起都丢掉，但拿焦点时读的是键盘状态，分不出真假，就补报了一个左 Ctrl 按下；用户松开 AltGr 时，假 Ctrl 的抬起又被丢掉。Slint 的修饰键只靠按键事件维护，于是一直当 Ctrl 按着：用 AltGr+键 的全局热键叫出 Listary 后，打的每个字都带 Ctrl，搜索框打不进字，`c` 还被当成 Ctrl+C。判断真假的条件和 `is_current_fake`、`get_agnostic_mods` 相同：布局有 AltGr 且右 Alt 按着。真的同时按着左 Ctrl 和 AltGr 拿焦点时，补报会少一个 Ctrl，松开左 Ctrl 时收到一个没有按下的抬起，Slint 照常当作「没按 Ctrl」。

上游 master 的 `synthesize_kbd_state` 也是这样补报（2026-11 查过），没有修。

## 升级或撤除时核对

1. 上游 0.30 线发了带 #4563 和拖放修复的版本，或 Slint 换到已带这两处修复的 winit（0.31 起）时，Listary 改回 registry 版本，删掉 `[patch.crates-io]` 里的 winit 一行。还要保留时，从新版本的标签拉新分支，把本分支的补丁提交搬过去。
2. 新版本里找 `TASKBAR_CREATED` 的分支，确认调 `set_skip_taskbar` 时不再持有 `window_state_lock()`；找拖放目标的 `QueryInterface`，确认不再是 `unimplemented!()`；找拿焦点时补报按键的地方（现在是 `synthesize_kbd_state`），确认右 Alt 按着时不补报 AltGr 的假左 Ctrl，没有就把这处补丁搬过去。
3. 在 `app/` 执行 `cargo tree -p listary --edges normal,build --locked -i winit` 和 `-i dpi`，确认 winit 和 dpi 各只有一份，winit 来源是本分支、dpi 来自 crates.io。`app/crates/listary-ui/tests/slint_discipline.rs` 也按锁文件检查 winit 的来源。
4. 拖放补丁在 Windows 上跑 `cargo test --lib drop_handler` 验证，AltGr 补丁跑 `cargo test --lib keyboard`（只测判断函数）。AltGr 的行为验证在虚拟机上做：加德语布局，把 Listary 热键设成 Ctrl+Alt+Ö，用 AltGr+Ö 叫出启动器后打 abc 要能进字。任务栏补丁只改函数体，丢失后不会有编译错误。行为验证在虚拟机上做：开着欢迎窗口和文件搜索窗口，主屏缩放 100% 和 150% 来回改多次，再重启一次 Explorer，Listary 不应卡死。
