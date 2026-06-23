# Rust 跨平台客户端设计

**目标**

新增一个统一的 Rust 跨平台客户端，覆盖桌面、Android 和 iOS。客户端需要连接现有 `rmc-server`，展示媒体库内容，并支持播放 `rmc-server` 返回的 302 重定向 URL。首版以可运行、可验证的播放闭环为目标，不实现离线下载、账号体系重构或完整播放器内核。

**已确认决策**

- 使用 Dioxus 作为统一 UI 技术栈，面向 desktop、Android 和 iOS。
- 新客户端作为新的跨平台客户端骨架开发，不继续扩展现有 `iced` 版 `crates/rmc-client`。
- 现有 `iced` 客户端在新客户端可用后逐步退役；退役前保留源码，避免破坏当前仓库历史和已有测试。
- 首版播放层使用 WebView/HTML `video` 打开服务端播放 URL，依赖平台媒体栈跟随 302。
- 保留 `PlaybackAdapter` 边界，后续可以为桌面接入 MPV 或系统播放器，为 Android 接入 Media3，为 iOS 接入 AVPlayer。

**范围**

首版必须包含：

- 服务器地址配置，默认使用 `http://127.0.0.1:19000`，并允许用户修改。
- 媒体库列表，读取 `GET /api/v1/library/items`，优先支持可播放的 movie 和 episode。
- 媒体详情页，展示标题、年份、海报、简介、时长、文件大小等已有 API 字段。
- 播放页，支持 direct 和 transcode 两种播放模式。
- direct 模式使用 `/api/v1/movies/{id}/direct`，必须支持该端点返回 302 后继续播放。
- transcode 模式使用 `/api/v1/movies/{id}/stream.mp4`，用于源格式不适合直接播放的场景。
- 桌面、Android 和 iOS 共用状态管理、API 客户端、URL 生成逻辑和核心组件。

首版不包含：

- 原生 Media3、AVPlayer 或 MPV 集成。
- 离线缓存和下载。
- 复杂字幕选择、音轨选择和播放进度同步。
- 替换 `web-client` 或 Emby/Jellyfin 兼容接口。

**仓库结构**

新增：

- `crates/rmc-app`：Dioxus 跨平台客户端入口，包含桌面、Android 和 iOS 共用 UI。
- `crates/rmc-app/src/api.rs`：封装 `rmc-server` HTTP 请求。
- `crates/rmc-app/src/config.rs`：保存和读取服务端地址配置。
- `crates/rmc-app/src/playback.rs`：生成 direct/transcode 播放 URL，定义 `PlaybackAdapter`。
- `crates/rmc-app/src/app.rs`：应用状态、路由和消息处理。
- `crates/rmc-app/src/views/`：媒体库、详情页、播放页等 Dioxus 组件。

保留：

- `crates/rmc-client`：现有 `iced` 桌面客户端，暂不删除。
- `web-client`：现有 Web 客户端，不受本设计影响。
- `crates/rmc-server`：服务端现有播放端点继续作为客户端契约来源。

**架构**

客户端分为四层：

1. API 层：负责调用 `rmc-server`，把 HTTP 错误转换成用户可读错误。
2. 状态层：保存服务器地址、当前列表、当前详情、播放模式和加载错误。
3. UI 层：Dioxus 组件，按 desktop/mobile 响应式布局渲染同一套状态。
4. 播放层：首版生成播放 URL 并交给 WebView `video`，后续由 `PlaybackAdapter` 分发到平台原生播放器。

三端差异仅允许存在于启动、窗口/安全区域适配、配置持久化位置和未来原生播放器适配中。媒体库请求、播放 URL 生成和错误处理必须共用。

**播放 URL 规则**

客户端只基于 `play_id` 生成播放 URL：

- direct: `{server_base_url}/api/v1/movies/{play_id}/direct`
- transcode: `{server_base_url}/api/v1/movies/{play_id}/stream.mp4`

客户端不主动解析 302。播放组件必须把 direct URL 交给媒体元素或平台播放器，让底层 HTTP/媒体栈跟随 `Location`。这保持客户端简单，并与浏览器/WebView 播放行为一致。

如果 direct 播放失败，UI 提供切换到 transcode 的明确入口。首版不自动重试 transcode，避免掩盖 direct 播放失败原因。

**错误处理**

- 服务端地址为空或非法时，在配置页阻止保存。
- 连接失败时展示可操作错误，提示检查 `rmc-server` 地址。
- 媒体库 API 返回非 2xx 时展示状态码和响应摘要。
- 播放页收到无效 `play_id` 时返回详情页并显示错误。
- direct 播放失败时保留当前页面，提示用户切换到转码模式。

**测试策略**

单元测试：

- `PlaybackUrlBuilder` 对 direct/transcode URL 的生成。
- `ServerConfig` 对默认地址、用户输入和尾部斜杠的规范化。
- API 层对非 2xx 响应的错误转换。

组件/状态测试：

- 媒体库加载成功后渲染可播放条目。
- 可播放条目进入详情页后生成正确 direct/transcode 链接。
- 播放模式切换不改变 `play_id`。

集成验证：

- `cargo test -p rmc-app` 验证客户端核心逻辑。
- `cargo check -p rmc-app` 验证桌面目标。
- Android/iOS 的真实打包命令在本机工具链可用时运行；如果当前环境缺少 Android SDK/NDK 或 Xcode，只报告环境缺口，不声称移动端打包已通过。

302 播放验收：

- 使用测试 HTTP 服务返回 `/api/v1/movies/1/direct -> 302 Location: /cdn/movie.mp4`。
- 客户端播放页必须把 direct URL 原样交给播放组件。
- URL 生成测试必须覆盖包含和不包含尾部斜杠的服务端地址。

**迁移策略**

第一阶段新增 `rmc-app`，不改动现有 `rmc-client` 行为。

第二阶段当 `rmc-app` 具备桌面可运行能力后，在 README 或开发文档中把 `rmc-app` 标记为推荐客户端。

第三阶段移除或归档 `rmc-client` 前，需要确认没有测试、文档或构建脚本仍依赖它。

**成功标准**

- 仓库包含统一 Rust 跨平台客户端骨架。
- 桌面目标能通过 `cargo check`。
- 核心 URL、配置和 API 行为有自动化测试。
- 播放页支持 direct/transcode 两种模式。
- direct 模式明确支持 `rmc-server` 302 重定向播放契约。
- Android/iOS 后续实现路径不需要重写 UI、状态或 API 层。
