### Added
- Screen readers on Windows and Linux: with the `accessibility` feature the
  retained UI (`ctx.ui`) is given to UI Automation on Windows (Narrator,
  NVDA, JAWS) and to AT-SPI on Linux and the BSDs (Orca), through
  AccessKit's adapters, with the same tree, focus and requests (press,
  focus, set value, increment, decrement) as on macOS. The engine's window
  gets the adapter by itself; `UiAccessAdapter::SUPPORTED` is true on all
  three desktop systems.
- `UiAccessAdapter::for_hwnd` (Windows) and `UiAccessAdapter::for_atspi`
  (Linux and the BSDs), for hosts with a window of their own, next to
  `for_ns_view`; `UiAccessAdapter::set_window_bounds` tells AT-SPI where the
  window is.
- `tests/ui_access_platforms.rs`: a UI Automation client (Windows) and an
  AT-SPI client (Linux, under `dbus-run-session` in CI) read and operate the
  guide's sample menu.

### Changed
- When AT-SPI reports that assistive technology quit and came back, the
  adapter sends the whole tree again instead of only the changes.
- The event loop's pause while a screen reader reads the window is taken
  only where the system asks through the event loop (macOS, Windows), not
  for AT-SPI, which is answered on AccessKit's own thread.

### Fixed
- Static text (`UiRole::Label`) is given to AccessKit as the node's value,
  where UI Automation and AT-SPI read a label's name; with only a label, it
  would have been read without its text there.
