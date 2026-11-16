# 0010. A reader pins Athenaeum as a build dependency, through a crate that holds no logic

## Status

Accepted

Amends ADR-0002.

## Context

The manifest's schema and its cases live in Athenaeum (ADR-0009), and the command-line tool in Hephaestium must check a manifest against them. ADR-0009 left open how the tool obtains the schema. ADR-0002 says Athenaeum holds no code.

- Option 1: The tool reads the schema at run time from the copy of Athenaeum that a repository's lock pinned. Athenaeum stays pure content and a repository is checked against the version it chose. But the tool has no schema of its own to be tested against, and nothing works until the sync exists.
- Option 2: Hephaestium keeps a copy of the schema and the cases. Simple, and a second home for the contract, which is how the two come to disagree without anyone noticing.
- Option 3: Athenaeum carries a crate that hands out the schema text and every case, unchanged. The tool takes Athenaeum at a tag as a Cargo git dependency, compiles the schema in, and runs the cases as its own tests.

## Decision

We will go with Option 3.

- The crate exposes text and nothing else. It does not parse, validate or interpret a manifest, and it has no dependencies.
- Its list of cases is generated from the case directory at build time. A case that does not state the rule it breaks fails the build.
- The crate's version is never bumped. A dependent pins a tag.
- Hephaestium names the tag in one place, and moving to a newer one arrives as a reviewable change.

ADR-0002's rule becomes: Athenaeum holds no logic. The scripts that check its own content, and a crate that hands that content out, are the only code in it.

## Consequences

A disagreement between the tool and the contract fails the tool's build instead of surfacing in a repository. Adding a case in Athenaeum grades every reader that moves to the new tag. The tool validates against the schema it was built with, not the one a repository pinned, so a repository on a newer Athenaeum than its tool is checked against older rules; the schema version in the manifest is what lets the tool refuse a manifest it does not understand. Athenaeum now needs a Rust toolchain in its checks. A reader in another language is not helped by the crate and reads the files directly.
