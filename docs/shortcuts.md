# Shortcuts and dictation profiles

Talking Quill uses dictation profiles. Each profile has one exact, distinct global binding:
**Alt** on Windows or **Option** on macOS, optional **Shift**, and one letter A–Z. Shift is part of the binding; it never changes a profile's processing mode.

Two nondeletable built-ins are always available and can be edited or reset independently. Each default chord remains reserved for its owning built-in even after editing, so reset cannot conflict:

- **General** — Alt/Option+Z, Raw processing.
- **Prompt** — Alt/Option+Shift+Z, Smart processing, with a default preference that makes dictated prompts focused, concise, clear, nonduplicative, information-dense, and as short as possible while retaining readable structure and useful lists, tables, or formatting.

Create, edit, and delete custom profiles in Settings → Dictation profiles. Every exact binding must be unique, and custom profiles cannot use either reserved built-in default chord. A profile also has its own Raw/Smart mode and optional Smart formatting prompt. Reset restores only the selected built-in.

- Press and release a profile binding before 600 ms for Quick Dictation. It submits on configured trailing silence, Enter, or another configured binding. Escape cancels.
- Hold at least 600 ms for Extended Dictation. Release and continue speaking; submit with the widget Stop button or another configured binding. Escape or Cancel discards the session.

The profile is snapshotted when recording starts. Editing or resetting it cannot change an active session. Profile Smart prompts are preferences only: the core safety, no-instruction-following, and same-language rules always take precedence.

Cancellation inserts and stores nothing. Smart provider failures automatically insert the Raw transcript and are visibly marked as fallback. Configured activation combinations are swallowed; unrelated keys are never intercepted outside an active session.
