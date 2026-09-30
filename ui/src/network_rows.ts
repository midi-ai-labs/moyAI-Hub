import { reconcileStableRows } from "./stable_rows.ts";
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
export function reconcileNetworkRows(owner: HTMLElement, html: string, fallbackFocus: HTMLElement): void {
    reconcileStableRows(owner, html, fallbackFocus, key);
}
