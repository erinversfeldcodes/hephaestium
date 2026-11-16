# 0011. A gate run reports one line on standard output and stops everything the gate started

## Status

Accepted

## Context

`heph gate run` is the seed of the executor: the same code will run gates inside a workload, and a workflow or the runtime will read what it prints and how it exits. Those are interfaces, and a caller will depend on every detail of them. Forge's runner enforces its limits itself and fails closed. A gate with no limit can hold a workload open for ever, and a tool that is missing must fail a gate and not make it vanish (ADR-0004).

- Option 1: Print the gate's own output and a human summary, and exit with the gate's exit code. Simple, and a caller cannot tell a gate that failed from one that could not be run, or a gate that was stopped for time from one that failed.
- Option 2: Print one line of JSON that follows Athenaeum's gate result schema on standard output and nothing else, send everything the gate prints to standard error, and use `heph`'s own exit code to say how the gate ended.

## Decision

We will go with Option 2.

- Standard output carries the result and nothing else. What the gate prints, on either stream, goes to standard error.
- The exit code is 0 when the gate passed or was skipped, 1 when it failed or timed out, 2 when the command line cannot be understood, and 3 when the gate could not be run: the manifest is missing or invalid, the gate is not declared, or a passing gate did not give the report the manifest says it gives. No result is printed for a 3, because a false one is worse than none.
- A program that does not exist fails the gate with exit code 127, and one that cannot be executed with 126, as a shell reports them, so a missing tool fails a gate and still yields a valid result. A command killed by a signal fails with 128 plus the signal number.
- A gate runs in a process group of its own, from the current directory, with no input. When it ends, or is stopped for running longer than its timeout, everything it started is stopped too. A gate leaves nothing running.
- Without a list of changed files every gate applies. With one, a gate that names `when_paths` runs only if a changed file matches, and is otherwise skipped with a reason. A `*` does not cross a directory and a `**` does.
- A gate whose manifest entry says `result = "json"` has its standard output read as a JSON object and returned as the result's `report`. The tool is graded against the pinned tag's schema in its own tests, and every result its tests produce conforms to it.

## Consequences

A caller can tell the four ways a gate ends apart, and a failure of the tool from a failure of the gate, without parsing text. A gate cannot hold a workload open past its limit, and a background process started by a gate is cleaned up even when the gate finishes normally, which means a gate cannot deliberately leave a server running for a later gate; a gate that needs one starts it itself. The tool supports only Unix, because it relies on process groups. Polling for the end of a command costs up to ten milliseconds of latency. The skipped outcome depends on the caller saying what changed, since working it out from git is not this command's job.
