/**
 * Web search with a backend chain.
 *
 * Keyless scraping of a search front-end is no longer dependable: DuckDuckGo
 * answers a few requests and then serves a bot challenge instead of results.
 * So a configured search API is tried first and the scrape is only the last
 * resort, with an error that says what to configure when it is refused.
 */

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Type } from "@earendil-works/pi-ai";
import { decodeHtml, fetchPublicUrl } from "./http.ts";

const DEFAULT_RESULTS = 5;
const MAX_RESULTS = 10;

export interface SearchResult {
  title: string;
  url: string;
  snippet: string;
}

interface Backend {
  name: string;
  enabled(): boolean;
  run(query: string, limit: number, signal?: AbortSignal): Promise<SearchResult[]>;
}

function env(...names: string[]): string | undefined {
  for (const name of names) {
    const value = process.env[name]?.trim();
    if (value) return value;
  }
  return undefined;
}

function clean(value: unknown): string {
  return typeof value === "string" ? decodeHtml(value) : "";
}

async function readJson(response: Response, backend: string): Promise<unknown> {
  if (!response.ok) {
    const detail = (await response.text()).slice(0, 300).trim();
    throw new Error(`${backend} returned HTTP ${response.status}${detail ? `: ${detail}` : "."}`);
  }
  return response.json();
}

const brave: Backend = {
  name: "brave",
  enabled: () => Boolean(env("BRAVE_SEARCH_API_KEY", "BRAVE_API_KEY")),
  async run(query, limit, signal) {
    const { response } = await fetchPublicUrl(
      `https://api.search.brave.com/res/v1/web/search?q=${encodeURIComponent(query)}&count=${limit}`,
      {
        signal,
        headers: {
          Accept: "application/json",
          "X-Subscription-Token": env("BRAVE_SEARCH_API_KEY", "BRAVE_API_KEY")!,
        },
      },
    );
    const payload = (await readJson(response, "Brave Search")) as {
      web?: { results?: Array<{ title?: string; url?: string; description?: string }> };
    };
    return (payload.web?.results ?? []).slice(0, limit).map((item) => ({
      title: clean(item.title),
      url: item.url ?? "",
      snippet: clean(item.description),
    }));
  },
};

const tavily: Backend = {
  name: "tavily",
  enabled: () => Boolean(env("TAVILY_API_KEY")),
  async run(query, limit, signal) {
    const key = env("TAVILY_API_KEY")!;
    const { response } = await fetchPublicUrl("https://api.tavily.com/search", {
      signal,
      method: "POST",
      headers: { "Content-Type": "application/json", Authorization: `Bearer ${key}` },
      body: JSON.stringify({ api_key: key, query, max_results: limit }),
    });
    const payload = (await readJson(response, "Tavily")) as {
      results?: Array<{ title?: string; url?: string; content?: string }>;
    };
    return (payload.results ?? []).slice(0, limit).map((item) => ({
      title: clean(item.title),
      url: item.url ?? "",
      snippet: clean(item.content),
    }));
  },
};

const serper: Backend = {
  name: "serper",
  enabled: () => Boolean(env("SERPER_API_KEY")),
  async run(query, limit, signal) {
    const { response } = await fetchPublicUrl("https://google.serper.dev/search", {
      signal,
      method: "POST",
      headers: { "Content-Type": "application/json", "X-API-KEY": env("SERPER_API_KEY")! },
      body: JSON.stringify({ q: query, num: limit }),
    });
    const payload = (await readJson(response, "Serper")) as {
      organic?: Array<{ title?: string; link?: string; snippet?: string }>;
    };
    return (payload.organic ?? []).slice(0, limit).map((item) => ({
      title: clean(item.title),
      url: item.link ?? "",
      snippet: clean(item.snippet),
    }));
  },
};

/**
 * A self-hosted SearXNG. The operator chose this address, so a private host is
 * allowed here and nowhere else. The instance needs `json` in its
 * `search.formats` setting.
 */
const searxng: Backend = {
  name: "searxng",
  enabled: () => Boolean(env("SEARXNG_URL", "SEARX_URL")),
  async run(query, limit, signal) {
    const base = env("SEARXNG_URL", "SEARX_URL")!.replace(/\/+$/, "");
    const { response } = await fetchPublicUrl(
      `${base}/search?q=${encodeURIComponent(query)}&format=json`,
      { signal, headers: { Accept: "application/json" }, allowPrivateHost: true },
    );
    const payload = (await readJson(response, "SearXNG")) as {
      results?: Array<{ title?: string; url?: string; content?: string }>;
    };
    return (payload.results ?? []).slice(0, limit).map((item) => ({
      title: clean(item.title),
      url: item.url ?? "",
      snippet: clean(item.content),
    }));
  },
};

/** Unwrap DuckDuckGo's `/l/?uddg=` click-tracking redirect. */
function resultUrl(value: string): string {
  const decoded = decodeHtml(value).replace(/^\/\//, "https://");
  try {
    const url = new URL(decoded);
    if (url.hostname.endsWith("duckduckgo.com") && url.searchParams.has("uddg")) {
      return url.searchParams.get("uddg")!;
    }
  } catch {
    // Keep a malformed href as text rather than failing the whole search.
  }
  return decoded;
}

const duckduckgo: Backend = {
  name: "duckduckgo",
  enabled: () => true,
  async run(query, limit, signal) {
    const { response } = await fetchPublicUrl(
      `https://html.duckduckgo.com/html/?q=${encodeURIComponent(query)}`,
      { signal, headers: { Accept: "text/html", "Accept-Language": "en-US,en;q=0.9" } },
    );
    if (!response.ok && response.status !== 202) {
      throw new Error(`DuckDuckGo returned HTTP ${response.status}.`);
    }
    const html = await response.text();
    if (/anomaly|challenge|captcha/i.test(html) && !/result__a/.test(html)) {
      throw new Error(
        "DuckDuckGo served a bot challenge instead of results. Set BRAVE_SEARCH_API_KEY, " +
          "TAVILY_API_KEY, SERPER_API_KEY, or SEARXNG_URL for a dependable backend.",
      );
    }
    const blocks = html.split(/<div[^>]*class="[^"]*\bresult\b[^"]*"/).slice(1);
    const results: SearchResult[] = [];
    for (const block of blocks) {
      const link = /<a[^>]*class="[^"]*result__a[^"]*"[^>]*href="([^"]+)"[^>]*>([\s\S]*?)<\/a>/.exec(block);
      if (!link) continue;
      const snippet = /class="[^"]*result__snippet[^"]*"[^>]*>([\s\S]*?)<\/a>/.exec(block);
      results.push({
        title: decodeHtml(link[2]),
        url: resultUrl(link[1]),
        snippet: snippet ? decodeHtml(snippet[1]) : "",
      });
      if (results.length >= limit) break;
    }
    return results;
  },
};

const BACKENDS: Backend[] = [brave, tavily, serper, searxng, duckduckgo];

export function registerWebSearch(pi: ExtensionAPI): void {
  pi.registerTool({
    name: "web_search",
    label: "Search the web",
    description:
      "Search the public web and return titles, URLs, and snippets. Use it for anything that " +
      "may have changed since training, then read the promising results with web_fetch. " +
      "Uses Brave, Tavily, Serper, or a SearXNG instance when one is configured by environment " +
      "variable, and falls back to DuckDuckGo, which sometimes answers with a bot challenge.",
    promptSnippet: "Search the public web for current information",
    promptGuidelines: [
      "Use web_search before answering a question about current versions, APIs, prices, or events, rather than relying on memory.",
      "Follow a web_search result with web_fetch on the URL when the snippet is not enough to answer.",
    ],
    parameters: Type.Object({
      query: Type.String({ minLength: 1, description: "Search query" }),
      max_results: Type.Optional(
        Type.Integer({
          minimum: 1,
          maximum: MAX_RESULTS,
          default: DEFAULT_RESULTS,
          description: `Results to return (1-${MAX_RESULTS}, default ${DEFAULT_RESULTS})`,
        }),
      ),
    }),
    async execute(_toolCallId, params, signal) {
      const limit = Math.min(params.max_results ?? DEFAULT_RESULTS, MAX_RESULTS);
      const failures: string[] = [];

      for (const backend of BACKENDS.filter((candidate) => candidate.enabled())) {
        if (signal?.aborted) throw new Error("Search cancelled.");
        try {
          const results = await backend.run(params.query, limit, signal);
          if (results.length === 0) {
            failures.push(`${backend.name}: no results`);
            continue;
          }
          const text = results
            .map((item, index) =>
              [`${index + 1}. ${item.title || item.url}`, `   ${item.url}`, item.snippet ? `   ${item.snippet}` : ""]
                .filter(Boolean)
                .join("\n"),
            )
            .join("\n\n");
          return {
            content: [{ type: "text", text: `Search: ${params.query} (via ${backend.name})\n\n${text}` }],
            details: { backend: backend.name, query: params.query, results },
          };
        } catch (error) {
          failures.push(`${backend.name}: ${error instanceof Error ? error.message : String(error)}`);
        }
      }

      throw new Error(`No search backend returned results.\n${failures.map((line) => `- ${line}`).join("\n")}`);
    },
  });
}
