import { defineConfig } from "@playwright/test";
import { randomUUID } from "node:crypto";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { GUI_SUITES } from "./scripts/gui-stage.mjs";

const repository = path.dirname(fileURLToPath(import.meta.url));
// Inherit one execution directory in Playwright workers. Never use a real Hub profile.
process.env.MOYAI_HUB_BROWSER_ARTIFACTS ??= path.resolve(repository,
    "../project_sandbox/hub-playwright", `${Date.now()}-${randomUUID()}`);
const artifacts = process.env.MOYAI_HUB_BROWSER_ARTIFACTS;
if (!path.isAbsolute(artifacts)) {
    throw new Error("MOYAI_HUB_BROWSER_ARTIFACTS must be an absolute execution directory");
}
const channel = process.env.MOYAI_HUB_BROWSER_CHANNEL ?? (process.platform === "win32" ? "msedge" : "chromium");
if (!["msedge", "chrome", "chromium"].includes(channel)) {
    throw new Error("MOYAI_HUB_BROWSER_CHANNEL must be msedge, chrome, or chromium");
}
// Direct Playwright remains the broad lower-level entry. The GUI phase chooses explicitly.
const suite = process.env.MOYAI_HUB_BROWSER_SUITE ?? "regression";
if (!Object.hasOwn(GUI_SUITES, suite)) throw new Error("MOYAI_HUB_BROWSER_SUITE must be smoke or regression");

export default defineConfig({
    testDir: "./tests/browser",
    testMatch: "**/*.spec.mjs",
    grep: GUI_SUITES[suite].grep,
    fullyParallel: false,
    workers: 1,
    retries: 0,
    forbidOnly: !!process.env.CI,
    timeout: 60_000,
    expect: { timeout: 10_000 },
    outputDir: path.join(artifacts, "results"),
    reporter: [["list"], ["html", { outputFolder: path.join(artifacts, "report"), open: "never" }],
        ["json", { outputFile: path.join(artifacts, "playwright-results.json") }]],
    use: {
        browserName: "chromium",
        channel,
        viewport: { width: 1440, height: 1000 },
        ignoreHTTPSErrors: false,
        trace: "retain-on-failure",
        screenshot: "only-on-failure",
    },
});
