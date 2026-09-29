export type PullRequestLink = { label: string; href: string };

const GITHUB_PR = /^https:\/\/github\.com\/([^/\s]+)\/([^/\s]+)\/pull\/(\d+)(?:[/?#].*)?$/;
const GITLAB_MR = /^https:\/\/([^/\s]+)\/(.+?)\/-\/merge_requests\/(\d+)(?:[/?#].*)?$/;

/** Recognise a pull/merge request URL so it renders as a link card. */
export function detectPullRequest(uri: string | undefined): PullRequestLink | undefined {
  if (!uri) return undefined;
  const gh = GITHUB_PR.exec(uri);
  if (gh) return { label: `${gh[1]}/${gh[2]}#${gh[3]}`, href: uri };
  const gl = GITLAB_MR.exec(uri);
  if (gl) return { label: `${gl[2]}!${gl[3]}`, href: uri };
  return undefined;
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
