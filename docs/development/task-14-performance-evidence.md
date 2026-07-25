# Task 14 packaged performance measurement (local engineering evidence)

This is local engineering evidence for the exact tested source below. It is not signed-candidate,
clean-VM, macOS, or release-approval evidence, and it does not change the Task 14 tracker.

## Binding and host

- Tested source: `aebba67720cbe1420d652f62524f6d95df9725ba`
- Package: Windows x64 unpacked directory built immediately from that clean source with
  `pnpm package:win:dir`; package inspection passed (156 ASAR entries, 19 resources, 94 physical
  entries, 14 native images, hardened Electron fuses, canonical provenance).
- Host: Microsoft Windows 11 Home 10.0.26200 build 26200, x64; AMD Ryzen 9 7900X (12 cores/24
  logical processors); 64 GiB RAM.
- Measurement: five separate launches, a new profile per launch, model absent/unloaded, five
  seconds settling after Ready and CDP disconnection.
- Command: `pnpm performance:packaged -- --runs=5 --settle-ms=5000
  --output=tmp/performance/aebba677.json`.

The harness uses the remote-debugging listener only to isolate the profile and observe the first
visible main heading. Playwright disconnects before settling and memory sampling. It does not use
fake media, change app behavior, disable a security control, or launch a Whisper worker. Each run owns a unique profile and records every descendant found in the stable sample. Cleanup is
in `finally`; it refreshes both parent-tree and unique-profile-marker ownership, kills the root tree
and every observed PID with bounded operations, and fails if an observed or marker-bearing
process survives.

## Contract results

The RAM contract is process-tree RAM with the model unloaded. For a Windows multi-process
application, the directly attributable resident metric is `Win32_PerfRawData_PerfProc_Process.
WorkingSetPrivate`: resident pages private to each process. The gate sums `WorkingSetPrivate`
across all owned descendants and compares it with 400 MiB. Shared resident pages are deliberately
reported through gross working set rather than charged repeatedly to every process; ordinary
Windows process counters do not expose a deduplicated unique shared-page total.

| Metric | Budget | Min | Median | Max | Result |
|---|---:|---:|---:|---:|---|
| Harness launch to visible Ready heading | 3,000 ms | 395 ms | 498 ms | 670 ms | PASS |
| Tree private working set | 400 MiB | 207.5 MiB | 210.4 MiB | 211.3 MiB | PASS |
| Gross summed working set/RSS (diagnostic) | — | 740.7 MiB | 742.9 MiB | 746.3 MiB | not a gate |
| Private committed bytes (diagnostic) | — | 415.3 MiB | 417.5 MiB | 418.3 MiB | not a RAM gate |

All five runs had the same ten-process topology. The representative 210.4 MiB private-working-set
run broke down as follows:

| Role | Count | Private working set | Gross working set | Private bytes |
|---|---:|---:|---:|---:|
| Electron main | 1 | 52.6 MiB | 128.9 MiB | 94.7 MiB |
| Renderers | 3 | 99.8 MiB | 286.8 MiB | 131.1 MiB |
| GPU | 1 | 26.2 MiB | 93.5 MiB | 88.4 MiB |
| Network service | 1 | 7.7 MiB | 43.1 MiB | 12.5 MiB |
| Audio service | 1 | 5.5 MiB | 69.5 MiB | 9.9 MiB |
| Video capture service | 1 | 17.2 MiB | 106.1 MiB | 78.5 MiB |
| Native helper | 1 | 0.6 MiB | 5.3 MiB | 1.1 MiB |
| Helper console host | 1 | 0.8 MiB | 7.5 MiB | 1.2 MiB |

The Ready observation recorded all three expected renderer documents (`main`, `widget`, and
`capture`). Chromium does not expose a renderer-document-to-OS-PID mapping through ordinary
process counters after CDP disconnects, so individual renderer PIDs are deliberately not assigned
possibly incorrect window roles. The full generated JSON retains PID, PPID, command line, role,
and all three counters for every run.

## Why the earlier result was over budget

The ignored exploratory script `tmp/measure-packaged.cjs` labeled
`PrivateMemorySize64` as `privateMiB`. That counter is **private committed virtual memory**, not
resident private working set. It includes private pages that are not resident. Its three earlier
runs reported 403.5–410.5 MiB and the bound run above reports 415.3–418.3 MiB, so applying the RAM
limit to it produces the reported failure.

The same script also summed `WorkingSet64` (approximately per-process RSS). Electron processes share Chromium code, DLL, mapped-file, and shared-memory pages, so the
740.7–746.3 MiB gross sum counts many physical pages more than once. It is an upper bound, not a
unique process-tree footprint. Summing `WorkingSetPrivate` does not double-count shared resident
pages and leaves at least 188.7 MiB of attributable-resident headroom in the worst run.

A production launch without remote debugging was also sampled as a control after the bound
package build. It held the same ten runtime roles and no Whisper worker at 197.5 MiB private working
set, 690.6 MiB gross summed working set, and 364.1 MiB private bytes across five samples. That
control used the normal existing host profile rather than a fresh isolated profile, so it is not
the contractual result; it shows that CDP/profile-test overhead did not create a false pass (the
isolated measurement is the more conservative resident result).

No stale children were present between runs, no external Playwright/Node process was included in
the owned application tree, and no Whisper utility process existed in any sample. The reported
overage was therefore a metric-selection bug, not duplicated stale processes or excessive private
resident runtime cost. A deduplicated total that charges shared pages exactly once would require a
lower-level page-attribution tool; gross RSS cannot supply that value.

## Optimization decision and validation

The worker is already lazy and absent while idle. The helper and console host together consume
1.4 MiB private working set. The three sandboxed renderer roles preserve an always-ready first
capture, widget, and isolated microphone partition. With a stable worst case of 211.3 MiB, lazily
creating or merging those security/session roles would add first-use and lifecycle risk without
being required by the contract. No production behavior was changed merely to lower a diagnostic
counter.

Validation at the tested source:

- focused metric unit tests: 3 passed;
- formatting, ESLint, and strict TypeScript: passed;
- Windows x64 production build, helper build, directory package, and package inspection: passed;
- packaged smoke: 1 passed, including all renderer/preload isolation checks, microphone first use,
  shutdown, and persisted restart;
- source lifecycle/session E2E: 15 passed, including Welcome-to-first-dictation, real capture
  teardown, deterministic Quick/Extended sessions, widget/insertion/teardown, history, Smart/OSA,
  security, and close lifecycle.
