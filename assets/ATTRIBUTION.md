# Icon attribution

`aiallowance.ico` is a byte-for-byte copy of the Aven Solutions starburst
favicon, retrieved on 2026-11-16 from https://avensolutions.ai/favicon.ico.
The site's HTML head declares it via `<link rel="shortcut icon" href="/favicon.ico">`.

The file already contains 48x48, 32x32 and 16x16 32-bit images, which covers
standard and scaled Windows notification-area sizes.

The `source/` directory preserves the original assets as retrieved:

- `source/favicon.ico` - https://avensolutions.ai/favicon.ico
- `source/favicon.svg` - https://avensolutions.ai/favicon.svg (vector original)
- `source/favicon-96x96.png` - https://avensolutions.ai/favicon-96x96.png
- `source/apple-touch-icon.png` - https://avensolutions.ai/apple-touch-icon.png

The Aven Solutions mark is the property of Aven Solutions and is used here as
the project icon at the maintainer's request. It is not covered by the
repository's MIT code license.

## Installer artwork

The installer and uninstaller use `aiallowance.ico` for their application
icons. `installer/sidebar.bmp` (164x314) and `installer/header.bmp` (150x57)
place the original turquoise starburst from `source/favicon-96x96.png` on
white backgrounds. Regenerate these 24-bit NSIS bitmaps on Windows with
`.\installer\generate-artwork.ps1` from the repository root.

## Provider marks

Provider marks were retrieved from their original sites on 2026-11-18:

- `providers/claude.svg`: https://claude.ai/favicon.svg
- `providers/chatgpt.svg`: https://chatgpt.com/cdn/assets/favicon.6a33781f61.svg
  (declared in the chatgpt.com page head; its colour-scheme stylesheet was
  removed so the mark renders in its light-theme black)
- `providers/copilot.svg`: https://github.githubassets.com/favicons/favicon-copilot.svg

Each SVG was rendered with resvg into a multi-size ICO (16 to 256 pixels,
PNG-compressed) so Windows can load the exact size it draws. The SVG
sources are retained alongside the ICO files.

These marks identify the respective providers. They remain the property of
Anthropic, OpenAI and GitHub, and are not covered by this project's MIT license.
