### Added

- **The build serving the page, in the footer of both of them.** `Cargo.toml`'s
  version is what a release is here, so it is the same string the image tag and
  the GitHub release carry — and neither of those is visible from a browser.
  "Which build am I looking at" is the first question a dashboard behaving oddly
  raises, and answering it previously meant `kubectl` against the deployment.

  On the login page too, because that is the page in front of you when the
  dashboard is the thing that won't load.
