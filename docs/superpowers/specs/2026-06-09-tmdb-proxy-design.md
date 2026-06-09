# TMDB API Proxy & Base URL Configuration Design

This document outlines the design specification for adding a standard HTTP/SOCKS proxy option and a custom API base URL override for the TMDB scraper.

## 1. Requirements

* **Configuration**: Allow configuring a TMDB Proxy URL (e.g., `http://127.0.0.1:7890`) and a TMDB API Base URL (e.g., `https://api.tmdb.org` or any reverse-proxy mirror) in the system's `config.toml` file.
* **Metadata Scraper**: Use `reqwest`'s proxy configuration in `TmdbScraper` to route queries if a proxy URL is supplied. Override the default `https://api.themoviedb.org` URL if a base URL is specified.
* **API Endpoints**: Expose and persist the two new settings in the `/api/v1/config` JSON endpoints.
* **Web settings**: Provide input fields on the settings page of the single page web application for the proxy and base URL.

## 2. Technical Details

### 2.1 Server Config (`crates/rmc-server/src/config.rs`)
Add fields `tmdb_proxy_url: Option<String>` and `tmdb_api_base: Option<String>` to `ServerConfig`. Update serialization tests.

### 2.2 Scraper (`crates/rmc-server/src/scraper.rs`)
Modify `TmdbScraper::new` to accept `proxy_url` and `api_base`. Use `reqwest::Proxy::all` to set up client proxying. Construct TMDB API request URLs using the dynamic `api_base`.

### 2.3 API Route Handlers (`crates/rmc-server/src/api.rs`)
Pass the values `config.tmdb_proxy_url` and `config.tmdb_api_base` when constructing `TmdbScraper` in the scan background task.

### 2.4 Web Settings Frontend (`web-client/main.js`)
Introduce UI form fields to load, modify, and save proxy/base URL settings.

## 3. Verification Plan

* **Automated Unit Tests**: Verify config loading/saving defaults and TMDB Scraper constructor behaviour.
* **Manual Verification**: Launch the server, configure proxy parameters via UI, and confirm they persist in `config.toml`.
