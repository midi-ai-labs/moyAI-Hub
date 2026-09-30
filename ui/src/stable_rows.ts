type RowKey = (node: Element) => string | null;
function key(node: Node, identify: RowKey): string | null {
    return node instanceof Element ? identify(node) : null;
}
function sameKind(current: Node, next: Node): boolean {
    return current.nodeType === next.nodeType && (!(current instanceof Element) || next instanceof Element && current.tagName === next.tagName);
}
function sync(current: Node, next: Node, identify: RowKey): void {
    if (!(current instanceof Element) || !(next instanceof Element)) {
        if (current.nodeValue !== next.nodeValue) current.nodeValue = next.nodeValue;
        return;
    }
    for (const attribute of Array.from(current.attributes)) {
        if (current instanceof HTMLDetailsElement && attribute.name === "open") continue;
        if (!next.hasAttribute(attribute.name)) current.removeAttribute(attribute.name);
    }
    for (const attribute of Array.from(next.attributes)) {
        if (current instanceof HTMLDetailsElement && attribute.name === "open") continue;
        if (current.getAttribute(attribute.name) !== attribute.value) current.setAttribute(attribute.name, attribute.value);
    }
    syncChildren(current, next, identify);
}
function syncChildren(current: Element, next: Element, identify: RowKey): void {
    for (const [index, desired] of Array.from(next.childNodes).entries()) {
        const desiredKey = key(desired, identify);
        const remaining = Array.from(current.childNodes).slice(index);
        const retained = remaining.find(candidate => sameKind(candidate, desired) && key(candidate, identify) === desiredKey);
        const node = retained ?? desired.cloneNode(true);
        if (current.childNodes[index] !== node) current.insertBefore(node, current.childNodes[index] ?? null);
        if (retained) sync(retained, desired, identify);
    }
    while (current.childNodes.length > next.childNodes.length) current.lastChild!.remove();
}

/** Reconcile server-owned facts/actions, never editable form fields or drafts. */
export function reconcileStableRows(owner: HTMLElement, html: string, fallbackFocus: HTMLElement, identify: RowKey): void {
    const desired = owner.cloneNode(false) as HTMLElement;
    desired.innerHTML = html;
    const focused = document.activeElement;
    const ownedFocus = focused !== null && owner.contains(focused);
    syncChildren(owner, desired, identify);
    if (ownedFocus && focused instanceof HTMLElement) {
        // Moving a retained row with insertBefore can blur its descendants.
        // Restore that exact action, never a replacement with a different key.
        const target = owner.contains(focused) && !focused.matches(":disabled") ? focused : fallbackFocus;
        if (document.activeElement !== target) target.focus({ preventScroll: true });
    }
}
