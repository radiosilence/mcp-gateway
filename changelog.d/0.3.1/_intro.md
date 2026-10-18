Released 2026-07-26.

### Added

- **Timing on both halves of the options fetch.** The 600ms render budget was a
  guess, and a guess with nothing measuring it stays one — worse, it failed
  quietly, since a page that came up short looked no different to one with
  nothing to fetch.

  Whatever beats the budget is logged as it arrives; whatever misses is timed by
  the endpoint that then serves it, which has to fetch the thing anyway. Both
  halves are covered without keeping abandoned work alive purely to measure it,
  and a render that came up short reports how many of how many. Enough to tell
  a budget that is too tight from a backend that is simply slow.
