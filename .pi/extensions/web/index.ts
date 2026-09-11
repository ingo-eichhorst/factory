/**
 * Web access for pi in this project: `web_search` to find pages and
 * `web_fetch` to read one. Both refuse private-network addresses, so neither
 * can be turned on the local daemon or a metadata endpoint.
 */

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { registerWebFetch } from "./fetch.ts";
import { registerWebSearch } from "./search.ts";

export default function (pi: ExtensionAPI) {
  registerWebSearch(pi);
  registerWebFetch(pi);
}
