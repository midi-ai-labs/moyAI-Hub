import { createServer } from "node:http";

/** A loopback metadata provider, not a mock of Hub's management API. */
export async function createMetadataProvider() {
    let models = ["fixture-alpha", "fixture-beta"];
    let nativeModels = ["fixture-native"];
    let apiKey = null;
    let nextHold = null;
    const held = new Set();
    const requests = [];
    const server = createServer(async (request, response) => {
        const authorized = apiKey === null || request.headers.authorization === `Bearer ${apiKey}`;
        requests.push({ method: request.method, path: request.url, authorized });
        if (request.method !== "GET" || !["/v1/models", "/api/v1/models"].includes(request.url)) {
            response.writeHead(404).end(); return;
        }
        if (!authorized) {
            response.writeHead(401, { "Content-Type": "application/json" }).end(JSON.stringify({ error: "API key required" })); return;
        }
        const body = request.url === "/v1/models"
            ? { data: models.map(id => ({ id, object: "model", owned_by: "browser-test-fixture" })) }
            : { models: nativeModels.map(key => ({ key, type: "llm", loaded_instances: [] })) };
        if (nextHold) {
            const hold = nextHold; nextHold = null; held.add(hold); hold.received();
            await hold.promise; held.delete(hold);
        }
        if (!response.destroyed) response.writeHead(200, { "Content-Type": "application/json" }).end(JSON.stringify(body));
    });
    await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
    return {
        url: `http://127.0.0.1:${server.address().port}`,
        requests,
        setModels(value) { models = [...value]; },
        setNativeModels(value) { nativeModels = [...value]; },
        setApiKey(value) { apiKey = value; },
        holdNext() {
            if (nextHold) throw new Error("Only one scheduled metadata hold is supported");
            let release, received;
            const promise = new Promise(resolve => { release = resolve; });
            const arrival = new Promise(resolve => { received = resolve; });
            nextHold = { promise, release, received };
            return { release, arrival };
        },
        async close() {
            nextHold?.release();
            for (const hold of held) hold.release();
            server.closeAllConnections();
            await new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
        },
    };
}
