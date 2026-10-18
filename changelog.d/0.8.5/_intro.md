### Changed

- **A setting still loading looks like a skeleton rather than an empty field.**
  The placeholder was the same control with its text made transparent, so a
  backend that was merely slow was indistinguishable from a setting with
  nothing chosen — two very different things to tell someone. It is now a
  filled bar in the same box, which reads as "not here yet".

  `field_control` split into `field_box` and the surface each state paints on
  it. The box is still shared, because a class on the placeholder and not on
  the control is what moved the page the last time these drifted; the surfaces
  are stated separately because two `bg-*` utilities on one element is a coin
  toss — equal specificity, so whichever Tailwind emits last wins.
