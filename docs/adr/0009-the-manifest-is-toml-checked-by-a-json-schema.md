# 0009. The manifest is TOML, defined by a JSON Schema, and its cases are the contract

## Status

Accepted

## Context

A managed repository describes itself in a manifest (ADR-0002). People write it by hand, skills read it for paths and commands, and Hephaestium runs what it declares. It needs a file format, a definition that more than one reader can be held to, and a place in the repository. ADR-0001 chose Protobuf for what crosses between components at run time; the manifest is a different kind of thing, a configuration file with comments that a person edits.

- Option 1: Define the manifest as a Protobuf message and read TOML through its JSON mapping. One schema language in the project, but Protobuf cannot state the rules that matter here (a path may not leave the repository, a name is a slug, an unknown key is an error), so they would live in each reader's code.
- Option 2: Let the Rust types in the command-line tool be the definition. Nothing to keep in step, but the definition would then be an implementation, in the repository that holds code, and a second reader would have to copy it.
- Option 3: TOML, defined by a JSON Schema in Athenaeum, with a set of manifests that must be accepted and manifests that must be rejected, each stating the one rule it breaks. A reference validator grades the schema against the cases in Athenaeum's own checks, and every other reader is graded against the same cases.

## Decision

We will go with Option 3.

- The schema lives at `manifest/schema.json` in Athenaeum, and each rule is stated there and nowhere else.
- An unknown key is an error. A command is a list of arguments, never a shell string. A path is relative to the repository root and may not contain a `..` segment.
- A case under `manifest/cases/invalid/` must be rejected for exactly the reason it states. A reader that rejects it for another reason has not passed it.
- The manifest carries a `schema_version`. A change that makes a valid manifest invalid takes a new version.
- In a managed repository the manifest is `.athenaeum/manifest.toml`, beside the lock that pins the Athenaeum revision it was written against.

## Consequences

The definition of a valid manifest is content, held apart from any program that reads it, so a disagreement between the command-line tool and the schema is a failing case and not a matter of opinion. Editors that understand JSON Schema can check a manifest as it is typed. The project has two schema languages, one for configuration and one for messages. The pattern that keeps paths inside the repository uses a lookahead, so a reader's regular-expression engine must support one. How the command-line tool obtains the schema, compiled in from a pinned Athenaeum release or read from the repository's own copy, is decided when `manifest check` is built.
