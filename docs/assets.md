# Rebuilding the stylesheet

`static/css/app.css` is generated. It contains the two webfonts and exactly
the Tailwind utilities the templates use — nothing else.

Everything the interface loads is served from the machine running it.
Upstream pulled Tailwind, HTMX, and Google Fonts from three CDNs on every
page load, which told each of those CDNs whenever someone opened a privacy
tool, and left the interface unstyled with no network connection.

## When to rebuild

After adding a Tailwind class to a template that no other template already
uses. If a class was never used before, it is not in the stylesheet and will
silently do nothing.

## How

Requires Node. Nothing is added to the repository except the output.

```bash
cd assets
npx tailwindcss@3 -c tailwind.config.js -i tailwind.css -o /tmp/tailwind.out.css --minify
```

Then put the font faces back in front of it — the generated file is
`fonts.css` followed by the Tailwind output. The font `@font-face` rules
currently in `static/css/app.css` can be copied across as-is; they point at
`/static/fonts/` and do not change.

## The fonts

Outfit and Plus Jakarta Sans, latin and latin-ext subsets, as woff2, in
`static/fonts/`. They were fetched once from Google Fonts and committed.
They are not refetched at runtime and should not be.

To update them, request the stylesheet with a modern browser user agent
(otherwise Google serves ttf instead of woff2), download each URL it names,
and rewrite the `src:` paths to `/static/fonts/`:

```bash
curl -A "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/120.0 Safari/537.36" \
  "https://fonts.googleapis.com/css2?family=Outfit:wght@400;500;600;700&family=Plus+Jakarta+Sans:wght@400;500;600&display=swap"
```

## HTMX

`static/js/htmx.min.js` is HTMX 1.9.10, vendored from upstream. Replacing it
means dropping in a newer release and checking the pages still work; there is
no build step.
