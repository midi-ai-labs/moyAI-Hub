import { defineConfig } from "vite";
import packageJson from "./package.json";
export default defineConfig({
    root: "ui",
    base: "./",
    clearScreen: false,
    plugins: [{
        name: "hub-release-version",
        transformIndexHtml: html => html.replaceAll("__MOYAI_HUB_VERSION__", packageJson.version)
    }],
    server: {
        host: "127.0.0.1",
        port: 1421,
        strictPort: true
    },
    build: {
        outDir: "dist",
        emptyOutDir: true,
        target: "es2022"
    }
});
