import { createSeekState } from './player_seek_state.js';

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
const PLAYER_UI_IDLE_MS = 3000;
const TRANSCODE_QUALITY_OPTIONS = [
  { value: 'source', label: '原始' },
  { value: '1080p', label: '1080P' },
  { value: '720p', label: '720P' },
  { value: '480p', label: '480P' },
  { value: '360p', label: '360P' },
];

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
const LIBRARY_PAGE_SIZE = 24;
let currentQuery = '';
let playerInstance = null;
let resolvedApiBasePromise = null;
let lastBrowseHash = '#/';
let lastDetailHash = '#/';
let homeLibraryState = createHomeLibraryState();

function createHomeLibraryState() {
  return {
    items: [],
    totalRecordCount: 0,
    loading: false,
  };
}

function rememberBrowseHash() {
  lastBrowseHash = window.location.hash || '#/';
}

function rememberDetailHash() {
  lastDetailHash = window.location.hash || '#/';
}

function destroyPlayerInstance() {
  if (!playerInstance) return;

  try {
    playerInstance.pause?.();
    playerInstance.destroy?.();
  } catch (err) {
    console.error('Failed to destroy player instance', err);
  }

  playerInstance = null;
}

function renderPlayerError(message) {
  const container = document.querySelector('#player-shell');
  if (!container) return;
  container.innerHTML = `
    <div class="error-container player-error-state">
      <div>
        <p style="font-size:1.05rem; margin-bottom:0.75rem;">播放器不可用</p>
        <p style="color: var(--text-secondary);">${escapeHtml(message)}</p>
      </div>
    </div>
  `;
}

function formatPlaybackTime(seconds) {
  if (!Number.isFinite(seconds) || seconds < 0) return '00:00';

  const rounded = Math.floor(seconds);
  const hours = Math.floor(rounded / 3600);
  const minutes = Math.floor((rounded % 3600) / 60);
  const secs = rounded % 60;

  if (hours > 0) {
    return `${String(hours).padStart(2, '0')}:${String(minutes).padStart(2, '0')}:${String(secs).padStart(2, '0')}`;
  }

  return `${String(minutes).padStart(2, '0')}:${String(secs).padStart(2, '0')}`;
}

function getTranscodeQualityLabel(quality) {
  return TRANSCODE_QUALITY_OPTIONS.find((item) => item.value === quality)?.label || '原始';
}

function buildTranscodeStreamPath(movieId, startTimeSeconds = 0, quality = 'source') {
  const params = new URLSearchParams();
  if (Number.isFinite(startTimeSeconds) && startTimeSeconds > 0) {
    params.set('start_time', startTimeSeconds.toFixed(3));
  }
  if (quality && quality !== 'source') {
    params.set('quality', quality);
  }

  const isIOSDevice = [
    'iPad Simulator',
    'iPhone Simulator',
    'iPod Simulator',
    'iPad',
    'iPhone',
    'iPod'
  ].includes(navigator.platform)
  || (navigator.userAgent.includes("Mac") && "ontouchend" in document)
  || /iPad|iPhone|iPod/.test(navigator.userAgent)
  || (/Safari/.test(navigator.userAgent) && !/Chrome/.test(navigator.userAgent));

  if (isIOSDevice) {
    return `/api/v1/movies/${movieId}/hls/master.m3u8`;
  }

  const query = params.toString();
  return query
    ? `/api/v1/movies/${movieId}/stream.mp4?${query}`
    : `/api/v1/movies/${movieId}/stream.mp4`;
}

function setRangeProgress(input) {
  if (!input) return;

  const min = Number(input.min || 0);
  const max = Number(input.max || 100);
  const value = Number(input.value || 0);
  const percent = max <= min ? 0 : ((value - min) / (max - min)) * 100;
  input.style.setProperty('--range-progress', `${Math.max(0, Math.min(100, percent))}%`);
}

async function toggleFullscreen(element) {
  if (!element) return;

  if (document.fullscreenElement === element) {
    await document.exitFullscreen();
    return;
  }

  await element.requestFullscreen?.();
}

function isMobileViewport() {
  return window.matchMedia('(max-width: 900px)').matches;
}

function getViewportOrientation() {
  return window.matchMedia('(orientation: portrait)').matches ? 'portrait' : 'landscape';
}

function updatePlayerViewportClasses(container, video) {
  if (!container) return;

  const mobile = isMobileViewport();
  const orientation = getViewportOrientation();
  const hasVideoMetadata = Number.isFinite(video?.videoWidth) && Number.isFinite(video?.videoHeight) && video.videoWidth > 0 && video.videoHeight > 0;
  const portraitVideo = hasVideoMetadata ? video.videoHeight > video.videoWidth : false;

  container.classList.toggle('player-mobile', mobile);
  container.classList.toggle('player-mobile-portrait', mobile && orientation === 'portrait');
  container.classList.toggle('player-mobile-landscape', mobile && orientation === 'landscape');
  container.classList.toggle('player-video-portrait', portraitVideo);
  container.classList.toggle('player-video-landscape', hasVideoMetadata && !portraitVideo);
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

function getLibraryKindLabel(kind) {
  switch (kind) {
    case 'movie':
      return 'Movie';
    case 'series':
      return 'Series';
    case 'season':
      return 'Season';
    case 'episode':
      return 'Episode';
    default:
      return 'Library';
  }
}

function formatRuntime(runtimeSeconds, runtimeMinutes) {
  const secondsValue = Number(runtimeSeconds);
  if (Number.isFinite(secondsValue) && secondsValue > 0) {
    const hours = Math.floor(secondsValue / 3600);
    const minutes = Math.floor((secondsValue % 3600) / 60);
    const seconds = secondsValue % 60;
    const parts = [];

    if (hours > 0) parts.push(`${hours}h`);
    if (minutes > 0 || hours > 0) parts.push(`${minutes}m`);
    parts.push(`${seconds}s`);
    return parts.join(' ');
  }

  const minutesValue = Number(runtimeMinutes);
  if (!Number.isFinite(minutesValue) || minutesValue <= 0) return '';

  const hours = Math.floor(minutesValue / 60);
  const minutes = minutesValue % 60;

  if (hours > 0 && minutes > 0) {
    return `${hours}h ${minutes}m`;
  }
  if (hours > 0) {
    return `${hours}h`;
  }
  return `${minutes}m`;
}

function formatLibraryItemMeta(item) {
  const parts = [];
  const runtime = formatRuntime(item.runtime_seconds, item.runtime_minutes);

  if (item.kind === 'movie') {
    if (item.year) parts.push(String(item.year));
    if (runtime) parts.push(runtime);
    return parts.join(' • ') || 'Feature film';
  }

  if (item.kind === 'series') {
    if (item.child_count > 0) {
      parts.push(`${item.child_count} ${item.child_count === 1 ? 'season' : 'seasons'}`);
    }
    if (item.year) parts.push(String(item.year));
    return parts.join(' • ') || 'Series collection';
  }

  if (item.kind === 'season') {
    if (Number.isFinite(Number(item.season_number))) {
      parts.push(`Season ${item.season_number}`);
    }
    if (item.child_count > 0) {
      parts.push(`${item.child_count} ${item.child_count === 1 ? 'episode' : 'episodes'}`);
    }
    return parts.join(' • ') || 'Season';
  }

  if (item.kind === 'episode') {
    if (Number.isFinite(Number(item.season_number)) && Number.isFinite(Number(item.episode_number))) {
      parts.push(`S${String(item.season_number).padStart(2, '0')}E${String(item.episode_number).padStart(2, '0')}`);
    } else if (Number.isFinite(Number(item.episode_number))) {
      parts.push(`Episode ${item.episode_number}`);
    }
    if (runtime) parts.push(runtime);
    return parts.join(' • ') || 'Episode';
  }

  return getLibraryKindLabel(item.kind);
}

function buildLibraryItemsPath({ parentId = null, searchTerm = '', startIndex = 0, limit } = {}) {
  const params = new URLSearchParams();

  if (parentId) {
    params.set('parent_id', parentId);
  }
  if (searchTerm && searchTerm.trim()) {
    params.set('search_term', searchTerm.trim());
  }
  if (startIndex > 0) {
    params.set('start_index', String(startIndex));
  }
  if (Number.isFinite(limit) && limit > 0) {
    params.set('limit', String(limit));
  }

  const query = params.toString();
  return query ? `/api/v1/library/items?${query}` : '/api/v1/library/items';
}

async function fetchLibraryItems(options = {}) {
  const response = await apiFetch(buildLibraryItemsPath(options));
  if (!response.ok) {
    throw new Error(`Failed to load library items: ${response.status}`);
  }
  return response.json();
}

async function fetchLibraryItem(id) {
  const response = await apiFetch(`/api/v1/library/items/${encodeURIComponent(id)}`);
  if (!response.ok) {
    throw new Error(`Failed to load library item: ${response.status}`);
  }
  return response.json();
}

function renderPoster(posterUrl, title, className = 'movie-poster') {
  if (posterUrl) {
    return `<div class="${className}" style="background-image: url('${encodeURI(posterUrl)}')"></div>`;
  }

  return `<div class="${className} placeholder-poster">${escapeHtml((title || '?').charAt(0).toUpperCase())}</div>`;
}

function buildLibraryItemHref(item) {
  const playId = Number(item.play_id);

  if (item.kind === 'movie' && Number.isFinite(playId)) {
    return `#/movie/${playId}`;
  }
  if (item.kind === 'series') {
    return `#/series/${encodeURIComponent(item.id)}`;
  }
  if (item.kind === 'season') {
    return `#/season/${encodeURIComponent(item.id)}`;
  }
  if (item.kind === 'episode' && Number.isFinite(playId)) {
    return `#/play/${playId}/direct`;
  }

  return '#/';
}

function renderLibraryCard(item) {
  const actionLabel = item.kind === 'episode' ? 'Play now' : 'Open';
  const description = item.overview
    ? `<p class="library-card-description">${escapeHtml(item.overview)}</p>`
    : '';

  return `
    <a href="${buildLibraryItemHref(item)}" class="movie-card library-card" style="text-decoration: none;">
      ${renderPoster(item.poster_url, item.title)}
      <div class="movie-info">
        <span class="library-card-badge">${escapeHtml(getLibraryKindLabel(item.kind))}</span>
        <h3 class="movie-title">${escapeHtml(item.title)}</h3>
        <span class="library-card-meta">${escapeHtml(formatLibraryItemMeta(item))}</span>
        ${description}
        <span class="library-card-action">${escapeHtml(actionLabel)}</span>
      </div>
    </a>
  `;
}

function renderLibraryDetailHero({ item, backHref, backLabel, overviewFallback, metaBadges = [] }) {
  return `
    <div class="movie-detail-hero library-detail-hero">
      ${renderPoster(item.poster_url, item.title, 'movie-detail-poster')}
      <div class="movie-detail-info">
        <a href="${backHref}" class="back-link">${escapeHtml(backLabel)}</a>
        <h1 class="movie-detail-title">${escapeHtml(item.title)}</h1>
        <div class="movie-detail-meta">
          ${metaBadges.filter(Boolean).map(value => `<span class="movie-detail-badge">${escapeHtml(value)}</span>`).join('')}
        </div>
        <p class="movie-detail-overview">${escapeHtml(item.overview || overviewFallback)}</p>
      </div>
    </div>
  `;
}

function renderLibrarySection({ title, countLabel, contentClass = 'movies-grid', contentHtml, emptyMessage }) {
  return `
    <section class="library-section">
      <div class="library-section-header">
        <h2>${escapeHtml(title)}</h2>
        <span class="library-section-count">${escapeHtml(countLabel)}</span>
      </div>
      ${contentHtml ? `
        <div class="${contentClass}">
          ${contentHtml}
        </div>
      ` : `
        <div class="library-empty-state">
          <p>${escapeHtml(emptyMessage)}</p>
        </div>
      `}
    </section>
  `;
}

function renderEpisodeList(episodes) {
  return episodes.map((episode) => {
    const playId = Number(episode.play_id);
    const href = Number.isFinite(playId) ? `#/play/${playId}/direct` : '#/';
    const indexLabel = Number.isFinite(Number(episode.episode_number))
      ? `Episode ${episode.episode_number}`
      : 'Episode';

    let html = '';
    if (href === '#') {
      html += `
      <div class="library-episode-row" style="opacity: 0.6; cursor: not-allowed;" onclick="event.preventDefault(); alert('This episode is not available for playback.');">
        <div class="library-episode-index">${escapeHtml(indexLabel)}</div>
        <div class="library-episode-main">
          <h3>${escapeHtml(episode.title)}</h3>`;
    } else {
      html += `
      <a href="${href}" class="library-episode-row" style="text-decoration: none;">
        <div class="library-episode-index">${escapeHtml(indexLabel)}</div>
        <div class="library-episode-main">
          <h3>${escapeHtml(episode.title)}</h3>`;
    }
    html += `
          <p>${escapeHtml(episode.overview || 'Open this episode in the player.')}</p>
        </div>
        <div class="library-episode-meta">${escapeHtml(formatLibraryItemMeta(episode))}</div>
      ${href === '#' ? '</div>' : '</a>'}
    `;
    return html.trim();
  }).join('');
}

function renderHomeLibraryContent() {
  const contentEl = document.querySelector('#content');
  if (!contentEl) return;

  const loadedCount = homeLibraryState.items.length;
  const totalCount = homeLibraryState.totalRecordCount;
  const hasMore = loadedCount < totalCount;

  if (loadedCount === 0) {
    contentEl.innerHTML = `
      <div class="library-empty-state">
        <p>${escapeHtml(currentQuery ? 'No movies or series matched your search.' : 'No media found in your library yet.')}</p>
      </div>
    `;
    return;
  }

  const secondaryMeta = currentQuery
    ? `Search: ${currentQuery}`
    : `Loaded ${loadedCount}${totalCount ? ` / ${totalCount}` : ''}`;

  contentEl.innerHTML = `
    <div class="library-results-meta">
      <span>${loadedCount}${totalCount ? ` of ${totalCount}` : ''} items</span>
      <span>${escapeHtml(secondaryMeta)}</span>
    </div>
    <div class="movies-grid">
      ${homeLibraryState.items.map(renderLibraryCard).join('')}
    </div>
    ${hasMore ? `
      <div class="load-more-wrap">
        <button type="button" id="load-more-library" class="load-more-btn" ${homeLibraryState.loading ? 'disabled' : ''}>
          ${homeLibraryState.loading ? 'Loading…' : 'Load more'}
        </button>
      </div>
    ` : ''}
  `;

  const loadMoreButton = document.querySelector('#load-more-library');
  if (loadMoreButton) {
    loadMoreButton.addEventListener('click', async () => {
      await loadAndRenderHomeItems();
    });
  }
}

async function loadAndRenderHomeItems({ reset = false } = {}) {
  const contentEl = document.querySelector('#content');
  if (!contentEl || homeLibraryState.loading) return;

  if (reset) {
    homeLibraryState = createHomeLibraryState();
    contentEl.innerHTML = `
      <div class="loading-container">
        <div class="spinner"></div>
        <p>Loading your library...</p>
      </div>
    `;
  } else if (homeLibraryState.totalRecordCount > 0 && homeLibraryState.items.length >= homeLibraryState.totalRecordCount) {
    return;
  }

  homeLibraryState.loading = true;
  if (!reset && homeLibraryState.items.length > 0) {
    renderHomeLibraryContent();
  }

  try {
    const payload = await fetchLibraryItems({
      searchTerm: currentQuery,
      startIndex: reset ? 0 : homeLibraryState.items.length,
      limit: LIBRARY_PAGE_SIZE,
    });

    homeLibraryState.items = reset
      ? payload.items
      : [...homeLibraryState.items, ...payload.items];
    homeLibraryState.totalRecordCount = payload.total_record_count || homeLibraryState.items.length;
  } catch (err) {
    console.error('Failed to fetch library items', err);
    contentEl.innerHTML = `
      <div class="error-container">
        <p>Failed to load library items. Please check connection.</p>
      </div>
    `;
    homeLibraryState.loading = false;
    return;
  }

  homeLibraryState.loading = false;
  renderHomeLibraryContent();
}

async function renderSeriesPage(app, id) {
  rememberBrowseHash();
  rememberDetailHash();

  app.innerHTML = `
    ${renderNav()}
    <main class="page-fade-in">
      <div id="detail-content">
        <div class="loading-container">
          <div class="spinner"></div>
          <p>Loading series...</p>
        </div>
      </div>
    </main>
  `;

  const detailContentEl = document.querySelector('#detail-content');
  if (!detailContentEl) return;

  try {
    const [item, childResponse] = await Promise.all([
      fetchLibraryItem(id),
      fetchLibraryItems({ parentId: id, limit: 500 }),
    ]);

    if (item.kind !== 'series') {
      throw new Error(`Expected series item but received ${item.kind}`);
    }

    const children = childResponse.items || [];
    const metaBadges = [formatLibraryItemMeta(item)];

    detailContentEl.innerHTML = `
      ${renderLibraryDetailHero({
        item,
        backHref: '#/',
        backLabel: '← Back to Library',
        overviewFallback: 'Browse the seasons available in this series.',
        metaBadges,
      })}
      ${renderLibrarySection({
        title: 'Seasons',
        countLabel: `${children.length} ${children.length === 1 ? 'season' : 'seasons'}`,
        contentHtml: children.map(renderLibraryCard).join(''),
        emptyMessage: 'No seasons found for this series.',
      })}
    `;
  } catch (err) {
    console.error('Failed to load series detail', err);
    detailContentEl.innerHTML = `
      <div class="error-container">
        <p>Failed to load series details.</p>
        <a href="#/" class="back-link" style="margin-top: 1rem;">← Return to Library</a>
      </div>
    `;
  }
}

async function renderSeasonPage(app, id) {
  rememberBrowseHash();
  rememberDetailHash();

  app.innerHTML = `
    ${renderNav()}
    <main class="page-fade-in">
      <div id="detail-content">
        <div class="loading-container">
          <div class="spinner"></div>
          <p>Loading season...</p>
        </div>
      </div>
    </main>
  `;

  const detailContentEl = document.querySelector('#detail-content');
  if (!detailContentEl) return;

  try {
    const [item, childResponse] = await Promise.all([
      fetchLibraryItem(id),
      fetchLibraryItems({ parentId: id, limit: 500 }),
    ]);

    if (item.kind !== 'season') {
      throw new Error(`Expected season item but received ${item.kind}`);
    }

    const episodes = childResponse.items || [];
    const metaBadges = [formatLibraryItemMeta(item)];
    if (item.parent_id) {
      const parentSeries = await fetchLibraryItem(item.parent_id);
      if (parentSeries?.title) {
        metaBadges.push(parentSeries.title);
      }
    }

    detailContentEl.innerHTML = `
      ${renderLibraryDetailHero({
        item,
        backHref: item.parent_id ? `#/series/${encodeURIComponent(item.parent_id)}` : '#/',
        backLabel: '← Back to Series',
        overviewFallback: 'Choose an episode to start playback immediately.',
        metaBadges,
      })}
      ${renderLibrarySection({
        title: 'Episodes',
        countLabel: `${episodes.length} ${episodes.length === 1 ? 'episode' : 'episodes'}`,
        contentClass: 'library-episode-list',
        contentHtml: renderEpisodeList(episodes),
        emptyMessage: 'No episodes found for this season.',
      })}
    `;
  } catch (err) {
    console.error('Failed to load season detail', err);
    detailContentEl.innerHTML = `
      <div class="error-container">
        <p>Failed to load season details.</p>
        <a href="#/" class="back-link" style="margin-top: 1rem;">← Return to Library</a>
      </div>
    `;
  }
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
  destroyPlayerInstance();

  const hash = window.location.hash || '#/';
  const app = document.querySelector('#app');

  if (hash === '#/' || hash === '') {
    await renderHomePage(app);
  } else if (hash.startsWith('#/series/')) {
    const id = decodeURIComponent(hash.slice('#/series/'.length));
    await renderSeriesPage(app, id);
  } else if (hash.startsWith('#/season/')) {
    const id = decodeURIComponent(hash.slice('#/season/'.length));
    await renderSeasonPage(app, id);
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
  rememberBrowseHash();

  app.innerHTML = `
    ${renderNav()}
    <main class="page-fade-in">
      <header>
        <h1>Library</h1>
        <p class="subtitle">Your personal collection of premium movies and shows</p>
      </header>
      <div class="search-container">
        <input type="text" id="search-input" class="search-input" placeholder="Search movies and series..." value="${escapeHtml(currentQuery)}">
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

  await loadAndRenderHomeItems({ reset: true });
}

// 防抖搜索处理
let searchTimeout;
function handleSearchInput(event) {
  const q = event.target.value;
  currentQuery = q;
  clearTimeout(searchTimeout);
  searchTimeout = setTimeout(async () => {
    await loadAndRenderHomeItems({ reset: true });
  }, DEBOUNCE_DELAY_MS);
}

// 电影详情页渲染
async function renderDetailPage(app, id) {
  rememberDetailHash();

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
    const runtimeStr = formatRuntime(movie.runtime_seconds, movie.runtime_minutes);

    detailContentEl.innerHTML = `
      <div class="movie-detail-hero">
        ${posterHtml}
        <div class="movie-detail-info">
          <a href="${escapeHtml(lastBrowseHash || '#/')}" class="back-link">← Back to Library</a>
          <h1 class="movie-detail-title">${escapeHtml(movie.title)}</h1>
          <div class="movie-detail-meta">
            <span class="movie-detail-year">${escapeHtml(movie.year || 'Unknown')}</span>
            ${runtimeStr ? `<span class="movie-detail-badge">${escapeHtml(runtimeStr)}</span>` : ''}
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
  const isTranscodeMode = mode === 'transcode';
  const modeLabel = isTranscodeMode ? '转码模式' : '直刷模式';
  const modeHint = isTranscodeMode
    ? '服务端实时转码为 MP4 流；拖动或快进会从目标时间重新建立转码会话。'
    : '浏览器直接解码原文件，零额外处理但取决于格式兼容性。';
  let selectedTranscodeQuality = 'source';
  const moviePromise = apiFetch(`/api/v1/movies/${id}`)
    .then(async (response) => {
      if (!response.ok) return null;
      return response.json();
    })
    .catch((err) => {
      console.warn('Failed to preload movie metadata for player', err);
      return null;
    });

  app.innerHTML = `
    <div class="player-container page-fade-in">
      <div class="player-topbar">
        <a href="${escapeHtml(lastDetailHash || `#/movie/${id}`)}" class="player-back">← 返回详情</a>
        <div class="player-status">
          <span class="player-status-pill">${modeLabel}</span>
          <span id="player-runtime-note" class="player-runtime-note">${modeHint}</span>
        </div>
      </div>
      <div class="player-stage">
        <div id="player-shell" class="player-shell">
          <video id="player-video" class="video-player" playsinline preload="metadata"></video>
          <button type="button" id="player-center-toggle" class="player-center-toggle">播放</button>
          <div class="player-controls">
            <div class="player-progress-row">
              <span id="player-current-time" class="player-time">00:00</span>
              <input id="player-progress" class="player-range player-progress" type="range" min="0" max="1000" value="0" step="1" aria-label="播放进度">
              <span id="player-duration" class="player-time">00:00</span>
            </div>
            <div class="player-controls-row">
              <div class="player-controls-group">
                <button type="button" id="player-play-toggle" class="player-control">播放</button>
                <button type="button" id="player-mute-toggle" class="player-control">静音</button>
                <input id="player-volume" class="player-range player-volume" type="range" min="0" max="100" value="100" step="1" aria-label="音量">
              </div>
              <div class="player-controls-group">
                <button type="button" id="player-pip-toggle" class="player-control">画中画</button>
                <button type="button" id="player-fullscreen-toggle" class="player-control">全屏</button>
              </div>
            </div>
          </div>
        </div>
      </div>
      <div class="player-switch-panel">
        <div class="player-mode-switch">
          <a href="#/play/${id}/direct" class="player-btn ${mode === 'direct' ? 'active' : ''}">直刷模式</a>
          <a href="#/play/${id}/transcode" class="player-btn ${mode === 'transcode' ? 'active' : ''}">转码模式</a>
        </div>
        <div id="player-quality-switch" class="player-quality-switch" ${isTranscodeMode ? '' : 'hidden'}>
          ${TRANSCODE_QUALITY_OPTIONS.map((item) => `
            <button type="button" class="player-btn player-quality-btn ${item.value === selectedTranscodeQuality ? 'active' : ''}" data-quality="${escapeHtml(item.value)}">${escapeHtml(item.label)}</button>
          `).join('')}
        </div>
      </div>
    </div>
  `;

  const playerContainer = document.querySelector('.player-container');
  const shell = document.querySelector('#player-shell');
  const topbar = document.querySelector('.player-topbar');
  const controls = document.querySelector('.player-controls');
  const modeSwitch = document.querySelector('.player-mode-switch');
  const qualitySwitch = document.querySelector('#player-quality-switch');
  const video = document.querySelector('#player-video');
  const playToggle = document.querySelector('#player-play-toggle');
  const centerToggle = document.querySelector('#player-center-toggle');
  const muteToggle = document.querySelector('#player-mute-toggle');
  const progress = document.querySelector('#player-progress');
  const volume = document.querySelector('#player-volume');
  const currentTimeEl = document.querySelector('#player-current-time');
  const durationEl = document.querySelector('#player-duration');
  const fullscreenToggle = document.querySelector('#player-fullscreen-toggle');
  const pipToggle = document.querySelector('#player-pip-toggle');
  const runtimeNote = document.querySelector('#player-runtime-note');

  if (!playerContainer || !shell || !topbar || !controls || !modeSwitch || !qualitySwitch || !video || !playToggle || !centerToggle || !muteToggle || !progress || !volume || !currentTimeEl || !durationEl || !fullscreenToggle || !pipToggle || !runtimeNote) {
    renderPlayerError('播放器 DOM 初始化不完整，请刷新页面重试。');
    return;
  }

  let idleTimer = null;
  let playbackOffsetSeconds = 0;
  let knownDurationSeconds = 0;
  let pendingTranscodeJump = false;
  const seekState = createSeekState();

  const setRuntimeNote = (message) => {
    runtimeNote.textContent = message;
  };

  const currentModeHint = () => {
    if (!isTranscodeMode) return modeHint;
    return `${modeHint} 当前清晰度：${getTranscodeQualityLabel(selectedTranscodeQuality)}。`;
  };

  const syncQualityButtons = () => {
    qualitySwitch.querySelectorAll('[data-quality]').forEach((button) => {
      button.classList.toggle('active', button.dataset.quality === selectedTranscodeQuality);
    });
  };

  const clampSeekTarget = (seconds) => {
    const normalized = Number.isFinite(seconds) ? Math.max(0, seconds) : 0;
    return knownDurationSeconds > 0 ? Math.min(normalized, knownDurationSeconds) : normalized;
  };

  const getDisplayCurrentTime = () => {
    const currentTime = Number.isFinite(video.currentTime) ? video.currentTime : 0;
    return isTranscodeMode ? clampSeekTarget(playbackOffsetSeconds + currentTime) : currentTime;
  };

  const getDisplayDuration = () => {
    if (isTranscodeMode && knownDurationSeconds > 0) {
      return knownDurationSeconds;
    }

    const duration = Number.isFinite(video.duration) ? video.duration : 0;
    return isTranscodeMode ? Math.max(duration, playbackOffsetSeconds + duration) : duration;
  };

  const buildStreamPath = (startTimeSeconds = 0) => {
    if (isTranscodeMode) {
      return buildTranscodeStreamPath(id, startTimeSeconds, selectedTranscodeQuality);
    }
    return `/api/v1/movies/${id}/direct`;
  };

  const setPlayerUiVisible = (visible) => {
    playerContainer.classList.toggle('player-ui-hidden', !visible);
  };

  const clearUiHideTimer = () => {
    if (idleTimer !== null) {
      window.clearTimeout(idleTimer);
      idleTimer = null;
    }
  };

  const scheduleUiHide = () => {
    clearUiHideTimer();
    if (video.paused || video.ended) {
      setPlayerUiVisible(true);
      return;
    }

    idleTimer = window.setTimeout(() => {
      if (!video.paused && !video.ended) {
        setPlayerUiVisible(false);
      }
    }, PLAYER_UI_IDLE_MS);
  };

  const markPlayerInteraction = () => {
    setPlayerUiVisible(true);
    scheduleUiHide();
  };

  const syncControls = () => {
    const paused = video.paused || video.ended;
    const duration = getDisplayDuration();
    const actualCurrentTime = getDisplayCurrentTime();
    seekState.settle(actualCurrentTime);
    const currentTime = seekState.getDisplayTime(actualCurrentTime);
    const volumeValue = video.muted ? 0 : Math.round((video.volume || 0) * 100);

    playToggle.textContent = paused ? '播放' : '暂停';
    centerToggle.textContent = paused ? '播放' : '暂停';
    muteToggle.textContent = video.muted || volumeValue === 0 ? '取消静音' : '静音';
    fullscreenToggle.textContent = document.fullscreenElement === shell ? '退出全屏' : '全屏';
    currentTimeEl.textContent = formatPlaybackTime(currentTime);
    durationEl.textContent = formatPlaybackTime(duration);

    progress.max = duration > 0 ? String(Math.floor(duration * 10)) : '1000';
    progress.value = duration > 0 ? String(Math.floor(currentTime * 10)) : '0';
    volume.value = String(volumeValue);
    setRangeProgress(progress);
    setRangeProgress(volume);

    if (paused) {
      clearUiHideTimer();
      setPlayerUiVisible(true);
    }
  };

  const loadPlayerSource = async (startTimeSeconds = 0, runtimeMessage = currentModeHint()) => {
    const normalizedStartTime = isTranscodeMode ? clampSeekTarget(startTimeSeconds) : 0;
    if (isTranscodeMode) {
      playbackOffsetSeconds = normalizedStartTime;
      pendingTranscodeJump = normalizedStartTime > 0;
    } else {
      playbackOffsetSeconds = 0;
      pendingTranscodeJump = false;
    }

    const streamUrl = await buildApiUrl(buildStreamPath(normalizedStartTime));
    video.pause();
    video.src = streamUrl;
    video.load();
    setRuntimeNote(runtimeMessage);
    syncControls();

    await video.play().catch((err) => {
      console.warn('Autoplay prevented', err);
      setRuntimeNote('自动播放被浏览器拦截，请点击播放继续。');
    });
  };

  const restartTranscodeAt = async (nextTimeSeconds) => {
    const target = clampSeekTarget(nextTimeSeconds);
    await loadPlayerSource(
      target,
      `正在以 ${getTranscodeQualityLabel(selectedTranscodeQuality)} 从 ${formatPlaybackTime(target)} 重新建立转码流...`
    );
  };

  const togglePlayback = async () => {
    if (video.paused || video.ended) {
      try {
        await video.play();
      } catch (err) {
        console.warn('Failed to start playback', err);
        setRuntimeNote('自动播放被浏览器拦截，请手动点击播放。');
      }
    } else {
      video.pause();
    }
  };

  const handleKeydown = async (event) => {
    const tagName = event.target?.tagName;
    if (tagName === 'INPUT' || tagName === 'TEXTAREA') return;

    if (event.code === 'Space') {
      event.preventDefault();
      await togglePlayback();
      return;
    }

    if (event.key === 'ArrowLeft') {
      event.preventDefault();
      if (isTranscodeMode) {
        await restartTranscodeAt(getDisplayCurrentTime() - 10);
      } else {
        video.currentTime = Math.max(0, video.currentTime - 10);
      }
      return;
    }

    if (event.key === 'ArrowRight') {
      event.preventDefault();
      if (isTranscodeMode) {
        await restartTranscodeAt(getDisplayCurrentTime() + 10);
      } else {
        video.currentTime = Math.min(Number.isFinite(video.duration) ? video.duration : video.currentTime + 10, video.currentTime + 10);
      }
      return;
    }

    if (event.key.toLowerCase() === 'm') {
      event.preventDefault();
      video.muted = !video.muted;
      syncControls();
      return;
    }

    if (event.key.toLowerCase() === 'f') {
      event.preventDefault();
      await toggleFullscreen(shell);
    }
  };

  const onPlay = () => {
    if (isTranscodeMode && playbackOffsetSeconds > 0) {
      setRuntimeNote(`已从 ${formatPlaybackTime(playbackOffsetSeconds)} 开始继续播放。`);
    } else {
      setRuntimeNote(currentModeHint());
    }
    syncControls();
    scheduleUiHide();
  };
  const onPause = () => {
    if (!video.ended) {
      setRuntimeNote('播放已暂停。');
    }
    clearUiHideTimer();
    setPlayerUiVisible(true);
    syncControls();
  };
  const onWaiting = () => {
    setRuntimeNote('正在缓冲媒体流...');
    setPlayerUiVisible(true);
    scheduleUiHide();
    syncControls();
  };
  const onEnded = () => {
    setRuntimeNote('播放完成。');
    clearUiHideTimer();
    setPlayerUiVisible(true);
    syncControls();
  };
  const onLoadedMetadata = () => {
    updatePlayerViewportClasses(playerContainer, video);
    if (pendingTranscodeJump) {
      setRuntimeNote(`已跳转到 ${formatPlaybackTime(playbackOffsetSeconds)}，继续转码播放。`);
      pendingTranscodeJump = false;
    } else {
      setRuntimeNote(currentModeHint());
    }
    syncControls();
  };
  const onTimeUpdate = () => syncControls();
  const onSeeked = () => syncControls();
  const onVolumeChange = () => syncControls();
  const onFullscreenChange = () => {
    updatePlayerViewportClasses(playerContainer, video);
    syncControls();
  };
  const onError = () => {
    console.error('Native video playback failed', video.error);
    clearUiHideTimer();
    setPlayerUiVisible(true);
    renderPlayerError('媒体加载失败，请尝试切换播放模式，或检查浏览器格式兼容性与后端转码日志。');
  };
  const onViewportChange = () => {
    updatePlayerViewportClasses(playerContainer, video);
    markPlayerInteraction();
  };

  const onShellMouseMove = () => markPlayerInteraction();
  const onShellTouchStart = () => markPlayerInteraction();
  const onShellTouchMove = () => markPlayerInteraction();
  const onShellPointerDown = () => markPlayerInteraction();
  const onShellFocusIn = () => markPlayerInteraction();

  playToggle.addEventListener('click', togglePlayback);
  centerToggle.addEventListener('click', togglePlayback);
  muteToggle.addEventListener('click', () => {
    video.muted = !video.muted;
    syncControls();
  });
  fullscreenToggle.addEventListener('click', async () => {
    await toggleFullscreen(shell);
  });
  pipToggle.addEventListener('click', async () => {
    if (!document.pictureInPictureEnabled || video.disablePictureInPicture) {
      setRuntimeNote('当前浏览器不支持画中画。');
      return;
    }

    try {
      if (document.pictureInPictureElement === video) {
        await document.exitPictureInPicture();
      } else {
        await video.requestPictureInPicture();
      }
    } catch (err) {
      console.warn('Failed to toggle picture-in-picture', err);
      setRuntimeNote('画中画切换失败，请检查浏览器权限。');
    }
  });
  qualitySwitch.querySelectorAll('[data-quality]').forEach((button) => {
    button.addEventListener('click', async () => {
      const nextQuality = button.dataset.quality || 'source';
      if (!isTranscodeMode || nextQuality === selectedTranscodeQuality) return;
      selectedTranscodeQuality = nextQuality;
      syncQualityButtons();
      await restartTranscodeAt(getDisplayCurrentTime());
    });
  });
  progress.addEventListener('input', () => {
    const nextTime = clampSeekTarget(Number(progress.value) / 10);
    seekState.update(nextTime);
    progress.value = String(Math.floor(nextTime * 10));
    currentTimeEl.textContent = formatPlaybackTime(nextTime);
    setRangeProgress(progress);
  });
  progress.addEventListener('change', async () => {
    const nextTime = seekState.commit(clampSeekTarget(Number(progress.value) / 10));
    progress.value = String(Math.floor(nextTime * 10));
    currentTimeEl.textContent = formatPlaybackTime(nextTime);
    setRangeProgress(progress);

    if (isTranscodeMode) {
      await restartTranscodeAt(nextTime);
      seekState.clear();
      syncControls();
      return;
    }

    if (Number.isFinite(video.duration)) {
      video.currentTime = nextTime;
      syncControls();
    } else {
      seekState.clear();
      syncControls();
    }
  });
  volume.addEventListener('input', () => {
    const nextVolume = Number(volume.value) / 100;
    video.volume = nextVolume;
    video.muted = nextVolume === 0;
    setRangeProgress(volume);
    syncControls();
  });
  video.addEventListener('click', togglePlayback);
  video.addEventListener('dblclick', async () => {
    await toggleFullscreen(shell);
  });
  video.addEventListener('play', onPlay);
  video.addEventListener('pause', onPause);
  video.addEventListener('waiting', onWaiting);
  video.addEventListener('ended', onEnded);
  video.addEventListener('loadedmetadata', onLoadedMetadata);
  video.addEventListener('timeupdate', onTimeUpdate);
  video.addEventListener('seeked', onSeeked);
  video.addEventListener('volumechange', onVolumeChange);
  video.addEventListener('error', onError);
  document.addEventListener('fullscreenchange', onFullscreenChange);
  document.addEventListener('keydown', handleKeydown);
  window.addEventListener('resize', onViewportChange);
  window.addEventListener('orientationchange', onViewportChange);
  shell.addEventListener('mousemove', onShellMouseMove);
  shell.addEventListener('touchstart', onShellTouchStart, { passive: true });
  shell.addEventListener('touchmove', onShellTouchMove, { passive: true });
  shell.addEventListener('pointerdown', onShellPointerDown);
  shell.addEventListener('focusin', onShellFocusIn);

  if (!document.pictureInPictureEnabled || video.disablePictureInPicture) {
    pipToggle.hidden = true;
  }

  playerInstance = {
    video,
    pause() {
      video.pause();
    },
    destroy() {
      clearUiHideTimer();
      document.removeEventListener('fullscreenchange', onFullscreenChange);
      document.removeEventListener('keydown', handleKeydown);
      window.removeEventListener('resize', onViewportChange);
      window.removeEventListener('orientationchange', onViewportChange);
      shell.removeEventListener('mousemove', onShellMouseMove);
      shell.removeEventListener('touchstart', onShellTouchStart);
      shell.removeEventListener('touchmove', onShellTouchMove);
      shell.removeEventListener('pointerdown', onShellPointerDown);
      shell.removeEventListener('focusin', onShellFocusIn);
      video.removeEventListener('click', togglePlayback);
      video.removeEventListener('dblclick', toggleFullscreen);
      video.removeEventListener('play', onPlay);
      video.removeEventListener('pause', onPause);
      video.removeEventListener('waiting', onWaiting);
      video.removeEventListener('ended', onEnded);
      video.removeEventListener('loadedmetadata', onLoadedMetadata);
      video.removeEventListener('timeupdate', onTimeUpdate);
      video.removeEventListener('seeked', onSeeked);
      video.removeEventListener('volumechange', onVolumeChange);
      video.removeEventListener('error', onError);
      playToggle.removeEventListener('click', togglePlayback);
      centerToggle.removeEventListener('click', togglePlayback);
      video.pause();
      video.removeAttribute('src');
      video.load();
    }
  };

  updatePlayerViewportClasses(playerContainer, video);
  syncQualityButtons();
  syncControls();
  setPlayerUiVisible(true);

  try {
    const movie = await moviePromise;
    const runtimeSeconds = Number(movie?.runtime_seconds);
    const runtimeMinutes = Number(movie?.runtime_minutes);
    if (Number.isFinite(runtimeSeconds) && runtimeSeconds > 0) {
      knownDurationSeconds = runtimeSeconds;
      syncControls();
    } else if (Number.isFinite(runtimeMinutes) && runtimeMinutes > 0) {
      knownDurationSeconds = runtimeMinutes * 60;
      syncControls();
    }

    await loadPlayerSource(0, currentModeHint());
  } catch (err) {
    console.error('Failed to initialize native player', err);
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
