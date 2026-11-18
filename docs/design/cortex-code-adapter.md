# The Cortex Code adapter

Status: a specification, from Snowflake's published documentation only, for the
maintainer to review before any of it is built.

Cortex Code (`cortex`, which Snowflake's own pages also call CoCo CLI) is the
third agent runtime ADR 0005 names. This records what its documentation
establishes, what it leaves out, and the adapter that follows from both. The
Claude Code adapter is the model it is measured against.

## The constraint that shapes this

Nothing here was run. The Snowflake account is off-limits to this project's
tooling: no `cortex` or `snow` command is run, and the adapter is built against
documentation. For Claude Code a real run was recorded first and the adapter was
tested against it. For Cortex Code there is no recorded run, and the pages do not
print the output of `--output-format stream-json`, so the adapter's central
function, reading what the runtime prints, rests on an inference described below.
It ships marked unverified, and the first person with a Snowflake account they
may use closes that, with the steps at the end.

## What the documentation establishes

Each row is from a page read in full. A figure or name that is not on a page is
under the next heading, not here.

| Subject | What the documentation says | Page |
|---|---|---|
| Starting it | `cortex -p "<prompt>"` runs one prompt and exits. `cortex exec` is for scripts: it reads a prompt from a file or standard input (`--file -`), plan mode is always off and interactive prompts are rejected. | CLI reference |
| Output | `--output-format stream-json` is "JSON output (for scripting)", and the only value listed. | CLI reference |
| Choosing a connection | `-c, --connection <name>`, from `~/.snowflake/connections.toml`. | CLI reference |
| Working directory | `-w, --workdir <path>`. | CLI reference |
| Model | `-m, --model <name>`. `--effort` is `minimal` to `max`. | CLI reference |
| Tool set | `--mode <standard\|code>`: `code` is a lean coding tool set. | CLI reference |
| Permissions when there is no one to ask | A tool call that is not pre-approved with `--allowed-tools` is denied, and Cortex Code carries on without it. `--disallowed-tools` removes tools from the model's context, and wins if a tool is in both lists. `--bypass` approves every tool call and "should be used only in trusted environments". | CLI reference, tools |
| Limits | `--max-turns <n>` is the only per-run limit. There is no spending flag. | CLI reference |
| Session history | `--private` does not save the session and disables server-side logging. | CLI reference |
| Exit codes | Zero for success and non-zero for failure, with no stable mapping beyond that. Errors go to standard error. | CLI reference |
| Where it keeps things | `~/.snowflake/cortex/` holds settings, permissions, MCP servers, hooks, conversations, memory, skills, agents and logs. `SNOWFLAKE_HOME` replaces `~/.snowflake`. | settings, security |
| Authentication | Browser sign-in by default. A programmatic access token (PAT) is the way to scope access to a role without a browser. Credentials can be taken from environment variables with `${NAME}` in the connections file. | security |
| What it reads from a repository | `.cortex/` or `.claude/` for skills, subagents, settings and hooks, and the local overrides `settings.local.json`. | extensibility |
| What it reads from the home directory | Skills, subagents and settings under `~/.snowflake/cortex/` and also under `~/.claude/`. | extensibility |
| Message types | `system`, `assistant`, `user`, `stream_event` and `result`. Assistant and user messages hold content blocks `text`, `thinking`, `tool_use` (`id`, `name`, `input`) and `tool_result` (`tool_use_id`, `content`, `is_error`). The result holds `subtype`, `is_error`, `num_turns`, `session_id`, `result`, `total_cost_usd` and `usage`. | Agent SDK references |
| Result subtypes | `success`, `error_during_execution`, `error_max_turns` and `error_max_budget_usd`. | Agent SDK TypeScript reference |
| Assistant errors | `authentication_failed`, `billing_error`, `rate_limit`, `invalid_request`, `server_error`, `unknown`. | Agent SDK Python reference |
| Spending | Usage is metered in credits. The one cap is an account or user parameter, `CORTEX_CODE_CLI_DAILY_EST_CREDIT_LIMIT_PER_USER`, over a rolling 24 hours, set in SQL by an administrator. When it is reached the CLI says "Daily credit usage limit reached". | cost controls |
| Isolation it offers | A local operating-system sandbox for shell commands, marked experimental. A flag, `--cloud`, runs tools in a Snowflake-managed container. | sandbox, CLI reference |

## What the documentation leaves out

These are the gaps the design has to be honest about.

1. **The wire format.** No page shows a line the CLI prints. The Agent SDK pages
   define the message types as Python and TypeScript types, and say the SDK
   starts the CLI as a subprocess and reads JSON from it, but no page says its
   bytes are what `--output-format stream-json` prints. The names match Claude
   Code's structured output closely (`system`, `assistant`, `user`, `result`,
   `tool_use`, `tool_result`, `total_cost_usd`, `usage`), which makes it likely
   they match on the wire, and that is an inference.
2. **Usage.** `usage` is an untyped dictionary and its keys are not named. No page
   names a credits field, and `total_cost_usd` is documented for Python only.
3. **Whether a connection falls back to a default** when none is named. The
   Workload contract already says an adapter never falls back, so the adapter
   always names one; but nothing says what the CLI does without `-c`.
4. **How to turn off what a repository or the home directory supplies.** Hooks,
   skills, subagents and MCP servers are read from the working directory and the
   home directory, and there is no switch to disable them. The only controls are
   a managed settings file that is system-wide and administrator-owned, and not
   having the files.
5. **Whether `AGENTS.md` and `CLAUDE.md` are read.** Not on the pages read.
6. **How `--allowed-tools` is written.** The CLI reference writes `Bash(git *)`
   and the tools page writes `bash(git *)`. The pages say tool names are
   case-sensitive runtime identifiers, and use both.
7. **Whether the connections file is `connections.toml` or `config.toml`.** The
   security page says the first, the settings page says the second.
8. **Whether `--bypass`, `--private` and the permission flags apply under `-p`
   and `exec`.** Only the rule about unapproved tools is stated for batch mode.

## The adapter

An implementation of the executor's adapter interface, selected when a workload's
runtime kind is Cortex Code. It differs from Claude Code's in four places: how it
authenticates, how it is kept apart from the maintainer's setup, what it may do,
and what it can say about cost.

### Launch

`cortex -p PROMPT --output-format stream-json --private --mode code --workdir
WORKSPACE --connection NAME --model MODEL --max-turns N`, with the prompt built
from the ticket as the Claude Code adapter builds it. The connection is the one
the workload names (`runtime.connection`), always, and a workload for this
runtime that names none is refused when it is read: the contract says so and the
adapter does not choose. `--private` keeps the run out of Snowflake's history.
The number of turns is a fixed, generous bound until the workload can carry one.

`--mode code` is chosen so the agent has a coding tool set and not the SQL and
semantic-model tools of the full set. Those are also named in
`--disallowed-tools` (the Snowflake SQL, object-search, semantic-model and
notebook tools, and the web tools): a workload edits a repository and must not
reach the maintainer's data.

### What it may do

`--allowed-tools` for read, edit, write, glob, grep and bash, so that anything
else is denied and the run cannot wait. This is the same decision as for Claude
Code, with the same cost: bash is unrestricted inside the workspace. `--bypass`
is never used. The identifiers' casing is gap 6: the first run settles it, and
until then the adapter passes the lowercase form the tools page lists.

### Keeping the maintainer's setup out

Cortex Code reads more than Claude Code does, and from more places, so isolation
needs two directories and not one.

- `SNOWFLAKE_HOME` is a directory of the run's own, beside the workspace. It holds
  a connections file written for the run and nothing else, so the maintainer's
  other connections, settings, hooks, memory and history are not there.
- `HOME` is also the run's own, because skills, subagents and settings are read
  from `~/.claude/` as well. The plan names neither as a variable to pass: they are
  set in the command, as the Claude Code adapter sets its configuration
  directory, because a plan names variables and holds no values.
- Memory is opt-in through an environment variable the plan never names, and so
  is off.

What this cannot do is gap 4: the repository's own `.cortex/` and `.claude/` are
read from the workspace, and a repository's hooks run. For the trusted tickets
the first version is limited to, that is the repository's own behaviour. It is not
acceptable for anything the maintainer did not write, and a container or
Snowflake's managed container (`--cloud`) is the answer there, as it is for every
runtime.

### Authentication

A headless run cannot use a browser, so the connection must use a programmatic
access token, and the token is a separate, revocable credential scoped to a role
of least privilege and never the maintainer's own.

The run's connections file holds one connection, the one the workload names. The
executor writes it from the maintainer's own file by copying that one section, and
refuses to run if the section does not use token authentication. The token itself
is in the executor's environment under a name the maintainer chooses, and the
copied section refers to it as `${NAME}` as the documentation allows, so the token
is never written into the run's directory. The plan names that variable and
`PATH` and nothing else.

### Events

The same mapping as the Claude Code adapter, on the inference in gap 1: text
blocks become `agent_message`, `tool_use` blocks `tool_call`, `tool_result`
blocks `tool_result`. Thinking, `system`, `stream_event` and every line of a kind
not understood produce nothing, so a kind added later cannot fail a run, and a
line that is not JSON fails the workload. That tolerance is also what makes the
inference safe to ship: if the format differs, the adapter reads nothing and the
workload ends on its gates and its exit code, which is the right failure, and not
on a wrong reading.

A result that is an error says why. `error_max_turns` and `error_max_budget_usd`
fail the workload with that reason, and an assistant error of
`authentication_failed`, `billing_error` or `rate_limit` is named in the reason,
since each means a person has to do something.

### Cost

Cortex Code meters in credits, and the contract has a unit for credits. The
documentation shows a dollar figure on the result (`total_cost_usd`, Python only)
and no credits figure. The adapter reports what the runtime reports and converts
nothing: `total_cost_usd` as dollars, reported by the runtime, and credits only if
a recorded run shows a field for them. Until then its description lists dollars
only. The workload's budget is in dollars or tokens and Cortex Code reports no
tokens, so the executor can check a dollar budget at the end of a run and cannot
stop a run during it: there is no spending flag, and no cost event before the
result. The real control is the daily credit limit on the user that owns the
token, a parameter the maintainer sets in SQL once, and the prerequisites below
say so.

### The adapter's description

Runtime Cortex Code, provider Snowflake, `requires_connection` true, cost units
dollars, produced events agent message, tool call and tool result. It does not
produce a skill-loaded event: nothing in the documented output names one.

## What the maintainer provides

Once, on a Snowflake account the maintainer may use for this:

1. A user and a role of least privilege for workloads, with the Cortex user
   database role, and never `ACCOUNTADMIN`.
2. A programmatic access token for that user, with an expiry of at most 90 days,
   put in the executor's environment under a name of the maintainer's choosing.
3. A connection for it in the maintainer's connections file, using token
   authentication.
4. The daily credit limit on that user, because it is the only spending cap.

## What has to be recorded before this is trusted

One run, by a person who may use the account, of a small task against a scratch
repository, with the standard output saved:

```
cortex -p "Read hello.txt and tell me the magic word in one sentence." \
  --output-format stream-json --private --mode code \
  --allowed-tools "read" --connection NAME > cortex-sample.jsonl
```

Then, in the sample, the lines for a tool call, a tool result and the result are
read against the adapter, and gaps 1, 2 and 6 are closed. A second run with the
same prompt and one tool disallowed shows how a denial appears. The sample is
sanitised as the Claude Code one was (paths, account and user names, identifiers)
before it is committed as a fixture. A run is a few credits.

## How it will be built and proven

As the Claude Code adapter was, in this order, each its own ticket and pull request
with tests first:

1. The adapter against the documented message shapes, with a fixture written by
   hand from them and marked as such, and a stand-in `cortex` that records how it
   was started. The tests prove the launch (the connection is always named, the
   modes and tools, the two directories), the plan's environment, the events from
   the hand-written fixture and the tolerance for unknown lines.
2. The connections file written for a run: one section copied, token
   authentication required, the token referred to by variable and never written.
3. The recorded run replaces the hand-written fixture, and whatever differs is
   fixed.

## Decisions for the maintainer

1. **How a run gets its connection.** Recommended: copy the one named section into
   a run-owned file, as above. The alternative is to point `SNOWFLAKE_HOME` at the
   maintainer's own directory, which is simpler and gives every run the maintainer's
   other connections, settings, hooks and memory.
2. **Tool set.** Recommended: `--mode code`, with the Snowflake and web tools
   disallowed. The alternative is the full set, which lets a workload query the
   maintainer's data.
3. **Cost.** Recommended: dollars only until a recorded run shows credits.
4. **Whether to build before there is a recorded run.** Recommended: yes, as the
   first step above, marked unverified, because the adapter is tolerant by design
   and the work is not wasted. The alternative is to wait for a person with an
   account, which costs nothing and delays the third runtime.

## Sources

Every page is on docs.snowflake.com, under `/en/user-guide/`: `cortex-code/cli-reference`,
`cortex-code/tools`, `cortex-code/settings`, `cortex-code/security`,
`cortex-code/extensibility`, `cortex-code/managed-settings`, `cortex-code/sandbox`,
`cortex-code/memory`, `cortex-code/cost-controls`, `cortex-code/credit-usage-limit`
and the Cortex Code Agent SDK pages `cortex-code-agent-sdk/python-reference`,
`cortex-code-agent-sdk/typescript-reference`, `cortex-code-agent-sdk/streaming-output`
and `cortex-code-agent-sdk/structured-output`. They were read on 2026-11-18 and
Snowflake changes them; the first thing to do before building is read them again.
