# Task 10 Ollama text and vision smoke evidence

This is narrowly scoped implementation evidence for Task 10. It does not change the Task 10 checklist or completion state. No Task 10-specific checklist file existed; this note follows the established `docs/task-9-provider-smoke.md` and `docs/task-11-provider-smoke.md` evidence location and format.

## 2026-07-20 — Windows x64 development host

- Host: Windows 10.0.26200 x64.
- Ollama endpoint: `http://127.0.0.1:11434`.
- Ollama version: `0.20.7`, queried from `/api/version` immediately before recording this evidence.
- Text-operation model: `qwen3-vl:4b-instruct`.
- Vision-operation model: `qwen3-vl:4b-instruct`, explicitly supplied through `TALKING_QUILL_LIVE_OLLAMA_VISION_MODEL`.
- Command:

  ```text
  TALKING_QUILL_LIVE_OLLAMA=1 TALKING_QUILL_LIVE_OLLAMA_MODEL=qwen3-vl:4b-instruct TALKING_QUILL_LIVE_OLLAMA_VISION_MODEL=qwen3-vl:4b-instruct pnpm.cmd test:live:providers
  ```

- Result: passed. The opt-in production provider path listed installed models, validated the selected local model and destination, completed a bounded text cleanup request, required a dynamically detected `supported` vision model, sent the bounded JPEG through Ollama's native `messages[].images` shape, and returned `Red` for the red-image prompt.
- Harness result: 1 live Ollama test passed; 6 unrelated credential-gated provider tests skipped.

The same installed multimodal model was intentionally used for both independent operations. This proves both the text and image-bearing request paths; it does not claim that two distinct Ollama models were verified. A separate installed text model was attempted but returned provider HTTP 5xx and is not recorded as a success.
