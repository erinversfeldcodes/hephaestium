# 0027. A gate gets the secrets it declares, and the agent never does

## Status

Accepted

## Context

A repository's gate can declare what it needs (ADR 0014): variables, and secrets such as an API key. The executor looks each one up in its own environment. The first real run on a repository whose `test` gate declares a secret did its work, passed the other gates, and then failed, because the executor had no way to be given that secret without also giving it to the agent. The executor takes the secrets it knows (the GitHub and Claude tokens) at the start and removes them from its environment, so that a bootstrap step, a gate or the agent, which run the repository's code and the agent's changes to it, cannot read them. A secret that a manifest names is not one it knows.

- Option 1: Give the executor the secret as an ordinary variable. It works, and the agent, every bootstrap step and every gate inherit it.
- Option 2: Treat a secret requirement as met without a value, because the executor cannot tell which gates are safe to give it to. A gate that needs a key then fails inside its own run, which ADR 0014 was written to prevent.
- Option 3: Hold secrets the way the tokens are held, under names the manifest chooses, and hand each to the gates that declare it.

## Decision

We will go with Option 3.

- The executor takes every variable that starts with `HEPH_GATE_SECRET_`, once at the start, before anything else runs. The rest of the name is the secret's name: `HEPH_GATE_SECRET_PIXABAY_API_KEY` is `PIXABAY_API_KEY`. A trailing `_FILE` makes the variable the path of a file that holds it, with the rules the tokens have: owner-only, a regular file, not empty, trimmed, and never both a variable and a file for one name.
- Both the variable and its pointer are removed from the executor's environment.
- A gate whose `requires` names a secret is given its value under that name, and no other gate, bootstrap step or agent is. A requirement is met by a held secret, or by a variable of that name in the executor's environment as before.
- A name that cannot be a variable's is an error, and the executor refuses to start. No message holds a value.
- A requirement that is still unmet is a gate that could not be run, as ADR 0014 says.

## Consequences

A repository's own secret gates can run, and the agent does not receive what they use. A gate that is given a secret can still print it: the repository's gates are trusted code, and the agent's changes to them are what a review reads. The secrets are the executor's, and so shared by every repository a control plane takes in. Two repositories whose gates declare the same name cannot have different values until the control plane has settings for each repository. A gate that cannot run for want of a secret still costs the agent's run, because whether a gate applies to a change is known only once the agent has made it.
