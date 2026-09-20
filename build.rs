//! Make sure the directory the web UI is embedded from exists.
//!
//! `frontend/dist/` is a build output and is not tracked, so a fresh clone has
//! no such directory and `rust-embed` would fail at compile time. Writing a
//! placeholder keeps `cargo build` working on its own, while a full build (see
//! the Dockerfile) compiles the real UI into the same place first.

use std::{fs, path::Path};

const PLACEHOLDER: &str = r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <title>arr-metadata-server</title>
    <style>
      body { background:#0a0908; color:#ede8df; font-family:ui-monospace,monospace;
             display:grid; place-items:center; min-height:100dvh; margin:0; }
      div  { max-width:40ch; line-height:1.7; padding:2rem; }
      code { color:#e8a33d; }
    </style>
  </head>
  <body>
    <div>
      <p>The API is running, but the web UI was not built into this binary.</p>
      <p>Build it with <code>cd frontend &amp;&amp; npm ci &amp;&amp; npm run build</code>, then rebuild the server.</p>
    </div>
  </body>
</html>
"#;

fn main() {
    let dist = Path::new("frontend/dist");

    println!("cargo:rerun-if-changed=frontend/dist");

    if dist.join("index.html").exists() {
        return;
    }

    if let Err(e) = fs::create_dir_all(dist).and_then(|_| fs::write(dist.join("index.html"), PLACEHOLDER)) {
        println!("cargo:warning=could not create the web UI placeholder: {e}");
        return;
    }

    println!("cargo:warning=frontend/dist was empty; embedding a placeholder page instead of the web UI");
}
