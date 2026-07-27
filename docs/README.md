# Talking Quill user guide

Talking Quill is local-first, system-wide dictation for Windows and macOS, packaged for x64 and ARM64. Local Whisper transcribes speech, then Talking Quill attempts to paste the result into the focused target.

Raw stays local. Smart can send the local transcript and context to a configured provider. Talking Quill needs no account, but a provider may require an account, credentials, or payment.

## Set up

1. Install the package for your operating system and processor, then open Talking Quill.
2. In Welcome, allow microphone access and run the microphone test.
3. Download and verify a local model: Large v3 Turbo is about 1.09 GB; Small is about 250 MB.
4. Optionally configure and test Smart processing. Skip it to use Raw dictation only.
5. Review the dynamically listed profiles and shortcuts on the Ready screen.

On macOS, allow Accessibility and Input Monitoring for shortcuts and text insertion. Screen Recording is needed only for On-Screen Awareness. **Settings** and **Info** can show Welcome again.

## Profiles and shortcuts

Put the cursor in a text field and use a profile shortcut:

| Profile | Shortcut | Mode |
| --- | --- | --- |
| General | `Alt+Z` on Windows, `Option+Z` on macOS | Raw |
| Prompt | `Alt+Shift+Z` on Windows, `Option+Shift+Z` on macOS | Smart |

Talking Quill has the two built-ins and up to ten profiles total. A shortcut chord uses one or more modifiers plus one or more ordered, simultaneously held A–Z keys. Examples include `Alt+X+P` and `Ctrl+Shift+P` on Windows; the same modifiers are shown as `Option` and `Control` on macOS. The last letter pressed is the **final trigger**. Focus a profile’s shortcut field, hold the modifiers and letter keys in order, then release them. `Tab` and `Shift+Tab` leave the field normally.

Every full chord must be unique. Chords with the same modifiers also cannot prefix one another: for example, `Alt+X` conflicts with `Alt+X+P`. The default General and Prompt chord prefixes stay reserved. Built-ins can be edited or reset but not deleted. Each profile selects Raw or Smart and may add a Smart prompt. `Shift` is only a modifier in the chord; only the final trigger’s down/up timing chooses dictation length.

Global detection does not suppress a chord’s prefix: its modifier and prefix key events pass through to the foreground application before the final trigger is recognized. Choose prefixes that are safe in the applications you use. Secure-input fields, operating-system-reserved shortcuts, and other system-level combinations are not guaranteed to work.

## Quick and Extended dictation

- **Quick:** the final trigger key goes down after the chord prefix and is released before 600 ms. Quick stays open until speech followed by the selected trailing silence (1.0, 1.8, or 3.0 seconds), `Enter`, the full activation chord again, or the two-minute cap. Its widget offers **Cancel**, not Stop.
- **Extended:** keep the final trigger key down for at least 600 ms. Extended streams transcription locally and does not submit on silence. Submit with `Enter`, the full activation chord again, or the widget’s **Stop** button. The cap is 30 minutes.
- Press `Escape` or use **Cancel** to cancel either mode.

The floating widget shows status and microphone level; Extended also shows elapsed time. Talking Quill attempts to paste the final text into the currently focused target. If native paste dispatch fails, the text remains copied for manual paste.

## Raw, Smart, and privacy

**Raw** keeps captured audio in memory and transcribes it with local Whisper without creating an audio recording file or making a Smart cleanup request. It matches the complete transcript against local Voice Commands, inserting a saved snippet on a match or the Raw transcript otherwise. Custom vocabulary does not affect Raw.

**Smart** also transcribes locally first; audio is not sent to the provider. Smart cleanup sends the transcript and cleanup context, including Smart-only vocabulary, an optional profile prompt, and an optional On-Screen Awareness screenshot. The request also uses the configured provider, model, and authentication. Provider failure or timeout falls back to Raw, although data may already have been sent.

A Voice Command match skips the Smart cleanup payload, so its transcript and prepared screenshot are not sent. Providers may run locally, on a LAN, or in the cloud; some uncredentialed LAN endpoints may use HTTP. Provider handling, retention, accounts, and charges are external.

### On-Screen Awareness

On-Screen Awareness is off by default and requires a vision-capable model. After recording stops and before Smart cleanup, it captures the full display containing the foreground app, excludes the widget, and sends a JPEG only with Smart. A Voice Command match prevents that prepared image from being sent.

A manually started vision/image-echo test also captures and sends a screenshot; the test image is not retained. Screenshot retention is a separate, off-by-default setting that can keep JPEGs and thumbnails for successful Smart history entries.

## Features and settings

- **Dashboard:** enable/disable dictation, inspect readiness, use a test field, and browse History with details, optional thumbnails, Copy, Delete, Delete all, and Load more.
- **General and profiles:** test shortcuts without recording or insertion; manage profiles, widget size, sounds, launch at login, and close to tray. The tray offers Open, Enable/Disable, and Quit; the window has themes and controls.
- **Recording and model:** select and live-test a microphone, choose silence detection, manage Whisper, and set language detection.
- **Smart processing:** configure/test providers, discover models where supported, and optionally enable vision. Pi adds installation-path and thinking-level settings.
- **Voice Commands and vocabulary:** manage and preview trigger/snippet pairs, Smart-only vocabulary, and text import/export.
- **Privacy and Info:** control future history, 7/30/90-day or no expiry, screenshots, diagnostics, and reset. Info includes manual updates, permissions, data/log folders, notices, and Welcome; updates are not automatic.

Supported Smart providers are OpenAI, Generic OpenAI, LM Studio, Local AI, KoboldCPP, Oobabooga Web UI, Docker Model Runner, Lemonade, Microsoft Foundry Local, oMLX, Groq, OpenRouter, Together AI, Fireworks AI, DeepSeek, Perplexity AI, Mistral, Novita AI, CometAPI, PPIO, APIpie, SambaNova, Cerebras, GiteeAI, Minimax, Moonshot AI, Z.AI, xAI, NVIDIA NIM, Privatemode, LiteLLM, Ollama, Pi, Anthropic, Google Gemini, Azure OpenAI, AWS Bedrock, and Cohere. Capabilities vary.

## Local data and network use

Settings, history, models, optional screenshots/diagnostics, temporary data, and the theme stay in Electron’s per-user application storage. Find it with **Info → Open data folder**. Credentials use Electron `safeStorage` and cannot be stored when secure storage is unavailable.

History is enabled by default with no automatic expiry; screenshot retention and diagnostics are off. Disabling history or diagnostics stops future collection but leaves existing data. **Reset all application data** removes Talking Quill’s local data and restarts the app, without removing external provider software or models. On Windows, uninstall keeps app data unless deletion is explicitly selected.

Network use includes requested, verified Hugging Face model downloads; provider discovery, tests, and Smart requests; manual GitHub update checks; and external Pi CLI/provider traffic.

## Development

Talking Quill uses Electron, React, TypeScript, and a Rust keyboard/paste helper. Helper protocol v3 configures up to ten profile bindings as `{ profileId, shortcut: { modifiers, keys } }` and reports activation down/up events with the same complete canonical shortcut snapshot. Prefix-conflict validation is shared across settings and helper configuration.

Run `pnpm install`, then `pnpm dev`, `pnpm build`, `pnpm typecheck`, `pnpm lint`, `pnpm test`, or `pnpm validate`. Packaging scripts build Windows and macOS x64/ARM64 artifacts.
