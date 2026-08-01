# Talking Quill user guide

Talking Quill is local-first, system-wide dictation for Windows and macOS, packaged for x64 and ARM64. Local Whisper transcribes speech, then Talking Quill attempts to paste the result into the focused target.

Raw stays local. Smart can send the local transcript and context to a configured provider. Talking Quill needs no account, but a provider may require an account, credentials, or payment.

## Set up

1. Install the package for your operating system and processor, then open Talking Quill.
2. In Welcome, allow microphone access and run the microphone test.
3. Choose the spoken/source language, then download and verify a local model: Large v3 Turbo is about 1.09 GB; Small is about 250 MB.
4. Optionally configure and test Smart processing. Skip it to use Raw dictation only.
5. Review the four built-in Smart reset defaults and their shortcuts on the Ready screen.

On macOS, allow Accessibility and Input Monitoring for shortcuts and text insertion. Screen Recording is needed only for On-Screen Awareness. **Settings** and **Info** can show Welcome again.

## Profiles and shortcuts

Put the cursor in a text field and use a profile shortcut:

| Profile | Shortcut | Mode |
| --- | --- | --- |
| General | `Alt+X` on Windows, `Option+X` on macOS | Smart |
| Prompt | `Alt+X+P` on Windows, `Option+X+P` on macOS | Smart |
| Markdown | `Alt+X+M` on Windows, `Option+X+M` on macOS | Smart |
| Translate to English | `Alt+X+E` on Windows, `Option+X+E` on macOS | Smart |

Talking Quill has four built-ins and up to twelve profiles total (eight custom profiles). A shortcut chord uses one or more modifiers plus one or more ordered, simultaneously held A–Z keys. The built-in family intentionally shares `Alt/Option+X`: keep X held while pressing P, M, or E for the longer profiles, then release the final suffix and X. The last letter pressed is the **final trigger**. Focus a profile’s shortcut field, hold the modifiers and letter keys in order, then release them. `Tab` and `Shift+Tab` leave the field normally.

Every full chord must be unique. Chords with the same modifiers also cannot prefix one another, except for the exact built-in `Alt/Option+X` default family. For example, unrelated `Alt+A` and `Alt+A+B` still conflict. Custom shortcuts and edited built-in shortcut values cannot claim any noncanonical `Alt/Option+X`-prefixed chord; only the four exact defaults are allowed for their built-in owners. Every default built-in chord prefix stays reserved. Built-ins can be edited or reset but not deleted. Each profile selects Raw or Smart and may add a Smart prompt. `Shift` is only a modifier in the chord; only the final trigger’s down/up timing chooses dictation length.

**Version 21 profile migration:** The upgrade preserves the existing General processing mode and its compatibility mirror; it never changes a Raw user to Smart or enables provider processing, including after completed setup with a cloud provider configured. Fresh installs and an explicit General reset use the new Smart default. Exact old General and Prompt defaults receive the new chord data and may receive updated, privacy-safe prompt text, but the prompt remains dormant while General is Raw.

The four built-ins permanently reserve their reset-safe `Alt/Option+X`, `Alt/Option+X+P`, `Alt/Option+X+M`, and `Alt/Option+X+E` defaults. If an existing noncanonical profile used `Alt/Option+X` or any same-modifier chord with that prefix, the upgrade preserves that profile’s ID, name, processing mode, and Smart prompt but deterministically assigns its shortcut to the first free `Alt/Option+A–Z` chord. This reassignment is unavoidable because retaining the collision would make the current profile list invalid and make a later built-in reset unsafe. Review migrated shortcuts in **Settings → Dictation profiles**.

Global detection does not suppress a chord’s prefix: its modifier and prefix key events pass through to the foreground application before the final trigger is recognized. For the built-in family, `Alt/Option+X` therefore reaches the foreground app; General resolves atomically when X is released if no built-in suffix was pressed. Choose prefixes that are safe in the applications you use. Secure-input fields, operating-system-reserved shortcuts, and other system-level combinations are not guaranteed to work.

## Quick and Extended dictation

- **Quick:** the final trigger key goes down after the chord prefix and is released before 600 ms. By default, Quick finishes after speech followed by the selected trailing silence (1.0, 1.8, or 3.0 seconds). Automatic finishing can be disabled so an explicit `Enter`, the full activation chord again, or the two-minute safety cap is required. Its widget offers **Cancel**, not Stop.
- **Extended:** keep the final trigger key down for at least 600 ms. Extended streams transcription locally and does not submit on silence. Submit with `Enter`, the full activation chord again, or the widget’s **Stop** button. The cap is 30 minutes.
- Press `Escape` or use **Cancel** through recording, transcription, Smart processing, and pre-commit insertion. Once native paste commits, Talking Quill finishes clipboard restoration rather than attempting an unsafe undo.

Because General's `Alt/Option+X` is also the family prefix, the helper resolves General when X is released and classifies Quick or Extended from the physical hold duration. Start speaking after the widget appears. The longer P/M/E chords resolve on their suffix key down as usual.

The floating widget shows status and microphone level; Extended also shows elapsed time. Talking Quill attempts to paste the final text into the currently focused target. If native paste dispatch fails, the text remains copied for manual paste.

## Raw, Smart, and privacy

**Raw** keeps captured audio in memory and transcribes it with local Whisper without creating an audio recording file or making a Smart cleanup request. Choose the spoken/source language during Welcome model setup or later in Settings: the pinned local runtime requires this explicit value and does not auto-detect it. Whisper always uses its `transcribe` task, so Raw remains in the source language and never uses Whisper's translate-to-English mode. Raw matches the complete transcript against local Voice Commands, inserting a saved snippet on a match or the Raw transcript otherwise. Custom vocabulary does not affect Raw.

**Smart** also transcribes locally in the configured source language first; audio is not sent to the provider. General, Markdown, and Prompt ask the Smart model to preserve that language. An ordinary Smart profile with no instruction receives a source-language-preservation fallback. Translate to English asks the Smart model—not Whisper—to translate the locally transcribed text to English, and any explicit profile transformation instruction replaces the fallback. Smart cleanup sends the transcript and cleanup context, including Smart-only vocabulary, saved Voice Command trigger phrases (never their snippets), an optional profile prompt, and an optional On-Screen Awareness screenshot. The request also uses the configured provider, model, and authentication. Provider failure or timeout falls back to Raw, although data may already have been sent.

An exact local Voice Command match skips the Smart cleanup payload, so its transcript and prepared screenshot are not sent. Close, cross-language, or naturally embedded command requests are checked by Smart processing for clear invocation intent; surrounding request language is allowed, while merely discussing or quoting a trigger should remain ordinary dictation. Providers may run locally, on a LAN, or in the cloud; some uncredentialed LAN endpoints may use HTTP. Provider handling, retention, accounts, and charges are external.

### On-Screen Awareness

On-Screen Awareness is off by default and requires a vision-capable model. After recording stops and before Smart cleanup, it captures the full display containing the foreground app, excludes the widget, and sends a JPEG only with Smart. A Voice Command match prevents that prepared image from being sent.

A manually started vision/image-echo test also captures and sends a screenshot; the test image is not retained. Screenshot retention is a separate, off-by-default setting that can keep JPEGs and thumbnails for successful Smart history entries.

## Features and settings

- **Dashboard:** enable/disable dictation, inspect readiness, use a test field, and browse History with details, optional thumbnails, Copy, Delete, Delete all, and Load more.
- **General and profiles:** test shortcuts without recording or insertion; manage profiles, widget size, sounds, launch at login, and close to tray. The tray offers Open, Enable/Disable, and Quit; the window has themes and controls.
- **Recording and model:** select and live-test a microphone, require manual finishing or choose silence detection, and opt into Windows system-audio capture for calls and other apps. System audio is off by default and currently unavailable on macOS. Manage Whisper and choose the spoken/source language here too.
- **Smart processing:** configure/test providers, discover models where supported, and optionally enable vision. Pi adds installation-path and thinking-level settings.
- **Voice Commands and vocabulary:** manage and preview trigger/snippet pairs, Smart-only vocabulary, and text import/export.
- **Privacy and Info:** control future history, 7/30/90-day or no expiry, screenshots, diagnostics, and reset. Info includes update status, permissions, data/log folders, notices, and Welcome. Installed releases check the public GitHub release feed at startup. Windows asks before downloading and installing; unsigned macOS releases provide a manual release-page fallback because a stable Developer ID signature is required for safe automatic replacement.

Supported Smart providers are OpenAI, Generic OpenAI, LM Studio, Local AI, KoboldCPP, Oobabooga Web UI, Docker Model Runner, Lemonade, Microsoft Foundry Local, oMLX, Groq, OpenRouter, Together AI, Fireworks AI, DeepSeek, Perplexity AI, Mistral, Novita AI, CometAPI, PPIO, APIpie, SambaNova, Cerebras, GiteeAI, Minimax, Moonshot AI, Z.AI, xAI, NVIDIA NIM, Privatemode, LiteLLM, Ollama, Pi, Anthropic, Google Gemini, Azure OpenAI, AWS Bedrock, and Cohere. Capabilities vary.

## Local data and network use

Settings, history, models, optional screenshots/diagnostics, temporary data, and the theme stay in Electron’s per-user application storage. Find it with **Info → Open data folder**. Credentials use Electron `safeStorage` and cannot be stored when secure storage is unavailable.

History is enabled by default with no automatic expiry; screenshot retention and diagnostics are off. Disabling history or diagnostics stops future collection but leaves existing data. **Reset all application data** removes Talking Quill’s local data and restarts the app, without removing external provider software or models. On Windows, uninstall keeps app data unless deletion is explicitly selected.

Network use includes requested, verified Hugging Face model downloads; provider discovery, tests, and Smart requests; startup and manual GitHub update checks; a Windows release download only after update consent; and external Pi CLI/provider traffic.

## Development

Talking Quill uses Electron, React, TypeScript, and a Rust keyboard/paste helper. Helper protocol v6 configures up to thirteen profile bindings as `{ profileId, shortcut: { modifiers, keys } }`, uses `off | recording | cancel-only` session-key capture modes, and reports exact activation down/up snapshots or an atomic General-prefix completion with its physical hold duration. Prefix-conflict validation, canonical family ownership, and the narrow built-in-family exception are enforced across settings and helper configuration. Local Whisper worker protocol v2 requires an explicit validated source language; the worker fixes the inference task to `transcribe`.

Run `pnpm install`, then `pnpm dev`, `pnpm build`, `pnpm typecheck`, `pnpm lint`, `pnpm test`, or `pnpm validate`. Packaging scripts build Windows and macOS x64/ARM64 artifacts.
