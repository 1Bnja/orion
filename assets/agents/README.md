Agent logos, drawn for Orion (MIT, like the rest of the repo). They are simple shapes that only
evoke each tool; none of them is the official artwork. Names and brands belong to their owners.

Each SVG is white on a 24×24 viewBox, so Orion can tint it with the theme color. Orion rasterizes
them itself (`src/logo.rs`), which only reads a single `<path>` with space-separated absolute
`M`, `L` and `Z` commands (nonzero fill: a hole goes in the opposite direction).
