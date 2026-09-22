# dagr — brand assets

All assets are SVG. The mark is the **Dagaz rune (ᛞ)** of the Elder Futhark,
meaning "day" — two uprights joined by a crossing stroke.

## Files

| File | Use |
|---|---|
| `favicon.svg` | Rune only, tuned for tiny sizes (16–32px). Site favicon. |
| `logo.svg` | Mark + `dagr` wordmark, horizontal lockup, light backgrounds. Default for README. |
| `logo-dark.svg` | Same lockup for dark backgrounds. Use with `<picture>` for theme-aware READMEs. |
| `screenshot.png` | The app window, for the README and social preview. |
| `index.html` | Self-contained one-pager for `gh-pages`. No build, no JS deps. |

## README integration

```markdown
<picture>
  <source media="(prefers-color-scheme: dark)" srcset="site/logo-dark.svg">
  <img src="site/logo.svg" alt="dagr" width="300">
</picture>
```

## Colors

| Token | Light | Dark |
|---|---|---|
| Rune (uprights) | `#0f172a` (slate-900) | `#faf6ed` (cream) |
| Cross (accent) | `#d97706` (amber-600) | `#f59e0b` (amber-500) |
| Background | `#faf6ed` (cream) | `#0f172a` (slate-900) |

The app icon itself (`data/icons/.../nu.bles.dagr.svg`) is the same rune in
white on a GNOME-blue tile — that is the launcher/desktop identity, while these
amber-on-cream marks are the project/web identity, matching the sibling
[skald](https://skald.bles.nu) site.

## gh-pages deploy

```bash
git checkout --orphan gh-pages
git rm -rf .
cp site/index.html site/favicon.svg site/CNAME site/robots.txt site/sitemap.xml site/screenshot.png .
git add index.html favicon.svg CNAME robots.txt sitemap.xml screenshot.png
git commit -m "gh-pages: initial deploy"
git push origin gh-pages
```

Then in GitHub: Settings → Pages → source: `gh-pages` branch. The `CNAME`
points at `dagr.bles.nu`.
