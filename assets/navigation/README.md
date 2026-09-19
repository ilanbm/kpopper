# README path thumbnails

These previews are crops of the existing story illustrations, displayed at 112 × 70
pixels in native `<summary>` elements. The PNG files are 320 × 200 pixels. Each thumbnail
links to its full source illustration and has an alternative-text preview label. The
adjacent summary text names and opens the path.

| Preview | Source | Crop: left, top, right, bottom |
|---|---|---|
| [coding.png](coding.png) | [cache-privacy.png](../stories/cache-privacy.png) | `1050, 42, 1363, 282` |
| [cowork.png](cowork.png) | [cowork-workshop.png](../stories/cowork-workshop.png) | `861, 443, 1260, 647` |
| [research.png](research.png) | [dark-matter-intro.png](../stories/dark-matter-intro.png) | `48, 143, 294, 320` |

Crop the source, then fit it within 320 × 200 using Lanczos resampling and white padding.
The originals remain unchanged. Keep the summary text outside the image; the thumbnail
does not replace a label or carry new explanatory text.
