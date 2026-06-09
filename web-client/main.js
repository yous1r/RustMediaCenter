
// 常量定义，消除魔术数字
const DEBOUNCE_DELAY_MS = 300;
const BYTES_IN_GB = 1024 * 1024 * 1024;
const BYTES_IN_MB = 1024 * 1024;
const MSG_TIMEOUT_MS = 5000;

// 防御 DOM-based XSS 的 Html 转义辅助函数
function escapeHtml(str) {
  if (str === null || str === undefined) return '';
  return String(str)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#039;');
}

// 全局状态
let movies = [];
let currentQuery = '';
let playerInstance = null;

// 路由与渲染导航栏
function renderNav() {
  return `
    <nav>
      <a href="#/" class="nav-brand" style="text-decoration:none;">RustMediaCenter</a>
      <div style="display: flex; gap: 1.5rem; align-items: center;">
        <a href="#/" class="nav-link">Library</a>
        <a href="#/settings" class="nav-link">Settings</a>
      </div>
    </nav>
  `;
}

// 简单的单页路由 (SPA Router)
async function router() {
  // 安全销毁 XGPlayer 实例
  if (playerInstance) {
    try {
      playerInstance.pause();
      if (playerInstance.video) {
        playerInstance.video.src = '';
        playerInstance.video.load();
      }
      playerInstance.destroy();
    } catch (e) {
      console.error('Failed to destroy player instance', e);
    }
    playerInstance = null;
  }

  const hash = window.location.hash || '#/';
  const app = document.querySelector('#app');

  if (hash === '#/' || hash === '') {
    await renderHomePage(app);
  } else if (hash.startsWith('#/movie/')) {
    const parts = hash.split('/');
    const id = parseInt(parts[2], 10);
    await renderDetailPage(app, id);
  } else if (hash.startsWith('#/play/')) {
    const parts = hash.split('/');
    const id = parseInt(parts[2], 10);
    const mode = parts[3] || 'direct';
    await renderPlayerPage(app, id, mode);
  } else if (hash === '#/settings') {
    await renderSettingsPage(app);
  } else {
    window.location.hash = '#/';
  }
}

// 首页渲染
async function renderHomePage(app) {
  app.innerHTML = `
    ${renderNav()}
    <main class="page-fade-in">
      <header>
        <h1>Library</h1>
        <p class="subtitle">Your personal collection of premium movies and shows</p>
      </header>
      <div class="search-container">
        <input type="text" id="search-input" class="search-input" placeholder="Search movies..." value="${escapeHtml(currentQuery)}">
      </div>
      <div id="content">
        <div class="loading-container">
          <div class="spinner"></div>
          <p>Loading your cinematic experience...</p>
        </div>
      </div>
    </main>
  `;

  const searchInput = document.querySelector('#search-input');
  if (searchInput) {
    searchInput.addEventListener('input', handleSearchInput);
    searchInput.focus();
    searchInput.setSelectionRange(searchInput.value.length, searchInput.value.length);
  }

  await loadAndRenderMoviesGrid(currentQuery);
}

// 防抖搜索处理
let searchTimeout;
function handleSearchInput(event) {
  const q = event.target.value;
  currentQuery = q;
  clearTimeout(searchTimeout);
  searchTimeout = setTimeout(async () => {
    await loadAndRenderMoviesGrid(q);
  }, DEBOUNCE_DELAY_MS);
}

// 异步加载电影网格并渲染
async function loadAndRenderMoviesGrid(q) {
  const contentEl = document.querySelector('#content');
  if (!contentEl) return;

  try {
    const url = q ? `/api/v1/movies?q=${encodeURIComponent(q)}` : '/api/v1/movies';
    const response = await fetch(url);
    if (!response.ok) throw new Error('API error');
    movies = await response.json();

    if (movies.length === 0) {
      contentEl.innerHTML = `
        <div style="text-align: center; color: var(--text-secondary); margin-top: 3rem; font-size: 1.125rem;">
          No movies found in library.
        </div>
      `;
      return;
    }

    contentEl.innerHTML = `
      <div class="movies-grid">
        ${movies.map(movie => {
          const hasPoster = movie.poster_url;
          const posterHtml = hasPoster
            ? `<div class="movie-poster" style="background-image: url('${encodeURI(movie.poster_url)}')"></div>`
            : `<div class="movie-poster placeholder-poster">${escapeHtml((movie.title || '?').charAt(0).toUpperCase())}</div>`;

          return `
            <a href="#/movie/${movie.id}" class="movie-card" style="text-decoration: none;">
              ${posterHtml}
              <div class="movie-info">
                <h3 class="movie-title">${escapeHtml(movie.title)}</h3>
                <span class="movie-year">${escapeHtml(movie.year || 'Unknown')}</span>
              </div>
            </a>
          `;
        }).join('')}
      </div>
    `;
  } catch (err) {
    console.error('Failed to fetch movies', err);
    contentEl.innerHTML = `
      <div class="error-container">
        <p>Failed to load movies. Please check connection.</p>
      </div>
    `;
  }
}

// 电影详情页渲染
async function renderDetailPage(app, id) {
  app.innerHTML = `
    ${renderNav()}
    <main class="page-fade-in">
      <div id="detail-content">
        <div class="loading-container">
          <div class="spinner"></div>
          <p>Fetching movie details...</p>
        </div>
      </div>
    </main>
  `;

  const detailContentEl = document.querySelector('#detail-content');
  if (!detailContentEl) return;

  try {
    const response = await fetch(`/api/v1/movies/${id}`);
    if (!response.ok) throw new Error('Movie not found');
    const movie = await response.json();

    const hasPoster = movie.poster_url;
    const posterHtml = hasPoster
      ? `<div class="movie-detail-poster" style="background-image: url('${encodeURI(movie.poster_url)}')"></div>`
      : `<div class="movie-detail-poster placeholder-poster">${escapeHtml((movie.title || '?').charAt(0).toUpperCase())}</div>`;

    const sizeStr = formatSize(movie.file_size);

    detailContentEl.innerHTML = `
      <div class="movie-detail-hero">
        ${posterHtml}
        <div class="movie-detail-info">
          <a href="#/" class="back-link">← Back to Library</a>
          <h1 class="movie-detail-title">${escapeHtml(movie.title)}</h1>
          <div class="movie-detail-meta">
            <span class="movie-detail-year">${escapeHtml(movie.year || 'Unknown')}</span>
            <span class="movie-detail-badge">${escapeHtml(sizeStr)}</span>
            <span class="movie-detail-badge">4K HDR</span>
          </div>
          <p class="movie-detail-overview">${escapeHtml(movie.overview || 'A breathtaking cinematic journey awaits. No description available yet.')}</p>
          <div style="display: flex; gap: 1.5rem; flex-wrap: wrap;">
            <a href="#/play/${movie.id}/direct" class="btn-play">
              <svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
                <path d="M5 3L19 12L5 21V3Z" fill="currentColor" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
              </svg>
              直刷播放 (客户端解码)
            </a>
            <a href="#/play/${movie.id}/transcode" class="btn-play" style="background: linear-gradient(135deg, #10b981 0%, #059669 100%); box-shadow: 0 4px 15px rgba(16, 185, 129, 0.4);">
              <svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
                <path d="M5 3L19 12L5 21V3Z" fill="currentColor" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
              </svg>
              转码播放 (核显硬解)
            </a>
          </div>
        </div>
      </div>
    `;
  } catch (err) {
    console.error('Failed to load movie detail', err);
    detailContentEl.innerHTML = `
      <div class="error-container">
        <p>Failed to load movie details. The video might not exist or connection is lost.</p>
        <a href="#/" class="back-link" style="margin-top: 1rem;">← Return to Library</a>
      </div>
    `;
  }
}

// 格式化文件大小为可读形式
function formatSize(bytes) {
  if (bytes === null || bytes === undefined) return 'Unknown Size';
  const gb = bytes / BYTES_IN_GB;
  if (gb >= 1) return `${gb.toFixed(2)} GB`;
  const mb = bytes / BYTES_IN_MB;
  return `${mb.toFixed(2)} MB`;
}

// 播放页面渲染
async function renderPlayerPage(app, id, mode) {
  const streamUrl = mode === 'transcode'
    ? `/api/v1/movies/${id}/hls/master.m3u8`
    : `/api/v1/movies/${id}/direct`;

  app.innerHTML = `
    <div class="player-container page-fade-in">
      <a href="#/movie/${id}" class="player-back">← Back</a>
      <div id="xgplayer-el" style="width: 100%; height: 100%;"></div>
      
      <div class="player-mode-switch">
        <a href="#/play/${id}/direct" class="player-btn ${mode === 'direct' ? 'active' : ''}">直刷模式 (客户端解码)</a>
        <a href="#/play/${id}/transcode" class="player-btn ${mode === 'transcode' ? 'active' : ''}">转码模式 (核显硬解)</a>
      </div>
    </div>
  `;

  if (!window.Player) {
    console.error('XGPlayer (window.Player) is not loaded.');
    return;
  }

  const plugins = [];
  if (mode === 'transcode' && window.XgplayerHls) {
    plugins.push(window.XgplayerHls);
  }

  try {
    playerInstance = new window.Player({
      id: 'xgplayer-el',
      url: streamUrl,
      plugins: plugins,
      autoplay: true,
      width: '100%',
      height: '100%',
    });
  } catch (err) {
    console.error('Failed to initialize XGPlayer', err);
  }
}

// 设置页面渲染
async function renderSettingsPage(app) {
  app.innerHTML = `
    ${renderNav()}
    <main class="page-fade-in">
      <div class="settings-container">
        <h2 style="margin-bottom: 2rem; font-size: 2rem;">System Settings</h2>
        <form id="settings-form" onsubmit="return false;">
          <div class="form-group">
            <label for="port">Server Port</label>
            <input type="number" id="port" class="login-input" placeholder="e.g. 8000" required>
          </div>
          <div class="form-group">
            <label for="db_path">Database Path</label>
            <input type="text" id="db_path" class="login-input" placeholder="e.g. rmc.db" required>
          </div>
          <div class="form-group">
            <label for="media_dirs">Media Directories (One per line)</label>
            <textarea id="media_dirs" class="login-input" style="min-height: 120px; font-family: monospace; resize: vertical;" placeholder="e.g. /media" required></textarea>
          </div>
          <div class="form-group">
            <label for="tmdb_api_key">TMDB API Key (Optional)</label>
            <input type="text" id="tmdb_api_key" class="login-input" placeholder="Enter your TMDB API Key">
          </div>
          <div class="form-group">
            <label for="tmdb_proxy_url">TMDB Proxy URL (Optional)</label>
            <input type="text" id="tmdb_proxy_url" class="login-input" placeholder="e.g. http://127.0.0.1:7890">
          </div>
          <div class="form-group">
            <label for="tmdb_api_base">TMDB API Base URL (Optional)</label>
            <input type="text" id="tmdb_api_base" class="login-input" placeholder="e.g. https://api.tmdb.org">
          </div>
          
          <div class="form-actions">
            <button type="button" id="btn-scan" class="btn-secondary">立即扫描入库 & 刮削</button>
            <button type="submit" id="btn-save" class="btn-primary btn-save">保存配置</button>
          </div>
        </form>
        <div id="settings-message" style="margin-top: 1.5rem; text-align: center; font-weight: 500; display: none;"></div>
      </div>
    </main>
  `;

  // 获取服务端当前配置并填充表表单
  try {
    const response = await fetch('/api/v1/config');
    if (!response.ok) throw new Error('Failed to load config');
    const config = await response.json();

    document.querySelector('#port').value = config.port || 8000;
    document.querySelector('#db_path').value = config.db_path || 'rmc.db';
    document.querySelector('#media_dirs').value = (config.media_dirs || []).join('\n');
    document.querySelector('#tmdb_api_key').value = config.tmdb_api_key || '';
    document.querySelector('#tmdb_proxy_url').value = config.tmdb_proxy_url || '';
    document.querySelector('#tmdb_api_base').value = config.tmdb_api_base || '';
  } catch (err) {
    console.error('Failed to load ServerConfig', err);
    showSettingsMessage('Failed to load server configuration.', '#ef4444');
  }

  // 绑定事件
  const btnSave = document.querySelector('#btn-save');
  const btnScan = document.querySelector('#btn-scan');

  if (btnSave) {
    btnSave.addEventListener('click', saveConfig);
  }
  if (btnScan) {
    btnScan.addEventListener('click', triggerScan);
  }
}

// 状态信息提示辅助函数
function showSettingsMessage(text, color) {
  const msgEl = document.querySelector('#settings-message');
  if (msgEl) {
    msgEl.textContent = text;
    msgEl.style.color = color;
    msgEl.style.display = 'block';
    setTimeout(() => {
      msgEl.style.display = 'none';
    }, MSG_TIMEOUT_MS);
  }
}

// 保存配置到后端
async function saveConfig() {
  const portVal = document.querySelector('#port').value;
  const db_path = document.querySelector('#db_path').value;
  const media_dirs_text = document.querySelector('#media_dirs').value;
  const tmdb_api_key_raw = document.querySelector('#tmdb_api_key').value;

  if (!portVal || !db_path || !media_dirs_text) {
    showSettingsMessage('Please fill in all required fields.', '#ef4444');
    return;
  }

  const port = parseInt(portVal, 10);
  const media_dirs = media_dirs_text.split('\n').map(s => s.trim()).filter(s => s.length > 0);
  const tmdb_api_key = tmdb_api_key_raw.trim() || null;
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

  try {
    const response = await fetch('/api/v1/config', {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json'
      },
      body: JSON.stringify(payload)
    });

    if (!response.ok) throw new Error('Save failed');
    showSettingsMessage('Configuration saved successfully!', '#10b981');
  } catch (err) {
    console.error('Failed to save config', err);
    showSettingsMessage('Failed to save configuration.', '#ef4444');
  }
}

// 触发后台媒体库扫描
async function triggerScan() {
  try {
    const response = await fetch('/api/v1/scan', {
      method: 'POST'
    });
    if (!response.ok) throw new Error('Scan initiation failed');
    showSettingsMessage('Scan and scraping started in the background!', '#10b981');
  } catch (err) {
    console.error('Failed to start scan', err);
    showSettingsMessage('Failed to start scan.', '#ef4444');
  }
}

// 路由事件注册
window.addEventListener('hashchange', router);
router();
