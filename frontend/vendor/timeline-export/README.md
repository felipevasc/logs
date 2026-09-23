Offline browser dependencies for timeline image/PDF export.

- html-to-image 1.11.13 — https://github.com/bubkoo/html-to-image — MIT
- jsPDF 4.2.1 — https://github.com/parallax/jsPDF — MIT

Licenses accompany the bundles. Versions are locked in the root package-lock.json.
To reproduce these files: `npm ci`, then `npm run vendor:timeline`.
The app loads only these local bundles; exporting never fetches a CDN or uploads logs.
APIs used: htmlToImage.toCanvas with embedded local fonts, jsPDF.addImage/addPage/output.
