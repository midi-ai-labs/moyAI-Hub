import assert from "node:assert/strict";
import test from "node:test";
import path from "node:path";
import { parseOptions, stagesFor, runStages, cargoExecutable, unitCounts, browserCounts, publicSummary } from "../scripts/gui-stage.mjs";

const browserReport = (statuses = ["expected", "expected", "expected"]) => ({
    stats: Object.fromEntries(["expected", "unexpected", "skipped", "flaky"].map(key => [key, statuses.filter(value => value === key).length])),
    errors: [], suites: [{ specs: statuses.map((status, index) => ({ id: `ab-${index}`, title: "Private fixture title", tests: [
        { status, results: [{ duration: 12, attachments: [{ body: "PRIVATE KEY" }], stdout: ["token"] }] },
    ] })) }],
});

test("GUI entry defaults to visible smoke, allows explicit regression and rejects ambiguous selection", () => {
    assert.deepEqual(parseOptions(["verify"]), { mode: "verify", suite: "smoke", headed: true, artifacts: null, artifactParent: null, help: false });
    assert.equal(parseOptions(["test", "--suite", "regression", "--headless"]).headed, false);
    assert.equal(parseOptions(["test", "--artifacts", path.resolve("fresh-output")]).suite, "smoke");
    assert.equal(parseOptions(["test", "--artifact-parent", path.resolve("shared-parent")]).artifactParent, path.resolve("shared-parent"));
    for (const args of [["wrong"], ["test", "--suite"], ["test", "--suite", "all"], ["test", "--grep", "x"], ["test", "--artifacts", "relative"]]) {
        assert.throws(() => parseOptions(args));
    }
    assert.throws(() => parseOptions(["test", "--artifacts", path.resolve("a"), "--artifact-parent", path.resolve("b")]));
});

test("verify runs prerequisites in order; prebuilt entry runs only browser E2E", async () => {
    const seen = [];
    const result = await runStages(stagesFor("verify"), async name => { seen.push(name); return { status: "passed", exit_code: 0 }; });
    assert.deepEqual(seen, ["unit", "frontend-build", "rust-build", "browser-e2e"]);
    assert.equal(result.every(stage => stage.status === "passed"), true);
    assert.deepEqual(stagesFor("test"), ["browser-e2e"]);
});

test("a failed unit or build cannot run the next phase or report unexecuted GUI as passed", async () => {
    for (const failure of ["unit", "frontend-build", "rust-build"]) {
        const seen = [];
        const stages = await runStages(stagesFor("verify"), async name => {
            seen.push(name); return { status: name === failure ? "failed" : "passed", exit_code: name === failure ? 7 : 0 };
        });
        assert.equal(seen.at(-1), failure);
        assert.equal(stages.at(-1).status, "not_run");
        assert.equal(publicSummary({ mode: "verify", suite: "smoke", stages }).status, "failed");
    }
});

test("Cargo artifact identity selects the exact binary, including a fresh cached build, without guessing target paths", () => {
    const executable = path.resolve("custom-target/debug/moyai-hub.exe");
    const message = { reason: "compiler-artifact", fresh: true, target: { name: "moyai-hub", kind: ["bin"] }, executable };
    assert.equal(cargoExecutable(`compiler text\n${JSON.stringify(message)}\n{"reason":"build-finished","success":true}`), executable);
    assert.throws(() => cargoExecutable(JSON.stringify({ ...message, target: { name: "other", kind: ["bin"] } })));
    assert.throws(() => cargoExecutable(`${JSON.stringify(message)}\n${JSON.stringify({ ...message, executable: path.resolve("other.exe") })}`));
});

test("unit footer counts and browser case counts remain separate", () => {
    assert.deepEqual(unitCounts("# tests 44\n# pass 43\n# fail 1\n# skipped 0\n"), { total: 44, passed: 43, failed: 1, skipped: 0 });
    assert.equal(unitCounts("unstructured report"), null);
    const browser = browserCounts(browserReport(), "smoke");
    assert.equal(browser.passed, true);
    assert.deepEqual(browser.counts, { total: 3, passed: 3, failed: 0, skipped: 0, flaky: 0 });
});

test("empty, skipped, flaky, failed, partial smoke selection and global Playwright errors never pass GUI", () => {
    for (const statuses of [[], ["expected"], ["expected", "skipped", "expected"], ["expected", "flaky", "expected"], ["unexpected", "expected", "expected"]]) {
        assert.equal(browserCounts(browserReport(statuses), "smoke").passed, false);
    }
    assert.equal(browserCounts(browserReport([]), "regression").passed, false);
    assert.equal(browserCounts({ ...browserReport(), errors: [{ message: "teardown failure" }] }, "smoke").passed, false);
    const report = browserReport(); report.suites = [];
    assert.equal(browserCounts(report, "smoke").passed, false);
    assert.throws(() => browserCounts({ stats: { expected: 3 } }, "smoke"));
});

test("public summary strips fixture secrets, arbitrary metadata and raw errors while marking visual review separately", () => {
    const stages = [{ name: "browser-e2e", status: "passed", exit_code: 0, elapsed_ms: 3,
        token: "secret-token", error_code: "secret-error", stdout: "PRIVATE KEY", private_path: "private-user" }];
    const browser = browserCounts(browserReport(), "smoke");
    const summary = publicSummary({ mode: "test", suite: "smoke", stages, browser, binarySha256: "a".repeat(64), secret: "token" });
    const serialized = JSON.stringify(summary);
    for (const forbidden of ["PRIVATE KEY", "secret", "Private fixture", "private-user", "stdout", "attachments"]) assert.equal(serialized.includes(forbidden), false);
    assert.equal(summary.build_source, "prebuilt_source_match_not_checked");
    assert.equal(summary.visual_review, "not_performed_by_runner");
    assert.equal(summary.binary_sha256, "a".repeat(64));
    assert.equal(summary.gui_counts.total, 3);
    assert.equal(summary.stages[0].unit_counts, null);
    assert.equal(summary.cases.length, 3);
});

test("interrupted phase reporting never publishes a partial build or missing GUI as overall passed", () => {
    const passed = stagesFor("verify").map(name => ({ name, status: "passed", exit_code: 0 }));
    const browser = browserCounts(browserReport(), "smoke");
    for (let length = 0; length < passed.length; length++) {
        assert.equal(publicSummary({ mode: "verify", suite: "smoke", stages: passed.slice(0, length), browser }).status, "incomplete");
    }
    assert.equal(publicSummary({ mode: "verify", suite: "smoke", stages: passed }).status, "incomplete");
    assert.equal(publicSummary({ mode: "verify", suite: "smoke", stages: passed.toReversed(), browser }).status, "incomplete");
    assert.equal(publicSummary({ mode: "verify", suite: "smoke", stages: passed, browser }).status, "passed");
});
