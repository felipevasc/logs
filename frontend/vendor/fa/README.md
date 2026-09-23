Font Awesome Free 7.3.1
=======================

The CSS and local WOFF2 fonts are bundled for offline use. The note picker uses
all 1,422 Free Solid icons and all 572 Brands from this same version.

`icon-catalog.json` is generated from the official version-pinned metadata:

- https://github.com/FortAwesome/Font-Awesome/blob/7.3.1/metadata/icons.json
- https://github.com/FortAwesome/Font-Awesome/blob/7.3.1/metadata/categories.yml

Run `npm run vendor:icons` from the repository root to regenerate it. The script
downloads missing metadata into the ignored `output/` cache, checks HTTP status,
and retains the pinned version. Normal application use never downloads icons or
metadata. The JSON contains official English names/search aliases, curated
Portuguese names for common icons, Portuguese search terms and grouped categories.

The Font Awesome Free license is included in `LICENSE.txt`: icons are licensed
under CC BY 4.0, fonts under SIL OFL 1.1, and code under MIT. Font Awesome is by
Fonticons, Inc. See https://fontawesome.com/license/free for the license details.
