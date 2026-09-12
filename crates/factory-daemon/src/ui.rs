//! The web UI, compiled into the daemon so there is still one artefact to run.
//!
//! The page is split into ES modules -- one per view -- rather than one long
//! file. The browser resolves the imports itself, so there is no build step and
//! no bundler; the cost is that every file has to be named here to be served.
//! The list is short and a missing entry fails loudly in the browser console,
//! which is the trade worth making at this size.

pub const INDEX_HTML: &str = include_str!("../../../ui/index.html");

/// Everything the page asks for after the first byte, by request path.
const ASSETS: &[(&str, &str, &str)] = &[
    (
        "app.css",
        "text/css; charset=utf-8",
        include_str!("../../../ui/app.css"),
    ),
    (
        "js/core.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../ui/js/core.js"),
    ),
    (
        "js/modal.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../ui/js/modal.js"),
    ),
    (
        "js/terminal.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../ui/js/terminal.js"),
    ),
    (
        "js/tasks.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../ui/js/tasks.js"),
    ),
    (
        "js/workflows.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../ui/js/workflows.js"),
    ),
    (
        "js/workflow-graph.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../ui/js/workflow-graph.js"),
    ),
    (
        "js/task-form.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../ui/js/task-form.js"),
    ),
    (
        "js/agents.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../ui/js/agents.js"),
    ),
    (
        "js/agent-config.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../ui/js/agent-config.js"),
    ),
    (
        "js/occupancy.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../ui/js/occupancy.js"),
    ),
    (
        "js/agent-runtime.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../ui/js/agent-runtime.js"),
    ),
    (
        "js/scopes.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../ui/js/scopes.js"),
    ),
    (
        "js/dashboard.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../ui/js/dashboard.js"),
    ),
    (
        "js/activity.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../ui/js/activity.js"),
    ),
    (
        "js/site.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../ui/js/site.js"),
    ),
    (
        "js/site-focus.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../ui/js/site-focus.js"),
    ),
    (
        "js/site-render.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../ui/js/site-render.js"),
    ),
    (
        "js/app.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../ui/js/app.js"),
    ),
    // Vendored rather than fetched from a CDN: the daemon compiles this in, so
    // a machine with no internet still has the render mode available. r128,
    // unmodified, from https://cdnjs.cloudflare.com/ajax/libs/three.js/r128/three.min.js
    (
        "vendor/three.min.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../ui/vendor/three.min.js"),
    ),
];

/// The content type and body for one asset path, or `None` -- which the route
/// turns into a 404 rather than guessing.
pub fn asset(path: &str) -> Option<(&'static str, &'static str)> {
    ASSETS
        .iter()
        .find(|(name, _, _)| *name == path)
        .map(|(_, mime, body)| (*mime, *body))
}

#[cfg(test)]
mod tests {
    /// Every module the page imports has to be in the table, or the browser
    /// gets a 404 for a file that exists on disk. Catch that here rather than
    /// in someone's console.
    #[test]
    fn every_module_on_disk_is_served() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../ui/js");
        for entry in std::fs::read_dir(dir).expect("ui/js") {
            let name = entry.unwrap().file_name().to_string_lossy().to_string();
            if !name.ends_with(".js") {
                continue;
            }
            assert!(
                super::asset(&format!("js/{name}")).is_some(),
                "ui/js/{name} is not in the served list"
            );
        }
    }

    /// The same, for the vendored libraries: a daemon with no internet still
    /// has to find them under `/ui/vendor/`.
    #[test]
    fn every_vendored_file_on_disk_is_served() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../ui/vendor");
        for entry in std::fs::read_dir(dir).expect("ui/vendor") {
            let name = entry.unwrap().file_name().to_string_lossy().to_string();
            assert!(
                super::asset(&format!("vendor/{name}")).is_some(),
                "ui/vendor/{name} is not in the served list"
            );
        }
    }

    #[test]
    fn dashboard_menu_owns_the_requested_views_in_order() {
        let dashboard = super::INDEX_HTML
            .find("id=\"lv-dash\"")
            .expect("Dashboard primary menu");
        let l6 = super::INDEX_HTML.find("id=\"lv-dir\"").expect("L6 menu");
        assert!(dashboard < l6, "Dashboard should be a peer before L6-L1");

        let tabs = ["tab-dashboard", "tab-site", "tab-activity", "tab-inbox"];
        let positions: Vec<_> = tabs
            .iter()
            .map(|id| super::INDEX_HTML.find(&format!("id=\"{id}\"")).expect(id))
            .collect();
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(super::INDEX_HTML.contains("id=\"view-inbox\""));
    }
}
