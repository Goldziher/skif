# skif docs site

The documentation site for [skif](https://github.com/Goldziher/skif), built with
[Astro Starlight](https://starlight.astro.build) and deployed to GitHub Pages by
`.github/workflows/docs.yaml` at <https://goldziher.github.io/skif/>.

## Develop

```bash
pnpm install
pnpm run dev      # local dev server at http://localhost:4321/skif/
pnpm run build    # static build into ./dist
pnpm run preview  # serve the built site
```

## Layout

- `src/content/docs/**` is the documentation (Markdown / MDX). Internal links are relative because the site is
  served under the `/skif` base path.
- `astro.config.mjs` holds the site config, sidebar and the `starlight-llms-txt` plugin (generates `/llms.txt`,
  `/llms-small.txt` and `/llms-full.txt`).
- `src/styles/custom.css` is the brand theme.
- `scripts/sync-rules.mjs` renders `../docs/rules.md` into one page per rule category under
  `src/content/docs/rules/` (`manifest`, `workspace`, `policy`, `catalog`, `js`). The generated `.mdx` files are
  git-ignored and the `dev`, `start` and `build` scripts run the script first; run `pnpm run sync-rules` to
  regenerate them by hand. `src/content/docs/rules/index.mdx` is hand-written. `../docs/rules.md` is generated
  from the rule metadata (`SKIF_UPDATE_DOCS=1 cargo test -p skif --features js-analysis --test docs`), so edit
  rule text in the Rust metadata, never in the generated pages.

## Branding assets to replace

The files below are placeholders (a plain "skif" wordmark or a solid colour). Replace them with the real
branding, keeping the file names so nothing else changes:

- `src/assets/logo.svg`: the header wordmark.
- `src/assets/mark.svg`: the hero image on the home page.
- `src/assets/banner.svg`: the wide banner.
- `public/favicon.svg`: the browser tab icon.
- `public/favicon-32.png`: 32x32 PNG favicon.
- `public/apple-touch-icon.png`: 180x180 PNG.
- `public/og.png`: 1280x640 social preview image.
- `src/styles/custom.css`: the accent palette (`--sl-color-accent*`, `--sk-*`).

If the site moves to a custom domain, set `site` in `astro.config.mjs`, drop `base`, update the absolute
`/skif/...` paths in its `head` entries, add `public/CNAME` and update the sitemap URL in `public/robots.txt`.

Content is sourced from the repository `README.md`, `docs/rules.md`, `adrs/` and the CLI and configuration
definitions; keep it in sync when those change.
