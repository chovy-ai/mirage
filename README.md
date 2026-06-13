# mirage

用 Rust + egui/eframe（wgpu 渲染）复刻 macOS 桌面窗口管理的基础骨架：自绘桌面、菜单栏、窗口、Dock 与 Launchpad，应用本身只是「壳」。

## 开源依赖选型

能用开源就不造轮子：**egui_term**（终端，alacritty_terminal 后端）、**egui_commonmark**（Markdown，pulldown-cmark）、**wry**（webview，Tauri）、**serde / image / chrono**。
ACP 协议层目前是手写的 ~300 行 JSON-RPC 客户端：官方 `agent-client-protocol` crate 基于 tokio 异步模型，接入需要引入 runtime 并重构已验证的线程模型，暂保留手写实现（接口同构，后续可平移）。

## 运行

```bash
cargo run
```

开发自检（自动走一遍关键场景、截图后退出，用于无人值守验证渲染）：

```bash
MIRAGE_SHOT=/tmp/mirage cargo run
# 生成 /tmp/mirage-{desktop,genie,launchpad,menu,tiled}.png
```

### 字体（macOS 系统字体栈）

界面字体严格使用 macOS 系统字体：西文/数字 = SF Pro（`SFNS.ttf`）、中文 = 苹方
（`PingFang.ttc`，Hiragino 备选）、等宽 = SF Mono → Menlo，Arial Unicode 殿后补
冷门字形（`main.rs install_macos_fonts`）。egui 不会对 fallback 字体做基线对齐，
苹方相对 SF 的字形偏高，因此苹方/Hiragino 带 `FontTweak::y_offset_factor = 0.26`：
该值的校准方法是分别截图本应用与真 macOS 菜单栏的中英混排时钟（如「6月13日 周六
05:10」），逐字测量字形的像素上下沿——真 macOS 中汉字相对数字基线上下各超出约
2 物理像素（对称），调整系数直到本应用得到相同分布。改字体后用
`MIRAGE_APPSHOT=finder` 截图复核访达列表的「2026年05月03日 21:47」混排行。
排查字体问题可用 `MIRAGE_PLAIN_FONTS=1` 退回单一 Arial Unicode 回退。

### 控件（苹果风格，自建）

调研过 egui 生态后确认**没有现成的 macOS 控件库**：egui-desktop 只做窗口
chrome/菜单（egui 0.33、alpha），第三方主题库（catppuccin 等）只是配色；
嵌入真原生 NSSlider/NSScroller 又有「原生子视图永远浮在 egui 画面之上」的
层级限制（与 webview 同款）。因此 [ui/widgets.rs](src/ui/widgets.rs) 对照
macOS System Settings 自建了滑杆（蓝色填充轨 + 白圆钮）与开关（38×22 胶囊、
带滑动动画），系统设置与照片缩放均使用；滚动条用 egui 内建
`ScrollStyle::floating`，形态即 macOS 覆盖式滚动条。

### 图标（macOS 系统原版）

Dock/Launchpad/窗口图标优先读 macOS 系统真实图标（[ui/icon.rs](src/ui/icon.rs)
`system_icon_path`）：各系统应用的 `.icns`（手动解析容器取内嵌 PNG，无新增依赖）、
废纸篓用 Dock 自己的 `trashempty@2x.png`/`trashfull@2x.png` 且按 `~/.Trash`
是否为空选图。日历刻意保持程序化绘制——真 macOS Dock 的日历就是动态显示当天
日期。Codex/Claude/启动台无系统对应物，走程序化精绘；任何 `.icns` 读不到时
（如未装 Chrome）也回退到程序化图标。

## 接入的真实应用

- **Codex Agent 与 Claude Code**（[codex.rs](src/codex.rs) + [ui/agent.rs](src/ui/agent.rs)）：同一套 ACP 客户端（JSON-RPC 2.0 over stdio）驱动 `codex-acp` 和 `claude-agent-acp` 两个后端，复刻 nextop `packages/agent` 的主流程与 GUI 结构。消息流：用户气泡、**助手 Markdown 渲染**（egui_commonmark / pulldown-cmark）、思考折叠、工具卡片（状态点 + diff +N/-N 统计 + 文件位置 + 输出折叠）、**Plan 计划清单**（○/◐/✓ 状态）、**上下文用量**（usage_update）、composer + 发送/停止。权限请求自动批准。
- **访达**（[ui/finder.rs](src/ui/finder.rs)）：真实浏览本地文件系统——「个人收藏」侧边栏、前进/后退、列表视图（名称/修改日期/大小/种类、斑马纹、选中高亮）、底部面包屑路径栏；双击进目录、双击文件交给系统默认应用打开。
- **Chrome 浏览器**（[ui/browser.rs](src/ui/browser.rs)）：wry（WKWebView）原生子视图，对标 nextop Browser Node。地址栏、前进/后退/刷新、bounds 跟随、聚焦显隐。
- **地图**（[ui/maps.rs](src/ui/maps.rs)）：第二个 webview 实例加载 OpenStreetMap，与 Chrome 并存。
- **提醒事项**（[ui/reminders.rs](src/ui/reminders.rs)）：macOS 风格圆形勾选、添加/完成/删除、未完成计数，JSON 持久化到 `~/MirageWorkspace/reminders.json`。
- **照片**（[ui/photos.rs](src/ui/photos.rs)）：一比一还原 macOS Photos——扫描 `~/Pictures`/`~/Desktop`/`~/Downloads` 的本机图片，侧边栏（图库/相簿）、「年份/月份/日期/所有照片」分段控件、缩放滑杆、正方形缩略图网格（3 worker 线程懒加载、可见才解码）、双击进单张查看器（2048px 原图、←/→ 切换、Esc/返回、日期与序号）；HEIC 经 `sips` 转码。
- **终端**（[ui/terminal.rs](src/ui/terminal.rs)）：开源方案 **egui_term**（Alacritty 抽出的 `alacritty_terminal` 后端）——真 PTY 终端，跑你的登录 shell（oh-my-zsh 提示符、vim、htop、着色、补全都支持）。
- **回收站**（[ui/trash.rs](src/ui/trash.rs)）：浏览 `~/.Trash`（只读）。
- **系统设置**（[ui/settings.rs](src/ui/settings.rs) + [config.rs](src/config.rs)）：4 套壁纸主题实时切换、Dock 大小/放大滑杆、时钟秒数。
- **微信**（外部应用机制）：闭源原生 GUI 无法嵌入窗口，点 Dock/Launchpad 图标用 `open -a` 拉起真实微信。
- **应用图标**（[ui/icon.rs](src/ui/icon.rs)）：全部程序化精绘（访达双色脸、Chrome 四色环、Claude 星芒、微信双气泡、动态日期日历……）。

窗口内容默认可见：未聚焦窗口照常渲染，仅禁用交互（egui 命中测试不知道手绘窗口的遮挡关系，禁用可防止被盖住的控件误响应）；webview 类（Chrome/地图）受原生视图永远置顶的限制，仅聚焦时显示。

> 运行前提：Codex 需本机已安装 `codex-acp`（`npm i -g @zed-industries/codex-acp`）并 `codex login`。
> 已知问题：从 `.app` bundle（经 LaunchServices/launchd）启动时，codex-acp 的 `session/new` 在该启动上下文下会静默卡住（终端 `cargo run`、独立进程复现均正常，0.2s 返回）；已加 12s 看门狗超时提示。开发期请用 `cargo run` 启动，Codex 即可正常连接。

## 已实现

- **窗口管理**：标题栏拖拽、8 向边缘缩放（含光标形状）、点击聚焦置顶、z-order、聚焦/失焦视觉区分、同应用多窗口
- **红绿灯**：关闭 / 最小化 / 最大化（zoom），悬停显示按钮符号；双击标题栏 = zoom；`Cmd+N` 新建、`Cmd+W` 关闭、`Cmd+M` 最小化
- **边缘吸附平铺**：拖到左右边 = 半屏、四角 = 四分之一屏、顶部 = 最大化，带磨砂预览框，松手动画归位
- **Genie 神灯动画**：最小化/恢复时窗口切片曲面变形，先弯折后下坠吸入 Dock 图标位
- **Dock**：鼠标邻近放大波浪（余弦衰减 + 重排居中）、启动弹跳动画、运行指示点、悬停 tooltip 气泡、分隔线、最小化窗口停靠区、磨砂面板
- **Launchpad**：Dock 图标开关、毛玻璃霜化背景（壁纸重绘把窗口隐入背景）+ zoom-out 入场动画、应用网格（末行居中）、实时搜索过滤、Esc / 点击空白关闭
- **菜单栏**：全套下拉菜单（点击打开、悬停切换、蓝色高亮、快捷键标注、禁用态），「新建窗口 / 关闭 / 最小化 / 缩放 / 系统设置」为真功能；实时时钟、苹果 logo、电池
- **动画**：统一 Tween + easing——打开缩放淡入（带回弹）、关闭缩小淡出、genie、最大化/平铺尺寸过渡

## 架构

```
src/
├── main.rs        # eframe 入口、输入路由（菜单 > Dock > Launchpad > 窗口）、自检/应用截图模式
├── anim.rs        # Tween + easing，所有动效共用
├── apps.rs        # 应用注册表（id / 名称 / 配色 / fallback 字形）
├── codex.rs       # Codex ACP 客户端（codex-acp 子进程 + JSON-RPC over stdio）
├── config.rs      # DesktopConfig：壁纸主题 / Dock 大小与放大 / 时钟秒数
├── wm.rs          # 窗口管理纯逻辑：z-order、焦点、动画状态机、边缘命中（不依赖渲染）
└── ui/
    ├── desktop.rs   # 壁纸（主题渐变 + 柔光，支持透明度用于 Launchpad 霜化）
    ├── menubar.rs   # 菜单栏 + 下拉菜单
    ├── chrome.rs    # 窗口外观：阴影/圆角/标题栏/红绿灯/genie + 动画状态换算
    ├── dock.rs      # Dock 布局（两遍：基础中心算放大 -> 重排居中）与绘制
    ├── launchpad.rs # 应用中心
    ├── agent.rs     # Codex 聊天界面（复刻 nextop Agent GUI）
    ├── browser.rs   # Chrome 浏览器窗口（wry webview）
    ├── terminal.rs  # 终端（真实命令执行）
    ├── trash.rs     # 回收站（浏览 ~/.Trash）
    ├── settings.rs  # 系统设置（实时改 DesktopConfig）
    └── icon.rs      # 程序化精绘应用图标（渐变 squircle + 专属图形）
```

应用截图回归（验证单个应用内容渲染）：

```bash
MIRAGE_APPSHOT="terminal,trash,settings" cargo run   # 生成 /tmp/appshot-<id>.png
```

设计原则：`wm` 是纯数据结构 + 纯函数，egui 只是渲染与输入适配层，便于单测和将来替换渲染端。

接入真实应用内容：实现窗口内容渲染（目前在 `chrome.rs` 的壳内容段），给 `apps.rs` 的条目挂一个 `render(ui)` 回调即可。

## 后续扩展点

- Mission Control、多桌面 Spaces
- 桌面图标、右键菜单
- 真实背景模糊（egui 单 pass 无后处理；需在 wgpu 层加离屏 blur pass 才能模糊到窗口内容）
- `dist/Mirage.app` 是用于系统集成测试的最小 bundle，`cp target/debug/mirage dist/Mirage.app/Contents/MacOS/` 可更新
