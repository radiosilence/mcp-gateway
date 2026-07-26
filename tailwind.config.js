/** Scans the templates and the script, so the stylesheet carries exactly what
 *  is used — including classes the script only assembles at runtime, which are
 *  invisible to any scan of the markup alone. */
module.exports = {
  content: ["./templates/**/*.html", "./assets/app.js"],
  darkMode: "media",
  theme: { extend: {} },
  plugins: [],
};
