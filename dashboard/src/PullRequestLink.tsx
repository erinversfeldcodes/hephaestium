import type { PullRequest } from "./api";

/**
 * A link to a ticket's pull request: its number, with a small dot for how its checks stand
 * (green passing, red failing, grey pending or none). A ticket with no pull request has nothing
 * here, not a placeholder. The state is what the control plane last recorded, so showing it
 * never waits on the code host.
 */
export function PullRequestLink({ pullRequest }: { readonly pullRequest: PullRequest | null }) {
  if (pullRequest === null) return null;
  return (
    <a className="pull-request" href={pullRequest.url} rel="noreferrer noopener">
      <span
        className="checks-dot"
        data-checks={pullRequest.checks}
        role="img"
        aria-label={`checks ${pullRequest.checks}, pull request ${pullRequest.state}`}
      />
      {`#${pullRequest.number}`}
    </a>
  );
}
