import { defineConfig } from "vite";
export default defineConfig({
    root: "ui",
    base: "./",
    clearScreen: false,
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
