# Tokens

What a run needs, what each permission is for, how to make the tokens, where to
keep them and what to do when one is wrong.

A run uses three credentials, and each is held by as little as possible:

| Credential | Used by | Held by |
|---|---|---|
| Your `gh` login | `heph workload` and `heph run`, to read the repository, the ticket and who you are | `gh` |
| A GitHub token made for Hephaestium | the executor's delivery step: the push and the pull request | the executor, at that step only |
| A Claude Code token | the Claude Code agent | the agent, and nothing else |

Neither token is your login. Each can be revoked on its own, without signing you
out of anything.

## Your `gh` login

`heph workload` and `heph run` read GitHub with `gh api`: the repository, its
default branch and the tip of it, the ticket, and the signed-in user, who is named
as the approver. They need `gh auth login` and read access to the repository. They
change nothing, and the token below is not used for any of it.

## The GitHub token

The executor uses it when a workload's gates have passed, to push the work to a
branch and open the pull request. Before that point, and after it, it is not in
the environment of anything the executor runs: the bootstrap steps, the gates and
the agent run the repository's code and the agent's changes to it, so none of
them can read it.

Make a **fine-grained personal access token** (GitHub, Settings, Developer
settings, Personal access tokens, Fine-grained tokens):

| Setting | Value | Why |
|---|---|---|
| Resource owner | the account or organization that owns the repository | a token belongs to one owner |
| Repository access | **Only select repositories**, and only the ones Hephaestium manages | so that a leaked token can write to those and nothing else |
| Expiration | 90 days or less | a token that is forgotten expires |
| Repository permission: **Contents** | Read and write | to push the branch |
| Repository permission: **Pull requests** | Read and write | to open the pull request |
| Repository permission: **Metadata** | Read-only | GitHub adds it, and everything else needs it |
| Repository permission: **Workflows** | Read and write | only for a change that touches `.github/workflows/` (see below) |

Leave everything else at No access. In particular it needs no access to issues
(the ticket is read with your own login), to Actions, to secrets, to
administration or to anything outside the selected repositories.

### The Workflows permission

GitHub refuses to let any token without it create or change a workflow file. A
run that edits one does all its work, passes its gates, and is then refused at the
push with:

```
refusing to allow a Personal Access Token to create or update workflow
`.github/workflows/<file>.yml` without `workflow` scope
```

The commit is intact in the run's workspace, and nothing is lost, but nothing is
delivered.

Granting it has a cost that should be a decision and not a default. A repository's
workflows run with its secrets, so a token that can edit them lets an agent, or
anything the agent was persuaded to do by what it read in a ticket or a file,
change what runs with those secrets. Hephaestium's first version runs only
against tickets and repositories the maintainer trusts, and the maintainer reviews
every pull request before it merges; the permission is for that situation. Leave
it off for a repository whose workflows you do not want an agent to be able to
change, and make such changes by hand.

### Making it

1. Open GitHub, then Settings, Developer settings, Personal access tokens,
   Fine-grained tokens, and choose Generate new token.
2. Fill in the table above. Copy the token when it is shown: GitHub shows it once.
3. Put it where the executor will find it (below).

Editing a token's permissions later keeps the same token, so a file that holds it
does not need to change.

### Checking it, without printing it

Reading the repository should print its name. Reading a repository the token was
not given should fail with Not Found, which shows it is limited to the ones
selected:

```
GH_TOKEN=$(cat ~/.config/hephaestium/github-token) gh api repos/OWNER/NAME --jq .full_name
GH_TOKEN=$(cat ~/.config/hephaestium/github-token) gh api repos/OWNER/OTHER --jq .full_name
```

### If it leaks or is lost

Revoke it in GitHub's settings and make another. It can write to the repositories
it was given, and to their workflows if it has that permission, until it is
revoked or expires, and no further.

## The Claude Code token

A Claude Code agent authenticates with a long-lived token that belongs to your
subscription and is separate from your login. Make it with:

```
claude setup-token
```

It opens a browser to sign in and prints the token once. Run it in a terminal of
your own and not through a tool that records output. It can be revoked from your
Claude account settings. Only the agent receives it, as `CLAUDE_CODE_OAUTH_TOKEN`:
the run gives the agent an empty configuration directory of its own, so nothing of
your own Claude setup (settings, hooks, memory) reaches it.

To check that the token works with an empty configuration directory, as a run
uses it (this costs a fraction of a cent):

```
CLAUDE_CONFIG_DIR=$(mktemp -d) CLAUDE_CODE_OAUTH_TOKEN=$(cat ~/.config/hephaestium/claude-token) \
  claude -p "Reply with the single word: ok" --model haiku --max-budget-usd 0.05
```

## Where to keep them

A token is the variable of its name or the contents of the file its `_FILE`
variable names. Keep each in a file outside every repository, readable only by you:

```
mkdir -p ~/.config/hephaestium && chmod 700 ~/.config/hephaestium
# copy the token, then:
( umask 077; pbpaste | tr -d '\n' > ~/.config/hephaestium/github-token )
( umask 077; pbpaste | tr -d '\n' > ~/.config/hephaestium/claude-token )
pbcopy < /dev/null
chmod 600 ~/.config/hephaestium/*
```

| Token | Variable | File variable | Default file |
|---|---|---|---|
| GitHub | `HEPH_GITHUB_TOKEN` | `HEPH_GITHUB_TOKEN_FILE` | `~/.config/hephaestium/github-token` |
| Claude Code | `CLAUDE_CODE_OAUTH_TOKEN` | `CLAUDE_CODE_OAUTH_TOKEN_FILE` | `~/.config/hephaestium/claude-token` |

- `heph run` uses what is already set, and otherwise offers the default file to the
  executor. It never overrides a variable or a `_FILE` variable you have set.
  `$XDG_CONFIG_HOME` is honoured in place of `~/.config`.
- Giving both a variable and a file for one token is an error, as is a file that
  the group or others can read, one that is empty, or one that is not a regular
  file. The message says `chmod 600`, and never prints the value.
- The executor reads each token once, at the start, and removes both the variable
  and its `_FILE` pointer from its own environment.

## Secrets a repository's gates need

A gate in a repository's manifest can declare a secret it needs (`NAME = "secret"`
under `[gates.<gate>.requires]`). Give it to the executor as
`HEPH_GATE_SECRET_<NAME>`, or as `HEPH_GATE_SECRET_<NAME>_FILE` naming a file kept
as above, and it is given to the gates that declare it and to nothing else:

```
( umask 077; pbpaste | tr -d '\n' > ~/.config/hephaestium/pixabay-key )
HEPH_GATE_SECRET_PIXABAY_API_KEY_FILE=~/.config/hephaestium/pixabay-key
```

The same rules apply as for the tokens (a file only you can read, not both a
variable and a file), and the executor removes both from its environment, so a
bootstrap step, another gate and the agent never see the value or where it is
kept. A variable the gate declares as `env` is read from the executor's own
environment, and a control plane started with it set passes it on. These values are
the executor's, not a repository's: every repository the control plane takes in
sees the same ones. See [ADR 0027](adr/0027-a-gate-gets-the-secrets-it-declares-and-the-agent-never-does.md).

## When one is wrong

| What you see | What it means | What to do |
|---|---|---|
| `no HEPH_GITHUB_TOKEN: set HEPH_GITHUB_TOKEN, or HEPH_GITHUB_TOKEN_FILE …` | `heph run` found no GitHub token anywhere and stopped before making anything | make one, and put it in the default file or give its variable |
| `… can be read by others (mode 644); run: chmod 600 …` | the file is too open | `chmod 600` it |
| `both set: give one` | a variable and a file were both given for one token | unset one |
| a push refused with `without workflow scope` | the change touches a workflow file and the token lacks Workflows | see above: add the permission, or make that change by hand |
| a push or `gh pr create` refused by GitHub for any other reason (its wording varies, for example `Resource not accessible by personal access token`, `Permission denied` or `Repository not found`) | the token lacks Contents or Pull requests write, or the repository was not selected, or it has expired | edit the token's permissions or repositories, or make a new one |
| a run fails at once with an authentication error from Claude Code | the Claude Code token is missing, wrong or revoked | make another with `claude setup-token` |

A run that is refused at the push keeps its work in the run's workspace, under
`~/.local/state/hephaestium/runs/`, with the commit made. Once the token is
corrected, push that branch from the workspace and open the pull request.
