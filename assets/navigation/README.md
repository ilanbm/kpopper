# README path banners

The three native `<summary>` elements use wide strips, displayed at up to 960 pixels
wide and scaled to fit smaller screens. The coding strip is 1800 × 360 (5:1);
the other two are 1800 × 300 (6:1). Product names and
**Click to expand workflows and examples ↓** remain ordinary text outside the banners.
The whole summary toggles its section, including a click on the banner. The banners
are not separate links to the image files.

GitHub otherwise wraps a bare image in an image-viewer link. Each banner therefore
has a named `<a id="...">` wrapper with no `href`: GitHub preserves that wrapper,
and it remains non-interactive so clicking the image activates its parent summary.

Path titles use small SVG labels aligned with the native disclosure marker. GitHub
removes inline CSS and wraps HTML headings in block elements, so a heading inside
`summary` places the marker on a separate line. The SVGs retain heading-sized text
without a block wrapper, provide the title as alt text, and adapt to light and dark
color schemes. The Cowork title has two inline parts so it can wrap between product
names on a narrow screen while the marker stays beside its first line.

| Banner | Source | Crop: left, top, right, bottom |
|---|---|---|
| [coding-banner.png](coding-banner.png) | [cache-privacy.png](../stories/cache-privacy.png) | Private-project screen: `98, 449, 570, 645`; cache author: `1045, 45, 1355, 280`; merged cache: `637, 446, 912, 645` |
| [cowork-banner.png](cowork-banner.png) | [cowork-workshop.png](../stories/cowork-workshop.png) | `64, 438, 1470, 651` |
| [research-banner.png](research-banner.png) | [dark-matter-intro.png](../stories/dark-matter-intro.png) | Galaxy: `47, 145, 295, 326`; lensing: `555, 124, 989, 318`; CMB: `1037, 143, 1408, 312` |

The coding strip puts the two PR changes on either side of the cache, using large
"PR A / Add private projects" and "PR B / Cache by query" labels above source-art crops.
Blue arrows point inward; a red failure mark and "Private results exposed" label
identify the consequence of their combination. This keeps the authored changes
distinct from the two user screens in the full story's consequence scene.
Remove clipped blue arrow fragments at the right edge of the private-project crop
and from the cache crop before adding the two complete inward arrows.
Cowork uses one continuous scene fitted inside 1780 × 284 with Lanczos resampling
and centered on white. The research strip places three
separately fitted crops in equal-width panels. A white mask at `0, 0, 290, 20` in
the lensing crop removes the clipped source citation while preserving the legend.
Use a thin black outer border and two panel dividers in the research strip.
Original story illustrations remain unchanged.

The earlier 320 × 200 thumbnails (`coding.png`, `cowork.png`, `research.png`) are
retained as assets; the README now uses the banners above.
