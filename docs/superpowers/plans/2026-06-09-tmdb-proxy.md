# TMDB Scraper Proxy and Base URL Configuration Implementation Plan

> **面向 AI 代理的工作者：** 必需子技能：使用 superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框（`- [ ]`）语法来跟踪进度。

**目标：** 在 RustMediaCenter 系统中支持通过标准 HTTP Proxy 和自定义 API Base URL 访问 TMDB Scraper，并打通前端 Settings UI 自助配置。

**架构：** 在 `ServerConfig` 中引入 `tmdb_proxy_url` 和 `tmdb_api_base` 字段，并在 `TmdbScraper` 实例化时配置 reqwest 代理和自定义 Base URL。更新 Axum 端点和 SPA Web 客户端相关设置表单。

**技术栈：** Rust, Axum, sqlx, reqwest, JavaScript (Vanilla JS SPA)

---

## 1. 计划涉及的文件结构

* `crates/rmc-server/src/config.rs`: 扩展 `ServerConfig` 属性及单元测试。
* `crates/rmc-server/src/scraper.rs`: 重构 `TmdbScraper` 的构造方法和请求发送。
* `crates/rmc-server/src/api.rs`: 更新 `/api/v1/config` JSON 路由有效载荷和扫描进程的 scraper 构造。
* `web-client/main.js`: 扩展前端 Settings 配置项及表单存取逻辑。

---

## 2. 任务分解

### 任务 1：ServerConfig 配置扩展
**文件：**
- 修改：`crates/rmc-server/src/config.rs`

- [ ] **步骤 1：编写失败的测试**
  在 `config.rs` 中修改 `test_config_defaults` 和 `test_config_load_and_save` 单元测试，添加对新字段 `tmdb_proxy_url` 和 `tmdb_api_base` 的断言测试。

  ```rust
  // 在 crates/rmc-server/src/config.rs::tests 模块中：
  // 1. test_config_defaults()
  assert_eq!(config.tmdb_proxy_url, None);
  assert_eq!(config.tmdb_api_base, None);

  // 2. test_config_load_and_save()
  config.tmdb_proxy_url = Some("http://127.0.0.1:7890".to_string());
  config.tmdb_api_base = Some("https://api.tmdb.org".to_string());
  // ...
  assert_eq!(loaded.tmdb_proxy_url, Some("http://127.0.0.1:7890".to_string()));
  assert_eq!(loaded.tmdb_api_base, Some("https://api.tmdb.org".to_string()));
  ```

- [ ] **步骤 2：运行测试验证失败**
  运行：`source ~/.cargo/env && cargo test -p rmc-server config::tests`
  预期：编译失败，因为 `ServerConfig` 没有 `tmdb_proxy_url` 和 `tmdb_api_base` 属性。

- [ ] **步骤 3：编写最少实现代码**
  修改 `ServerConfig` 结构体和其 `Default` 实现：

  ```rust
  #[derive(Debug, Deserialize, Serialize, Clone)]
  pub struct ServerConfig {
      pub port: u16,
      pub media_dirs: Vec<String>,
      pub db_path: String,
      pub tmdb_api_key: Option<String>,
      pub tmdb_proxy_url: Option<String>,
      pub tmdb_api_base: Option<String>,
  }

  impl Default for ServerConfig {
      fn default() -> Self {
          Self {
              port: 8000,
              media_dirs: vec!["/media".to_string()],
              db_path: "rmc.db".to_string(),
              tmdb_api_key: None,
              tmdb_proxy_url: None,
              tmdb_api_base: None,
          }
      }
  }
  ```

- [ ] **步骤 4：运行测试验证通过**
  运行：`source ~/.cargo/env && cargo test -p rmc-server config::tests`
  预期：测试编译并全部通过。

- [ ] **步骤 5：Commit**
  ```bash
  git add crates/rmc-server/src/config.rs
  git commit -m "feat(config): add tmdb_proxy_url and tmdb_api_base to ServerConfig"
  ```

---

### 任务 2：TMDB 刮削器代理与自定义 URL 实现
**文件：**
- 修改：`crates/rmc-server/src/scraper.rs`

- [ ] **步骤 1：编写失败的测试**
  在 `scraper.rs` 中编写测试 `test_fetch_metadata_custom_base`。

  ```rust
  #[tokio::test]
  async fn test_fetch_metadata_custom_base() {
      // 传入一个不存在的无效 base URL，当 scraper 访问时，应该返回网络错误或包含该 url 域名的错误，从而证明确实使用了自定义 base
      let scraper = TmdbScraper::new(
          "dummy_key".to_string(),
          None,
          Some("https://invalid.domain.tmdb-proxy.com".to_string())
      );
      let res = scraper.fetch_movie_metadata("Inception").await;
      assert!(res.is_err());
      let err_msg = res.unwrap_err().to_string();
      assert!(err_msg.contains("invalid.domain.tmdb-proxy.com") || err_msg.contains("dns") || err_msg.contains("builder"));
  }
  ```

  并在 `tests` 模块已有的测试中为 `TmdbScraper::new` 调用传入额外的两个参数 `None, None`。

- [ ] **步骤 2：运行测试验证失败**
  运行：`source ~/.cargo/env && cargo test -p rmc-server scraper::tests`
  预期：编译失败，因为 `TmdbScraper::new` 参数个数不匹配。

- [ ] **步骤 3：编写最少实现代码**
  修改 `TmdbScraper` 结构体、`new` 方法和 `fetch_movie_metadata` 方法：

  ```rust
  pub struct TmdbScraper {
      api_key: String,
      client: reqwest::Client,
      api_base: String,
  }

  impl TmdbScraper {
      pub fn new(api_key: String, proxy_url: Option<String>, api_base: Option<String>) -> Self {
          let mut builder = reqwest::Client::builder()
              .timeout(std::time::Duration::from_secs(10));
          
          if let Some(proxy_str) = proxy_url.filter(|s| !s.trim().is_empty()) {
              if let Ok(proxy) = reqwest::Proxy::all(&proxy_str) {
                  builder = builder.proxy(proxy);
              }
          }

          let client = builder.build().unwrap_or_else(|_| reqwest::Client::new());
          
          let final_api_base = api_base
              .filter(|s| !s.trim().is_empty())
              .unwrap_or_else(|| "https://api.themoviedb.org".to_string());

          Self {
              api_key,
              client,
              api_base: final_api_base,
          }
      }

      pub async fn fetch_movie_metadata(&self, title: &str) -> Result<Movie, Box<dyn std::error::Error + Send + Sync>> {
          let api_url = format!("{}/3/search/movie", self.api_base.trim_end_matches('/'));
          let response = self.client.get(&api_url)
              .query(&[
                  ("api_key", self.api_key.as_str()),
                  ("query", title),
                  ("language", "zh-CN"),
              ])
              .send()
              .await?;
          
          // ... 剩下的解析代码保持不变
  ```

- [ ] **步骤 4：运行测试验证通过**
  运行：`source ~/.cargo/env && cargo test -p rmc-server scraper::tests`
  预期：所有 Scraper 的测试，包括我们新写的自定义 Base URL 测试全部通过。

- [ ] **步骤 5：Commit**
  ```bash
  git add crates/rmc-server/src/scraper.rs
  git commit -m "feat(scraper): support HTTP/SOCKS proxy and custom base URL in TmdbScraper"
  ```

---

### 任务 3：API 路由适配及配置更新
**文件：**
- 修改：`crates/rmc-server/src/api.rs`

- [ ] **步骤 1：编写失败的测试**
  在 `api.rs` 的 `tests` 模块修改 `test_config_load_and_save` 类型的 API 测试。
  如果不存在该测试，则在 `test_get_movies_api` 所在的测试中追加对 `/api/v1/config` GET 和 POST 接口新增字段的序列化/反序列化校验。

  ```rust
  // 例如，使用一个现成的 config 更新 API 测试进行断言扩展，确认返回和接受的 json 带有 tmdb_proxy_url 和 tmdb_api_base。
  ```

- [ ] **步骤 2：运行测试验证失败**
  运行：`source ~/.cargo/env && cargo test -p rmc-server api::tests`
  预期：编译失败，因为在 api.rs 中调用 `TmdbScraper::new` 时参数数量与新签名不符。

- [ ] **步骤 3：编写最少实现代码**
  在 `api.rs` 中：
  1. 修复 API 端点中的 `scraper` 创建：
     ```rust
     let scraper = crate::scraper::TmdbScraper::new(
         api_key.clone(),
         config.tmdb_proxy_url.clone(),
         config.tmdb_api_base.clone(),
     );
     ```
  2. 修改 `update_config` 的 payload 结构体定义，使其接收 `tmdb_proxy_url` 和 `tmdb_api_base` 字段并持久化写入：
     ```rust
     #[derive(Deserialize, Serialize, Debug, Clone)]
     pub struct ConfigPayload {
         pub port: u16,
         pub db_path: String,
         pub media_dirs: Vec<String>,
         pub tmdb_api_key: Option<String>,
         pub tmdb_proxy_url: Option<String>,
         pub tmdb_api_base: Option<String>,
     }
     ```
     在 `update_config` 实现中：
     ```rust
     config.tmdb_api_key = payload.tmdb_api_key;
     config.tmdb_proxy_url = payload.tmdb_proxy_url;
     config.tmdb_api_base = payload.tmdb_api_base;
     ```

- [ ] **步骤 4：运行测试验证通过**
  运行：`source ~/.cargo/env && cargo test -p rmc-server api::tests`
  预期：测试全部编译并顺利通过。

- [ ] **步骤 5：Commit**
  ```bash
  git add crates/rmc-server/src/api.rs
  git commit -m "feat(api): expose tmdb_proxy_url and tmdb_api_base in configuration endpoints"
  ```

---

### 任务 4：网页端设置 UI 对接
**文件：**
- 修改：`web-client/main.js`

- [ ] **步骤 1：确认现有的表单渲染和提交路径**
  阅读 `web-client/main.js` 中的 `renderSettings` 和 `saveConfig` 方法。

- [ ] **步骤 2：添加 HTML 表单输入域并对接 JS 加载/提交**
  1. 在 `renderSettings` 的表单中，在 `tmdb_api_key` 输入域下方添加：
     ```html
     <div class="form-group">
       <label for="tmdb_proxy_url">TMDB Proxy URL (Optional)</label>
       <input type="text" id="tmdb_proxy_url" class="login-input" placeholder="e.g. http://127.0.0.1:7890">
     </div>
     <div class="form-group">
       <label for="tmdb_api_base">TMDB API Base URL (Optional)</label>
       <input type="text" id="tmdb_api_base" class="login-input" placeholder="e.g. https://api.tmdb.org">
     </div>
     ```
  2. 在加载配置分支中，填充 input：
     ```javascript
     document.querySelector('#tmdb_proxy_url').value = config.tmdb_proxy_url || '';
     document.querySelector('#tmdb_api_base').value = config.tmdb_api_base || '';
     ```
  3. 在 `saveConfig` 中，读取值并附加到 `payload`：
     ```javascript
     const tmdb_proxy_url = document.querySelector('#tmdb_proxy_url').value.trim() || null;
     const tmdb_api_base = document.querySelector('#tmdb_api_base').value.trim() || null;

     const payload = {
       port,
       db_path,
       media_dirs,
       tmdb_api_key,
       tmdb_proxy_url,
       tmdb_api_base
     };
     ```

- [ ] **步骤 3：验证手动启动与设置保存**
  启动服务：`source ~/.cargo/env && cargo run -p rmc-server`
  访问 UI：进入设置页面，填写代理 URL / Base URL 并点击保存。确认 `config.toml` 中成功更新。

- [ ] **步骤 4：Commit**
  ```bash
  git add web-client/main.js
  git commit -m "feat(web): add TMDB proxy and base URL fields to settings panel"
  ```
