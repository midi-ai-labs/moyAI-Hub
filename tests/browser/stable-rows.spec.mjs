import { readFile } from "node:fs/promises";
import { ModuleKind, ScriptTarget, transpileModule } from "typescript";
import { test, expect } from "@playwright/test";

// Real DOM regression for the shared fact-row reconciler; no Hub, provider,
// private application globals, or editable form state are involved.
test("Stable fact rows preserve focus when a preceding row disappears", async ({ page }) => {
    const source = transpileModule(await readFile(new URL("../../ui/src/stable_rows.ts", import.meta.url), "utf8"),
        { compilerOptions: { target: ScriptTarget.ES2022, module: ModuleKind.ES2022 } }).outputText;
    const result = await page.evaluate(async source => {
        const moduleUrl = URL.createObjectURL(new Blob([source], { type: "text/javascript" }));
        try {
            const { reconcileStableRows } = await import(moduleUrl);
            document.body.innerHTML = '<section id="facts" tabindex="-1"><article data-row="a"><button>仕事A</button></article><article data-row="b"><button>仕事B</button></article></section>';
            const owner = document.getElementById("facts");
            const focused = owner.querySelector('[data-row="b"] button');
            focused.focus();
            reconcileStableRows(owner, '<article data-row="b"><button>更新した仕事B</button></article>', owner,
                node => node.hasAttribute("data-row") ? node.getAttribute("data-row") : null);
            return { retained: owner.querySelector("button") === focused,
                focused: document.activeElement === focused, label: focused.textContent };
        } finally { URL.revokeObjectURL(moduleUrl); }
    }, source);
    expect(result).toEqual({ retained: true, focused: true, label: "更新した仕事B" });
});
