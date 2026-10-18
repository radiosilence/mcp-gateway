### Added

- **`node`, pinning Hydra and Postgres to one machine.** Every OAuth step is a
  database round trip, so the two belong together — and nothing was making that
  happen. An unconstrained pod goes wherever has the most unrequested capacity,
  which on a mixed cluster is usually the machine you least want it on: in the
  deployment this was found in, Hydra had drifted onto a laptop on a residential
  line while its Postgres stayed on the VPS, so every login crossed a home
  broadband link twice.

  Worse than slow, it was fragile: that link going down took sign-in out for
  every service in the estate, including ones running nowhere near it.

  Left unset the scheduler still chooses, which is the old behaviour — but the
  dynamically-provisioned volume then anchors Postgres wherever it first landed
  while Hydra stays free to drift away from it. Set it.
