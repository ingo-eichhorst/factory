/**
 * Fetch one public page and hand back readable text.
 *
 * The size cap exists because a fetched page goes straight into the context
 * window; `offset` lets a later call walk further into a long document instead
 * of losing everything past the cap.
 */

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Type } from "@earendil-works/pi-ai";
import { fetchPublicUrl, htmlToText } from "./http.ts";

const DEFAULT_MAX_CHARS = 20_000;
const HARD_MAX_CHARS = 100_000;

const READABLE_TYPES =
  /text\/|application\/(json|xml|xhtml\+xml|rss\+xml|atom\+xml|javascript|x-ndjson)|\+json|\+xml/i;

export function registerWebFetch(pi: ExtensionAPI): void {
  pi.registerTool({
    name: "web_fetch",
    label: "Fetch a web page",
    description:
      "Fetch a public HTTP(S) URL and return its readable text. HTML is reduced to text; " +
      "JSON, XML, and plain text come back as sent. Binary responses are refused, and so are " +
      "localhost and private-network addresses.",
    promptSnippet: "Read a public web page or API response as text",
    promptGuidelines: [
      "Use web_fetch to read a page found with web_search, or any URL the user gives, instead of guessing its contents.",
      "When a web_fetch result says it was truncated, call web_fetch again with offset set to continue from that point.",
    ],
    parameters: Type.Object({
      url: Type.String({ description: "Public HTTP(S) URL" }),
      max_chars: Type.Optional(
        Type.Integer({
          minimum: 500,
          maximum: HARD_MAX_CHARS,
          default: DEFAULT_MAX_CHARS,
          description: `Characters to return (default ${DEFAULT_MAX_CHARS})`,
        }),
      ),
      offset: Type.Optional(
        Type.Integer({ minimum: 0, default: 0, description: "Character offset to start from, for continuing a truncated read" }),
      ),
      raw: Type.Optional(
        Type.Boolean({ default: false, description: "Return the response body unprocessed instead of extracting text" }),
      ),
    }),
    async execute(_toolCallId, params, signal) {
      const target = params.url.trim().replace(/^@/, "");
      const limit = Math.min(params.max_chars ?? DEFAULT_MAX_CHARS, HARD_MAX_CHARS);
      const offset = params.offset ?? 0;

      const { url, response } = await fetchPublicUrl(target, {
        signal,
        headers: { Accept: "text/html,application/xhtml+xml,application/json;q=0.9,text/plain;q=0.8,*/*;q=0.5" },
      });
      if (!response.ok) throw new Error(`Fetch failed with HTTP ${response.status} ${response.statusText} for ${url}.`);

      const contentType = response.headers.get("content-type") ?? "";
      if (contentType && !READABLE_TYPES.test(contentType)) {
        throw new Error(`Unsupported content type for ${url}: ${contentType}. web_fetch reads text, HTML, JSON, and XML only.`);
      }

      const body = await response.text();
      const isHtml = /html/i.test(contentType) || /^\s*<(!doctype|html)/i.test(body);
      const text = params.raw || !isHtml ? body : htmlToText(body);

      const slice = text.slice(offset, offset + limit);
      const truncated = offset + slice.length < text.length;
      const header = [
        `Source: ${url}`,
        contentType ? `Content-Type: ${contentType}` : "",
        offset > 0 ? `Offset: ${offset}` : "",
        truncated ? `Truncated: ${offset + slice.length} of ${text.length} characters; call again with offset ${offset + slice.length}` : "",
      ]
        .filter(Boolean)
        .join("\n");

      if (slice.length === 0) {
        return {
          content: [{ type: "text", text: `${header}\n\nNo text at offset ${offset}; the document is ${text.length} characters.` }],
          details: { url, contentType, length: text.length, offset, truncated: false },
        };
      }

      return {
        content: [{ type: "text", text: `${header}\n\n${slice}` }],
        details: { url, contentType, length: text.length, offset, truncated },
      };
    },
  });
}
