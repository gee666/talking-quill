![Talking Quill dashboard](docs/assets/talking-quill-dashboard.png)

# Talking Quill

> **Keyboard-owner status:** Windows packages contain two native roles: the non-suppressing gateway and the same-interactive-user detached owner. Windows installs no service, LocalSystem role, enrollment, maintenance executable, or ProgramData runtime. macOS includes its LoginItem and maintenance bridge. Any package identity, authentication, maintenance, or native-health mismatch fails closed.

**Talk naturally. Get useful text wherever you are typing.**

Talking Quill is a dictation app for Windows and macOS. Once the I1 safety gates pass and activation capture is enabled, a shortcut will start dictation and put the result into the app you were already using—an email, a document, a chat window, a terminal, a browser, or almost any other text field.

The speech-to-text part runs on your own computer. You can keep the result exactly as it was transcribed, or optionally let a Smart profile tidy it up, format it, translate it, or reshape it using an AI provider you choose.

## Download and install

Open the **[latest Talking Quill release](https://github.com/gee666/talking-quill/releases/latest)**. You only need one installer for your computer; the other files on the release page are updater and verification files.

### Windows

Choose one of these installers:

- **Most Windows computers:** `Talking-Quill-<version>-win-x64.exe`
Talking Quill supports Windows x64 and ARM64, plus macOS x64 and ARM64.

Download the matching `.exe`, open it, choose where to install Talking Quill, and follow the prompts.

The current release is unsigned, so Windows SmartScreen may show **Windows protected your PC**. If you downloaded the installer from this repository's release page, choose **More info -> Run anyway** to continue. Windows then shows a **User Account Control (UAC)** prompt because the per-machine installer writes Program Files and removes obsolete service state. Confirm the publisher is **Unknown publisher** only when you intentionally downloaded this unsigned local build, then choose **Yes**. Talking Quill does not claim Authenticode signing.

### macOS

Choose one of these disk images:

- **Apple Silicon (M1, M2, M3, M4, and newer):** `Talking-Quill-<version>-mac-arm64.dmg`
- **Intel Mac:** `Talking-Quill-<version>-mac-x64.dmg`

You can check by opening **Apple menu → About This Mac**. If it shows **Chip: Apple…**, use ARM64. If it shows an Intel processor, use x64.

Open the `.dmg`, then drag **Talking Quill** into **Applications**. The `.zip` files on the release page are selected automatically by the architecture-specific updater; the DMG is the easier way to install the app yourself. Updater metadata binds the exact ZIP, native-role layout, and one-hop predecessor before owner maintenance begins.

The current release is unsigned, so macOS may refuse the first launch. In Finder, open **Applications**, Control-click **Talking Quill**, choose **Open**, and confirm. If macOS still blocks it, go to **System Settings → Privacy & Security** and choose **Open Anyway** for Talking Quill.

### Optional: verify your download

The release includes `SHA256SUMS.txt`. You can compare its checksum with your downloaded installer:

```powershell
# Windows PowerShell
Get-FileHash .\Talking-Quill-<version>-win-x64.exe -Algorithm SHA256
```

```bash
# macOS
shasum -a 256 Talking-Quill-<version>-mac-arm64.dmg
```

## Getting started

The first-run setup walks you through the important parts:

1. Allow microphone access and pick the microphone you want to use.
2. Choose the language you normally speak.
3. Download a local speech model. The Small model is roughly 250 MB; Large v3 Turbo is roughly 1.09 GB and is usually more accurate.
4. Configure and test your shortcut in the built-in editor test box. On Windows, global capture becomes available only after the same-session owner authenticates over its private inherited channel and completes disabled-first reconciliation.
5. Optionally connect a Smart processing provider. You can skip this and use fully local Raw dictation.

On a Mac, Talking Quill also needs **Accessibility** and **Input Monitoring** permission so the global shortcuts work and the finished text can be inserted into other apps. Screen Recording permission is only needed if you turn on On-Screen Awareness.

## What it can do

### Dictate into the apps you already use

With an enrolled, healthy Windows keyboard owner, you will not have to open a special editor: put the cursor where you want the text, use a Talking Quill shortcut, and start speaking. A small floating widget shows when it is listening, transcribing, or cleaning up the result.

A quick press is good for a sentence or short note. Hold the final shortcut key a little longer for Extended dictation, which keeps listening through pauses and can handle longer thoughts. In an installed local-owner build, **Enter** or the repeated shortcut finishes and **Escape** cancels. The detached owner remains responsible for balancing any key it captures if Electron or its gateway exits.

### Keep everything local—or clean it up with Smart processing

Every recording is transcribed locally with Whisper. Audio is not sent to an AI provider.

- **Raw** inserts the local transcript without sending it to a Smart provider.
- **Smart** sends the transcript—and only the optional context you have enabled—to your chosen provider so it can remove filler words, fix punctuation, organize ideas, follow a custom instruction, or translate the text. With Pi selected, Talking Quill may start the configured Pi process and extensions while you are listening to reduce submission latency, but it does not send the transcript until you submit the dictation.

If Smart processing fails, Talking Quill falls back to the Raw transcript instead of losing what you said.

### Use profiles for different kinds of writing

Talking Quill starts with four profiles:

- **General** for everyday writing
- **Prompt** for turning a spoken idea into a clearer AI prompt
- **Markdown** for headings, lists, code-friendly formatting, and structured notes
- **Translate to English** for speaking in your configured language and receiving English text

You can change their shortcuts and behavior, or create your own profiles. A profile can stay Raw, use Smart cleanup, or include its own instruction—something like “turn this into a friendly email,” “write concise meeting notes,” or “keep my wording but fix punctuation.”

### Choose how recording behaves

You can select a specific microphone, watch a live input meter, and test the device before using it. Quick dictation can finish automatically after a short, medium, or longer pause, or you can make finishing completely manual.

On Windows, there is also an optional system-audio mode for transcribing calls, meetings, or other audio playing on the computer. It is off by default and is not currently available on macOS.

### Add Voice Commands and useful snippets

Voice Commands let a short phrase insert saved text. You might say “my email address,” “meeting link,” or “support reply” and have Talking Quill insert the snippet you saved. Exact command matches are handled locally and skip Smart processing.

### Help Smart processing understand your words

Custom Vocabulary gives Smart profiles extra context for names, product terms, abbreviations, and phrases that are easy to misread. Entries can be added by hand or imported from a text file, and you can export them again whenever you want.

### Let the AI see the current screen—only when you choose

On-Screen Awareness can take a screenshot of the display containing the app you are using and include it with a Smart request. This can help with requests such as “reply to this message” or “turn what is on screen into a checklist.”

It is off by default, requires a vision-capable model, and is never used by Raw dictation. Screenshot history is a separate setting and is also off by default.

### Keep a useful history without giving up control

Talking Quill can keep a local history of your finished dictations so you can copy something again later. You can delete one entry, clear everything, disable future history, or automatically remove entries after 7, 30, or 90 days. Optional screenshot thumbnails have their own setting.

### Pick the provider that suits you

Smart processing works with local tools, services on your network, and cloud providers. Options include Ollama, LM Studio, OpenAI, Anthropic, Gemini, OpenRouter, Azure OpenAI, AWS Bedrock, Pi, and many OpenAI-compatible services. Provider availability, cost, privacy, and model support depend on the service you choose.

### Make it feel at home on your computer

You can change the widget size, sounds, theme, launch-at-login behavior, and whether closing the window keeps Talking Quill in the tray. The dashboard shows whether the microphone, local model, keyboard helper, and Smart provider are ready before you start.

## Privacy in plain language

- Speech recognition happens on your computer.
- Raw dictation does not send the transcript to a Smart provider.
- Smart dictation sends the transcript and the Smart context you enabled to the provider you selected only after submission. With Pi, the configured Pi process and extensions may start while you are listening, before any transcript is sent.
- Audio is not sent to that provider.
- On-Screen Awareness is optional and off by default.
- History, screenshot retention, and diagnostic logging can all be controlled separately.
- Credentials are stored using the operating system’s secure storage.

Talking Quill does use the network when you download a speech model, test or use an online Smart provider, or check for an app update.

For a detailed walkthrough of profiles, shortcuts, privacy settings, providers, and recording modes, see the **[Talking Quill user guide](docs/README.md)**.

## Personal-use Windows and macOS packages

Build and install a fresh unsigned Windows x64/ARM64 or macOS Intel/Apple Silicon package with the commands in [`docs/personal-use-install.md`](docs/personal-use-install.md). That guide covers SmartScreen/UAC, local macOS signing, privacy grants, first run, verification, repair, rollback, uninstall, troubleshooting, and friend-Mac transfer. Fresh local installation needs no predecessor or promotion evidence; updater packages still require exact predecessor metadata and authenticated publication gates are unchanged.

For the detailed macOS owner security/lifecycle contract, see [`docs/macos-local-owner-install.md`](docs/macos-local-owner-install.md). Ad-hoc builds may require permission reauthorization after every changed binary.

## License

Talking Quill is available under the [MIT License](LICENSE). Third-party components and model notices are included with the app and attached to each release.
