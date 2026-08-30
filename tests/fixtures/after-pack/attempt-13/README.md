# Attempt 13 afterPack fixture

These files came from the failed `0.0.68` Windows x64 release attempt 13 output in `release/win-unpacked`.

- `app.asar-header.json` is the complete parsed header returned by `@electron/asar` `getRawHeader()` for `resources/app.asar`. The archive SHA-256 was `0ab4aea9cebbe2d5d46685100effb95ce460bb76055611056a553b63dcfda495`.
- `builder-debug.yml` is the generated electron-builder matcher configuration. Its SHA-256 is `88d71d9b3055694be067d62303c38ebb32f78b2fd8d817df672b79be2aca4dd1`.

The generated dependency matcher excluded the ONNX native tree. The platform matcher selected `win32/x64`, but the ASAR header contains no ONNX `bin` entry. afterPack therefore saw a genuinely absent native payload. Its first error happened to name the synthetic `win32` parent because the old required-path list treated directory metadata as required runtime content.
