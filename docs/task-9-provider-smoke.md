# Task 9 provider smoke note

This note records verification evidence without changing the Task 9 checklist state.

## Automated provider evidence

- The Task 9 mock contract suite established the complete 37-ID catalog. Its original five deferred
  native cloud providers are implemented and covered by the Task 11 contract and smoke note.
- Unit, integration, component, source Electron E2E, build, and package-policy commands are
  recorded in the implementing commit/report.
- The Electron canary test uses a hostile local provider that echoes its received credential in
  model metadata and completion output, then checks a fresh renderer and persisted stores for the
  absence of that credential.
- Provider requests deliberately omit optional product attribution (`HTTP-Referer`, `X-Title`, and
  AnythingLLM identity headers). Contract tests assert this omission; Talking Quill sends only headers
  required by the configured provider protocol.
- OpenAI Responses validation accepts only `status: "completed"` responses and extracts transcript
  text from documented `output[].content[]` records whose type is `output_text`; incomplete,
  refusal-only, empty, malformed, and oversized responses are rejected.

## Opt-in live harness

Run:

```text
TALKING_QUILL_LIVE_OLLAMA=1 pnpm test:live:providers
TALKING_QUILL_LIVE_OPENAI_KEY=<key> pnpm test:live:providers
```

Optional overrides are `TALKING_QUILL_LIVE_OLLAMA_URL`, `TALKING_QUILL_LIVE_OLLAMA_MODEL`, and
`TALKING_QUILL_LIVE_OPENAI_MODEL`. The OpenAI check makes a real, potentially billable API request
and uses API-key authentication only.

## Live state for this worktree

- **Ollama:** passed on 2026-07-20 on Windows against `http://127.0.0.1:11434` with
  `qwen3-vl:4b-instruct`. The opt-in harness listed installed models, validated the selected model,
  and classified the endpoint as Local. The expanded harness was rerun with the same model and also
  passed capability lookup, discovered-context completion, and `cleanTranscript`. An unpinned run
  selected a different installed model that returned HTTP 5xx; this is not recorded as a provider
  success and is why release smoke runs should name the intended model explicitly.
- **OpenAI:** not executed; no credential was provided. No OpenAI success is claimed.

Update only these state lines after an operator actually runs the opt-in harness, including date,
platform, endpoint/model (never the credential), and result.
