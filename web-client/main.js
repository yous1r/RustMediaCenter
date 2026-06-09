import './style.css';

// 全局状态
let movies = [];

const MOCK_MOVIES = [
  { id: 1, title: 'Inception', year: 2010, poster_url: 'https://image.tmdb.org/t/p/w500/9gk7adHYeDvHkCSEqAvQNLV5Uge.jpg', overview: 'A thief who steals corporate secrets through the use of dream-sharing technology is given the inverse task of planting an idea into the mind of a C.E.O.' },
  { id: 2, title: 'Interstellar', year: 2014, poster_url: 'https://image.tmdb.org/t/p/w500/gEU2QniE6E77NI6lCU6MxlNBvIx.jpg', overview: 'A team of explorers travel through a wormhole in space in an attempt to ensure humanity\'s survival.' },
  { id: 3, title: 'The Dark Knight', year: 2008, poster_url: 'https://image.tmdb.org/t/p/w500/qJ2tW6WMUDux911r6m7haRef0WH.jpg', overview: 'When the menace known as the Joker wreaks havoc and chaos on the people of Gotham, Batman must accept one of the greatest psychological and physical tests of his ability to fight injustice.' }
];

async function fetchMovies() {
  try {
    const response = await fetch('http://127.0.0.1:8000/api/v1/movies');
    if (!response.ok) throw new Error('API error');
    movies = await response.json();
  } catch (err) {
    console.warn('Failed to fetch API, using premium mock data', err);
    movies = MOCK_MOVIES;
  }
}

// 简单的单页路由 (SPA Router)
async function router() {
  const hash = window.location.hash || '#/';
  const app = document.querySelector('#app');

  if (hash === '#/') {
    app.innerHTML = renderHome();
    if (movies.length === 0) {
      await fetchMovies();
      app.innerHTML = renderHome();
    }
  } else if (hash.startsWith('#/movie/')) {
    const id = parseInt(hash.split('/')[2]);
    const movie = movies.find(m => m.id === id) || MOCK_MOVIES.find(m => m.id === id) || { id, title: 'Unknown Movie', year: 2024 };
    app.innerHTML = renderDetail(movie);
  } else if (hash.startsWith('#/play/')) {
    const id = parseInt(hash.split('/')[2]);
    app.innerHTML = renderPlayer(id);
  } else if (hash === '#/login') {
    app.innerHTML = renderLogin();
  }
}

function renderNav() {
  return `
    <nav>
      <a href="#/" class="nav-brand" style="text-decoration:none;">RustMediaCenter</a>
      <a href="#/login" class="nav-login" style="text-decoration:none; color: var(--text-secondary); transition: color 0.3s; font-weight: 500;">Login</a>
    </nav>
  `;
}

function renderHome() {
  return `
    ${renderNav()}
    <main class="page-fade-in">
      <header>
        <h1>Library</h1>
        <p class="subtitle">Your personal collection of premium movies and shows</p>
      </header>
      <div id="content">
        ${movies.length === 0 ? `
          <div class="loading-container">
            <div class="spinner"></div>
            <p>Loading your cinematic experience...</p>
          </div>
        ` : `
          <div class="movies-grid">
            ${movies.map(movie => `
              <a href="#/movie/${movie.id}" class="movie-card" style="text-decoration: none;">
                <div class="movie-poster" style="background-image: url('${movie.poster_url || ''}')">
                  ${movie.poster_url ? '' : '🎬'}
                </div>
                <div class="movie-info">
                  <h3 class="movie-title">${movie.title}</h3>
                  <span class="movie-year">${movie.year || 'Unknown'}</span>
                </div>
              </a>
            `).join('')}
          </div>
        `}
      </div>
    </main>
  `;
}

function renderDetail(movie) {
  return `
    ${renderNav()}
    <main class="page-fade-in">
      <div class="movie-detail-hero">
        <div class="movie-detail-poster" style="background-image: url('${movie.poster_url || ''}')">
          ${movie.poster_url ? '' : '🎬'}
        </div>
        <div class="movie-detail-info">
          <a href="#/" class="back-link">← Back to Library</a>
          <h1 class="movie-detail-title">${movie.title}</h1>
          <div class="movie-detail-meta">
            <span class="movie-detail-year">${movie.year || 'Unknown'}</span>
            <span class="movie-detail-badge">4K HDR</span>
          </div>
          <p class="movie-detail-overview">${movie.overview || 'A breathtaking cinematic journey awaits. No description available yet.'}</p>
          <div class="movie-detail-actions">
            <a href="#/play/${movie.id}" class="btn-play">
              <svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
                <path d="M5 3L19 12L5 21V3Z" fill="currentColor" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
              </svg>
              Play Now
            </a>
          </div>
        </div>
      </div>
    </main>
  `;
}

function renderPlayer(id) {
  return `
    <div class="player-container page-fade-in">
      <a href="#/movie/${id}" class="player-back">← Back</a>
      <video controls autoplay class="video-player">
        <source src="http://127.0.0.1:8000/api/v1/movies/${id}/direct" type="video/mp4">
        Your browser does not support the video tag.
      </video>
    </div>
  `;
}

function renderLogin() {
  return `
    ${renderNav()}
    <main class="page-fade-in" style="display:flex; justify-content:center; align-items:center; min-height: 80vh;">
      <div class="login-card">
        <h2>Welcome Back</h2>
        <p style="color: var(--text-secondary); margin-bottom: 2rem;">Sign in to your premium theater</p>
        <form class="login-form">
          <input type="text" placeholder="Username" class="login-input" />
          <input type="password" placeholder="Password" class="login-input" />
          <button type="button" onclick="window.location.hash='#/'" class="btn-primary">Sign In</button>
        </form>
      </div>
    </main>
  `;
}

// 监听路由改变并初始化
window.addEventListener('hashchange', router);
router();

