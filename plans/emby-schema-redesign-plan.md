# Emby 协议兼容与媒体库表结构重设计计划

## 最终目标

把当前以 `movies` 单表为中心的媒体库，演进为可以稳定表达 Emby/Jellyfin 客户端核心协议语义的通用媒体库模型，并让常用查询在大媒体库下保持可分页、可排序、可索引。

目标完成后应满足：

- Emby 客户端可以稳定浏览 `CollectionFolder -> Series/Movie -> Season -> Episode` 层级。
- `/Items`、`/Users/{UserId}/Items`、`/Items/{Id}`、`/Items/{Id}/PlaybackInfo`、`/Users/{UserId}/Items/Latest` 等核心接口优先由数据库查询驱动，而不是全量加载后内存过滤。
- 播放进度、已播放、收藏、播放次数等 `UserData` 可以按用户持久化。
- 文件、STRM、媒体源、视频/音频/字幕流信息有独立存储，为 DirectPlay、DirectStream、Transcoding 决策提供数据基础。
- 旧的本地 API 和现有扫描流程可以通过兼容层平滑迁移，不一次性破坏现有客户端。

## 当前问题

当前结构主要问题不是缺少几个索引，而是实体边界不足：

- 只有 `movies` 表，电影、剧集分集、STRM 源、元数据全部混在同一行。
- Series、Season、Episode 不是持久化实体，而是每次从路径和文件名推导。
- Emby 查询路径会反复 `get_available_movies()`，再构建 `MediaTree` 做内存分页、过滤、排序。
- `UserData` 固定返回默认值，播放上报接口没有落库。
- `PlaybackInfo` 里缺少持久化的媒体流、字幕、音轨、码率、容器等信息。

这会导致大库下查询成本高，也会让 Emby 客户端依赖的稳定 ID、父子层级、用户状态和播放能力声明不够可靠。

## 目标数据模型

### libraries

保存媒体库入口。

关键字段：

- `id`
- `name`
- `collection_type`: `movies` / `tvshows` / `mixed`
- `path`
- `created_at`
- `updated_at`

### media_items

统一保存 Emby 视角的 Item。

关键字段：

- `id`
- `library_id`
- `parent_id`
- `item_type`: `CollectionFolder` / `Movie` / `Series` / `Season` / `Episode`
- `title`
- `sort_title`
- `original_title`
- `overview`
- `production_year`
- `index_number`
- `parent_index_number`
- `premiere_date`
- `date_created`
- `date_modified`
- `runtime_ticks`
- `provider_tmdb_id`
- `provider_imdb_id`
- `provider_tvdb_id`
- `is_folder`
- `is_virtual`

核心索引：

```sql
CREATE INDEX idx_media_items_parent_type_sort
ON media_items(parent_id, item_type, sort_title);

CREATE INDEX idx_media_items_library_type_created
ON media_items(library_id, item_type, date_created DESC);

CREATE INDEX idx_media_items_parent_indexes
ON media_items(parent_id, parent_index_number, index_number);

CREATE INDEX idx_media_items_type_sort
ON media_items(item_type, sort_title);
```

### media_files

保存实际文件和扫描状态。

关键字段：

- `id`
- `library_id`
- `path`
- `canonical_path`
- `extension`
- `size`
- `modified_at`
- `hash`
- `is_available`
- `last_seen_at`

核心索引：

```sql
CREATE UNIQUE INDEX idx_media_files_path
ON media_files(canonical_path);

CREATE INDEX idx_media_files_library_seen
ON media_files(library_id, last_seen_at);
```

### media_sources

一个可播放 Item 可以有一个或多个播放源。

关键字段：

- `id`
- `item_id`
- `file_id`
- `source_type`: `local` / `strm` / `remote`
- `protocol`
- `path`
- `container`
- `size`
- `runtime_ticks`
- `bitrate`
- `supports_direct_play`
- `supports_direct_stream`
- `supports_transcoding`

核心索引：

```sql
CREATE INDEX idx_media_sources_item
ON media_sources(item_id);
```

### media_streams

保存 ffprobe 得到的流信息。

关键字段：

- `id`
- `source_id`
- `stream_type`: `Video` / `Audio` / `Subtitle`
- `index_number`
- `codec`
- `language`
- `title`
- `width`
- `height`
- `channels`
- `bitrate`
- `is_default`
- `is_forced`
- `is_external`

核心索引：

```sql
CREATE INDEX idx_media_streams_source_type
ON media_streams(source_id, stream_type);
```

### images

保存 Emby 图片类型。

关键字段：

- `id`
- `item_id`
- `image_type`: `Primary` / `Backdrop` / `Logo` / `Thumb`
- `url`
- `path`
- `tag`
- `width`
- `height`

核心索引：

```sql
CREATE INDEX idx_images_item_type
ON images(item_id, image_type);
```

### user_item_data

保存用户播放状态。

关键字段：

- `id`
- `user_id`
- `item_id`
- `playback_position_ticks`
- `play_count`
- `is_favorite`
- `played`
- `last_played_at`
- `updated_at`

核心索引：

```sql
CREATE UNIQUE INDEX idx_user_item_data_user_item
ON user_item_data(user_id, item_id);

CREATE INDEX idx_user_item_data_user_played
ON user_item_data(user_id, played, last_played_at DESC);
```

### playback_sessions

保存当前播放会上报状态。

关键字段：

- `id`
- `user_id`
- `item_id`
- `media_source_id`
- `device_id`
- `client_name`
- `play_method`
- `position_ticks`
- `is_paused`
- `started_at`
- `updated_at`
- `stopped_at`

### media_items_fts

搜索索引。

建议字段：

- `title`
- `original_title`
- `overview`
- `path_hint`

## 迁移策略

### 阶段 1：引入新 schema 和兼容读取层

目标：

- 新增迁移逻辑，创建目标表。
- 保留 `movies` 表。
- 新增 repository 查询接口，但暂不替换所有 HTTP handler。
- 从现有 `movies` 行同步生成 `media_items`、`media_files`、`media_sources`。

验收：

- 现有测试全部通过。
- 新增测试覆盖 `movies -> media_items` 迁移。
- 同一个电影旧 API 和新 repository 查到的标题、年份、路径、运行时一致。

### 阶段 2：持久化 Series / Season / Episode 层级

目标：

- 扫描时直接生成或更新 `Series`、`Season`、`Episode`。
- `MediaTree::from_movies` 退化为兼容工具，主查询改为数据库父子查询。
- 保证 Item ID 稳定，不再依赖每次即时 slug 推导。

验收：

- `/Items?ParentId=tvshows` 直接查出 Series。
- `/Items?ParentId={seriesId}` 直接查出 Season。
- `/Items?ParentId={seasonId}` 直接查出 Episode。
- 支持 `StartIndex`、`Limit`、`SortBy`、`SortOrder` 的数据库分页排序。

### 阶段 3：替换 Emby Items 查询路径

目标：

- `/Items`
- `/Users/{UserId}/Items`
- `/Users/{UserId}/Items/Latest`
- `/Items/{Id}`
- `/Users/{UserId}/Items/{Id}`

全部通过新 repository 查询。

验收：

- 不再为普通列表查询全量加载所有电影。
- `ParentId`、`Ids`、`IncludeItemTypes`、`Recursive`、`SearchTerm` 组合查询可工作。
- 大库下分页查询只返回当前页所需行。

### 阶段 4：落库 UserData 和播放上报

目标：

- `/Sessions/Playing`
- `/Sessions/Playing/Progress`
- `/Sessions/Playing/Stopped`

写入 `playback_sessions` 和 `user_item_data`。

验收：

- 进度上报后，重新请求同一 Item 能返回正确 `PlaybackPositionTicks`。
- 停止播放接近片尾时可以标记 `Played=true`。
- 收藏、播放次数、最近播放时间按用户隔离。

### 阶段 5：完善 PlaybackInfo 和媒体流

目标：

- ffprobe 结果写入 `media_streams`。
- `PlaybackInfo.MediaSources[].MediaStreams` 返回真实音视频字幕流。
- DirectPlay / DirectStream / Transcoding 能基于容器、编码、客户端参数做基本判断。

验收：

- 本地文件和 STRM 都能生成稳定 `MediaSource`。
- `RunTimeTicks`、`Container`、`Size`、`MediaStreams` 与数据库一致。
- 缺少 probe 信息时可以懒加载并回填。

### 阶段 6：移除或冻结旧 `movies` 主路径

目标：

- 内部主路径完全使用新 schema。
- `movies` 表仅作为迁移来源，或通过兼容 view/repository 继续服务旧 API。

验收：

- 新扫描不再依赖 `movies` 表表达媒体层级。
- 旧 `/api/v1/movies` 仍可返回兼容响应。
- 文档说明数据迁移和回滚方式。

## 风险与约束

- SQLite 可以继续使用，但需要把查询形态设计成索引友好，避免应用层全量过滤。
- Item ID 必须尽早确定稳定策略。建议数据库自增 ID 作为内部 ID，Emby 输出 ID 使用同一数值字符串；虚拟视图保留固定字符串 ID。
- 电视剧识别仍可沿用当前路径解析逻辑，但解析结果必须落库，避免每次请求重新计算。
- 需要保留现有 STRM 支持，且把远程 URL 与请求头信息纳入 `media_sources`。
- 迁移期间要避免一次性删除旧表，先双写或同步生成新表，再逐步切查询。

## 推荐第一批实现任务

1. 建立 `schema_version` 或迁移模块，避免继续在 `init_schema()` 中散落 `ALTER TABLE`。
2. 新增 `media_items`、`media_files`、`media_sources`、`user_item_data` 四张基础表。
3. 写 `backfill_media_schema_from_movies()`，从旧数据生成新结构。
4. 写 repository 查询：
   - `get_children(parent_id, query)`
   - `get_items_by_ids(ids)`
   - `get_latest(user_id, parent_id, limit)`
   - `get_playback_item(item_id)`
   - `get_user_item_data(user_id, item_ids)`
5. 先替换 Emby `/Items?ParentId=...` 的非递归查询。

## 完成定义

本重设计完成时，应能用测试证明：

- 电影库、电视剧库、混合库都能浏览。
- Emby 客户端常用首屏请求不会触发全库加载。
- 播放进度可以从上报接口写入，并在 Item 响应里读回。
- PlaybackInfo 可以返回可播放源和基础媒体流。
- 旧数据可以自动迁移，新老 API 在核心字段上保持兼容。
