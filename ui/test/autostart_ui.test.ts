import assert from "node:assert/strict";
import test from "node:test";
import { setImmediate } from "node:timers/promises";
import { attachAutostart } from "../src/autostart.ts";
import { invoke, onAuthenticationChange } from "../src/management.ts";

class NodeFixture {
  textContent = ""; value = ""; disabled = false;
  nodes = new Map<string, NodeFixture>();
  set innerHTML(value: string) {
    for (const match of value.matchAll(/<[^>]+\bid="([^"]+)"[^>]*>/g)) {
      const child = new NodeFixture(); child.disabled = /\sdisabled(?:\s|>)/.test(match[0]); this.nodes.set(match[1],child);
    }
  }
  querySelector(selector: string) { return this.nodes.get(selector.slice(1)); }
  addEventListener() {}
}

test("remote management keeps its authenticated session and never queries local logon registration", async () => {
  const globals = ["fetch", "document", "window"] as const;
  const originals = globals.map(key=>Object.getOwnPropertyDescriptor(globalThis,key));
  const calls:string[]=[]; let detail:NodeFixture|null=null, invalidated=false;
  const stop = onAuthenticationChange(session=>{if(!session?.authentication.user) invalidated=true;});
  const values = [async (url:string, options?:RequestInit)=>{
    if(url==="/admin/session") return Response.json({csrf:"csrf",epoch:"epoch",authentication:{configured:true,bootstrap_allowed:false,login_allowed:true,session_id:"session",user:{user_id:"admin",display_name:"Admin",administrator:true}}});
    const {command}=JSON.parse(String(options?.body)); calls.push(command);
    if(command.startsWith("hub_autostart")) return Response.json({error:"forbidden"},{status:403});
    if(command==="hub_web_status") return Response.json({result:{local_url:"http://127.0.0.1:9080/admin/"}});
    return Response.json({result:{ready:true}});
  }, {createElement:()=>new NodeFixture()}, {location:{origin:"https://team.example:9081"}}];
  globals.forEach((key,index)=>Object.defineProperty(globalThis,key,{configurable:true,writable:true,value:values[index]}));
  try {
    attachAutostart({append:(node:NodeFixture)=>{detail=node;}} as unknown as HTMLElement);
    await setImmediate();
    assert.deepEqual(calls,["hub_web_status"]);
    assert.match(detail!.nodes.get("hub-autostart-status")!.textContent,/Hub PC上のローカル管理画面/);
    assert.equal(detail!.nodes.get("hub-autostart-toggle")!.disabled,true);
    assert.deepEqual(await invoke("hub_snapshot"),{ready:true});
    assert.equal(invalidated,false);
  } finally {
    stop(); globals.forEach((key,index)=>{if(originals[index]) Object.defineProperty(globalThis,key,originals[index]!);else delete (globalThis as Record<string,unknown>)[key];});
  }
});
