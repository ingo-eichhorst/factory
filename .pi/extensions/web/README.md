# web

Two tools for pi in this project:

- `web_search` — search the public web, returns title, URL, and snippet.
- `web_fetch` — fetch one public HTTP(S) URL as readable text.

## Search backends

Tried in order; the first one configured and answering wins.

| Backend | Environment variable |
|---|---|
| Brave Search | `BRAVE_SEARCH_API_KEY` or `BRAVE_API_KEY` |
| Tavily | `TAVILY_API_KEY` |
| Serper | `SERPER_API_KEY` |
| SearXNG | `SEARXNG_URL` (needs `json` in the instance's `search.formats`) |
| DuckDuckGo | none — keyless fallback |

Configure one of the first four for dependable results. DuckDuckGo answers an
HTML scrape intermittently and otherwise returns a bot challenge, which the
tool reports as an error naming these variables rather than as an empty result.

## Limits

Both tools refuse anything that is not a public HTTP(S) URL — loopback,
RFC 1918, link-local, `.internal`, and `.local` are rejected on every redirect
hop, not just the first. A configured SearXNG address is the one exception,
because the operator chose it. `web_fetch` reads text, HTML, JSON, and XML;
binary responses are refused. It returns 20,000 characters by default and says
what offset to pass to continue.
