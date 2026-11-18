/** Recovery envelope/journal mechanics; these synthetic bytes do not qualify the native workbook codec. */
import test from 'node:test';
import assert from 'node:assert/strict';
import {captureDraft,createDraftJournal} from '../drafts.mjs';
const data=value=>btoa(JSON.stringify({workbook:{synthetic:value}}));
const draft=value=>({path:'Documents/Report.xlsx',revision:'a'.repeat(64),snapshot:{version:1,revision:'17',data:data(value)}});
function store(){const rows=new Map(),writes=[];return {rows,writes,
  async get(id){return structuredClone(rows.get(id));},
  async put(id,value){const generation=crypto.randomUUID();rows.set(id,{...structuredClone(value),id,generation});writes.push(value);return generation;},
  async remove(id,generation){if(generation&&rows.get(id)?.generation!==generation)return false;return rows.delete(id);}};}

test('only the native snapshot, account path and confirmed base enter recovery storage',()=>{
  const value=draft('synthetic');value.token='not-for-storage';value.snapshot.password='not-for-storage';
  assert.deepEqual(captureDraft(value),draft('synthetic'));
});
test('queued captures remain immutable and closing cannot resurrect an earlier write',async()=>{
  const s=store(),j=createDraftJournal({store:s}),value=draft('first');const first=j.write(value);value.snapshot.data=data('changed');
  await Promise.all([first,j.write(draft('second')),j.clear()]);
  assert.deepEqual(s.writes.map(v=>v.snapshot.data),[data('first'),data('second')]);assert.equal(s.rows.size,0);
});
test('later edits retain the new confirmed Save As identity in their recovery record',async()=>{
  const s=store(),j=createDraftJournal({store:s});await j.write(draft('old'));
  const next=draft('new');next.path='Documents/Copy.xlsx';next.revision='b'.repeat(64);
  await Promise.all([j.clear(),j.write(next)]);assert.deepEqual(s.writes.at(-1),next);assert.equal(s.rows.size,1);
});
test('a recovered writer cannot erase another views newer checkpoint',async()=>{
  const s=store(),id=crypto.randomUUID();await s.put(id,draft('old'));const j=createDraftJournal({store:s,sourceId:id});
  await j.load();await s.put(id,draft('other writer'));await j.write(draft('recovered'));
  assert.equal(s.rows.get(id).snapshot.data,data('other writer'));assert.equal(s.rows.size,2);
});
test('invalid resource pairs and unsupported snapshot versions are rejected',()=>{
  for(const bad of [null,{}, {...draft('x'),path:'../Report.xlsx'},{...draft('x'),revision:null},
    {...draft('x'),snapshot:{version:2,revision:'1',data:data('x')}},{...draft('x'),snapshot:{version:1,revision:'1',data:'arbitrary bytes'}}]){
    assert.throws(()=>captureDraft(bad),/Invalid/);
  }
  const unbound={path:null,revision:null,snapshot:{version:1,revision:'1',data:data('draft')}};
  assert.deepEqual(captureDraft(unbound),unbound);
});
