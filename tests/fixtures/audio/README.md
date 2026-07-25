# Realistic deterministic speech fixtures

These fixtures are synthetic **speech**, generated locally with eSpeak NG 1.52.0 and converted to
16 kHz mono PCM16 WAV with FFmpeg 8.0. They contain no human recording and no downloaded audio.

## Reproduction and verification

From the repository root on Windows:

```powershell
node scripts/generate-audio-fixture.mjs --write
node scripts/generate-audio-fixture.mjs --check
```

The generator invokes this eSpeak executable and equivalent commands for every text clip:

```powershell
& 'C:/Program Files/eSpeak NG/espeak-ng.exe' -v en-us -s 145 -p 45 -a 150 -w tmp/audio-fixture-generation/<clip>.wav '<exact text below>'
ffmpeg -hide_banner -loglevel error -y -i tmp/audio-fixture-generation/<clip>.wav -af 'aresample=16000,...' -ac 1 -ar 16000 -c:a pcm_s16le -fflags +bitexact -flags:a +bitexact -map_metadata -1 tests/fixtures/audio/<fixture>.wav
```

For the 90-second fixture, the script creates an exact 90-second 16 kHz mono silence bed, delays
each clip by the millisecond offsets below, combines them with `amix` and `alimiter`, and writes
bit-exact PCM16. The complete FFmpeg filter graph is constructed deterministically in
`scripts/generate-audio-fixture.mjs`; all intermediate files live under ignored `tmp/`.

## Content

### `speech-short-16k-mono.wav`

Exact text:

> Talking Quill keeps every spoken word private and local. Clear speech becomes useful text.

The output is padded to exactly 8 seconds.

- SHA-256: `5e8257938418febb3687496a48ab80cac537c054cd4a52a749ade967bf8ee59a`
- Format: PCM16, mono, 16,000 Hz
- Samples: 128,000
- Duration: 8.000 seconds

### `speech-boundaries-90s-16k-mono.wav`

Exact scheduled content:

| Start | Text |
|---:|---|
| 5.000 s | The session begins with a calm natural sentence. |
| 15.000 s | We continue speaking at a steady pace. |
| 24.100 s | alpha boundary marker |
| 35.000 s | The middle section remains clear and conversational. |
| 44.100 s | bravo boundary marker |
| 55.000 s | Another ordinary sentence keeps the recording realistic. |
| 64.100 s | charlie boundary marker |
| 75.000 s | The final section continues without rushing. |
| 84.100 s | delta. delta boundary marker |

The unique anchors straddle the rolling ownership boundaries at approximately 25, 45, 65, and
85 seconds for 30-second windows with 5-second left/right overlap and a 20-second hop. Production
streaming tests require every anchor exactly once.

- SHA-256: `e1da4174bda6cc213003773fc6ef3327ec7462443d0e4fc0d951904e0b3b4c91`
- Format: PCM16, mono, 16,000 Hz
- Samples: 1,440,000
- Duration: 90.000 seconds

## Tool and license provenance

- eSpeak NG 1.52.0 is distributed under GPL-3.0-or-later and uses its bundled `en-us` voice data.
  The application does not bundle or execute eSpeak; only generated PCM test fixtures are
  committed. This provenance is retained so downstream distributors can perform their own legal
  review of generated test assets.
- FFmpeg is used only as a local build tool. The locally installed build reports its own enabled
  license configuration; FFmpeg is not bundled with the application.
- The spoken text was written specifically for this project.
