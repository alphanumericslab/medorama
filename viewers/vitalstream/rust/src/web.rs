//! The web page (HTML, CSS, JavaScript), embedded in the binary: the one copy in `<repo>/web/`,
//! shared by the Python, Rust and C++ viewers (`build.rs` rebuilds when it changes).

use include_dir::{include_dir, Dir};

static WEB: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../web");

/// Contents of a web file (`index.html`, `style.css`, `js/app.js`, ...).
pub fn file(path: &str) -> Option<Vec<u8>> {
    if path.split('/').any(|c| c == "..") || path == "README.md" {
        return None;
    }
    WEB.get_file(path).map(|f| f.contents().to_vec())
}
