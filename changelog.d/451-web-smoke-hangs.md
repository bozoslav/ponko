### Fixed
- The web smoke test's WebKit runs no longer hang CI for an hour. Headless
  WebKit on Linux crashed its content process when a game opened its audio at
  the first click, because the runner had no sound server, and the runner
  waited for the crashed page forever. CI now starts a PulseAudio server with a
  null sink before the WebKit runs and tells WebKit the output is stereo (its
  device scan reported 0 output channels there, so the engine's audio could
  not open), so the fixtures' music plays there too. `web/smoke/README.md`
  says how to do the same on a headless Linux machine.

### Changed
- The web smoke runner can no longer hang. Once the page is open every browser
  call has a 30 s deadline, a crashed page fails at once (`browser-crash`), and
  the whole run has `--run-timeout` (default 300 s). Both failures name the step
  the run was on and print the end of the browser's own output, which each run
  now saves as `browser.log`. In CI each browser step also has a
  `timeout-minutes`.
