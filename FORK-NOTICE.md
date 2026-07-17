# Fork and modification notice

This repository is an unofficial fork of
[`xai-org/grok-build`](https://github.com/xai-org/grok-build), originally
published by SpaceXAI/xAI. The upstream source, history, copyright notices,
Apache License 2.0, and bundled third-party notices are retained.

## Fork-maintained changes

The `ocque41/bandicot` fork adds and maintains:

- a secret-free OpenAI Platform profile using the Responses API and curated
  GPT-5.6/GPT-5.3 Codex entries;
- provider-isolation guards that prevent xAI-only headers, tools, media
  endpoints, sessions, and credentials from crossing into OpenAI requests;
- fail-closed OpenAI credential selection and credential-safe diagnostics;
- an isolated `bandicot` source-build installer and macOS Keychain helper;
- a transactional one-command upstream update workflow; and
- OpenAI-specific mock integration tests, documentation, and release gates.

The fork's scripts do not install upstream xAI release binaries, replace the
official `grok` command, modify shell startup files, or write API keys into the
repository/configuration profile.

## No affiliation or endorsement

This fork is not affiliated with, endorsed by, sponsored by, or supported by
SpaceXAI/xAI or OpenAI. `Grok Build`, `Grok`, `xAI`, `SpaceXAI`, `OpenAI`, model
names, logos, and related marks belong to their respective owners. The
`Bandicot` is this fork's name and does not imply an
official product or partnership.

Support requests and fork-specific security reports should go to this fork's
owner, not to either upstream company, unless the issue is independently
reproducible in an unmodified upstream release.

## License and third-party attribution

The fork does not replace or narrow the upstream licenses. First-party source
continues under the Apache License, Version 2.0 in [`LICENSE`](LICENSE).
Dependency, vendored-source, and port attribution remains in
[`THIRD-PARTY-NOTICES`](THIRD-PARTY-NOTICES),
[`crates/codegen/xai-grok-tools/THIRD_PARTY_NOTICES.md`](crates/codegen/xai-grok-tools/THIRD_PARTY_NOTICES.md),
and [`third_party/NOTICE`](third_party/NOTICE). Recipients must retain all
applicable notices when redistributing source or binaries.
