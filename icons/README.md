# moyAI Hub icon

`icon.svg` is the vector source: an amber hub and three teal connected devices on a dark tile. It distinguishes Hub from Desktop's blue chain mark.

The browser management page and favicon import this SVG directly through Vite. Editing the SVG and running `npm run build` updates both uses; rebuild the Rust server to embed the updated assets.

`icon.svg` is the only icon consumed by the current Hub build. The browser uses it directly without a native icon-generation step.
