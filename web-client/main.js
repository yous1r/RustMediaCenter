import './style.css';

document.querySelector('#app').innerHTML = `
  <nav>
    <div class="nav-brand">RustMediaCenter</div>
  </nav>
  <main>
    <header>
      <h1>Library</h1>
      <p class="subtitle">Your personal collection of movies and shows</p>
    </header>
    <div id="content">
      <div class="loading-container">
        <div class="spinner"></div>
        <p>加载中...</p>
      </div>
    </div>
  </main>
`;

async function fetchMovies() {
  const contentDiv = document.querySelector('#content');
  try {
    const response = await fetch('http://127.0.0.1:8000/api/v1/movies');
    if (!response.ok) {
      throw new Error(`HTTP error! status: ${response.status}`);
    }
    const data = await response.json();
    renderMovies(data);
  } catch (error) {
    console.warn('Failed to fetch from API, using fallback data:', error);
    // Fallback mock data
    const mockMovies = [
      { id: 1, title: 'Inception', year: 2010 },
      { id: 2, title: 'Interstellar', year: 2014 },
      { id: 3, title: 'The Dark Knight', year: 2008 },
      { id: 4, title: 'Dune: Part One', year: 2021 },
      { id: 5, title: 'Blade Runner 2049', year: 2017 },
      { id: 6, title: 'Arrival', year: 2016 },
      { id: 7, title: 'The Matrix', year: 1999 },
      { id: 8, title: 'Avatar: The Way of Water', year: 2022 },
      { id: 9, title: 'Tenet', year: 2020 },
      { id: 10, title: 'Everything Everywhere All at Once', year: 2022 }
    ];
    renderMovies(mockMovies);
  }
}

function renderMovies(movies) {
  const contentDiv = document.querySelector('#content');
  
  if (!movies || movies.length === 0) {
    contentDiv.innerHTML = `
      <div class="error-container">
        <p>No movies found.</p>
      </div>
    `;
    return;
  }

  const gridHtml = `
    <div class="movies-grid">
      ${movies.map(movie => `
        <div class="movie-card">
          <div class="movie-poster-placeholder">
            🎬
          </div>
          <div class="movie-info">
            <h3 class="movie-title">${movie.title}</h3>
            <span class="movie-year">${movie.year}</span>
          </div>
        </div>
      `).join('')}
    </div>
  `;

  contentDiv.innerHTML = gridHtml;
}

// Initialize
fetchMovies();
