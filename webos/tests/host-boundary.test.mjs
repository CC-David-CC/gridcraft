/** Transport/authorization tests use a synthetic endpoint; native workbook fidelity is tested separately. */
import test from 'node:test';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {webcrypto} from 'node:crypto';
import vm from 'node:vm';
import {accountAutosaveRetry,createAccountAutosave as actualAccountAutosave} from '@webos/runtime/account-autosave';

class Element extends EventTarget {
  children=[];dataset={};
  append(...nodes){this.children.push(...nodes);}
  setAttribute(){} remove(){} showModal(){} close(){} focus(){}
  querySelector(){return null;}
}

async function fixture({write,resource=null,authorize:policy,autosaveOptions={}}={}){
  const origin='https://example.test',window=new EventTarget(),authority={},network={onLine:true};
  let frame,dirty=true,revision=1,next=0,nativeEligible=true,autosaveEnabled=true,nativePaused=false;const pending=new Map(),calls=[],writes=[];
  const state=()=>({hostDocumentVersion:1,hostOwnsDocuments:true,hostDocumentState:{active:'1',documents:[{id:'1',revision:String(revision),dirty,pendingSaves:pending.size,autosaveEligible:nativeEligible&&!nativePaused,autosaveEnabled}]}});
  const emit=value=>{const event=new Event('message');Object.defineProperties(event,{origin:{value:origin},source:{value:frame.contentWindow},data:{value}});window.dispatchEvent(event);};
  const endpoint={async request(method,args){
    calls.push({method,args});
    if(method==='ui.state')return state();
    assert.equal(method,'ui.documentHost');
    const {command,params={}}=args;
    if(command==='document.recoverySnapshot')return {version:1,revision:String(revision),data:btoa(JSON.stringify({workbook:{synthetic:true}}))};
    if(command==='file.new'||command==='file.open')return {id:1};
    if(command==='document.pendingSave')return {path:pending.get(params.ticket)?.path??null,document:1};
    if(command==='ui.hostSaveResult'){
      assert(pending.has(params.ticket));
      if(params.outcome==='unknown')throw Error('Unknown outcome remains pending');
      const old=pending.get(params.ticket);pending.delete(params.ticket);
      if(params.outcome==='saved')dirty=old.revision!==revision;
      return {saved:params.outcome==='saved',clean:!dirty};
    }
    if(command==='file.save'||command==='file.saveAs'){
      const ticket=++next;pending.set(ticket,{path:params.path,revision});
      queueMicrotask(()=>emit({type:'gridcraft:save',id:'gridcraft-save-'+ticket,documentId:'1',revision:String(revision),
        name:'Report.xlsx',bytes:new ArrayBuffer(1),saveAs:command==='file.saveAs',documentSave:true,operation:command}));
      return {pendingSave:ticket};
    }
    if(command==='ui.hostPause'){nativePaused=params.paused;queueMicrotask(()=>emit({type:'gridcraft:document-state',...state().hostDocumentState}));return {paused:nativePaused};}
    if(command==='test.autosave'){autosaveEnabled=params.enabled;return {};}
    if(command==='test.editor'){nativeEligible=false;dirty=true;revision++;return {};}
    if(command==='test.commit'){nativeEligible=true;dirty=true;revision++;return {};}
    if(command==='test.edit'){dirty=true;revision++;return {edited:true};}
    if(command==='ui.backstage')return {};
    return {};
  }};
  const document=Object.assign(new EventTarget(),{createElement(tag){const node=new Element();node.tagName=tag;if(tag==='iframe'){frame=node;node.contentDocument={querySelector(){return null;}};node.contentWindow={gridcraft:endpoint,focus(){},postMessage(){}};}return node;}});
  const context=vm.createContext({window,document,navigator:network,location:{origin,href:origin+'/'},URL,AbortController,DOMException,setTimeout,clearTimeout,
    structuredClone,Blob,ArrayBuffer,Uint8Array,TextEncoder,TextDecoder,btoa,queueMicrotask,crypto:webcrypto,
    MutationObserver:class{observe(){} disconnect(){}}});
  const module=new vm.SourceTextModule(await readFile(new URL('../index.mjs',import.meta.url),'utf8'),{context});
  await module.link(async name=>{
    if(name==='./style.css')return new vm.SyntheticModule([],()=>{},{context});
    if(name==='@webos/runtime/account-autosave')return new vm.SyntheticModule(['accountAutosaveRetry','createAccountAutosave'],function(){
      this.setExport('accountAutosaveRetry',accountAutosaveRetry);
      this.setExport('createAccountAutosave',options=>actualAccountAutosave({...options,...autosaveOptions}));
    },{context});
    return new vm.SourceTextModule(await readFile(new URL('../'+name.slice(2),import.meta.url),'utf8'),{context});
  });
  await module.evaluate();
  const view=module.namespace.mount({instanceId:'view',sessionId:'session',profileId:'account-A',url:'/gridcraft/index.html',authority,resource,
    authorize:async(candidate,target)=>policy?policy(candidate,target):candidate===authority,
    files:{check(){},async version(){return {sha256:'b'.repeat(64)};},async read(){return {sha256:'b'.repeat(64),data:new Uint8Array([1])};},
      async write(path,bytes,previous){writes.push({path,previous});return write?write(path,bytes,previous):{sha256:'a'.repeat(64)};}}});
  emit({type:'gridcraft:ready'});await view.ready;calls.length=0;
  const request=(command,args={},resourceId=view.fileBinding.snapshot().path||'draft:1')=>({instanceId:'view',sessionId:'session',documentId:'1',resourceId,command,args});
  return {view,calls,writes,authority,request,pending,endpoint,frame,emit,network};
}

test('caller and exact instance/session/resource authorization precede native dispatch',async t=>{
  const f=await fixture();t.after(()=>f.view.dispose());
  await assert.rejects(f.view.execute(f.request('test.edit')),/authorization/i);
  await assert.rejects(f.view.execute(f.request('test.edit'),{}),/denied/i);
  await assert.rejects(f.view.execute({...f.request('test.edit'),sessionId:'other'},f.authority),/target/i);
  await assert.rejects(f.view.execute(f.request('test.edit',{},'other.xlsx'),f.authority),/denied|resource/i);
  assert.equal(f.calls.length,0);
});

test('Save As adopts a confirmed binding and subsequent Save uses its server revision',async t=>{
  const f=await fixture();t.after(()=>f.view.dispose());
  const result=await f.view.execute(f.request('file.saveAs',{path:'Documents/Report.xlsx'}),f.authority);
  assert.equal(result.path,'Documents/Report.xlsx');assert.equal(f.view.fileBinding.snapshot().revision,'a'.repeat(64));
  await f.view.execute(f.request('file.save',{path:result.path}),f.authority);
  assert.deepEqual(f.writes,[{path:result.path,previous:null},{path:result.path,previous:'a'.repeat(64)}]);
  assert.equal(f.pending.size,0);
});

test('public document commands cannot impersonate a durable storage acknowledgement',async t=>{
  const f=await fixture();t.after(()=>f.view.dispose());
  for(const command of ['ui.hostSaveResult','document.pendingSave','document.openResult','ui.hostPause','document.restoreSnapshot']){
    await assert.rejects(f.view.execute(f.request(command,{ticket:1,outcome:'saved',path:'Documents/Fake.xlsx'}),f.authority),/Desktop/);
  }
  assert.equal(f.calls.length,0);assert.equal(f.writes.length,0);
});

test('name collision leaves the old identity and unsaved work intact',async t=>{
  const f=await fixture({write:async()=>{throw Object.assign(Error('Name collision'),{status:409});}});t.after(()=>f.view.dispose());
  await assert.rejects(f.view.execute(f.request('file.saveAs',{path:'Documents/Exists.xlsx'}),f.authority),/collision/);
  assert.equal(f.view.fileBinding.snapshot().path,null);assert.equal((await f.view.inspect()).dirty,true);
  assert.equal(f.pending.size,0);
});

test('unknown server outcome retains the native pending save and refuses a duplicate write',async t=>{
  const f=await fixture({write:async()=>{throw Object.assign(Error('Connection interrupted'),{outcome:'unknown'});}});t.after(()=>f.view.dispose());
  await assert.rejects(f.view.execute(f.request('file.saveAs',{path:'Documents/Maybe.xlsx'}),f.authority),/interrupted/);
  assert.equal(f.pending.size,1);assert.equal(f.view.fileBinding.snapshot().path,null);
  assert.equal(await f.view.requestClose(),false);
});

test('an edit during storage remains dirty after its earlier version is acknowledged',async t=>{
  let release,entered;const waiting=new Promise(resolve=>{entered=resolve;});
  const f=await fixture({write:async()=>{entered();return new Promise(resolve=>{release=()=>resolve({sha256:'a'.repeat(64)});});}});t.after(()=>f.view.dispose());
  const save=f.view.execute(f.request('file.saveAs',{path:'Documents/Report.xlsx'}),f.authority);await waiting;
  await f.endpoint.request('ui.documentHost',{command:'test.edit'});release();await save;
  assert.equal((await f.view.inspect()).dirty,true);assert.equal(f.view.fileBinding.snapshot().path,'Documents/Report.xlsx');
});

test('captured commands cannot be rebound while asynchronous authorization is pending',async t=>{
  let allow,entered;const gate=new Promise(resolve=>{entered=resolve;});
  const f=await fixture({authorize:async(candidate,target)=>{
    if(target.operation==='test.edit'){entered();return new Promise(resolve=>{allow=resolve;});}
    return true;
  }});t.after(()=>f.view.dispose());
  const request=f.request('test.edit',{value:'captured'});
  const edit=f.view.execute(request,f.authority);await gate;
  request.args.value='mutated';
  await f.view.execute(f.request('file.saveAs',{path:'Documents/Rebound.xlsx'}),f.authority);
  allow(true);
  await assert.rejects(edit,/resource changed/i);
  assert.equal(f.calls.filter(c=>c.args.command==='test.edit').length,0);
});

test('public scripting and foreign window operations are not implicitly enabled',async t=>{
  const f=await fixture();t.after(()=>f.view.dispose());
  for(const command of ['automate.runScript','python.execute','window.activate','view.newWindow']) {
    await assert.rejects(f.view.execute(f.request(command),f.authority),/unavailable|Desktop/i);
  }
  assert.equal(f.calls.length,0);
});

test('native pause lease freezes public edits until its release completes',async t=>{
  const f=await fixture();t.after(()=>f.view.dispose());
  const lease=await f.view.lifecycle.quiesce();
  await assert.rejects(f.view.execute(f.request('test.edit'),f.authority),/paused/i);
  assert.equal(f.calls.filter(c=>c.args.command==='test.edit').length,0);
  lease.release();await new Promise(resolve=>setTimeout(resolve,0));
  await f.view.execute(f.request('test.edit'),f.authority);
  const pauses=f.calls.filter(c=>c.args.command==='ui.hostPause');
  assert.equal(pauses.length,2);assert.equal(pauses[0].args.params.paused,true);assert.equal(pauses[1].args.params.paused,false);
  assert.equal(pauses[0].args.params.lease,pauses[1].args.params.lease);
});

test('an invalid durable receipt keeps the native save pending for reconciliation',async t=>{
  const f=await fixture({write:async()=>({sha256:'invalid-receipt'})});t.after(()=>f.view.dispose());
  await assert.rejects(f.view.execute(f.request('file.saveAs',{path:'Documents/Receipt.xlsx'}),f.authority),/confirmation is invalid/);
  assert.equal(f.pending.size,1);assert.equal(f.view.fileBinding.snapshot().path,null);
  assert.equal(await f.view.requestClose(),false);
});

test('native acknowledgement failure after server commit is an unresolved write',async t=>{
  const f=await fixture();t.after(()=>f.view.dispose());
  const request=f.endpoint.request.bind(f.endpoint);
  f.endpoint.request=async(method,args)=>{
    if(args?.command==='ui.hostSaveResult'&&args.params.outcome==='saved')throw Error('Native acknowledgement unavailable');
    return request(method,args);
  };
  await assert.rejects(f.view.execute(f.request('file.saveAs',{path:'Documents/Committed.xlsx'}),f.authority),/acknowledgement unavailable/);
  assert.equal(f.writes.length,1);assert.equal(f.pending.size,1);assert.equal(f.view.fileBinding.snapshot().path,null);
  assert.equal(await f.view.requestClose(),false);
});

function virtualClock(){
  let now=0,next=0;const timers=new Map();
  const options={now:()=>now,setTimer(fn,delay){const id=++next;timers.set(id,{fn,at:now+delay});return id;},clearTimer:id=>timers.delete(id),random:()=>0};
  const flush=async()=>{for(let i=0;i<3;i++)await new Promise(resolve=>setImmediate(resolve));};
  return {options,async advance(ms){const until=now+ms;for(let steps=0;steps<1000;steps++){
    const entry=[...timers].sort((a,b)=>a[1].at-b[1].at)[0];if(!entry||entry[1].at>until)break;
    now=entry[1].at;timers.delete(entry[0]);entry[1].fn();await flush();
  }now=until;await flush();}};
}
const accountResource={path:'Documents/Existing.xlsx',sha256:'b'.repeat(64)};

test('actual Runtime scheduler never names an untitled workbook or opens a timer dialog',async t=>{
  const clock=virtualClock(),f=await fixture({autosaveOptions:clock.options});t.after(()=>f.view.dispose());
  await clock.advance(120000);
  assert.equal(f.writes.length,0);assert.equal((await f.view.inspect()).autosave.state,'waiting');
  assert.equal(f.view.element.children.filter(node=>node.tagName==='dialog').length,0);
});

test('actual Runtime scheduler waits for committed typing and uses the existing conditional native Save',async t=>{
  const clock=virtualClock(),f=await fixture({resource:accountResource,autosaveOptions:clock.options});t.after(()=>f.view.dispose());
  await clock.advance(9999);assert.equal(f.writes.length,0);
  await clock.advance(1);assert.equal(f.writes.length,1);
  assert.deepEqual(f.writes[0],{path:accountResource.path,previous:accountResource.sha256});
  assert.equal(f.calls.find(c=>c.args.command==='file.save').args.params.background,true);
  await f.endpoint.request('ui.documentHost',{command:'test.editor'});await f.view.inspect();
  await clock.advance(120000);assert.equal(f.writes.length,1,'unfinished native typing remains local');
  await f.endpoint.request('ui.documentHost',{command:'test.commit'});await f.view.inspect();
  await clock.advance(10000);assert.equal(f.writes.length,2);
  assert.equal(f.calls.filter(c=>c.args.command==='ui.edit.commit').length,0);
});

test('actual Runtime retries only verified no-write failures and honors Retry-After',async t=>{
  const clock=virtualClock();let attempts=0;
  const f=await fixture({resource:accountResource,autosaveOptions:clock.options,write:async()=>{
    attempts++;if(attempts<3)throw Object.assign(Error('Transfer allowance reached; try later'),{status:429,retryAfterMs:attempts===1?75_000:undefined});
    return {sha256:'a'.repeat(64)};
  }});t.after(()=>f.view.dispose());
  await clock.advance(10000);assert.equal(attempts,1);assert.equal((await f.view.inspect()).autosave.state,'retrying');
  await clock.advance(74999);assert.equal(attempts,1);await clock.advance(1);assert.equal(attempts,2);
  await clock.advance(59999);assert.equal(attempts,2);await clock.advance(1);assert.equal(attempts,3);
  assert.equal((await f.view.inspect()).autosave.state,'idle');
});

test('timeouts, unexplained throttling and unknown writes pause actual Runtime AutoSave',async t=>{
  for(const error of [Object.assign(Error('File request timed out'),{status:408}),Object.assign(Error('Unexplained throttling'),{status:429}),
    Object.assign(Error('Transport interrupted'),{outcome:'unknown'})]){
    const clock=virtualClock(),f=await fixture({resource:accountResource,autosaveOptions:clock.options,write:async()=>{throw error;}});t.after(()=>f.view.dispose());
    await clock.advance(10000);assert.equal(f.writes.length,1);
    const autosave=(await f.view.inspect()).autosave;
    assert.equal(autosave.state,'paused');
    if(error.outcome==='unknown')assert.match(autosave.reason,/outcome is unknown; check the file before retrying/);
    await clock.advance(900000);assert.equal(f.writes.length,1);
  }
});

test('manual confirmed Save As resolves an AutoSave conflict and retains the write cooldown',async t=>{
  const clock=virtualClock();let attempts=0;
  const f=await fixture({resource:accountResource,autosaveOptions:clock.options,write:async()=>{
    if(++attempts===1)throw Object.assign(Error('File changed'),{status:409});return {sha256:'a'.repeat(64)};
  }});t.after(()=>f.view.dispose());
  await clock.advance(10000);assert.equal((await f.view.inspect()).autosave.state,'paused');
  await f.view.execute(f.request('file.saveAs',{path:'Documents/Resolved.xlsx'}),f.authority);
  await f.endpoint.request('ui.documentHost',{command:'test.edit'});await f.view.inspect();
  await clock.advance(59999);assert.equal(attempts,2);await clock.advance(1);assert.equal(attempts,3);
  assert.equal(f.writes.at(-1).path,'Documents/Resolved.xlsx');assert.equal(f.writes.at(-1).previous,'a'.repeat(64));
});

test('a reloaded iframe Document cannot receive an old durable save acknowledgement',async t=>{
  let entered,release;const started=new Promise(resolve=>{entered=resolve;});
  const f=await fixture({write:async()=>{entered();return new Promise(resolve=>{release=()=>resolve({sha256:'a'.repeat(64)});});}});t.after(()=>f.view.dispose());
  const pending=f.view.execute(f.request('file.saveAs',{path:'Documents/Reloaded.xlsx'}),f.authority);await started;
  f.frame.contentDocument={querySelector(){return null;}};release();
  await assert.rejects(pending,/reloaded unexpectedly/i);
  assert.equal(f.calls.filter(c=>c.args.command==='ui.hostSaveResult').length,0);
  const state=await f.view.inspect();assert.equal(state.phase,'unavailable');assert.equal(state.loaded,false);
  assert.equal(await f.view.requestClose(),true);await f.view.lifecycle.commitClose();
});

test('the native AutoSave preference controls only account scheduling and remains explicit',async t=>{
  const clock=virtualClock(),f=await fixture({resource:accountResource,autosaveOptions:clock.options});t.after(()=>f.view.dispose());
  await f.endpoint.request('ui.documentHost',{command:'test.autosave',params:{enabled:false}});let state=await f.view.inspect();
  assert.equal(state.documents[0].autosaveEnabled,false);assert.equal(state.documents[0].autosaveEligible,true);
  await clock.advance(120000);assert.equal(f.writes.length,0);
  await f.endpoint.request('ui.documentHost',{command:'test.autosave',params:{enabled:true}});await f.view.inspect();
  await clock.advance(10000);assert.equal(f.writes.length,1);
});

test('offline pre-admission retries without capturing or dispatching an account Save',async t=>{
  const clock=virtualClock(),f=await fixture({resource:accountResource,autosaveOptions:clock.options});t.after(()=>f.view.dispose());
  f.network.onLine=false;await clock.advance(10000);
  assert.equal(f.writes.length,0);assert.equal(f.calls.filter(c=>c.args.command==='file.save').length,0);
  assert.equal((await f.view.inspect()).autosave.state,'retrying');
  f.network.onLine=true;await clock.advance(30000);assert.equal(f.writes.length,1);
});

test('iframe reload rejects in-flight old-realm commands so the unavailable view remains closeable',async t=>{
  const f=await fixture();t.after(()=>f.view.dispose());
  const request=f.endpoint.request.bind(f.endpoint);let entered;const started=new Promise(resolve=>{entered=resolve;});
  f.endpoint.request=(method,args)=>args?.command==='test.long'?new Promise(()=>{entered();}):request(method,args);
  const command=f.view.execute(f.request('test.long'),f.authority);await started;
  f.frame.contentDocument={querySelector(){return null;}};f.frame.dispatchEvent(new Event('load'));
  await assert.rejects(command,/reloaded unexpectedly/i);
  assert.equal(await f.view.requestClose(),true);
});

test('a failed native pause acknowledgement releases its exact lease before reporting failure',async t=>{
  const f=await fixture();t.after(()=>f.view.dispose());const request=f.endpoint.request.bind(f.endpoint);const leases=[];
  f.endpoint.request=async(method,args)=>{
    if(args?.command==='ui.hostPause'){leases.push(args.params);if(args.params.paused)throw Error('Pause acknowledgement lost');}
    return request(method,args);
  };
  await assert.rejects(f.view.lifecycle.quiesce(),/acknowledgement lost/);
  assert.equal(leases.length,2);assert.equal(leases[0].paused,true);assert.equal(leases[1].paused,false);assert.equal(leases[0].lease,leases[1].lease);
  await f.view.execute(f.request('test.edit'),f.authority);
});

test('native pause eligibility changes do not invalidate a reviewed Discard close decision',async t=>{
  const f=await fixture();t.after(()=>f.view.dispose());
  const closing=f.view.requestClose();await new Promise(resolve=>setImmediate(resolve));
  const dialog=f.view.element.children.find(node=>node.tagName==='dialog');assert(dialog);
  const discard=dialog.children[2].children.find(node=>node.textContent==='Discard changes');assert(discard);
  discard.dispatchEvent(new Event('click'));assert.equal(await closing,true);
  await f.view.lifecycle.commitClose();
  assert.equal(f.calls.filter(c=>c.args.command==='ui.hostPause'&&c.args.params.paused).length,1);
});
