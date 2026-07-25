# Task 11 native provider smoke note

## Automated evidence

Anthropic, Gemini, Azure OpenAI, AWS Bedrock, and Cohere use production native REST protocols in
the Electron main process through the socket-pinned provider transport. The mock contract suite
covers native request and response shapes, authentication headers, model discovery, completion,
vision metadata, malformed/incomplete responses, cancellation, endpoint policy, and strict
configuration. The full registry contains exactly 37 runnable providers and no stubs.

Task 11 reports model vision capability where native discovery provides it, but deliberately does
not add image bytes to `ProviderCompletionRequest`. Task 10 owns the bounded image schema,
screenshot lifecycle, prompt image part, and OSA privacy tests; changing that shared request in this
parallel task would duplicate and conflict with Task 10. Until Task 10 lands, no UI can initiate an
image-bearing provider request.

Azure OpenAI API-key data-plane authentication cannot enumerate deployments. Its model list is
therefore the single deployment name explicitly entered by the user, and the UI labels deployment
discovery as requiring separate management-plane credentials rather than pretending to discover
it remotely. Adding Entra/ARM authentication would violate Task 11's existing one-secret API-key
configuration.

## Opt-in live harness

The live checks are billable and run only when both a credential and explicit model/deployment are
provided. Run `pnpm test:live:providers` with the appropriate pair:

- `TALKING_QUILL_LIVE_ANTHROPIC_KEY`, `TALKING_QUILL_LIVE_ANTHROPIC_MODEL`
- `TALKING_QUILL_LIVE_GEMINI_KEY`, `TALKING_QUILL_LIVE_GEMINI_MODEL`
- `TALKING_QUILL_LIVE_AZURE_KEY`, `TALKING_QUILL_LIVE_AZURE_ENDPOINT`,
  `TALKING_QUILL_LIVE_AZURE_DEPLOYMENT`; optional `TALKING_QUILL_LIVE_AZURE_MODEL_TYPE=reasoning`
- `TALKING_QUILL_LIVE_BEDROCK_ACCESS_KEY_ID`, `TALKING_QUILL_LIVE_BEDROCK_SECRET_ACCESS_KEY`,
  `TALKING_QUILL_LIVE_BEDROCK_MODEL`; optional `TALKING_QUILL_LIVE_BEDROCK_SESSION_TOKEN` and
  `TALKING_QUILL_LIVE_BEDROCK_REGION` (defaults to `us-west-2`). Bedrock requests use AWS Signature
  Version 4; no bearer-token mode is supported. Access key ID, secret access key, and optional
  session token are stored together as one encrypted, write-only vault value bound to the selected
  AWS region; changing region requires re-entry and never exposes stored fields to the renderer.
- `TALKING_QUILL_LIVE_COHERE_KEY`, `TALKING_QUILL_LIVE_COHERE_MODEL`

Never record credentials in this file.

## Live state for this worktree

No credentials for the five native cloud providers were available during Task 11 implementation.
No live-provider success is claimed. Anthropic, Gemini, Azure OpenAI, AWS Bedrock, and Cohere remain
**community-verified pending** until an operator runs the opt-in harness and records date, platform,
provider, model/deployment, region where applicable, and pass/fail result here.
