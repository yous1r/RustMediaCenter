
// 常量定义，消除魔术数字
const DEBOUNCE_DELAY_MS = 300;
const BYTES_IN_GB = 1024 * 1024 * 1024;
const BYTES_IN_MB = 1024 * 1024;
const MSG_TIMEOUT_MS = 5000;
const AUTH_TOKEN_STORAGE_KEY = 'rmc_auth_token';
const API_BASE_STORAGE_KEY = 'rmc_api_base';
const API_BASE_QUERY_KEY = 'apiBase';
const DEFAULT_SERVER_PORT = '19000';
const API_DISCOVERY_TIMEOUT_MS = 1500;

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
let resolvedApiBasePromise = null;

function renderPlayerError(message) {
  const container = document.querySelector('#xgplayer-el');
  if (!container) return;
  container.innerHTML = `
    <div class="error-container" style="height:100%; display:flex; align-items:center; justify-content:center; padding:2rem; text-align:center;">
      <div>
        <p style="font-size:1.05rem; margin-bottom:0.75rem;">播放器初始化失败</p>
        <p style="color: var(--text-secondary);">${escapeHtml(message)}</p>
      </div>
    </div>
  `;
}

function resolvePlayerCtor(mode) {
  if (mode === 'transcode') {
    return window.Player || window.HlsPlayer || null;
  }
  return window.Player || null;
}

function normalizeApiBase(base) {
  if (!base) return null;
  const trimmed = String(base).trim();
  if (!trimmed) return null;
  return trimmed.replace(/\/+$/, '');
}

function isAbsoluteUrl(url) {
  return /^https?:\/\//i.test(url);
}

function buildUrlFromBase(path, base) {
  if (isAbsoluteUrl(path)) return path;
  const normalizedBase = normalizeApiBase(base) || window.location.origin;
  return new URL(path, `${normalizedBase}/`).toString();
}

function getQueryApiBaseOverride() {
  const params = new URLSearchParams(window.location.search);
  const fromQuery = normalizeApiBase(params.get(API_BASE_QUERY_KEY));
  if (fromQuery) {
    window.localStorage.setItem(API_BASE_STORAGE_KEY, fromQuery);
  }
  return fromQuery;
}

function getConfiguredApiBase() {
  const fromQuery = getQueryApiBaseOverride();
  if (fromQuery) return fromQuery;

  const fromGlobal = normalizeApiBase(window.__RMC_API_BASE__);
  if (fromGlobal) return fromGlobal;

  const fromMeta = normalizeApiBase(
    document.querySelector('meta[name="rmc-api-base"]')?.getAttribute('content')
  );
  if (fromMeta) return fromMeta;

  return normalizeApiBase(window.localStorage.getItem(API_BASE_STORAGE_KEY));
}

function getApiBaseCandidates() {
  const candidates = [];
  const configured = getConfiguredApiBase();
  if (configured) {
    candidates.push(configured);
  }

  candidates.push(window.location.origin);

  if (window.location.hostname && window.location.port !== DEFAULT_SERVER_PORT) {
    candidates.push(`${window.location.protocol}//${window.location.hostname}:${DEFAULT_SERVER_PORT}`);
  }

  return [...new Set(candidates.map(normalizeApiBase).filter(Boolean))];
}

async function probeApiBase(base) {
  const controller = new AbortController();
  const timer = window.setTimeout(() => controller.abort(), API_DISCOVERY_TIMEOUT_MS);

  try {
    const response = await fetch(buildUrlFromBase('/health', base), {
      method: 'GET',
      signal: controller.signal,
    });
    return response.ok;
  } catch (err) {
    console.warn('Failed to probe API base', base, err);
    return false;
  } finally {
    window.clearTimeout(timer);
  }
}

async function resolveApiBase() {
  if (!resolvedApiBasePromise) {
    resolvedApiBasePromise = (async () => {
      const candidates = getApiBaseCandidates();

      for (const candidate of candidates) {
        if (await probeApiBase(candidate)) {
          window.localStorage.setItem(API_BASE_STORAGE_KEY, candidate);
          return candidate;
        }
      }

      return candidates[0] || window.location.origin;
    })();
  }

  return resolvedApiBasePromise;
}

async function buildApiUrl(path) {
  const base = await resolveApiBase();
  return buildUrlFromBase(path, base);
}

async function apiFetch(path, options = {}) {
  const url = await buildApiUrl(path);
  return fetch(url, options);
}

function getAuthToken() {
  return window.localStorage.getItem(AUTH_TOKEN_STORAGE_KEY);
}

function setAuthToken(token) {
  window.localStorage.setItem(AUTH_TOKEN_STORAGE_KEY, token);
}

function clearAuthToken() {
  window.localStorage.removeItem(AUTH_TOKEN_STORAGE_KEY);
}

function buildAuthorizedHeaders(headers = {}) {
  const token = getAuthToken();
  if (!token) return headers;
  return {
    ...headers,
    Authorization: `Bearer ${token}`
  };
}

async function authorizedFetch(url, options = {}) {
  const headers = buildAuthorizedHeaders(options.headers || {});
  return apiFetch(url, {
    ...options,
    headers,
  });
}

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
    const response = await apiFetch(url);
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
    const response = await apiFetch(`/api/v1/movies/${id}`);
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
  const streamPath = mode === 'transcode'
    ? `/api/v1/movies/${id}/stream.mp4`
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

  const PlayerCtor = resolvePlayerCtor(mode);
  if (!PlayerCtor) {
    const globalName = mode === 'transcode' ? 'window.HlsPlayer' : 'window.Player';
    const message = `${globalName} is not loaded.`;
    console.error(message);
    renderPlayerError(message);
    return;
  }

  try {
    const streamUrl = await buildApiUrl(streamPath);
    const config = {
      id: 'xgplayer-el',
      url: streamUrl,
      autoplay: true,
      playsinline: true,
      fluid: true,
      width: '100%',
      height: '100%',
    };

    playerInstance = new PlayerCtor(config);

    if (playerInstance?.on) {
      playerInstance.on('error', (err) => {
        console.error('XGPlayer runtime error', err);
        renderPlayerError(err?.message || '播放器运行时发生错误，请检查转码日志和浏览器控制台。');
      });
    }
  } catch (err) {
    console.error('Failed to initialize XGPlayer', err);
    renderPlayerError(err?.message || '播放器初始化失败，请检查浏览器控制台。');
  }
}

// 设置页面渲染
async function renderSettingsPage(app) {
  if (!getAuthToken()) {
    renderSettingsLogin(app);
    return;
  }

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
            <input type="password" id="tmdb_api_key" class="login-input" placeholder="Leave blank to keep existing key">
            <div style="display:flex; align-items:center; gap:0.5rem; color: var(--text-secondary); font-size: 0.95rem;">
              <input type="checkbox" id="clear_tmdb_api_key">
              <label for="clear_tmdb_api_key" style="margin:0; cursor:pointer;">Clear stored TMDB API Key</label>
            </div>
            <div id="tmdb_api_key_status" style="color: var(--text-secondary); font-size: 0.95rem;">
              Checking saved key status...
            </div>
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
        <div style="display:flex; justify-content:flex-end; margin-top: 1rem;">
          <button type="button" id="btn-logout" class="btn-secondary" style="max-width: 220px;">退出控制面</button>
        </div>
        <div id="settings-message" style="margin-top: 1.5rem; text-align: center; font-weight: 500; display: none;"></div>
      </div>
    </main>
  `;

  // 获取服务端当前配置并填充表表单
  try {
    const response = await authorizedFetch('/api/v1/config');
    if (response.status === 401) {
      clearAuthToken();
      renderSettingsLogin(app, '登录已失效，请重新登录。');
      return;
    }
    if (!response.ok) throw new Error('Failed to load config');
    const config = await response.json();

    document.querySelector('#port').value = config.port || 8000;
    document.querySelector('#db_path').value = config.db_path || 'rmc.db';
    document.querySelector('#media_dirs').value = (config.media_dirs || []).join('\n');
    document.querySelector('#tmdb_proxy_url').value = config.tmdb_proxy_url || '';
    document.querySelector('#tmdb_api_base').value = config.tmdb_api_base || '';
    const keyStatusEl = document.querySelector('#tmdb_api_key_status');
    if (keyStatusEl) {
      keyStatusEl.textContent = config.tmdb_api_key_present
        ? 'A TMDB API key is already stored on the server.'
        : 'No TMDB API key is currently stored on the server.';
    }
  } catch (err) {
    console.error('Failed to load ServerConfig', err);
    showSettingsMessage('Failed to load server configuration.', '#ef4444');
  }

  // 绑定事件
  const btnSave = document.querySelector('#btn-save');
  const btnScan = document.querySelector('#btn-scan');
  const btnLogout = document.querySelector('#btn-logout');

  if (btnSave) {
    btnSave.addEventListener('click', saveConfig);
  }
  if (btnScan) {
    btnScan.addEventListener('click', triggerScan);
  }
  if (btnLogout) {
    btnLogout.addEventListener('click', () => {
      clearAuthToken();
      renderSettingsLogin(app, 'You have been signed out of the control panel.');
    });
  }
}

function renderSettingsLogin(app, message = '') {
  app.innerHTML = `
    ${renderNav()}
    <main class="page-fade-in" style="display:flex; justify-content:center; padding-top: 6rem; padding-bottom: 6rem;">
      <div class="login-card">
        <h2>Control Panel Sign-in</h2>
        <p class="subtitle" style="margin-bottom: 2rem;">Authenticate to access settings and scan controls.</p>
        <form id="settings-login-form" class="login-form" onsubmit="return false;">
          <input type="text" id="settings-username" class="login-input" placeholder="Username" autocomplete="username" required>
          <input type="password" id="settings-password" class="login-input" placeholder="Password" autocomplete="current-password" required>
          <button type="submit" class="btn-primary">Sign in</button>
        </form>
        <div id="settings-login-message" style="margin-top: 1rem; color: ${message ? '#ef4444' : 'var(--text-secondary)'}; min-height: 1.5rem;">
          ${escapeHtml(message)}
        </div>
      </div>
    </main>
  `;

  const form = document.querySelector('#settings-login-form');
  if (form) {
    form.addEventListener('submit', async () => {
      await loginForSettings(app);
    });
  }
}

async function loginForSettings(app) {
  const username = document.querySelector('#settings-username')?.value.trim() || '';
  const password = document.querySelector('#settings-password')?.value || '';
  const messageEl = document.querySelector('#settings-login-message');

  if (!username || !password) {
    if (messageEl) messageEl.textContent = 'Please provide both username and password.';
    return;
  }

  try {
    const response = await apiFetch('/api/v1/auth/login', {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json'
      },
      body: JSON.stringify({ username, password })
    });

    if (!response.ok) {
      throw new Error('Invalid credentials');
    }

    const data = await response.json();
    if (!data.token) {
      throw new Error('Missing token in login response');
    }

    setAuthToken(data.token);
    await renderSettingsPage(app);
  } catch (err) {
    console.error('Failed to sign in to control panel', err);
    if (messageEl) {
      messageEl.textContent = 'Sign-in failed. Check your credentials and try again.';
      messageEl.style.color = '#ef4444';
    }
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
  const clearTmdbApiKey = Boolean(document.querySelector('#clear_tmdb_api_key')?.checked);

  if (!portVal || !db_path || !media_dirs_text) {
    showSettingsMessage('Please fill in all required fields.', '#ef4444');
    return;
  }

  if (clearTmdbApiKey && tmdb_api_key_raw.trim()) {
    showSettingsMessage('Enter a new TMDB key or clear the existing one, not both.', '#ef4444');
    return;
  }

  const port = parseInt(portVal, 10);
  const media_dirs = media_dirs_text.split('\n').map(s => s.trim()).filter(s => s.length > 0);
  const tmdb_api_key = clearTmdbApiKey ? null : (tmdb_api_key_raw.trim() || null);
  const replace_tmdb_api_key = clearTmdbApiKey || Boolean(tmdb_api_key_raw.trim());
  const tmdb_proxy_url = document.querySelector('#tmdb_proxy_url').value.trim() || null;
  const tmdb_api_base = document.querySelector('#tmdb_api_base').value.trim() || null;

  const payload = {
    port,
    db_path,
    media_dirs,
    tmdb_api_key,
    replace_tmdb_api_key,
    tmdb_proxy_url,
    tmdb_api_base
  };

  try {
    const response = await authorizedFetch('/api/v1/config', {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json'
      },
      body: JSON.stringify(payload)
    });

    if (response.status === 401) {
      clearAuthToken();
      await renderSettingsPage(document.querySelector('#app'));
      return;
    }
    if (!response.ok) throw new Error('Save failed');
    const config = await response.json();
    const keyStatusEl = document.querySelector('#tmdb_api_key_status');
    if (keyStatusEl) {
      keyStatusEl.textContent = config.tmdb_api_key_present
        ? 'A TMDB API key is already stored on the server.'
        : 'No TMDB API key is currently stored on the server.';
    }
    document.querySelector('#tmdb_api_key').value = '';
    document.querySelector('#clear_tmdb_api_key').checked = false;
    showSettingsMessage('Configuration saved successfully!', '#10b981');
  } catch (err) {
    console.error('Failed to save config', err);
    showSettingsMessage('Failed to save configuration.', '#ef4444');
  }
}

// 触发后台媒体库扫描
async function triggerScan() {
  try {
    const response = await authorizedFetch('/api/v1/scan', {
      method: 'POST'
    });
    if (response.status === 401) {
      clearAuthToken();
      await renderSettingsPage(document.querySelector('#app'));
      return;
    }
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
