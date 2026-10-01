import { safeHttpUrl } from "./a2ui/url";

export type PullRequestLink = { label: string; href: string };

/** Where a pull request URL points, as far as its own text says (never the agent's payload). */
export type PullRequestLocation = {
  /** The host the link opens (lower case): what a card shows when nothing else is known. */
  host: string;
  /** `host/owner/name` (lower case), when the path names one before its marker. */
  repository?: string;
  number?: number;
  /**
   * `owner/name#12` on github.com, `group/project!7` on gitlab.com, else with the host in front
   * (`evil.example/acme/demo#9`), so the label never names a place the link does not go.
   */
  label?: string;
};

const MARKERS = ["/-/merge_requests/", "/merge_requests/", "/pulls/", "/pull/"] as const;
/** Hosts whose pull requests a label may name without the host. */
const WELL_KNOWN = new Set(["github.com", "gitlab.com"]);

/**
 * Reads a pull request URL: its host always, and the repository and number when its path is
 * `<owner/…/name>/pull/<n>` (or `/pulls/`, `/-/merge_requests/`, `/merge_requests/`). Only a URL
 * `safeHttpUrl` accepts (no user information, no backslash) is read.
 */
export function locatePullRequest(href: string): PullRequestLocation | undefined {
  const safe = safeHttpUrl(href);
  if (!safe) return undefined;
  const url = new URL(safe);
  const host = url.host.toLowerCase();
  for (const marker of MARKERS) {
    const at = url.pathname.lastIndexOf(marker);
    if (at < 0) continue;
    const digits = url.pathname.slice(at + marker.length).split("/")[0] ?? "";
    const repo = url.pathname
      .slice(0, at)
      .split("/")
      .filter((s) => s !== "")
      .join("/")
      .replace(/\.git$/, "");
    if (!/^\d+$/.test(digits) || repo.split("/").length < 2) return { host };
    const sep = marker.includes("merge_requests") ? "!" : "#";
    const where = WELL_KNOWN.has(host) ? repo : `${host}/${repo}`;
    return {
      host,
      repository: `${host}/${repo}`.toLowerCase(),
      number: Number(digits),
      label: `${where}${sep}${digits}`,
    };
  }
  return { host };
}

const GITHUB_PR = /^https:\/\/github\.com\/[^/\s]+\/[^/\s]+\/pull\/\d+(?:[/?#].*)?$/;
const GITLAB_MR = /^https:\/\/[^/\s]+\/.+?\/-\/merge_requests\/\d+(?:[/?#].*)?$/;

/**
 * Recognise a GitHub pull or GitLab merge request URL (of a file artifact) so it renders as a
 * card; its label comes from the URL (`locatePullRequest`), with the host unless it is
 * github.com or gitlab.com.
 */
export function detectPullRequest(uri: string | undefined): PullRequestLink | undefined {
  if (!uri || !(GITHUB_PR.test(uri) || GITLAB_MR.test(uri))) return undefined;
  const label = locatePullRequest(uri)?.label;
  return label ? { label, href: uri } : undefined;
}

/**
 * The URI as a link target, only when it is an absolute `http:` or `https:` URL.
 * Artifact URIs come from agents, so anything else (`javascript:`, `data:`, relative
 * paths) must never become a clickable `href`.
 */
export function safeLinkHref(uri: string | undefined): string | undefined {
  if (!uri) return undefined;
  let url: URL;
  try {
    url = new URL(uri);
  } catch {
    return undefined;
  }
  return url.protocol === "https:" || url.protocol === "http:" ? url.href : undefined;
}
