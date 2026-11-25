# Browser control

Status: implemented (roadmap item 2, `docs/agent-features-roadmap.md`; protocol bump pending:
`AgentToolResult.image` is new). Agents use the pages of Meninx's browser — the session's own web
page and its browser tabs (also 공용 ones and widget tabs) — through bridge tools, like Hermes
Desk's `browser_inspect` / `browser_act`. Everything happens inside the app's CEF views through
the Chrome DevTools protocol (CDP): no OS input (AGENTS.md).

Code: `meninx-web/src/devtools.rs` (CDP channel, console log, `Awake`), `meninx-web/src/cdp.rs`
(messages, mouse / key events, key names; pure), `meninx-proto/src/browser.rs` (tool names,
capabilities, `origin()`), daemon `meninx-server/src/service/browser.rs` (tool list, argument
checks, approvals, screenshot -> image), app `meninx-app/src/browser_agent.rs` (carries the tools
out). Tests: `meninx-web` `cdp::tests`, `meninx-proto` `browser::tests`, `meninx-server`
`service::browser::tests` and `tests/browser_e2e.rs`, `meninx-app` `browser_agent::tests`, app
script `MENINX_APP_SCRIPT_ONLY=browser` (`script_browser.rs`, real CEF, screenshot
`browser_01_tab`).

## Tools

| Tool | Approval | What |
|---|---|---|
| `meninx_browser_list` | none | tabs this session can use: `tab-<n>` (its folder's browser tabs, 공용 tabs, widget tabs) with kind, address, title, shown, loading |
| `meninx_browser_open {url}` | `browser.act` for the address's site | a new browser tab of the session (shown in its detail area); answers `{tab, url, loading}` after the page loaded (at most 15 s) |
| `meninx_browser_read {tab, what, selector?, max_chars?, limit?}` | `browser.read` | `text` (`innerText`, default), `dom` (`outerHTML`) of the page or of the first element matching `selector` (cut at `max_chars`, default 20 000, at most 200 000); `console`: the last `limit` (default 100, at most 300) console messages |
| `meninx_browser_act {tab, action, ...}` | `browser.act` (`wait`: `browser.read`; `navigate`: for the destination's site) | `navigate {url}`, `click {selector \| x, y}`, `type {text, selector?}`, `key {key}`, `scroll {selector \| dx?, dy?, x?, y?}`, `wait {selector \| text \| ms, timeout_ms?}` (at most 45 s), `evaluate {expression}` |
| `meninx_browser_screenshot {tab}` | `browser.read` | a PNG of the viewport as MCP image content |

Addresses: `http(s)://`, `file:`, `data:`, `about:`; a bare host gets `http://` (localhost,
IPs) or `https://`; other schemes (`javascript:`) are refused.

## How a call runs

1. The daemon checks and normalises the arguments (`plan()`): wrong ones never reach the app.
2. Approval (`docs/approvals.md`), target = the site (`meninx_proto::browser::origin`:
   `scheme://host[:port]`, `file://`, `data:`, `about:`):
   - `open` / `navigate`: the destination's site, `browser.act`.
   - tab tools: the daemon asks the app which site the tab shows (`Notify::AgentRequest`
     `meninx.browser.site {tab}` -> `{origin, url, title}`), then asks for that site. A kept
     `browser.act` grant also allows reading.
   - The question shows the requesting session, the site and what (`click #go`, `run
     JavaScript: ...`, `screenshot` with the page's address).
3. The daemon hands the call to the app with the approved `origin`; the app refuses when the
   tab shows another site by now ("call again": a new approval for the new site).
4. The app runs it on the tab's `WebView` (created when the tab was never shown), within 55 s.

## CDP in the app

- `WebView::devtools()` -> `DevTools::call(method, params)`: `send_dev_tools_message` with
  `{"id", "method", "params"}`, posted to CEF's UI thread (`post_task`; the runtime uses
  `multi_threaded_message_loop`); a `DevToolsMessageObserver` registered per browser
  (re-registered after a GPU fallback re-creates the browser) answers the request's future in
  `on_dev_tools_method_result`. The handle does not keep the browser open.
- `evaluate` / `read`: `Runtime.evaluate` (`returnByValue`, `awaitPromise`, `userGesture`);
  exceptions come back as `JavaScript error: ...`. Selectors are passed as JSON string literals.
- `click`: the element is scrolled into view, its centre taken from `getBoundingClientRect`;
  `Input.dispatchMouseEvent` moved / pressed / released.
- `type`: `focus()` the element, `Input.insertText` (IME-free, any script).
- `key`: `Input.dispatchKeyEvent` (`keyDown` with text / `rawKeyDown`, `keyUp`) from a key
  name: `Enter`, `Tab`, `Escape`, `Backspace`, `Delete`, arrows (`up` ...), `Home`, `End`,
  `PageUp`, `PageDown`, `F1`-`F12`, `Space`, one character; modifiers `ctrl+`, `alt+`, `shift+`,
  `meta+` / `cmd+`.
- `scroll`: `scrollIntoView` or a `mouseWheel` event at the viewport's centre; answers the
  scroll position.
- `screenshot`: `Page.captureScreenshot` (PNG). The app answers `{"text", "image": {"data",
  "mime_type"}}`; the daemon turns it into `AgentToolResult.image`, the MCP server into
  `content: [{type: text}, {type: image, data, mimeType}]`.
- Console: every view keeps its last 300 console messages (`on_console_message`, the same
  source as `WebEvent::Console`), from the moment it was created — not only after an agent
  attached (simpler and more useful than `Runtime.consoleAPICalled`).
- Hidden tabs (another tab shown, a modal dims the view) do not paint: while an agent acts on or
  captures a page, `DevTools::awake()` keeps it painting and taking input; it hides again after.

## Not like Desk (yet)

- No element picking / highlighting / recording / raw `cdp` passthrough / `browser_diagnose`.
- No per-site allow list editor beyond Settings -> 권한 (grants are per requesting session).
- The detached detail window shows the same views: no special handling needed.
- `macOS` / Linux: no OS-specific code here (CDP and `post_task` are portable), but only
  checked on Windows.
