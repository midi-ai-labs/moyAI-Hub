// Heartbeat facts change often. Keep disclosure, focus and action nodes mounted.
const keyAttributes = ["data-id", "data-device-identity", "data-network-edit", "data-network-request", "data-network-history", "data-network-delete-device", "data-network-action"];
function key(node: Node): string | null {
    if (!(node instanceof Element)) return null;
    for (const attribute of keyAttributes) {
        if (node.hasAttribute(attribute)) {
            return `${node.tagName}:${attribute}:${attribute === "data-network-action" ? "action" : node.getAttribute(attribute)}`;
        }
    }
    return null;
}
function sameKind(current: Node, next: Node): boolean {
    return current.nodeType === next.nodeType && (!(current instanceof Element) || next instanceof Element && current.tagName === next.tagName);
}
function sync(current: Node, next: Node): void {
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
    syncChildren(current, next);
}
function syncChildren(current: Element, next: Element): void {
    for (const [index, desired] of Array.from(next.childNodes).entries()) {
        const desiredKey = key(desired);
        const remaining = Array.from(current.childNodes).slice(index);
        const retained = remaining.find(candidate => sameKind(candidate, desired) && key(candidate) === desiredKey);
        const node = retained ?? desired.cloneNode(true);
        if (current.childNodes[index] !== node) current.insertBefore(node, current.childNodes[index] ?? null);
        if (retained) sync(retained, desired);
    }
    while (current.childNodes.length > next.childNodes.length) current.lastChild!.remove();
}
export function reconcileNetworkRows(owner: HTMLElement, html: string, fallbackFocus: HTMLElement): void {
    const desired = document.createElement("tbody");
    desired.innerHTML = html;
    const focused = document.activeElement;
    const ownedFocus = focused !== null && owner.contains(focused);
    syncChildren(owner, desired);
    if (ownedFocus && !owner.contains(focused)) fallbackFocus.focus({ preventScroll: true });
}
