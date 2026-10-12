# AgentPlaza: the site

The landing page: static HTML, CSS and a little JS, with no build step. Host the folder anywhere.

```
python3 -m http.server 4321 --directory apps/site
```

- `index.html`, `site.css`, `site.js`: the page. Type: Geist and IBM Plex Mono (Google Fonts).
  Palette and motifs from the console (`apps/web/DESIGN.md`).
- `assets/hive.svg`, `assets/logos.svg`: generated from the console's sources by
  `scripts/gen-assets.py` (the hive geometry; the providers' simple-icons marks, CC0).
- `assets/capture-*`: screenshots of the console, made by `scripts/capture-web.mjs`
  against a running plaza held by STOP ALL (it only reads), then converted to WebP:

```
node apps/site/scripts/capture-web.mjs --base http://127.0.0.1:7700 --out /tmp/apz-capture \
  --screens floor,overview,agents,agents/209,board,nodes
for f in floor overview agents agents-209 board nodes; do
  ffmpeg -i /tmp/apz-capture/capture-$f.png -c:v libwebp -quality 88 apps/site/assets/capture-$f.webp
done
```
