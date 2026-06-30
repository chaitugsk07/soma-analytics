# soma-analytics dashboard

Leptos SPA compiled by [Trunk](https://trunkrs.dev). Two run modes:

## Dev mode (hot reload)

Run the dashboard with Trunk's dev server, and soma-server separately:

```sh
# Terminal 1 — dashboard hot-reload dev server (default: http://127.0.0.1:8080)
cd dashboard
trunk serve

# Terminal 2 — API server (allow cross-origin requests from Trunk's dev server)
ANALYTICS_CORS_ORIGINS=http://127.0.0.1:8080 cargo run -p soma-server
```

Set the API base URL and bearer token in the dashboard header inputs (the UI
reads these from the page; they are never stored server-side).

> Note: opening `dashboard/dist/index.html` directly from the filesystem (or
> via a plain static file server) will 404 on any SPA sub-route. Always use
> `trunk serve` or an index.html-fallback-capable HTTP server for dev.

## Single-binary mode (prod)

Build the dashboard first, then compile the server with the `dashboard` feature
so `dashboard/dist` is embedded at compile time via `rust-embed`.

```sh
# 1. Build the frontend (produces dashboard/dist/)
cd dashboard
trunk build --release

# 2. From the repo root — embed dist into the binary
cd ..
cargo build -p soma-server --release --features dashboard

# 3. Run — serves the SPA at / and the API at /api/v1/* from the same origin.
#    No ANALYTICS_CORS_ORIGINS needed (same-origin).
./target/release/soma-analytics
```

The `dashboard` feature is **OFF by default**. A plain `cargo build --workspace`
succeeds without `dashboard/dist` existing — the embed is feature-gated off.
