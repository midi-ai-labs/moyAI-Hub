import assert from "node:assert/strict";
import test from "node:test";
import { attachFirewall, firewallScope, firewallSummary, firewallStatusMessage } from "../src/firewall.ts";
import { invoke } from "../src/management.ts";

class Element {
  textContent = ""; value = ""; checked = false; disabled = false; hidden = false;
  nodes = new Map<string, Element>(); listeners = new Map<string, (() => unknown)[]>();
  html = "";
  get innerHTML() { return this.html; }
  set innerHTML(value: string) {
    this.html = value;
    for (const match of value.matchAll(/<[^>]+\bid="([^"]+)"[^>]*>/g)) {
      const child = new Element(); child.disabled = /\sdisabled(?:\s|>)/.test(match[0]); child.hidden = /\shidden(?:\s|>)/.test(match[0]); this.nodes.set(match[1], child);
    }
  }
  querySelector(selector: string) { return this.nodes.get(selector.slice(1)); }
  addEventListener(name: string, listener: () => unknown) { this.listeners.set(name, [...(this.listeners.get(name) ?? []), listener]); }
  replaceChildren() { this.nodes.clear(); }
  async fire(name: string) { for (const listener of this.listeners.get(name) ?? []) await listener(); }
}
const rule = { name: "moyai-hub-inbound-v1-" + "a".repeat(24), program: "C:\\Hub\\moyai-hub.exe", local_address: "192.168.1.2", local_ports: [9471,9472], remote_addresses: ["192.168.1.0/24"], profile_mask: 2 };
const preview = () => ({spec:rule,binary_sha256:"a".repeat(64),previous:null as typeof rule|null,conflict:false,matching:false,fingerprint:"exact-preview"});
async function fixture(remote: boolean, operation: (command:string,args:Record<string,unknown>)=>unknown|Promise<unknown>, run: (host:Element,calls:string[],owner:ReturnType<typeof attachFirewall>)=>Promise<void>, onStatus: Parameters<typeof attachFirewall>[1] = undefined) {
  const oldFetch=globalThis.fetch, oldWindow=Object.getOwnPropertyDescriptor(globalThis,"window"); const calls:string[]=[];
  globalThis.fetch=async(url,options)=>{
    if(String(url)==="/admin/session") return Response.json({csrf:"csrf",epoch:"epoch",authentication:{configured:true,bootstrap_allowed:false,login_allowed:true,session_id:"session",user:{user_id:"admin",display_name:"Admin",administrator:true}}});
    const body=JSON.parse(String(options?.body)); calls.push(body.command);
    if(body.command==="hub_web_status") return Response.json({result:{local_url:"http://127.0.0.1:9080/admin/"}});
    return Response.json({result:await operation(body.command,body.args)});
  };
  Object.defineProperty(globalThis,"window",{configurable:true,value:{location:{origin:remote ? "https://hub.example:9081" : "http://127.0.0.1:9080"}}});
  const host=new Element();
  try { const owner = attachFirewall(host as unknown as HTMLElement, onStatus); await run(host,calls,owner); }
  finally { globalThis.fetch=oldFetch; if(oldWindow) Object.defineProperty(globalThis,"window",oldWindow);else delete (globalThis as Record<string,unknown>).window; }
}
test("scope has no automatic profile and request summary contains the exact restrictions",()=>{
  assert.deepEqual(firewallScope("192.168.1.4, 192.168.2.0/24",false,false),{remote_addresses:["192.168.1.4","192.168.2.0/24"],profiles:[]});
  assert.deepEqual(firewallScope("10.0.0.1",true,true).profiles,["Domain","Private"]);
  const summary=firewallSummary(rule); assert.match(summary,/9471, 9472/); assert.match(summary,/192.168.1.0\/24/); assert.match(summary,/Private/); assert.doesNotMatch(summary,/Public/);
});
test("remote browser does not invoke local firewall APIs or lose its management session",async()=>{
  await fixture(true,()=>({ready:true}),async(host,calls)=>{
    await host.nodes.get("hub-firewall-review")!.fire("click");
    assert.deepEqual(calls,["hub_web_status"]);
    assert.match(host.nodes.get("hub-firewall-status")!.textContent,/Hub PC上のローカル管理画面/);
    assert.equal(host.nodes.get("hub-firewall-apply")!.disabled,true);
    assert.deepEqual(await invoke("hub_snapshot"),{ready:true});
  });
});
test("apply requires review and confirmation; changing a draft discards the old preview",async()=>{
  await fixture(false,(command,args)=>{
    assert.equal(command,"hub_firewall_preview"); assert.deepEqual(args.scope,{remote_addresses:["192.168.1.0/24"],profiles:["Private"]}); return preview();
  },async(host)=>{
    const n=(id:string)=>host.nodes.get(`hub-firewall-${id}`)!;
    n("addresses").value="192.168.1.0/24"; n("private").checked=true;
    await n("review").fire("click"); assert.equal(n("apply").disabled,true);
    n("confirm").checked=true; await n("confirm").fire("change"); assert.equal(n("apply").disabled,false);
    await n("addresses").fire("input"); assert.equal(n("apply").disabled,true); assert.equal(n("preview").hidden,true); assert.equal(n("confirm").checked,false);
  });
});
test("unconfirmed OS outcome remains uncertain, resets confirmation and never retries",async()=>{
  let resolve:(value:unknown)=>void=()=>{};
  await fixture(false,(command,args)=>{
    if(command==="hub_firewall_preview") return preview();
    assert.equal(command,"hub_firewall_apply"); assert.equal(args.expectedFingerprint,"exact-preview");
    return new Promise(done=>{resolve=done;});
  },async(host,calls)=>{
    const n=(id:string)=>host.nodes.get(`hub-firewall-${id}`)!;
    n("addresses").value="192.168.1.0/24"; n("private").checked=true;
    await n("review").fire("click"); n("confirm").checked=true; await n("confirm").fire("change");
    await n("apply").fire("click"); await new Promise(done=>setImmediate(done));
    assert.equal(n("apply").disabled,true); assert.equal(n("review").disabled,true);
    resolve({status:"unconfirmed",preview:{...preview(),previous:rule,conflict:true}});
    await new Promise(done=>setImmediate(done));
    assert.match(n("status").textContent,/完了を確認できません/); assert.equal(n("confirm").checked,false);
    assert.equal(n("apply").disabled,true); assert.equal(n("remove").disabled,true);
    assert.equal(calls.filter(c=>c==="hub_firewall_apply").length,1);
  });
});

const health = () => ({state:"missing" as const,program:rule.program,local_address:rule.local_address,local_ports:rule.local_ports,current:null,other_programs:["C:\\Old\\moyai-hub.exe"],suggestion:{remote_addresses:rule.remote_addresses,profiles:["Private"]},suggestion_source:"lan" as const});
test("startup status reports relocated rules and suggestions require a separate explicit review",async()=>{
  await fixture(false,(command)=>{ assert.equal(command,"hub_firewall_status"); return health(); },async(host,calls,owner)=>{
    const n=(id:string)=>host.nodes.get(`hub-firewall-${id}`)!;
    owner.refresh("active-target",true); await new Promise(done=>setImmediate(done));
    assert.match(n("health").textContent,/別の設置先/);
    assert.equal(n("addresses").value,""); assert.equal(n("private").checked,false);
    await n("suggestion").fire("click");
    assert.equal(n("addresses").value,"192.168.1.0/24"); assert.equal(n("private").checked,true);
    assert.equal(n("apply").disabled,true); assert.equal(n("confirm").checked,false);
    owner.refresh("active-target",true); await new Promise(done=>setImmediate(done));
    assert.equal(calls.filter(name=>name==="hub_firewall_status").length,1);
    assert.equal(calls.some(name=>name==="hub_firewall_apply"),false);
  });
});
test("a late status cannot restore a stopped target or discard manually entered scope",async()=>{
  let resolve:(value:unknown)=>void=()=>{};
  await fixture(false,()=>new Promise(done=>{resolve=done;}),async(host,calls,owner)=>{
    const n=(id:string)=>host.nodes.get(`hub-firewall-${id}`)!;
    n("addresses").value="10.2.3.4";
    owner.refresh("active",true); await new Promise(done=>setImmediate(done));
    owner.refresh("stopped",false); resolve(health()); await new Promise(done=>setImmediate(done));
    assert.match(n("health").textContent,/Hubを開始すると/);
    assert.equal(n("suggestion").hidden,true); assert.equal(n("addresses").value,"10.2.3.4");
    assert.equal(calls.some(name=>name==="hub_firewall_apply"),false);
  });
});
test("matching OS rule never claims remote reachability",()=>{
  assert.match(firewallStatusMessage({...health(),state:"matching"}),/接続するDesktopで確認/);
  assert.match(firewallStatusMessage({...health(),state:"mismatch"}),/ポート/);
  assert.match(firewallStatusMessage({...health(),state:"conflict"}),/上書きせず/);
});
test("parent snapshot updates during a status callback cannot republish the previous target",async()=>{
  let owner: ReturnType<typeof attachFirewall>;
  await fixture(false,()=>health(),async(host,_calls,current)=>{
    owner = current; current.refresh("running",true); await new Promise(done=>setImmediate(done));
    assert.match(host.nodes.get("hub-firewall-health")!.textContent,/Hubを開始すると/);
    assert.equal(host.nodes.get("hub-firewall-suggestion")!.hidden,true);
  }, state => { if (state) owner.refresh("stopped",false); });
});
