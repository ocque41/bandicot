# 0001: Bandicot Public Identity

Status: accepted

## Context

The fork currently exposes `bandicot`, `grok-openai`, and an official npm
`grok` installation. Runtime data is split across Grok-named homes.

## Decision

Use `bandicot` as the only public command, `~/.bandicot` as the canonical user
home, and `.bandicot` as the canonical project directory. Continue accepting
legacy names as migration inputs where required. Retain internal upstream crate
names and serialized wire identifiers unless a versioned migration exists.

## Consequences

- Installation cleanup must verify ownership before deleting commands.
- Existing Bandicot data must be migrated and verified before old paths become
  inactive.
- Documentation and UI use Bandicot while attribution still names upstream.
