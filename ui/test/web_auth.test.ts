import test from "node:test";
import assert from "node:assert/strict";
import { consumeAccessTicket, authenticationError, attachWebAuthentication } from "../src/web_auth.ts";

test("launch ticket leaves the address immediately and only known startup notices remain", () => {
  const replacements: unknown[][] = [];
  const history = { replaceState: (...args: unknown[]) => { replacements.push(args); } };
  assert.equal(consumeAccessTicket({hash:"#access=one-use-secret&notice=hub-started",pathname:"/admin/",search:"?view=setup"}, history), "one-use-secret");
  assert.deepEqual(replacements, [[null,"","/admin/?view=setup#hub-started"]]);
  assert.equal(consumeAccessTicket({hash:"#access=&notice=unknown-secret",pathname:"/admin/",search:""}, history), "");
  assert.deepEqual(replacements[1], [null,"","/admin/"]);
  assert.equal(consumeAccessTicket({hash:"#models",pathname:"/admin/",search:""}, history), null);
  assert.equal(replacements.length,2);
});

test("expired access directs the user to reopen management without displaying a raw failure", () => {
  assert.match(authenticationError("invalid_ticket"), /使用済み、または期限切れ/);
  assert.match(authenticationError("invalid_ticket"), /開き直してください/);
  assert.doesNotMatch(authenticationError(new Error("access=confidential-value")), /confidential-value/);
});

test("a new fragment ticket opens the existing tab and supersedes an older pending exchange", async () => {
  class NodeFixture {
    hidden = false; inert = false; className = ""; disabled = false; textContent = "";
    nodes = new Map<string, NodeFixture>();
    set innerHTML(value: string) {
      this.nodes.clear();
      for (const match of value.matchAll(/\bid="([^"]+)"/g)) this.nodes.set(match[1],new NodeFixture());
    }
    querySelector(selector: string) { return this.nodes.get(selector.slice(1)); }
    querySelectorAll() { return []; }
    addEventListener() {}
  }
  const originalFetch = globalThis.fetch;
  const originalWindow = Object.getOwnPropertyDescriptor(globalThis,"window");
  const callbacks = new Map<string, () => void>();
  const location = {hash:"",pathname:"/admin/",search:""};
  const history = { replaceState: (_state: unknown, _unused: string, url: string) => { location.hash=new URL(url,"http://127.0.0.1").hash; } };
  let releaseOld!: (value: Response) => void;
  const oldResponse = new Promise<Response>(resolve => { releaseOld=resolve; });
  const response = (id: string | null) => ({csrf:"csrf",epoch:"epoch",authentication:{configured:true,bootstrap_allowed:false,login_allowed:true,session_id:id,user:id ? {user_id:id,display_name:id,administrator:true}:null}});
  const received: string[] = [];
  let starts=0;
  Object.defineProperty(globalThis,"window",{configurable:true,value:{location,history,addEventListener:(name:string,callback:()=>void)=>callbacks.set(name,callback)}});
  globalThis.fetch=async (path, options) => {
    if (path==="/admin/session") return Response.json(response(null));
    assert.equal(path,"/admin/access");
    assert.equal(location.hash, "", "the ticket is removed before the request");
    const {ticket}=JSON.parse(String(options?.body)); received.push(ticket);
    return ticket==="old" ? oldResponse : Response.json(response(ticket));
  };
  try {
    const root=new NodeFixture(), management=new NodeFixture();
    const ui=attachWebAuthentication(root as unknown as HTMLElement,management as unknown as HTMLElement,async()=>{++starts;});
    await ui.refresh(); assert.equal(management.hidden,true);
    location.hash="#access=old"; callbacks.get("hashchange")!();
    await new Promise(resolve=>setImmediate(resolve));
    location.hash="#access=new"; callbacks.get("hashchange")!();
    await new Promise(resolve=>setImmediate(resolve));
    assert.deepEqual(received,["old","new"]);
    assert.equal(management.hidden,false); assert.equal(starts,1);
    releaseOld(Response.json(response("old")));
    await new Promise(resolve=>setImmediate(resolve));
    assert.equal(management.hidden,false); assert.equal(starts,1);
    assert.equal(root.nodes.get("web-auth-user")?.textContent,"new");
  } finally {
    globalThis.fetch=originalFetch;
    if (originalWindow) Object.defineProperty(globalThis,"window",originalWindow);
    else delete (globalThis as Record<string, unknown>).window;
  }
});
