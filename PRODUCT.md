# Product

## Register

product

## Users

项目作者本人（开发者，中文环境）。在 macOS 上运行这个 Rust 应用,把它当成"桌面里的桌面"来把玩与演示:打开窗口、拖拽、最小化、用 Dock 和 Launchpad、跑真实的终端 / Agent / 浏览器。

## Product Purpose

用 Rust + egui 复刻 macOS 桌面体验(窗口管理、Dock、Launchpad、菜单栏、系统应用壳)。成功标准:截图乍看像一台真的 Mac;交互动画(genie 最小化、Dock 放大、Launchpad 缩放)像 macOS 一样顺滑自然。

## Brand Personality

原生、克制、精致。视觉语言完全跟随 Apple macOS(Sonoma/Sequoia 代):半透明材质、细腻的层次阴影、SF 风格的排版节奏、系统级的灰阶与强调蓝。

## Anti-references

- 不要"一眼 egui/imgui 默认皮肤":默认深灰面板、生硬的 1px 边框、默认间距。
- 不要 Windows / Linux 桌面的视觉语言(直角窗口、粗标题栏)。
- 不要过度装饰:不加 macOS 本身没有的渐变、彩色边框、花哨动效。

## Design Principles

1. **以 macOS 为唯一基准**:每个控件、间距、颜色先问"真 macOS 长什么样",而不是"egui 默认给什么"。
2. **材质与层次**:窗口、Dock、菜单是悬浮的半透明材质,靠阴影和描边分层,不靠粗边框。
3. **动画传达状态**:150–400ms、ease-out;动画只用于窗口/图标状态变化,不做装饰性动效。
4. **应用是壳,但壳要可信**:工具栏、侧边栏、列表的布局密度和字号要像真应用,内容可以假,骨架不能假。
5. **一致的视觉词汇**:所有应用窗口共用同一套 chrome、强调色、选中态、分隔线规则。

## Accessibility & Inclusion

- 文字对比满足可读性(浅色材质上用深灰墨色,不用中灰)。
- 全界面中英文混排,CJK 字体回退必须保留。
- 动画均有明确时长上限,无闪烁。
