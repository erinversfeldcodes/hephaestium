# 0029. A pull request closes its ticket only when the agent says it resolves it

## Status

Accepted

## Context

Delivery ended every pull request description with `Closes #N`, so merging closed the ticket. A ticket is closed only when every item that affects it is resolved. The first pull request that was a mitigation (gazebolang #61 for #58: a test-side change whose own document says the cause is unproven and the measurement is owed) still said `Closes #58`, because the agent had no way to say its change was partial.

- Option 1: Keep `Closes` always, and leave a person to reopen the ticket. The ticket reads as closed in between, and a merge does the opposite of what the work supports.
- Option 2: Never write `Closes`, only `Refs`. Nothing closes a finished ticket, and a person does every time.
- Option 3: The agent says whether its change resolves the whole ticket, and the description follows what it said.

## Decision

We will go with Option 3.

- The agent leaves `complete`, or `partial` and what remains, in `.heph-resolution`, beside the commit message. The prompt says what each means: `complete` is everything the ticket asks, done and shown to work, not only passing the checks.
- The description ends with `Closes #N` only for `complete`. For `partial`, and when the file is absent, empty or says anything else, it ends with `Refs #N`. A `partial` says what is not resolved. Only the single word `complete`, in any case and with any whitespace around it, closes: `completely`, `done`, or `complete` followed by more words do not.
- The file is never part of the change, as the commit message file is not.
- A revision pushes to an existing pull request and does not rewrite its description.

## Consequences

A ticket is closed by a merge only when the agent has said the work is whole, and the default is not to close. The agent's claim can be wrong, and nothing here checks it: that is for review against the ticket (the `spec` lens). A finished ticket whose pull request said `Refs` is closed by a person or by a later change, which is the cost of the default.
