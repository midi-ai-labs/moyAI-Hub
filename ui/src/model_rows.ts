/** Keep row actions mounted while passive host facts change. */
export function reconcileModelRows(owner: HTMLTableSectionElement, html: string, fallbackFocus: HTMLElement): void {
    const desired = document.createElement("tbody");
    desired.innerHTML = html;
    const existing = new Map(Array.from(owner.rows).map(row => [row.dataset.id, row]));
    const retained = new Set<string | undefined>();

    for (const [index, next] of Array.from(desired.rows).entries()) {
        const id = next.dataset.id;
        const current = existing.get(id);
        retained.add(id);
        if (!current) {
            owner.insertBefore(next, owner.rows[index] ?? null);
            continue;
        }
        for (let cell = 0; cell < next.cells.length; cell++) {
            const oldCell = current.cells[cell];
            const nextCell = next.cells[cell];
            const action = oldCell.querySelector<HTMLButtonElement>("[data-edit]");
            const nextAction = nextCell.querySelector<HTMLButtonElement>("[data-edit]");
            if (action && nextAction) {
                action.setAttribute("aria-label", nextAction.getAttribute("aria-label") ?? "編集");
            } else if (oldCell.innerHTML !== nextCell.innerHTML) {
                oldCell.innerHTML = nextCell.innerHTML;
            }
        }
        if (owner.rows[index] !== current) {
            owner.insertBefore(current, owner.rows[index] ?? null);
        }
    }
    for (const [id, row] of existing) {
        if (!retained.has(id)) {
            if (row.contains(document.activeElement)) {
                fallbackFocus.focus({ preventScroll: true });
            }
            row.remove();
        }
    }
}
