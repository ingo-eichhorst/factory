/**
 * Shared HTTP helpers for the web tools.
 *
 * Every outbound request is funnelled through `fetchPublicUrl` so the guard
 * against private-network targets holds for redirects as well as the first
 * hop. An agent that can fetch a URL of its own choosing is a confused deputy
 * unless something keeps it off the loopback and RFC 1918 ranges.
 */

const REDIRECT_LIMIT = 5;
const REQUEST_TIMEOUT_MS = 20_000;

/**
 * An honest user agent. A browser string was tried and changed nothing: across
 * example.com, GitHub, docs.rs, Wikipedia, Stack Overflow, and Reuters the two
 * drew identical status codes, and DuckDuckGo served its challenge to the
 * browser string while answering this one. Nothing is bought by pretending.
 */
export const USER_AGENT = "Mozilla/5.0 (compatible; pi-web-tools/1.0; +local agent)";

const NAMED_ENTITIES: Record<string, string> = {
  nbsp: " ",
  amp: "&",
  quot: '"',
  apos: "'",
  lt: "<",
  gt: ">",
  hellip: "…",
  mdash: "—",
  ndash: "–",
  rsquo: "’",
  lsquo: "‘",
  ldquo: "“",
  rdquo: "”",
};

function decodeEntities(value: string): string {
  return value
    .replace(/&#x([0-9a-f]+);/gi, (_match, hex: string) => String.fromCodePoint(Number.parseInt(hex, 16)))
    .replace(/&#(\d+);/g, (_match, digits: string) => String.fromCodePoint(Number.parseInt(digits, 10)))
    .replace(/&([a-z]+);/gi, (match, name: string) => NAMED_ENTITIES[name.toLowerCase()] ?? match);
}

/** Collapse a fragment of HTML to a single line of readable text. */
export function decodeHtml(value: string): string {
  return decodeEntities(value.replace(/<[^>]*>/g, " "))
    .replace(/\s+/g, " ")
    .trim();
}

/** Turn a full HTML document into readable text, keeping paragraph breaks. */
export function htmlToText(html: string): string {
  const stripped = html
    .replace(/<(script|style|noscript|template|svg)(?=[\s>])[\s\S]*?<\/\1>/gi, " ")
    .replace(/<!--[\s\S]*?-->/g, " ")
    .replace(/<\/(p|div|section|article|li|tr|h[1-6]|title|blockquote|pre|ul|ol|table|header|footer|figcaption)>/gi, "\n\n")
    .replace(/<br\s*\/?>/gi, "\n")
    .replace(/<li(?=[\s>])[^>]*>/gi, "\n- ");
  return decodeEntities(stripped.replace(/<[^>]*>/g, " "))
    .replace(/[ \t\f\v ]+/g, " ")
    .replace(/ ?\n ?/g, "\n")
    .replace(/\n{3,}/g, "\n\n")
    .trim();
}

/** Reject anything that is not a public HTTP(S) endpoint. */
export function isPublicWebUrl(value: string): boolean {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    return false;
  }
  if (url.protocol !== "https:" && url.protocol !== "http:") return false;
  const host = url.hostname.toLowerCase().replace(/^\[|\]$/g, "");
  if (host === "localhost" || host.endsWith(".localhost") || host === "::1" || host === "0.0.0.0") return false;
  if (host.endsWith(".local") || host.endsWith(".internal")) return false;
  if (/^127\./.test(host) || /^0\./.test(host) || /^10\./.test(host) || /^192\.168\./.test(host)) return false;
  if (/^172\.(1[6-9]|2\d|3[01])\./.test(host) || /^169\.254\./.test(host)) return false;
  if (/^(fc|fd)[0-9a-f]{2}:/i.test(host) || /^fe80:/i.test(host)) return false;
  return true;
}

function requestSignal(signal?: AbortSignal): AbortSignal {
  const timeout = AbortSignal.timeout(REQUEST_TIMEOUT_MS);
  return signal ? AbortSignal.any([signal, timeout]) : timeout;
}

export interface FetchOptions {
  signal?: AbortSignal;
  method?: string;
  headers?: Record<string, string>;
  body?: string;
  /**
   * Allow a private-network host. Only for endpoints the operator configured
   * themselves, such as a self-hosted SearXNG on localhost; never for a URL
   * that came from the model or from a page it read.
   */
  allowPrivateHost?: boolean;
}

/** Fetch a URL, following redirects by hand so each hop is checked. */
export async function fetchPublicUrl(
  input: string,
  options: FetchOptions = {},
): Promise<{ url: string; response: Response }> {
  let url = input;
  for (let redirects = 0; redirects <= REDIRECT_LIMIT; redirects += 1) {
    if (!options.allowPrivateHost && !isPublicWebUrl(url)) {
      throw new Error(`Only public HTTP(S) URLs are allowed: ${url}`);
    }
    const response = await fetch(url, {
      method: options.method ?? "GET",
      body: options.body,
      redirect: "manual",
      headers: { "User-Agent": USER_AGENT, ...options.headers },
      signal: requestSignal(options.signal),
    });
    if (response.status >= 300 && response.status < 400) {
      const location = response.headers.get("location");
      if (!location) throw new Error("Redirect response did not include a location.");
      url = new URL(location, url).toString();
      continue;
    }
    return { url, response };
  }
  throw new Error(`Too many redirects while fetching ${input}.`);
}
