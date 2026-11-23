# Running the control plane on Fly with Neon

How the hosted control plane is deployed, checked, rolled back and fixed. The decision
behind it is [ADR 0037](adr/0037-a-hosted-control-plane-serves-executors-that-run-on-the-developers-machines.md);
what the control plane does with webhooks is
[ADR 0039](adr/0039-the-control-plane-learns-from-webhooks-and-polls-only-as-a-backstop.md); its
settings are in [the control plane's README](../control-plane/README.md).

Each step says who does it. Steps marked **you** create an account, change the GitHub App or
hold a secret, and nobody else can do them. Everything else runs in CI.

Anything in this document that was inferred rather than read from the code or observed is
marked *(inferred)* or *(unverified)*. The first live run has been made; points it did not exercise are still marked.

## What is deployed

| Environment | Fly app | Neon | Made by |
| --- | --- | --- | --- |
| production | `hephaestium-control-plane` | project `hephaestium` (`damp-bar-96968173`), default branch `production` | `deploy-production.yml`, from `main` |
| preview, one per pull request | `heph-cp-pr-<branch>` | a branch `preview/<branch>` of project `hephaestium-staging` (`cool-mode-78714280`), made from its `production` branch | CI, deleted when the pull request closes |

Both Fly apps are in the Fly organisation the other projects use (`personal`, by what
thestacks's scripts pass to Fly), region `fra`, matching Neon's `aws-eu-central-1`. The
production app name is the one already in `control-plane/fly.toml`; keep it. Neon is Postgres 18
and the CI tests run Postgres 16, see [the compatibility risk](#compatibility-risk-postgres-16-in-ci-18-on-neon).

No connection string is ever typed or pasted. CI asks the Neon API for the pooled and the direct
string of the branch it is deploying and gives them to Fly with `fly secrets set`, on every
deploy. To change any secret, change the GitHub Actions secret and deploy again.

## How the control plane reaches GitHub (read this first)

From the code, not from the documents:

- **Signing in** uses the App's client id and client secret only (`ControlPlane.Api.GitHubLogin.Http`). It
  exchanges the code for a user token and reads `GET /user`. No private key is read.
- **Everything else the control plane does on GitHub** (intake, settling, merging, comments,
  checks, the collaborator-permission check a reply makes) runs `gh api` as a child process
  (`ControlPlane.GitHub`). `gh` takes its identity from `GH_TOKEN` in the environment. The
  control plane has no code that mints an installation token for these calls. On the laptop the
  identity is the signed-in `gh`.
- **The review App** (`HEPH_REVIEW_APP_ID`, `HEPH_REVIEW_APP_KEY`) is the one place an App's key is
  used: it mints a per-repository installation token, for approving a pull request only.
- **A reply** (comment and ready label) is made with the signed-in person's user token, passed
  to `gh` as `GH_TOKEN` for that one request. It also needs the control plane's own identity,
  because the permission check (`collaborators/<login>/permission`) is made as the control plane.
- **`heph`** is run by intake to build a workload (`heph workload`), and it reads GitHub with the
  same `gh`.

What the hosted skeleton therefore has, as of this change:

| Needed | State |
| --- | --- |
| `gh` in the image | In the image: GitHub's own release, pinned by version and checked against its checksum (`GH_VERSION` in `control-plane/Dockerfile`). Debian's package is not used, because its `gh` lacks `--slurp`. |
| An identity for `gh` | **Only a token in `GH_TOKEN`.** An installation token lasts about an hour, so the deployed value has to be a fine-grained personal access token, which is a person's identity, not an App's. This differs from what ADR 0037 recommends (an App that mints short-lived tokens). Use a dedicated machine account, not your own, so that the labels and comments the control plane writes are not yours. A label the control plane applies as you would be read by intake as your approval. |
| `heph` in the image | In the image at `/usr/local/bin/heph`, and `HEPH_BIN` points to it. Intake builds every workload with `heph workload`. |

The identity for `gh` is still a finding, not fixed: a token source for every `gh` call (an App
key, a per-repository installation token, an App decided on, ADR 0037's open question about
which identity the control plane writes as). Until it exists the deployed `GH_TOKEN` is a
fine-grained personal access token.

## Prerequisites (you)

1. **Accounts**: the Fly organisation (`personal`), the Neon organisation
   `org-purple-pond-98438335`, both already yours. GitHub: owner of the App and of the repository.
2. **Neon**: the projects `hephaestium` and `hephaestium-staging` exist (Postgres 18,
   `aws-eu-central-1`). Make one API key per project (Neon console, Account settings, API keys) and
   keep it for step 5. Nothing else is done by hand in Neon.
3. **Fly**: make a token that can create and destroy apps in the organisation:
   `fly tokens create org --org personal --expiry 8760h`. Keep it for step 5. *(unverified: the
   flag spelling is from Fly's CLI help as remembered; `fly tokens create --help` is
   authoritative.)* thestacks and gazebolang use the same kind of token.
4. **The production Fly app**: `fly apps create hephaestium-control-plane --org personal`.
   Previews are created by CI; production is created once, by you, so that a typo in a workflow
   cannot create it under another name.
5. **GitHub Actions secrets**, in the repository's settings. Names follow thestacks's where one
   exists.

   | Secret | Scope | What it is |
   | --- | --- | --- |
   | `FLY_API_TOKEN` | repository | from step 3 |
   | `NEON_STAGING_API_KEY`, `NEON_STAGING_PROJECT_ID` | repository | the staging project's key and `cool-mode-78714280` |
   | `NEON_PRODUCTION_API_KEY`, `NEON_PRODUCTION_PROJECT_ID` | environment `production` | the production project's key and `damp-bar-96968173` |
   | `HEPH_API_SECRET_KEY_BASE` | one per environment (`production`, `preview`) | `openssl rand -base64 48`. Different in each, so a session from one is no good in the other. |
   | `HEPH_API_GITHUB_CLIENT_ID`, `HEPH_API_GITHUB_CLIENT_SECRET` | per environment | the App's, see below. A preview needs them because the API refuses to start without them. |
   | `HEPH_WEBHOOK_SECRET` | `production` only | `openssl rand -hex 32`. The same value goes into the App. |
   | `CONTROL_PLANE_GH_TOKEN` | `production` only | the machine account's fine-grained token, see above. Set on Fly as `GH_TOKEN`. |
   | `HEPH_REVIEW_APP_KEY` | `production`, optional | the review App's private key, only if it is to approve from the hosted control plane. Set with `HEPH_REVIEW_APP_ID` (a variable, below). Never printed. |

   Variables (not secrets), environment `production`: `HEPH_API_ALLOWED_LOGINS` (the logins that may
   sign in, comma-separated) and `HEPH_REPOSITORIES` (`erinversfeldcodes/hephaestium`, and others
   comma-separated).

   Variables, environment `production`, each optional and passed to the control plane only when
   set, so a deployment that leaves one out behaves as it did without it:

   | Variable | What it sets |
   | --- | --- |
   | `HEPH_REVIEW_APP_ID` | the review App's id; needs `HEPH_REVIEW_APP_KEY` too |
   | `HEPH_ATHENAEUM_REF` | the Athenaeum release lenses and skills are read from |
   | `HEPH_REVISIONS` | how many revisions a pull request gets |
   | `HEPH_RERUNS` | how many times a cancelled check is re-run |
   | `HEPH_REVIEWS` | repositories whose pull requests are reviewed under lenses |
   | `HEPH_COMPLETENESS` | repositories whose merged tickets are judged |
   | `HEPH_CLOSURES` | repositories where a ticket is closed when its verdict is met (also list them in `HEPH_COMPLETENESS`) |
   | `HEPH_AUTOMERGE` | repositories whose green, approved pull requests are merged |

   The four that are lists of repositories may name only repositories in `HEPH_REPOSITORIES`; the
   deploy stops before changing anything if one names another.

   Create the two environments, `production` and `preview`, under Settings, Environments. Put a
   required reviewer on `production` if you want a pause before it deploys.

## GitHub App settings (you)

One App signs people in and delivers webhooks to the production control plane. Everything below
is derived from the code unless marked *(inferred)*. `<base>` is `https://hephaestium-control-plane.fly.dev`
or the domain that replaces it, and must equal `HEPH_API_BASE_URL` with no trailing slash.

**Identifying and authorizing users**

| Setting | Value | Why |
| --- | --- | --- |
| Callback URL | `<base>/auth/callback` | `Auth.callback`; the redirect URI the login sends is exactly `<base>/auth/callback` |
| Request user authorization (OAuth) during installation | off *(inferred: the flow starts from `/auth/login`, not from an install)* | |
| Expire user authorization tokens | **on** *(inferred)*, or off | The user token is kept, sealed, in a cookie for the session's eight hours (`Session.start`, `Session.user_token`). The code reads only `access_token` from GitHub's answer and never a refresh token, so an expiring token (GitHub gives it eight hours *(unverified)*) lapses with the session, and a reply made after it lapses is a 401 that says to sign in again (`Reply`). Off keeps a non-expiring user token in a cookie; on is the safer of the two. |
| Setup URL, Redirect on update | empty | nothing reads it |
| Device flow | off | the worker's device token is the control plane's own, not GitHub's |

The login asks for no scopes (`allow_signup=false`, no `scope` in `Auth.login`).

**Webhook**

| Setting | Value |
| --- | --- |
| Active | on |
| Webhook URL | `<base>/webhooks/github` |
| Content type | `application/json` (the receiver decodes the raw body as JSON, `Webhook.normalise`) |
| Secret | the value of `HEPH_WEBHOOK_SECRET` |
| SSL verification | enabled |

**Subscribed events**, six, from `ControlPlane.GitHub.Webhook`: `Issues`, `Issue comment`,
`Pull request`, `Pull request review`, `Check run`, `Push`. Anything else is acknowledged and
dropped.

**Repository permissions**, from the calls `ControlPlane.GitHub.Tracker` and
`ControlPlane.GitHub.CodeHost` make *(the mapping from endpoint to permission is from GitHub's
documentation as remembered, not read in this repository, so each is inferred)*:

| Permission | Level | What needs it |
| --- | --- | --- |
| Metadata | read | always granted; repository reads, `collaborators/<login>/permission`, branch rules |
| Issues | read and write | list and read tickets, events, comments and labels; create and close issues; labels; comments (on a ticket, and on a pull request, which GitHub treats as an issue); sub-issues and dependencies; the person's reply |
| Pull requests | read and write | read a pull request, its files and reviews; the merge |
| Contents | read and write | read files at a ref, `compare`; the squash merge also needs it |
| Checks | read | check runs and their annotations |
| Actions | read and write | a job's log (read), a job re-run (write) |
| Administration | none *(inferred: reading `rules/branches/<branch>` needs no more than read access to the repository)* | |

**User permissions (user-to-server)**: none to ask for. A user token can do at most what the App's
repository permissions above allow and the person may do, so the reply's two calls (comment, and
`POST .../labels`) need only Issues write. The `GET /user` that names the person needs nothing
*(inferred)*.

None of the above is used by the control plane's own `gh` calls today, which run as the
`GH_TOKEN` token, not as the App (see the first section). They are what the App needs for the
user-token calls and for the day the control plane runs as the App.

**Where each credential goes**

- Client id and a client secret (Generate a new client secret): into the two Actions secrets
  `HEPH_API_GITHUB_CLIENT_ID` and `HEPH_API_GITHUB_CLIENT_SECRET` of the `production` environment.
- Webhook secret: `HEPH_WEBHOOK_SECRET`.
- The App's private key (Generate a private key): **not read by any code today.** Keep the
  downloaded file offline. It becomes `HEPH_REVIEW_APP_KEY` only if this App is also used as the
  review App, which ADR 0037 recommends against.

**Which installation**: install the App on the account that owns the repositories, on the
repositories in `HEPH_REPOSITORIES` only. Webhooks arrive for the repositories the App is
installed on, and the control plane drops those not in `HEPH_REPOSITORIES`.

### What works for previews

Finding, not a guess: the App has one webhook URL and one list of callback URLs, and every
preview has its own host name.

- **Webhooks**: the App's single URL is production's. A preview receives none, so it never has
  `HEPH_WEBHOOK_SECRET`, and its route does not exist. Redirecting the App's webhook to a preview
  would take production's deliveries away.
- **Login**: GitHub accepts a redirect URI only if it is one of the App's callback URLs. The list
  is short *(unverified: GitHub's documentation says up to ten for an App)* and a preview's
  host is new each time, so a preview cannot be added to it. A sign-in attempted on a preview
  ends on GitHub's "redirect_uri is not associated with this application".
- **Repositories**: a preview must not read or write the real repositories. It has no
  `HEPH_REPOSITORIES` and no `GH_TOKEN`, so intake and settling have nothing to act on, and two
  control planes do not work the same tickets.

So a preview proves that the image builds, migrates the branch's database, starts, serves the
dashboard and answers `401` where it should. It does not prove sign-in, a webhook or a worker's
claim, which need a signed-in person. **Recommendation:** accept that, and prove those on
production the first time (a rollback restores the previous release). If a preview that signs
in is wanted later, the cost is a second, separate App whose callback is one fixed hostname, and
previews would have to share that hostname; that is a change to how previews are named and is
not done here.

## Deploy

CI does this. **You** do it once by hand only to find the first thing that goes wrong sooner:

```sh
# from the repository root; the production Fly app and its secrets must already exist
fly deploy --config control-plane/fly.toml --ha=false
```

The workflow runs the same command after setting the secrets, and the first time it is the same
as the first deploy in [the checklist](#verify).

1. **Production**: push to `main`, or run `deploy-production.yml` by hand. It reads the
   production Neon project's default branch, gets its pooled and direct strings from the Neon
   API, sets the Fly secrets (`DATABASE_URL`, `HEPH_MIGRATION_DATABASE_URL`, `HEPH_API_SECRET_KEY_BASE`,
   `HEPH_API_GITHUB_CLIENT_ID`, `HEPH_API_GITHUB_CLIENT_SECRET`, `HEPH_WEBHOOK_SECRET`, `GH_TOKEN`,
   `HEPH_API_ALLOWED_LOGINS`, `HEPH_REPOSITORIES`, the review App's two and each of the optional settings above that is set), then runs
   `fly deploy`. The release command, `/app/bin/migrate`, runs on a temporary machine from the new
   image first, over the direct string; if it fails the old version keeps serving.
2. **A preview**: opening or updating a pull request runs the checks, then, if they pass, makes
   the Fly app and the Neon branch for it and deploys. Closing the pull request deletes both. A
   weekly sweep deletes any that no open pull request owns.

What is in `control-plane/fly.toml`, and why:

- `[http_service]` with `internal_port = 4000` (the API: login, dashboard, `/worker/*`,
  `/webhooks/github`); `force_https`; `auto_stop_machines = "off"`, `min_machines_running = 1`, one
  machine (`--ha=false`), because the control plane holds the queues and the long polls and a
  stopped machine drops deliveries. Port 8080, the health listener, is not exposed.
- `HEPH_API_BASE_URL` is a placeholder for the production host. CI overrides it with the preview's
  own address.
- `HEPH_EXECUTOR_LINK = "workers"`, so executors on developers' machines run the work. The
  long poll waits 25 seconds (`HEPH_CLAIM_WAIT_SECONDS`); Fly's idle timeout is not set in the file
  and is believed to be longer *(unverified, the first claim through the public address shows
  it; if a claim ends early, lower the wait)*.

### Secrets, and what `_FILE` means on Fly

Fly gives a secret to the machine as an environment variable. The control plane reads these as
variables: `DATABASE_URL`, `HEPH_MIGRATION_DATABASE_URL`, `HEPH_API_SECRET_KEY_BASE`,
`HEPH_API_GITHUB_CLIENT_ID`, `HEPH_API_GITHUB_CLIENT_SECRET`, `HEPH_REVIEW_APP_KEY`, and now
`HEPH_WEBHOOK_SECRET` (it used to be readable only from a file named by
`HEPH_WEBHOOK_SECRET_FILE`, which Fly cannot supply without writing a file at boot). The `_FILE`
variants stay for the laptop. Setting both `HEPH_WEBHOOK_SECRET` and `HEPH_WEBHOOK_SECRET_FILE`
stops the start. A variable cannot be rotated without a restart; `fly secrets set` restarts the
machine.

## Verify

Against production, in this order. Each step names what a pass looks like.

1. **Health**: `curl -sS https://<host>/` answers `200` (the dashboard's page). The health listener
   is private: `fly ssh console -a hephaestium-control-plane -C "wget -qO- localhost:8080/health"`
   *(unverified: the image has no `wget`; use `fly ssh console` and `/app/bin/control_plane rpc`
   if it is missing)*. `fly status` shows the machine passing both checks.
2. **Unauthenticated surface**: `curl -si https://<host>/api/tickets` is `401`;
   `curl -si https://<host>/worker/claim -X POST` is `401`; `curl -si https://<host>/webhooks/github -X POST -d '{}'` is `401`.
3. **Login with the allowlist**: open `https://<host>/auth/login` in a browser, sign in as a login
   in `HEPH_API_ALLOWED_LOGINS`; you land on the dashboard and `/api/session` says your login.
   Sign in as a login not on the list: `403`. A wrong callback shows GitHub's "redirect_uri" error:
   the App's callback does not equal `<HEPH_API_BASE_URL>/auth/callback`.
4. **Dashboard**: the tickets page loads and lists nothing yet without an error.
5. **Make a device token**: the dashboard has no page for it yet *(finding: the routes exist, the
   page does not)*. In a signed-in browser's console on the site: fetch `/api/csrf`, then
   `fetch('/api/device-tokens', {method: 'POST', headers: {'x-csrf-token': <csrfToken>}, body: JSON.stringify({name: 'laptop'})})`,
   where `<csrfToken>` is the `csrfToken` field of the `/api/csrf` answer (not `token`).
   The answer holds the token once; save it to `~/.config/hephaestium/worker-token` with mode 600.
6. **`heph worker` enrols and makes a first claim**:
   `heph worker --control-plane https://<host>`. It prints nothing and stays running with no
   error; a `204` claim every 25 seconds is normal; `GET /api/device-tokens` shows the token
   `connected`. A wrong token ends with "does not accept the device token".

   Two things about the machine the worker runs on:

   - If the worker will work on a private repository, run `gh auth setup-git` once first, so
     that `git` uses `gh`'s credentials for clones and pushes; without it the worker's `git`
     is refused on the private repository.
   - If `heph` is not installed, run it from the repository's build output:
     `cargo build --release -p heph`, then `target/release/heph worker --control-plane https://<host>`
     from the repository root.
7. **A webhook delivery**: open the App's settings, the Advanced tab, and its Recent Deliveries
   list. Redeliver the `ping` or the latest delivery. The response column shows the status the
   control plane answered:
   - `202`: the signature was valid and the delivery was kept (`accepted`) or deliberately
     dropped (`ignored`, for an event or repository not listed). This is a pass.
   - `401`: the signature did not verify; the secret in the App differs from `HEPH_WEBHOOK_SECRET`.
   - `503`: the control plane has no webhook secret.

   Then label a ticket in a listed repository `heph:ready`: a new delivery shows `202` and
   `fly logs` has no `missed delivery`.
8. **A reply shows the person as the actor**: when a run has ended blocked with a question,
   answer it on the dashboard. On GitHub the ticket's event list shows the `labeled` event with
   *you* as the actor, and the comment is yours. This is the check ADR 0037 asks for before the
   reply is relied on. It also needs the `GH_TOKEN` identity to have write access to the
   repository, for the permission check.

## Roll back

`fly releases -a hephaestium-control-plane` lists releases; `fly deploy -a hephaestium-control-plane --image <the previous release's image> --ha=false --config control-plane/fly.toml`
runs the previous image. A migration the new release made stays: migrations are additive in this
repository, and for the data, Neon's point-in-time restore (project, Restore, to a time before
the deploy) is the way back. Take the restore into a new branch first and look at it. Restoring
the production branch in place discards what was written since.

## Rotate the App key, the secrets

- A secret: change the Actions secret, run `deploy-production.yml`.
- The webhook secret: change it in the App's settings and in `HEPH_WEBHOOK_SECRET` together, then
  deploy; deliveries between the two are `401` and are redelivered by hand from the App's
  Advanced tab.
- `HEPH_API_SECRET_KEY_BASE`: signs everyone out and breaks every sealed user-token cookie.
- The App's private key: only the review App uses it; make a new key, set `HEPH_REVIEW_APP_KEY`,
  deploy, delete the old key in the App.

## Compatibility risk: Postgres 16 in CI, 18 on Neon

The control plane's tests and the migrate check in CI run Postgres 16 (`postgres:16` in
`.github/workflows/ci.yml`). Neon runs 18. This has not been run against 18. What to check on the
first preview deploy, which migrates a real 18 branch:

- the migrations apply (the release command's output in the deploy log);
- Oban's own tables and its notifier: `HEPH_OBAN_NOTIFIER=pg` is set so nothing depends on
  `LISTEN`, and the pooled connection is transaction-pooled, so look for jobs that stay `available`;
- Postgrex's named prepared statements through the pooled string *(unverified: Neon's pooler is
  pgbouncer in transaction mode, which supports protocol-level prepared statements from
  pgbouncer 1.21; if queries fail with "prepared statement does not exist", the Repo needs
  `prepare: :unnamed`, a one-line change)*;
- the connection strings' `?sslmode=require` query: CI strips it, because `HEPH_DB_SSL` sets TLS
  itself and an option Postgrex does not know can stop the start *(unverified)*.

The cheap, durable fix is to move the CI image to `postgres:18`; that is a separate change.

## What is expected to go wrong first

In the order they are likely:

1. The deploy's secret step fails because a GitHub Actions secret or variable is not set: the
   script names the missing one and stops before it touches Fly.
2. `HEPH_API_BASE_URL` does not equal the App's callback host: GitHub's `redirect_uri` error at
   login. Fix whichever is wrong.
3. The release command fails on Neon: a migration, the pooled string used by mistake (an advisory
   lock needs the direct one), or an 18-only difference above.
4. `fly deploy` marks the machine unhealthy: the `/` check needs `HEPH_API_STATIC_DIR`, which the
   image sets; the health check on 8080 needs the database.
5. A claim ends early through the public address: the proxy's idle timeout.
6. A webhook delivery is `401`: the secret differs between the App and the Actions secret. `503`:
   the control plane has no secret (variable not set).
7. A job fails with `gh could not be run` or `heph could not be run`: the binary is missing or
   not executable on the machine the job ran on; both are in the image.
