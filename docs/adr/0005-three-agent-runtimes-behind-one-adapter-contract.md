# 0005. Three agent runtimes sit behind one adapter contract

## Status

Accepted

## Context

Forge supports two agent runtimes in fixed pairs: `pi` with OpenRouter and Claude Code with an Anthropic subscription. Hephaestium must also support Snowflake Cortex Code, whose use is paid for in a third way.

- Option 1: Fixed runtime and provider pairs, as Forge has.
- Option 2: Runtime and provider are separate, with an explicit table of which combinations work. Each runtime has an adapter that normalises its events to one Protobuf schema. A cost is recorded with its unit and its source.

## Decision

We will go with Option 2. The Cortex Code adapter is written against Snowflake's public documentation and tested against recorded fixtures. No live Snowflake account is used in development or in continuous integration. An adapter requires a connection to be named explicitly and never falls back to a default.

## Consequences

The dashboard and the evaluation code read one event schema and never a runtime's own format. Costs in different units are shown apart, not summed. The Cortex Code adapter can be wrong where the documentation is incomplete, and is corrected from fixtures a maintainer records.
