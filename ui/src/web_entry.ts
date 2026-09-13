import { attachWebAuthentication } from "./web_auth.ts";
import "./style.css";
import "./web_auth.css";

const gate = document.getElementById("web-authentication")!;
const management = document.getElementById("management-app")!;
let managementLoaded = false;
const authentication = attachWebAuthentication(gate, management, async () => {
    if (managementLoaded) {
        // A new authentication owner gets a fresh document, including all polling,
        // dialogs and acquired history. Never reveal the previous owner's DOM.
        window.location.reload();
        await new Promise<never>(() => {});
    }
    managementLoaded = true;
    await import("./main.ts");
});
void authentication.refresh();
window.addEventListener("pageshow", event => {
    if (event.persisted) window.location.reload();
});
