import { spawn } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { createReadStream, createWriteStream } from "node:fs";
import { access, mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repository = fileURLToPath(new URL("../", import.meta.url));
const stageNames = ["unit", "frontend-build", "rust-build", "browser-e2e"];
export const GUI_SUITES = Object.freeze({ smoke: { grep: /@smoke\b/, expected: 3 }, regression: { grep: undefined } });

export function parseOptions(args) {
    const options = { mode: args[0], suite: "smoke", headed: true, artifacts: null, artifactParent: null, help: false };
    if (!["verify", "test"].includes(options.mode)) throw new Error("Use npm run verify:gui or npm run test:gui");
    for (let index = 1; index < args.length; index++) {
        const arg = args[index];
        if (arg === "--help") options.help = true;
        else if (arg === "--headed") options.headed = true;
        else if (arg === "--headless") options.headed = false;
        else if (arg === "--suite" || arg === "--artifacts" || arg === "--artifact-parent") {
            const value = args[++index];
            if (!value || value.startsWith("--")) throw new Error(`${arg} needs a value`);
            options[arg === "--suite" ? "suite" : arg === "--artifacts" ? "artifacts" : "artifactParent"] = value;
        } else throw new Error(`Unknown option: ${arg}`);
    }
    if (!Object.hasOwn(GUI_SUITES, options.suite)) throw new Error("--suite must be smoke or regression");
    if (options.artifacts && !path.isAbsolute(options.artifacts)) throw new Error("--artifacts must be a new absolute directory");
    if (options.artifactParent && !path.isAbsolute(options.artifactParent)) throw new Error("--artifact-parent must be absolute");
    if (options.artifacts && options.artifactParent) throw new Error("Choose --artifacts or --artifact-parent");
    return options;
}

export function stagesFor(mode) {
    if (mode === "verify") return [...stageNames];
    if (mode === "test") return ["browser-e2e"];
    throw new Error("Unknown GUI stage mode");
}

/** Run existing commands in order. A failed prerequisite never admits the GUI phase. */
export async function runStages(names, execute, record = async () => {}) {
    const stages = [];
    let failed = false;
    for (const name of names) {
        const started = Date.now();
        const result = failed ? { status: "not_run", exit_code: null, elapsed_ms: 0, error_code: "prerequisite_failed" }
            : await execute(name);
        const stage = { name, elapsed_ms: Date.now() - started, ...result };
        stages.push(stage);
        failed ||= stage.status !== "passed";
        await record(stages);
    }
    return stages;
}

export function cargoExecutable(output) {
    const binaries = new Set();
    for (const line of output.split(/\r?\n/)) {
        let message;
        try { message = JSON.parse(line); } catch { continue; }
        if (message.reason === "compiler-artifact" && message.target?.name === "moyai-hub"
            && message.target.kind?.includes("bin") && typeof message.executable === "string") binaries.add(message.executable);
    }
    if (binaries.size !== 1 || !path.isAbsolute([...binaries][0])) throw new Error("Cargo must report exactly one moyai-hub executable");
    return [...binaries][0];
}

export function unitCounts(output) {
    const counts = {};
    for (const [field, label] of [["total", "tests"], ["passed", "pass"], ["failed", "fail"], ["skipped", "skipped"]]) {
        const matches = [...output.matchAll(new RegExp(`^# ${label} (\\d+)\\s*$`, "gm"))];
        if (!matches.length) return null;
        counts[field] = Number(matches.at(-1)[1]);
    }
    return counts;
}

export function browserCounts(report, suite) {
    const stats = report?.stats;
    if (!stats || ["expected", "unexpected", "flaky", "skipped"].some(key => !Number.isInteger(stats[key]) || stats[key] < 0)) {
        throw new Error("Invalid Playwright report counts");
    }
    const counts = { passed: stats.expected, failed: stats.unexpected, flaky: stats.flaky, skipped: stats.skipped };
    counts.total = counts.passed + counts.failed + counts.flaky + counts.skipped;
    const cases = [];
    const visit = groups => { for (const group of groups ?? []) {
        for (const spec of group.specs ?? []) for (const result of spec.tests ?? []) cases.push({
            // No titles, paths, errors, DOM, stdout or attachments enter the public summary.
            case_id: /^[a-f0-9-]+$/i.test(spec.id ?? "") ? spec.id : null,
            status: ["expected", "unexpected", "flaky", "skipped"].includes(result.status) ? result.status : "unknown",
            elapsed_ms: (result.results ?? []).reduce((sum, item) => sum + (Number.isFinite(item.duration) ? item.duration : 0), 0),
        });
        visit(group.suites);
    } };
    visit(report.suites);
    const selectionMatches = counts.total > 0 && cases.length === counts.total
        && (GUI_SUITES[suite].expected === undefined || counts.total === GUI_SUITES[suite].expected);
    return { counts, cases, passed: selectionMatches && counts.failed === 0 && counts.flaky === 0 && counts.skipped === 0
        && !report.errors?.length && cases.every(item => item.status === "expected") };
}

/** Strict upload allowlist. Raw command / Playwright reports stay local. */
export function publicSummary({ mode, suite, headed, stages = [], binarySha256 = null, browser = null }) {
    const statuses = new Set(["passed", "failed", "not_run"]);
    const errorCodes = new Set(["prerequisite_failed", "command_failed", "stage_exception", "browser_report_failed"]);
    const countKeys = ["total", "passed", "failed", "skipped", "flaky"];
    const counts = value => value === null || value === undefined ? null : Object.fromEntries(countKeys
        .filter(key => Number.isInteger(value[key]) && value[key] >= 0).map(key => [key, value[key]]));
    const expectedStages = ["verify", "test"].includes(mode) ? stagesFor(mode) : [];
    const complete = expectedStages.length > 0 && stages.length === expectedStages.length
        && stages.every((stage, index) => stage.name === expectedStages[index] && stage.status === "passed")
        && browser?.passed === true;
    return {
        schema_version: 1, product: "moyai-hub", mode: ["verify", "test"].includes(mode) ? mode : "unknown",
        suite: Object.hasOwn(GUI_SUITES, suite) ? suite : "unknown", browser_mode: headed === true ? "headed" : "headless",
        build_source: mode === "verify" ? (stages.some(stage => stage.name === "rust-build" && stage.status === "passed")
            ? "built_in_this_run" : "build_not_completed") : "prebuilt_source_match_not_checked",
        status: complete ? "passed" : stages.some(stage => stage.status !== "passed") ? "failed" : "incomplete",
        visual_review: "not_performed_by_runner",
        binary_sha256: /^[a-f0-9]{64}$/i.test(binarySha256 ?? "") ? binarySha256 : null,
        stages: stages.filter(stage => stageNames.includes(stage.name)).map(stage => ({
            name: stage.name, status: statuses.has(stage.status) ? stage.status : "failed",
            exit_code: Number.isInteger(stage.exit_code) ? stage.exit_code : null,
            elapsed_ms: Number.isFinite(stage.elapsed_ms) && stage.elapsed_ms >= 0 ? stage.elapsed_ms : null,
            error_code: errorCodes.has(stage.error_code) ? stage.error_code : null,
            unit_counts: counts(stage.unit_counts),
        })),
        gui_counts: counts(browser?.counts),
        cases: (browser?.cases ?? []).map(item => ({
            case_id: /^[a-f0-9-]+$/i.test(item.case_id ?? "") ? item.case_id : null,
            status: ["expected", "unexpected", "flaky", "skipped"].includes(item.status) ? item.status : "unknown",
            elapsed_ms: Number.isFinite(item.elapsed_ms) && item.elapsed_ms >= 0 ? item.elapsed_ms : null,
        })),
    };
}

async function hashFile(binary) {
    const hash = createHash("sha256");
    for await (const bytes of createReadStream(binary)) hash.update(bytes);
    return hash.digest("hex");
}

async function command(file, args, { env, logFile }) {
    const log = createWriteStream(logFile, { flags: "wx" });
    let stdout = "";
    const child = spawn(file, args, { cwd: repository, env, windowsHide: true, stdio: ["ignore", "pipe", "pipe"] });
    const interrupt = signal => child.kill(signal);
    const sigint = () => interrupt("SIGINT"), sigterm = () => interrupt("SIGTERM");
    process.on("SIGINT", sigint); process.on("SIGTERM", sigterm);
    // CI step logs are shared too: raw failure/DOM/fixture output belongs only in local files.
    child.stdout.on("data", bytes => { stdout += bytes.toString(); log.write(bytes); });
    child.stderr.on("data", bytes => { log.write(bytes); });
    try {
        const exit_code = await new Promise((resolve, reject) => { child.once("error", reject); child.once("close", resolve); });
        return { exit_code, stdout };
    } finally {
        process.off("SIGINT", sigint); process.off("SIGTERM", sigterm);
        await new Promise(resolve => log.end(resolve));
    }
}

export async function main(args = process.argv.slice(2)) {
    const options = parseOptions(args);
    if (options.help) {
        console.log("npm run verify:gui -- [--suite smoke|regression] [--headed|--headless] [--artifact-parent <absolute parent> | --artifacts <new absolute directory>]\n"
            + "npm run test:gui -- [same options]\nverify: unit -> frontend build -> Rust binary build -> browser E2E. test: prebuilt E2E only; source match is NOT checked.\n"
            + "Default: smoke, headed. Browser channel: MOYAI_HUB_BROWSER_CHANNEL (Windows: msedge). No browser is installed automatically.");
        return 0;
    }
    const artifacts = options.artifacts ?? path.resolve(options.artifactParent ?? path.resolve(repository, "../project_sandbox/hub-gui"), `${Date.now()}-${randomUUID()}`);
    await mkdir(path.dirname(artifacts), { recursive: true });
    await mkdir(artifacts, { recursive: false });
    await mkdir(path.join(artifacts, "logs"));
    let binary, binarySha256 = null, browser = null;
    const raw = new Map();
    const report = async stages => {
        const summary = publicSummary({ ...options, stages, binarySha256, browser });
        await writeFile(path.join(artifacts, "public-summary.json"), JSON.stringify(summary, null, 2));
        await writeFile(path.join(artifacts, "stage-summary.json"), JSON.stringify({ ...summary,
            artifacts, binary: binary ?? null, logs: "logs/", browser_report: "browser/report/index.html" }, null, 2));
        const last = summary.stages.at(-1);
        if (last) console.log(`${last.name}: ${last.status}; exit=${last.exit_code}; elapsed_ms=${last.elapsed_ms}`
            + (last.unit_counts ? `; unit=${JSON.stringify(last.unit_counts)}` : "")
            + (last.name === "browser-e2e" && summary.gui_counts ? `; gui=${JSON.stringify(summary.gui_counts)}` : ""));
    };
    console.log(`Hub GUI ${options.mode} / ${options.suite} / ${options.headed ? "headed" : "headless"}\nEvidence: ${artifacts}`);
    const stages = await runStages(stagesFor(options.mode), async name => {
        try {
            let file = process.execPath, parameters, env = { ...process.env };
            if (name === "unit" || name === "frontend-build") {
                if (!process.env.npm_execpath) throw new Error("Run this entry through npm so its current npm executable is reused");
                parameters = [process.env.npm_execpath, "run", name === "unit" ? "test" : "build"];
            } else if (name === "rust-build") {
                file = "cargo";
                parameters = ["build", "--bin", "moyai-hub", "--message-format=json-render-diagnostics"];
            } else {
                binary = options.mode === "verify" ? cargoExecutable(raw.get("rust-build").stdout)
                    : process.env.MOYAI_HUB_TEST_BINARY ?? path.join(repository, "target/debug", process.platform === "win32" ? "moyai-hub.exe" : "moyai-hub");
                if (!path.isAbsolute(binary)) throw new Error("MOYAI_HUB_TEST_BINARY must be absolute");
                await access(binary);
                binarySha256 = await hashFile(binary);
                env = { ...env, MOYAI_HUB_TEST_BINARY: binary, MOYAI_HUB_BROWSER_SUITE: options.suite,
                    MOYAI_HUB_BROWSER_ARTIFACTS: path.join(artifacts, "browser") };
                parameters = [path.join(repository, "node_modules/@playwright/test/cli.js"), "test", ...(options.headed ? ["--headed"] : [])];
            }
            const result = await command(file, parameters, { env, logFile: path.join(artifacts, "logs", `${name}.log`) });
            raw.set(name, result);
            let passed = result.exit_code === 0, error_code = passed ? null : "command_failed";
            if (name === "browser-e2e") {
                browser = browserCounts(JSON.parse(await readFile(path.join(artifacts, "browser/playwright-results.json"), "utf8")), options.suite);
                if (!browser.passed) { passed = false; error_code = "browser_report_failed"; }
            }
            return { status: passed ? "passed" : "failed", exit_code: result.exit_code, error_code,
                ...(name === "unit" ? { unit_counts: unitCounts(result.stdout) } : {}) };
        } catch (error) {
            await writeFile(path.join(artifacts, "logs", `${name}-error.txt`), String(error.stack ?? error));
            console.error(`${name}: failed (details in local logs)`);
            return { status: "failed", exit_code: raw.get(name)?.exit_code ?? null, error_code: "stage_exception" };
        }
    }, report);
    console.log(`Unit/build and GUI results: ${path.join(artifacts, "stage-summary.json")}\nUpload allowlist: public-summary.json only; raw logs/reports may contain fixture secrets.`);
    return stages.every(stage => stage.status === "passed") ? 0 : 1;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
    main().then(code => { process.exitCode = code; }, error => { console.error(error.message); process.exitCode = 1; });
}
