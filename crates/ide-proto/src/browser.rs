//! Browser control by agents (`docs/browser-control.md`): tool names shared by the daemon and
//! the app, and the site (origin) an address belongs to, the target of its approvals.

/// `ide_browser_list`: the pages this session can reach (app).
pub const TOOL_LIST: &str = "ide_browser_list";
/// `ide_browser_open {url}`: a new browser tab of the session (approval of the site, app).
pub const TOOL_OPEN: &str = "ide_browser_open";
/// `ide_browser_read {tab, what}`: text / console / DOM of a page (approval, app).
pub const TOOL_READ: &str = "ide_browser_read";
/// `ide_browser_act {tab, action, ...}`: navigate, click, type, key, scroll, wait, evaluate
/// (approval, app).
pub const TOOL_ACT: &str = "ide_browser_act";
/// `ide_browser_screenshot {tab}`: a PNG of the page (approval, app).
pub const TOOL_SCREENSHOT: &str = "ide_browser_screenshot";
/// Not an agent tool: the daemon asks the app which site a tab shows (`{tab}` -> `{origin,
/// url, title}`) before it asks the user's approval for that site.
pub const TAB_SITE: &str = "ide.browser.site";

/// Every agent tool of browser control.
pub const TOOLS: [&str; 5] = [TOOL_LIST, TOOL_OPEN, TOOL_READ, TOOL_ACT, TOOL_SCREENSHOT];

/// Capability of acting on a site (open, navigate, click, type, evaluate, ...).
pub const CAP_ACT: &str = "browser.act";
/// Capability of reading a site (text, console, DOM, screenshot); a `browser.act` grant also
/// allows it.
pub const CAP_READ: &str = "browser.read";

/// The site an address belongs to, as approvals key it: `scheme://host[:port]` for web
/// addresses (lower case, no user info, no default port), `file://` for local files, the
/// scheme (`data:`, `about:`) for pages without a host.
pub fn origin(url: &str) -> String {
    let url = url.trim();
    let Some((scheme, rest)) = url.split_once(':') else {
        return url.to_ascii_lowercase();
    };
    let scheme = scheme.to_ascii_lowercase();
    let Some(rest) = rest.strip_prefix("//") else {
        return format!("{scheme}:");
    };
    if scheme == "file" {
        return "file://".into();
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let hostport = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let hostport = hostport.to_ascii_lowercase();
    let default_port = match scheme.as_str() {
        "http" | "ws" => Some(":80"),
        "https" | "wss" => Some(":443"),
        _ => None,
    };
    let hostport = match default_port {
        Some(p) => hostport.strip_suffix(p).unwrap_or(&hostport).to_owned(),
        None => hostport,
    };
    format!("{scheme}://{hostport}")
}

#[cfg(test)]
mod tests {
    use super::origin;

    #[test]
    fn origins() {
        assert_eq!(origin("https://Example.com/a?b#c"), "https://example.com");
        assert_eq!(origin("http://localhost:5173/x"), "http://localhost:5173");
        assert_eq!(origin("http://u:p@host:80/"), "http://host");
        assert_eq!(origin("https://h:443"), "https://h");
        assert_eq!(origin("http://[::1]:8080/"), "http://[::1]:8080");
        assert_eq!(origin("file:///C:/x/y.html"), "file://");
        assert_eq!(origin("data:text/html,<p>hi</p>"), "data:");
        assert_eq!(origin("about:blank"), "about:");
        assert_eq!(origin("ide-widget://w.s/index.html"), "ide-widget://w.s");
    }
}
